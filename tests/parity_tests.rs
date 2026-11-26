//! Behavioral parity test suite ported from all 18 Python `tests/test_*.py` files,
//! plus new tests for Audio-Only recording mode and LazyModelManager.

use chrono::NaiveDate;
use cronista::downloads::{download_with_chunk, missing, DownloadError, RemoteFile};
use cronista::engine::audio_sources::{AudioTimeline, StartGate, TICKS_PER_SECOND};
use cronista::engine::canary_pass::{guess_language, spaced, CanaryPass, CanaryRecognizer};
use cronista::engine::final_pass::{
    find_quiet_cut, join_tokens, loudest, run_final_pass, run_final_pass_file,
    segments_from_tokens, FinalPassOptions, SpeechModel, TimestampedResult,
};
use cronista::engine::local_agreement::{common_prefix, tokenize, LocalAgreement};
use cronista::engine::recorders::{
    pick_microphone, PipeInputSpec, RecorderSettings, WindowsRecorder,
};
use cronista::engine::stt::NemoMelExtractor;
use cronista::engine::transcript::{
    format_srt_timestamp, tidy_text, to_srt, word_difference, TranscriptionSegment,
};
use cronista::engine::voice_activity::{VadConfig, VoiceActivitySegmenter, VoiceEventKind};
use cronista::engine::voices::{drop_echoes, line, speakers};
use cronista::engine::wasapi::AudioDevice;
use cronista::export::{write_pdf, write_text_files, write_word};
use cronista::importer::{read_report, ImportError};
use cronista::language::{default_labels, default_title, language_or_default};
use cronista::library::{Library, NewRecording, sanitize_fts_query, SCHEMA};
use cronista::settings::{load_settings, save_settings_patch, RecordMode};
use cronista::subtitles::ass_document;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

// =================================================================================================
// 1. transcript.rs parity (test_transcript.py)
// =================================================================================================

#[test]
fn test_transcript_tidy_text_and_wer_and_srt() {
    assert_eq!(tidy_text("hola.¿Qué tal?¿Cómo te va?"), "hola. ¿Qué tal? ¿Cómo te va?");
    assert_eq!(tidy_text("¡Hola!¡Chau!"), "¡Hola! ¡Chau!");
    assert_eq!(tidy_text("Dijo: (¿en serio?) sí."), "Dijo: (¿en serio?) sí.");

    assert_eq!(word_difference("Hola, qué tal.", "hola qué tal"), 0.0);
    assert!(
        (word_difference(
            "este es un test para ir probando",
            "este es un para ir probando"
        ) - 1.0 / 7.0)
            .abs()
            < 1e-9
    );
    assert_eq!(word_difference("", ""), 0.0);
    assert_eq!(word_difference("", "algo"), 1.0);

    assert_eq!(format_srt_timestamp(0.0), "00:00:00,000");
    assert_eq!(format_srt_timestamp(3661.5), "01:01:01,500");
    assert_eq!(format_srt_timestamp(59.9996), "00:01:00,000");

    let srt = to_srt(
        &[
            TranscriptionSegment::new(1.0, 2.5, "Hola.", None),
            TranscriptionSegment::new(3.0, 4.25, "This is a test.", None),
        ],
        None,
    );
    assert_eq!(
        srt,
        "1\n00:00:01,000 --> 00:00:02,500\nHola.\n\n2\n00:00:03,000 --> 00:00:04,250\nThis is a test.\n"
    );
    assert_eq!(to_srt(&[TranscriptionSegment::new(0.0, 1.0, "  ", None)], None), "");
}

// =================================================================================================
// 2. voices.rs parity (test_voices.py)
// =================================================================================================

#[test]
fn test_voices_echo_dropping_and_labels() {
    let seg = |s, e, t: &str, src: &str| TranscriptionSegment::new(s, e, t, Some(src.into()));

    let computer = seg(10.0, 13.0, "Sure, I have the screens ready.", "computer");
    let echo = seg(10.1, 13.1, "Sure I have the screens ready", "mic");
    let own = seg(14.0, 16.0, "Great, share them.", "mic");
    assert_eq!(
        drop_echoes(&[echo, computer.clone(), own.clone()], 0.6, 1.0),
        vec![computer.clone(), own]
    );

    let weak_echo = seg(10.2, 12.0, "the screens ready", "mic");
    assert_eq!(drop_echoes(&[computer.clone(), weak_echo], 0.6, 1.0), vec![computer.clone()]);

    let talking_over = seg(11.0, 12.5, "Wait, which screens?", "mic");
    assert_eq!(
        drop_echoes(&[computer.clone(), talking_over.clone()], 0.6, 1.0),
        vec![computer, talking_over]
    );

    let first = seg(1.0, 2.0, "Okay, see you then.", "computer");
    let later = seg(30.0, 31.0, "Okay, see you then.", "mic");
    assert_eq!(drop_echoes(&[later.clone(), first.clone()], 0.6, 1.0), vec![first, later]);

    let labels: HashMap<String, String> = [
        ("mic".into(), "Me".into()),
        ("computer".into(), "Ana".into()),
    ]
    .into();
    let both = vec![seg(0.0, 1.0, "Hi.", "mic"), seg(1.0, 2.0, "Hello.", "computer")];
    assert_eq!(speakers(&both, Some(&labels)), Some(labels.clone()));
    assert_eq!(speakers(&[seg(0.0, 1.0, "Hi.", "mic")], Some(&labels)), None);
    let names = speakers(&both, Some(&labels));
    assert_eq!(line(&both[1], names.as_ref()), "Ana: Hello.");
    assert_eq!(line(&both[1], None), "Hello.");
}

// =================================================================================================
// 3. local_agreement.rs parity (test_local_agreement.py)
// =================================================================================================

#[test]
fn test_local_agreement_parity() {
    assert_eq!(tokenize(Some("  hola   cómo estás ")), vec!["hola", "cómo", "estás"]);
    assert!(tokenize(Some("")).is_empty());
    assert!(tokenize(None).is_empty());
    assert_eq!(tokenize(Some("hola, qué tal")), vec!["hola,", "qué", "tal"]);

    assert_eq!(
        common_prefix(&[
            vec!["a".into(), "b".into(), "c".into()],
            vec!["a".into(), "b".into(), "d".into()],
        ]),
        vec!["a", "b"]
    );
    assert!(LocalAgreement::new(1).is_err());

    let mut la = LocalAgreement::default_n2();
    let r = la.update(Some("hola como estas"));
    assert_eq!(r.committed, "");
    assert_eq!(r.pending, "hola como estas");

    let r = la.update(Some("hola cómo estás amigo"));
    assert_eq!(r.committed, "hola");
    assert_eq!(r.pending, "cómo estás amigo");
    assert_eq!(r.newly_committed, "hola");

    let r = la.update(Some("hola cómo estás amigo mío"));
    assert_eq!(r.committed, "hola cómo estás amigo");
    assert_eq!(r.pending, "mío");

    la.reset();
    let h1 = "Good morning, everyone. Thanks for joining";
    la.update(Some(h1));
    la.update(Some(h1));
    let r = la.update(Some("Good morning. Everyone thanks for joining the call"));
    assert_eq!(r.committed, h1);
    assert_eq!(r.pending, "the call");

    let mut la3 = LocalAgreement::new(3).unwrap();
    assert_eq!(la3.update(Some("uno dos")).committed, "");
    assert_eq!(la3.update(Some("uno dos")).committed, "");
    assert_eq!(la3.update(Some("uno dos tres")).committed, "uno dos");

    la.reset();
    la.update(Some("uno dos"));
    la.update(Some("uno dos"));
    la.flush(Some("uno dos tres"));
    assert_eq!(la.text(), "uno dos tres");
}

// =================================================================================================
// 4. voice_activity.rs parity (test_voice_activity.py)
// =================================================================================================

fn tone(seconds: f64, amplitude: f32) -> Vec<f32> {
    let n = (seconds * 16_000.0) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / 16_000.0;
            amplitude * 2.0_f32.sqrt() * (2.0 * std::f32::consts::PI * 220.0 * t).sin()
        })
        .collect()
}

fn steady_noise(seconds: f64, amplitude: f32) -> Vec<f32> {
    let n = (seconds * 16_000.0) as usize;
    (0..n)
        .map(|i| {
            // Deterministic zero-mean signal with RMS == amplitude
            let t = i as f32 / 16_000.0;
            amplitude * 2.0_f32.sqrt() * (2.0 * std::f32::consts::PI * 733.0 * t).sin()
        })
        .collect()
}

fn feed(seg: &mut VoiceActivitySegmenter, parts: &[&[f32]]) -> Vec<cronista::engine::voice_activity::VoiceEvent> {
    let mut audio = Vec::new();
    for p in parts {
        audio.extend_from_slice(p);
    }
    let mut events = Vec::new();
    for chunk in audio.chunks(1600) {
        events.extend(seg.push(chunk));
    }
    events
}

#[test]
fn test_voice_activity_parity() {
    let mk = |max_s: f64| {
        VoiceActivitySegmenter::new(VadConfig {
            calibration_seconds: 0.5,
            max_utterance_seconds: max_s,
            ..Default::default()
        })
    };

    // Silence produces no events
    assert!(feed(&mut mk(15.0), &[&steady_noise(5.0, 0.001)]).is_empty());

    // Short phrase -> one final with pre-roll and trimmed trailing silence
    let events = feed(
        &mut mk(15.0),
        &[&steady_noise(1.0, 0.001), &tone(0.6, 0.1), &steady_noise(2.0, 0.001)],
    );
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, VoiceEventKind::Final);
    assert_eq!(events[0].reason, "silence");
    let start_s = events[0].start_sample as f64 / 16_000.0;
    let len_s = events[0].audio.len() as f64 / 16_000.0;
    assert!((0.6..=1.0).contains(&start_s));
    assert!((0.6..=1.3).contains(&len_s));

    // Cumulative partials every ~1s
    let events = feed(&mut mk(15.0), &[&steady_noise(1.0, 0.001), &tone(2.6, 0.1)]);
    let partials: Vec<_> = events
        .iter()
        .filter(|e| e.kind == VoiceEventKind::Partial)
        .collect();
    assert_eq!(partials.len(), 2);
    assert!(partials[1].audio.len() > partials[0].audio.len());
    assert_eq!(partials[0].start_sample, partials[1].start_sample);

    // Cap splits long speech with zero lost samples
    let events = feed(
        &mut mk(8.0),
        &[&steady_noise(1.0, 0.001), &tone(11.0, 0.1), &steady_noise(2.0, 0.001)],
    );
    let finals: Vec<_> = events
        .iter()
        .filter(|e| e.kind == VoiceEventKind::Final)
        .collect();
    assert_eq!(finals.len(), 2);
    assert_eq!(finals[0].reason, "cap");
    assert_eq!(finals[1].reason, "silence");
    assert_eq!(finals[1].start_sample, finals[0].end_sample());

    // Calibration learns steady room noise above 0.015
    assert!(feed(&mut mk(15.0), &[&steady_noise(6.0, 0.05)]).is_empty());

    // Flush finalizes open utterance
    let mut seg = mk(15.0);
    feed(&mut seg, &[&steady_noise(1.0, 0.001), &tone(0.5, 0.1)]);
    let flushed = seg.flush();
    assert_eq!(flushed.len(), 1);
    assert_eq!(flushed[0].reason, "end");
}

// =================================================================================================
// 5. final_pass.rs & chunked final pass parity (test_final_pass.py, test_chunked_final_pass.py)
// =================================================================================================

struct FakeChunkModel {
    lengths: Mutex<Vec<f64>>,
}

impl SpeechModel for FakeChunkModel {
    fn recognize_text(&self, _audio: &[f32], _sr: usize) -> Result<String, String> {
        Ok(String::new())
    }
    fn recognize_with_timestamps(&self, audio: &[f32], sr: usize) -> Result<TimestampedResult, String> {
        let mut g = self.lengths.lock().unwrap();
        g.push(audio.len() as f64 / sr as f64);
        let idx = g.len();
        Ok(TimestampedResult {
            tokens: vec![format!(" chunk{idx}"), ".".into()],
            timestamps: vec![0.5, 0.6],
        })
    }
}

#[test]
fn test_final_pass_tokens_and_chunking_parity() {
    let tokens: Vec<String> = [" H", "ola", ",", " qué", " tal", ".", " This", " is", " a", " test", "."]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let times = vec![0.0, 0.24, 0.48, 0.56, 0.8, 1.04, 2.0, 2.24, 2.4, 2.56, 2.88];
    let segs = segments_from_tokens(&tokens, &times, 3.5, 0.8, 1.0);
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[0].text, "Hola, qué tal.");
    assert_eq!((segs[0].start, segs[0].end), (0.0, 2.0));
    assert_eq!(segs[1].text, "This is a test.");
    assert_eq!((segs[1].start, segs[1].end), (2.0, 3.5));

    // Closing mark after pause stays with its sentence
    let segs = segments_from_tokens(
        &[" en".into(), " el".into(), " momento".into(), ".".into()],
        &[0.0, 0.2, 0.4, 1.6],
        3.0,
        0.8,
        1.0,
    );
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].text, "en el momento.");

    // Opening mark after pause starts next sentence
    let segs = segments_from_tokens(
        &[" bien".into(), ".".into(), " ¿".into(), "Y".into(), " vos".into(), "?".into()],
        &[0.0, 0.2, 1.5, 1.6, 1.8, 2.0],
        3.0,
        0.8,
        1.0,
    );
    assert_eq!(segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), vec!["bien.", "¿Y vos?"]);

    // Quiet cut & chunked file pass
    let mut audio = tone(7.0, 0.2);
    audio.extend(vec![0.0_f32; 8_000]);
    audio.extend(tone(5.0, 0.2));
    let cut = find_quiet_cut(&audio, 10 * 16_000, 5 * 16_000, 1_600);
    assert!((7 * 16_000..=15 * 8_000).contains(&cut));
    assert!(loudest(&vec![0.0_f32; 16_000], 1_600) == 0.0);

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("audio.f32");
    let mut raw = Vec::new();
    for _ in 0..5 {
        for s in tone(4.0, 0.2) {
            raw.extend_from_slice(&s.to_le_bytes());
        }
        for _ in 0..16_000 {
            raw.extend_from_slice(&0.0_f32.to_le_bytes());
        }
    }
    fs::write(&path, &raw).unwrap();

    let model = FakeChunkModel {
        lengths: Mutex::new(Vec::new()),
    };
    let segs = run_final_pass_file(
        &model,
        &path,
        FinalPassOptions {
            sample_rate: 16_000,
            chunk_seconds: 8.0,
            search_seconds: 3.0,
            silence: 0.002,
        },
        |_, _| {},
        || false,
        None,
    )
    .unwrap();
    let lens = model.lengths.lock().unwrap();
    assert!(lens.len() > 1);
    assert!((lens.iter().sum::<f64>() - 25.0).abs() < 0.01);
    assert_eq!(segs[0].text, "chunk1.");
    assert_eq!(segs[1].text, "chunk2.");
}

// =================================================================================================
// 6. canary_pass.rs parity (test_canary_pass.py)
// =================================================================================================

struct FakeCanary {
    answers: Mutex<Vec<String>>,
    pieces: Mutex<Vec<(f64, String)>>,
}

impl FakeCanary {
    fn new(answers: &[&str]) -> Self {
        Self {
            answers: Mutex::new(answers.iter().map(|s| s.to_string()).collect()),
            pieces: Mutex::new(Vec::new()),
        }
    }
}

impl CanaryRecognizer for FakeCanary {
    fn recognize_lang(&self, audio: &[f32], sr: usize, lang: &str) -> Result<String, String> {
        self.pieces
            .lock()
            .unwrap()
            .push((audio.len() as f64 / sr as f64, lang.to_string()));
        let mut ans = self.answers.lock().unwrap();
        Ok(if ans.is_empty() {
            String::new()
        } else {
            ans.remove(0)
        })
    }
}

#[test]
fn test_canary_pass_parity() {
    assert_eq!(
        guess_language("Bueno, entonces la reunión es el jueves con todo el equipo de diseño"),
        Some("es")
    );
    assert_eq!(
        guess_language("So the meeting is on Thursday with the whole design team, right?"),
        Some("en")
    );
    assert_eq!(guess_language("OK."), None);
    assert_eq!(guess_language("Bonjour à tous, merci d'être venus ici"), None);

    assert_eq!(
        spaced("Son 1.200 dólares.¿Quién se encarga? Poco,¡ya!"),
        "Son 1.200 dólares. ¿Quién se encarga? Poco, ¡ya!"
    );

    let tokens: Vec<String> = [
        " Estoy", " grab", "ando", " la", " pantalla", " mientro,", " y", " el", " sistema", " que",
        " tenemos.",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let times = vec![0.4, 0.8, 0.88, 1.3, 1.5, 2.1, 2.9, 3.1, 3.3, 3.8, 4.0];

    let mut pass = CanaryPass::new(FakeCanary::new(&[
        "Estoy grabando la pantalla mientras hablo, y el sistema que tenemos.",
    ]));
    let (words, at) = pass.refine(&tone(6.0, 0.2), tokens, times, 16_000);
    assert_eq!(
        join_tokens(&words),
        "Estoy grabando la pantalla mientras hablo, y el sistema que tenemos."
    );
    let timed: HashMap<String, f64> = words
        .iter()
        .map(|w| w.trim().to_string())
        .zip(at.iter().copied())
        .collect();
    assert!((timed["Estoy"] - 0.4).abs() < 1e-6);
    assert!((timed["grabando"] - 0.8).abs() < 1e-6);
    assert!((timed["y"] - 2.9).abs() < 1e-6);
    assert!(timed["hablo,"] > 2.1 && timed["hablo,"] < 2.9);

    // Final pass with refiner hook
    struct OneWordParakeet;
    impl SpeechModel for OneWordParakeet {
        fn recognize_text(&self, _: &[f32], _: usize) -> Result<String, String> {
            Ok("mientro.".into())
        }
        fn recognize_with_timestamps(&self, _: &[f32], _: usize) -> Result<TimestampedResult, String> {
            Ok(TimestampedResult {
                tokens: vec![" mientro".into(), ".".into()],
                timestamps: vec![0.5, 0.9],
            })
        }
    }
    let mut refiner = |_a: &[f32], _t: Vec<String>, _ts: Vec<f64>, _sr: usize| {
        (vec![" mientras".into(), " hablo.".into()], vec![0.5, 0.9])
    };
    let segs = run_final_pass(&OneWordParakeet, &tone(2.0, 0.2), 16_000, Some(&mut refiner)).unwrap();
    assert_eq!(segs[0].text, "mientras hablo.");
}

// =================================================================================================
// 7. audio_sources.rs AudioTimeline & StartGate parity (test_audio_sources.py)
// =================================================================================================

#[test]
fn test_audio_timeline_and_start_gate_parity() {
    let rate = 1_000usize;
    let start = 5_000 * TICKS_PER_SECOND;
    let at = |ms: i64| start + ms * (TICKS_PER_SECOND / rate as i64);
    let sound = |ms: usize| vec![1u8; ms];
    let silence = |ms: usize| vec![0u8; ms];

    let mut tl = AudioTimeline::new(start, rate, 1, 0.06, 0.2);
    assert_eq!(tl.packet(&sound(10), at(0), false), sound(10));
    assert_eq!(tl.packet(&sound(10), at(13), false), sound(10)); // jitter within slack
    assert_eq!(tl.packet(&sound(10), at(18), false), sound(10));
    assert_eq!(tl.written, 30);
    assert_eq!(tl.corrections, 0);

    let mut tl2 = AudioTimeline::new(start, rate, 1, 0.06, 0.2);
    assert_eq!(tl2.idle(at(100)), silence(40));
    let mut expected = silence(55);
    expected.extend(sound(10));
    assert_eq!(tl2.packet(&sound(10), at(95), false), expected);
    assert_eq!(tl2.written, 105);
    assert_eq!(tl2.corrections, 0);

    // Pre-start audio trimmed
    let mut tl3 = AudioTimeline::new(start, rate, 1, 0.06, 0.2);
    assert!(tl3.packet(&sound(10), at(-30), false).is_empty());
    assert_eq!(tl3.packet(&sound(10), at(-5), false), sound(5));
    assert_eq!((tl3.written, tl3.corrections), (5, 0));

    // Drift beyond slack counted
    let mut tl4 = AudioTimeline::new(start, rate, 1, 0.06, 0.2);
    tl4.packet(&sound(10), at(0), false);
    let mut exp = silence(290);
    exp.extend(sound(10));
    assert_eq!(tl4.packet(&sound(10), at(300), false), exp);
    assert_eq!(tl4.corrections, 1);

    let gate = StartGate::new();
    assert_eq!(gate.start(), None);
    assert_eq!(gate.open(100, "first frame"), 100);
    assert_eq!(gate.open(50, "fallback"), 100);
    assert_eq!(gate.how(), "first frame");
}

// =================================================================================================
// 8. recorders.rs parity + NEW Audio-Only mode tests (test_recorders.py)
// =================================================================================================

#[test]
fn test_recorders_screen_and_audio_only_modes() {
    let dir = TempDir::new().unwrap();
    let rec = WindowsRecorder::new("ffmpeg".into());
    let mic = PipeInputSpec {
        ffmpeg_input: vec!["-f".into(), "f32le".into(), "-i".into(), r"\\.\pipe\mic".into()],
    };
    let comp = PipeInputSpec {
        ffmpeg_input: vec!["-f".into(), "f32le".into(), "-i".into(), r"\\.\pipe\comp".into()],
    };

    // Screen + Audio -> 2 separate processes (screen + audio)
    let procs = rec.processes(dir.path(), &RecorderSettings::default(), &[mic.clone(), comp.clone()]);
    assert_eq!(procs.len(), 2);
    assert_eq!(procs[0].name, "screen");
    assert!(!procs[0].speech);
    assert_eq!(procs[1].name, "audio");
    assert!(procs[1].speech);
    assert_eq!(procs[1].speech_channels, 2);
    assert!(procs[0].args.iter().any(|a| a.contains("gfxcapture=")));
    assert!(procs[1].args.iter().any(|a| a.contains("amix=")));

    // Audio Only -> 1 lightweight process (NO screen capture, outputs recording.m4a directly!)
    let audio_settings = RecorderSettings {
        mode: RecordMode::AudioOnly,
        ..Default::default()
    };
    let procs_audio = rec.processes(dir.path(), &audio_settings, &[mic.clone(), comp]);
    assert_eq!(procs_audio.len(), 1);
    assert_eq!(procs_audio[0].name, "audio");
    assert!(procs_audio[0].speech);
    assert!(procs_audio[0].args.iter().any(|a| a.ends_with("recording.m4a")));
    assert!(!procs_audio[0].args.iter().any(|a| a.contains("gfxcapture")));
    assert!(rec.finish(dir.path(), RecordMode::AudioOnly).is_ok());

    // First-frame detection
    assert!(WindowsRecorder::screen_started(
        "[Parsed_showinfo_3 @ 0000] [info] n:   0 pts:      0 pts_time:0"
    ));
    assert!(!WindowsRecorder::screen_started(
        "[Parsed_showinfo_3 @ 0000] [info] config in time_base: 1/10000000"
    ));

    // Microphone selection
    let devs = vec![
        AudioDevice { id: "{a}".into(), name: "Mic A".into() },
        AudioDevice { id: "{b}".into(), name: "Mic B".into() },
    ];
    assert_eq!(pick_microphone(&devs, Some("{a}"), Some("{b}")).unwrap().name, "Mic A");
    assert_eq!(pick_microphone(&devs, Some("Mic A"), Some("{b}")).unwrap().name, "Mic A");
    assert_eq!(pick_microphone(&devs, None, Some("{b}")).unwrap().name, "Mic B");
    assert_eq!(pick_microphone(&devs, Some("{gone}"), Some("{b}")).unwrap().name, "Mic B");
}

// =================================================================================================
// 9. library.rs SQLite + FTS5 + legacy migration parity (test_library.py)
// =================================================================================================

#[test]
fn test_library_fts5_and_legacy_migration() {
    assert_eq!(
        sanitize_fts_query(r#"hola (mundo) "x""#).as_deref(),
        Some(r#""hola"* "(mundo)"* """x"""*"#)
    );
    assert_eq!(sanitize_fts_query("   "), None);

    let dir = TempDir::new().unwrap();
    let lib = Library::open(dir.path().join("library.db")).unwrap();
    let id1 = lib
        .add(&NewRecording {
            title: "demo".into(),
            created_at: "2026-10-19T10:00:00".into(),
            duration_seconds: 60.0,
            size_bytes: 1000,
            folder: dir.path().to_string_lossy().into(),
            video_path: dir.path().join("recording.mp4").to_string_lossy().into(),
            transcript: "Después, precios: el plan anual".into(),
            segments: vec![TranscriptionSegment::new(0.0, 1.0, "Después, precios: el plan anual", Some("mic".into()))],
            labels: [("mic".into(), "Me".into()), ("computer".into(), "Others".into())].into(),
            kind: "recording".into(),
        })
        .unwrap();

    let found = lib.list(Some("prec"), 50).unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].snippet.contains("[precios]"));
    assert_eq!(lib.list(Some("despues pla"), 50).unwrap().len(), 1);
    assert!(lib.list(Some(r#"(unbalanced "quote"#), 50).unwrap().is_empty());

    lib.rename(&id1, "Sprint review").unwrap();
    assert_eq!(lib.list(Some("sprint"), 50).unwrap()[0].id, id1);

    lib.set_label(&id1, "computer", "Ana").unwrap();
    assert_eq!(lib.get(&id1).unwrap().unwrap().labels["computer"], "Ana");

    assert!(lib.delete(&id1).unwrap());
    assert!(lib.get(&id1).unwrap().is_none());

    // Legacy DB without labels_json and kind migrates cleanly
    let legacy_path = dir.path().join("legacy.db");
    {
        let conn = rusqlite::Connection::open(&legacy_path).unwrap();
        let old = SCHEMA
            .replace(",\n    labels_json TEXT DEFAULT '{}'", "")
            .replace(",\n    kind TEXT DEFAULT 'recording'", "");
        conn.execute_batch(&old).unwrap();
        conn.execute(
            "INSERT INTO recordings (id, title, created_at, folder, video_path, transcript, segments_json) \
             VALUES ('old', 'Old', '2026-10-01T10:00:00', 'f', 'v', 'hola', '[]')",
            [],
        )
        .unwrap();
    }
    let migrated = Library::open(&legacy_path).unwrap();
    let old_rec = migrated.get("old").unwrap().unwrap();
    assert_eq!(old_rec.kind, "recording");
    assert!(old_rec.labels.is_empty());
}

// =================================================================================================
// 10. export.rs (SRT, TXT, DOCX, PDF) & language.rs & subtitles.rs & importer.rs & settings.rs
// =================================================================================================

#[test]
fn test_exports_language_subtitles_importer_and_settings() {
    let dt = NaiveDate::from_ymd_opt(2026, 10, 5)
        .unwrap()
        .and_hms_opt(17, 42, 0)
        .unwrap();
    assert_eq!(default_title(dt, Some("en")), "Recording Oct 05, 17:42");
    assert_eq!(default_title(dt, Some("es")), "Grabación 5 oct, 17:42");
    assert_eq!(language_or_default(Some("fr")), "en");
    assert_eq!(default_labels(Some("es"))["computer"], "Otros");

    let dir = TempDir::new().unwrap();
    let segs = vec![
        TranscriptionSegment::new(0.0, 2.0, "Let's start {now}.", Some("mic".into())),
        TranscriptionSegment::new(2.5, 4.0, "Sure, acentos y ñ: ¿así?", Some("computer".into())),
    ];
    let names: HashMap<String, String> =
        [("mic".into(), "Me".into()), ("computer".into(), "Ana".into())].into();

    let (srt, txt) = write_text_files(&segs, dir.path(), Some(&names)).unwrap();
    assert!(fs::read_to_string(srt).unwrap().contains("Ana: Sure, acentos y ñ: ¿así?"));
    assert!(fs::read_to_string(txt).unwrap().starts_with("Me: Let's start {now}.\n"));

    // DOCX is a valid ZIP containing word/document.xml with exact text
    let docx_path = write_word(
        &segs,
        &dir.path().join("transcript.docx"),
        "Demo & <co>",
        dt,
        Some("es"),
        Some(&names),
        false,
    )
    .unwrap();
    let mut archive = zip::ZipArchive::new(fs::File::open(&docx_path).unwrap()).unwrap();
    let mut xml = String::new();
    archive.by_name("word/document.xml").unwrap().read_to_string(&mut xml).unwrap();
    assert!(xml.contains("Demo &amp; &lt;co&gt;"));
    assert!(xml.contains("Grabado el 05/10/2026 a las 17:42"));
    assert!(xml.contains("Transcripción"));
    assert!(xml.contains("[00:00:02 → 00:00:04]"));

    // PDF is valid, multi-page, and embeds Windows TrueType font (/FontFile2)
    let long: Vec<_> = (0..220)
        .map(|i| TranscriptionSegment::new(i as f64 * 5.0, i as f64 * 5.0 + 4.0, "Frase con acentos, ñ, ¿así? ¡Sí! → listo.", None))
        .collect();
    let pdf_path = write_pdf(
        &long,
        &dir.path().join("transcript.pdf"),
        "Grabación larga",
        dt,
        Some("es"),
        None,
        false,
    )
    .unwrap();
    let pdf_bytes = fs::read(&pdf_path).unwrap();
    assert!(pdf_bytes.starts_with(b"%PDF-"));
    assert!(pdf_bytes.windows(5).any(|w| w == b"%%EOF"));
    if cfg!(windows) {
        assert!(pdf_bytes.windows(10).any(|w| w == b"/FontFile2"));
    }

    // ASS subtitles
    let ass = ass_document(&segs, Some(&names));
    assert!(ass.contains(r"{\c&H00FFB48A&\b1}Ana:{\c&H00FFFFFF&\b0}"));
    assert!(ass.contains("(now)"));

    // Importer report parser
    let song = read_report(
        "Input #0, mov,mp4,m4a, from 'song.m4a':\n  Duration: 01:02:03.50, start: 0.0\n  Stream #0:0[0x1]: Audio: aac, 44100 Hz\n  Stream #0:1[0x0]: Video: png, 600x600 (attached pic)\n",
    )
    .unwrap();
    assert_eq!(song.duration, 3723.5);
    assert!(song.has_audio && !song.has_video);
    assert!(matches!(read_report("invalid"), Err(ImportError::Failed(_))));

    // Settings persistence (including new audio_only mode)
    let spath = dir.path().join("settings.json");
    let mut patch = serde_json::Map::new();
    patch.insert("theme".into(), serde_json::Value::String("midnight".into()));
    patch.insert("record_mode".into(), serde_json::Value::String("audio".into()));
    save_settings_patch(&spath, patch).unwrap();
    let loaded = load_settings(&spath);
    assert_eq!(loaded.theme, "midnight");
    assert_eq!(loaded.record_mode, RecordMode::AudioOnly);

    // Mel spectrogram sanity check
    let mel = NemoMelExtractor::new_128();
    let (feats, len) = mel.extract(&tone(1.0, 0.2));
    assert_eq!(feats.shape()[1], 128);
    assert!(len >= 95);
}

// =================================================================================================
// 11. downloads.rs resumable HTTP Range + SHA-256 parity (test_downloads.py)
// =================================================================================================

#[test]
fn test_downloads_resume_and_sha256_verification() {
    let dir = TempDir::new().unwrap();
    let payload: Vec<u8> = (0..180_000).map(|i| (i % 251) as u8).collect();
    let sha = {
        let mut h = Sha256::new();
        h.update(&payload);
        h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()
    };

    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let addr = server.server_addr().to_ip().unwrap();
    let payload_srv = payload.clone();
    let requests = Arc::new(AtomicUsize::new(0));
    let req_srv = Arc::clone(&requests);
    let srv_handle = Arc::clone(&server);

    std::thread::spawn(move || {
        for _ in 0..4 {
            let Ok(req) = srv_handle.recv() else { break };
            req_srv.fetch_add(1, Ordering::SeqCst);
            let range_start = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("Range"))
                .and_then(|h| {
                    let s = h.value.as_str();
                    s.strip_prefix("bytes=")
                        .and_then(|rest| rest.trim_end_matches('-').parse::<usize>().ok())
                })
                .unwrap_or(0);
            let slice = &payload_srv[range_start..];
            let status = if range_start > 0 { 206 } else { 200 };
            let resp = tiny_http::Response::from_data(slice.to_vec()).with_status_code(status);
            let _ = req.respond(resp);
        }
    });

    let files = vec![RemoteFile::new(
        "encoder.onnx",
        format!("http://{addr}/encoder.onnx"),
        payload.len() as u64,
        sha,
    )];

    // First attempt cancels after first chunk -> keeps .part
    let cancel_after = AtomicUsize::new(0);
    let err = download_with_chunk(
        dir.path(),
        &files,
        32 * 1024,
        |_, _| {
            cancel_after.fetch_add(1, Ordering::SeqCst);
        },
        || cancel_after.load(Ordering::SeqCst) >= 1,
        10,
    );
    assert!(matches!(err, Err(DownloadError::Cancelled)));
    assert!(dir.path().join("encoder.onnx.part").exists());
    assert!(!dir.path().join("encoder.onnx").exists());

    // Second attempt resumes from .part and passes SHA-256!
    download_with_chunk(dir.path(), &files, 32 * 1024, |_, _| {}, || false, 10).unwrap();
    assert_eq!(fs::read(dir.path().join("encoder.onnx")).unwrap(), payload);
    assert!(missing(dir.path(), &files).is_empty());
    assert_eq!(requests.load(Ordering::SeqCst), 2);
}

// =================================================================================================
// 12. Windows WASAPI + NamedPipe + Real FFmpeg integration tests
// =================================================================================================

#[cfg(windows)]
#[test]
fn test_windows_named_pipe_delivers_payload_and_wasapi_enumerates() {
    use cronista::engine::audio_sources::NamedPipeWriter;
    use cronista::engine::wasapi;
    use std::sync::atomic::AtomicBool;

    // 1. Named pipe roundtrip (1 MB > pipe buffer)
    let mut writer = NamedPipeWriter::new(1 << 18).unwrap();
    let pipe_path = writer.path.clone();
    let payload: Vec<u8> = (0u8..=255).cycle().take(1_048_576).collect();
    let reader_handle = std::thread::spawn(move || {
        let mut file = fs::File::open(&pipe_path).unwrap();
        let mut received = Vec::new();
        file.read_to_end(&mut received).unwrap();
        received
    });
    let stop = AtomicBool::new(false);
    assert!(writer.wait_for_reader(&stop).unwrap());
    writer.write(&payload, &stop).unwrap();
    writer.close();
    let received = reader_handle.join().unwrap();
    assert_eq!(received, payload);

    // 2. WASAPI device enumeration succeeds cleanly via native windows-rs COM
    let (devices, default_id) = wasapi::capture_devices().unwrap();
    for dev in &devices {
        assert!(!dev.id.is_empty());
        assert!(!dev.name.is_empty());
    }
    if let Some(def) = default_id {
        assert!(devices.iter().any(|d| d.id == def));
    }
}

#[test]
fn test_real_ffmpeg_probe_and_decode_audio() {
    let Some(ffmpeg) = cronista::engine::ffmpeg_locator::find_ffmpeg() else {
        return; // skip if ffmpeg absent
    };
    let dir = TempDir::new().unwrap();
    let wav = dir.path().join("sine.wav");
    let status = std::process::Command::new(&ffmpeg)
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2:sample_rate=44100",
            "-ac",
            "2",
        ])
        .arg(&wav)
        .status()
        .unwrap();
    assert!(status.success());

    let info = cronista::importer::probe(&ffmpeg, &wav).unwrap();
    assert!(info.has_audio && !info.has_video);
    assert!((info.duration - 2.0).abs() < 0.1);

    let target = dir.path().join("audio-16k.f32");
    let mut last_sec = 0.0;
    cronista::importer::decode_audio(&ffmpeg, &wav, &target, |s| last_sec = s, || false).unwrap();
    let sample_count = fs::metadata(&target).unwrap().len() / 4;
    assert!((sample_count as i64 - 32_000).abs() < 400);
    assert!((last_sec - 2.0).abs() < 0.1);
}
