from cronista.engine.transcript import TranscriptionSegment, format_srt_timestamp, tidy_text, to_srt, word_difference


def test_adds_the_missing_space_before_spanish_opening_marks():
    # onnx-asr's token joining drops every space before a symbol, "¿" included.
    assert tidy_text("hola.¿Qué tal?¿Cómo te va?") == "hola. ¿Qué tal? ¿Cómo te va?"
    assert tidy_text("¡Hola!¡Chau!") == "¡Hola! ¡Chau!"


def test_leaves_opening_marks_after_brackets_alone():
    assert tidy_text("Dijo: (¿en serio?) sí.") == "Dijo: (¿en serio?) sí."


def test_word_difference_ignores_case_and_punctuation():
    assert word_difference("Hola, qué tal.", "hola qué tal") == 0.0


def test_word_difference_counts_a_lost_word():
    assert word_difference("este es un test para ir probando", "este es un para ir probando") == 1 / 7


def test_word_difference_of_empty_texts():
    assert word_difference("", "") == 0.0
    assert word_difference("", "algo") == 1.0


def test_formats_srt_timestamps_with_milliseconds():
    assert format_srt_timestamp(0) == "00:00:00,000"
    assert format_srt_timestamp(3661.5) == "01:01:01,500"
    assert format_srt_timestamp(59.9996) == "00:01:00,000"


def test_builds_numbered_srt_blocks():
    srt = to_srt(
        [
            TranscriptionSegment(start=1.0, end=2.5, text="Hola."),
            TranscriptionSegment(start=3.0, end=4.25, text="This is a test."),
        ]
    )
    assert srt == (
        "1\n00:00:01,000 --> 00:00:02,500\nHola.\n\n"
        "2\n00:00:03,000 --> 00:00:04,250\nThis is a test.\n"
    )


def test_skips_segments_without_text():
    assert to_srt([TranscriptionSegment(start=0, end=1, text="  ")]) == ""
