//! Finds `ffmpeg` / `ffprobe` and runs FFmpeg commands with progress reporting and no console flash.

use crate::paths::app_root;
use std::env;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

pub fn configure_no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// Runs an FFmpeg command that includes `-progress pipe:1`, calling `progress(seconds)`
/// as FFmpeg reports `out_time_us=...`. Kills FFmpeg promptly if `stopped()` becomes true.
pub fn run_with_progress<P, S>(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    mut progress: P,
    stopped: S,
) -> Result<(i32, String), String>
where
    P: FnMut(f64),
    S: Fn() -> bool,
{
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    configure_no_window(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to start {program}: {e}"))?;

    let stderr = child.stderr.take();
    let stderr_handle = thread::spawn(move || {
        let mut buf = String::new();
        if let Some(err) = stderr {
            let mut reader = BufReader::new(err);
            let _ = std::io::Read::read_to_string(&mut reader, &mut buf);
        }
        buf
    });

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            if stopped() {
                let _ = child.kill();
                break;
            }
            if let Ok(line) = line {
                if let Some((key, val)) = line.trim().split_once('=') {
                    if key == "out_time_us" {
                        if let Ok(us) = val.parse::<u64>() {
                            progress(us as f64 / 1e6);
                        }
                    }
                }
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("Waiting for {program} failed: {e}"))?;
    let errors = stderr_handle.join().unwrap_or_default();
    Ok((status.code().unwrap_or(-1), errors))
}

fn find_in_path(binary: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub fn find_ffmpeg() -> Option<String> {
    // 1. Bundled next to the executable
    let bundled = app_root().join("ffmpeg").join("ffmpeg.exe");
    if bundled.is_file() {
        return Some(bundled.to_string_lossy().into_owned());
    }
    let beside = app_root().join("ffmpeg.exe");
    if beside.is_file() {
        return Some(beside.to_string_lossy().into_owned());
    }

    // 2. PATH
    let exe_name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Some(p) = find_in_path(exe_name) {
        return Some(p.to_string_lossy().into_owned());
    }

    // 3. Common Windows locations (WinGet, Scoop, C:\ffmpeg)
    #[cfg(windows)]
    {
        let local_app_data = env::var("LOCALAPPDATA").unwrap_or_default();
        let user_profile = env::var("USERPROFILE").unwrap_or_default();
        let common = [
            PathBuf::from(r"C:\ffmpeg\bin\ffmpeg.exe"),
            PathBuf::from(r"C:\Program Files\ffmpeg\bin\ffmpeg.exe"),
            PathBuf::from(&local_app_data)
                .join("Microsoft")
                .join("WinGet")
                .join("Links")
                .join("ffmpeg.exe"),
            PathBuf::from(&user_profile)
                .join("scoop")
                .join("apps")
                .join("ffmpeg")
                .join("current")
                .join("bin")
                .join("ffmpeg.exe"),
        ];
        for p in common {
            if p.is_file() {
                return Some(p.to_string_lossy().into_owned());
            }
        }
        let winget_packages = PathBuf::from(&local_app_data)
            .join("Microsoft")
            .join("WinGet")
            .join("Packages");
        if let Ok(entries) = std::fs::read_dir(&winget_packages) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if name.contains("ffmpeg") || name.contains("gyan") {
                    if let Some(found) = walk_find(&entry.path(), "ffmpeg.exe", 4) {
                        return Some(found.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }

    None
}

#[cfg(windows)]
fn walk_find(dir: &Path, target: &str, depth: usize) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() && p.file_name().and_then(|n| n.to_str()) == Some(target) {
            return Some(p);
        }
        if p.is_dir() {
            if let Some(found) = walk_find(&p, target, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

pub fn find_ffprobe(ffmpeg: &str) -> String {
    let p = Path::new(ffmpeg);
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
    let name = if ext.is_empty() {
        "ffprobe".to_string()
    } else {
        format!("ffprobe.{ext}")
    };
    p.with_file_name(name).to_string_lossy().into_owned()
}
