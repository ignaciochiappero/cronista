//! Localized strings written outside the UI: default recording titles, export headings,
//! and default speaker labels (English and Spanish).

use chrono::{Datelike, NaiveDateTime, Timelike};
use std::collections::HashMap;

pub const LANGUAGES: &[&str] = &["en", "es"];

const MONTHS_EN: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MONTHS_ES: [&str; 12] = [
    "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportWords {
    pub transcript: &'static str,
    pub timestamps: &'static str,
    pub is_spanish: bool,
}

impl ExportWords {
    pub fn format_date(&self, dt: NaiveDateTime, from_file: bool) -> String {
        if self.is_spanish {
            let prefix = if from_file {
                "Transcrito el"
            } else {
                "Grabado el"
            };
            format!(
                "{prefix} {:02}/{:02}/{:04} a las {:02}:{:02}",
                dt.day(),
                dt.month(),
                dt.year(),
                dt.hour(),
                dt.minute()
            )
        } else {
            let prefix = if from_file {
                "Transcribed on"
            } else {
                "Recorded on"
            };
            format!(
                "{prefix} {:04}-{:02}-{:02} {:02}:{:02}",
                dt.year(),
                dt.month(),
                dt.day(),
                dt.hour(),
                dt.minute()
            )
        }
    }
}

pub fn language_or_default(language: Option<&str>) -> &'static str {
    match language {
        Some("es") => "es",
        _ => "en",
    }
}

/// A new recording's title: `"Recording Oct 05, 17:42"` or `"Grabación 5 oct, 17:42"`.
pub fn default_title(started: NaiveDateTime, language: Option<&str>) -> String {
    let lang = language_or_default(language);
    let month_idx = (started.month().saturating_sub(1) as usize).min(11);
    if lang == "es" {
        format!(
            "Grabación {} {}, {:02}:{:02}",
            started.day(),
            MONTHS_ES[month_idx],
            started.hour(),
            started.minute()
        )
    } else {
        format!(
            "Recording {} {:02}, {:02}:{:02}",
            MONTHS_EN[month_idx],
            started.day(),
            started.hour(),
            started.minute()
        )
    }
}

pub fn export_words(language: Option<&str>) -> ExportWords {
    if language_or_default(language) == "es" {
        ExportWords {
            transcript: "Transcripción",
            timestamps: "Marcas de tiempo",
            is_spanish: true,
        }
    } else {
        ExportWords {
            transcript: "Transcript",
            timestamps: "Timestamps",
            is_spanish: false,
        }
    }
}

pub fn default_labels(language: Option<&str>) -> HashMap<String, String> {
    let mut map = HashMap::with_capacity(2);
    if language_or_default(language) == "es" {
        map.insert("mic".to_string(), "Yo".to_string());
        map.insert("computer".to_string(), "Otros".to_string());
    } else {
        map.insert("mic".to_string(), "Me".to_string());
        map.insert("computer".to_string(), "Others".to_string());
    }
    map
}
