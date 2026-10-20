"""Transcribes an audio or video file the user drops on the window or picks: any format ffmpeg reads.

Nothing of the file is copied. ffmpeg's own report on it says how long it is and whether it has sound
and moving pictures (the installer ships ffmpeg.exe only: ffprobe would add 134 MB to say the same),
ffmpeg decodes its first audio stream to 16 kHz mono float32, the format of a recording's tapes, in
the folder Cronista keeps for the transcript, and the final pass reads that in chunks, as it does for
a recording. The decoded audio is deleted at the end: the history keeps the transcript and where the
file is, so Play and Show in folder open the file itself.
"""
from __future__ import annotations

import re
import subprocess
import threading
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

from .engine.ffmpeg_locator import NO_WINDOW
from .engine.final_pass import run_final_pass_file
from .engine.transcript import TranscriptionSegment

SAMPLE_RATE = 16_000
AUDIO_NAME = "audio-16k.f32"  # in the transcript's folder while the pass runs
# A recording's final pass reads 90 s at a time; a dropped file goes in 30 s chunks, each cut in
# the quietest moment of its last 5 s, so its progress and lines come every couple of seconds.
CHUNK_SECONDS, SEARCH_SECONDS = 30.0, 5.0
# The file dialog's filter. A drop takes any file and lets ffmpeg decide.
MEDIA_PATTERNS = ";".join(f"*.{extension}" for extension in (
    "mp3", "wav", "m4a", "aac", "ogg", "oga", "opus", "flac", "wma", "aif", "aiff", "amr", "mka",
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "mts", "m2ts", "3gp"))

# In `ffmpeg -i`'s report: "  Duration: 01:02:03.50, start: ..." and, one per stream,
# "  Stream #0:1[0x2](eng): Audio: aac (LC), 44100 Hz, ..." (a song's cover art is a Video stream
# marked "(attached pic)").
_DURATION = re.compile(r"^\s*Duration: (\d+):(\d\d):(\d\d(?:\.\d+)?)", re.MULTILINE)
_STREAM = re.compile(r"^\s*Stream #\d+:\d+\S*: (\w+): (.*)$", re.MULTILINE)


class ImportFailed(Exception):
    """A file that cannot be transcribed; the message is for the user."""


class Cancelled(Exception):
    """The user cancelled while the file was being read or transcribed."""


@dataclass
class MediaInfo:
    duration: float  # 0 when the file does not say
    has_audio: bool
    has_video: bool


@dataclass
class ImportResult:
    duration: float
    has_video: bool
    segments: list[TranscriptionSegment] = field(default_factory=list)


def read_report(report: str) -> MediaInfo:
    """What `ffmpeg -i <file>` says about the file: its length, and whether it has sound and pictures."""
    if "Input #0" not in report:
        raise ImportFailed("Cronista can't read this file. It doesn't look like audio or video.")
    streams = _STREAM.findall(report)
    duration = _DURATION.search(report)
    return MediaInfo(
        duration=int(duration[1]) * 3600 + int(duration[2]) * 60 + float(duration[3]) if duration else 0.0,
        has_audio=any(kind == "Audio" for kind, _ in streams),
        has_video=any(kind == "Video" and "(attached pic)" not in details for kind, details in streams),
    )


def probe(ffmpeg: str, path: Path) -> MediaInfo:
    # With no output file ffmpeg reports on the input and stops: it reads only the file's start.
    result = subprocess.run([ffmpeg, "-hide_banner", "-nostdin", "-i", str(path)], capture_output=True,
                            text=True, encoding="utf-8", errors="replace", **NO_WINDOW)
    return read_report(result.stderr)


def decode_audio(ffmpeg: str, path: Path, target: Path, *, progress: Callable[[float], None],
                 stopped: Callable[[], bool]) -> None:
    """Decodes the file's first audio stream to 16 kHz mono float32 at `target`, saying how many seconds
    of it have been read as it goes. Raises Cancelled, leaving nothing behind, when stopped() says so."""
    command = [ffmpeg, "-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i", str(path),
               "-map", "0:a:0", "-vn", "-sn", "-dn", "-ac", "1", "-ar", str(SAMPLE_RATE), "-f", "f32le",
               "-progress", "pipe:1", "-stats_period", "0.25", str(target)]
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                               encoding="utf-8", errors="replace", **NO_WINDOW)
    errors: list[str] = []
    reader = threading.Thread(target=lambda: errors.extend(process.stderr), daemon=True)
    reader.start()
    try:
        for line in process.stdout:
            if stopped():
                raise Cancelled
            key, _, value = line.strip().partition("=")
            if key == "out_time_us" and value.isdigit():
                progress(int(value) / 1e6)
        process.wait()
        reader.join(timeout=5)
        if stopped():
            raise Cancelled
        if process.returncode != 0:
            raise ImportFailed(f"Reading the file's sound failed: {''.join(errors).strip()[-300:]}")
    except BaseException:
        if process.poll() is None:
            process.kill()
            process.wait()
        target.unlink(missing_ok=True)
        raise
    progress(target.stat().st_size / 4 / SAMPLE_RATE)


def transcribe_file(model, ffmpeg: str, path: Path, folder: Path, *,
                    progress: Callable[[str, float, float], None],
                    lines: Callable[[list[TranscriptionSegment]], None],
                    stopped: Callable[[], bool]) -> ImportResult:
    """The transcript of the file at `path`, made in `folder`, which is left as it was.

    progress(stage, done, total): stage "reading" then "transcribing", with how many seconds of the
    file are done out of its length; lines(segments) gets each chunk's transcript as soon as it is done;
    stopped() is asked between steps, and when it says so, Cancelled is raised.
    """
    path, folder = Path(path), Path(folder)
    info = probe(ffmpeg, path)
    if not info.has_audio:
        raise ImportFailed("This file has no sound to transcribe.")
    audio = folder / AUDIO_NAME
    try:
        progress("reading", 0.0, info.duration)
        decode_audio(ffmpeg, path, audio, stopped=stopped,
                     progress=lambda seconds: progress("reading", seconds, max(info.duration, seconds)))
        total = audio.stat().st_size / 4 / SAMPLE_RATE
        progress("transcribing", 0.0, total)

        def chunk_done(new: list[TranscriptionSegment], seconds: float) -> None:
            if new:
                lines(new)
            progress("transcribing", seconds, total)

        segments = run_final_pass_file(model, audio, chunk_seconds=CHUNK_SECONDS, search_seconds=SEARCH_SECONDS,
                                       on_chunk=chunk_done, stopped=stopped)
        if stopped():
            raise Cancelled
    finally:
        audio.unlink(missing_ok=True)
    return ImportResult(duration=info.duration or total, has_video=info.has_video, segments=segments)
