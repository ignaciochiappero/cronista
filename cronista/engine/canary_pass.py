"""High quality: Canary 1B v2 writes the words of Parakeet's final pass again.

Canary (NVIDIA, 1B parameters, the same 25 languages as Parakeet) makes fewer mistakes in Spanish,
but three things keep it from replacing Parakeet outright. onnx-asr gives no timestamps for it, and
the subtitles need them. It has to be told the language: without one it assumes English, and it
translated a Spanish sample into English. And on long audio it can loop: on 2 minutes of real speech
in one go it came back with twice the words.

So Parakeet's pass still runs: its words say when each one was said and, through their little words,
which language it is. Canary then hears the audio again in pieces of up to PIECE_SECONDS, cut in the
pauses between Parakeet's words, and its words take the times of the words they stand for. A piece
whose words have little to do with Parakeet's (nothing, a translation, a loop) keeps Parakeet's.
"""
from __future__ import annotations

import re
import traceback
import unicodedata
from difflib import SequenceMatcher

import numpy as np

from .final_pass import _join, find_quiet_cut

PIECE_SECONDS = 24.0  # the longest piece Canary hears: in 24 s pieces, 2 minutes came back right
SHORTEST_PIECE = 12.0  # a piece ends in the longest pause between this and PIECE_SECONDS
AGREEMENT = 0.5  # below this share of words in common with Parakeet, a piece of Canary's is not used
# Without accents, as _plain() leaves them. Words both languages use ("no", "me", "he") count for neither.
_SPANISH = frozenset("""de la que el en y los se del las un por con una su para es al lo como mas pero sus
    le ya este si porque esta entre cuando muy sin sobre tambien hasta hay donde quien desde todo nos durante
    todos uno les ni contra otros ese eso ellos esto mi antes algunos unos yo otro otras otra tanto esa estos
    mucho nada muchos cual poco ella estar estas algo nosotros mis tu te ti vos estoy estan bueno pues entonces
    aca asi hola gracias vamos tenemos hacer puede creo dale""".split())
_ENGLISH = frozenset("""the of and to in is you that it was for on are as with his they at be this have from
    or had by but not what all were we when your can there an which she do how their if will up about out many
    then them these so would like more could just yeah okay right think know going get really well here now
    because our my been did does its thats dont im were youre theres lets""".split())
_OPENING = re.compile(r"(?<=\S)([¿¡])")


def _plain(word: str) -> str:
    """Lower case, without accents or punctuation: "Sí," and "si" are the same word, "1.200" and "1,200" too."""
    decomposed = unicodedata.normalize("NFD", word.lower())
    return "".join(c for c in decomposed if unicodedata.category(c)[0] not in "MP")


def guess_language(text: str) -> str | None:
    """"es" or "en" from how many of each language's little words the text has; None when it cannot tell."""
    words = [_plain(word) for word in text.split()]
    spanish = sum(word in _SPANISH for word in words)
    english = sum(word in _ENGLISH for word in words)
    if spanish + english < 3:
        return None
    if spanish >= 2 * english:
        return "es"
    if english >= 2 * spanish:
        return "en"
    return None


def spaced(text: str) -> str:
    """Canary writes "dólares.¿Quién": the opening mark of a question or an exclamation gets its space."""
    return _OPENING.sub(r" \1", text)


def _words(tokens: list[str], timestamps: list[float]) -> list[tuple[int, int, float]]:
    """(first token, end token, time) of each word: a token with a leading space starts one."""
    words: list[list] = []
    for i, token in enumerate(tokens):
        if not words or token.startswith(" "):
            words.append([i, i + 1, timestamps[i]])
        else:
            words[-1][1] = i + 1
    return [tuple(word) for word in words]


def _pieces(audio: np.ndarray, times: list[float], sample_rate: int) -> list[tuple[int, int]]:
    """Where Canary's pieces of the audio begin and end, in samples. Each but the last ends in the
    quietest moment of the longest pause between words (or before the first, or after the last)
    that falls between SHORTEST_PIECE and PIECE_SECONDS from its start."""
    total = len(audio) / sample_rate
    marks = [0.0, *times, total]
    pieces, start = [], 0.0
    while total - start > PIECE_SECONDS:
        # (pause, earliest cut, latest cut); with no pause at all, the quietest moment of the stretch
        best = (0.0, start + SHORTEST_PIECE, start + PIECE_SECONDS)
        for before, after in zip(marks, marks[1:]):
            earliest, latest = max(before + 0.2, start + SHORTEST_PIECE), min(after, start + PIECE_SECONDS)
            if earliest < latest and after - before > best[0]:
                best = (after - before, earliest, latest)
        _, earliest, latest = best
        cut = find_quiet_cut(audio, target=int(latest * sample_rate), search=int((latest - earliest) * sample_rate))
        pieces.append((int(start * sample_rate), cut))
        start = cut / sample_rate
    pieces.append((int(start * sample_rate), len(audio)))
    return pieces


def _timed(opcodes, times: list[float], count: int, begin: float, end: float) -> list[float]:
    """A time for each of Canary's `count` words, from Parakeet's word `times` through the alignment:
    a word that stands for Parakeet's takes its time, several spread over the words they replace,
    and a word only Canary heard goes between its neighbours."""
    timed: list[float | None] = [None] * count
    for tag, i1, i2, j1, j2 in opcodes:
        if tag in ("equal", "replace"):
            for k in range(j1, j2):
                position = i1 + (k - j1) * (i2 - i1) / (j2 - j1)
                whole = int(position)
                after = times[whole + 1] if whole + 1 < len(times) else times[whole]
                timed[k] = times[whole] + (position - whole) * (after - times[whole])
    k = 0
    while k < count:
        if timed[k] is not None:
            k += 1
            continue
        run_end = k
        while run_end < count and timed[run_end] is None:
            run_end += 1
        left = timed[k - 1] if k > 0 else None
        right = timed[run_end] if run_end < count else None
        n = run_end - k
        if left is None:
            left = max(begin, (right if right is not None else end) - 0.3 * (n + 1))
        if right is None:
            right = min(end - 0.05, left + 0.3 * (n + 1))
        for step in range(n):
            timed[k + step] = left + (step + 1) * (right - left) / (n + 1)
        k = run_end
    for k in range(1, count):
        timed[k] = max(timed[k], timed[k - 1])
    return timed


class CanaryPass:
    """The refiner for run_final_pass: (audio, Parakeet's tokens and times) -> Canary's words and times.

    One per source of a recording, or per dropped file: it remembers the language it found, for a
    stretch too short to tell."""

    def __init__(self, model) -> None:
        self.model = model
        self.language: str | None = None

    def __call__(self, audio: np.ndarray, tokens: list[str], timestamps: list[float],
                 sample_rate: int = 16_000) -> tuple[list[str], list[float]]:
        language = guess_language(_join(tokens)) or self.language
        if language is None or not tokens:
            return tokens, timestamps
        self.language = language
        words = _words(tokens, timestamps)
        out_tokens: list[str] = []
        out_times: list[float] = []
        for begin, end in _pieces(audio, [time for _, _, time in words], sample_rate):
            last = end == len(audio)  # it keeps a word stamped at the audio's very end
            inside = [word for word in words if begin <= word[2] * sample_rate and (last or word[2] * sample_rate < end)]
            if not inside:
                continue  # Parakeet heard nobody here: neither does Canary
            first, last = inside[0][0], inside[-1][1]
            try:
                heard = spaced(str(self.model.recognize(audio[begin:end], sample_rate=sample_rate,
                                                        language=language))).split()
            except Exception:  # noqa: BLE001 - this piece keeps Parakeet's words; the rest goes on
                traceback.print_exc()
                heard = []
            matcher = SequenceMatcher(None, [_plain(_join(tokens[a:b])) for a, b, _ in inside],
                                      [_plain(word) for word in heard], autojunk=False)
            if not heard or len(heard) > 2 * len(inside) + 5 or matcher.ratio() < AGREEMENT:
                out_tokens += tokens[first:last]
                out_times += timestamps[first:last]
                continue
            out_tokens += [" " + word for word in heard]
            out_times += _timed(matcher.get_opcodes(), [time for _, _, time in inside], len(heard),
                                begin / sample_rate, end / sample_rate)
        return out_tokens, out_times
