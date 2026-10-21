"""Downloading a model's files: progress, a stop that can be resumed, and nothing kept that is not exact."""
import hashlib
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from cronista import downloads
from cronista.downloads import DownloadCancelled, DownloadFailed, RemoteFile, download, missing


@pytest.fixture
def server():
    """Serves `server.files` ({name: bytes}) over HTTP, honouring "Range: bytes=N-" as Hugging Face's
    storage does, and keeps each request's path and range."""
    state = type("Served", (), {"files": {}, "requests": []})()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):  # noqa: N802 - http.server's name
            name = self.path.lstrip("/")
            state.requests.append((name, self.headers.get("Range")))
            if name not in state.files:
                self.send_error(404)
                return
            body = state.files[name]
            start = int(self.headers["Range"].split("=")[1].rstrip("-")) if self.headers.get("Range") else 0
            self.send_response(206 if start else 200)
            if start:
                self.send_header("Content-Range", f"bytes {start}-{len(body) - 1}/{len(body)}")
            self.send_header("Content-Length", str(len(body) - start))
            self.end_headers()
            self.wfile.write(body[start:])

        def log_message(self, *args):
            pass

    httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    state.url = f"http://127.0.0.1:{httpd.server_address[1]}"
    yield state
    httpd.shutdown()


def remote(server, name: str, body: bytes, *, serve: bytes | None = None) -> RemoteFile:
    server.files[name] = body if serve is None else serve
    return RemoteFile(name, f"{server.url}/{name}", len(body), hashlib.sha256(body).hexdigest())


def test_the_files_arrive_checked_and_the_progress_is_told(server, tmp_path, monkeypatch):
    monkeypatch.setattr(downloads, "CHUNK", 64 * 1024)
    files = [remote(server, "config.json", b'{"features_size": 128}'),
             remote(server, "encoder.onnx", os.urandom(700_000))]
    seen = []
    download(tmp_path, files, progress=lambda done, total: seen.append((done, total)))
    assert all((tmp_path / f.name).read_bytes() == server.files[f.name] for f in files)
    assert sorted(p.name for p in tmp_path.iterdir()) == ["config.json", "encoder.onnx"]  # no partial file left
    total = sum(f.size for f in files)
    assert seen[-1] == (total, total) and [done for done, _ in seen] == sorted(done for done, _ in seen)
    assert missing(tmp_path, files) == []


def test_a_stopped_download_goes_on_from_where_it_stopped(server, tmp_path, monkeypatch):
    monkeypatch.setattr(downloads, "CHUNK", 64 * 1024)
    files = [remote(server, "encoder.onnx", os.urandom(700_000))]
    with pytest.raises(DownloadCancelled):
        download(tmp_path, files, progress=lambda done, total: None, stopped=lambda: len(server.requests) > 0)
    kept = (tmp_path / "encoder.onnx.part").stat().st_size
    assert 0 < kept < 700_000 and not (tmp_path / "encoder.onnx").exists()
    seen = []
    download(tmp_path, files, progress=lambda done, total: seen.append(done))
    assert server.requests[-1] == ("encoder.onnx", f"bytes={kept}-")
    assert seen[0] > kept - 1 and (tmp_path / "encoder.onnx").read_bytes() == server.files["encoder.onnx"]


def test_a_file_that_is_not_the_one_expected_is_thrown_away(server, tmp_path):
    files = [remote(server, "encoder.onnx", b"the right weights", serve=b"other weights!!!!")]
    with pytest.raises(DownloadFailed, match="not the expected"):
        download(tmp_path, files, progress=lambda done, total: None)
    assert list(tmp_path.iterdir()) == []


def test_a_dropped_connection_keeps_what_arrived_for_next_time(server, tmp_path, monkeypatch):
    monkeypatch.setattr(downloads, "CHUNK", 64 * 1024)
    body = os.urandom(700_000)
    files = [remote(server, "encoder.onnx", body, serve=body[:300_000])]  # the server stops early
    with pytest.raises(DownloadFailed, match="Check the connection"):
        download(tmp_path, files, progress=lambda done, total: None)
    assert (tmp_path / "encoder.onnx.part").stat().st_size == 300_000
    server.files["encoder.onnx"] = body
    download(tmp_path, files, progress=lambda done, total: None)
    assert server.requests[-1] == ("encoder.onnx", "bytes=300000-")
    assert (tmp_path / "encoder.onnx").read_bytes() == body


def test_a_download_that_cannot_happen_says_why(server, tmp_path):
    gone = RemoteFile("encoder.onnx", f"{server.url}/moved.onnx", 10, "0" * 64)
    with pytest.raises(DownloadFailed, match="404"):
        download(tmp_path, [gone], progress=lambda done, total: None)


def test_files_already_there_are_not_downloaded_again(server, tmp_path):
    files = [remote(server, "vocab.txt", b"<|en|>\n<|es|>\n")]
    download(tmp_path, files, progress=lambda done, total: None)
    download(tmp_path, files, progress=lambda done, total: None)
    assert len(server.requests) == 1
    (tmp_path / "vocab.txt").write_bytes(b"cut")
    assert missing(tmp_path, files) == files  # a file of another size is not the model's
