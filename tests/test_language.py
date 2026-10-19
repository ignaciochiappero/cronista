from datetime import datetime

from cronista.language import default_title, language_or_default

STARTED = datetime(2026, 10, 5, 17, 42)


def test_default_titles_in_english_and_spanish():
    assert default_title(STARTED, "en") == "Recording Oct 05, 17:42"
    assert default_title(STARTED, "es") == "Grabación 5 oct, 17:42"


def test_an_unknown_language_is_english():
    assert language_or_default(None) == "en"
    assert language_or_default("fr") == "en"
    assert language_or_default("es") == "es"
    assert default_title(STARTED, None) == "Recording Oct 05, 17:42"
