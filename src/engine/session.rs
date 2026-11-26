//! RecordingSession: orchestrates recorder processes, WASAPI audio sources, live transcription
//! (via SpeechScheduler + VAD + LocalAgreement), and the final pass (with optional Canary refiner).

use crate::engine::audio_sources::{qpc_now, StartGate};
use crate::engine::final_pass::{
    refine_chunks_file, run_final_pass_file, run_parakeet_chunks_file, FinalPassOptions, RefinerFn,
    SpeechModel,
};
use crate::engine::local_agreement::LocalAgreement;
use crate::engine::recorders::{
    pick_microphone, wake_screen, PipeInputSpec, RecorderProcessSpec, RecorderSettings,
    WindowsRecorder, STT_SAMPLE_RATE,
};
use crate::engine::transcript::{tidy_text, TranscriptionSegment};
use crate::engine::voice_activity::{VoiceActivitySegmenter, VoiceEvent, VoiceEventKind};
use crate::engine::voices::{drop_echoes, is_echo, SOURCES};
use crate::settings::RecordMode;
use chrono::Local;
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use crate::engine::audio_sources::AudioSource;
#[cfg(windows)]
use crate::engine::ffmpeg_locator::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};
#[cfg(windows)]
use std::os::windows::process::CommandExt;

const READ_BLOCK_FRAMES: usize = 1_600; // 100 ms
const PROBLEM_TAGS: &[&str] = &["[warning]", "[error]", "[fatal]", "[panic]"];

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Listening,
    Recording { started_at: f64 },
    Partial {
        committed: String,
        pending: String,
        source: String,
    },
    Phrase {
        start: f64,
        text: String,
        source: String,
    },
    Ended { code: i32, reason: String },
    Notice { message: String },
    Finalizing {
        progress: Option<f64>,
        eta: Option<u64>,
    },
    Finished(SessionResult),
}

pub type EventSink = Arc<dyn Fn(SessionEvent) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct SessionResult {
    pub folder: PathBuf,
    pub media_path: PathBuf,
    pub mode: RecordMode,
    pub started_at: String,
    pub duration_seconds: f64,
    pub size_bytes: u64,
    pub segments: Vec<TranscriptionSegment>,
}

impl SessionResult {
    pub fn transcript(&self) -> String {
        self.segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Drops superseded partials while guaranteeing every final is transcribed.
pub struct SpeechScheduler {
    state: Mutex<SchedulerInner>,
    cond: Condvar,
}

struct SchedulerInner {
    finals: VecDeque<(usize, VoiceEvent)>,
    partials: Vec<(usize, VoiceEvent)>,
    closed: bool,
}

impl SpeechScheduler {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(SchedulerInner {
                finals: VecDeque::new(),
                partials: Vec::new(),
                closed: false,
            }),
            cond: Condvar::new(),
        }
    }

    pub fn submit(&self, channel: usize, event: VoiceEvent) {
        let mut s = self.state.lock().unwrap();
        if event.kind == VoiceEventKind::Final {
            s.finals.push_back((channel, event));
            s.partials.retain(|(ch, _)| *ch != channel);
        } else if let Some(entry) = s.partials.iter_mut().find(|(ch, _)| *ch == channel) {
            entry.1 = event;
        } else {
            s.partials.push((channel, event));
        }
        self.cond.notify_one();
    }

    pub fn close(&self) {
        let mut s = self.state.lock().unwrap();
        s.closed = true;
        self.cond.notify_all();
    }

    pub fn next(&self) -> Option<(usize, VoiceEvent)> {
        let mut s = self.state.lock().unwrap();
        while s.finals.is_empty() && s.partials.is_empty() && !s.closed {
            s = self.cond.wait(s).unwrap();
        }
        if let Some(item) = s.finals.pop_front() {
            return Some(item);
        }
        if !s.partials.is_empty() {
            return Some(s.partials.remove(0));
        }
        None
    }
}

pub type RefinerFactory = Box<dyn Fn() -> Option<Box<dyn FnMut(&[f32], Vec<String>, Vec<f64>, usize) -> (Vec<String>, Vec<f64>) + Send>> + Send + Sync>;

struct SharedLogState {
    log: File,
    last_line: String,
    last_problem: String,
    ended: bool,
}

pub struct RecordingSession {
    model: Option<Arc<dyn SpeechModel>>,
    on_release_model: Option<Box<dyn FnOnce() + Send>>,
    refiner: Option<RefinerFactory>,
    recorder: WindowsRecorder,
    settings: RecorderSettings,
    folder: PathBuf,
    on_event: EventSink,
    tape_paths: Vec<PathBuf>,
    channels: usize,
    scheduler: Arc<SpeechScheduler>,
    running: Vec<(RecorderProcessSpec, Arc<Mutex<Child>>)>,
    gate: Arc<StartGate>,
    #[cfg(windows)]
    sources: Vec<AudioSource>,
    threads: Vec<JoinHandle<()>>,
    log_readers: Vec<JoinHandle<()>>,
    samples: Arc<Mutex<usize>>,
    started_at: Arc<Mutex<Option<chrono::DateTime<Local>>>>,
    stopping: Arc<AtomicBool>,
    audio_level: Arc<AtomicU32>,
    log_state: Option<Arc<Mutex<SharedLogState>>>,
}

impl RecordingSession {
    pub fn new(
        model: Arc<dyn SpeechModel>,
        recorder: WindowsRecorder,
        settings: RecorderSettings,
        folder: PathBuf,
        on_event: EventSink,
        refiner: Option<RefinerFactory>,
    ) -> Self {
        Self {
            model: Some(model),
            on_release_model: None,
            refiner,
            recorder,
            settings,
            folder,
            on_event,
            tape_paths: Vec::new(),
            channels: 1,
            scheduler: Arc::new(SpeechScheduler::new()),
            running: Vec::new(),
            gate: Arc::new(StartGate::new()),
            #[cfg(windows)]
            sources: Vec::new(),
            threads: Vec::new(),
            log_readers: Vec::new(),
            samples: Arc::new(Mutex::new(0)),
            started_at: Arc::new(Mutex::new(None)),
            stopping: Arc::new(AtomicBool::new(false)),
            audio_level: Arc::new(AtomicU32::new(0.0_f32.to_bits())),
            log_state: None,
        }
    }

    pub fn set_audio_level_sink(&mut self, sink: Arc<AtomicU32>) {
        self.audio_level = sink;
    }

    pub fn set_on_release_model(&mut self, cb: Box<dyn FnOnce() + Send>) {
        self.on_release_model = Some(cb);
    }

    pub fn start(&mut self) -> Result<(), String> {
        fs::create_dir_all(&self.folder)
            .map_err(|e| format!("Creating recording folder failed: {e}"))?;

        let notice_sink = Arc::clone(&self.on_event);
        let on_notice: crate::engine::audio_sources::NoticeFn = Arc::new(move |msg: String| {
            (notice_sink)(SessionEvent::Notice { message: msg });
        });

        let mut pipe_specs = Vec::new();
        #[cfg(windows)]
        {
            let (devices, default_id) = self.recorder.microphones()?;
            let mic = pick_microphone(
                &devices,
                self.settings.microphone.as_deref(),
                default_id.as_deref(),
            )
            .ok_or_else(|| "No microphone found. Plug one in and try again.".to_string())?;

            let mic_src = AudioSource::microphone(mic.id.clone(), Arc::clone(&self.gate), Arc::clone(&on_notice))?;
            pipe_specs.push(PipeInputSpec {
                ffmpeg_input: mic_src.ffmpeg_input(),
            });
            self.sources.push(mic_src);

            if self.settings.system_audio {
                let comp_src = AudioSource::computer_sound(Arc::clone(&self.gate), on_notice)?;
                pipe_specs.push(PipeInputSpec {
                    ffmpeg_input: comp_src.ffmpeg_input(),
                });
                self.sources.push(comp_src);
            }
        }

        // In AudioOnly mode there is no screen first-frame event, so open the gate immediately!
        if self.settings.mode == RecordMode::AudioOnly {
            self.gate.open(qpc_now(), "audio-only immediate start");
        }

        let specs = self.recorder.processes(&self.folder, &self.settings, &pipe_specs);
        let mut speech_stdout = None;
        let mut speech_channels = 1usize;

        let mut spawned: Vec<(RecorderProcessSpec, Arc<Mutex<Child>>, Option<std::process::ChildStderr>)> =
            Vec::new();
        for spec in specs {
            let mut cmd = Command::new(&spec.program);
            cmd.args(&spec.args)
                .stdin(Stdio::piped())
                .stderr(Stdio::piped())
                .stdout(if spec.speech {
                    Stdio::piped()
                } else {
                    Stdio::null()
                });
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    for (_, ch, _) in spawned {
                        let _ = ch.lock().unwrap().kill();
                        let _ = ch.lock().unwrap().wait();
                    }
                    #[cfg(windows)]
                    for src in &mut self.sources {
                        src.stop();
                    }
                    return Err(format!("Failed to launch recorder {}: {e}", spec.name));
                }
            };
            if spec.speech {
                speech_stdout = child.stdout.take();
                speech_channels = spec.speech_channels;
            }
            let stderr = child.stderr.take();
            spawned.push((spec, Arc::new(Mutex::new(child)), stderr));
        }

        self.channels = speech_channels;
        self.tape_paths = if self.channels > 1 {
            (0..self.channels)
                .map(|ch| self.folder.join(format!("audio-16k-{}.f32", SOURCES[ch])))
                .collect()
        } else {
            vec![self.folder.join("audio-16k.f32")]
        };

        #[cfg(windows)]
        for src in &mut self.sources {
            src.start();
        }

        let log_file = File::create(self.folder.join("recorder.log"))
            .map_err(|e| format!("Failed to create recorder.log: {e}"))?;
        let log_state = Arc::new(Mutex::new(SharedLogState {
            log: log_file,
            last_line: String::new(),
            last_problem: String::new(),
            ended: false,
        }));
        self.log_state = Some(Arc::clone(&log_state));

        let several = spawned.len() > 1;
        for (spec, child, stderr) in spawned {
            self.running.push((spec.clone(), Arc::clone(&child)));
            let log_st = Arc::clone(&log_state);
            let gate = Arc::clone(&self.gate);
            let stopping = Arc::clone(&self.stopping);
            let on_event = Arc::clone(&self.on_event);
            let prefix = if several {
                format!("[{}] ", spec.name)
            } else {
                String::new()
            };
            let is_screen = spec.name == "screen";
            let reader = thread::spawn(move || {
                if let Some(err) = stderr {
                    let br = BufReader::new(err);
                    for line_res in br.lines() {
                        let Ok(line) = line_res else { break };
                        let arrived = qpc_now();
                        if is_screen && gate.start().is_none() && WindowsRecorder::screen_started(&line) {
                            gate.open(arrived, "first frame");
                            if let Ok(mut ch) = child.lock() {
                                if let Some(stdin) = ch.stdin.as_mut() {
                                    let _ = stdin.write_all(b"-");
                                    let _ = stdin.flush();
                                }
                            }
                        } else if is_screen && gate.start().is_some() && WindowsRecorder::frame_report(&line) {
                            continue;
                        }
                        let mut st = log_st.lock().unwrap();
                        if !line.is_empty() {
                            st.last_line = format!("{prefix}{line}");
                            if PROBLEM_TAGS.iter().any(|tag| line.contains(tag)) {
                                st.last_problem = format!("{prefix}{line}");
                            }
                        }
                        let _ = writeln!(st.log, "{prefix}{line}");
                    }
                }
                let code = child
                    .lock()
                    .ok()
                    .and_then(|mut c| c.wait().ok())
                    .and_then(|s| s.code())
                    .unwrap_or(-1);
                let reason = {
                    let mut st = log_st.lock().unwrap();
                    let _ = st.log.flush();
                    if stopping.load(Ordering::Relaxed) || st.ended {
                        return;
                    }
                    st.ended = true;
                    if !st.last_problem.is_empty() {
                        st.last_problem.clone()
                    } else {
                        st.last_line.clone()
                    }
                };
                (on_event)(SessionEvent::Ended { code, reason });
            });
            self.log_readers.push(reader);
        }

        // Audio reader thread: reads 16 kHz f32le from FFmpeg stdout, writes tapes to disk,
        // and feeds VoiceActivitySegmenter -> SpeechScheduler (when live transcription is on).
        let mut tapes = Vec::with_capacity(self.tape_paths.len());
        for p in &self.tape_paths {
            tapes.push(File::create(p).map_err(|e| format!("Failed to create tape file: {e}"))?);
        }
        let channels = self.channels;
        let samples_counter = Arc::clone(&self.samples);
        let started_at_ref = Arc::clone(&self.started_at);
        let on_event_audio = Arc::clone(&self.on_event);
        let scheduler_audio = Arc::clone(&self.scheduler);
        let live_stt = self.settings.live_transcription;
        let level_sink = Arc::clone(&self.audio_level);

        let read_thread = thread::spawn(move || {
            let Some(mut stdout) = speech_stdout else {
                scheduler_audio.close();
                return;
            };
            let mut segmenters: Vec<VoiceActivitySegmenter> = (0..channels)
                .map(|_| VoiceActivitySegmenter::default_16k())
                .collect();
            let frame_bytes = 4 * channels;
            let mut buf = vec![0u8; READ_BLOCK_FRAMES * frame_bytes];
            let mut started_emitted = false;
            let mut leftover = Vec::<u8>::new();

            loop {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if !started_emitted {
                            started_emitted = true;
                            *started_at_ref.lock().unwrap() = Some(Local::now());
                            let ts = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map(|d| d.as_secs_f64())
                                .unwrap_or(0.0);
                            (on_event_audio)(SessionEvent::Recording { started_at: ts });
                        }
                        leftover.extend_from_slice(&buf[..n]);
                        let usable_len = leftover.len() - (leftover.len() % frame_bytes);
                        if usable_len == 0 {
                            continue;
                        }
                        let frame_count = usable_len / frame_bytes;
                        *samples_counter.lock().unwrap() += frame_count;

                        let mut per_ch: Vec<Vec<f32>> =
                            (0..channels).map(|_| Vec::with_capacity(frame_count)).collect();
                        for frame in leftover[..usable_len].chunks_exact(frame_bytes) {
                            for ch in 0..channels {
                                let b = &frame[ch * 4..(ch + 1) * 4];
                                per_ch[ch].push(f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                            }
                        }
                        leftover.drain(..usable_len);

                        let mut max_rms = 0.0_f32;
                        for ch in 0..channels {
                            let mut sum_sq = 0.0_f32;
                            let mut raw_bytes = Vec::with_capacity(per_ch[ch].len() * 4);
                            for &s in &per_ch[ch] {
                                sum_sq += s * s;
                                raw_bytes.extend_from_slice(&s.to_le_bytes());
                            }
                            if !per_ch[ch].is_empty() {
                                let rms = (sum_sq / per_ch[ch].len() as f32).sqrt();
                                if rms > max_rms {
                                    max_rms = rms;
                                }
                            }
                            let _ = tapes[ch].write_all(&raw_bytes);
                            if live_stt {
                                for ev in segmenters[ch].push(&per_ch[ch]) {
                                    scheduler_audio.submit(ch, ev);
                                }
                            }
                        }
                        // Perceptual voice level curve (0.0..=1.0) for live organic LED matrix
                        let level = (max_rms * 7.5).sqrt().clamp(0.0, 1.0);
                        level_sink.store(level.to_bits(), Ordering::Relaxed);
                    }
                }
            }
            if live_stt {
                for (ch, seg) in segmenters.iter_mut().enumerate() {
                    for ev in seg.flush() {
                        scheduler_audio.submit(ch, ev);
                    }
                }
            }
            drop(tapes);
            level_sink.store(0.0_f32.to_bits(), Ordering::Relaxed);
            scheduler_audio.close();
        });
        self.threads.push(read_thread);

        // Live transcription worker thread
        if self.settings.live_transcription {
            let scheduler_stt = Arc::clone(&self.scheduler);
            let model_stt = Arc::clone(self.model.as_ref().expect("model present at start"));
            let on_event_stt = Arc::clone(&self.on_event);
            let stt_thread = thread::spawn(move || {
                let mut agreements: Vec<LocalAgreement> =
                    (0..channels).map(|_| LocalAgreement::default_n2()).collect();
                let mut heard_computer: VecDeque<TranscriptionSegment> = VecDeque::with_capacity(21);

                while let Some((ch, ev)) = scheduler_stt.next() {
                    let source = SOURCES[ch].to_string();
                    let raw_text = match model_stt.recognize_text(&ev.audio, STT_SAMPLE_RATE) {
                        Ok(t) => tidy_text(t.trim()),
                        Err(_) => continue,
                    };
                    if ev.kind == VoiceEventKind::Partial {
                        let snap = agreements[ch].update(Some(&raw_text));
                        (on_event_stt)(SessionEvent::Partial {
                            committed: snap.committed,
                            pending: snap.pending,
                            source,
                        });
                        continue;
                    }
                    agreements[ch].reset();
                    if raw_text.is_empty() {
                        continue;
                    }
                    let seg = TranscriptionSegment::new(
                        ev.start_sample as f64 / STT_SAMPLE_RATE as f64,
                        (ev.speech_end_sample.max(0) as f64) / STT_SAMPLE_RATE as f64,
                        raw_text.clone(),
                        Some(source.clone()),
                    );
                    if source == "mic" {
                        let comp_slice: Vec<TranscriptionSegment> = heard_computer.iter().cloned().collect();
                        if is_echo(&seg, &comp_slice, 0.6, 1.0) {
                            (on_event_stt)(SessionEvent::Partial {
                                committed: String::new(),
                                pending: String::new(),
                                source,
                            });
                            continue;
                        }
                    } else if source == "computer" {
                        if heard_computer.len() >= 20 {
                            heard_computer.pop_front();
                        }
                        heard_computer.push_back(seg.clone());
                    }
                    (on_event_stt)(SessionEvent::Phrase {
                        start: seg.start,
                        text: raw_text,
                        source,
                    });
                }
            });
            self.threads.push(stt_thread);
        }

        (self.on_event)(SessionEvent::Listening);
        Ok(())
    }

    pub fn stop(&mut self) -> SessionResult {
        self.stopping.store(true, Ordering::Relaxed);
        if self.settings.mode == RecordMode::ScreenAndAudio {
            wake_screen(0);
        }

        for (_, child) in &self.running {
            if let Ok(mut ch) = child.lock() {
                if let Some(stdin) = ch.stdin.as_mut() {
                    let _ = stdin.write_all(b"q");
                    let _ = stdin.flush();
                }
            }
        }

        let deadline = Instant::now() + Duration::from_secs(30);
        for (_, child) in &self.running {
            loop {
                let done = child
                    .lock()
                    .ok()
                    .and_then(|mut c| c.try_wait().ok())
                    .flatten()
                    .is_some();
                if done {
                    break;
                }
                if self.settings.mode == RecordMode::ScreenAndAudio {
                    wake_screen(0);
                }
                if Instant::now() > deadline {
                    if let Ok(mut c) = child.lock() {
                        let _ = c.kill();
                        let _ = c.wait();
                    }
                    break;
                }
                thread::sleep(Duration::from_millis(150));
            }
        }

        #[cfg(windows)]
        for src in &mut self.sources {
            src.stop();
        }
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
        for reader in self.log_readers.drain(..) {
            let _ = reader.join();
        }

        if let Some(log_st) = self.log_state.take() {
            if let Ok(mut st) = log_st.lock() {
                let how = self.gate.how();
                let _ = writeln!(
                    st.log,
                    "audio start: {}",
                    if how.is_empty() { "never" } else { &how }
                );
                #[cfg(windows)]
                for src in &self.sources {
                    let _ = writeln!(st.log, "{}", src.summary());
                }
                let _ = st.log.flush();
            }
        }

        (self.on_event)(SessionEvent::Finalizing {
            progress: None,
            eta: None,
        });

        // Mux screen + audio concurrently with final pass
        let recorder = self.recorder.clone();
        let folder_mux = self.folder.clone();
        let mode = self.settings.mode;
        let on_event_mux = Arc::clone(&self.on_event);
        let finishing = thread::spawn(move || {
            if let Err(err) = recorder.finish(&folder_mux, mode) {
                (on_event_mux)(SessionEvent::Notice { message: err });
            }
        });

        let sample_count = *self.samples.lock().unwrap();
        let mut segments = Vec::new();
        if sample_count > 0 {
            let seconds = sample_count as f64 / STT_SAMPLE_RATE as f64;
            let started = Instant::now();
            let has_refiner = self.refiner.is_some();

            if has_refiner {
                // Two-stage sequential pass: Parakeet first -> drop Parakeet -> Canary refine!
                let total = seconds * self.tape_paths.len() as f64 * 2.0;
                let mut per_channel_chunks = Vec::with_capacity(self.tape_paths.len());
                let mut parakeet_fallback = Vec::new();

                if let Some(model) = self.model.as_ref() {
                    for (ch, path) in self.tape_paths.iter().enumerate() {
                        let before = ch as f64 * seconds;
                        let on_ev = Arc::clone(&self.on_event);
                        if let Ok((raw, ch_segs)) = run_parakeet_chunks_file(
                            model.as_ref(),
                            path,
                            FinalPassOptions::default(),
                            |_, done| {
                                let overall_done = before + done;
                                let eta = if overall_done > 0.0 {
                                    let elapsed = started.elapsed().as_secs_f64();
                                    Some(((total - overall_done).max(0.0) * elapsed / overall_done).round() as u64)
                                } else {
                                    None
                                };
                                let pct = ((overall_done / total.max(0.001)).min(1.0) * 10_000.0).round() / 10_000.0;
                                (on_ev)(SessionEvent::Finalizing {
                                    progress: Some(pct),
                                    eta,
                                });
                            },
                            || false,
                        ) {
                            per_channel_chunks.push((ch, path.clone(), raw));
                            for seg in ch_segs {
                                parakeet_fallback.push(TranscriptionSegment::new(
                                    seg.start,
                                    seg.end,
                                    seg.text,
                                    Some(SOURCES[ch].to_string()),
                                ));
                            }
                        }
                    }
                }

                // Drop Parakeet from RAM BEFORE loading Canary!
                self.model = None;
                if let Some(release) = self.on_release_model.take() {
                    release();
                }

                let stage1_done = seconds * self.tape_paths.len() as f64;
                let mut used_canary = false;
                for (ch, path, raw) in per_channel_chunks {
                    let mut refiner_box = self.refiner.as_ref().and_then(|f| f());
                    let Some(refiner) = refiner_box.as_mut() else {
                        continue;
                    };
                    used_canary = true;
                    let refiner_fn: RefinerFn<'_> = &mut **refiner;
                    let before = stage1_done + ch as f64 * seconds;
                    let on_ev = Arc::clone(&self.on_event);
                    if let Ok(ch_segs) = refine_chunks_file(
                        &path,
                        STT_SAMPLE_RATE,
                        raw,
                        refiner_fn,
                        |_, done| {
                            let overall_done = before + done;
                            let eta = if overall_done > 0.0 {
                                let elapsed = started.elapsed().as_secs_f64();
                                Some(((total - overall_done).max(0.0) * elapsed / overall_done).round() as u64)
                            } else {
                                None
                            };
                            let pct = ((overall_done / total.max(0.001)).min(1.0) * 10_000.0).round() / 10_000.0;
                            (on_ev)(SessionEvent::Finalizing {
                                progress: Some(pct),
                                eta,
                            });
                        },
                        || false,
                    ) {
                        for seg in ch_segs {
                            segments.push(TranscriptionSegment::new(
                                seg.start,
                                seg.end,
                                seg.text,
                                Some(SOURCES[ch].to_string()),
                            ));
                        }
                    }
                }
                if !used_canary {
                    segments = parakeet_fallback;
                }
            } else if let Some(model) = self.model.as_ref() {
                let total = seconds * self.tape_paths.len() as f64;
                for (ch, path) in self.tape_paths.iter().enumerate() {
                    let before = ch as f64 * seconds;
                    let on_ev = Arc::clone(&self.on_event);
                    if let Ok(ch_segs) = run_final_pass_file(
                        model.as_ref(),
                        path,
                        FinalPassOptions::default(),
                        |_, done| {
                            let overall_done = before + done;
                            let eta = if overall_done > 0.0 {
                                let elapsed = started.elapsed().as_secs_f64();
                                Some(((total - overall_done).max(0.0) * elapsed / overall_done).round() as u64)
                            } else {
                                None
                            };
                            let pct = ((overall_done / total.max(0.001)).min(1.0) * 10_000.0).round() / 10_000.0;
                            (on_ev)(SessionEvent::Finalizing {
                                progress: Some(pct),
                                eta,
                            });
                        },
                        || false,
                        None,
                    ) {
                        for seg in ch_segs {
                            segments.push(TranscriptionSegment::new(
                                seg.start,
                                seg.end,
                                seg.text,
                                Some(SOURCES[ch].to_string()),
                            ));
                        }
                    }
                }
            }
            segments = drop_echoes(&segments, 0.6, 1.0);
        }
        self.model = None;
        if let Some(release) = self.on_release_model.take() {
            release();
        }

        for path in &self.tape_paths {
            let _ = fs::remove_file(path);
        }
        let _ = finishing.join();

        let media_path = self.folder.join(WindowsRecorder::output_name(mode));
        let size_bytes = fs::metadata(&media_path).map(|m| m.len()).unwrap_or(0);
        let started_dt = self
            .started_at
            .lock()
            .unwrap()
            .unwrap_or_else(Local::now);
        let duration_seconds = ((sample_count as f64 / STT_SAMPLE_RATE as f64) * 10.0).round() / 10.0;

        let result = SessionResult {
            folder: self.folder.clone(),
            media_path,
            mode,
            started_at: started_dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
            duration_seconds,
            size_bytes,
            segments,
        };
        (self.on_event)(SessionEvent::Finished(result.clone()));
        result
    }
}
