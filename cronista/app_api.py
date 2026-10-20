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
import traceback
from datetime import datetime
from pathlib import Path

from .engine.recorders import QUALITY, RecorderSettings, default_recorder, pick_microphone
from .engine.session import RecordingSession
from .engine.stt_engine import load_parakeet
from .engine.transcript import TranscriptionSegment
from .engine.voices import SOURCES, line, speakers
from .export import write_pdf, write_text_files, write_word
from .language import default_labels, default_title, language_or_default
from .library import Library
from .paths import IS_WINDOWS, data_dir, recordings_dir
from .subtitles import burn_subtitles


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
        try:
            self._recorder = default_recorder()
        except RuntimeError as error:
            self._recorder_error = str(error)

    # --- set-up -------------------------------------------------------------------------------
    def _attach(self, window) -> None:
        self._window = window
        threading.Thread(target=self._load_model, daemon=True).start()
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
                         labels: dict | None = None, subtitles: bool | None = None) -> dict:
        """The page's own preferences: its language ("en" or "es"), whether the tour was seen, the names
        the user gave the audio sources (an empty one goes back to the default), and whether recordings
        get a subtitled copy of their video."""
        changes = {}
        if language in ("en", "es"):
            changes["language"] = language
        if tour_seen is not None:
            changes["tour_seen"] = bool(tour_seen)
        if labels is not None:
            changes["labels"] = _clean_labels(labels)
        if subtitles is not None:
            changes["subtitles"] = bool(subtitles)
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
                         "subtitles": bool(settings.get("subtitles", False))},
            "recordings_dir": str(recordings_dir()),
        }

    # --- recording ----------------------------------------------------------------------------
    def start(self, microphone: str | None, quality: str, system_audio: bool = True,
              language: str | None = None, labels: dict | None = None, subtitles: bool | None = None) -> dict:
        """microphone: a device id from state()["microphones"]; language: the page's, "en" or "es";
        labels: the audio sources' names for this recording; subtitles: make a subtitled copy of it."""
        with self._lock:
            if self._phase != "idle":
                return {"error": "A recording is already running."}
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
        """Closing while recording first saves the recording, then closes."""
        if self._phase == "idle":
            return True
        self._close_when_saved = True
        self._emit("notice", {"message": "Saving the recording before closing…"})
        self.stop()
        return False

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
        """Starts making recording-subtitled.mp4; "subtitled" (or "error") says when it is done."""
        record = self._library.get(recording_id)
        if not record:
            return {"error": "Recording not found."}
        if self._recorder is None or not getattr(self._recorder, "ffmpeg", None):
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
                path = burn_subtitles(self._recorder.ffmpeg, Path(record["folder"]), segments, names)
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
                         recorded_at=datetime.fromisoformat(record["created_at"]), language=language, names=names)
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
