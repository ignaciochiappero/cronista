"""Cronista: records the screen and writes down what is said, live and on this machine.

Run from source with: .venv\\Scripts\\python.exe app.py
  --debug       opens the browser developer tools in the window
  --self-test   records 5 seconds (screen, microphone and computer sound) without a window, checks
                the result, deletes it, and exits (used to check a packaged build; the report goes
                to the log file). With Canary downloaded it also transcribes in high quality the file
                CRONISTA_SELF_TEST_SPEECH names (else the recording); with CRONISTA_SELF_TEST_NETWORK
                set, it downloads Canary's two small files to check HTTPS.
"""
import os
import sys
import time
from datetime import datetime
from pathlib import Path

BASE_DIR = Path(getattr(sys, "_MEIPASS", Path(__file__).resolve().parent))


def send_output_to_log() -> None:
    """The packaged app has no console, so its output and errors go to a log file instead.

    Done first, before the engine is imported, so an import error is logged too.
    """
    if not getattr(sys, "frozen", False) and sys.stdout is not None and sys.stderr is not None:
        return
    from cronista.paths import data_dir

    data_dir().mkdir(parents=True, exist_ok=True)
    log = open(data_dir() / "cronista.log", "a", encoding="utf-8", buffering=1)
    log.write(f"\n--- {datetime.now():%Y-%m-%d %H:%M:%S} {' '.join(sys.argv[1:])} ---\n")
    sys.stdout = sys.stderr = log


def self_test() -> int:
    from cronista.app_api import Api

    api = Api()
    notices: list[str] = []  # e.g. the computer's sound could not be recorded
    endings: list[tuple[str, dict]] = []  # how the file given to transcribe ended

    def emit(kind: str, payload: dict) -> None:
        if kind == "notice":
            notices.append(payload["message"])
        elif kind in ("imported", "import-error", "import-cancelled"):
            endings.append((kind, payload))

    api._emit = emit
    api._model_state = {"state": "loading"}
    api._load_model()
    state = api.state()
    print("self-test: model", state["model"], "| microphones", [m["name"] for m in state["microphones"]],
          "| error", state["recorder_error"] or "none")
    started = api.start(state["settings"]["microphone"], "balanced", True)
    print("self-test: start", started)
    if "error" in started:
        return 1
    # The recorder can take seconds to start (the first run of a freshly built ffmpeg.exe gets an
    # antivirus scan), so the 5 seconds count from the first audio.
    clicked = time.monotonic()
    while api._phase == "listening" and time.monotonic() - clicked < 60:
        time.sleep(0.05)
    print(f"self-test: recording after {time.monotonic() - clicked:.1f} s")
    time.sleep(5)
    api.stop()
    deadline = time.monotonic() + 120
    while api._phase != "idle" and time.monotonic() < deadline:
        time.sleep(0.3)
    records = api.recordings("")
    if not records:
        print("self-test: FAILED, nothing was saved")
        return 1
    record = api.recording(records[0]["id"])
    video = Path(record["video_path"])
    ok = video.exists() and record["size_bytes"] > 0 and record["duration_seconds"] > 4 and not notices
    print(f"self-test: saved {video.name} {record['size_bytes']} bytes, {record['duration_seconds']} s, "
          f"{len(record['segments'])} segments, notices {notices or 'none'} -> {'OK' if ok else 'FAILED'}")
    exported = export_check(record, api._recorder.ffmpeg)
    print(f"self-test: exported {exported or 'nothing'}")
    transcribed = import_check(api, endings, record["video_path"])
    print(f"self-test: transcribed the video as a dropped file: {transcribed}")
    ok = ok and len(exported) == 3 and transcribed == "imported"
    if ok and os.environ.get("CRONISTA_SELF_TEST_NETWORK"):
        fetched = download_check()
        print(f"self-test: downloaded Canary's small files over HTTPS: {fetched}")
        ok = fetched == "checked"
    if ok and api._canary_status()["state"] == "ready":
        high = high_quality_check(api, endings, os.environ.get("CRONISTA_SELF_TEST_SPEECH") or record["video_path"])
        print(f"self-test: in high quality: {high}")
        ok = high.startswith("imported") and not notices
    if ok:
        api.delete(record["id"])
        return 0
    import faulthandler

    print(f"self-test: kept {record['folder']} (recorder.log is there); the threads still running:")
    sys.stdout.flush()
    faulthandler.dump_traceback(file=sys.stdout, all_threads=True)
    return 1


def export_check(record: dict, ffmpeg: str) -> list[str]:
    """Writes the Word and PDF files and the subtitled copy of a saved recording (without opening
    Explorer): the files that came out non-empty. A silent recording gets two sample lines, one per
    audio source, so the labels and the subtitles are laid out too; the subtitled copy also proves
    that the bundled ffmpeg has libass and the Media Foundation encoder."""
    from datetime import datetime

    from cronista.engine.transcript import TranscriptionSegment
    from cronista.engine.voices import speakers
    from cronista.export import write_pdf, write_word
    from cronista.subtitles import burn_subtitles

    segments = [TranscriptionSegment(s["start"], s["end"], s["text"], s.get("source")) for s in record["segments"]] \
        or [TranscriptionSegment(0.0, 1.5, "Prueba de exportación: acentos, eñes y ¿signos?", "mic"),
            TranscriptionSegment(1.5, 3.0, "And a second line, from the computer.", "computer")]
    names = speakers(segments, {"mic": "Yo", "computer": "Ana"})
    folder = Path(record["folder"])
    written = []
    for write, name in ((write_word, "transcript.docx"), (write_pdf, "transcript.pdf")):
        path = write(segments, folder / name, title=record["title"],
                     recorded_at=datetime.fromisoformat(record["created_at"]), names=names)
        if path.stat().st_size > 0:
            written.append(f"{name} ({path.stat().st_size} bytes)")
    try:
        path = burn_subtitles(ffmpeg, folder, segments, names)
        if path.stat().st_size > 0:
            written.append(f"{path.name} ({path.stat().st_size} bytes)")
    except RuntimeError as error:
        print(f"self-test: {error}")
    return written


def import_check(api, endings: list, video: str, said: list | None = None) -> str:
    """Transcribes the recording's video as if it had been dropped on the window, which proves that
    the bundled ffmpeg reports on a file and decodes its sound. Removes that transcript from the
    history again (its folder only: the video stays). Says how it ended: "imported", or why not;
    `said` gets the transcript."""
    endings.clear()
    result = api._import_files([video])
    if "error" in result:
        return result["error"]
    deadline = time.monotonic() + 300
    while not endings and time.monotonic() < deadline:
        time.sleep(0.2)
    if not endings:
        return "no answer in 300 s"
    kind, payload = endings[0]
    if kind == "imported":
        if said is not None:
            said.append(" ".join(segment["text"] for segment in api.recording(payload["id"])["segments"]))
        api.delete(payload["id"])
        return kind
    return f"{kind}: {payload.get('message', '')}"


def high_quality_check(api, endings: list, path: str) -> str:
    """With Canary downloaded, transcribes `path` in high quality, which proves that Canary loads and
    writes in this build (CRONISTA_SELF_TEST_SPEECH names a file with speech, for Canary to have words
    to write again). The quality chosen before is put back. Says how it ended, with the transcript."""
    chosen = api._load_settings().get("transcription", "standard")
    api.set_transcription("high")
    said: list[str] = []
    try:
        ended = import_check(api, endings, path, said)
    finally:
        api.set_transcription(chosen)
    return f"{ended}: {said[0][:300] if said else ''}"


def download_check() -> str:
    """Downloads Canary's two small files into a temporary folder through the app's downloader, which
    proves HTTPS, Hugging Face's redirects and the SHA-256 checks work in this build."""
    import tempfile

    from cronista.downloads import DownloadFailed, download
    from cronista.engine.stt_engine import CANARY_FILES

    with tempfile.TemporaryDirectory() as folder:
        try:
            download(Path(folder), [file for file in CANARY_FILES if file.size < 1_000_000], progress=lambda *_: None)
        except DownloadFailed as error:
            return str(error)
    return "checked"


def main() -> int:
    send_output_to_log()
    try:
        return run()
    except Exception:  # noqa: BLE001 - logged, since a windowed app has nowhere else to show it
        import traceback

        traceback.print_exc()
        return 1


def run() -> int:
    if "--self-test" in sys.argv:
        return self_test()

    import webview

    from cronista.app_api import Api

    api = Api()
    window = webview.create_window(
        title="Cronista",
        url=str(BASE_DIR / "ui" / "index.html"),
        width=1120,
        height=740,
        min_size=(900, 600),
        js_api=api,
    )
    window.events.closing += api._on_closing
    webview.start(api._attach, (window,), debug="--debug" in sys.argv)
    return 0


if __name__ == "__main__":
    sys.exit(main())
