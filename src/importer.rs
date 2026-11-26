//! Transcribes dropped or picked audio/video files in any format FFmpeg reads.
//! Exact behavioral parity with `cronista/importer.py`.

use crate::engine::ffmpeg_locator::{configure_no_window, run_with_progress};
use crate::engine::final_pass::{
    refine_chunks_file, run_final_pass_file, run_parakeet_chunks_file, FinalPassOptions, RefinerFn,
    SpeechModel,
};
use crate::engine::transcript::TranscriptionSegment;
use regex::Regex;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use thiserror::Error;

pub const SAMPLE_RATE: usize = 16_000;
pub const AUDIO_NAME: &str = "audio-16k.f32";
pub const CHUNK_SECONDS: f64 = 30.0;
pub const SEARCH_SECONDS: f64 = 5.0;

pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "m4a", "aac", "ogg", "oga", "opus", "flac", "wma", "aif", "aiff", "amr", "mka",
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "mts", "m2ts",
    "3gp",
];

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("{0}")]
    Failed(String),
    #[error("Transcription cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub duration: f64,
    pub has_audio: bool,
    pub has_video: bool,
}

#[derive(Debug, Clone)]
pub struct ImportResult {
    pub duration: f64,
    pub has_video: bool,
    pub segments: Vec<TranscriptionSegment>,
}

fn duration_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)^\s*Duration: (\d+):(\d\d):(\d\d(?:\.\d+)?)").unwrap())
}

fn stream_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)^\s*Stream #\d+:\d+\S*: (\w+): (.*)$").unwrap())
}

/// Parses `ffmpeg -i <file>` stderr output for duration, audio, and non-cover-art video streams.
pub fn read_report(report: &str) -> Result<MediaInfo, ImportError> {
    if !report.contains("Input #0") {
        return Err(ImportError::Failed(
            "Cronista can't read this file. It doesn't look like audio or video.".to_string(),
        ));
    }
    let duration = duration_re()
        .captures(report)
        .map(|cap| {
            let h: f64 = cap[1].parse().unwrap_or(0.0);
            let m: f64 = cap[2].parse().unwrap_or(0.0);
            let s: f64 = cap[3].parse().unwrap_or(0.0);
            h * 3600.0 + m * 60.0 + s
        })
        .unwrap_or(0.0);

    let mut has_audio = false;
    let mut has_video = false;
    for cap in stream_re().captures_iter(report) {
        let kind = &cap[1];
        let details = &cap[2];
        if kind == "Audio" {
            has_audio = true;
        } else if kind == "Video" && !details.contains("(attached pic)") {
            has_video = true;
        }
    }
    Ok(MediaInfo {
        duration,
        has_audio,
        has_video,
    })
}

pub fn probe(ffmpeg: &str, path: &Path) -> Result<MediaInfo, ImportError> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-i"]).arg(path);
    configure_no_window(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| ImportError::Failed(format!("Running ffmpeg failed: {e}")))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    read_report(&stderr)
}

pub fn decode_audio<P, S>(
    ffmpeg: &str,
    path: &Path,
    target: &Path,
    mut progress: P,
    stopped: S,
) -> Result<(), ImportError>
where
    P: FnMut(f64),
    S: Fn() -> bool,
{
    let args: Vec<String> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-i".into(),
        path.to_string_lossy().into_owned(),
        "-map".into(),
        "0:a:0".into(),
        "-vn".into(),
        "-sn".into(),
        "-dn".into(),
        "-ac".into(),
        "1".into(),
        "-ar".into(),
        SAMPLE_RATE.to_string(),
        "-f".into(),
        "f32le".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-stats_period".into(),
        "0.25".into(),
        target.to_string_lossy().into_owned(),
    ];

    let res = run_with_progress(ffmpeg, &args, None, &mut progress, &stopped);
    if stopped() {
        let _ = fs::remove_file(target);
        return Err(ImportError::Cancelled);
    }
    match res {
        Ok((0, _)) => {
            let secs = fs::metadata(target)
                .map(|m| m.len() as f64 / 4.0 / SAMPLE_RATE as f64)
                .unwrap_or(0.0);
            progress(secs);
            Ok(())
        }
        Ok((_, errors)) => {
            let _ = fs::remove_file(target);
            let tail: String = errors
                .trim()
                .chars()
                .rev()
                .take(300)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            Err(ImportError::Failed(format!(
                "Reading the file's sound failed: {tail}"
            )))
        }
        Err(e) => {
            let _ = fs::remove_file(target);
            Err(ImportError::Failed(e))
        }
    }
}

pub fn transcribe_file<P, L, S>(
    model: &dyn SpeechModel,
    ffmpeg: &str,
    path: &Path,
    folder: &Path,
    mut progress: P,
    mut lines: L,
    stopped: S,
    refine: Option<RefinerFn<'_>>,
) -> Result<ImportResult, ImportError>
where
    P: FnMut(&str, f64, f64),
    L: FnMut(&[TranscriptionSegment]),
    S: Fn() -> bool,
{
    let info = probe(ffmpeg, path)?;
    if !info.has_audio {
        return Err(ImportError::Failed(
            "This file has no sound to transcribe.".to_string(),
        ));
    }

    let audio = folder.join(AUDIO_NAME);
    progress("reading", 0.0, info.duration);
    if let Err(err) = decode_audio(
        ffmpeg,
        path,
        &audio,
        |secs| progress("reading", secs, info.duration.max(secs)),
        &stopped,
    ) {
        let _ = fs::remove_file(&audio);
        return Err(err);
    }

    let total = fs::metadata(&audio)
        .map(|m| m.len() as f64 / 4.0 / SAMPLE_RATE as f64)
        .unwrap_or(0.0);
    progress("transcribing", 0.0, total);

    let opts = FinalPassOptions {
        sample_rate: SAMPLE_RATE,
        chunk_seconds: CHUNK_SECONDS,
        search_seconds: SEARCH_SECONDS,
        silence: 0.002,
    };
    let pass_res = run_final_pass_file(
        model,
        &audio,
        opts,
        |new_segs, secs| {
            if !new_segs.is_empty() {
                lines(new_segs);
            }
            progress("transcribing", secs, total);
        },
        &stopped,
        refine,
    );
    let _ = fs::remove_file(&audio);

    let segments = pass_res.map_err(ImportError::Failed)?;
    if stopped() {
        return Err(ImportError::Cancelled);
    }

    Ok(ImportResult {
        duration: if info.duration > 0.0 {
            info.duration
        } else {
            total
        },
        has_video: info.has_video,
        segments,
    })
}

/// Memory-efficient two-stage file transcription:
/// 1. Decodes audio to `audio-16k.f32`.
/// 2. Acquires Parakeet (`acquire_parakeet`) and runs Stage 1 over all chunks.
/// 3. Drops Parakeet (`on_release_parakeet`) BEFORE loading Canary via `acquire_refiner`.
/// 4. If a refiner is returned, runs Stage 2 over the saved chunks, then drops the refiner.
/// Peak RAM stays bounded by `max(Parakeet, Canary)` instead of `Parakeet + Canary`!
pub fn transcribe_file_sequential<A, R, F, P, L, S>(
    acquire_parakeet: A,
    on_release_parakeet: R,
    acquire_refiner: Option<F>,
    ffmpeg: &str,
    path: &Path,
    folder: &Path,
    mut progress: P,
    mut lines: L,
    stopped: S,
) -> Result<ImportResult, ImportError>
where
    A: FnOnce() -> Result<std::sync::Arc<dyn SpeechModel>, String>,
    R: FnOnce(),
    F: FnOnce() -> Option<Box<dyn FnMut(&[f32], Vec<String>, Vec<f64>, usize) -> (Vec<String>, Vec<f64>) + Send>>,
    P: FnMut(&str, f64, f64),
    L: FnMut(&[TranscriptionSegment]),
    S: Fn() -> bool,
{
    let info = probe(ffmpeg, path)?;
    if !info.has_audio {
        return Err(ImportError::Failed(
            "This file has no sound to transcribe.".to_string(),
        ));
    }

    let audio = folder.join(AUDIO_NAME);
    progress("reading", 0.0, info.duration);
    if let Err(err) = decode_audio(
        ffmpeg,
        path,
        &audio,
        |secs| progress("reading", secs, info.duration.max(secs)),
        &stopped,
    ) {
        let _ = fs::remove_file(&audio);
        return Err(err);
    }

    let total = fs::metadata(&audio)
        .map(|m| m.len() as f64 / 4.0 / SAMPLE_RATE as f64)
        .unwrap_or(0.0);
    let has_refiner = acquire_refiner.is_some();
    let total_units = if has_refiner { total * 2.0 } else { total };
    progress("transcribing", 0.0, total_units);

    let opts = FinalPassOptions {
        sample_rate: SAMPLE_RATE,
        chunk_seconds: CHUNK_SECONDS,
        search_seconds: SEARCH_SECONDS,
        silence: 0.002,
    };

    // Stage 1: Parakeet only
    let parakeet = match acquire_parakeet() {
        Ok(m) => m,
        Err(e) => {
            let _ = fs::remove_file(&audio);
            return Err(ImportError::Failed(e));
        }
    };
    let stage1 = run_parakeet_chunks_file(
        parakeet.as_ref(),
        &audio,
        opts,
        |new_segs, secs| {
            if !new_segs.is_empty() {
                lines(new_segs);
            }
            progress("transcribing", secs, total_units);
        },
        &stopped,
    );
    drop(parakeet);
    on_release_parakeet();

    let (raw_chunks, parakeet_segments) = match stage1 {
        Ok(res) => res,
        Err(e) => {
            let _ = fs::remove_file(&audio);
            return Err(ImportError::Failed(e));
        }
    };

    if stopped() {
        let _ = fs::remove_file(&audio);
        return Err(ImportError::Cancelled);
    }

    // Stage 2: Optional Canary refinement (Parakeet is already out of RAM!)
    let segments = if let Some(make_refiner) = acquire_refiner {
        if let Some(mut refiner_box) = make_refiner() {
            let refiner_fn: RefinerFn<'_> = &mut *refiner_box;
            let stage2 = refine_chunks_file(
                &audio,
                SAMPLE_RATE,
                raw_chunks,
                refiner_fn,
                |new_segs, secs| {
                    if !new_segs.is_empty() {
                        lines(new_segs);
                    }
                    progress("transcribing", total + secs, total_units);
                },
                &stopped,
            );
            drop(refiner_box);
            let _ = fs::remove_file(&audio);
            stage2.map_err(ImportError::Failed)?
        } else {
            let _ = fs::remove_file(&audio);
            parakeet_segments
        }
    } else {
        let _ = fs::remove_file(&audio);
        parakeet_segments
    };

    if stopped() {
        return Err(ImportError::Cancelled);
    }

    Ok(ImportResult {
        duration: if info.duration > 0.0 {
            info.duration
        } else {
            total
        },
        has_video: info.has_video,
        segments,
    })
}
