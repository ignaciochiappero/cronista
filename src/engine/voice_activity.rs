//! Energy-based voice activity segmentation for live transcription.
//! Exact behavioral parity with `cronista/engine/voice_activity.py`.

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq)]
pub enum VoiceEventKind {
    Partial,
    Final,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VoiceEvent {
    pub kind: VoiceEventKind,
    pub audio: Vec<f32>,
    pub start_sample: usize,
    pub speech_end_sample: isize,
    pub reason: String,
}

impl VoiceEvent {
    pub fn end_sample(&self) -> usize {
        self.start_sample + self.audio.len()
    }
}

pub fn rms(frame: &[f32]) -> f64 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = frame.iter().map(|&x| (x as f64) * (x as f64)).sum();
    (sum_sq / frame.len() as f64).sqrt()
}

/// Linear-interpolated percentile matching `numpy.percentile(data, q)` (0..=100).
fn percentile(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let pos = (q / 100.0) * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        let frac = pos - lo as f64;
        sorted[lo] * (1.0 - frac) + sorted[hi] * frac
    }
}

#[derive(Debug, Clone)]
pub struct VadConfig {
    pub sample_rate: usize,
    pub frame_seconds: f64,
    pub speech_threshold: f64,
    pub silence_flush_seconds: f64,
    pub partial_every_seconds: f64,
    pub max_utterance_seconds: f64,
    pub calibration_seconds: f64,
    pub noise_multiplier: f64,
    pub max_threshold: f64,
    pub pre_roll_seconds: f64,
    pub keep_trailing_silence_seconds: f64,
    pub min_speech_seconds: f64,
    pub cut_search_seconds: f64,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            frame_seconds: 0.085,
            speech_threshold: 0.015,
            silence_flush_seconds: 1.2,
            partial_every_seconds: 1.0,
            max_utterance_seconds: 15.0,
            calibration_seconds: 1.0,
            noise_multiplier: 2.0,
            max_threshold: 0.08,
            pre_roll_seconds: 0.2,
            keep_trailing_silence_seconds: 0.3,
            min_speech_seconds: 0.2,
            cut_search_seconds: 2.0,
        }
    }
}

pub struct VoiceActivitySegmenter {
    pub sample_rate: usize,
    pub frame_size: usize,
    silence_flush_frames: usize,
    partial_every_frames: usize,
    max_utterance_frames: usize,
    calibration_frames: usize,
    keep_trailing_frames: usize,
    min_speech_frames: usize,
    cut_search_frames: usize,
    noise_multiplier: f64,
    min_threshold: f64,
    max_threshold: f64,
    pub threshold: f64,
    pre_roll_maxlen: usize,

    leftover: Vec<f32>,
    next_frame_start: usize,
    calibration_rms: Vec<f64>,
    pre_roll: VecDeque<(usize, Vec<f32>, f64)>,

    start: Option<usize>,
    frames: Vec<Vec<f32>>,
    rms_values: Vec<f64>,
    speech_frames: usize,
    speech_since_partial: usize,
    silence_run: usize,
    speech_end: isize,
}

impl VoiceActivitySegmenter {
    pub fn new(cfg: VadConfig) -> Self {
        let frame_size = (cfg.sample_rate as f64 * cfg.frame_seconds).round() as usize;
        let frames_ceil = |secs: f64| -> usize { ((secs / cfg.frame_seconds).ceil() as usize).max(1) };

        let calibration_frames = if cfg.calibration_seconds > 0.0 {
            frames_ceil(cfg.calibration_seconds)
        } else {
            0
        };
        let pre_roll_maxlen = frames_ceil(cfg.pre_roll_seconds);

        Self {
            sample_rate: cfg.sample_rate,
            frame_size,
            silence_flush_frames: frames_ceil(cfg.silence_flush_seconds),
            partial_every_frames: frames_ceil(cfg.partial_every_seconds),
            max_utterance_frames: (cfg.max_utterance_seconds / cfg.frame_seconds) as usize,
            calibration_frames,
            keep_trailing_frames: (cfg.keep_trailing_silence_seconds / cfg.frame_seconds) as usize,
            min_speech_frames: frames_ceil(cfg.min_speech_seconds),
            cut_search_frames: frames_ceil(cfg.cut_search_seconds),
            noise_multiplier: cfg.noise_multiplier,
            min_threshold: cfg.speech_threshold,
            max_threshold: cfg.max_threshold,
            threshold: cfg.speech_threshold,
            pre_roll_maxlen,
            leftover: Vec::new(),
            next_frame_start: 0,
            calibration_rms: Vec::new(),
            pre_roll: VecDeque::with_capacity(pre_roll_maxlen + 1),
            start: None,
            frames: Vec::new(),
            rms_values: Vec::new(),
            speech_frames: 0,
            speech_since_partial: 0,
            silence_run: 0,
            speech_end: -1,
        }
    }

    pub fn default_16k() -> Self {
        Self::new(VadConfig::default())
    }

    pub fn calibrating(&self) -> bool {
        self.calibration_rms.len() < self.calibration_frames
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<VoiceEvent> {
        let mut data = Vec::with_capacity(self.leftover.len() + samples.len());
        data.extend_from_slice(&self.leftover);
        data.extend_from_slice(samples);

        let count = data.len() / self.frame_size;
        let mut events = Vec::new();
        for i in 0..count {
            let slice = data[i * self.frame_size..(i + 1) * self.frame_size].to_vec();
            events.extend(self.on_frame(slice));
        }
        self.leftover = data[count * self.frame_size..].to_vec();
        events
    }

    pub fn flush(&mut self) -> Vec<VoiceEvent> {
        if self.start.is_none() {
            return Vec::new();
        }
        if !self.leftover.is_empty() {
            let tail = std::mem::take(&mut self.leftover);
            let r = rms(&tail);
            self.frames.push(tail);
            self.rms_values.push(r);
        }
        self.finalize(false, "end")
    }

    fn reset_utterance(&mut self) {
        self.start = None;
        self.frames.clear();
        self.rms_values.clear();
        self.speech_frames = 0;
        self.speech_since_partial = 0;
        self.silence_run = 0;
        self.speech_end = -1;
    }

    fn push_pre_roll(&mut self, start: usize, frame: Vec<f32>, r: f64) {
        if self.pre_roll.len() == self.pre_roll_maxlen {
            self.pre_roll.pop_front();
        }
        self.pre_roll.push_back((start, frame, r));
    }

    fn concat_frames(frames: &[Vec<f32>]) -> Vec<f32> {
        let total: usize = frames.iter().map(|f| f.len()).sum();
        let mut out = Vec::with_capacity(total);
        for f in frames {
            out.extend_from_slice(f);
        }
        out
    }

    fn on_frame(&mut self, frame: Vec<f32>) -> Vec<VoiceEvent> {
        let start = self.next_frame_start;
        self.next_frame_start += frame.len();
        let r = rms(&frame);

        if self.calibrating() {
            self.calibration_rms.push(r);
            if !self.calibrating() {
                let floor = percentile(&self.calibration_rms, 20.0);
                self.threshold = (floor * self.noise_multiplier)
                    .max(self.min_threshold)
                    .min(self.max_threshold);
            }
            self.push_pre_roll(start, frame, r);
            return Vec::new();
        }

        let is_speech = r > self.threshold;
        if self.start.is_none() {
            if !is_speech {
                self.push_pre_roll(start, frame, r);
                return Vec::new();
            }
            self.start = Some(self.pre_roll.front().map(|x| x.0).unwrap_or(start));
            while let Some((_, pre_frame, pre_rms)) = self.pre_roll.pop_front() {
                self.frames.push(pre_frame);
                self.rms_values.push(pre_rms);
            }
        }

        let frame_len = frame.len();
        self.frames.push(frame);
        self.rms_values.push(r);

        if is_speech {
            self.silence_run = 0;
            self.speech_frames += 1;
            self.speech_since_partial += 1;
            self.speech_end = (start + frame_len) as isize;
        } else {
            self.silence_run += 1;
            if self.silence_run >= self.silence_flush_frames {
                return self.finalize(true, "silence");
            }
        }

        if is_speech && self.frames.len() >= self.max_utterance_frames {
            return self.cut_at_quietest();
        }
        if is_speech && self.speech_since_partial >= self.partial_every_frames {
            self.speech_since_partial = 0;
            return vec![VoiceEvent {
                kind: VoiceEventKind::Partial,
                audio: Self::concat_frames(&self.frames),
                start_sample: self.start.unwrap_or(0),
                speech_end_sample: self.speech_end,
                reason: String::new(),
            }];
        }
        Vec::new()
    }

    fn finalize(&mut self, trim_trailing: bool, reason: &str) -> Vec<VoiceEvent> {
        let slice_end = if trim_trailing && self.silence_run > 0 {
            let keep = self.frames.len() + self.keep_trailing_frames;
            keep.saturating_sub(self.silence_run).min(self.frames.len())
        } else {
            self.frames.len()
        };
        let event = if self.speech_frames >= self.min_speech_frames {
            Some(VoiceEvent {
                kind: VoiceEventKind::Final,
                audio: Self::concat_frames(&self.frames[..slice_end]),
                start_sample: self.start.unwrap_or(0),
                speech_end_sample: self.speech_end,
                reason: reason.to_string(),
            })
        } else {
            None
        };
        self.reset_utterance();
        event.into_iter().collect()
    }

    fn cut_at_quietest(&mut self) -> Vec<VoiceEvent> {
        let start_val = self.start.expect("start set during utterance");
        let window_start = self
            .frames
            .len()
            .saturating_sub(self.cut_search_frames)
            .max(1);
        let mut best_offset = 0usize;
        let mut best_val = f64::INFINITY;
        for (i, &val) in self.rms_values[window_start..].iter().enumerate() {
            if val < best_val {
                best_val = val;
                best_offset = i;
            }
        }
        let cut = window_start + best_offset;
        let tail_frames = self.frames.split_off(cut);
        let tail_rms = self.rms_values.split_off(cut);
        let head_frames = std::mem::replace(&mut self.frames, tail_frames);
        let head_rms = std::mem::replace(&mut self.rms_values, tail_rms);
        let head_samples: usize = head_frames.iter().map(|f| f.len()).sum();

        let mut events = Vec::new();
        let head_speech = head_rms.iter().filter(|&&r| r > self.threshold).count();
        if head_speech >= self.min_speech_frames {
            events.push(VoiceEvent {
                kind: VoiceEventKind::Final,
                audio: Self::concat_frames(&head_frames),
                start_sample: start_val,
                speech_end_sample: (start_val + head_samples) as isize,
                reason: "cap".to_string(),
            });
        }

        self.start = Some(start_val + head_samples);
        self.speech_frames = self.rms_values.iter().filter(|&&r| r > self.threshold).count();
        self.speech_since_partial = 0;
        let mut trailing_silence = 0usize;
        for &r in self.rms_values.iter().rev() {
            if r > self.threshold {
                break;
            }
            trailing_silence += 1;
        }
        self.silence_run = trailing_silence;
        events
    }
}
