# Copyright 2026 Google LLC
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
#
# Ported to Python from bavel, with the changes listed below.
"""Energy-based voice activity segmentation for live transcription.

Ported from bavel: frontend/src/hooks/useTabAudioCapture.js. It keeps bavel's RMS
threshold (0.015), its ~85 ms frames, an update every ~1 s of continuous speech, and the
end of an utterance after ~1.2 s of silence.

Changes, each for something bavel did not need:
- Calibration: the first second sets the threshold from the room's noise floor, because
  a laptop mic in an ordinary room can sit above bavel's fixed 0.015.
- Pre-roll: keeps ~0.2 s before the first loud frame, so soft onsets are not clipped.
- Cap: an utterance with no pause is cut at its quietest frame after 15 s, because
  re-transcribing a growing buffer gets slower with its length. The first live run used 8 s
  and cut four sentences, losing a word at most cuts; the model turned out fast enough for 15 s.
- Cumulative partials: bavel streamed each chunk into Moonshine. Parakeet does not
  stream, so every partial carries the whole utterance so far and gets re-transcribed.
- Minimum length: a click or a cough shorter than 0.2 s of speech is dropped.
"""
from __future__ import annotations

import math
from collections import deque
from dataclasses import dataclass

import numpy as np


@dataclass(frozen=True)
class VoiceEvent:
    kind: str  # "partial" while the utterance goes on, "final" when it ended
    audio: np.ndarray  # the whole utterance so far, float32 mono
    start_sample: int  # stream position of audio[0]
    speech_end_sample: int = -1  # stream position where the last speech frame ended
    reason: str = ""  # finals only: "silence" (a pause), "cap" (too long, cut), "end" (stream ended)

    @property
    def end_sample(self) -> int:
        return self.start_sample + len(self.audio)


def _rms(frame: np.ndarray) -> float:
    if len(frame) == 0:
        return 0.0
    return float(np.sqrt(np.mean(np.square(frame, dtype=np.float64))))


class VoiceActivitySegmenter:
    def __init__(
        self,
        sample_rate: int = 16_000,
        *,
        frame_seconds: float = 0.085,
        speech_threshold: float = 0.015,
        silence_flush_seconds: float = 1.2,
        partial_every_seconds: float = 1.0,
        max_utterance_seconds: float = 15.0,
        calibration_seconds: float = 1.0,
        noise_multiplier: float = 2.0,
        max_threshold: float = 0.08,
        pre_roll_seconds: float = 0.2,
        keep_trailing_silence_seconds: float = 0.3,
        min_speech_seconds: float = 0.2,
        cut_search_seconds: float = 2.0,
    ) -> None:
        self.sample_rate = sample_rate
        self.frame_size = int(round(sample_rate * frame_seconds))

        def frames(seconds: float) -> int:
            return max(1, math.ceil(seconds / frame_seconds))

        self._silence_flush_frames = frames(silence_flush_seconds)
        self._partial_every_frames = frames(partial_every_seconds)
        self._max_utterance_frames = int(max_utterance_seconds / frame_seconds)
        self._calibration_frames = frames(calibration_seconds) if calibration_seconds > 0 else 0
        self._keep_trailing_frames = int(keep_trailing_silence_seconds / frame_seconds)
        self._min_speech_frames = frames(min_speech_seconds)
        self._cut_search_frames = frames(cut_search_seconds)
        self._noise_multiplier = noise_multiplier
        self._min_threshold = speech_threshold
        self._max_threshold = max_threshold
        self.threshold = speech_threshold

        self._leftover = np.empty(0, dtype=np.float32)
        self._next_frame_start = 0
        self._calibration_rms: list[float] = []
        self._pre_roll: deque[tuple[int, np.ndarray, float]] = deque(maxlen=frames(pre_roll_seconds))
        self._reset_utterance()

    @property
    def calibrating(self) -> bool:
        return len(self._calibration_rms) < self._calibration_frames

    def push(self, samples: np.ndarray) -> list[VoiceEvent]:
        data = np.concatenate([self._leftover, np.asarray(samples, dtype=np.float32)])
        count = len(data) // self.frame_size
        events: list[VoiceEvent] = []
        for i in range(count):
            events.extend(self._on_frame(data[i * self.frame_size : (i + 1) * self.frame_size]))
        self._leftover = data[count * self.frame_size :]
        return events

    def flush(self) -> list[VoiceEvent]:
        """End of stream: finalizes the utterance in progress, if any."""
        if self._start is None:
            return []
        if len(self._leftover):
            self._frames.append(self._leftover)
            self._rms.append(_rms(self._leftover))
            self._leftover = np.empty(0, dtype=np.float32)
        return self._finalize(trim_trailing=False, reason="end")

    def _reset_utterance(self) -> None:
        self._start: int | None = None
        self._frames: list[np.ndarray] = []
        self._rms: list[float] = []
        self._speech_frames = 0
        self._speech_since_partial = 0
        self._silence_run = 0
        self._speech_end = -1

    def _on_frame(self, frame: np.ndarray) -> list[VoiceEvent]:
        start = self._next_frame_start
        self._next_frame_start += len(frame)
        rms = _rms(frame)

        if self.calibrating:
            self._calibration_rms.append(rms)
            if not self.calibrating:
                # 20th percentile: robust to someone talking during part of the calibration.
                floor = float(np.percentile(self._calibration_rms, 20))
                self.threshold = min(max(self._min_threshold, floor * self._noise_multiplier), self._max_threshold)
            self._pre_roll.append((start, frame, rms))
            return []

        is_speech = rms > self.threshold
        if self._start is None:
            if not is_speech:
                self._pre_roll.append((start, frame, rms))
                return []
            self._start = self._pre_roll[0][0] if self._pre_roll else start
            for _, pre_frame, pre_rms in self._pre_roll:
                self._frames.append(pre_frame)
                self._rms.append(pre_rms)
            self._pre_roll.clear()

        self._frames.append(frame)
        self._rms.append(rms)
        if is_speech:
            self._silence_run = 0
            self._speech_frames += 1
            self._speech_since_partial += 1
            self._speech_end = start + len(frame)
        else:
            self._silence_run += 1
            if self._silence_run >= self._silence_flush_frames:
                return self._finalize(trim_trailing=True, reason="silence")

        # Only while speech goes on: during a pause the silence flush is about to close it anyway.
        if is_speech and len(self._frames) >= self._max_utterance_frames:
            return self._cut_at_quietest()
        if is_speech and self._speech_since_partial >= self._partial_every_frames:
            self._speech_since_partial = 0
            return [VoiceEvent("partial", np.concatenate(self._frames), self._start, self._speech_end)]
        return []

    def _finalize(self, *, trim_trailing: bool, reason: str) -> list[VoiceEvent]:
        frames = self._frames
        if trim_trailing and self._silence_run:
            frames = frames[: len(frames) - self._silence_run + self._keep_trailing_frames]
        event = None
        if self._speech_frames >= self._min_speech_frames:
            event = VoiceEvent("final", np.concatenate(frames), self._start, self._speech_end, reason)
        self._reset_utterance()
        return [event] if event else []

    def _cut_at_quietest(self) -> list[VoiceEvent]:
        """Cuts a pause-less utterance at its quietest frame in the last few seconds."""
        assert self._start is not None
        window_start = max(1, len(self._frames) - self._cut_search_frames)
        cut = window_start + int(np.argmin(self._rms[window_start:]))
        head, tail = self._frames[:cut], self._frames[cut:]
        head_rms, tail_rms = self._rms[:cut], self._rms[cut:]
        head_samples = sum(len(f) for f in head)

        events: list[VoiceEvent] = []
        if sum(r > self.threshold for r in head_rms) >= self._min_speech_frames:
            head_speech_end = self._start + head_samples
            events.append(VoiceEvent("final", np.concatenate(head), self._start, head_speech_end, "cap"))

        self._start += head_samples
        self._frames, self._rms = tail, tail_rms
        self._speech_frames = sum(r > self.threshold for r in tail_rms)
        self._speech_since_partial = 0
        trailing_silence = 0
        for r in reversed(tail_rms):
            if r > self.threshold:
                break
            trailing_silence += 1
        self._silence_run = trailing_silence
        return events
