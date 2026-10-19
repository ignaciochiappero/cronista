from cronista.engine.final_pass import segments_from_tokens


def test_splits_sentences_and_times_them_with_token_timestamps():
    tokens = [" H", "ola", ",", " qué", " tal", ".", " This", " is", " a", " test", "."]
    times = [0.0, 0.24, 0.48, 0.56, 0.8, 1.04, 2.0, 2.24, 2.4, 2.56, 2.88]
    segments = segments_from_tokens(tokens, times, audio_seconds=3.5)
    assert [s.text for s in segments] == ["Hola, qué tal.", "This is a test."]
    # A segment lasts until the next one starts, at most 1 s after its last token.
    assert (segments[0].start, segments[0].end) == (0.0, 2.0)
    assert (segments[1].start, segments[1].end) == (2.0, 3.5)


def test_a_long_pause_also_ends_a_segment():
    segments = segments_from_tokens([" uno", " dos", " tres", " cuatro"], [0.0, 0.3, 2.0, 2.3], audio_seconds=3.0)
    assert [s.text for s in segments] == ["uno dos", "tres cuatro"]
    assert segments[0].end == 1.3


def test_restores_the_space_before_an_opening_question_mark():
    segments = segments_from_tokens([" Bueno", " ¿", "qué", " tal", "?"], [0.0, 0.3, 0.4, 0.6, 0.8], audio_seconds=1.5)
    assert [s.text for s in segments] == ["Bueno ¿qué tal?"]


def test_a_closing_mark_after_a_pause_stays_with_its_sentence():
    # Seen in the worst-case run: the final "." came 1.2 s after the last word and became a
    # subtitle of its own.
    segments = segments_from_tokens([" en", " el", " momento", "."], [0.0, 0.2, 0.4, 1.6], audio_seconds=3.0)
    assert [s.text for s in segments] == ["en el momento."]


def test_an_opening_mark_after_a_pause_starts_the_next_sentence():
    segments = segments_from_tokens(
        [" bien", ".", " ¿", "Y", " vos", "?"], [0.0, 0.2, 1.5, 1.6, 1.8, 2.0], audio_seconds=3.0
    )
    assert [s.text for s in segments] == ["bien.", "¿Y vos?"]


def test_no_tokens_means_no_segments():
    assert segments_from_tokens([], [], audio_seconds=5.0) == []
