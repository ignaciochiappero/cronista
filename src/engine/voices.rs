//! Speaker attribution ("mic" vs "computer") and microphone echo suppression.

use crate::engine::transcript::{extract_words_lower, TranscriptionSegment};
use std::collections::{HashMap, HashSet};

pub const SOURCES: &[&str] = &["mic", "computer"];

/// Whether most of the words the microphone heard are what the computer played at that moment.
pub fn is_echo(
    mic: &TranscriptionSegment,
    computer: &[TranscriptionSegment],
    share: f64,
    margin: f64,
) -> bool {
    let words = extract_words_lower(&mic.text);
    if words.is_empty() {
        return false;
    }
    let mut played: HashSet<String> = HashSet::new();
    for seg in computer {
        if seg.start - margin <= mic.end && mic.start <= seg.end + margin {
            for w in extract_words_lower(&seg.text) {
                played.insert(w);
            }
        }
    }
    let hits = words.iter().filter(|w| played.contains(*w)).count();
    (hits as f64) / (words.len() as f64) >= share
}

/// Drops microphone echoes of computer audio and returns segments sorted by start time.
pub fn drop_echoes(
    segments: &[TranscriptionSegment],
    share: f64,
    margin: f64,
) -> Vec<TranscriptionSegment> {
    let computer: Vec<TranscriptionSegment> = segments
        .iter()
        .filter(|s| s.source.as_deref() == Some("computer"))
        .cloned()
        .collect();
    let mut kept: Vec<TranscriptionSegment> = segments
        .iter()
        .filter(|s| {
            s.source.as_deref() != Some("mic") || !is_echo(s, &computer, share, margin)
        })
        .cloned()
        .collect();
    kept.sort_by(|a, b| a.start.total_cmp(&b.start));
    kept
}

/// Returns each source's label when more than one source spoke; `None` otherwise.
pub fn speakers(
    segments: &[TranscriptionSegment],
    labels: Option<&HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let sources: HashSet<&str> = segments
        .iter()
        .filter_map(|s| s.source.as_deref())
        .filter(|s| !s.is_empty())
        .collect();
    let labels = labels?;
    if sources.len() < 2 || labels.is_empty() {
        return None;
    }
    let mut out = HashMap::with_capacity(sources.len());
    for src in sources {
        let name = labels
            .get(src)
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or_else(|| src.to_string());
        out.insert(src.to_string(), name);
    }
    Some(out)
}

/// Formats a phrase as `"Ana: Hello."` when speaker names apply, or `"Hello."` otherwise.
pub fn line(segment: &TranscriptionSegment, names: Option<&HashMap<String, String>>) -> String {
    if let (Some(map), Some(src)) = (names, segment.source.as_deref()) {
        if let Some(label) = map.get(src) {
            return format!("{label}: {}", segment.text);
        }
    }
    segment.text.clone()
}
