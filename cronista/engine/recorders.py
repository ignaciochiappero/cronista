"""Recorders: the processes that record, launched by RecordingSession.

A recorder records the screen and the audio to a file, streams the audio to stdout as 16 kHz mono
float32 for the speech model, and stops cleanly when it reads "q" on stdin. On Windows that is two
ffmpeg processes, the screen (the spike measured ~0.4 % CPU for capture and hardware encoding) and
the audio, which takes the audio_sources.py pipes (the microphone, and the computer's sound when
that is on); finish() puts both in one file when they stop. On Linux it is one process,
linux/portal_recorder.py (desktop portal plus GStreamer), microphone only.
"""
from __future__ import annotations

import os
import re
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path

from .ffmpeg_locator import NO_WINDOW, find_ffmpeg

SAMPLE_RATE = 16_000
IS_WINDOWS = os.name == "nt"
LINUX_RECORDER = Path(__file__).resolve().parents[1] / "linux" / "portal_recorder.py"
# The Linux recorder needs PyGObject, which lives in the system Python, not in the app's environment.
SYSTEM_PYTHON = os.environ.get("CRONISTA_SYSTEM_PYTHON", "/usr/bin/python3")
QUALITY = {"best": 85, "balanced": 70, "smallest": 55}  # Media Foundation quality, 0-100
# The screen, with showinfo logging each frame: its line about the first one is how the session
# knows when the video's time zero was, so the audio can start at the same moment. The log level
# starts at 34 (info is 32) and the session types "-" once that line is in: down to 24, warnings,
# so the per-frame lines stop. A branch that logs only the first frame is no way out: a filter
# graph without inputs aborts ("Assertion best_input >= 0 failed") when a sink inside it waits on
# frames that never come, and on a still screen they don't.
SCREEN_GRAPH = "gfxcapture=monitor_idx=0:max_framerate={fps},showinfo=checksum=0[screen]"
FIRST_FRAME = re.compile(r"Parsed_showinfo.*\bn:\s*0\s+pts:")
STEREO_48K = "aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo"
# Microphone (input 0) plus the computer's sound (input 1). Summed rather than averaged, so the
# microphone keeps the level it has without the computer's sound; the limiter only acts on the
# rare peak where both are loud at once. One mix goes to the file and one to the speech model.
MIX_GRAPH = (f"[0:a]{STEREO_48K}[mic];[1:a]{STEREO_48K}[computer];"
             "[mic][computer]amix=inputs=2:duration=first:normalize=0,"
             "alimiter=level=false:latency=true,asplit[rec][stt]")


@dataclass(frozen=True)
class RecorderSettings:
    microphone: str | None = None  # Windows: a WASAPI device id; Linux uses the default source
    quality: str = "balanced"
    fps: int = 30
    max_seconds: int = 4 * 3600
    system_audio: bool = False  # also record what the computer plays (Windows)


@dataclass(frozen=True)
class RecorderProcess:
    name: str  # starts its lines in recorder.log when a recorder has more than one
    command: list[str]
    speech: bool = False  # its stdout is the 16 kHz audio for the speech model


def wake_screen(monitor_index: int = 0) -> None:
    """Shows a one-pixel window in the corner of a monitor for a moment. Windows only.

    gfxcapture sends a frame only when the screen changes, and only between frames does its ffmpeg
    look at a request to stop: on a still screen "q" waited for the next change, once for more than
    30 s. Showing and hiding the pixel are two changes, two frames. The last one also ends the video
    at the moment the recording stops, so it lasts as long as the audio.
    """
    import ctypes
    from ctypes import wintypes

    user32 = ctypes.windll.user32
    corners: list[tuple[int, int]] = []
    callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HMONITOR, wintypes.HDC, ctypes.POINTER(wintypes.RECT),
                                  wintypes.LPARAM)(lambda monitor, dc, rect, data: corners.append(
                                      (rect.contents.left, rect.contents.top)) or True)
    user32.EnumDisplayMonitors(None, None, callback, 0)  # the order gfxcapture's monitor_idx counts in
    left, top = corners[monitor_index] if monitor_index < len(corners) else (0, 0)
    style, ex_style = 0x80000000 | 0x10000000, 0x00000008 | 0x00000080 | 0x08000000  # popup, visible; topmost, tool, no activate
    user32.CreateWindowExW.restype = wintypes.HWND
    user32.DestroyWindow.argtypes = [wintypes.HWND]
    window = user32.CreateWindowExW(ex_style, "STATIC", None, style, left, top, 1, 1, None, None, None, None)
    message = wintypes.MSG()
    for _ in range(2):  # shown, then gone: each a change on the screen
        deadline = time.monotonic() + 0.1
        while time.monotonic() < deadline:
            while user32.PeekMessageW(ctypes.byref(message), None, 0, 0, 1):
                user32.TranslateMessage(ctypes.byref(message))
                user32.DispatchMessageW(ctypes.byref(message))
            time.sleep(0.01)
        if window:
            user32.DestroyWindow(window)
            window = None


def pick_microphone(devices: list[dict], wanted: str | None, default_id: str | None) -> dict | None:
    """The microphone to record: the one asked for, by id or by name (settings kept the name before
    version 0.2.0), else the Windows default, else the first one plugged in."""
    for device in devices:
        if wanted in (device["id"], device["name"]):
            return device
    for device in devices:
        if device["id"] == default_id:
            return device
    return next(iter(devices), None)


class WindowsRecorder:
    output_name = "recording.mp4"
    screen_name = "screen.mp4"  # the parts, put together in recording.mp4 when the recording stops
    audio_name = "audio.m4a"
    supports_system_audio = True
    # How long after the first frame was captured ffmpeg's log line about it arrives (measured).
    first_frame_report_delay = 0.0
    after_first_frame = b"-"  # typed into ffmpeg once the first frame was reported: see SCREEN_GRAPH
    wake_screen = staticmethod(wake_screen)  # after "q": see wake_screen()

    def __init__(self, ffmpeg: str) -> None:
        self.ffmpeg = ffmpeg

    def microphones(self) -> tuple[list[dict], str | None]:
        """The recording devices plugged in, as {"id", "name"}, and the Windows default's id."""
        from . import wasapi

        return wasapi.capture_devices()

    def audio_sources(self, settings: RecorderSettings, gate, on_notice) -> list:
        from .audio_sources import AudioSource

        devices, default = self.microphones()
        microphone = pick_microphone(devices, settings.microphone, default)
        if microphone is None:
            raise RuntimeError("No microphone found. Plug one in and try again.")
        sources = [AudioSource.microphone(microphone["id"], gate, on_notice)]
        if settings.system_audio:
            sources.append(AudioSource.computer_sound(gate, on_notice))
        return sources

    def processes(self, folder: Path, settings: RecorderSettings, sources) -> list[RecorderProcess]:
        """The screen and the audio, each in its own ffmpeg. In one ffmpeg a still screen held the
        audio back: ffmpeg paces all its inputs to the stream that lags most, and gfxcapture sends
        frames only when the screen changes. 20 s of still screen let 1 s of audio through."""
        return [RecorderProcess("screen", self._screen_command(folder, settings)),
                RecorderProcess("audio", self._audio_command(folder, settings, sources), speech=True)]

    def _screen_command(self, folder: Path, settings: RecorderSettings) -> list[str]:
        return [
            # Keys typed into ffmpeg ("-", and "q" to stop) are read every stats period: 0.1 s
            # rather than 0.5. -nostats keeps the progress line out of the log.
            self.ffmpeg, "-hide_banner", "-nostats", "-stats_period", "0.1", "-loglevel", "level+34",
            "-filter_complex", SCREEN_GRAPH.format(fps=settings.fps),
            # Media Foundation picks the machine's hardware encoder and takes the captured GPU frames
            # as they are; frames only come when the screen changes (vfr).
            "-map", "[screen]", "-t", str(settings.max_seconds), "-fps_mode:v", "vfr",
            "-c:v", "h264_mf", "-hw_encoding", "1", "-rate_control", "quality",
            "-quality", str(QUALITY[settings.quality]), "-scenario", "display_remoting",
            "-profile:v", "100",  # High; Media Foundation defaults to Constrained Baseline
            "-g", "300",
            # Not fragmented MP4: it writes the header before Media Foundation hands over the codec
            # parameters, and ffprobe reported "missing picture" on the result.
            str(folder / self.screen_name),
        ]

    def _audio_command(self, folder: Path, settings: RecorderSettings, sources) -> list[str]:
        """The audio sources' pipes (the microphone first) to the audio file and to the model."""
        seconds = str(settings.max_seconds)
        inputs: list[str] = []
        for source in sources:
            inputs += ["-thread_queue_size", "1024", *source.ffmpeg_input()]
        mixing: list[str] = []
        recording_audio = speech_audio = "0:a"
        if len(sources) > 1:
            mixing = ["-filter_complex", MIX_GRAPH]
            recording_audio, speech_audio = "[rec]", "[stt]"
        return [
            self.ffmpeg, "-hide_banner", "-nostats", "-stats_period", "0.1", "-loglevel", "level+warning",
            *mixing,
            *inputs,
            "-map", recording_audio, "-t", seconds, "-c:a", "aac", "-b:a", "128k", str(folder / self.audio_name),
            # The same audio as 16 kHz mono float32 for the speech model.
            "-map", speech_audio, "-t", seconds, "-ac", "1", "-ar", str(SAMPLE_RATE), "-f", "f32le", "pipe:1",
        ]

    def finish(self, folder: Path) -> None:
        """Puts the screen and the sound in recording.mp4, copied as they are, and removes the parts.

        Both start at the same moment (the audio waits for the screen's first frame), so they line
        up from the start. A part that is missing or empty is left out: the recording keeps the rest.
        """
        parts = [folder / self.screen_name, folder / self.audio_name]
        present = [part for part in parts if part.exists() and part.stat().st_size > 0]
        if not present:
            return
        command = [self.ffmpeg, "-hide_banner", "-loglevel", "warning", "-y"]
        for part in present:
            command += ["-i", str(part)]
        for index in range(len(present)):
            command += ["-map", str(index)]
        command += ["-c", "copy", "-movflags", "+faststart", str(folder / self.output_name)]
        result = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", errors="replace",
                                **NO_WINDOW)
        if result.returncode != 0:
            # The parts stay, so nothing is lost; the message says why.
            raise RuntimeError(f"Putting the screen and the sound together failed: {result.stderr.strip()[-300:]}")
        for part in present:
            part.unlink()

    @staticmethod
    def screen_started(line: str) -> bool:
        """Whether a line of ffmpeg's log reports the screen's first frame."""
        return bool(FIRST_FRAME.search(line))

    @staticmethod
    def frame_report(line: str) -> bool:
        """Whether a line is showinfo's, which the log can do without after the first frame."""
        return "Parsed_showinfo" in line


class LinuxRecorder:
    output_name = "recording.mkv"
    supports_system_audio = False

    def microphones(self) -> tuple[list[dict], str | None]:
        return [{"id": "default", "name": "Default microphone"}], "default"

    def processes(self, folder: Path, settings: RecorderSettings, sources=()) -> list[RecorderProcess]:
        return [RecorderProcess("recorder", [
            SYSTEM_PYTHON, str(LINUX_RECORDER), "--out", str(folder / self.output_name),
            "--seconds", str(settings.max_seconds), "--fps", str(settings.fps),
            "--status", str(folder / "recorder-status.json"),
        ], speech=True)]


def default_recorder():
    if IS_WINDOWS:
        ffmpeg = find_ffmpeg()
        if not ffmpeg:
            raise RuntimeError("ffmpeg was not found. Install it with: winget install ffmpeg")
        return WindowsRecorder(ffmpeg)
    return LinuxRecorder()
