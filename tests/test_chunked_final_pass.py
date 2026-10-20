from types import SimpleNamespace

import numpy as np

from cronista.engine.final_pass import find_quiet_cut, run_final_pass_file

RATE = 16_000


def tone(seconds: float) -> np.ndarray:
    t = np.arange(int(seconds * RATE)) / RATE
    return (0.2 * np.sin(2 * np.pi * 220 * t)).astype(np.float32)


def test_cuts_in_the_pause_before_the_target():
    # Speech, a 0.5 s pause at 7.0-7.5 s, more speech; target 10 s, searching the 5 s before it.
    audio = np.concatenate([tone(7.0), np.zeros(RATE // 2, np.float32), tone(5.0)])
    cut = find_quiet_cut(audio, target=10 * RATE, search=5 * RATE)
    assert 7.0 * RATE <= cut <= 7.5 * RATE


class FakeTimestampedModel:
    """Says the chunk's length in tokens, so the test can check how the audio was split."""

    def __init__(self):
        self.chunk_lengths = []

    def with_timestamps(self):
        return self

    def recognize(self, audio, sample_rate):
        self.chunk_lengths.append(len(audio) / sample_rate)
        return SimpleNamespace(tokens=[f" chunk{len(self.chunk_lengths)}", "."], timestamps=[0.5, 0.6])


def test_long_audio_is_transcribed_in_chunks_with_shifted_times(tmp_path):
    audio = np.concatenate([tone(4.0), np.zeros(RATE, np.float32)] * 5)  # 25 s with a pause every 5 s
    path = tmp_path / "audio.f32"
    audio.tofile(path)
    model = FakeTimestampedModel()
    segments = run_final_pass_file(model, path, chunk_seconds=8, search_seconds=3)
    assert len(model.chunk_lengths) > 1
    assert abs(sum(model.chunk_lengths) - 25.0) < 0.01  # every sample once, nothing lost or repeated
    starts = [round(segment.start, 2) for segment in segments]
    assert starts[0] == 0.5
    assert all(later > earlier for earlier, later in zip(starts, starts[1:]))
    assert [s.text for s in segments][:2] == ["chunk1.", "chunk2."]


def test_silent_chunks_are_not_sent_to_the_model(tmp_path):
    # The computer's sound is digital silence while nothing plays, and each source is quiet while the
    # other one talks: with one pass per source, skipping those chunks saves most of the second pass.
    audio = np.concatenate([tone(4.0), np.zeros(20 * RATE, np.float32)])
    path = tmp_path / "audio.f32"
    audio.tofile(path)
    model = FakeTimestampedModel()
    segments = run_final_pass_file(model, path, chunk_seconds=8, search_seconds=3)
    assert len(model.chunk_lengths) == 1
    assert [s.text for s in segments] == ["chunk1."]

    silent = tmp_path / "silent.f32"
    np.zeros(10 * RATE, np.float32).tofile(silent)
    model = FakeTimestampedModel()
    assert run_final_pass_file(model, silent) == []
    assert model.chunk_lengths == []


def test_short_audio_is_a_single_chunk(tmp_path):
    path = tmp_path / "audio.f32"
    tone(5.0).tofile(path)
    model = FakeTimestampedModel()
    run_final_pass_file(model, path, chunk_seconds=90)
    assert model.chunk_lengths == [5.0]
