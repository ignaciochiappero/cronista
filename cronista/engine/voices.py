"""Who said it: the microphone (the person recording) or the computer's sound (a call, a video).

The two are transcribed apart (recorders.py streams them to the model on two channels), so every
phrase knows its source. With speakers instead of headphones the microphone also hears what the
computer plays, about 40 ms later and softer, and transcribes some of the same words: those echoes
are dropped. Labels ("Me", "Ana") are the names the user gives the sources; they are only shown
when both sources said something, since with one there is nobody to tell apart.
"""
from __future__ import annotations

import re

from .transcript import TranscriptionSegment

SOURCES = ("mic", "computer")  # the speech stream's channels, in order
_WORD = re.compile(r"\w+")


def _words(text: str) -> list[str]:
    return _WORD.findall(text.lower())


def is_echo(mic: TranscriptionSegment, computer: list[TranscriptionSegment], *, share: float = 0.6,
            margin: float = 1.0) -> bool:
    """Whether most of the words the microphone heard are what the computer played at that moment."""
    words = _words(mic.text)
    if not words:
        return False
    played: set[str] = set()
    for segment in computer:
        if segment.start - margin <= mic.end and mic.start <= segment.end + margin:
            played.update(_words(segment.text))
    return sum(word in played for word in words) / len(words) >= share


def drop_echoes(segments: list[TranscriptionSegment], *, share: float = 0.6,
                margin: float = 1.0) -> list[TranscriptionSegment]:
    """The transcript without the microphone's echoes of the computer, in the order things were said."""
    computer = [segment for segment in segments if segment.source == "computer"]
    kept = [segment for segment in segments
            if segment.source != "mic" or not is_echo(segment, computer, share=share, margin=margin)]
    return sorted(kept, key=lambda segment: segment.start)


def speakers(segments: list[TranscriptionSegment], labels: dict | None) -> dict[str, str] | None:
    """Each source's label when more than one source spoke; None when there is nobody to tell apart."""
    sources = {segment.source for segment in segments if segment.source}
    if len(sources) < 2 or not labels:
        return None
    return {source: labels.get(source) or source for source in sources}


def line(segment: TranscriptionSegment, names: dict[str, str] | None) -> str:
    """A phrase as a line of text: "Ana: Hello." when the transcript has speakers to tell apart."""
    if names and segment.source in names:
        return f"{names[segment.source]}: {segment.text}"
    return segment.text
