//! Gapless QPC-synchronized audio timeline, StartGate, Windows NamedPipeWriter, and AudioSource.
//! Exact behavioral parity with `cronista/engine/audio_sources.py`.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const SAMPLE_RATE: usize = 48_000;
pub const TICKS_PER_SECOND: i64 = 10_000_000; // QPC in 100 ns ticks
pub const POLL_MS: u64 = 10;
pub const CHECK_DEVICE_SECS: f64 = 1.0;
pub const FALLBACK_SECONDS: f64 = 10.0;

pub type NoticeFn = Arc<dyn Fn(String) + Send + Sync>;

/// Current QPC clock in 100 ns ticks (matches Windows WASAPI `pu64QPCPosition`).
pub fn qpc_now() -> i64 {
    #[cfg(windows)]
    {
        use std::sync::OnceLock;
        use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
        static FREQ: OnceLock<i64> = OnceLock::new();
        let freq = *FREQ.get_or_init(|| {
            let mut f = 0i64;
            unsafe {
                let _ = QueryPerformanceFrequency(&mut f);
            }
            f.max(1)
        });
        let mut counter = 0i64;
        unsafe {
            let _ = QueryPerformanceCounter(&mut counter);
        }
        ((counter as i128 * TICKS_PER_SECOND as i128) / freq as i128) as i64
    }
    #[cfg(not(windows))]
    {
        use std::sync::OnceLock;
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        let origin = ORIGIN.get_or_init(Instant::now);
        (origin.elapsed().as_nanos() / 100) as i64
    }
}

/// Turns timestamped packets into one gapless stream starting at QPC time `start`.
pub struct AudioTimeline {
    pub start: i64,
    pub rate: usize,
    pub frame_bytes: usize,
    pub written: i64,
    pub corrections: usize,
    lag: i64,
    slack: i64,
    flowing: bool,
}

impl AudioTimeline {
    pub fn new(start: i64, rate: usize, frame_bytes: usize, lag_seconds: f64, slack_seconds: f64) -> Self {
        Self {
            start,
            rate,
            frame_bytes,
            written: 0,
            corrections: 0,
            lag: (lag_seconds * rate as f64).round() as i64,
            slack: (slack_seconds * rate as f64).round() as i64,
            flowing: false,
        }
    }

    pub fn position(&self, qpc: i64) -> i64 {
        let num = (qpc - self.start) as i128 * self.rate as i128;
        // Floor division matching Python `//` for negative timestamps before start!
        let den = TICKS_PER_SECOND as i128;
        num.div_euclid(den) as i64
    }

    pub fn packet(&mut self, data: &[u8], qpc: i64, discontinuity: bool) -> Vec<u8> {
        let frames = (data.len() / self.frame_bytes) as i64;
        let mut target = self.position(qpc);
        if self.flowing {
            if !discontinuity && (target - self.written).abs() <= self.slack {
                target = self.written;
            } else if target != self.written {
                self.corrections += 1;
            }
        }
        if target + frames <= self.written {
            return Vec::new();
        }
        self.flowing = true;
        if target >= self.written {
            let gap = (target - self.written) as usize;
            self.written = target + frames;
            let mut out = vec![0u8; gap * self.frame_bytes + data.len()];
            out[gap * self.frame_bytes..].copy_from_slice(data);
            out
        } else {
            let covered = (self.written - target) as usize;
            self.written = target + frames;
            data[covered * self.frame_bytes..].to_vec()
        }
    }

    pub fn idle(&mut self, now: i64) -> Vec<u8> {
        let target = self.position(now) - self.lag;
        if target <= self.written {
            return Vec::new();
        }
        let gap = (target - self.written) as usize;
        self.written = target;
        self.flowing = false;
        vec![0u8; gap * self.frame_bytes]
    }
}

/// Shared QPC start time for all audio sources in a recording.
#[derive(Default)]
pub struct StartGate {
    inner: Mutex<(Option<i64>, String)>,
}

impl StartGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(&self) -> Option<i64> {
        self.inner.lock().unwrap().0
    }

    pub fn how(&self) -> String {
        self.inner.lock().unwrap().1.clone()
    }

    pub fn open(&self, start: i64, how: &str) -> i64 {
        let mut guard = self.inner.lock().unwrap();
        if guard.0.is_none() {
            guard.0 = Some(start);
            guard.1 = how.to_string();
        }
        guard.0.unwrap()
    }
}

/// Pluggable audio endpoint (real WASAPI or fake for unit tests).
pub trait AudioEndpoint: Send {
    fn open(&mut self) {}
    fn read(&mut self) -> Vec<(Vec<u8>, i64, bool)>;
    fn close(&mut self) {}
}

#[cfg(windows)]
pub struct NamedPipeWriter {
    pub path: String,
    handle: Option<windows::Win32::Foundation::HANDLE>,
}

#[cfg(windows)]
unsafe impl Send for NamedPipeWriter {}

#[cfg(windows)]
impl NamedPipeWriter {
    pub fn new(buffer_bytes: u32) -> Result<Self, String> {
        use windows::core::HSTRING;
        use windows::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_OUTBOUND,
        };
        use windows::Win32::System::Pipes::{
            CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
        };

        let id = uuid::Uuid::new_v4().simple().to_string();
        let path = format!(r"\\.\pipe\cronista-{}-{}", std::process::id(), &id[..12]);
        let wide = HSTRING::from(&path);
        let handle = unsafe {
            CreateNamedPipeW(
                &wide,
                PIPE_ACCESS_OUTBOUND | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                buffer_bytes,
                0,
                0,
                None,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "CreateNamedPipeW failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self {
            path,
            handle: Some(handle),
        })
    }

    pub fn wait_for_reader(&self, stop: &AtomicBool) -> Result<bool, std::io::Error> {
        use windows::Win32::Foundation::{
            CloseHandle, GetLastError, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED,
            ERROR_PIPE_CONNECTED,
        };
        use windows::Win32::System::Pipes::ConnectNamedPipe;
        use windows::Win32::System::Threading::CreateEventW;
        use windows::Win32::System::IO::OVERLAPPED;

        let handle = self.handle.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe closed")
        })?;
        let event = unsafe { CreateEventW(None, true, false, None) }
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        let mut overlapped = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };

        let res = unsafe { ConnectNamedPipe(handle, Some(&mut overlapped)) };
        let result = if res.is_ok() {
            Ok(true)
        } else {
            let err = unsafe { GetLastError() };
            if err == ERROR_PIPE_CONNECTED {
                Ok(true)
            } else if err == ERROR_IO_PENDING {
                self.complete_overlapped(handle, &mut overlapped, stop)
                    .map(|ok| ok)
                    .or_else(|e| {
                        if e.raw_os_error() == Some(ERROR_OPERATION_ABORTED.0 as i32) {
                            Ok(false)
                        } else {
                            Err(e)
                        }
                    })
            } else {
                Err(std::io::Error::from_raw_os_error(err.0 as i32))
            }
        };
        unsafe {
            let _ = CloseHandle(event);
        }
        result
    }

    pub fn write(&self, data: &[u8], stop: &AtomicBool) -> Result<(), std::io::Error> {
        use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_IO_PENDING};
        use windows::Win32::Storage::FileSystem::WriteFile;
        use windows::Win32::System::Threading::CreateEventW;
        use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};

        if data.is_empty() {
            return Ok(());
        }
        let handle = self.handle.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe closed")
        })?;
        let event = unsafe { CreateEventW(None, true, false, None) }
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        let mut overlapped = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };

        let mut written = 0u32;
        let res = unsafe { WriteFile(handle, Some(data), Some(&mut written), Some(&mut overlapped)) };
        let out = if res.is_ok() {
            let mut n = 0u32;
            unsafe { GetOverlappedResult(handle, &overlapped, &mut n, true) }
                .map_err(|_| std::io::Error::last_os_error())
                .map(|_| ())
        } else {
            let err = unsafe { GetLastError() };
            if err == ERROR_IO_PENDING {
                self.complete_overlapped(handle, &mut overlapped, stop).map(|_| ())
            } else {
                Err(std::io::Error::from_raw_os_error(err.0 as i32))
            }
        };
        unsafe {
            let _ = CloseHandle(event);
        }
        out
    }

    fn complete_overlapped(
        &self,
        handle: windows::Win32::Foundation::HANDLE,
        overlapped: &mut windows::Win32::System::IO::OVERLAPPED,
        stop: &AtomicBool,
    ) -> Result<bool, std::io::Error> {
        use windows::Win32::Foundation::{
            GetLastError, ERROR_OPERATION_ABORTED, WAIT_OBJECT_0,
        };
        use windows::Win32::System::Threading::WaitForSingleObject;
        use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult};

        loop {
            let wait = unsafe { WaitForSingleObject(overlapped.hEvent, 100) };
            if wait == WAIT_OBJECT_0 {
                break;
            }
            if stop.load(Ordering::Relaxed) {
                unsafe {
                    let _ = CancelIoEx(handle, Some(overlapped));
                }
                break;
            }
        }
        let mut transferred = 0u32;
        let res = unsafe { GetOverlappedResult(handle, overlapped, &mut transferred, true) };
        match res {
            Ok(()) => Ok(true),
            Err(_) => {
                let err = unsafe { GetLastError() };
                if err == ERROR_OPERATION_ABORTED {
                    Ok(false)
                } else {
                    Err(std::io::Error::from_raw_os_error(err.0 as i32))
                }
            }
        }
    }

    pub fn close(&mut self) {
        if let Some(h) = self.handle.take() {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(h);
            }
        }
    }
}

#[cfg(windows)]
impl Drop for NamedPipeWriter {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(windows)]
pub struct WasapiEndpoint {
    device_id: Option<String>,
    channels: u16,
    on_notice: NoticeFn,
    label: String,
    missing_msg: String,
    noticed: HashSet<String>,
    com: Option<crate::engine::wasapi::ComGuard>,
    enumerator: Option<windows::Win32::Media::Audio::IMMDeviceEnumerator>,
    open_id: Option<String>,
    stream: Option<crate::engine::wasapi::WasapiStream>,
    next_check: Instant,
}

#[cfg(windows)]
unsafe impl Send for WasapiEndpoint {}

#[cfg(windows)]
impl WasapiEndpoint {
    pub fn new(
        device_id: Option<String>,
        channels: u16,
        on_notice: NoticeFn,
        label: &str,
        missing_msg: &str,
    ) -> Self {
        Self {
            device_id,
            channels,
            on_notice,
            label: label.to_string(),
            missing_msg: missing_msg.to_string(),
            noticed: HashSet::new(),
            com: None,
            enumerator: None,
            open_id: None,
            stream: None,
            next_check: Instant::now() - Duration::from_secs(10),
        }
    }

    fn notice_once(&mut self, key: &str, msg: String) -> bool {
        if self.noticed.insert(key.to_string()) {
            (self.on_notice)(msg);
            true
        } else {
            false
        }
    }

    fn target(&self) -> Option<String> {
        let enumerator = self.enumerator.as_ref()?;
        match &self.device_id {
            None => crate::engine::wasapi::default_output_id(enumerator).ok(),
            Some(id) => {
                if crate::engine::wasapi::is_active(enumerator, id) {
                    Some(id.clone())
                } else {
                    None
                }
            }
        }
    }

    fn follow(&mut self) {
        let target = self.target();
        let Some(target) = target else {
            self.stream = None;
            self.open_id = None;
            let msg = self.missing_msg.clone();
            self.notice_once("missing", msg);
            return;
        };
        if self.open_id.as_deref() == Some(target.as_str()) && self.stream.is_some() {
            return;
        }
        self.stream = None;
        self.open_id = None;
        let Some(enumerator) = self.enumerator.as_ref() else {
            return;
        };
        match crate::engine::wasapi::WasapiStream::open(
            enumerator,
            &target,
            SAMPLE_RATE as u32,
            self.channels,
            self.device_id.is_none(),
        ) {
            Ok(s) => {
                self.stream = Some(s);
                self.open_id = Some(target);
            }
            Err(err) => {
                let msg = format!("{} can't be recorded: {err}", self.label);
                self.notice_once("open-failed", msg);
            }
        }
    }
}

#[cfg(windows)]
impl AudioEndpoint for WasapiEndpoint {
    fn open(&mut self) {
        self.com = Some(crate::engine::wasapi::ComGuard::init());
        match crate::engine::wasapi::device_enumerator() {
            Ok(e) => self.enumerator = Some(e),
            Err(err) => {
                let msg = format!("{} can't be recorded: {err}", self.label);
                self.notice_once("unavailable", msg);
            }
        }
    }

    fn read(&mut self) -> Vec<(Vec<u8>, i64, bool)> {
        if self.enumerator.is_none() {
            return Vec::new();
        }
        let now = Instant::now();
        if now >= self.next_check {
            self.next_check = now + Duration::from_secs_f64(CHECK_DEVICE_SECS);
            self.follow();
        }
        let Some(stream) = self.stream.as_ref() else {
            return Vec::new();
        };
        match stream.read_packets() {
            Ok(pkts) => pkts,
            Err(_) => {
                self.stream = None;
                self.open_id = None;
                Vec::new()
            }
        }
    }

    fn close(&mut self) {
        self.stream = None;
        self.open_id = None;
        self.enumerator = None;
        self.com = None;
    }
}

#[cfg(windows)]
pub struct AudioSource {
    pub label: String,
    pub channels: usize,
    pub path: String,
    stop: Arc<AtomicBool>,
    summary: Arc<Mutex<String>>,
    pipe: Option<NamedPipeWriter>,
    endpoint: Option<Box<dyn AudioEndpoint>>,
    gate: Arc<StartGate>,
    lag_seconds: f64,
    fallback_ticks: i64,
    thread: Option<JoinHandle<()>>,
}

#[cfg(windows)]
impl AudioSource {
    pub fn new(
        label: &str,
        endpoint: Box<dyn AudioEndpoint>,
        channels: usize,
        gate: Arc<StartGate>,
        lag_seconds: f64,
        fallback_seconds: f64,
    ) -> Result<Self, String> {
        let pipe = NamedPipeWriter::new(1 << 18)?;
        let path = pipe.path.clone();
        Ok(Self {
            label: label.to_string(),
            channels,
            path,
            stop: Arc::new(AtomicBool::new(false)),
            summary: Arc::new(Mutex::new(format!("{label}: nothing written"))),
            pipe: Some(pipe),
            endpoint: Some(endpoint),
            gate,
            lag_seconds,
            fallback_ticks: (fallback_seconds * TICKS_PER_SECOND as f64).round() as i64,
            thread: None,
        })
    }

    pub fn microphone(device_id: String, gate: Arc<StartGate>, on_notice: NoticeFn) -> Result<Self, String> {
        let ep = WasapiEndpoint::new(
            Some(device_id),
            1,
            on_notice,
            "The microphone",
            "The microphone was disconnected, so it is not being recorded. It comes back when it is plugged in again.",
        );
        Self::new("microphone", Box::new(ep), 1, gate, 0.2, FALLBACK_SECONDS)
    }

    pub fn computer_sound(gate: Arc<StartGate>, on_notice: NoticeFn) -> Result<Self, String> {
        let ep = WasapiEndpoint::new(
            None,
            2,
            on_notice,
            "The computer's sound",
            "No speakers or headphones found, so the computer's sound is not being recorded. It starts when one is connected.",
        );
        Self::new("computer sound", Box::new(ep), 2, gate, 0.06, FALLBACK_SECONDS)
    }

    pub fn ffmpeg_input(&self) -> Vec<String> {
        let layout = if self.channels == 1 { "mono" } else { "stereo" };
        vec![
            "-f".into(),
            "f32le".into(),
            "-sample_rate".into(),
            SAMPLE_RATE.to_string(),
            "-ch_layout".into(),
            layout.into(),
            "-i".into(),
            self.path.clone(),
        ]
    }

    pub fn start(&mut self) {
        let Some(mut pipe) = self.pipe.take() else {
            return;
        };
        let Some(mut endpoint) = self.endpoint.take() else {
            return;
        };
        let stop = Arc::clone(&self.stop);
        let gate = Arc::clone(&self.gate);
        let summary = Arc::clone(&self.summary);
        let label = self.label.clone();
        let frame_bytes = 4 * self.channels;
        let lag = self.lag_seconds;
        let fallback = self.fallback_ticks;

        let handle = thread::Builder::new()
            .name(format!("audio: {label}"))
            .spawn(move || {
                let connected_ok = match pipe.wait_for_reader(&stop) {
                    Ok(v) => v,
                    Err(_) => false,
                };
                if !connected_ok {
                    pipe.close();
                    return;
                }
                let connected = qpc_now();
                let mut early: Vec<(Vec<u8>, i64, bool)> = Vec::new();
                let mut timeline: Option<AudioTimeline> = None;

                endpoint.open();
                while !stop.load(Ordering::Relaxed) {
                    let mut packets = endpoint.read();
                    if timeline.is_none() {
                        early.extend(packets);
                        let mut start = gate.start();
                        if start.is_none() && qpc_now() - connected > fallback {
                            start = Some(gate.open(connected, "fallback: no first frame reported"));
                        }
                        let Some(start) = start else {
                            thread::sleep(Duration::from_millis(POLL_MS));
                            continue;
                        };
                        timeline = Some(AudioTimeline::new(start, SAMPLE_RATE, frame_bytes, lag, 0.02));
                        packets = std::mem::take(&mut early);
                    }
                    let tl = timeline.as_mut().unwrap();
                    let mut broken = false;
                    for (data, raw_qpc, disc) in packets {
                        let qpc = plausible_qpc(raw_qpc, data.len(), frame_bytes);
                        let bytes = tl.packet(&data, qpc, disc);
                        if !bytes.is_empty() && pipe.write(&bytes, &stop).is_err() {
                            broken = true;
                            break;
                        }
                    }
                    if broken {
                        break;
                    }
                    let idle_bytes = tl.idle(qpc_now());
                    if !idle_bytes.is_empty() && pipe.write(&idle_bytes, &stop).is_err() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(POLL_MS));
                }
                endpoint.close();
                pipe.close();

                if let Some(tl) = timeline {
                    let mut s = summary.lock().unwrap();
                    *s = format!(
                        "{label}: {:.1} s written, {} timing corrections",
                        tl.written as f64 / SAMPLE_RATE as f64,
                        tl.corrections
                    );
                }
            })
            .expect("spawn audio thread");
        self.thread = Some(handle);
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        if let Some(mut pipe) = self.pipe.take() {
            pipe.close();
        }
    }

    pub fn summary(&self) -> String {
        self.summary.lock().unwrap().clone()
    }
}

#[cfg(windows)]
fn plausible_qpc(qpc: i64, size: usize, frame_bytes: usize) -> i64 {
    let now = qpc_now();
    if qpc > 0 && qpc < now + TICKS_PER_SECOND && now - qpc < 60 * TICKS_PER_SECOND {
        qpc
    } else {
        now - ((size / frame_bytes) as i64) * TICKS_PER_SECOND / SAMPLE_RATE as i64
    }
}
