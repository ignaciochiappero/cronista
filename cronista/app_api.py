"""The bridge between the window and the engine: window.pywebview.api in the page.

Same pattern as escriba's launcher.py: pywebview exposes this object's public methods to the page,
and the page gets pushed events through evaluate_js (window.cronista.onEvent). Every method runs
in a pywebview worker thread and returns plain JSON-able values.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import threading
import time
import traceback
from datetime import datetime
from pathlib import Path

from .engine.ffmpeg_locator import find_ffmpeg
from .engine.recorders import QUALITY, RecorderSettings, default_recorder, pick_microphone
from .engine.session import RecordingSession
from .engine.stt_engine import load_parakeet
from .engine.transcript import TranscriptionSegment
from .engine.voices import SOURCES, line, speakers
from .export import write_pdf, write_text_files, write_word
from .importer import MEDIA_PATTERNS, Cancelled, ImportFailed, transcribe_file
from .language import default_labels, default_title, language_or_default
from .library import Library
from .paths import IS_WINDOWS, data_dir, recordings_dir
from .subtitles import burn_subtitles


# The page's looks (ui/index.html draws each one); "system" follows Windows' light or dark mode.
THEMES = ("system", "dark", "light", "midnight", "ocean", "forest", "grape", "sunset", "paper", "contrast")
# Of a dropped file's progress bar, the part for reading its sound; the rest is the transcript.
READING_SHARE = 0.1


class Api:
    def __init__(self) -> None:
        self._window = None
        self._library = Library(data_dir() / "library.db")
        self._settings_path = data_dir() / "settings.json"
        self._model = None
        self._model_state = {"state": "loading"}
        self._recorder = None
        self._recorder_error = ""
        self._session: RecordingSession | None = None
        self._language = "en"  # the page's, when the recording started: its title's language
        self._labels: dict[str, str] = {}  # the audio sources' names for the recording that runs
        self._subtitles = False  # whether that recording gets a subtitled copy of its video
        self._subtitling: set[str] = set()  # recordings whose subtitled copy is being made
        self._phase = "idle"  # idle, listening, recording, finishing
        self._lock = threading.Lock()
        self._close_when_saved = False
        # Files dropped on the window or picked, transcribed one at a time, never while recording.
        self._import_queue: list[Path] = []
        self._importing: dict | None = None  # the one being transcribed, as the page shows it
        self._importer: threading.Thread | None = None
        self._import_stop = threading.Event()
        try:
            self._recorder = default_recorder()
        except RuntimeError as error:
            self._recorder_error = str(error)

    # --- set-up -------------------------------------------------------------------------------
    def _attach(self, window) -> None:
        self._window = window
        threading.Thread(target=self._load_model, daemon=True).start()
        threading.Thread(target=self._listen_for_drops, args=(window,), daemon=True).start()
        # Started from a hidden or minimized console, Windows applies that state to the window's
        # first appearance, and WebView2 lays the page out at zero size: blank once restored.
        # Restoring and changing the size once makes WebView2 lay the page out again.
        if window.events.shown.wait(15):
            window.restore()
            width, height = window.width, window.height
            window.resize(width, height + 1)
            window.resize(width, height)

    def _load_model(self) -> None:
        try:
            self._model = load_parakeet(threads=4)
            self._model_state = {"state": "ready"}
        except Exception as error:  # noqa: BLE001 - shown to the user, details in the console
            traceback.print_exc()
            self._model_state = {"state": "error", "message": f"The speech model did not load: {error}"}
        self._emit("model", self._model_state)

    def _emit(self, kind: str, payload: dict) -> None:
        if self._window is None:
            return
        try:
            self._window.evaluate_js(
                f"window.cronista && window.cronista.onEvent({json.dumps(kind)}, {json.dumps(payload, ensure_ascii=False)})")
        except Exception:  # noqa: BLE001 - the window may be closing
            traceback.print_exc()

    def _load_settings(self) -> dict:
        try:
            return json.loads(self._settings_path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return {}

    def _save_settings(self, **changes) -> None:
        """Merges `changes` into settings.json, keeping what is already there."""
        settings = {**self._load_settings(), **changes}
        self._settings_path.parent.mkdir(parents=True, exist_ok=True)
        self._settings_path.write_text(json.dumps(settings), encoding="utf-8")

    def save_preferences(self, language: str | None = None, tour_seen: bool | None = None,
                         labels: dict | None = None, subtitles: bool | None = None, theme: str | None = None) -> dict:
        """The page's own preferences: its language ("en" or "es"), whether the tour was seen, the names
        the user gave the audio sources (an empty one goes back to the default), whether recordings
        get a subtitled copy of their video, and its theme (one of THEMES)."""
        changes = {}
        if language in ("en", "es"):
            changes["language"] = language
        if tour_seen is not None:
            changes["tour_seen"] = bool(tour_seen)
        if labels is not None:
            changes["labels"] = _clean_labels(labels)
        if subtitles is not None:
            changes["subtitles"] = bool(subtitles)
        if theme in THEMES:
            changes["theme"] = theme
        if changes:
            self._save_settings(**changes)
        return {"ok": True}

    # --- state --------------------------------------------------------------------------------
    def state(self) -> dict:
        microphones: list[dict] = []  # {"id", "name"}
        default = None
        if self._recorder is not None:
            try:
                microphones, default = self._recorder.microphones()
            except Exception:  # noqa: BLE001 - an empty list is shown as "no microphone"
                traceback.print_exc()
        settings = self._load_settings()
        # The microphone chosen last time, else the Windows default, else the first one.
        chosen = pick_microphone(microphones, settings.get("microphone"), default)
        return {
            "model": self._model_state,
            "phase": self._phase,
            "recorder_error": self._recorder_error,
            "microphones": microphones,
            "qualities": list(QUALITY),
            "system_audio_supported": bool(getattr(self._recorder, "supports_system_audio", False)),
            "settings": {"microphone": chosen["id"] if chosen else None,
                         "quality": settings.get("quality", "balanced"),
                         "system_audio": bool(settings.get("system_audio", True)),
                         "language": settings.get("language"),  # None: the page follows the system's
                         "tour_seen": bool(settings.get("tour_seen", False)),
                         "labels": _clean_labels(settings.get("labels") or {}),  # only the ones the user named
                         "subtitles": bool(settings.get("subtitles", False)),
                         "theme": settings.get("theme") if settings.get("theme") in THEMES else "system"},
            "recordings_dir": str(recordings_dir()),
            "importing": dict(self._importing) if self._importing else None,
        }

    # --- recording ----------------------------------------------------------------------------
    def start(self, microphone: str | None, quality: str, system_audio: bool = True,
              language: str | None = None, labels: dict | None = None, subtitles: bool | None = None) -> dict:
        """microphone: a device id from state()["microphones"]; language: the page's, "en" or "es";
        labels: the audio sources' names for this recording; subtitles: make a subtitled copy of it."""
        with self._lock:
            if self._phase != "idle":
                return {"error": "A recording is already running."}
            if self._importing is not None or self._import_queue:
                return {"error": "A file is being transcribed. Cancel it to record."}
            if self._recorder is None:
                return {"error": self._recorder_error}
            if self._model is None:
                return {"error": "The speech model is still loading."}
            self._save_settings(microphone=microphone, quality=quality, system_audio=bool(system_audio))
            folder = recordings_dir() / datetime.now().strftime("%Y-%m-%d_%H-%M-%S")
            settings = RecorderSettings(microphone=microphone or None,
                                        quality=quality if quality in QUALITY else "balanced",
                                        system_audio=bool(system_audio))
            session = RecordingSession(self._model, self._recorder, settings, folder, self._on_session_event)
            try:
                session.start()
            except Exception as error:  # noqa: BLE001
                traceback.print_exc()
                return {"error": str(error)}
            self._session = session
            self._language = language_or_default(language or self._load_settings().get("language"))
            self._labels = {**default_labels(self._language), **_clean_labels(labels or {})}
            self._subtitles = bool(subtitles)
            self._phase = "listening"
        return {"ok": True}

    def stop(self) -> dict:
        with self._lock:
            if self._session is None or self._phase in ("idle", "finishing"):
                return {"ok": True}
            self._phase = "finishing"
        threading.Thread(target=self._finish, daemon=True).start()
        return {"ok": True}

    def _on_session_event(self, kind: str, payload: dict) -> None:
        if kind == "recording":
            self._phase = "recording"
        elif kind == "ended":
            # The recorder stopped on its own (time cap or an error): keep what was recorded.
            self._emit("notice", {"message": "The recorder stopped. Saving what was recorded."
                                             + (f" ({payload['reason']})" if payload.get("reason") else "")})
            self.stop()
        if kind != "finished":  # "saved" (below) is what the page needs at the end
            self._emit(kind, payload)

    def _finish(self) -> None:
        session = self._session
        try:
            result = session.stop()
            write_text_files(result.segments, Path(result.folder), speakers(result.segments, self._labels))
            started = datetime.fromisoformat(result.started_at)
            recording_id = self._library.add(
                title=default_title(started, self._language),
                created_at=result.started_at,
                duration_seconds=result.duration_seconds,
                size_bytes=result.size_bytes,
                folder=result.folder,
                video_path=result.video_path,
                transcript=result.transcript,
                segments=[{"start": s.start, "end": s.end, "text": s.text, "source": s.source}
                          for s in result.segments],
                labels=self._labels,
            )
            self._emit("saved", {"id": recording_id})
            if self._subtitles and Path(result.video_path).exists():
                self._subtitle(recording_id, self._language, reveal=False)
        except Exception as error:  # noqa: BLE001
            traceback.print_exc()
            self._emit("error", {"message": f"Saving the recording failed: {error}"})
        finally:
            self._session = None
            self._phase = "idle"
            if self._close_when_saved and self._window is not None:
                self._window.destroy()

    def _on_closing(self) -> bool:
        """Closing while recording first saves the recording, then closes. A file being transcribed is
        cancelled first, so no half-made transcript stays behind (it can be dropped again)."""
        if self._phase == "idle" and self._importer is None:
            return True
        self._close_when_saved = True
        if self._phase != "idle":
            self._emit("notice", {"message": "Saving the recording before closing…"})
            self.stop()
        else:
            self.cancel_import()
        return False

    # --- transcribing a file ------------------------------------------------------------------
    def pick_files(self, language: str | None = None) -> dict:
        """Windows' file dialog, for audio and video: the files chosen are transcribed one after another."""
        import webview

        spanish = language_or_default(language or self._load_settings().get("language")) == "es"
        kinds = (f"{'Audio y video' if spanish else 'Audio and video'} ({MEDIA_PATTERNS})",
                 f"{'Todos los archivos' if spanish else 'All files'} (*.*)")
        paths = self._window.create_file_dialog(webview.FileDialog.OPEN, allow_multiple=True, file_types=kinds)
        if not paths:
            return {"cancelled": True}
        return self._import_files(list(paths))

    def _import_files(self, paths: list[str]) -> dict:
        """Transcribes these files one after another, folders left out: "import" events say how the one
        being transcribed goes, then "imported", "import-error" or "import-cancelled" how it ended."""
        files = [Path(path) for path in paths if path and Path(path).is_file()]
        with self._lock:
            if self._phase != "idle":
                return {"error": "Stop the recording first, then drop the file again."}
            if self._model is None:
                return {"error": self._model_state.get("message") or "The speech model is still loading."}
            ffmpeg = self._ffmpeg()
            if not ffmpeg:
                return {"error": "ffmpeg was not found. Install it with: winget install ffmpeg"}
            if not files:
                return {"error": "Drop an audio or video file, not a folder."}
            self._import_queue += files
            if self._importer is None:
                self._importer = threading.Thread(target=self._import_all, args=(ffmpeg,), daemon=True)
                self._importer.start()
        self._update_import()  # the one being transcribed, if any, has more waiting now
        return {"ok": True, "queued": len(files)}

    def cancel_import(self) -> dict:
        """Stops the file being transcribed and forgets the ones waiting."""
        with self._lock:
            self._import_queue.clear()
            if self._importing is not None:
                self._import_stop.set()
        return {"ok": True}

    def _import_all(self, ffmpeg: str) -> None:
        """The files waiting, one at a time, in a thread of their own."""
        while True:
            with self._lock:
                if not self._import_queue:
                    self._importer = None
                    break
                path = self._import_queue.pop(0)
                self._import_stop.clear()
                self._importing = {"name": path.name, "stage": "reading", "progress": 0.0, "done": 0.0,
                                   "total": 0.0, "eta": None, "waiting": len(self._import_queue)}
                started = dict(self._importing)  # the page shows it from the start
            self._emit("import", started)
            self._import_one(ffmpeg, path)
        if self._close_when_saved and self._window is not None and self._phase == "idle":
            self._window.destroy()

    def _import_one(self, ffmpeg: str, path: Path) -> None:
        name = path.name
        folder = _new_folder(recordings_dir())
        pace: list[float] = []  # when the transcript began, and how many seconds of the file were done then

        def progress(stage: str, done: float, total: float) -> None:
            share = min(done / total, 1.0) if total > 0 else 0.0
            eta = None
            if stage == "reading":
                overall = READING_SHARE * share
            else:
                overall = READING_SHARE + (1 - READING_SHARE) * share
                now = time.monotonic()
                if not pace:
                    pace[:] = [now, done]
                elif done > pace[1]:
                    eta = round((total - done) * (now - pace[0]) / (done - pace[1]))
            self._update_import(stage=stage, progress=round(overall, 4), done=round(done, 1), total=round(total, 1),
                                eta=eta)

        def lines(segments: list[TranscriptionSegment]) -> None:
            self._update_import(lines=[{"start": s.start, "end": s.end, "text": s.text} for s in segments])

        try:
            result = transcribe_file(self._model, ffmpeg, path, folder, progress=progress, lines=lines,
                                     stopped=self._import_stop.is_set)
            self._update_import(stage="saving", progress=1.0, eta=0)
            write_text_files(result.segments, folder)
            recording_id = self._library.add(
                title=path.stem.strip()[:120] or name,
                created_at=datetime.now().isoformat(timespec="seconds"),
                duration_seconds=round(result.duration, 1),
                size_bytes=path.stat().st_size,
                folder=str(folder),
                video_path=str(path),
                transcript=" ".join(segment.text for segment in result.segments),
                segments=[{"start": s.start, "end": s.end, "text": s.text} for s in result.segments],
                kind="video" if result.has_video else "audio",
            )
            ended = ("imported", {"id": recording_id, "name": name})
        except Cancelled:
            ended = ("import-cancelled", {"name": name})
        except ImportFailed as error:
            ended = ("import-error", {"name": name, "message": str(error)})
        except Exception as error:  # noqa: BLE001 - shown to the user, details in the log
            traceback.print_exc()
            ended = ("import-error", {"name": name, "message": f"Transcribing the file failed: {error}"})
        if ended[0] != "imported":
            shutil.rmtree(folder, ignore_errors=True)  # only the folder made for it, never the file
        with self._lock:
            self._importing = None
        self._emit(*ended)

    def _update_import(self, **changes) -> None:
        """Merges what changed into the file being transcribed and tells the page, with how many files
        wait after it; `lines` go in the event only."""
        lines = changes.pop("lines", None)
        with self._lock:
            if self._importing is None:
                return
            self._importing.update(changes, waiting=len(self._import_queue))
            payload = dict(self._importing)
        if lines:
            payload["lines"] = lines
        self._emit("import", payload)

    def _on_drop(self, event: dict) -> None:
        """Files dropped on the window, with their full paths: pywebview adds them, as in escriba's launcher.
        A drop that cannot be taken says why with "import-refused", which ends nothing that is running."""
        files = (event.get("dataTransfer") or {}).get("files") or []
        paths = [file["pywebviewFullPath"] for file in files if file.get("pywebviewFullPath")]
        result = self._import_files(paths) if paths else {
            "error": "The dropped file could not be read. Choose it with Transcribe a file."}
        if "error" in result:
            self._emit("import-refused", {"message": result["error"]})

    def _listen_for_drops(self, window) -> None:
        """Subscribes to files dropped on the page once it has loaded, as escriba's launcher does: WebView2
        gives a dropped file's full path only to pywebview's DOM events. If this fails, the page still has
        the file dialog."""
        from webview.dom import DOMEventHandler

        if not window.events.loaded.wait(60):
            return
        for _ in range(20):  # pywebview's bridge comes with the page, a moment after it loads
            try:
                window.dom.document.on("drop", DOMEventHandler(self._on_drop, prevent_default=True))
                self._emit("drop-ready", {})
                return
            except Exception:  # noqa: BLE001 - tried again; in the end the page keeps the file dialog only
                time.sleep(0.5)
        traceback.print_exc()

    def _ffmpeg(self) -> str | None:
        return getattr(self._recorder, "ffmpeg", None) or find_ffmpeg()

    # --- library ------------------------------------------------------------------------------
    def recordings(self, search: str = "") -> list[dict]:
        return self._library.list(search=search or None)

    def recording(self, recording_id: str) -> dict | None:
        return self._library.get(recording_id)

    def rename_label(self, recording_id: str, source: str, label: str) -> dict:
        """Names an audio source of one recording ("computer" -> "Ana"), there and in its SRT and TXT."""
        record = self._library.get(recording_id)
        if not record or source not in SOURCES:
            return {"error": "Recording not found."}
        label = " ".join(label.split())[:40]
        if not label:
            return {"error": "Enter a label."}
        self._library.set_label(recording_id, source, label)
        record = self._library.get(recording_id)
        segments = _segments(record)
        if Path(record["folder"]).is_dir():
            write_text_files(segments, Path(record["folder"]), speakers(segments, record["labels"]))
        return {"ok": True, "labels": record["labels"]}

    def subtitled_video(self, recording_id: str, language: str | None = None) -> dict:
        """Starts making the subtitled copy (recording-subtitled.mp4, or "<file>-subtitled.mp4" for a
        dropped video); "subtitled" (or "error") says when it is done."""
        record = self._library.get(recording_id)
        if not record:
            return {"error": "Recording not found."}
        if record["kind"] == "audio":
            return {"error": "This file has no pictures to put the subtitles on."}
        if not self._ffmpeg():
            return {"error": self._recorder_error or "ffmpeg was not found. Install it with: winget install ffmpeg"}
        if not Path(record["video_path"]).exists():
            return {"error": "The video file is no longer there."}
        self._subtitle(recording_id, language, reveal=True)
        return {"ok": True}

    def _subtitle(self, recording_id: str, language: str | None, *, reveal: bool) -> None:
        """Makes the subtitled copy in a thread of its own: it re-encodes the whole video."""
        with self._lock:
            if recording_id in self._subtitling:
                return
            self._subtitling.add(recording_id)
        language = language_or_default(language or self._load_settings().get("language"))

        def work() -> None:
            self._emit("subtitling", {"id": recording_id})
            try:
                record = self._library.get(recording_id)
                segments = _segments(record)
                names = speakers(segments, {**default_labels(language), **record["labels"]})
                dropped = {"source": Path(record["video_path"])} if record["kind"] == "video" else {}
                path = burn_subtitles(self._ffmpeg(), Path(record["folder"]), segments, names, **dropped)
                if reveal and IS_WINDOWS:
                    subprocess.Popen(["explorer", "/select,", str(path)])
                self._emit("subtitled", {"id": recording_id, "path": str(path)})
            except Exception as error:  # noqa: BLE001 - shown to the user, details in the log
                traceback.print_exc()
                self._emit("error", {"message": str(error), "id": recording_id})
            finally:
                with self._lock:
                    self._subtitling.discard(recording_id)

        threading.Thread(target=work, daemon=True).start()

    def rename(self, recording_id: str, title: str) -> dict:
        title = title.strip()
        if not title:
            return {"error": "Enter a title."}
        self._library.rename(recording_id, title)
        return {"ok": True}

    def play(self, recording_id: str) -> dict:
        record = self._library.get(recording_id)
        if not record or not Path(record["video_path"]).exists():
            return {"error": "The video file is no longer there."}
        self._open(record["video_path"])
        return {"ok": True}

    def reveal(self, recording_id: str) -> dict:
        record = self._library.get(recording_id)
        if not record:
            return {"error": "Recording not found."}
        target = Path(record["video_path"])
        if IS_WINDOWS and target.exists():
            subprocess.Popen(["explorer", "/select,", str(target)])
        else:
            self._open(record["folder"])
        return {"ok": True}

    def open_recordings_folder(self) -> dict:
        recordings_dir().mkdir(parents=True, exist_ok=True)
        self._open(str(recordings_dir()))
        return {"ok": True}

    def copy_text(self, recording_id: str) -> dict:
        record = self._library.get(recording_id)
        if not record:
            return {"error": "Recording not found."}
        segments = _segments(record)
        names = speakers(segments, {**default_labels(self._load_settings().get("language")), **record["labels"]})
        text = "\n".join(line(segment, names) for segment in segments)
        if IS_WINDOWS:
            # clip.exe reads UTF-16 with a byte-order mark as Unicode.
            subprocess.run(["clip"], input=("﻿" + text).encode("utf-16-le"), check=False,
                           creationflags=subprocess.CREATE_NO_WINDOW)
            return {"ok": True}
        return {"text": text}  # the page copies it with the Clipboard API

    def export_word(self, recording_id: str, language: str | None = None) -> dict:
        return self._export(recording_id, write_word, "transcript.docx", language)

    def export_pdf(self, recording_id: str, language: str | None = None) -> dict:
        return self._export(recording_id, write_pdf, "transcript.pdf", language)

    def _export(self, recording_id: str, write, file_name: str, language: str | None) -> dict:
        """Writes the transcript next to the video, its headings in `language`, and shows it in Explorer."""
        record = self._library.get(recording_id)
        if not record:
            return {"error": "Recording not found."}
        segments = _segments(record)
        language = language_or_default(language or self._load_settings().get("language"))
        names = speakers(segments, {**default_labels(language), **record["labels"]})
        try:
            path = write(segments, Path(record["folder"]) / file_name, title=record["title"],
                         recorded_at=datetime.fromisoformat(record["created_at"]), language=language, names=names,
                         from_file=record["kind"] != "recording")
        except PermissionError:
            return {"error": f"{file_name} is open in another program. Close it and try again."}
        if IS_WINDOWS:
            subprocess.Popen(["explorer", "/select,", str(path)])
        return {"ok": True, "path": str(path)}

    def delete(self, recording_id: str) -> dict:
        record = self._library.get(recording_id)
        if not record:
            return {"ok": True}
        folder = Path(record["folder"]).resolve()
        root = recordings_dir().resolve()
        # Only ever delete a folder this app created under its recordings directory.
        if folder.parent == root and folder.exists():
            shutil.rmtree(folder)
        self._library.delete(recording_id)
        return {"ok": True}

    @staticmethod
    def _open(path: str) -> None:
        if IS_WINDOWS:
            os.startfile(path)  # noqa: S606 - opens with the user's default app
        elif sys.platform == "darwin":
            subprocess.Popen(["open", path])
        else:
            subprocess.Popen(["xdg-open", path])


def _new_folder(root: Path) -> Path:
    """A new folder named for this moment, like a recording's ("-2", "-3"... when two start in the same second)."""
    stamp = datetime.now().strftime("%Y-%m-%d_%H-%M-%S")
    folder, number = root / stamp, 1
    while True:
        try:
            folder.mkdir(parents=True)
            return folder
        except FileExistsError:
            number += 1
            folder = root / f"{stamp}-{number}"


def _clean_labels(labels: dict) -> dict[str, str]:
    """The names the user gave the audio sources: known sources only, trimmed, at most 40 characters."""
    clean = {}
    for source in SOURCES:
        label = " ".join(str(labels.get(source) or "").split())[:40]
        if label:
            clean[source] = label
    return clean


def _segments(record: dict) -> list[TranscriptionSegment]:
    return [TranscriptionSegment(s["start"], s["end"], s["text"], s.get("source")) for s in record["segments"]]
