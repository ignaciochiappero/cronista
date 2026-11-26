//! Burns transcripts into video copies as ASS subtitles via FFmpeg's libass + hardware H.264.
//! Exact behavioral parity with `cronista/subtitles.py`.

use crate::engine::ffmpeg_locator::run_with_progress;
use crate::engine::transcript::TranscriptionSegment;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const SOURCE_NAME: &str = "recording.mp4";
pub const SUBTITLED_NAME: &str = "recording-subtitled.mp4";
pub const ASS_NAME: &str = "subtitles.ass";

const COLOR_MIC: &str = "&H007F8AFF&";
const COLOR_COMPUTER: &str = "&H00FFB48A&";
const WHITE: &str = "&H00FFFFFF&";

const HEADER: &str = "[Script Info]\n\
ScriptType: v4.00+\n\
PlayResX: 1920\n\
PlayResY: 1080\n\
WrapStyle: 0\n\
ScaledBorderAndShadow: yes\n\
\n\
[V4+ Styles]\n\
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, \
Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, \
MarginV, Encoding\n\
Style: Default,Segoe UI,46,&H00FFFFFF,&H00FFFFFF,&HFF000000,&H48000000,0,0,0,0,100,100,0,0,4,14,0,2,160,160,64,1\n\
\n\
[Events]\n\
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";

fn ass_time(seconds: f64) -> String {
    let cs = (seconds.max(0.0) * 100.0).round() as u64;
    let hours = cs / 360_000;
    let rest = cs % 360_000;
    let minutes = rest / 6_000;
    let rest = rest % 6_000;
    let secs = rest / 100;
    let cents = rest % 100;
    format!("{hours}:{minutes:02}:{secs:02}.{cents:02}")
}

fn plain(text: &str) -> String {
    text.replace('{', "(")
        .replace('}', ")")
        .replace('\\', "/")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn ass_document(
    segments: &[TranscriptionSegment],
    names: Option<&HashMap<String, String>>,
) -> String {
    let mut out = String::from(HEADER);
    for seg in segments {
        let mut text = plain(&seg.text);
        if text.is_empty() {
            continue;
        }
        if let (Some(map), Some(src)) = (names, seg.source.as_deref()) {
            if let Some(label) = map.get(src) {
                let color = match src {
                    "mic" => COLOR_MIC,
                    "computer" => COLOR_COMPUTER,
                    _ => WHITE,
                };
                text = format!(
                    r"{{\c{color}\b1}}{}:{{\c{WHITE}\b0}} {text}",
                    plain(label)
                );
            }
        }
        out.push_str(&format!(
            "Dialogue: 0,{},{},Default,,0,0,0,,{text}\n",
            ass_time(seg.start),
            ass_time(seg.end)
        ));
    }
    out
}

pub fn burn_subtitles<P>(
    ffmpeg: &str,
    folder: &Path,
    segments: &[TranscriptionSegment],
    names: Option<&HashMap<String, String>>,
    quality: u32,
    source: Option<&Path>,
    progress: P,
) -> Result<PathBuf, String>
where
    P: FnMut(f64),
{
    let (video, output_name, frames_filter, sound_args, tuning): (
        String,
        String,
        &str,
        Vec<&str>,
        Vec<&str>,
    ) = match source {
        None => (
            SOURCE_NAME.to_string(),
            SUBTITLED_NAME.to_string(),
            "fps=30,",
            vec!["-c:a", "copy"],
            vec!["-scenario", "display_remoting"],
        ),
        Some(src) => {
            let stem = src
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("video");
            (
                src.to_string_lossy().into_owned(),
                format!("{stem}-subtitled.mp4"),
                "scale=trunc(iw/2)*2:trunc(ih/2)*2,",
                vec!["-c:a", "aac", "-b:a", "160k"],
                vec![],
            )
        }
    };

    let video_check = folder.join(&video);
    if !video_check.exists() {
        return Err("The video file is no longer there.".to_string());
    }

    let ass_path = folder.join(ASS_NAME);
    fs::write(&ass_path, ass_document(segments, names))
        .map_err(|e| format!("Writing subtitles.ass failed: {e}"))?;

    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-i".into(),
        video,
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "0:a:0?".into(),
        "-vf".into(),
        format!("{frames_filter}subtitles={ASS_NAME},format=nv12"),
        "-c:v".into(),
        "h264_mf".into(),
        "-hw_encoding".into(),
        "1".into(),
        "-rate_control".into(),
        "quality".into(),
        "-quality".into(),
        quality.to_string(),
    ];
    for t in tuning {
        args.push(t.into());
    }
    args.extend(["-profile:v".into(), "100".into(), "-g".into(), "300".into()]);
    for s in sound_args {
        args.push(s.into());
    }
    args.extend([
        "-movflags".into(),
        "+faststart".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-stats_period".into(),
        "0.5".into(),
        output_name.clone(),
    ]);

    let run_res = run_with_progress(ffmpeg, &args, Some(folder), progress, || false);
    let _ = fs::remove_file(&ass_path);

    let (code, errors) = run_res?;
    if code != 0 {
        let tail: String = errors
            .trim()
            .chars()
            .rev()
            .take(300)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return Err(format!("Making the subtitled video failed: {tail}"));
    }
    Ok(folder.join(output_name))
}
