//! High quality pass: Canary 1B v2 rewrites the words of Parakeet's final pass in pause-aligned
//! pieces of up to 24 s, inheriting Parakeet's timestamps via sequence alignment.
//! Exact behavioral parity with `cronista/engine/canary_pass.py`.

use crate::engine::final_pass::{find_quiet_cut, join_tokens};
use std::collections::HashSet;
use std::sync::OnceLock;
use unicode_normalization::UnicodeNormalization;

pub const PIECE_SECONDS: f64 = 24.0;
pub const SHORTEST_PIECE: f64 = 12.0;
pub const AGREEMENT: f64 = 0.5;

const SPANISH_RAW: &str = "de la que el en y los se del las un por con una su para es al lo como mas pero sus \
    le ya este si porque esta entre cuando muy sin sobre tambien hasta hay donde quien desde todo nos durante \
    todos uno les ni contra otros ese eso ellos esto mi antes algunos unos yo otro otras otra tanto esa estos \
    mucho nada muchos cual poco ella estar estas algo nosotros mis tu te ti vos estoy estan bueno pues entonces \
    aca asi hola gracias vamos tenemos hacer puede creo dale";

const ENGLISH_RAW: &str = "the of and to in is you that it was for on are as with his they at be this have from \
    or had by but not what all were we when your can there an which she do how their if will up about out many \
    then them these so would like more could just yeah okay right think know going get really well here now \
    because our my been did does its thats dont im were youre theres lets";

fn spanish_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| SPANISH_RAW.split_whitespace().collect())
}

fn english_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| ENGLISH_RAW.split_whitespace().collect())
}

/// Lowercase without accents or punctuation (matches NFD + filtering Mark/Punctuation).
fn plain(word: &str) -> String {
    word.to_lowercase()
        .nfd()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Returns `Some("es")`, `Some("en")`, or `None` when uncertain.
pub fn guess_language(text: &str) -> Option<&'static str> {
    let es = spanish_set();
    let en = english_set();
    let mut spanish = 0usize;
    let mut english = 0usize;
    for raw in text.split_whitespace() {
        let w = plain(raw);
        if es.contains(w.as_str()) {
            spanish += 1;
        }
        if en.contains(w.as_str()) {
            english += 1;
        }
    }
    if spanish + english < 3 {
        return None;
    }
    if spanish >= 2 * english {
        return Some("es");
    }
    if english >= 2 * spanish {
        return Some("en");
    }
    None
}

/// Inserts a space before `¿` or `¡` when preceded by a non-whitespace character.
pub fn spaced(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    let mut prev: Option<char> = None;
    for ch in text.chars() {
        if matches!(ch, '¿' | '¡') {
            if let Some(p) = prev {
                if !p.is_whitespace() {
                    out.push(' ');
                }
            }
        }
        out.push(ch);
        prev = Some(ch);
    }
    out
}

fn words_from_tokens(tokens: &[String], timestamps: &[f64]) -> Vec<(usize, usize, f64)> {
    let mut words: Vec<(usize, usize, f64)> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if words.is_empty() || token.starts_with(' ') {
            words.push((i, i + 1, timestamps[i]));
        } else {
            words.last_mut().unwrap().1 = i + 1;
        }
    }
    words
}

fn pieces(audio: &[f32], times: &[f64], sample_rate: usize) -> Vec<(usize, usize)> {
    let total = audio.len() as f64 / sample_rate as f64;
    let mut marks = Vec::with_capacity(times.len() + 2);
    marks.push(0.0);
    marks.extend_from_slice(times);
    marks.push(total);

    let mut out = Vec::new();
    let mut start = 0.0_f64;
    while total - start > PIECE_SECONDS {
        let mut best = (0.0_f64, start + SHORTEST_PIECE, start + PIECE_SECONDS);
        for pair in marks.windows(2) {
            let (before, after) = (pair[0], pair[1]);
            let earliest = (before + 0.2).max(start + SHORTEST_PIECE);
            let latest = after.min(start + PIECE_SECONDS);
            if earliest < latest && after - before > best.0 {
                best = (after - before, earliest, latest);
            }
        }
        let (_, earliest, latest) = best;
        let cut = find_quiet_cut(
            audio,
            (latest * sample_rate as f64) as usize,
            ((latest - earliest) * sample_rate as f64) as usize,
            1_600,
        );
        out.push(((start * sample_rate as f64) as usize, cut));
        start = cut as f64 / sample_rate as f64;
    }
    out.push(((start * sample_rate as f64) as usize, audio.len()));
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpTag {
    Equal,
    Replace,
    Delete,
    Insert,
}

/// Ratcliff/Obershelp recursive longest-contiguous-block matching (exact equivalent of
/// Python's `difflib.SequenceMatcher(autojunk=False)`).
fn find_longest_match(
    a: &[String],
    alo: usize,
    ahi: usize,
    b: &[String],
    blo: usize,
    bhi: usize,
) -> (usize, usize, usize) {
    let mut best_i = alo;
    let mut best_j = blo;
    let mut best_size = 0usize;

    let mut j2len: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for i in alo..ahi {
        let mut new_j2len = std::collections::HashMap::new();
        for j in blo..bhi {
            if a[i] == b[j] {
                let k = if j > blo {
                    j2len.get(&(j - 1)).copied().unwrap_or(0) + 1
                } else {
                    1
                };
                new_j2len.insert(j, k);
                if k > best_size {
                    best_i = i + 1 - k;
                    best_j = j + 1 - k;
                    best_size = k;
                }
            }
        }
        j2len = new_j2len;
    }
    (best_i, best_j, best_size)
}

fn matching_blocks(a: &[String], b: &[String]) -> Vec<(usize, usize, usize)> {
    let mut queue = vec![(0, a.len(), 0, b.len())];
    let mut blocks = Vec::new();
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = find_longest_match(a, alo, ahi, b, blo, bhi);
        if k > 0 {
            blocks.push((i, j, k));
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    blocks.sort_unstable();
    let mut non_adjacent: Vec<(usize, usize, usize)> = Vec::new();
    let (mut i1, mut j1, mut k1) = (0, 0, 0);
    for (i2, j2, k2) in blocks {
        if i1 + k1 == i2 && j1 + k1 == j2 {
            k1 += k2;
        } else {
            if k1 > 0 {
                non_adjacent.push((i1, j1, k1));
            }
            i1 = i2;
            j1 = j2;
            k1 = k2;
        }
    }
    if k1 > 0 {
        non_adjacent.push((i1, j1, k1));
    }
    non_adjacent.push((a.len(), b.len(), 0));
    non_adjacent
}

fn seq_ratio_and_opcodes(a: &[String], b: &[String]) -> (f64, Vec<(OpTag, usize, usize, usize, usize)>) {
    let blocks = matching_blocks(a, b);
    let matches: usize = blocks.iter().map(|x| x.2).sum();
    let total = a.len() + b.len();
    let ratio = if total == 0 {
        1.0
    } else {
        (2.0 * matches as f64) / total as f64
    };

    let mut opcodes = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    for (ai, bj, size) in blocks {
        let tag = if i < ai && j < bj {
            Some(OpTag::Replace)
        } else if i < ai {
            Some(OpTag::Delete)
        } else if j < bj {
            Some(OpTag::Insert)
        } else {
            None
        };
        if let Some(t) = tag {
            opcodes.push((t, i, ai, j, bj));
        }
        i = ai + size;
        j = bj + size;
        if size > 0 {
            opcodes.push((OpTag::Equal, ai, i, bj, j));
        }
    }
    (ratio, opcodes)
}

fn timed(
    opcodes: &[(OpTag, usize, usize, usize, usize)],
    times: &[f64],
    count: usize,
    begin: f64,
    end: f64,
) -> Vec<f64> {
    let mut out: Vec<Option<f64>> = vec![None; count];
    for &(tag, i1, i2, j1, j2) in opcodes {
        if matches!(tag, OpTag::Equal | OpTag::Replace) && j2 > j1 {
            for k in j1..j2 {
                let position = i1 as f64 + ((k - j1) as f64) * ((i2 - i1) as f64) / ((j2 - j1) as f64);
                let whole = (position as usize).min(times.len().saturating_sub(1));
                let after = if whole + 1 < times.len() {
                    times[whole + 1]
                } else {
                    times[whole]
                };
                out[k] = Some(times[whole] + (position - whole as f64) * (after - times[whole]));
            }
        }
    }

    let mut k = 0usize;
    while k < count {
        if out[k].is_some() {
            k += 1;
            continue;
        }
        let mut run_end = k;
        while run_end < count && out[run_end].is_none() {
            run_end += 1;
        }
        let left_opt = if k > 0 { out[k - 1] } else { None };
        let right_opt = if run_end < count { out[run_end] } else { None };
        let n = run_end - k;
        let left = left_opt.unwrap_or_else(|| {
            begin.max(right_opt.unwrap_or(end) - 0.3 * (n + 1) as f64)
        });
        let right = right_opt.unwrap_or_else(|| {
            (end - 0.05).min(left + 0.3 * (n + 1) as f64)
        });
        for step in 0..n {
            out[k + step] = Some(left + ((step + 1) as f64) * (right - left) / ((n + 1) as f64));
        }
        k = run_end;
    }

    let mut result: Vec<f64> = out.into_iter().map(|v| v.unwrap_or(begin)).collect();
    for idx in 1..count {
        if result[idx] < result[idx - 1] {
            result[idx] = result[idx - 1];
        }
    }
    result
}

pub trait CanaryRecognizer: Send + Sync {
    fn recognize_lang(&self, audio: &[f32], sample_rate: usize, language: &str) -> Result<String, String>;
}

pub struct CanaryPass<M> {
    pub model: M,
    pub language: Option<&'static str>,
}

impl<M: CanaryRecognizer> CanaryPass<M> {
    pub fn new(model: M) -> Self {
        Self {
            model,
            language: None,
        }
    }

    pub fn refine(
        &mut self,
        audio: &[f32],
        tokens: Vec<String>,
        timestamps: Vec<f64>,
        sample_rate: usize,
    ) -> (Vec<String>, Vec<f64>) {
        let language = guess_language(&join_tokens(&tokens)).or(self.language);
        let Some(language) = language else {
            return (tokens, timestamps);
        };
        if tokens.is_empty() {
            return (tokens, timestamps);
        }
        self.language = Some(language);

        let words = words_from_tokens(&tokens, &timestamps);
        let word_times: Vec<f64> = words.iter().map(|w| w.2).collect();
        let mut out_tokens = Vec::new();
        let mut out_times = Vec::new();

        for (begin, end) in pieces(audio, &word_times, sample_rate) {
            let is_last = end == audio.len();
            let inside: Vec<(usize, usize, f64)> = words
                .iter()
                .copied()
                .filter(|w| {
                    let pos = w.2 * sample_rate as f64;
                    (begin as f64) <= pos && (is_last || pos < end as f64)
                })
                .collect();
            if inside.is_empty() {
                continue;
            }
            let first = inside[0].0;
            let last = inside.last().unwrap().1;

            let heard: Vec<String> = match self.model.recognize_lang(&audio[begin..end], sample_rate, language) {
                Ok(text) => spaced(&text).split_whitespace().map(|s| s.to_string()).collect(),
                Err(err) => {
                    eprintln!("canary_pass error (keeping Parakeet): {err}");
                    Vec::new()
                }
            };

            let ref_plain: Vec<String> = inside
                .iter()
                .map(|&(a, b, _)| plain(&join_tokens(&tokens[a..b])))
                .collect();
            let hyp_plain: Vec<String> = heard.iter().map(|w| plain(w)).collect();
            let (ratio, opcodes) = seq_ratio_and_opcodes(&ref_plain, &hyp_plain);

            if heard.is_empty() || heard.len() > 2 * inside.len() + 5 || ratio < AGREEMENT {
                out_tokens.extend_from_slice(&tokens[first..last]);
                out_times.extend_from_slice(&timestamps[first..last]);
                continue;
            }

            let inside_times: Vec<f64> = inside.iter().map(|w| w.2).collect();
            let new_times = timed(
                &opcodes,
                &inside_times,
                heard.len(),
                begin as f64 / sample_rate as f64,
                end as f64 / sample_rate as f64,
            );
            for w in heard {
                out_tokens.push(format!(" {w}"));
            }
            out_times.extend(new_times);
        }

        (out_tokens, out_times)
    }
}
