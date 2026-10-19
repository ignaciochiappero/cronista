"""What Cronista writes for the user outside the page, in the page's language: English or Spanish.

The page translates its own words; these are the ones Python writes into files and the history:
a new recording's title and the headings of the Word and PDF files. Months come from these lists
rather than strftime's %b, which follows the computer's locale.
"""
from __future__ import annotations

from datetime import datetime

LANGUAGES = ("en", "es")
MONTHS = {
    "en": ("Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"),
    "es": ("ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic"),
}
# The headings of an exported transcript; "recorded" is formatted with the recording's start.
EXPORT_WORDS = {
    "en": {"recorded": "Recorded on {:%Y-%m-%d %H:%M}", "transcript": "Transcript", "timestamps": "Timestamps"},
    "es": {"recorded": "Grabado el {:%d/%m/%Y a las %H:%M}", "transcript": "Transcripción",
           "timestamps": "Marcas de tiempo"},
}


def language_or_default(language: str | None) -> str:
    return language if language in LANGUAGES else "en"


def default_title(started: datetime, language: str | None) -> str:
    """A new recording's title: "Recording Oct 05, 17:42" or "Grabación 5 oct, 17:42"."""
    month = MONTHS[language_or_default(language)][started.month - 1]
    if language_or_default(language) == "es":
        return f"Grabación {started.day} {month}, {started:%H:%M}"
    return f"Recording {month} {started.day:02d}, {started:%H:%M}"


def export_words(language: str | None) -> dict[str, str]:
    return EXPORT_WORDS[language_or_default(language)]
