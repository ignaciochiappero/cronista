//! Definitive final-pass transcription over a whole recording in pause-aligned chunks.
//! Exact behavioral parity with `cronista/engine/final_pass.py`.

use crate::engine::transcript::{tidy_text, TranscriptionSegment};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct TimestampedResult {
    pub tokens: Vec<String>,
    pub timestamps: Vec<f64>,
}

/// Core speech-to-text model interface (implemented by Parakeet and test fakes).
pub trait SpeechModel: Send + Sync {
    fn recognize_text(&self, audio: &[f32], sample_rate: usize) -> Result<String, String>;
    fn recognize_with_timestamps(
        &self,
        audio: &[f32],
        sample_rate: usize,
    ) -> Result<TimestampedResult, String>;
}

/// Refiner callback invoked per chunk before token grouping (e.g. `CanaryPass`).
pub type RefinerFn<'a> = &'a mut dyn FnMut(&[f32], Vec<String>, Vec<f64>, usize) -> (Vec<String>, Vec<f64>);

/// Matches Python `_DECODE_SPACE = re.compile(r"\A\s|\s\B|(\s)\b")`:
/// - Drops leading whitespace at the start of the joined string.
/// - Drops whitespace immediately before a non-word character (`\B` after whitespace = not word boundary).
/// - Normalizes whitespace immediately before a word character to `" "`.
pub fn join_tokens(tokens: &[String]) -> String {
    let raw: String = tokens.iter().map(|s| s.as_str()).collect();
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let is_word = |c: char| c.is_alphanumeric() || c == '_';

    for i in 0..chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            if i == 0 {
                // \A\s -> drop
                continue;
            }
            let next_is_word = chars.get(i + 1).copied().map(is_word).unwrap_or(false);
            if next_is_word {
                // (\s)\b -> " "
                out.push(' ');
            }
            // else \s\B -> drop
        } else {
            out.push(c);
        }
    }
    out
}

fn is_closing_only(token: &str) -> bool {
    let trimmed = token.trim();
    !trimmed.is_empty()
        && trimmed.chars().all(|c| {
            matches!(
                c,
                '.' | ',' | ';' | ':' | '!' | '?' | '…' | ')' | ']' | '»' | '"' | '\''
            )
        })
}

fn ends_sentence(token: &str) -> bool {
    let trimmed = token.trim();
    trimmed.ends_with('.')
        || trimmed.ends_with('?')
        || trimmed.ends_with('!')
        || trimmed.ends_with('…')
}

/// Groups tokens into subtitle segments: one per sentence, or per stretch between long pauses.
pub fn segments_from_tokens(
    tokens: &[String],
    timestamps: &[f64],
    audio_seconds: f64,
    pause_seconds: f64,
    hold_seconds: f64,
) -> Vec<TranscriptionSegment> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();

    for (i, token) in tokens.iter().enumerate() {
        let closing = is_closing_only(token);
        if closing && current.is_empty() && !groups.is_empty() {
            groups.last_mut().unwrap().push(i);
            continue;
        }
        if !current.is_empty() && !closing && i > 0 && timestamps[i] - timestamps[i - 1] > pause_seconds {
            groups.push(std::mem::take(&mut current));
        }
        current.push(i);
        if ends_sentence(token) {
            groups.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }

    let mut segments = Vec::with_capacity(groups.len());
    for (n, group) in groups.iter().enumerate() {
        let next_start = if n + 1 < groups.len() {
            timestamps[groups[n + 1][0]]
        } else {
            audio_seconds
        };
        let last_idx = *group.last().unwrap();
        let end = next_start.min(timestamps[last_idx] + hold_seconds);
        let group_tokens: Vec<String> = group.iter().map(|&idx| tokens[idx].clone()).collect();
        let text = tidy_text(join_tokens(&group_tokens).trim());
        if !text.is_empty() {
            segments.push(TranscriptionSegment::new(timestamps[group[0]], end, text, None));
        }
    }
    segments
}

pub fn run_final_pass<F>(
    model: &dyn SpeechModel,
    audio: &[f32],
    sample_rate: usize,
    refine: Option<&mut F>,
) -> Result<Vec<TranscriptionSegment>, String>
where
    F: FnMut(&[f32], Vec<String>, Vec<f64>, usize) -> (Vec<String>, Vec<f64>) + ?Sized,
{
    let res = model.recognize_with_timestamps(audio, sample_rate)?;
    let (tokens, timestamps) = if let Some(refiner) = refine {
        refiner(audio, res.tokens, res.timestamps, sample_rate)
    } else {
        (res.tokens, res.timestamps)
    };
    Ok(segments_from_tokens(
        &tokens,
        &timestamps,
        audio.len() as f64 / sample_rate as f64,
        0.8,
        1.0,
    ))
}

/// The middle of the quietest `frame` (default 1600 = 100 ms) in `search` samples before `target`.
pub fn find_quiet_cut(audio: &[f32], target: usize, search: usize, frame: usize) -> usize {
    let target = target.min(audio.len());
    let low = target.saturating_sub(search);
    let window = &audio[low..target];
    let frames = window.len() / frame;
    if frames == 0 {
        return target;
    }
    let mut best_idx = 0usize;
    let mut best_energy = f64::INFINITY;
    for f in 0..frames {
        let slice = &window[f * frame..(f + 1) * frame];
        let energy: f64 = slice.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / frame as f64;
        if energy < best_energy {
            best_energy = energy;
            best_idx = f;
        }
    }
    low + best_idx * frame + frame / 2
}

/// The RMS level of the loudest 100 ms frame.
pub fn loudest(audio: &[f32], frame: usize) -> f64 {
    let frames = audio.len() / frame;
    if frames == 0 {
        if audio.is_empty() {
            return 0.0;
        }
        let mean_sq: f64 =
            audio.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / audio.len() as f64;
        return mean_sq.sqrt();
    }
    let mut max_energy = 0.0_f64;
    for f in 0..frames {
        let slice = &audio[f * frame..(f + 1) * frame];
        let energy: f64 = slice.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / frame as f64;
        if energy > max_energy {
            max_energy = energy;
        }
    }
    max_energy.sqrt()
}

fn read_f32_slice(file: &mut File, sample_offset: usize, sample_count: usize) -> Result<Vec<f32>, String> {
    file.seek(SeekFrom::Start((sample_offset * 4) as u64))
        .map_err(|e| format!("seek audio tape failed: {e}"))?;
    let mut raw = vec![0u8; sample_count * 4];
    file.read_exact(&mut raw)
        .map_err(|e| format!("read audio tape failed: {e}"))?;
    let mut out = Vec::with_capacity(sample_count);
    for chunk in raw.chunks_exact(4) {
        out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Ok(out)
}

pub struct FinalPassOptions {
    pub sample_rate: usize,
    pub chunk_seconds: f64,
    pub search_seconds: f64,
    pub silence: f64,
}

impl Default for FinalPassOptions {
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            chunk_seconds: 90.0,
            search_seconds: 10.0,
            silence: 0.002,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RawChunk {
    pub start: usize,
    pub end: usize,
    pub tokens: Vec<String>,
    pub timestamps: Vec<f64>,
}

/// Stage 1 of two-stage sequential transcription: runs ONLY Parakeet across the tape file,
/// emitting preview segments and returning lightweight token/timestamp chunks so Parakeet can be
/// completely unloaded from RAM before Canary runs Stage 2.
pub fn run_parakeet_chunks_file<C, S>(
    model: &dyn SpeechModel,
    path: &Path,
    opts: FinalPassOptions,
    mut on_chunk: C,
    stopped: S,
) -> Result<(Vec<RawChunk>, Vec<TranscriptionSegment>), String>
where
    C: FnMut(&[TranscriptionSegment], f64),
    S: Fn() -> bool,
{
    let meta = std::fs::metadata(path).map_err(|e| format!("stat audio tape failed: {e}"))?;
    let total = (meta.len() / 4) as usize;
    let chunk = (opts.chunk_seconds * opts.sample_rate as f64) as usize;
    let search = (opts.search_seconds * opts.sample_rate as f64) as usize;

    let mut file = File::open(path).map_err(|e| format!("open audio tape failed: {e}"))?;
    let mut raw_chunks = Vec::new();
    let mut segments = Vec::new();
    let mut start = 0usize;

    while start < total && !stopped() {
        let end = if total - start <= chunk + search {
            total
        } else {
            let window_start = start + chunk - search;
            let window = read_f32_slice(&mut file, window_start, search)?;
            window_start + find_quiet_cut(&window, window.len(), search, 1_600)
        };
        let audio = read_f32_slice(&mut file, start, end - start)?;
        let offset = start as f64 / opts.sample_rate as f64;
        let mut new_segs = Vec::new();
        if loudest(&audio, 1_600) >= opts.silence {
            let res = model.recognize_with_timestamps(&audio, opts.sample_rate)?;
            let preview = segments_from_tokens(
                &res.tokens,
                &res.timestamps,
                audio.len() as f64 / opts.sample_rate as f64,
                0.8,
                1.0,
            );
            for seg in preview {
                new_segs.push(TranscriptionSegment::new(
                    seg.start + offset,
                    seg.end + offset,
                    seg.text,
                    None,
                ));
            }
            segments.extend(new_segs.iter().cloned());
            raw_chunks.push(RawChunk {
                start,
                end,
                tokens: res.tokens,
                timestamps: res.timestamps,
            });
        }
        start = end;
        on_chunk(&new_segs, end as f64 / opts.sample_rate as f64);
    }

    Ok((raw_chunks, segments))
}

/// Stage 2 of two-stage sequential transcription: runs the refiner (Canary) on previously computed
/// Parakeet chunks after Parakeet has been unloaded from RAM, avoiding dual-model memory spikes.
pub fn refine_chunks_file<C, S>(
    path: &Path,
    sample_rate: usize,
    chunks: Vec<RawChunk>,
    refine: RefinerFn<'_>,
    mut on_chunk: C,
    stopped: S,
) -> Result<Vec<TranscriptionSegment>, String>
where
    C: FnMut(&[TranscriptionSegment], f64),
    S: Fn() -> bool,
{
    let mut file = File::open(path).map_err(|e| format!("open audio tape failed: {e}"))?;
    let mut segments = Vec::new();
    for ch in chunks {
        if stopped() {
            break;
        }
        let audio = read_f32_slice(&mut file, ch.start, ch.end - ch.start)?;
        let offset = ch.start as f64 / sample_rate as f64;
        let (tokens, timestamps) = refine(&audio, ch.tokens, ch.timestamps, sample_rate);
        let passed = segments_from_tokens(
            &tokens,
            &timestamps,
            audio.len() as f64 / sample_rate as f64,
            0.8,
            1.0,
        );
        let mut new_segs = Vec::with_capacity(passed.len());
        for seg in passed {
            new_segs.push(TranscriptionSegment::new(
                seg.start + offset,
                seg.end + offset,
                seg.text,
                None,
            ));
        }
        segments.extend(new_segs.iter().cloned());
        on_chunk(&new_segs, ch.end as f64 / sample_rate as f64);
    }
    Ok(segments)
}

pub fn run_final_pass_file<C, S>(
    model: &dyn SpeechModel,
    path: &Path,
    opts: FinalPassOptions,
    mut on_chunk: C,
    stopped: S,
    mut refine: Option<RefinerFn<'_>>,
) -> Result<Vec<TranscriptionSegment>, String>
where
    C: FnMut(&[TranscriptionSegment], f64),
    S: Fn() -> bool,
{
    let meta = std::fs::metadata(path).map_err(|e| format!("stat audio tape failed: {e}"))?;
    let total = (meta.len() / 4) as usize;
    let chunk = (opts.chunk_seconds * opts.sample_rate as f64) as usize;
    let search = (opts.search_seconds * opts.sample_rate as f64) as usize;

    let mut file = File::open(path).map_err(|e| format!("open audio tape failed: {e}"))?;
    let mut segments = Vec::new();
    let mut start = 0usize;

    while start < total && !stopped() {
        let end = if total - start <= chunk + search {
            total
        } else {
            let window_start = start + chunk - search;
            let window = read_f32_slice(&mut file, window_start, search)?;
            window_start + find_quiet_cut(&window, window.len(), search, 1_600)
        };
        let audio = read_f32_slice(&mut file, start, end - start)?;
        let offset = start as f64 / opts.sample_rate as f64;
        let mut new_segs = Vec::new();
        if loudest(&audio, 1_600) >= opts.silence {
            let passed = match refine.as_mut() {
                Some(r) => run_final_pass(model, &audio, opts.sample_rate, Some(&mut **r))?,
                None => run_final_pass::<dyn FnMut(&[f32], Vec<String>, Vec<f64>, usize) -> (Vec<String>, Vec<f64>)>(
                    model,
                    &audio,
                    opts.sample_rate,
                    None,
                )?,
            };
            for seg in passed {
                new_segs.push(TranscriptionSegment::new(
                    seg.start + offset,
                    seg.end + offset,
                    seg.text,
                    None,
                ));
            }
            segments.extend(new_segs.iter().cloned());
        }
        start = end;
        on_chunk(&new_segs, end as f64 / opts.sample_rate as f64);
    }

    Ok(segments)
}
