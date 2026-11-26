//! Transcript segments, Spanish opening punctuation cleanup, WER metric, and SRT formatting.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<String>,
}

impl TranscriptionSegment {
    pub fn new(start: f64, end: f64, text: impl Into<String>, source: Option<String>) -> Self {
        Self {
            start,
            end,
            text: text.into(),
            source,
        }
    }
}

/// Put back a space before `¿` or `¡` when glued to a preceding non-opening character.
/// Matches Python regex `(?<=[^\s(\[{«"'¿¡])([¿¡])`.
pub fn tidy_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    let mut prev: Option<char> = None;
    for ch in text.chars() {
        if matches!(ch, '¿' | '¡') {
            if let Some(p) = prev {
                if !p.is_whitespace()
                    && !matches!(p, '(' | '[' | '{' | '«' | '"' | '\'' | '¿' | '¡')
                {
                    out.push(' ');
                }
            }
        }
        out.push(ch);
        prev = Some(ch);
    }
    out
}

pub(crate) fn extract_words_lower(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            for lc in ch.to_lowercase() {
                cur.push(lc);
            }
        } else if !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Share of the reference's words that the hypothesis gets wrong (Word Error Rate).
/// Case and punctuation are ignored.
pub fn word_difference(reference: &str, hypothesis: &str) -> f64 {
    let ref_w = extract_words_lower(reference);
    let hyp_w = extract_words_lower(hypothesis);
    if ref_w.is_empty() {
        return if hyp_w.is_empty() { 0.0 } else { 1.0 };
    }
    // Exact Levenshtein word edit distance (matches SequenceMatcher opcodes max(i2-i1, j2-j1) for simple cases)
    let m = ref_w.len();
    let n = hyp_w.len();
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = usize::from(ref_w[i - 1] != hyp_w[j - 1]);
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n] as f64 / m as f64
}

pub fn format_srt_timestamp(seconds: f64) -> String {
    let total_ms = (seconds.max(0.0) * 1000.0).round() as u64;
    let hours = total_ms / 3_600_000;
    let rest = total_ms % 3_600_000;
    let minutes = rest / 60_000;
    let rest = rest % 60_000;
    let secs = rest / 1000;
    let millis = rest % 1000;
    format!("{hours:02}:{minutes:02}:{secs:02},{millis:03}")
}

pub fn to_srt(segments: &[TranscriptionSegment], names: Option<&HashMap<String, String>>) -> String {
    let mut blocks = Vec::new();
    for seg in segments {
        let trimmed = seg.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        let text = match (names, seg.source.as_deref()) {
            (Some(map), Some(src)) if map.contains_key(src) => format!("{}: {trimmed}", map[src]),
            _ => trimmed.to_string(),
        };
        let timing = format!(
            "{} --> {}",
            format_srt_timestamp(seg.start),
            format_srt_timestamp(seg.end)
        );
        blocks.push(format!("{}\n{timing}\n{text}\n", blocks.len() + 1));
    }
    blocks.join("\n")
}
