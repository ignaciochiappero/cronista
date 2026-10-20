"""Definitive transcript: one pass of the model over the whole recording, after it ends.

The live text has to cut long utterances to stay fast, and it can lose a word at each cut. In
the first run a single pass over all 55 s took 3 s and had none of those errors, so the live
text is for watching and this pass is the transcript that gets saved.

Timing comes from onnx-asr's token timestamps (`with_timestamps()`, 0.08 s resolution).
"""
from __future__ import annotations

import re
from pathlib import Path

import numpy as np

from .transcript import TranscriptionSegment, tidy_text

# Same rule as onnx_asr.asr._AsrWithDecoding.DECODE_SPACE_PATTERN: tokens carry their own leading
# space, the first one is dropped, and so is any space before a symbol.
_DECODE_SPACE = re.compile(r"\A\s|\s\B|(\s)\b")
_SENTENCE_END = (".", "?", "!", "…")
# A token that only closes something ("." "," "?" ")" ...) belongs to the words before it, even
# when the model emits it after a pause. Opening marks such as "¿" start the next sentence.
_CLOSING_ONLY = re.compile(r"^\s*[.,;:!?…)\]»\"']+\s*$")


def _join(tokens: list[str]) -> str:
    return _DECODE_SPACE.sub(lambda match: " " if match.group(1) else "", "".join(tokens))


def segments_from_tokens(
    tokens: list[str],
    timestamps: list[float],
    audio_seconds: float,
    *,
    pause_seconds: float = 0.8,
    hold_seconds: float = 1.0,
) -> list[TranscriptionSegment]:
    """Groups tokens into subtitle segments: one per sentence, or per stretch between long pauses."""
    groups: list[list[int]] = []
    current: list[int] = []
    for i, token in enumerate(tokens):
        closing = bool(_CLOSING_ONLY.match(token))
        if closing and not current and groups:
            groups[-1].append(i)
            continue
        if current and not closing and timestamps[i] - timestamps[i - 1] > pause_seconds:
            groups.append(current)
            current = []
        current.append(i)
        if token.strip().endswith(_SENTENCE_END):
            groups.append(current)
            current = []
    if current:
        groups.append(current)

    segments: list[TranscriptionSegment] = []
    for n, group in enumerate(groups):
        next_start = timestamps[groups[n + 1][0]] if n + 1 < len(groups) else audio_seconds
        end = min(next_start, timestamps[group[-1]] + hold_seconds)
        text = tidy_text(_join([tokens[i] for i in group]).strip())
        if text:
            segments.append(TranscriptionSegment(timestamps[group[0]], end, text))
    return segments


def run_final_pass(model, audio: np.ndarray, sample_rate: int = 16_000) -> list[TranscriptionSegment]:
    result = model.with_timestamps().recognize(audio, sample_rate=sample_rate)
    return segments_from_tokens(list(result.tokens or []), list(result.timestamps or []), len(audio) / sample_rate)


def find_quiet_cut(audio: np.ndarray, target: int, search: int, frame: int = 1_600) -> int:
    """The middle of the quietest 100 ms frame in the `search` samples before `target`."""
    low = max(0, target - search)
    window = np.asarray(audio[low:target], dtype=np.float32)
    frames = len(window) // frame
    if frames == 0:
        return target
    energy = np.square(window[: frames * frame].reshape(frames, frame)).mean(axis=1)
    return low + int(np.argmin(energy)) * frame + frame // 2


def loudest(audio: np.ndarray, frame: int = 1_600) -> float:
    """The RMS level of the loudest 100 ms frame."""
    frames = len(audio) // frame
    if frames == 0:
        return float(np.sqrt(np.mean(np.square(audio, dtype=np.float64)))) if len(audio) else 0.0
    energy = np.square(audio[: frames * frame].reshape(frames, frame), dtype=np.float64).mean(axis=1)
    return float(np.sqrt(energy.max()))


def run_final_pass_file(model, path, sample_rate: int = 16_000, *, chunk_seconds: float = 90.0,
                        search_seconds: float = 10.0, silence: float = 0.002) -> list[TranscriptionSegment]:
    """The final pass over a recording's raw 16 kHz float32 audio file, in pause-aligned chunks.

    One pass over an hour of audio does not fit in memory (the encoder's attention grows with the
    square of the length), so the audio goes in chunks of about chunk_seconds, each cut at the
    quietest moment of the last search_seconds before its end so no word is split. Chunks are
    read from disk one at a time; nothing else of the recording is kept in RAM.

    A chunk whose loudest moment is below `silence` is not sent to the model: the computer's sound
    is digital silence while nothing plays, and each source is quiet while the other one talks.
    """
    total = Path(path).stat().st_size // 4
    chunk = int(chunk_seconds * sample_rate)
    search = int(search_seconds * sample_rate)
    segments: list[TranscriptionSegment] = []
    start = 0
    while start < total:
        if total - start <= chunk + search:
            end = total
        else:
            window_start = start + chunk - search
            window = np.fromfile(path, dtype=np.float32, count=search, offset=window_start * 4)
            end = window_start + find_quiet_cut(window, target=len(window), search=search)
        audio = np.fromfile(path, dtype=np.float32, count=end - start, offset=start * 4)
        offset = start / sample_rate
        if loudest(audio) < silence:
            start = end
            continue
        for segment in run_final_pass(model, audio, sample_rate):
            segments.append(TranscriptionSegment(segment.start + offset, segment.end + offset, segment.text))
        start = end
    return segments
