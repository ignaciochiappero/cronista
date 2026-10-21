"""High quality: Canary 1B v2 writes the words again, on the times Parakeet's pass gave them."""
import numpy as np
import pytest

from cronista.engine.canary_pass import CanaryPass, guess_language, spaced
from cronista.engine.final_pass import _join, run_final_pass

RATE = 16_000
SPANISH = ["de", "la", "reunión", "que", "tenemos", "el", "jueves", "con", "todo", "equipo"]


def test_the_language_is_told_by_its_little_words():
    assert guess_language("Bueno, entonces la reunión es el jueves con todo el equipo de diseño") == "es"
    assert guess_language("So the meeting is on Thursday with the whole design team, right?") == "en"
    assert guess_language("OK.") is None  # too little to tell
    assert guess_language("Bonjour à tous, merci d'être venus ici") is None  # neither: Parakeet's text stays


def test_an_opening_mark_gets_the_space_canary_leaves_out():
    assert spaced("Son 1.200 dólares.¿Quién se encarga? Poco,¡ya!") == "Son 1.200 dólares. ¿Quién se encarga? Poco, ¡ya!"


class FakeCanary:
    """Answers each piece of audio with the next of `answers` (a function of the piece's Parakeet text
    when it is callable), and keeps how long each piece was and the language it was asked for."""

    def __init__(self, *answers):
        self.answers = list(answers)
        self.pieces: list[tuple[float, str]] = []

    def recognize(self, audio, sample_rate, language):
        self.pieces.append((len(audio) / sample_rate, language))
        return self.answers.pop(0) if self.answers else ""


def speech(seconds: float, pauses=()) -> np.ndarray:
    """A tone, silent during each (start, end) of `pauses`."""
    t = np.arange(int(seconds * RATE)) / RATE
    audio = (0.2 * np.sin(2 * np.pi * 220 * t)).astype(np.float32)
    for start, end in pauses:
        audio[int(start * RATE):int(end * RATE)] = 0
    return audio


def parakeet(words_and_times) -> tuple[list[str], list[float]]:
    """Parakeet's tokens for (word, time) pairs: a leading space starts a word; "grab|ando" is two tokens."""
    tokens, times = [], []
    for word, time in words_and_times:
        for n, piece in enumerate(word.split("|")):
            tokens.append((" " if n == 0 else "") + piece)
            times.append(time + 0.08 * n)
    return tokens, times


def test_canary_hears_pieces_cut_in_the_pauses_never_longer_than_its_limit():
    pauses = [(19.0, 20.5), (39.0, 40.5)]
    words = [(SPANISH[n % len(SPANISH)], n * 0.5) for n in range(120)
             if not any(start <= n * 0.5 < end for start, end in pauses)]
    tokens, times = parakeet(words)
    canary = FakeCanary()
    CanaryPass(canary)(speech(60, pauses), tokens, times, RATE)
    lengths = [length for length, _ in canary.pieces]
    assert sum(lengths) == pytest.approx(60.0)  # every sample once, nothing lost or heard twice
    ends = np.cumsum(lengths)[:-1]
    assert 19.0 <= ends[0] <= 20.5 and 39.0 <= ends[1] <= 40.5  # cut where nobody spoke
    assert max(lengths) <= 24.0
    assert {language for _, language in canary.pieces} == {"es"}


def test_canarys_words_take_the_times_of_the_words_they_stand_for():
    tokens, times = parakeet([("Estoy", 0.4), ("grab|ando", 0.8), ("la", 1.3), ("pantalla", 1.5),
                              ("mientro,", 2.1), ("y", 2.9), ("el", 3.1), ("sistema", 3.3), ("que", 3.8),
                              ("tenemos.", 4.0)])
    canary = FakeCanary("Estoy grabando la pantalla mientras hablo, y el sistema que tenemos.")
    words, at = CanaryPass(canary)(speech(6), tokens, times, RATE)
    assert _join(words) == "Estoy grabando la pantalla mientras hablo, y el sistema que tenemos."
    timed = dict(zip([word.strip() for word in words], at))
    assert timed["Estoy"] == 0.4 and timed["grabando"] == 0.8 and timed["y"] == 2.9
    assert timed["mientras"] == pytest.approx(2.1)  # in place of "mientro,"
    assert 2.1 < timed["hablo,"] < 2.9  # heard only by Canary: between its neighbours
    assert at == sorted(at)


@pytest.mark.parametrize("answer", [
    "",  # nothing
    "So the meeting is on Thursday with the whole team.",  # a translation: the language was wrong
    "de la reunión que tenemos " * 12,  # a loop
])
def test_a_piece_canary_gets_wrong_keeps_parakeets_words(answer):
    tokens, times = parakeet([(word, 0.5 + n * 0.4) for n, word in enumerate(SPANISH)])
    words, at = CanaryPass(FakeCanary(answer))(speech(6), tokens, times, RATE)
    assert (words, at) == (tokens, times)


def test_a_piece_canary_fails_on_keeps_parakeets_words_and_the_recording_goes_on():
    class Failing:
        def recognize(self, audio, sample_rate, language):
            raise RuntimeError("onnxruntime: bad input")

    tokens, times = parakeet([(word, 0.5 + n * 0.4) for n, word in enumerate(SPANISH)])
    assert CanaryPass(Failing())(speech(6), tokens, times, RATE) == (tokens, times)


def test_a_language_it_cannot_tell_leaves_the_text_alone_and_canary_unasked():
    tokens, times = parakeet([("Bonjour", 0.5), ("à", 0.9), ("tous,", 1.1), ("merci", 1.6), ("d'être", 2.0),
                              ("venus", 2.4), ("ici.", 2.8)])
    canary = FakeCanary("Bonjour à tous.")
    assert CanaryPass(canary)(speech(4), tokens, times, RATE) == (tokens, times)
    assert canary.pieces == []


def test_the_language_carries_over_to_a_piece_too_short_to_tell():
    canary = FakeCanary("de la reunión que tenemos el jueves con todo equipo", "Sí.")
    refine = CanaryPass(canary)
    refine(speech(6), *parakeet([(word, 0.5 + n * 0.4) for n, word in enumerate(SPANISH)]), RATE)
    words, _ = refine(speech(2), *parakeet([("Si.", 0.5)]), RATE)
    assert [language for _, language in canary.pieces] == ["es", "es"]
    assert _join(words) == "Sí."


def test_speech_without_a_pause_is_still_cut_in_pieces_canary_can_take():
    words = [(SPANISH[n % len(SPANISH)], n * 0.15) for n in range(400)]  # 60 s with no gap of 0.2 s
    canary = FakeCanary()
    CanaryPass(canary)(speech(60), *parakeet(words), RATE)
    assert max(length for length, _ in canary.pieces) <= 24.0
    assert sum(length for length, _ in canary.pieces) == pytest.approx(60.0)


def test_a_word_at_the_very_end_of_the_audio_is_kept():
    tokens, times = parakeet([(word, 0.5 + n * 0.4) for n, word in enumerate(SPANISH)] + [("fin.", 6.0)])
    words, _ = CanaryPass(FakeCanary(""))(speech(6), tokens, times, RATE)
    assert words == tokens


def test_quiet_stretches_without_words_are_not_sent_to_canary():
    words = [(SPANISH[n % len(SPANISH)], 0.5 + n * 0.4) for n in range(10)]  # 0.5-4.1 s, then silence
    canary = FakeCanary("de la reunión que tenemos el jueves con todo equipo")
    CanaryPass(canary)(speech(50, [(5.0, 50.0)]), *parakeet(words), RATE)
    assert len(canary.pieces) == 1


class FakeParakeet:
    def with_timestamps(self):
        return self

    def recognize(self, audio, sample_rate):
        from types import SimpleNamespace
        return SimpleNamespace(tokens=[" mientro", "."], timestamps=[0.5, 0.9])


def test_the_final_pass_lets_a_refiner_rewrite_the_words_before_they_become_lines():
    def refine(audio, tokens, timestamps, sample_rate):
        assert (tokens, timestamps) == ([" mientro", "."], [0.5, 0.9])
        return [" mientras", " hablo."], [0.5, 0.9]

    segments = run_final_pass(FakeParakeet(), speech(2), RATE, refine=refine)
    assert [(s.start, s.text) for s in segments] == [(0.5, "mientras hablo.")]
