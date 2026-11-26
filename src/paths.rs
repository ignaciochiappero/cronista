//! Where Cronista keeps its data, models, and recordings.
//! 100% compatible with the Python version's `%LOCALAPPDATA%\Cronista` and `~\Videos\Cronista`.

use std::env;
use std::path::PathBuf;

/// The history database (`library.db`), `settings.json`, downloaded models, and `cronista.log`.
pub fn data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(local) = env::var("LOCALAPPDATA") {
            return PathBuf::from(local).join("Cronista");
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("AppData")
            .join("Local")
            .join("Cronista")
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = env::var("XDG_DATA_HOME") {
            return PathBuf::from(xdg).join("cronista");
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local")
            .join("share")
            .join("cronista")
    }
}

/// One folder per recording (`~/Videos/Cronista`).
pub fn recordings_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Videos")
        .join("Cronista")
}

/// Directory containing the executable (or current working directory as fallback).
pub fn app_root() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Candidate paths for Parakeet TDT 0.6B v3 (int8):
/// 1. `<exe_dir>/models/parakeet-tdt-0.6b-v3`
/// 2. `<cwd>/models/parakeet-tdt-0.6b-v3`
/// 3. `%LOCALAPPDATA%/Cronista/models/parakeet-tdt-0.6b-v3`
pub fn parakeet_model_dir() -> PathBuf {
    let rel = PathBuf::from("models").join("parakeet-tdt-0.6b-v3");
    let beside_exe = app_root().join(&rel);
    if beside_exe.join("encoder-model.int8.onnx").is_file() {
        return beside_exe;
    }
    if let Ok(cwd) = env::current_dir() {
        let in_cwd = cwd.join(&rel);
        if in_cwd.join("encoder-model.int8.onnx").is_file() {
            return in_cwd;
        }
    }
    // Check HuggingFace hub cache if already downloaded by onnx-asr
    if let Some(home) = dirs::home_dir() {
        let snapshots = home
            .join(".cache")
            .join("huggingface")
            .join("hub")
            .join("models--istupakov--parakeet-tdt-0.6b-v3-onnx")
            .join("snapshots");
        if let Ok(entries) = std::fs::read_dir(&snapshots) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.join("encoder-model.int8.onnx").is_file() {
                    return p;
                }
            }
        }
    }
    data_dir().join(&rel)
}

/// Directory for the optional high-quality Canary 1B v2 (int8) model.
pub fn canary_model_dir() -> PathBuf {
    data_dir().join("models").join("canary-1b-v2")
}

/// Directory for the ONNX Runtime shared library (`onnxruntime.dll`).
pub fn runtime_dir() -> PathBuf {
    data_dir().join("runtime")
}
