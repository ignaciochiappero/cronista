import numpy as np

from cronista.engine.voice_activity import VoiceActivitySegmenter

RATE = 16_000


def tone(seconds: float, amplitude: float = 0.1) -> np.ndarray:
    t = np.arange(int(seconds * RATE)) / RATE
    return (amplitude * np.sqrt(2) * np.sin(2 * np.pi * 220 * t)).astype(np.float32)  # RMS == amplitude


def silence(seconds: float, amplitude: float = 0.001) -> np.ndarray:
    rng = np.random.default_rng(0)
    return (amplitude * rng.standard_normal(int(seconds * RATE))).astype(np.float32)


def feed(segmenter: VoiceActivitySegmenter, *parts: np.ndarray, block: int = 1600):
    audio = np.concatenate(parts)
    events = []
    for i in range(0, len(audio), block):
        events.extend(segmenter.push(audio[i : i + block]))
    return events


def new_segmenter(**kwargs) -> VoiceActivitySegmenter:
    return VoiceActivitySegmenter(sample_rate=RATE, calibration_seconds=0.5, **kwargs)


def test_silence_produces_no_events():
    assert feed(new_segmenter(), silence(5)) == []


def test_short_phrase_followed_by_silence_is_one_final_utterance():
    events = feed(new_segmenter(), silence(1), tone(0.6), silence(2))
    assert [e.kind for e in events] == ["final"]
    final = events[0]
    assert final.reason == "silence"
    # Starts a little before the speech onset (pre-roll) and keeps at most ~0.3 s of trailing silence.
    assert 0.6 <= final.start_sample / RATE <= 1.0
    assert 0.6 <= len(final.audio) / RATE <= 1.3


def test_long_speech_emits_cumulative_partials_about_every_second():
    events = feed(new_segmenter(), silence(1), tone(2.6))
    partials = [e for e in events if e.kind == "partial"]
    assert len(partials) == 2
    # Each partial carries the whole utterance so far, like a streaming recognizer's hypothesis.
    assert len(partials[1].audio) > len(partials[0].audio)
    assert partials[0].start_sample == partials[1].start_sample


def test_speech_longer_than_the_cap_is_cut_into_utterances():
    events = feed(new_segmenter(max_utterance_seconds=8), silence(1), tone(11), silence(2))
    finals = [e for e in events if e.kind == "final"]
    assert [f.reason for f in finals] == ["cap", "silence"]
    assert len(finals[0].audio) / RATE <= 8.3
    # The second utterance starts where the first one was cut: no audio lost or repeated.
    assert finals[1].start_sample == finals[0].end_sample


def test_steady_background_noise_above_the_fixed_threshold_is_learned_during_calibration():
    noisy = silence(6, amplitude=0.05)  # RMS 0.05 > the fixed 0.015 threshold
    assert feed(new_segmenter(), noisy) == []


def test_flush_finalizes_an_utterance_still_in_progress():
    segmenter = new_segmenter()
    feed(segmenter, silence(1), tone(0.5))
    events = segmenter.flush()
    assert [e.kind for e in events] == ["final"]
    assert events[0].reason == "end"


def test_the_default_cap_lets_a_14_second_phrase_through_uncut():
    # The first live run cut continuous speech at 8 s and lost words at each cut.
    events = feed(new_segmenter(), silence(1), tone(14), silence(2))
    finals = [e for e in events if e.kind == "final"]
    assert [f.reason for f in finals] == ["silence"]
