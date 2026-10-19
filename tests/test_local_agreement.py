# Ported from bavel: frontend/src/utils/localAgreement.test.js (same cases, Vitest -> pytest).
import pytest

from cronista.engine.local_agreement import LocalAgreement, common_prefix, tokenize


class TestTokenize:
    def test_splits_on_whitespace_and_drops_empties(self):
        assert tokenize("  hola   cómo estás ") == ["hola", "cómo", "estás"]

    def test_returns_an_empty_list_for_empty_input(self):
        assert tokenize("") == []
        assert tokenize(None) == []

    def test_keeps_punctuation_attached_to_the_token(self):
        assert tokenize("hola, qué tal") == ["hola,", "qué", "tal"]


class TestCommonPrefix:
    def test_returns_the_longest_shared_prefix(self):
        assert common_prefix([["a", "b", "c"], ["a", "b", "d"]]) == ["a", "b"]

    def test_returns_nothing_when_the_first_token_already_differs(self):
        assert common_prefix([["a"], ["b"]]) == []

    def test_is_bounded_by_the_shortest_list(self):
        assert common_prefix([["a", "b"], ["a", "b", "c"]]) == ["a", "b"]

    def test_returns_an_empty_prefix_for_no_lists(self):
        assert common_prefix([]) == []


class TestLocalAgreement:
    def test_commits_nothing_on_the_first_hypothesis(self):
        la = LocalAgreement()
        r = la.update("hola como estas")
        assert r.committed == ""
        assert r.pending == "hola como estas"

    def test_commits_only_the_prefix_two_hypotheses_agree_on(self):
        la = LocalAgreement()
        la.update("hola como estas")
        r = la.update("hola cómo estás amigo")
        # "como" was revised to "cómo", so only "hola" is stable.
        assert r.committed == "hola"
        assert r.pending == "cómo estás amigo"
        assert r.newly_committed == "hola"

    def test_grows_the_committed_prefix_as_hypotheses_stabilize(self):
        la = LocalAgreement()
        la.update("hola como estas")
        la.update("hola cómo estás amigo")
        r = la.update("hola cómo estás amigo mío")
        assert r.committed == "hola cómo estás amigo"
        assert r.pending == "mío"

    def test_never_revises_committed_text_when_the_recognizer_contradicts_it(self):
        la = LocalAgreement()
        la.update("hola cómo estás")
        la.update("hola cómo estás")
        assert la.text == "hola cómo estás"

        r = la.update("hola qué tal")
        assert r.committed == "hola cómo estás"
        assert r.pending == ""

    def test_does_not_re_emit_the_transcript_when_the_recognizer_revises_far_back(self):
        la = LocalAgreement()
        h1 = "Good morning, everyone. Thanks for joining"
        la.update(h1)
        la.update(h1)
        assert la.text == h1

        r = la.update("Good morning. Everyone thanks for joining the call")
        assert r.committed == h1
        assert r.pending == "the call"
        assert f"{r.committed} {r.pending}" == "Good morning, everyone. Thanks for joining the call"

    def test_ignores_a_hypothesis_that_shrinks_below_the_committed_length(self):
        la = LocalAgreement()
        la.update("uno dos tres")
        la.update("uno dos tres")
        r = la.update("uno dos")
        assert r.committed == "uno dos tres"
        assert r.pending == ""

    def test_respects_a_larger_n(self):
        la = LocalAgreement(n=3)
        assert la.update("uno dos").committed == ""
        assert la.update("uno dos").committed == ""
        assert la.update("uno dos tres").committed == "uno dos"

    def test_rejects_n_below_2(self):
        with pytest.raises(ValueError):
            LocalAgreement(n=1)

    def test_commits_the_pending_tail_on_flush(self):
        la = LocalAgreement()
        la.update("hola como")
        la.update("hola cómo estás")
        r = la.flush()
        assert r.pending == ""
        assert la.text == "hola cómo estás"

    def test_prefers_the_final_transcript_passed_to_flush(self):
        la = LocalAgreement()
        la.update("hola como")
        la.update("hola cómo estás")
        la.flush("hola cómo estás amigo")
        assert la.text == "hola cómo estás amigo"

    def test_does_not_duplicate_committed_words_when_flushing_a_full_final_text(self):
        la = LocalAgreement()
        la.update("uno dos")
        la.update("uno dos")
        assert la.text == "uno dos"
        la.flush("uno dos tres")
        assert la.text == "uno dos tres"

    def test_resets_back_to_an_empty_state(self):
        la = LocalAgreement()
        la.update("hola")
        la.update("hola")
        la.reset()
        assert la.text == ""
        assert la.update("otra cosa").committed == ""

    def test_handles_an_empty_hypothesis_without_committing_anything(self):
        la = LocalAgreement()
        assert la.update("").committed == ""
        assert la.update("").committed == ""
        assert la.text == ""
