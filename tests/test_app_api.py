"""The window's API on its own: a history in a temporary folder, no recorder and no model."""
import pytest
from docx import Document

import cronista.app_api as app_api


@pytest.fixture
def api(tmp_path, monkeypatch):
    monkeypatch.setattr(app_api, "data_dir", lambda: tmp_path / "data")
    monkeypatch.setattr(app_api, "recordings_dir", lambda: tmp_path / "videos")
    monkeypatch.setattr(app_api, "IS_WINDOWS", False)  # no Explorer windows from the tests
    monkeypatch.setattr(app_api, "default_recorder", lambda: None)
    return app_api.Api()


def add_call(api, tmp_path):
    folder = tmp_path / "videos" / "2026-10-20_10-00-00"
    folder.mkdir(parents=True)
    segments = [{"start": 0.0, "end": 1.0, "text": "Let's start.", "source": "mic"},
                {"start": 1.5, "end": 3.0, "text": "Sure.", "source": "computer"}]
    recording_id = api._library.add(
        title="Call", created_at="2026-10-20T10:00:00", duration_seconds=3.0, size_bytes=1, folder=str(folder),
        video_path=str(folder / "recording.mp4"), transcript="Let's start. Sure.", segments=segments,
        labels={"mic": "Me", "computer": "Others"})
    return recording_id, folder


def test_renaming_a_label_renames_it_in_the_files_too(api, tmp_path):
    recording_id, folder = add_call(api, tmp_path)
    assert api.rename_label(recording_id, "computer", "  Ana  ") == {"ok": True, "labels": {"mic": "Me", "computer": "Ana"}}
    assert "Ana: Sure." in (folder / "transcript.srt").read_text(encoding="utf-8")
    assert (folder / "transcript.txt").read_text(encoding="utf-8").splitlines() == ["Me: Let's start.", "Ana: Sure."]
    assert api.rename_label(recording_id, "computer", "   ") == {"error": "Enter a label."}
    assert api.rename_label(recording_id, "screen", "X") == {"error": "Recording not found."}


def test_exports_say_who_said_what(api, tmp_path):
    recording_id, _ = add_call(api, tmp_path)
    path = api.export_word(recording_id, "en")["path"]
    assert "[00:00:01 → 00:00:03] Others: Sure." in [paragraph.text for paragraph in Document(path).paragraphs]


def test_the_default_labels_are_kept_only_when_named(api):
    assert api.state()["settings"]["labels"] == {}
    api.save_preferences(labels={"mic": " Nacho ", "computer": "", "screen": "x"})
    assert api.state()["settings"]["labels"] == {"mic": "Nacho"}
    api.save_preferences(subtitles=True)
    assert api.state()["settings"]["subtitles"] is True
    assert api.state()["settings"]["labels"] == {"mic": "Nacho"}  # one preference does not undo another


def test_the_subtitled_video_is_made_in_the_background_with_the_labels(api, tmp_path, monkeypatch):
    import threading
    from types import SimpleNamespace

    recording_id, folder = add_call(api, tmp_path)
    (folder / "recording.mp4").write_bytes(b"video")
    api.rename_label(recording_id, "computer", "Ana")
    made = []

    def fake_burn(ffmpeg, where, segments, names):
        made.append((ffmpeg, where, [s.source for s in segments], names))
        (where / "recording-subtitled.mp4").write_bytes(b"video")
        return where / "recording-subtitled.mp4"

    monkeypatch.setattr(app_api, "burn_subtitles", fake_burn)
    api._recorder = SimpleNamespace(ffmpeg="ffmpeg.exe")
    events, done = [], threading.Event()
    api._emit = lambda kind, payload: (events.append((kind, payload)), kind == "subtitled" and done.set())
    assert api.subtitled_video(recording_id, "en") == {"ok": True}
    assert done.wait(5)
    assert made == [("ffmpeg.exe", folder, ["mic", "computer"], {"mic": "Me", "computer": "Ana"})]
    assert events[0] == ("subtitling", {"id": recording_id})
    assert events[-1] == ("subtitled", {"id": recording_id, "path": str(folder / "recording-subtitled.mp4")})


def test_copied_text_says_who_said_what(api, tmp_path):
    recording_id, _ = add_call(api, tmp_path)
    assert api.copy_text(recording_id) == {"text": "Me: Let's start.\nOthers: Sure."}
