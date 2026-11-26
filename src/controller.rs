//! Application controller (Rust counterpart of `cronista/app_api.py`).
//! Coordinates the SQLite library, settings, LazyModelManager (Kiri-style idle unload),
//! Screen+Audio / Audio-Only recording sessions, file import queue, Canary downloader,
//! Word/PDF/SRT/TXT exports, and background subtitle burning.

use crate::downloads::{download, missing, DownloadError, RemoteFile};
use crate::engine::canary_pass::CanaryPass;
use crate::engine::ffmpeg_locator::find_ffmpeg;
use crate::engine::recorders::{pick_microphone, RecorderSettings, WindowsRecorder};
use crate::engine::session::{RecordingSession, RefinerFactory, SessionEvent};
use crate::engine::stt::{
    canary_files, extract_ort_dlls, find_onnxruntime_lib, parakeet_files, CanaryModel,
    LazyModelManager, ORT_WIN_WHEEL_SIZE, ORT_WIN_WHEEL_URL,
};
use crate::engine::transcript::TranscriptionSegment;
use crate::engine::voices::{line, speakers, SOURCES};
use crate::engine::wasapi::AudioDevice;
use crate::export::{write_pdf, write_text_files, write_word};
use crate::importer::{transcribe_file_sequential, ImportError};
use crate::language::{default_labels, default_title, language_or_default};
use crate::library::{Library, NewRecording, RecordingDetail, RecordingSummary};
use crate::paths::{canary_model_dir, data_dir, parakeet_model_dir, recordings_dir, runtime_dir};
use crate::settings::{
    clean_labels, load_settings, save_settings_patch, AppSettings, RecordMode, THEMES,
};
use crate::subtitles::burn_subtitles;
use chrono::{Local, NaiveDateTime};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const READING_SHARE: f64 = 0.1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Listening,
    Recording,
    Finishing,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModelStatus {
    /// Model is on disk and ready; sleeping out of RAM (~20 MB total app RAM).
    Sleeping,
    Loading,
    Ready,
    Missing { dir: String },
    Downloading { done: u64, total: u64 },
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CanaryState {
    Missing,
    Downloading {
        done: u64,
        total: u64,
        eta: Option<u64>,
    },
    Ready,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct ImportProgress {
    pub name: String,
    pub stage: String,
    pub progress: f64,
    pub done: f64,
    pub total: f64,
    pub eta: Option<u64>,
    pub waiting: usize,
    pub new_lines: Vec<TranscriptionSegment>,
}

#[derive(Debug, Clone)]
pub enum UiEvent {
    Model(ModelStatus),
    Phase(Phase),
    RecordingStarted { started_at: f64 },
    Partial {
        source: String,
        committed: String,
        pending: String,
    },
    Phrase {
        start: f64,
        text: String,
        source: String,
    },
    Finalizing {
        progress: Option<f64>,
        eta: Option<u64>,
    },
    Saved { id: String },
    Canary {
        state: CanaryState,
        transcription: String,
    },
    Subtitling {
        id: String,
        progress: f64,
        eta: Option<u64>,
    },
    Subtitled {
        id: String,
        path: String,
        from_file: bool,
    },
    Import(ImportProgress),
    Imported { id: String, name: String },
    ImportCancelled { name: String },
    ImportError { name: String, message: String },
    Notice(String),
    Error {
        message: String,
        recording_id: Option<String>,
    },
    CloseApproved,
}

pub struct AppController {
    data_path: PathBuf,
    recordings_path: PathBuf,
    settings_path: PathBuf,
    library: Library,
    recorder: Option<WindowsRecorder>,
    pub recorder_error: String,
    model_mgr: Arc<LazyModelManager>,
    tx: Sender<UiEvent>,
    repaint: Arc<dyn Fn() + Send + Sync>,

    state: Arc<Mutex<ControllerState>>,
    canary_model: Arc<Mutex<Option<Arc<CanaryModel>>>>,
    import_stop: Arc<AtomicBool>,
    download_stop: Arc<AtomicBool>,
    parakeet_download_stop: Arc<AtomicBool>,
    audio_level: Arc<AtomicU32>,
}

struct ControllerState {
    phase: Phase,
    session: Option<RecordingSession>,
    language: String,
    labels: HashMap<String, String>,
    subtitles: bool,
    subtitling: HashSet<String>,
    close_when_saved: bool,
    import_queue: VecDeque<PathBuf>,
    importing: bool,
    downloading_canary: bool,
    downloading_parakeet: bool,
}

impl AppController {
    pub fn new(tx: Sender<UiEvent>, repaint: Arc<dyn Fn() + Send + Sync>) -> Result<Arc<Self>, String> {
        let data = data_dir();
        let recs = recordings_dir();
        let settings_path = data.join("settings.json");
        let settings = load_settings(&settings_path);
        let library = Library::open(data.join("library.db"))?;
        let (recorder, recorder_error) = match WindowsRecorder::default_recorder() {
            Ok(r) => (Some(r), String::new()),
            Err(e) => (None, e),
        };
        let model_dir = parakeet_model_dir();
        let model_mgr = Arc::new(LazyModelManager::new(
            model_dir,
            4,
            settings.model_idle_unload_secs,
        ));

        let ctrl = Arc::new(Self {
            data_path: data,
            recordings_path: recs,
            settings_path,
            library,
            recorder,
            recorder_error,
            model_mgr,
            tx,
            repaint,
            state: Arc::new(Mutex::new(ControllerState {
                phase: Phase::Idle,
                session: None,
                language: "en".to_string(),
                labels: HashMap::new(),
                subtitles: false,
                subtitling: HashSet::new(),
                close_when_saved: false,
                import_queue: VecDeque::new(),
                importing: false,
                downloading_canary: false,
                downloading_parakeet: false,
            })),
            canary_model: Arc::new(Mutex::new(None)),
            import_stop: Arc::new(AtomicBool::new(false)),
            download_stop: Arc::new(AtomicBool::new(false)),
            parakeet_download_stop: Arc::new(AtomicBool::new(false)),
            audio_level: Arc::new(AtomicU32::new(0.0_f32.to_bits())),
        });

        // Kiri-style background idle watcher: unloads Parakeet from RAM after idle timeout!
        let weak = Arc::downgrade(&ctrl);
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(5));
            let Some(c) = weak.upgrade() else {
                break;
            };
            if c.model_mgr.maybe_unload_idle() {
                c.emit(UiEvent::Model(ModelStatus::Sleeping));
            }
        });

        Ok(ctrl)
    }

    fn emit(&self, ev: UiEvent) {
        let _ = self.tx.send(ev);
        (self.repaint)();
    }

    pub fn initial_model_status(&self) -> ModelStatus {
        if self.model_mgr.is_loaded_in_ram() {
            ModelStatus::Ready
        } else if self.model_mgr.is_model_present_on_disk() {
            ModelStatus::Sleeping
        } else {
            ModelStatus::Missing {
                dir: parakeet_model_dir().to_string_lossy().into_owned(),
            }
        }
    }

    pub fn is_model_loaded(&self) -> bool {
        self.model_mgr.is_loaded_in_ram()
    }

    pub fn live_audio_level(&self) -> f32 {
        f32::from_bits(self.audio_level.load(Ordering::Relaxed))
    }

    pub fn sleep_model_now(&self) -> bool {
        if self.model_mgr.unload_now() {
            self.emit(UiEvent::Model(ModelStatus::Sleeping));
            true
        } else {
            false
        }
    }

    pub fn download_parakeet_and_runtime(self: &Arc<Self>) {
        {
            let mut st = self.state.lock().unwrap();
            if st.downloading_parakeet {
                return;
            }
            st.downloading_parakeet = true;
        }
        self.parakeet_download_stop.store(false, Ordering::Relaxed);

        let need_ort = find_onnxruntime_lib().is_none();
        let p_files = parakeet_files();
        let p_total: u64 = p_files.iter().map(|f| f.size).sum();
        let grand_total = p_total + if need_ort { ORT_WIN_WHEEL_SIZE } else { 0 };
        self.emit(UiEvent::Model(ModelStatus::Downloading {
            done: 0,
            total: grand_total,
        }));

        let ctrl = Arc::clone(self);
        thread::spawn(move || {
            let stop_flag = Arc::clone(&ctrl.parakeet_download_stop);
            let mut last_emit = Instant::now() - Duration::from_secs(1);
            let mut offset = 0u64;

            if need_ort {
                let rt_dir = runtime_dir();
                let wheel = vec![RemoteFile::new(
                    "onnxruntime-win-x64.whl",
                    ORT_WIN_WHEEL_URL,
                    ORT_WIN_WHEEL_SIZE,
                    "",
                )];
                let res = download(
                    &rt_dir,
                    &wheel,
                    |done, _| {
                        let now = Instant::now();
                        if now.duration_since(last_emit) >= Duration::from_millis(200) {
                            last_emit = now;
                            ctrl.emit(UiEvent::Model(ModelStatus::Downloading {
                                done,
                                total: grand_total,
                            }));
                        }
                    },
                    || stop_flag.load(Ordering::Relaxed),
                    60,
                );
                match res {
                    Ok(()) => {
                        let whl_path = rt_dir.join("onnxruntime-win-x64.whl");
                        let extract_res = extract_ort_dlls(&whl_path, &rt_dir);
                        let _ = fs::remove_file(&whl_path);
                        if let Err(err) = extract_res {
                            ctrl.state.lock().unwrap().downloading_parakeet = false;
                            ctrl.emit(UiEvent::Model(ModelStatus::Missing {
                                dir: parakeet_model_dir().to_string_lossy().into_owned(),
                            }));
                            ctrl.emit(UiEvent::Error {
                                message: err,
                                recording_id: None,
                            });
                            return;
                        }
                        offset = ORT_WIN_WHEEL_SIZE;
                    }
                    Err(DownloadError::Cancelled) => {
                        ctrl.state.lock().unwrap().downloading_parakeet = false;
                        ctrl.emit(UiEvent::Model(ModelStatus::Missing {
                            dir: parakeet_model_dir().to_string_lossy().into_owned(),
                        }));
                        return;
                    }
                    Err(DownloadError::Failed(msg)) => {
                        ctrl.state.lock().unwrap().downloading_parakeet = false;
                        ctrl.emit(UiEvent::Model(ModelStatus::Missing {
                            dir: parakeet_model_dir().to_string_lossy().into_owned(),
                        }));
                        ctrl.emit(UiEvent::Error {
                            message: msg,
                            recording_id: None,
                        });
                        return;
                    }
                }
            }

            let target_dir = parakeet_model_dir();
            let res = download(
                &target_dir,
                &p_files,
                |done, _| {
                    let now = Instant::now();
                    if now.duration_since(last_emit) >= Duration::from_millis(200) || done == p_total {
                        last_emit = now;
                        ctrl.emit(UiEvent::Model(ModelStatus::Downloading {
                            done: offset + done,
                            total: grand_total,
                        }));
                    }
                },
                || stop_flag.load(Ordering::Relaxed),
                60,
            );
            ctrl.state.lock().unwrap().downloading_parakeet = false;
            match res {
                Ok(()) => {
                    ctrl.emit(UiEvent::Model(ModelStatus::Sleeping));
                    ctrl.emit(UiEvent::Notice("Speech engine ready".into()));
                }
                Err(DownloadError::Cancelled) => {
                    ctrl.emit(UiEvent::Model(ModelStatus::Missing {
                        dir: target_dir.to_string_lossy().into_owned(),
                    }));
                }
                Err(DownloadError::Failed(msg)) => {
                    ctrl.emit(UiEvent::Model(ModelStatus::Missing {
                        dir: target_dir.to_string_lossy().into_owned(),
                    }));
                    ctrl.emit(UiEvent::Error {
                        message: msg,
                        recording_id: None,
                    });
                }
            }
        });
    }

    pub fn cancel_parakeet_download(&self) {
        self.parakeet_download_stop.store(true, Ordering::Relaxed);
    }

    pub fn load_settings(&self) -> AppSettings {
        load_settings(&self.settings_path)
    }

    pub fn save_preferences(
        &self,
        language: Option<&str>,
        tour_seen: Option<bool>,
        labels: Option<&HashMap<String, String>>,
        subtitles: Option<bool>,
        theme: Option<&str>,
        record_mode: Option<RecordMode>,
        live_transcription: Option<bool>,
    ) {
        let mut patch = Map::new();
        if let Some(l) = language {
            if matches!(l, "en" | "es") {
                patch.insert("language".into(), Value::String(l.to_string()));
            }
        }
        if let Some(t) = tour_seen {
            patch.insert("tour_seen".into(), Value::Bool(t));
        }
        if let Some(lbls) = labels {
            let clean = clean_labels(lbls);
            let obj: Map<String, Value> = clean
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect();
            patch.insert("labels".into(), Value::Object(obj));
        }
        if let Some(sub) = subtitles {
            patch.insert("subtitles".into(), Value::Bool(sub));
        }
        if let Some(th) = theme {
            if THEMES.contains(&th) {
                patch.insert("theme".into(), Value::String(th.to_string()));
            }
        }
        if let Some(mode) = record_mode {
            patch.insert("record_mode".into(), Value::String(mode.as_str().to_string()));
        }
        if let Some(live) = live_transcription {
            patch.insert("live_transcription".into(), Value::Bool(live));
        }
        if !patch.is_empty() {
            let _ = save_settings_patch(&self.settings_path, patch);
        }
    }

    pub fn microphones(&self) -> (Vec<AudioDevice>, Option<String>) {
        self.recorder
            .as_ref()
            .and_then(|r| r.microphones().ok())
            .unwrap_or_default()
    }

    pub fn default_microphone(&self, devices: &[AudioDevice], win_default: Option<&str>) -> Option<String> {
        let settings = self.load_settings();
        pick_microphone(devices, settings.microphone.as_deref(), win_default).map(|d| d.id.clone())
    }

    pub fn canary_status(&self) -> CanaryState {
        let st = self.state.lock().unwrap();
        if st.downloading_canary {
            return CanaryState::Downloading {
                done: 0,
                total: canary_files().iter().map(|f| f.size).sum(),
                eta: None,
            };
        }
        drop(st);
        if missing(&canary_model_dir(), &canary_files()).is_empty() {
            CanaryState::Ready
        } else {
            CanaryState::Missing
        }
    }

    pub fn active_transcription_quality(&self) -> String {
        let chosen = self.load_settings().transcription;
        if chosen == "high" && matches!(self.canary_status(), CanaryState::Ready) {
            "high".to_string()
        } else {
            "standard".to_string()
        }
    }

    pub fn set_transcription(self: &Arc<Self>, quality: &str) -> Result<String, String> {
        if quality == "standard" {
            self.download_stop.store(true, Ordering::Relaxed);
            let mut p = Map::new();
            p.insert("transcription".into(), Value::String("standard".into()));
            let _ = save_settings_patch(&self.settings_path, p);
            return Ok("standard".into());
        }
        if quality != "high" {
            return Err("Unknown transcription quality.".into());
        }
        if matches!(self.canary_status(), CanaryState::Ready) {
            let mut p = Map::new();
            p.insert("transcription".into(), Value::String("high".into()));
            let _ = save_settings_patch(&self.settings_path, p);
            return Ok("high".into());
        }

        let mut st = self.state.lock().unwrap();
        if st.downloading_canary {
            return Ok("downloading".into());
        }
        st.downloading_canary = true;
        self.download_stop.store(false, Ordering::Relaxed);
        drop(st);

        let total: u64 = canary_files().iter().map(|f| f.size).sum();
        self.emit(UiEvent::Canary {
            state: CanaryState::Downloading {
                done: 0,
                total,
                eta: None,
            },
            transcription: self.active_transcription_quality(),
        });

        let ctrl = Arc::clone(self);
        thread::spawn(move || {
            let started = Instant::now();
            let mut last_emit = Instant::now() - Duration::from_secs(1);
            let mut first_done: Option<u64> = None;
            let stop_flag = Arc::clone(&ctrl.download_stop);
            let files = canary_files();

            let res = download(
                &canary_model_dir(),
                &files,
                |done, tot| {
                    let first = *first_done.get_or_insert(done);
                    let now = Instant::now();
                    let eta = if done > first {
                        let elapsed = now.duration_since(started).as_secs_f64();
                        Some(((tot.saturating_sub(done)) as f64 * elapsed / (done - first) as f64).round() as u64)
                    } else {
                        None
                    };
                    if now.duration_since(last_emit) >= Duration::from_millis(250) || done == tot {
                        last_emit = now;
                        ctrl.emit(UiEvent::Canary {
                            state: CanaryState::Downloading {
                                done,
                                total: tot,
                                eta,
                            },
                            transcription: ctrl.active_transcription_quality(),
                        });
                    }
                },
                || stop_flag.load(Ordering::Relaxed),
                30,
            );

            let final_state = match res {
                Ok(()) => {
                    if !ctrl.download_stop.load(Ordering::Relaxed) {
                        let mut p = Map::new();
                        p.insert("transcription".into(), Value::String("high".into()));
                        let _ = save_settings_patch(&ctrl.settings_path, p);
                    }
                    CanaryState::Ready
                }
                Err(DownloadError::Cancelled) => CanaryState::Missing,
                Err(DownloadError::Failed(msg)) => CanaryState::Error(msg),
            };
            ctrl.state.lock().unwrap().downloading_canary = false;
            let tr = ctrl.active_transcription_quality();
            ctrl.emit(UiEvent::Canary {
                state: final_state,
                transcription: tr,
            });
        });

        Ok("downloading".into())
    }

    fn make_refiner_factory(self: &Arc<Self>) -> Option<RefinerFactory> {
        if self.active_transcription_quality() != "high" {
            return None;
        }
        let ctrl = Arc::clone(self);
        let failed = Arc::new(AtomicBool::new(false));
        Some(Box::new(move || {
            if failed.load(Ordering::Relaxed) {
                return None;
            }
            let mut guard = ctrl.canary_model.lock().unwrap();
            if guard.is_none() {
                match CanaryModel::load(&canary_model_dir(), 4) {
                    Ok(m) => *guard = Some(Arc::new(m)),
                    Err(err) => {
                        failed.store(true, Ordering::Relaxed);
                        ctrl.emit(UiEvent::Notice(format!(
                            "High quality could not be used ({err}). This transcript was written in medium-low quality."
                        )));
                        return None;
                    }
                }
            }
            let model = Arc::clone(guard.as_ref().unwrap());
            let mut pass = CanaryPass::new(SharedCanary(model));
            Some(Box::new(move |audio: &[f32], tokens: Vec<String>, times: Vec<f64>, sr: usize| {
                pass.refine(audio, tokens, times, sr)
            }))
        }))
    }

    fn release_canary(&self) {
        *self.canary_model.lock().unwrap() = None;
    }

    // --- Recording -------------------------------------------------------------------------------

    pub fn start_recording(
        self: &Arc<Self>,
        microphone: Option<String>,
        quality: String,
        system_audio: bool,
        mode: RecordMode,
        live_transcription: bool,
        language: Option<&str>,
        labels: HashMap<String, String>,
        subtitles: bool,
    ) -> Result<(), String> {
        {
            let st = self.state.lock().unwrap();
            if st.phase != Phase::Idle {
                return Err("A recording is already running.".into());
            }
            if st.importing || !st.import_queue.is_empty() {
                return Err("A file is being transcribed. Cancel it to record.".into());
            }
        }
        let Some(recorder) = self.recorder.clone() else {
            return Err(self.recorder_error.clone());
        };

        // Save chosen controls
        let mut patch = Map::new();
        if let Some(m) = &microphone {
            patch.insert("microphone".into(), Value::String(m.clone()));
        }
        patch.insert("quality".into(), Value::String(quality.clone()));
        patch.insert("system_audio".into(), Value::Bool(system_audio));
        patch.insert("record_mode".into(), Value::String(mode.as_str().into()));
        patch.insert("live_transcription".into(), Value::Bool(live_transcription));
        let _ = save_settings_patch(&self.settings_path, patch);

        let lang = language_or_default(language).to_string();
        let mut merged_labels = default_labels(Some(&lang));
        for (k, v) in clean_labels(&labels) {
            merged_labels.insert(k, v);
        }

        {
            let mut st = self.state.lock().unwrap();
            st.phase = Phase::Listening;
            st.language = lang;
            st.labels = merged_labels;
            st.subtitles = subtitles && mode == RecordMode::ScreenAndAudio;
        }
        self.emit(UiEvent::Phase(Phase::Listening));

        let ctrl = Arc::clone(self);
        thread::spawn(move || {
            // Lazy-load Parakeet if present on disk; otherwise record video/audio cleanly without STT!
            let (model, used_real_model): (Arc<dyn crate::engine::final_pass::SpeechModel>, bool) =
                if ctrl.model_mgr.is_model_present_on_disk() {
                    if !ctrl.model_mgr.is_loaded_in_ram() {
                        ctrl.emit(UiEvent::Model(ModelStatus::Loading));
                    }
                    match ctrl.model_mgr.acquire_parakeet() {
                        Ok(m) => {
                            ctrl.emit(UiEvent::Model(ModelStatus::Ready));
                            (m, true)
                        }
                        Err(err) => {
                            ctrl.emit(UiEvent::Notice(format!(
                                "Recording without transcription ({err})"
                            )));
                            (Arc::new(NoopSpeechModel), false)
                        }
                    }
                } else {
                    ctrl.emit(UiEvent::Notice(
                        "Recording without transcription (speech model not installed).".into(),
                    ));
                    (Arc::new(NoopSpeechModel), false)
                };

            let folder = new_folder(&ctrl.recordings_path);
            let rec_settings = RecorderSettings {
                microphone,
                quality,
                fps: 30,
                max_seconds: 4 * 3600,
                system_audio,
                mode,
                live_transcription: live_transcription && used_real_model,
            };

            let ctrl_ev = Arc::clone(&ctrl);
            let on_event = Arc::new(move |ev: SessionEvent| match ev {
                SessionEvent::Listening => ctrl_ev.emit(UiEvent::Phase(Phase::Listening)),
                SessionEvent::Recording { started_at } => {
                    ctrl_ev.state.lock().unwrap().phase = Phase::Recording;
                    ctrl_ev.emit(UiEvent::Phase(Phase::Recording));
                    ctrl_ev.emit(UiEvent::RecordingStarted { started_at });
                }
                SessionEvent::Partial {
                    committed,
                    pending,
                    source,
                } => ctrl_ev.emit(UiEvent::Partial {
                    source,
                    committed,
                    pending,
                }),
                SessionEvent::Phrase {
                    start,
                    text,
                    source,
                } => ctrl_ev.emit(UiEvent::Phrase {
                    start,
                    text,
                    source,
                }),
                SessionEvent::Ended { reason, .. } => {
                    let msg = if reason.is_empty() {
                        "The recorder stopped. Saving what was recorded.".to_string()
                    } else {
                        format!("The recorder stopped. Saving what was recorded. ({reason})")
                    };
                    ctrl_ev.emit(UiEvent::Notice(msg));
                    ctrl_ev.stop_recording();
                }
                SessionEvent::Notice { message } => ctrl_ev.emit(UiEvent::Notice(message)),
                SessionEvent::Finalizing { progress, eta } => {
                    ctrl_ev.emit(UiEvent::Finalizing { progress, eta });
                }
                SessionEvent::Finished(_) => {}
            });

            let refiner = ctrl.make_refiner_factory();
            let mut session = RecordingSession::new(
                model,
                recorder,
                rec_settings,
                folder,
                on_event,
                refiner,
            );
            session.set_audio_level_sink(Arc::clone(&ctrl.audio_level));
            if used_real_model {
                let ctrl_rel = Arc::clone(&ctrl);
                session.set_on_release_model(Box::new(move || {
                    ctrl_rel.model_mgr.release_parakeet();
                    if ctrl_rel.model_mgr.unload_now() {
                        ctrl_rel.emit(UiEvent::Model(ModelStatus::Sleeping));
                    }
                }));
            }
            if let Err(err) = session.start() {
                if used_real_model {
                    ctrl.model_mgr.release_parakeet();
                }
                ctrl.state.lock().unwrap().phase = Phase::Idle;
                ctrl.emit(UiEvent::Error {
                    message: err,
                    recording_id: None,
                });
                ctrl.emit(UiEvent::Phase(Phase::Idle));
                return;
            }
            ctrl.state.lock().unwrap().session = Some(session);
        });

        Ok(())
    }

    pub fn stop_recording(self: &Arc<Self>) {
        let session = {
            let mut st = self.state.lock().unwrap();
            if st.session.is_none() || matches!(st.phase, Phase::Idle | Phase::Finishing) {
                return;
            }
            st.phase = Phase::Finishing;
            st.session.take()
        };
        self.emit(UiEvent::Phase(Phase::Finishing));

        let Some(mut session) = session else {
            return;
        };
        let ctrl = Arc::clone(self);
        thread::spawn(move || {
            let (lang, labels, auto_sub) = {
                let st = ctrl.state.lock().unwrap();
                (st.language.clone(), st.labels.clone(), st.subtitles)
            };

            let result = session.stop();
            let names = speakers(&result.segments, Some(&labels));
            let _ = write_text_files(&result.segments, &result.folder, names.as_ref());

            let started = NaiveDateTime::parse_from_str(&result.started_at, "%Y-%m-%dT%H:%M:%S")
                .unwrap_or_else(|_| Local::now().naive_local());
            let kind = match result.mode {
                RecordMode::ScreenAndAudio => "recording",
                RecordMode::AudioOnly => "audio_recording",
            };
            let new_rec = NewRecording {
                title: default_title(started, Some(&lang)),
                created_at: result.started_at.clone(),
                duration_seconds: result.duration_seconds,
                size_bytes: result.size_bytes,
                folder: result.folder.to_string_lossy().into_owned(),
                video_path: result.media_path.to_string_lossy().into_owned(),
                transcript: result.transcript(),
                segments: result.segments.clone(),
                labels,
                kind: kind.to_string(),
            };

            match ctrl.library.add(&new_rec) {
                Ok(id) => {
                    ctrl.emit(UiEvent::Saved { id: id.clone() });
                    if auto_sub && result.mode == RecordMode::ScreenAndAudio && result.media_path.exists() {
                        let _ = ctrl.start_subtitling(&id, Some(&lang), false);
                    }
                }
                Err(err) => {
                    ctrl.emit(UiEvent::Error {
                        message: format!("Saving the recording failed: {err}"),
                        recording_id: None,
                    });
                }
            }

            ctrl.release_canary();
            if ctrl.model_mgr.unload_now() {
                ctrl.emit(UiEvent::Model(ModelStatus::Sleeping));
            }
            let should_close = {
                let mut st = ctrl.state.lock().unwrap();
                st.phase = Phase::Idle;
                st.close_when_saved
            };
            ctrl.emit(UiEvent::Phase(Phase::Idle));
            if should_close {
                ctrl.emit(UiEvent::CloseApproved);
            }
        });
    }

    pub fn request_close(self: &Arc<Self>) -> bool {
        let (is_idle, is_importing) = {
            let st = self.state.lock().unwrap();
            (st.phase == Phase::Idle, st.importing)
        };
        if is_idle && !is_importing {
            return true;
        }
        self.state.lock().unwrap().close_when_saved = true;
        if !is_idle {
            self.emit(UiEvent::Notice("Saving the recording before closing…".into()));
            self.stop_recording();
        } else {
            self.cancel_import();
        }
        false
    }

    // --- File importing --------------------------------------------------------------------------

    pub fn import_files(self: &Arc<Self>, paths: Vec<PathBuf>) -> Result<usize, String> {
        let files: Vec<PathBuf> = paths.into_iter().filter(|p| p.is_file()).collect();
        let ffmpeg = self
            .ffmpeg()
            .ok_or_else(|| "ffmpeg was not found. Install it with: winget install ffmpeg".to_string())?;
        if !self.model_mgr.is_model_present_on_disk() {
            return Err(format!(
                "Parakeet speech model not found in {}.",
                parakeet_model_dir().display()
            ));
        }
        if files.is_empty() {
            return Err("Drop an audio or video file, not a folder.".into());
        }

        let start_worker = {
            let mut st = self.state.lock().unwrap();
            if st.phase != Phase::Idle {
                return Err("Stop the recording first, then drop the file again.".into());
            }
            for f in &files {
                st.import_queue.push_back(f.clone());
            }
            if !st.importing {
                st.importing = true;
                true
            } else {
                false
            }
        };

        if start_worker {
            let ctrl = Arc::clone(self);
            thread::spawn(move || ctrl.run_import_worker(ffmpeg));
        }
        Ok(files.len())
    }

    pub fn cancel_import(&self) {
        let mut st = self.state.lock().unwrap();
        st.import_queue.clear();
        if st.importing {
            self.import_stop.store(true, Ordering::Relaxed);
        }
    }

    fn run_import_worker(self: Arc<Self>, ffmpeg: String) {
        loop {
            let (path, waiting) = {
                let mut st = self.state.lock().unwrap();
                let Some(next) = st.import_queue.pop_front() else {
                    st.importing = false;
                    break;
                };
                self.import_stop.store(false, Ordering::Relaxed);
                (next, st.import_queue.len())
            };

            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string();
            let folder = new_folder(&self.recordings_path);
            let _ = fs::create_dir_all(&folder);

            self.emit(UiEvent::Import(ImportProgress {
                name: name.clone(),
                stage: "reading".into(),
                progress: 0.0,
                done: 0.0,
                total: 0.0,
                eta: None,
                waiting,
                new_lines: Vec::new(),
            }));

            let refiner_factory = self.make_refiner_factory();
            let has_high = refiner_factory.is_some();

            let mut pace: Option<(Instant, f64)> = None;
            let stop_flag = Arc::clone(&self.import_stop);
            let ctrl_acq = Arc::clone(&self);
            let ctrl_rel = Arc::clone(&self);
            let res = transcribe_file_sequential(
                move || {
                    if !ctrl_acq.model_mgr.is_loaded_in_ram() {
                        ctrl_acq.emit(UiEvent::Model(ModelStatus::Loading));
                    }
                    let m = ctrl_acq.model_mgr.acquire_parakeet()?;
                    ctrl_acq.emit(UiEvent::Model(ModelStatus::Ready));
                    Ok(m as Arc<dyn crate::engine::final_pass::SpeechModel>)
                },
                move || {
                    ctrl_rel.model_mgr.release_parakeet();
                    // If Canary is about to run next (or queue is empty), immediately drop Parakeet from RAM!
                    let queue_empty = ctrl_rel.state.lock().unwrap().import_queue.is_empty();
                    if (has_high || queue_empty) && ctrl_rel.model_mgr.unload_now() {
                        ctrl_rel.emit(UiEvent::Model(ModelStatus::Sleeping));
                    }
                },
                refiner_factory,
                &ffmpeg,
                &path,
                &folder,
                |stage, done, total| {
                    let share = if total > 0.0 {
                        (done / total).min(1.0)
                    } else {
                        0.0
                    };
                    let mut eta = None;
                    let overall = if stage == "reading" {
                        READING_SHARE * share
                    } else {
                        let now = Instant::now();
                        match pace {
                            None => pace = Some((now, done)),
                            Some((t0, d0)) if done > d0 => {
                                let elapsed = now.duration_since(t0).as_secs_f64();
                                eta = Some(((total - done).max(0.0) * elapsed / (done - d0)).round() as u64);
                            }
                            _ => {}
                        }
                        READING_SHARE + (1.0 - READING_SHARE) * share
                    };
                    let w = self.state.lock().unwrap().import_queue.len();
                    self.emit(UiEvent::Import(ImportProgress {
                        name: name.clone(),
                        stage: stage.to_string(),
                        progress: (overall * 10_000.0).round() / 10_000.0,
                        done: (done * 10.0).round() / 10.0,
                        total: (total * 10.0).round() / 10.0,
                        eta,
                        waiting: w,
                        new_lines: Vec::new(),
                    }));
                },
                |new_segs| {
                    let w = self.state.lock().unwrap().import_queue.len();
                    self.emit(UiEvent::Import(ImportProgress {
                        name: name.clone(),
                        stage: "transcribing".into(),
                        progress: 0.0, // UI retains latest progress when new_lines arrive
                        done: 0.0,
                        total: 0.0,
                        eta: None,
                        waiting: w,
                        new_lines: new_segs.to_vec(),
                    }));
                },
                || stop_flag.load(Ordering::Relaxed),
            );
            // Drop Canary immediately after each file so RAM returns to idle right away!
            self.release_canary();

            match res {
                Ok(imported) => {
                    let _ = write_text_files(&imported.segments, &folder, None);
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .unwrap_or(&name);
                    let title: String = stem.chars().take(120).collect();
                    let size_bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    let transcript = imported
                        .segments
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");
                    let new_rec = NewRecording {
                        title,
                        created_at: Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
                        duration_seconds: (imported.duration * 10.0).round() / 10.0,
                        size_bytes,
                        folder: folder.to_string_lossy().into_owned(),
                        video_path: path.to_string_lossy().into_owned(),
                        transcript,
                        segments: imported.segments,
                        labels: HashMap::new(),
                        kind: if imported.has_video { "video" } else { "audio" }.to_string(),
                    };
                    match self.library.add(&new_rec) {
                        Ok(id) => self.emit(UiEvent::Imported { id, name }),
                        Err(err) => {
                            let _ = fs::remove_dir_all(&folder);
                            self.emit(UiEvent::ImportError { name, message: err });
                        }
                    }
                }
                Err(ImportError::Cancelled) => {
                    let _ = fs::remove_dir_all(&folder);
                    self.emit(UiEvent::ImportCancelled { name });
                }
                Err(ImportError::Failed(msg)) => {
                    let _ = fs::remove_dir_all(&folder);
                    self.emit(UiEvent::ImportError { name, message: msg });
                }
            }
        }

        self.release_canary();
        if self.model_mgr.unload_now() {
            self.emit(UiEvent::Model(ModelStatus::Sleeping));
        }
        let should_close = {
            let st = self.state.lock().unwrap();
            st.close_when_saved && st.phase == Phase::Idle
        };
        if should_close {
            self.emit(UiEvent::CloseApproved);
        }
    }

    // --- Library & Exports -----------------------------------------------------------------------

    pub fn recordings(&self, search: &str) -> Vec<RecordingSummary> {
        let q = search.trim();
        self.library
            .list(if q.is_empty() { None } else { Some(q) }, 200)
            .unwrap_or_default()
    }

    pub fn recording(&self, id: &str) -> Option<RecordingDetail> {
        self.library.get(id).ok().flatten()
    }

    pub fn rename(&self, id: &str, title: &str) -> Result<(), String> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err("Enter a title.".into());
        }
        self.library.rename(id, trimmed)
    }

    pub fn rename_label(&self, id: &str, source: &str, label: &str) -> Result<HashMap<String, String>, String> {
        if !SOURCES.contains(&source) {
            return Err("Recording not found.".into());
        }
        let rec = self
            .library
            .get(id)?
            .ok_or_else(|| "Recording not found.".to_string())?;
        let cleaned: String = label
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(40)
            .collect();
        if cleaned.is_empty() {
            return Err("Enter a label.".into());
        }
        self.library.set_label(id, source, &cleaned)?;
        let updated = self.library.get(id)?.unwrap_or(rec);
        let folder = Path::new(&updated.folder);
        if folder.is_dir() {
            let names = speakers(&updated.segments, Some(&updated.labels));
            let _ = write_text_files(&updated.segments, folder, names.as_ref());
        }
        Ok(updated.labels)
    }

    pub fn copy_text(&self, id: &str) -> Result<String, String> {
        let rec = self
            .library
            .get(id)?
            .ok_or_else(|| "Recording not found.".to_string())?;
        let settings = self.load_settings();
        let mut merged = default_labels(settings.language.as_deref());
        for (k, v) in &rec.labels {
            merged.insert(k.clone(), v.clone());
        }
        let names = speakers(&rec.segments, Some(&merged));
        let text = rec
            .segments
            .iter()
            .map(|s| line(s, names.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        let mut cb = arboard::Clipboard::new().map_err(|e| format!("Clipboard failed: {e}"))?;
        cb.set_text(text.clone())
            .map_err(|e| format!("Clipboard write failed: {e}"))?;
        Ok(text)
    }

    pub fn export_word(&self, id: &str, language: Option<&str>) -> Result<PathBuf, String> {
        self.export_doc(id, "transcript.docx", language, true)
    }

    pub fn export_pdf(&self, id: &str, language: Option<&str>) -> Result<PathBuf, String> {
        self.export_doc(id, "transcript.pdf", language, false)
    }

    fn export_doc(&self, id: &str, filename: &str, language: Option<&str>, is_word: bool) -> Result<PathBuf, String> {
        let rec = self
            .library
            .get(id)?
            .ok_or_else(|| "Recording not found.".to_string())?;
        let lang = language_or_default(language.or(self.load_settings().language.as_deref()));
        let mut merged = default_labels(Some(lang));
        for (k, v) in &rec.labels {
            merged.insert(k.clone(), v.clone());
        }
        let names = speakers(&rec.segments, Some(&merged));
        let dt = NaiveDateTime::parse_from_str(&rec.created_at, "%Y-%m-%dT%H:%M:%S")
            .unwrap_or_else(|_| Local::now().naive_local());
        let out_path = Path::new(&rec.folder).join(filename);
        let written = if is_word {
            write_word(
                &rec.segments,
                &out_path,
                &rec.title,
                dt,
                Some(lang),
                names.as_ref(),
                rec.is_imported(),
            )?
        } else {
            write_pdf(
                &rec.segments,
                &out_path,
                &rec.title,
                dt,
                Some(lang),
                names.as_ref(),
                rec.is_imported(),
            )?
        };
        reveal_in_explorer(&written);
        Ok(written)
    }

    pub fn start_subtitling(
        self: &Arc<Self>,
        id: &str,
        language: Option<&str>,
        reveal: bool,
    ) -> Result<(), String> {
        let rec = self
            .library
            .get(id)?
            .ok_or_else(|| "Recording not found.".to_string())?;
        if !rec.has_video() {
            return Err("This file has no pictures to put the subtitles on.".into());
        }
        let ffmpeg = self
            .ffmpeg()
            .ok_or_else(|| "ffmpeg was not found. Install it with: winget install ffmpeg".to_string())?;
        if !Path::new(&rec.video_path).exists() {
            return Err("The video file is no longer there.".into());
        }

        {
            let mut st = self.state.lock().unwrap();
            if !st.subtitling.insert(id.to_string()) {
                return Ok(());
            }
        }

        let lang = language_or_default(language.or(self.load_settings().language.as_deref())).to_string();
        let rec_id = id.to_string();
        let ctrl = Arc::clone(self);
        thread::spawn(move || {
            ctrl.emit(UiEvent::Subtitling {
                id: rec_id.clone(),
                progress: 0.0,
                eta: None,
            });
            let started = Instant::now();
            let mut merged = default_labels(Some(&lang));
            for (k, v) in &rec.labels {
                merged.insert(k.clone(), v.clone());
            }
            let names = speakers(&rec.segments, Some(&merged));
            let source = if rec.kind == "video" {
                Some(PathBuf::from(&rec.video_path))
            } else {
                None
            };
            let length = rec.duration_seconds;
            let rec_id_prog = rec_id.clone();
            let ctrl_prog = Arc::clone(&ctrl);

            let res = burn_subtitles(
                &ffmpeg,
                Path::new(&rec.folder),
                &rec.segments,
                names.as_ref(),
                70,
                source.as_deref(),
                |done| {
                    if length > 0.0 && done > 0.0 {
                        let elapsed = started.elapsed().as_secs_f64();
                        let eta = ((length - done).max(0.0) * elapsed / done).round() as u64;
                        let pct = ((done / length).min(0.99) * 10_000.0).round() / 10_000.0;
                        ctrl_prog.emit(UiEvent::Subtitling {
                            id: rec_id_prog.clone(),
                            progress: pct,
                            eta: Some(eta),
                        });
                    }
                },
            );

            ctrl.state.lock().unwrap().subtitling.remove(&rec_id);
            match res {
                Ok(path) => {
                    if reveal {
                        reveal_in_explorer(&path);
                    }
                    ctrl.emit(UiEvent::Subtitled {
                        id: rec_id,
                        path: path.to_string_lossy().into_owned(),
                        from_file: rec.kind == "video",
                    });
                }
                Err(msg) => {
                    ctrl.emit(UiEvent::Error {
                        message: msg,
                        recording_id: Some(rec_id),
                    });
                }
            }
        });
        Ok(())
    }

    pub fn play(&self, id: &str) -> Result<(), String> {
        let rec = self
            .library
            .get(id)?
            .ok_or_else(|| "Recording not found.".to_string())?;
        let p = Path::new(&rec.video_path);
        if !p.exists() {
            return Err("The media file is no longer there.".into());
        }
        open_with_default_app(p);
        Ok(())
    }

    pub fn reveal(&self, id: &str) -> Result<(), String> {
        let rec = self
            .library
            .get(id)?
            .ok_or_else(|| "Recording not found.".to_string())?;
        let target = Path::new(&rec.video_path);
        if target.exists() {
            reveal_in_explorer(target);
        } else {
            open_with_default_app(Path::new(&rec.folder));
        }
        Ok(())
    }

    pub fn open_recordings_folder(&self) {
        let _ = fs::create_dir_all(&self.recordings_path);
        open_with_default_app(&self.recordings_path);
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let Some(rec) = self.library.get(id)? else {
            return Ok(());
        };
        if let (Ok(folder), Ok(root)) = (
            fs::canonicalize(&rec.folder),
            fs::canonicalize(&self.recordings_path),
        ) {
            if folder.parent() == Some(root.as_path()) && folder.exists() {
                let _ = fs::remove_dir_all(&folder);
            }
        }
        self.library.delete(id)?;
        Ok(())
    }

    pub fn ffmpeg(&self) -> Option<String> {
        self.recorder
            .as_ref()
            .map(|r| r.ffmpeg.clone())
            .or_else(find_ffmpeg)
    }

    pub fn data_path(&self) -> &Path {
        &self.data_path
    }
}

struct NoopSpeechModel;
impl crate::engine::final_pass::SpeechModel for NoopSpeechModel {
    fn recognize_text(&self, _audio: &[f32], _sample_rate: usize) -> Result<String, String> {
        Ok(String::new())
    }
    fn recognize_with_timestamps(
        &self,
        _audio: &[f32],
        _sample_rate: usize,
    ) -> Result<crate::engine::final_pass::TimestampedResult, String> {
        Ok(crate::engine::final_pass::TimestampedResult::default())
    }
}

struct SharedCanary(Arc<CanaryModel>);
impl crate::engine::canary_pass::CanaryRecognizer for SharedCanary {
    fn recognize_lang(&self, audio: &[f32], sample_rate: usize, language: &str) -> Result<String, String> {
        self.0.recognize_lang(audio, sample_rate, language)
    }
}

fn new_folder(root: &Path) -> PathBuf {
    let stamp = Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let mut candidate = root.join(&stamp);
    let mut n = 1u32;
    while candidate.exists() {
        n += 1;
        candidate = root.join(format!("{stamp}-{n}"));
    }
    candidate
}

fn reveal_in_explorer(path: &Path) {
    #[cfg(windows)]
    {
        let _ = Command::new("explorer")
            .arg("/select,")
            .arg(path)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        if let Some(parent) = path.parent() {
            let _ = Command::new("xdg-open").arg(parent).spawn();
        }
    }
}

fn open_with_default_app(path: &Path) {
    #[cfg(windows)]
    {
        use crate::engine::ffmpeg_locator::configure_no_window;
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", ""]).arg(path);
        configure_no_window(&mut cmd);
        let _ = cmd.spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("xdg-open").arg(path).spawn();
    }
}
