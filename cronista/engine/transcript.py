"""Transcript segments and SRT output.

`TranscriptionSegment` is the same shape as escriba's (backend/app/services/transcription.py),
and `format_srt_timestamp` extends escriba's DocumentService._format_timestamp (HH:MM:SS)
with the milliseconds that SRT requires.
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from difflib import SequenceMatcher

# onnx-asr joins tokens by dropping every space before a symbol, so "hola. ¿Qué" comes out as
# "hola.¿Qué". Put the space back unless the mark follows an opening bracket or quote.
_GLUED_OPENING_MARK = re.compile(r"(?<=[^\s(\[{«\"'¿¡])([¿¡])")


@dataclass
class TranscriptionSegment:
    start: float
    end: float
    text: str


def tidy_text(text: str) -> str:
    return _GLUED_OPENING_MARK.sub(r" \1", text)


def word_difference(reference: str, hypothesis: str) -> float:
    """Share of the reference's words that the hypothesis gets wrong (word error rate).

    Case and punctuation are ignored. Used to compare the live text against the final pass.
    """
    ref = re.findall(r"\w+", reference.lower())
    hyp = re.findall(r"\w+", hypothesis.lower())
    if not ref:
        return 0.0 if not hyp else 1.0
    edits = 0
    for tag, i1, i2, j1, j2 in SequenceMatcher(None, ref, hyp, autojunk=False).get_opcodes():
        if tag == "replace":
            edits += max(i2 - i1, j2 - j1)
        elif tag == "delete":
            edits += i2 - i1
        elif tag == "insert":
            edits += j2 - j1
    return edits / len(ref)


def format_srt_timestamp(seconds: float) -> str:
    total_ms = int(round(max(seconds, 0.0) * 1000))
    hours, rest = divmod(total_ms, 3_600_000)
    minutes, rest = divmod(rest, 60_000)
    secs, millis = divmod(rest, 1000)
    return f"{hours:02d}:{minutes:02d}:{secs:02d},{millis:03d}"


def to_srt(segments: list[TranscriptionSegment]) -> str:
    blocks = []
    for segment in segments:
        text = segment.text.strip()
        if not text:
            continue
        timing = f"{format_srt_timestamp(segment.start)} --> {format_srt_timestamp(segment.end)}"
        blocks.append(f"{len(blocks) + 1}\n{timing}\n{text}\n")
    return "\n".join(blocks)
