//! Windows recorder: supports BOTH `RecordMode::ScreenAndAudio` (two FFmpeg processes: screen
//! via `gfxcapture` + hardware H.264, audio via WASAPI pipes -> `recording.mp4`) AND
//! `RecordMode::AudioOnly` (single lightweight FFmpeg audio process -> `recording.m4a`, zero GPU
//! screen capture overhead!).

use crate::engine::ffmpeg_locator::{configure_no_window, find_ffmpeg};
use crate::engine::wasapi::AudioDevice;
use crate::settings::RecordMode;
use regex::Regex;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

pub const STT_SAMPLE_RATE: usize = 16_000;

pub fn quality_value(quality: &str) -> u32 {
    match quality {
        "best" => 85,
        "smallest" => 55,
        _ => 70,
    }
}

pub const QUALITIES: &[&str] = &["best", "balanced", "smallest"];

pub const STEREO_48K: &str = "aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo";
pub const MONO_16K: &str = "aresample=16000,aformat=sample_fmts=flt:channel_layouts=mono";

pub fn mix_graph() -> String {
    format!(
        "[0:a]{STEREO_48K},asplit[mic][mic_stt];[1:a]{STEREO_48K},asplit[computer][computer_stt];\
         [mic][computer]amix=inputs=2:duration=first:normalize=0,alimiter=level=false:latency=true[rec];\
         [mic_stt]{MONO_16K}[mic16];[computer_stt]{MONO_16K}[computer16];\
         [mic16][computer16]amerge=inputs=2[stt]"
    )
}

fn first_frame_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"Parsed_showinfo.*\bn:\s*0\s+pts:").unwrap())
}

#[derive(Debug, Clone)]
pub struct RecorderSettings {
    pub microphone: Option<String>,
    pub quality: String,
    pub fps: u32,
    pub max_seconds: u32,
    pub system_audio: bool,
    pub mode: RecordMode,
    pub live_transcription: bool,
}

impl Default for RecorderSettings {
    fn default() -> Self {
        Self {
            microphone: None,
            quality: "balanced".to_string(),
            fps: 30,
            max_seconds: 4 * 3600,
            system_audio: false,
            mode: RecordMode::ScreenAndAudio,
            live_transcription: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecorderProcessSpec {
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
    pub speech: bool,
    pub speech_channels: usize,
}

#[derive(Debug, Clone)]
pub struct PipeInputSpec {
    pub ffmpeg_input: Vec<String>,
}

/// Picks the microphone to record: requested by id or legacy name, else Windows default, else first.
pub fn pick_microphone<'a>(
    devices: &'a [AudioDevice],
    wanted: Option<&str>,
    default_id: Option<&str>,
) -> Option<&'a AudioDevice> {
    if let Some(w) = wanted {
        if let Some(dev) = devices.iter().find(|d| d.id == w || d.name == w) {
            return Some(dev);
        }
    }
    if let Some(def) = default_id {
        if let Some(dev) = devices.iter().find(|d| d.id == def) {
            return Some(dev);
        }
    }
    devices.first()
}

/// Shows a 1x1 pixel window in the corner of the primary monitor briefly so `gfxcapture` emits
/// a frame and FFmpeg notices `"q"` promptly on a still screen.
#[cfg(windows)]
pub fn wake_screen(monitor_index: usize) {
    use std::thread;
    use std::time::{Duration, Instant};
    use windows::core::w;
    use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, PeekMessageW, TranslateMessage, MSG,
        PM_REMOVE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
    };

    unsafe extern "system" fn enum_proc(
        _monitor: HMONITOR,
        _dc: HDC,
        rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let corners = &mut *(data.0 as *mut Vec<(i32, i32)>);
        if !rect.is_null() {
            corners.push(((*rect).left, (*rect).top));
        }
        BOOL(1)
    }

    let mut corners: Vec<(i32, i32)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(enum_proc),
            LPARAM(&mut corners as *mut _ as isize),
        );
    }
    let (left, top) = corners.get(monitor_index).copied().unwrap_or((0, 0));

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("STATIC"),
            w!(""),
            WS_POPUP | WS_VISIBLE,
            left,
            top,
            1,
            1,
            None,
            None,
            None,
            None,
        )
    };
    let mut hwnd_opt = hwnd.ok();
    let mut msg = MSG::default();
    for _ in 0..2 {
        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline {
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        if let Some(h) = hwnd_opt.take() {
            unsafe {
                let _ = DestroyWindow(h);
            }
        }
    }
}

#[cfg(not(windows))]
pub fn wake_screen(_monitor_index: usize) {}

#[derive(Debug, Clone)]
pub struct WindowsRecorder {
    pub ffmpeg: String,
}

impl WindowsRecorder {
    pub fn new(ffmpeg: String) -> Self {
        Self { ffmpeg }
    }

    pub fn default_recorder() -> Result<Self, String> {
        let ffmpeg = find_ffmpeg()
            .ok_or_else(|| "ffmpeg was not found. Install it with: winget install ffmpeg".to_string())?;
        Ok(Self::new(ffmpeg))
    }

    pub fn output_name(mode: RecordMode) -> &'static str {
        match mode {
            RecordMode::ScreenAndAudio => "recording.mp4",
            RecordMode::AudioOnly => "recording.m4a",
        }
    }

    pub fn screen_name() -> &'static str {
        "screen.mp4"
    }

    pub fn audio_part_name(mode: RecordMode) -> &'static str {
        match mode {
            RecordMode::ScreenAndAudio => "audio.m4a",
            RecordMode::AudioOnly => "recording.m4a",
        }
    }

    pub fn microphones(&self) -> Result<(Vec<AudioDevice>, Option<String>), String> {
        crate::engine::wasapi::capture_devices()
    }

    pub fn processes(
        &self,
        folder: &Path,
        settings: &RecorderSettings,
        sources: &[PipeInputSpec],
    ) -> Vec<RecorderProcessSpec> {
        let mut procs = Vec::new();
        if settings.mode == RecordMode::ScreenAndAudio {
            procs.push(RecorderProcessSpec {
                name: "screen".to_string(),
                program: self.ffmpeg.clone(),
                args: self.screen_args(folder, settings),
                speech: false,
                speech_channels: 1,
            });
        }
        procs.push(RecorderProcessSpec {
            name: "audio".to_string(),
            program: self.ffmpeg.clone(),
            args: self.audio_args(folder, settings, sources),
            speech: true,
            speech_channels: if sources.len() > 1 { 2 } else { 1 },
        });
        procs
    }

    fn screen_args(&self, folder: &Path, settings: &RecorderSettings) -> Vec<String> {
        let graph = format!(
            "gfxcapture=monitor_idx=0:max_framerate={},showinfo=checksum=0[screen]",
            settings.fps
        );
        vec![
            "-hide_banner".into(),
            "-nostats".into(),
            "-stats_period".into(),
            "0.1".into(),
            "-loglevel".into(),
            "level+34".into(),
            "-filter_complex".into(),
            graph,
            "-map".into(),
            "[screen]".into(),
            "-t".into(),
            settings.max_seconds.to_string(),
            "-fps_mode:v".into(),
            "vfr".into(),
            "-c:v".into(),
            "h264_mf".into(),
            "-hw_encoding".into(),
            "1".into(),
            "-rate_control".into(),
            "quality".into(),
            "-quality".into(),
            quality_value(&settings.quality).to_string(),
            "-scenario".into(),
            "display_remoting".into(),
            "-profile:v".into(),
            "100".into(),
            "-g".into(),
            "300".into(),
            folder.join(Self::screen_name()).to_string_lossy().into_owned(),
        ]
    }

    fn audio_args(
        &self,
        folder: &Path,
        settings: &RecorderSettings,
        sources: &[PipeInputSpec],
    ) -> Vec<String> {
        let seconds = settings.max_seconds.to_string();
        let mut args = vec![
            "-hide_banner".into(),
            "-nostats".into(),
            "-stats_period".into(),
            "0.1".into(),
            "-loglevel".into(),
            "level+warning".into(),
        ];

        let (rec_map, stt_map, channels) = if sources.len() > 1 {
            args.push("-filter_complex".into());
            args.push(mix_graph());
            ("[rec]", "[stt]", "2")
        } else {
            ("0:a", "0:a", "1")
        };

        for src in sources {
            args.push("-thread_queue_size".into());
            args.push("1024".into());
            args.extend(src.ffmpeg_input.iter().cloned());
        }

        let out_file = folder
            .join(Self::audio_part_name(settings.mode))
            .to_string_lossy()
            .into_owned();

        args.extend([
            "-map".into(),
            rec_map.into(),
            "-t".into(),
            seconds.clone(),
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "128k".into(),
        ]);
        if settings.mode == RecordMode::AudioOnly {
            args.push("-movflags".into());
            args.push("+faststart".into());
        }
        args.push(out_file);

        args.extend([
            "-map".into(),
            stt_map.into(),
            "-t".into(),
            seconds,
            "-ac".into(),
            channels.into(),
            "-ar".into(),
            STT_SAMPLE_RATE.to_string(),
            "-f".into(),
            "f32le".into(),
            "pipe:1".into(),
        ]);
        args
    }

    /// Muxes `screen.mp4` + `audio.m4a` into `recording.mp4` for Screen+Audio mode.
    /// For AudioOnly mode, `recording.m4a` is already complete — zero extra FFmpeg work!
    pub fn finish(&self, folder: &Path, mode: RecordMode) -> Result<(), String> {
        if mode == RecordMode::AudioOnly {
            return Ok(());
        }
        let parts = [folder.join(Self::screen_name()), folder.join("audio.m4a")];
        let present: Vec<&std::path::PathBuf> = parts
            .iter()
            .filter(|p| fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false))
            .collect();
        if present.is_empty() {
            return Ok(());
        }

        let mut cmd = Command::new(&self.ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "warning", "-y"]);
        for part in &present {
            cmd.arg("-i").arg(part);
        }
        for i in 0..present.len() {
            cmd.arg("-map").arg(i.to_string());
        }
        cmd.args(["-c", "copy", "-movflags", "+faststart"])
            .arg(folder.join(Self::output_name(mode)));
        configure_no_window(&mut cmd);

        let output = cmd
            .output()
            .map_err(|e| format!("Running ffmpeg mux failed: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: String = stderr
                .trim()
                .chars()
                .rev()
                .take(300)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            return Err(format!("Putting the screen and the sound together failed: {tail}"));
        }
        for part in present {
            let _ = fs::remove_file(part);
        }
        Ok(())
    }

    pub fn screen_started(line: &str) -> bool {
        first_frame_regex().is_match(line)
    }

    pub fn frame_report(line: &str) -> bool {
        line.contains("Parsed_showinfo")
    }
}
