//! Persistent settings stored in `%LOCALAPPDATA%\Cronista\settings.json`.
//! Fully backward-compatible with Cronista 0.8.0's JSON schema, plus Rust-native
//! fields (`record_mode`, `live_transcription`, `model_idle_unload_secs`).

use crate::engine::voices::SOURCES;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub const THEMES: &[&str] = &[
    "system", "dark", "light", "midnight", "ocean", "forest", "grape", "sunset", "paper",
    "contrast",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecordMode {
    #[default]
    ScreenAndAudio,
    AudioOnly,
}

impl RecordMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RecordMode::ScreenAndAudio => "screen",
            RecordMode::AudioOnly => "audio",
        }
    }

    pub fn from_str_opt(s: Option<&str>) -> Self {
        match s {
            Some("audio") | Some("audio_only") => RecordMode::AudioOnly,
            _ => RecordMode::ScreenAndAudio,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppSettings {
    pub microphone: Option<String>,
    pub quality: String,
    pub system_audio: bool,
    pub language: Option<String>,
    pub tour_seen: bool,
    pub labels: HashMap<String, String>,
    pub subtitles: bool,
    pub theme: String,
    pub transcription: String,
    pub record_mode: RecordMode,
    /// When false, skips live STT during recording and only transcribes in the final pass
    /// (near-zero CPU/RAM while recording!).
    pub live_transcription: bool,
    /// Seconds of idle time before unloading Parakeet from RAM (Kiri-style ~20MB idle footprint).
    pub model_idle_unload_secs: u64,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            microphone: None,
            quality: "balanced".to_string(),
            system_audio: true,
            language: None,
            tour_seen: false,
            labels: HashMap::new(),
            subtitles: false,
            theme: "system".to_string(),
            transcription: "standard".to_string(),
            record_mode: RecordMode::ScreenAndAudio,
            live_transcription: true,
            model_idle_unload_secs: 60,
        }
    }
}

pub fn clean_labels(raw: &HashMap<String, String>) -> HashMap<String, String> {
    let mut clean = HashMap::new();
    for &source in SOURCES {
        if let Some(val) = raw.get(source) {
            let trimmed: String = val.split_whitespace().collect::<Vec<_>>().join(" ");
            let truncated: String = trimmed.chars().take(40).collect();
            if !truncated.is_empty() {
                clean.insert(source.to_string(), truncated);
            }
        }
    }
    clean
}

pub fn load_raw(path: &Path) -> Map<String, Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

pub fn load_settings(path: &Path) -> AppSettings {
    let raw = load_raw(path);
    let mut s = AppSettings::default();

    s.microphone = raw
        .get("microphone")
        .and_then(|v| v.as_str())
        .map(|v| v.to_string());
    if let Some(q) = raw.get("quality").and_then(|v| v.as_str()) {
        if matches!(q, "best" | "balanced" | "smallest") {
            s.quality = q.to_string();
        }
    }
    if let Some(sa) = raw.get("system_audio").and_then(|v| v.as_bool()) {
        s.system_audio = sa;
    }
    if let Some(lang) = raw.get("language").and_then(|v| v.as_str()) {
        if matches!(lang, "en" | "es") {
            s.language = Some(lang.to_string());
        }
    }
    if let Some(ts) = raw.get("tour_seen").and_then(|v| v.as_bool()) {
        s.tour_seen = ts;
    }
    if let Some(obj) = raw.get("labels").and_then(|v| v.as_object()) {
        let mut map = HashMap::new();
        for (k, v) in obj {
            if let Some(val) = v.as_str() {
                map.insert(k.clone(), val.to_string());
            }
        }
        s.labels = clean_labels(&map);
    }
    if let Some(sub) = raw.get("subtitles").and_then(|v| v.as_bool()) {
        s.subtitles = sub;
    }
    if let Some(theme) = raw.get("theme").and_then(|v| v.as_str()) {
        if THEMES.contains(&theme) {
            s.theme = theme.to_string();
        }
    }
    if let Some(tr) = raw.get("transcription").and_then(|v| v.as_str()) {
        if matches!(tr, "standard" | "high") {
            s.transcription = tr.to_string();
        }
    }
    s.record_mode = RecordMode::from_str_opt(raw.get("record_mode").and_then(|v| v.as_str()));
    if let Some(live) = raw.get("live_transcription").and_then(|v| v.as_bool()) {
        s.live_transcription = live;
    }
    if let Some(secs) = raw.get("model_idle_unload_secs").and_then(|v| v.as_u64()) {
        s.model_idle_unload_secs = secs;
    }
    s
}

pub fn save_settings_patch(path: &Path, patch: Map<String, Value>) -> std::io::Result<()> {
    let mut merged = load_raw(path);
    for (k, v) in patch {
        merged.insert(k, v);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string(&Value::Object(merged))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(path, text)
}
