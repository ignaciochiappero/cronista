"""A copy of a recording with its transcript burned in as subtitles: recording-subtitled.mp4.

The transcript becomes an ASS file, which ffmpeg's subtitles filter (libass) draws onto the picture:
white text on one dark box per line at the bottom and, when both audio sources spoke, each line
starting with its speaker's label in that source's color. The picture is re-encoded through Media
Foundation, on the graphics card like the recording itself; the sound is copied as it is.

The screen recording only has frames when the screen changed, so a subtitle that starts during a
still moment would wait for the next change to be drawn. The copy is made at a steady 30 frames per
second (the recording's own maximum) so each subtitle shows when it is said.
"""
from __future__ import annotations

import subprocess
from pathlib import Path

from .engine.ffmpeg_locator import NO_WINDOW
from .engine.transcript import TranscriptionSegment

SOURCE_NAME = "recording.mp4"
SUBTITLED_NAME = "recording-subtitled.mp4"
ASS_NAME = "subtitles.ass"  # next to the video while the copy is made, then removed
# The labels' colors, as ASS writes them (&HAABBGGRR): the page's red for the microphone, blue for the computer.
LABEL_COLORS = {"mic": "&H007F8AFF&", "computer": "&H00FFB48A&"}
WHITE = "&H00FFFFFF&"

# Scaled by libass from a 1080-line canvas to the video's size. BorderStyle 4 draws one box per
# line (style 3 drew one per run of text, darker where the label's and the text's overlapped); the
# outline's width is the box's padding and its color is fully transparent; the box is BackColour.
HEADER = """[Script Info]
ScriptType: v4.00+
PlayResX: 1920
PlayResY: 1080
WrapStyle: 0
ScaledBorderAndShadow: yes

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, \
Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, \
MarginV, Encoding
Style: Default,Segoe UI,46,&H00FFFFFF,&H00FFFFFF,&HFF000000,&H48000000,0,0,0,0,100,100,0,0,4,14,0,2,160,160,64,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
"""


def _time(seconds: float) -> str:
    centiseconds = int(round(max(seconds, 0.0) * 100))
    hours, rest = divmod(centiseconds, 360_000)
    minutes, rest = divmod(rest, 6_000)
    secs, cents = divmod(rest, 100)
    return f"{hours}:{minutes:02d}:{secs:02d}.{cents:02d}"


def _plain(text: str) -> str:
    # In ASS, braces open and close override blocks and a backslash starts a command.
    return " ".join(text.replace("{", "(").replace("}", ")").replace("\\", "/").split())


def ass_document(segments: list[TranscriptionSegment], names: dict[str, str] | None) -> str:
    """The subtitles, one line per phrase; with names (voices.speakers), each starts with its speaker."""
    lines = [HEADER]
    for segment in segments:
        text = _plain(segment.text)
        if not text:
            continue
        if names and segment.source in names:
            color = LABEL_COLORS.get(segment.source, WHITE)
            text = rf"{{\c{color}\b1}}{_plain(names[segment.source])}:{{\c{WHITE}\b0}} {text}"
        lines.append(f"Dialogue: 0,{_time(segment.start)},{_time(segment.end)},Default,,0,0,0,,{text}\n")
    return "".join(lines)


def burn_subtitles(ffmpeg: str, folder: Path, segments: list[TranscriptionSegment],
                   names: dict[str, str] | None, *, quality: int = 70, source: Path | None = None) -> Path:
    """Writes recording-subtitled.mp4 next to recording.mp4 and returns its path.

    With `source`, a video the user dropped (any format, wherever it is), the copy is written in
    `folder`, Cronista's, as "<its name>-subtitled.mp4", and the video itself is only read: at its own
    frame rate, its size rounded to even numbers for H.264, and its sound in AAC, which an MP4 holds.
    """
    folder = Path(folder)
    if source is None:
        video, output, frames, sound = SOURCE_NAME, SUBTITLED_NAME, "fps=30,", ["-c:a", "copy"]
        tuning = ["-scenario", "display_remoting"]  # the encoder's setting for screen content
    else:
        video, output = str(source), f"{Path(source).stem}-subtitled.mp4"
        frames, sound = "scale=trunc(iw/2)*2:trunc(ih/2)*2,", ["-c:a", "aac", "-b:a", "160k"]
        tuning = []
    if not (folder / video).exists():  # an absolute source stays itself when joined to the folder
        raise RuntimeError("The video file is no longer there.")
    ass = folder / ASS_NAME
    ass.write_text(ass_document(segments, names), encoding="utf-8")
    try:
        # Run in the recording's folder so the filter gets a bare file name: a Windows path would
        # need its drive's colon and its backslashes escaped twice, for the filter and for its option.
        result = subprocess.run(
            [ffmpeg, "-hide_banner", "-loglevel", "error", "-y", "-i", video, "-map", "0:v:0", "-map", "0:a:0?",
             "-vf", f"{frames}subtitles={ASS_NAME},format=nv12",
             "-c:v", "h264_mf", "-hw_encoding", "1", "-rate_control", "quality", "-quality", str(quality),
             *tuning, "-profile:v", "100", "-g", "300", *sound, "-movflags", "+faststart", output],
            cwd=folder, capture_output=True, text=True, encoding="utf-8", errors="replace", **NO_WINDOW)
    finally:
        ass.unlink(missing_ok=True)
    if result.returncode != 0:
        raise RuntimeError(f"Making the subtitled video failed: {result.stderr.strip()[-300:]}")
    return folder / output
