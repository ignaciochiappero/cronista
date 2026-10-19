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
# Ported to Python from bavel: frontend/src/utils/localAgreement.js. The behaviour and the
# tests (tests/test_local_agreement.py) are the same; only the language changed.
"""LocalAgreement-n confirmation policy (Macháček et al., arXiv:2307.14743).

A recognizer that re-transcribes a growing utterance emits a full hypothesis on every
update and may revise earlier words. Rendering the raw latest hypothesis makes the text
flicker. LocalAgreement-n only commits the prefix that the last n hypotheses agree on;
committed text is immutable, and everything after it stays pending.
"""
from __future__ import annotations

from dataclasses import dataclass


def tokenize(text: str | None) -> list[str]:
    """Splits on whitespace. Tokens keep their punctuation, so a punctuation change is a revision."""
    if not text:
        return []
    return str(text).split()


def common_prefix(token_lists: list[list[str]]) -> list[str]:
    if not token_lists:
        return []
    shortest = min(len(tokens) for tokens in token_lists)
    prefix: list[str] = []
    for i in range(shortest):
        token = token_lists[0][i]
        if all(tokens[i] == token for tokens in token_lists):
            prefix.append(token)
        else:
            break
    return prefix


@dataclass(frozen=True)
class Snapshot:
    committed: str
    pending: str
    newly_committed: str


class LocalAgreement:
    """Stateful aggregator for one utterance stream."""

    def __init__(self, n: int = 2) -> None:
        if n < 2:
            raise ValueError("LocalAgreement needs n >= 2")
        self._n = n
        self._committed: list[str] = []
        self._history: list[list[str]] = []  # last n hypotheses, each without the committed prefix

    def _strip_committed(self, tokens: list[str]) -> list[str]:
        # By position, not by content: the hypothesis always describes the same cumulative
        # audio, so its first len(committed) tokens are words already emitted, however the
        # recognizer now spells them. This makes a late revision far back a no-op.
        return tokens[len(self._committed) :]

    def _snapshot(self, pending: list[str], newly_committed: list[str]) -> Snapshot:
        return Snapshot(" ".join(self._committed), " ".join(pending), " ".join(newly_committed))

    def update(self, text: str | None) -> Snapshot:
        """Feeds one hypothesis (the recognizer's full text so far)."""
        rest = self._strip_committed(tokenize(text))
        self._history.append(rest)
        if len(self._history) > self._n:
            self._history.pop(0)
        if len(self._history) < self._n:
            return self._snapshot(rest, [])

        agreed = common_prefix(self._history)
        if agreed:
            self._committed += agreed
            self._history = [hypothesis[len(agreed) :] for hypothesis in self._history]
        return self._snapshot(self._history[-1], agreed)

    def flush(self, final_text: str | None = None) -> Snapshot:
        """End of stream: commits the final transcript if given, otherwise the last pending tail."""
        if final_text is None:
            tail = self._history[-1] if self._history else []
        else:
            tail = self._strip_committed(tokenize(final_text))
        self._committed += tail
        self._history = []
        return self._snapshot([], tail)

    def reset(self) -> None:
        self._committed = []
        self._history = []

    @property
    def text(self) -> str:
        return " ".join(self._committed)
