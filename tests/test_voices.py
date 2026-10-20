from cronista.engine.transcript import TranscriptionSegment
from cronista.engine.voices import drop_echoes, line, speakers


def seg(start: float, end: float, text: str, source: str) -> TranscriptionSegment:
    return TranscriptionSegment(start, end, text, source)


def test_the_microphone_repeating_the_computer_is_dropped():
    # With speakers instead of headphones the microphone hears the call too, about 40 ms later.
    computer = seg(10.0, 13.0, "Sure, I have the screens ready.", "computer")
    echo = seg(10.1, 13.1, "Sure I have the screens ready", "mic")
    own = seg(14.0, 16.0, "Great, share them.", "mic")
    assert drop_echoes([echo, computer, own]) == [computer, own]


def test_a_weaker_echo_that_only_caught_some_words_is_dropped_too():
    computer = seg(10.0, 13.0, "Sure, I have the screens ready for Thursday.", "computer")
    echo = seg(10.2, 12.0, "the screens ready", "mic")
    assert drop_echoes([computer, echo]) == [computer]


def test_different_words_at_the_same_time_are_both_kept():
    computer = seg(10.0, 13.0, "Sure, I have the screens ready.", "computer")
    talking_over = seg(11.0, 12.5, "Wait, which screens?", "mic")
    assert drop_echoes([computer, talking_over]) == [computer, talking_over]


def test_the_same_words_at_another_time_are_kept():
    first = seg(1.0, 2.0, "Okay, see you then.", "computer")
    later = seg(30.0, 31.0, "Okay, see you then.", "mic")
    assert drop_echoes([later, first]) == [first, later]  # and sorted by when they were said


def test_labels_only_when_more_than_one_source_spoke():
    labels = {"mic": "Me", "computer": "Ana"}
    both = [seg(0, 1, "Hi.", "mic"), seg(1, 2, "Hello.", "computer")]
    assert speakers(both, labels) == labels
    assert speakers([seg(0, 1, "Hi.", "mic")], labels) is None
    assert speakers([TranscriptionSegment(0, 1, "From before 0.5.")], labels) is None
    names = speakers(both, labels)
    assert line(both[1], names) == "Ana: Hello."
    assert line(both[1], None) == "Hello."
