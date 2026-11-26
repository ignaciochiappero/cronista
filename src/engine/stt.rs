//! Local ONNX Runtime speech-to-text engine for NVIDIA Parakeet TDT 0.6B v3 (int8)
//! and NVIDIA Canary 1B v2 (int8), plus Kiri-style LazyModelManager (auto-unloads from RAM when idle).

use crate::downloads::RemoteFile;
use crate::engine::canary_pass::CanaryRecognizer;
use crate::engine::final_pass::{join_tokens, SpeechModel, TimestampedResult};
use crate::paths::{app_root, runtime_dir};
use ndarray::{Array1, Array2, Array3, ArrayD, IxDyn};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use rustfft::{num_complex::Complex, FftPlanner};
use std::collections::HashMap;
use std::f32::consts::PI;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const CANARY_BASE: &str =
    "https://huggingface.co/istupakov/canary-1b-v2-onnx/resolve/5ebc1520cef7b6b318b3526ad17adbfe00bc1bfc";

pub fn canary_files() -> Vec<RemoteFile> {
    vec![
        RemoteFile::new(
            "config.json",
            format!("{CANARY_BASE}/config.json"),
            68,
            "f90ace8e35326dcd47c7330b230644fa0835083ed1e89e2f59aa08ba10d74f54",
        ),
        RemoteFile::new(
            "vocab.txt",
            format!("{CANARY_BASE}/vocab.txt"),
            208_022,
            "2c9efe6104fd29522ea27ce0e3aef5d37c690af4e5a4232e643e23ca403ffea3",
        ),
        RemoteFile::new(
            "decoder-model.int8.onnx",
            format!("{CANARY_BASE}/decoder-model.int8.onnx"),
            170_040_374,
            "52d83aa7aad41fbbe4f9dfcd341d784735a6eb4c6eb0d3290fc27a0d8ac39abf",
        ),
        RemoteFile::new(
            "encoder-model.int8.onnx",
            format!("{CANARY_BASE}/encoder-model.int8.onnx"),
            859_078_138,
            "6d96e9945898e5ace48f4efecd459ca1df81859730be27b8af6b197639403ee1",
        ),
    ]
}

const PARAKEET_BASE: &str =
    "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main";

pub const ORT_WIN_WHEEL_URL: &str =
    "https://files.pythonhosted.org/packages/a9/3e/27039badc39a1b01628b6b40b1ff1bf4a886e6dfb2f311c483606b7c86f7/onnxruntime-1.32.0-cp312-cp312-win_amd64.whl";
pub const ORT_WIN_WHEEL_SIZE: u64 = 16_698_767;

pub fn parakeet_files() -> Vec<RemoteFile> {
    vec![
        RemoteFile::new("config.json", format!("{PARAKEET_BASE}/config.json"), 97, ""),
        RemoteFile::new("vocab.txt", format!("{PARAKEET_BASE}/vocab.txt"), 93_939, ""),
        RemoteFile::new("nemo128.onnx", format!("{PARAKEET_BASE}/nemo128.onnx"), 139_764, ""),
        RemoteFile::new(
            "decoder_joint-model.int8.onnx",
            format!("{PARAKEET_BASE}/decoder_joint-model.int8.onnx"),
            18_202_004,
            "",
        ),
        RemoteFile::new(
            "encoder-model.int8.onnx",
            format!("{PARAKEET_BASE}/encoder-model.int8.onnx"),
            652_183_999,
            "",
        ),
    ]
}

/// Extracts `onnxruntime*.dll` from a downloaded ONNX Runtime wheel/zip into `dest_dir`.
pub fn extract_ort_dlls(zip_path: &Path, dest_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dest_dir).map_err(|e| format!("create runtime dir failed: {e}"))?;
    let file = fs::File::open(zip_path).map_err(|e| format!("open ort zip failed: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("read ort zip failed: {e}"))?;
    let mut extracted = false;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("zip entry failed: {e}"))?;
        let name = entry.name().to_string();
        if let Some(fname) = name.rsplit('/').next() {
            if fname.eq_ignore_ascii_case("onnxruntime.dll")
                || fname.eq_ignore_ascii_case("onnxruntime_providers_shared.dll")
            {
                let mut out = fs::File::create(dest_dir.join(fname))
                    .map_err(|e| format!("write {fname} failed: {e}"))?;
                std::io::copy(&mut entry, &mut out)
                    .map_err(|e| format!("extract {fname} failed: {e}"))?;
                if fname.eq_ignore_ascii_case("onnxruntime.dll") {
                    extracted = true;
                }
            }
        }
    }
    if !extracted {
        return Err("onnxruntime.dll was not found inside the downloaded package.".into());
    }
    Ok(())
}

/// Locates `onnxruntime.dll` (Windows) or `libonnxruntime.so` (Linux) for `ort` `load-dynamic`.
pub fn find_onnxruntime_lib() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ORT_DYLIB_PATH") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    let lib_name = if cfg!(windows) {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };
    let candidates = [
        app_root().join(lib_name),
        app_root().join("runtime").join(lib_name),
        runtime_dir().join(lib_name),
    ];
    for c in candidates {
        if c.is_file() {
            return Some(c);
        }
    }
    // Check if an existing Python `.venv` inside the repo has onnxruntime's DLL
    if let Ok(cwd) = std::env::current_dir() {
        let venv_dll = cwd
            .join(".venv")
            .join("Lib")
            .join("site-packages")
            .join("onnxruntime")
            .join("capi")
            .join(lib_name);
        if venv_dll.is_file() {
            return Some(venv_dll);
        }
    }
    None
}

static ORT_ENV: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();

pub fn ensure_ort_initialized() -> Result<ort::environment::Environment, String> {
    ORT_ENV
        .get_or_init(|| {
            let lib = find_onnxruntime_lib().ok_or_else(|| {
                format!(
                    "onnxruntime library not found. Place onnxruntime.dll next to cronista.exe or in {}",
                    runtime_dir().display()
                )
            })?;
            ort::init_from(lib)
                .map_err(|e| format!("Failed to load ONNX Runtime DLL: {e}"))?
                .build()
                .map_err(|e| format!("Failed to initialize ONNX Runtime: {e}"))
        })
        .clone()
}

fn build_session(model_path: &Path, threads: usize) -> Result<Session, String> {
    let env = ensure_ort_initialized()?;
    Session::builder(&env)
        .map_err(|e| format!("ort session builder failed: {e}"))?
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
        .map_err(|e| format!("ort cpu ep failed: {e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| format!("ort opt level failed: {e}"))?
        .with_intra_threads(threads)
        .map_err(|e| format!("ort intra_threads failed: {e}"))?
        .with_inter_threads(1)
        .map_err(|e| format!("ort inter_threads failed: {e}"))?
        .with_memory_pattern(false)
        .map_err(|e| format!("ort memory_pattern failed: {e}"))?
        .with_config_entry("session.intra_op.allow_spinning", "0")
        .map_err(|e| format!("ort config entry failed: {e}"))?
        .with_config_entry("session.inter_op.allow_spinning", "0")
        .map_err(|e| format!("ort config entry failed: {e}"))?
        .commit_from_file(model_path)
        .map_err(|e| format!("Failed to load ONNX model {}: {e}", model_path.display()))
}

fn load_vocab(path: &Path) -> Result<(Vec<String>, HashMap<String, i64>), String> {
    let text = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read vocab {}: {e}", path.display()))?;
    let mut entries: Vec<(usize, String)> = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        // istupakov vocab.txt format: "<token> <id>"
        if let Some((tok, id_str)) = line.rsplit_once(' ') {
            if let Ok(id) = id_str.parse::<usize>() {
                entries.push((id, tok.to_string()));
                continue;
            }
        }
        entries.push((line_no, line.to_string()));
    }
    let max_id = entries.iter().map(|(id, _)| *id).max().unwrap_or(0);
    let mut id_to_token = vec![String::new(); max_id + 1];
    let mut token_to_id = HashMap::with_capacity(entries.len());
    for (id, tok) in entries {
        token_to_id.insert(tok.clone(), id as i64);
        id_to_token[id] = tok;
    }
    Ok((id_to_token, token_to_id))
}

fn decode_sp_token(raw: &str) -> String {
    raw.replace('\u{2581}', " ")
}

/// Pure-Rust NeMo 128-bin Log-Mel Spectrogram preprocessor (used when `nemo128.onnx` is not bundled,
/// such as for Canary 1B v2). Matches NeMo `AudioToMelSpectrogramPreprocessor` (16 kHz, 25 ms window,
/// 10 ms hop, 512 FFT, 128 Slaney mel bins, preemph 0.97, per-feature normalization).
pub struct NemoMelExtractor {
    n_mels: usize,
    n_fft: usize,
    win_len: usize,
    hop_len: usize,
    window: Vec<f32>,
    mel_fb: Vec<Vec<f32>>,
}

impl NemoMelExtractor {
    pub fn new_128() -> Self {
        let sample_rate = 16_000.0_f32;
        let n_mels = 128usize;
        let n_fft = 512usize;
        let win_len = 400usize;
        let hop_len = 160usize;

        let window: Vec<f32> = (0..win_len)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / (win_len as f32)).cos())
            .collect();

        // Slaney mel scale filterbank
        let hz_to_mel = |hz: f32| -> f32 {
            let f_sp = 200.0 / 3.0;
            let min_log_hz = 1000.0;
            let min_log_mel = min_log_hz / f_sp;
            let logstep = (6.4_f32).ln() / 27.0;
            if hz >= min_log_hz {
                min_log_mel + (hz / min_log_hz).ln() / logstep
            } else {
                hz / f_sp
            }
        };
        let mel_to_hz = |mel: f32| -> f32 {
            let f_sp = 200.0 / 3.0;
            let min_log_hz = 1000.0;
            let min_log_mel = min_log_hz / f_sp;
            let logstep = (6.4_f32).ln() / 27.0;
            if mel >= min_log_mel {
                min_log_hz * ((mel - min_log_mel) * logstep).exp()
            } else {
                mel * f_sp
            }
        };

        let n_freqs = n_fft / 2 + 1;
        let mel_min = hz_to_mel(0.0);
        let mel_max = hz_to_mel(sample_rate / 2.0);
        let mel_pts: Vec<f32> = (0..=n_mels + 1)
            .map(|i| mel_to_hz(mel_min + (mel_max - mel_min) * i as f32 / (n_mels + 1) as f32))
            .collect();
        let fft_freqs: Vec<f32> = (0..n_freqs)
            .map(|i| sample_rate * i as f32 / n_fft as f32)
            .collect();

        let mut mel_fb = vec![vec![0.0_f32; n_freqs]; n_mels];
        for m in 0..n_mels {
            let left = mel_pts[m];
            let center = mel_pts[m + 1];
            let right = mel_pts[m + 2];
            let enorm = 2.0 / (right - left).max(1e-6);
            for k in 0..n_freqs {
                let f = fft_freqs[k];
                let lower = (f - left) / (center - left).max(1e-6);
                let upper = (right - f) / (right - center).max(1e-6);
                let w = lower.min(upper).max(0.0);
                mel_fb[m][k] = w * enorm;
            }
        }

        Self {
            n_mels,
            n_fft,
            win_len,
            hop_len,
            window,
            mel_fb,
        }
    }

    /// Computes `[1, 128, T]` normalized log-mel features and valid frame length `T`.
    pub fn extract(&self, audio: &[f32]) -> (Array3<f32>, i64) {
        if audio.is_empty() {
            return (Array3::zeros((1, self.n_mels, 1)), 1);
        }

        // Pre-emphasis (0.97)
        let mut pre = Vec::with_capacity(audio.len());
        pre.push(audio[0]);
        for i in 1..audio.len() {
            pre.push(audio[i] - 0.97 * audio[i - 1]);
        }

        // Center pad by n_fft / 2 (reflect/zero)
        let pad = self.n_fft / 2;
        let mut padded = vec![0.0_f32; pre.len() + 2 * pad];
        padded[pad..pad + pre.len()].copy_from_slice(&pre);

        let n_frames = ((padded.len().saturating_sub(self.n_fft)) / self.hop_len) + 1;
        let n_freqs = self.n_fft / 2 + 1;

        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(self.n_fft);
        let mut buffer = vec![Complex::new(0.0_f32, 0.0_f32); self.n_fft];
        let mut power = vec![0.0_f32; n_freqs];

        let mut feats = Array3::<f32>::zeros((1, self.n_mels, n_frames));
        let win_offset = (self.n_fft - self.win_len) / 2;
        let guard = 2.0_f32.powi(-24);

        for t in 0..n_frames {
            let start = t * self.hop_len;
            for c in &mut buffer {
                *c = Complex::new(0.0, 0.0);
            }
            for i in 0..self.win_len {
                let sample = padded.get(start + win_offset + i).copied().unwrap_or(0.0);
                buffer[win_offset + i] = Complex::new(sample * self.window[i], 0.0);
            }
            fft.process(&mut buffer);
            for k in 0..n_freqs {
                let c = buffer[k];
                power[k] = c.re * c.re + c.im * c.im;
            }
            for m in 0..self.n_mels {
                let mut sum = 0.0_f32;
                let fb = &self.mel_fb[m];
                for k in 0..n_freqs {
                    sum += fb[k] * power[k];
                }
                feats[[0, m, t]] = (sum + guard).ln();
            }
        }

        // Per-feature normalization (mean 0, std 1) across T
        let inv_t = 1.0 / (n_frames as f32).max(1.0);
        for m in 0..self.n_mels {
            let mut mean = 0.0_f32;
            for t in 0..n_frames {
                mean += feats[[0, m, t]];
            }
            mean *= inv_t;
            let mut var = 0.0_f32;
            for t in 0..n_frames {
                let d = feats[[0, m, t]] - mean;
                var += d * d;
            }
            let std = (var / (n_frames.saturating_sub(1).max(1) as f32)).sqrt() + 1e-5;
            for t in 0..n_frames {
                feats[[0, m, t]] = (feats[[0, m, t]] - mean) / std;
            }
        }

        (feats, n_frames as i64)
    }
}

/// NVIDIA Parakeet TDT 0.6B v3 (int8) running natively via `ort`.
pub struct ParakeetModel {
    preprocessor: Option<Mutex<Session>>,
    mel_fallback: NemoMelExtractor,
    encoder: Mutex<Session>,
    decoder_joint: Mutex<Session>,
    vocab: Vec<String>,
    blank_id: i32,
    durations: Vec<usize>,
    frame_shift_secs: f64,
}

impl ParakeetModel {
    pub fn load(folder: &Path, threads: usize) -> Result<Self, String> {
        let enc_path = folder.join("encoder-model.int8.onnx");
        let dec_path = folder.join("decoder_joint-model.int8.onnx");
        let vocab_path = folder.join("vocab.txt");
        let nemo_path = folder.join("nemo128.onnx");

        if !enc_path.is_file() || !dec_path.is_file() || !vocab_path.is_file() {
            return Err(format!(
                "Parakeet model files missing in {}",
                folder.display()
            ));
        }

        let (vocab, _) = load_vocab(&vocab_path)?;
        let blank_id = (vocab.len() as i32) - 1;
        let preprocessor = if nemo_path.is_file() {
            Some(Mutex::new(build_session(&nemo_path, threads)?))
        } else {
            None
        };
        let encoder = Mutex::new(build_session(&enc_path, threads)?);
        let decoder_joint = Mutex::new(build_session(&dec_path, threads)?);

        Ok(Self {
            preprocessor,
            mel_fallback: NemoMelExtractor::new_128(),
            encoder,
            decoder_joint,
            vocab,
            blank_id,
            durations: vec![0, 1, 2, 3, 4],
            frame_shift_secs: 0.08, // 10ms hop * 8x subsampling
        })
    }

    fn compute_features(&self, audio: &[f32]) -> Result<(Array3<f32>, i64), String> {
        if let Some(pre) = &self.preprocessor {
            let mut sess = pre.lock().unwrap();
            let wav = Array2::from_shape_vec((1, audio.len()), audio.to_vec())
                .map_err(|e| format!("wav array failed: {e}"))?;
            let wav_len = Array1::from_vec(vec![audio.len() as i64]);
            let wav_t = Tensor::from_array(wav).map_err(|e| format!("wav tensor failed: {e}"))?;
            let len_t =
                Tensor::from_array(wav_len).map_err(|e| format!("wav_len tensor failed: {e}"))?;
            let outputs = sess
                .run(ort::inputs![
                    "waveforms" => wav_t,
                    "waveforms_lens" => len_t,
                ])
                .map_err(|e| format!("nemo128 run failed: {e}"))?;
            let (shape, data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("extract features failed: {e}"))?;
            let (_, lens_data) = outputs[1]
                .try_extract_tensor::<i64>()
                .map_err(|e| format!("extract features_lens failed: {e}"))?;
            let b = shape[0] as usize;
            let d = shape[1] as usize;
            let t = shape[2] as usize;
            let arr = Array3::from_shape_vec((b, d, t), data.to_vec())
                .map_err(|e| format!("features reshape failed: {e}"))?;
            return Ok((arr, lens_data[0]));
        }
        Ok(self.mel_fallback.extract(audio))
    }

    fn decode_internal(&self, audio: &[f32]) -> Result<TimestampedResult, String> {
        if audio.is_empty() {
            return Ok(TimestampedResult::default());
        }
        let (features, feat_len) = self.compute_features(audio)?;

        // 1. Run encoder
        let (enc_data, enc_dim, enc_time, valid_len) = {
            let mut enc = self.encoder.lock().unwrap();
            let sig_t = Tensor::from_array(features).map_err(|e| format!("enc input failed: {e}"))?;
            let len_t = Tensor::from_array(Array1::from_vec(vec![feat_len]))
                .map_err(|e| format!("enc len failed: {e}"))?;
            let outputs = enc
                .run(ort::inputs![
                    "audio_signal" => sig_t,
                    "length" => len_t,
                ])
                .map_err(|e| format!("Parakeet encoder failed: {e}"))?;
            let (shape, data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("extract encoder outputs failed: {e}"))?;
            let (_, len_out) = outputs[1]
                .try_extract_tensor::<i64>()
                .map_err(|e| format!("extract encoded_lengths failed: {e}"))?;
            (
                data.to_vec(),
                shape[1] as usize,
                shape[2] as usize,
                len_out[0] as usize,
            )
        };

        // 2. Greedy TDT decoding loop
        let mut dec = self.decoder_joint.lock().unwrap();
        let state_dim = 640usize;
        let num_layers = 2usize;
        let mut state1 = Array3::<f32>::zeros((num_layers, 1, state_dim));
        let mut state2 = Array3::<f32>::zeros((num_layers, 1, state_dim));
        let mut last_token: i32 = self.blank_id;

        let vocab_size = self.vocab.len();
        let max_symbols_per_step = 10usize;
        let mut tokens = Vec::new();
        let mut timestamps = Vec::new();

        let mut t = 0usize;
        let mut emitted_at_t = 0usize;

        while t < valid_len && t < enc_time {
            // Slice [1, enc_dim, 1] for frame t
            let mut frame = Vec::with_capacity(enc_dim);
            for d in 0..enc_dim {
                frame.push(enc_data[d * enc_time + t]);
            }
            let enc_frame = Array3::from_shape_vec((1, enc_dim, 1), frame)
                .map_err(|e| format!("enc_frame reshape failed: {e}"))?;
            let targets = Array2::from_elem((1, 1), last_token);
            let target_len = Array1::from_vec(vec![1i32]);

            let enc_t = Tensor::from_array(enc_frame).map_err(|e| format!("{e}"))?;
            let tgt_t = Tensor::from_array(targets).map_err(|e| format!("{e}"))?;
            let tlen_t = Tensor::from_array(target_len).map_err(|e| format!("{e}"))?;
            let s1_t = Tensor::from_array(state1.clone()).map_err(|e| format!("{e}"))?;
            let s2_t = Tensor::from_array(state2.clone()).map_err(|e| format!("{e}"))?;

            let outputs = dec
                .run(ort::inputs![
                    "encoder_outputs" => enc_t,
                    "targets" => tgt_t,
                    "target_length" => tlen_t,
                    "input_states_1" => s1_t,
                    "input_states_2" => s2_t,
                ])
                .map_err(|e| format!("Parakeet decoder_joint failed: {e}"))?;

            let (_, logits) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("extract logits failed: {e}"))?;

            let token_logits = &logits[..vocab_size.min(logits.len())];
            let (best_tok_idx, _) = token_logits
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap_or((self.blank_id as usize, &0.0));

            let skip = if logits.len() > vocab_size {
                let dur_logits = &logits[vocab_size..];
                let (dur_idx, _) = dur_logits
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .unwrap_or((1, &0.0));
                self.durations.get(dur_idx).copied().unwrap_or(1)
            } else {
                1
            };

            if best_tok_idx as i32 != self.blank_id {
                // Commit new LSTM states
                let (s1_shape, s1_data) = outputs[2]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| format!("extract output_states_1 failed: {e}"))?;
                let (s2_shape, s2_data) = outputs[3]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| format!("extract output_states_2 failed: {e}"))?;
                state1 = Array3::from_shape_vec(
                    (s1_shape[0] as usize, s1_shape[1] as usize, s1_shape[2] as usize),
                    s1_data.to_vec(),
                )
                .map_err(|e| format!("s1 reshape: {e}"))?;
                state2 = Array3::from_shape_vec(
                    (s2_shape[0] as usize, s2_shape[1] as usize, s2_shape[2] as usize),
                    s2_data.to_vec(),
                )
                .map_err(|e| format!("s2 reshape: {e}"))?;

                last_token = best_tok_idx as i32;
                let tok_str = decode_sp_token(&self.vocab[best_tok_idx]);
                if !tok_str.is_empty() && !tok_str.starts_with('<') {
                    tokens.push(tok_str);
                    timestamps.push((t as f64) * self.frame_shift_secs);
                }
                emitted_at_t += 1;
                if skip > 0 {
                    t += skip;
                    emitted_at_t = 0;
                } else if emitted_at_t >= max_symbols_per_step {
                    t += 1;
                    emitted_at_t = 0;
                }
            } else {
                t += skip.max(1);
                emitted_at_t = 0;
            }
        }

        Ok(TimestampedResult { tokens, timestamps })
    }
}

impl SpeechModel for ParakeetModel {
    fn recognize_text(&self, audio: &[f32], _sample_rate: usize) -> Result<String, String> {
        let res = self.decode_internal(audio)?;
        Ok(join_tokens(&res.tokens))
    }

    fn recognize_with_timestamps(
        &self,
        audio: &[f32],
        _sample_rate: usize,
    ) -> Result<TimestampedResult, String> {
        self.decode_internal(audio)
    }
}

/// NVIDIA Canary 1B v2 (int8) encoder-decoder running natively via `ort`.
pub struct CanaryModel {
    mel: NemoMelExtractor,
    encoder: Mutex<Session>,
    decoder: Mutex<Session>,
    vocab: Vec<String>,
    token_to_id: HashMap<String, i64>,
}

impl CanaryModel {
    pub fn load(folder: &Path, threads: usize) -> Result<Self, String> {
        let enc_path = folder.join("encoder-model.int8.onnx");
        let dec_path = folder.join("decoder-model.int8.onnx");
        let vocab_path = folder.join("vocab.txt");
        if !enc_path.is_file() || !dec_path.is_file() || !vocab_path.is_file() {
            return Err(format!("Canary model files missing in {}", folder.display()));
        }
        let (vocab, token_to_id) = load_vocab(&vocab_path)?;
        let encoder = Mutex::new(build_session(&enc_path, threads)?);
        let decoder = Mutex::new(build_session(&dec_path, threads)?);
        Ok(Self {
            mel: NemoMelExtractor::new_128(),
            encoder,
            decoder,
            vocab,
            token_to_id,
        })
    }

    fn tok(&self, name: &str) -> Option<i64> {
        self.token_to_id.get(name).copied()
    }
}

impl CanaryRecognizer for CanaryModel {
    fn recognize_lang(&self, audio: &[f32], _sample_rate: usize, language: &str) -> Result<String, String> {
        if audio.is_empty() {
            return Ok(String::new());
        }
        let (features, feat_len) = self.mel.extract(audio);

        // 1. Run encoder
        let (enc_emb, enc_mask) = {
            let mut enc = self.encoder.lock().unwrap();
            let sig_t = Tensor::from_array(features).map_err(|e| format!("{e}"))?;
            let len_t = Tensor::from_array(Array1::from_vec(vec![feat_len]))
                .map_err(|e| format!("{e}"))?;
            let outputs = enc
                .run(ort::inputs![
                    "audio_signal" => sig_t,
                    "length" => len_t,
                ])
                .map_err(|e| format!("Canary encoder failed: {e}"))?;
            let (e_shape, e_data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("Canary encoder emb failed: {e}"))?;
            let (m_shape, m_data) = outputs[1]
                .try_extract_tensor::<i64>()
                .map_err(|e| format!("Canary encoder mask failed: {e}"))?;
            let emb = ArrayD::from_shape_vec(
                IxDyn(&e_shape.iter().map(|&d| d as usize).collect::<Vec<_>>()),
                e_data.to_vec(),
            )
            .map_err(|e| format!("Canary emb reshape: {e}"))?;
            let mask = ArrayD::from_shape_vec(
                IxDyn(&m_shape.iter().map(|&d| d as usize).collect::<Vec<_>>()),
                m_data.to_vec(),
            )
            .map_err(|e| format!("Canary mask reshape: {e}"))?;
            (emb, mask)
        };

        // 2. Build Canary prompt tokens
        let lang_tok = format!("<|{language}|>");
        let prompt_names = [
            "<|startofcontext|>",
            "<|startoftranscript|>",
            "<|emo:undefined|>",
            lang_tok.as_str(),
            lang_tok.as_str(),
            "<|pnc|>",
            "<|noitn|>",
            "<|notimestamp|>",
            "<|nodiarize|>",
        ];
        let mut ids: Vec<i64> = prompt_names
            .iter()
            .filter_map(|n| self.tok(n))
            .collect();
        let eos_id = self.tok("<|endoftext|>").unwrap_or(3);
        let prompt_len = ids.len();
        let max_new = 256usize;

        // 3. Autoregressive greedy decoding (handles both cached and uncached decoder signatures)
        let mut dec = self.decoder.lock().unwrap();
        let input_names: Vec<String> = dec.inputs().iter().map(|i| i.name().to_string()).collect();
        let has_mems = input_names.iter().any(|n| n == "decoder_mems");

        let mut mems: Option<ArrayD<f32>> = None;
        for step in 0..max_new {
            let step_ids = if has_mems && step > 0 {
                vec![*ids.last().unwrap()]
            } else {
                ids.clone()
            };
            let ids_arr = Array2::from_shape_vec((1, step_ids.len()), step_ids)
                .map_err(|e| format!("{e}"))?;
            let ids_t = Tensor::from_array(ids_arr).map_err(|e| format!("{e}"))?;
            let emb_t = Tensor::from_array(enc_emb.clone()).map_err(|e| format!("{e}"))?;
            let mask_t = Tensor::from_array(enc_mask.clone()).map_err(|e| format!("{e}"))?;

            let outputs = if has_mems {
                let m = mems
                    .take()
                    .unwrap_or_else(|| ArrayD::zeros(IxDyn(&[10, 1, 0, 1024])));
                let mems_t = Tensor::from_array(m).map_err(|e| format!("{e}"))?;
                dec.run(ort::inputs![
                    "input_ids" => ids_t,
                    "encoder_embeddings" => emb_t,
                    "encoder_mask" => mask_t,
                    "decoder_mems" => mems_t,
                ])
                .map_err(|e| format!("Canary decoder failed: {e}"))?
            } else {
                dec.run(ort::inputs![
                    "input_ids" => ids_t,
                    "encoder_embeddings" => emb_t,
                    "encoder_mask" => mask_t,
                ])
                .map_err(|e| format!("Canary decoder failed: {e}"))?
            };

            let (l_shape, l_data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| format!("Canary logits failed: {e}"))?;
            let seq_len = l_shape[1] as usize;
            let vocab_sz = l_shape[2] as usize;
            let last_slice = &l_data[(seq_len - 1) * vocab_sz..seq_len * vocab_sz];
            let (next_id, _) = last_slice
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap_or((eos_id as usize, &0.0));

            if has_mems && outputs.len() > 1 {
                if let Ok((ms, md)) = outputs[1].try_extract_tensor::<f32>() {
                    if let Ok(arr) = ArrayD::from_shape_vec(
                        IxDyn(&ms.iter().map(|&d| d as usize).collect::<Vec<_>>()),
                        md.to_vec(),
                    ) {
                        mems = Some(arr);
                    }
                }
            }

            if next_id as i64 == eos_id {
                break;
            }
            ids.push(next_id as i64);
        }

        let piece_tokens: Vec<String> = ids[prompt_len..]
            .iter()
            .filter_map(|&id| self.vocab.get(id as usize))
            .map(|t| decode_sp_token(t))
            .filter(|t| !t.starts_with('<'))
            .collect();
        Ok(join_tokens(&piece_tokens))
    }
}

/// Kiri-style lazy model manager: keeps RAM at ~20-35 MB while idle by loading Parakeet on demand
/// and automatically dropping the ONNX sessions after `idle_unload` elapses without activity.
pub struct LazyModelManager {
    threads: usize,
    idle_unload: Duration,
    inner: Mutex<LazyInner>,
}

struct LazyInner {
    parakeet: Option<Arc<ParakeetModel>>,
    last_used: Instant,
    active_users: usize,
}

impl LazyModelManager {
    pub fn new(_parakeet_dir: PathBuf, threads: usize, idle_unload_secs: u64) -> Self {
        Self {
            threads,
            idle_unload: Duration::from_secs(idle_unload_secs),
            inner: Mutex::new(LazyInner {
                parakeet: None,
                last_used: Instant::now(),
                active_users: 0,
            }),
        }
    }

    pub fn is_model_present_on_disk(&self) -> bool {
        let dir = crate::paths::parakeet_model_dir();
        dir.join("encoder-model.int8.onnx").is_file()
            && dir.join("decoder_joint-model.int8.onnx").is_file()
            && dir.join("vocab.txt").is_file()
            && find_onnxruntime_lib().is_some()
    }

    pub fn is_loaded_in_ram(&self) -> bool {
        self.inner.lock().unwrap().parakeet.is_some()
    }

    /// Acquires Parakeet (loading from disk if currently sleeping).
    pub fn acquire_parakeet(&self) -> Result<Arc<ParakeetModel>, String> {
        {
            let mut guard = self.inner.lock().unwrap();
            if let Some(m) = guard.parakeet.as_ref().cloned() {
                guard.active_users += 1;
                guard.last_used = Instant::now();
                return Ok(m);
            }
        }
        let loaded = Arc::new(ParakeetModel::load(&crate::paths::parakeet_model_dir(), self.threads)?);
        let mut guard = self.inner.lock().unwrap();
        guard.parakeet = Some(Arc::clone(&loaded));
        guard.active_users += 1;
        guard.last_used = Instant::now();
        Ok(loaded)
    }

    pub fn release_parakeet(&self) {
        let mut guard = self.inner.lock().unwrap();
        guard.active_users = guard.active_users.saturating_sub(1);
        guard.last_used = Instant::now();
    }

    /// Unloads Parakeet from RAM if idle longer than `idle_unload` and nobody is using it.
    pub fn maybe_unload_idle(&self) -> bool {
        if self.idle_unload.is_zero() {
            return false;
        }
        let mut guard = self.inner.lock().unwrap();
        if guard.active_users == 0
            && guard.parakeet.is_some()
            && guard.last_used.elapsed() >= self.idle_unload
        {
            guard.parakeet = None;
            return true;
        }
        false
    }

    /// Force-unloads Parakeet immediately if no task is active.
    pub fn unload_now(&self) -> bool {
        let mut guard = self.inner.lock().unwrap();
        if guard.active_users == 0 && guard.parakeet.is_some() {
            guard.parakeet = None;
            return true;
        }
        false
    }
}

/// Returns current process Working Set RAM in megabytes (Windows `GetProcessMemoryInfo`).
pub fn current_ram_mb() -> f64 {
    #[cfg(windows)]
    {
        use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows::Win32::System::Threading::GetCurrentProcess;
        let mut counters = PROCESS_MEMORY_COUNTERS::default();
        let ok = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut counters,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            )
        };
        if ok.is_ok() {
            return counters.WorkingSetSize as f64 / (1024.0 * 1024.0);
        }
    }
    0.0
}
