//! Native Rust desktop GUI built with `egui` + `eframe` (glow OpenGL backend).
//! Handcrafted minimalist design matching Cronista's calm, modern studio aesthetic.

use crate::controller::{
    AppController, CanaryState, ImportProgress, ModelStatus, Phase, UiEvent,
};
use crate::engine::recorders::QUALITIES;
use crate::engine::stt::current_ram_mb;
use crate::engine::voices::speakers;
use crate::engine::wasapi::AudioDevice;
use crate::importer::MEDIA_EXTENSIONS;
use crate::language::default_labels;
use crate::library::{RecordingDetail, RecordingSummary};
use crate::settings::{RecordMode, THEMES};
use crate::ui::i18n::{strings, theme_name, tour_steps};
use crate::ui::icons::{paint_cronista_logo, paint_icon, Icon};
use crate::ui::theme::{apply_to_egui, palette_for, ThemePalette};
use crate::ui::widgets::{
    action_button, action_button_styled, dashed_import_tile, dot_matrix_loader, ghost_button,
    paint_card_specular, paint_dashed_rect, paint_scroll_fade, paint_studio_backdrop,
    record_button, speaker_pill, studio_waveform, toggle_pill, window_control_button,
};
use egui::{
    pos2, vec2, Align, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame,
    Layout, Margin, Rect, RichText, ScrollArea, Sense, Stroke, TextEdit, Vec2,
};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct LivePhrase {
    start: f64,
    text: String,
    source: String,
}

struct Toast {
    message: String,
    is_error: bool,
    until: Instant,
}

fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub struct CronistaApp {
    ctrl: Arc<AppController>,
    rx: Receiver<UiEvent>,

    lang: String,
    theme_id: String,
    system_dark: bool,
    phase: Phase,
    model_status: ModelStatus,
    canary_state: CanaryState,
    transcription: String,

    microphones: Vec<AudioDevice>,
    selected_mic: Option<String>,
    quality: String,
    system_audio: bool,
    record_mode: RecordMode,
    live_transcription: bool,
    auto_subtitles: bool,
    custom_labels: HashMap<String, String>,
    editing_chip: Option<String>,
    chip_draft: String,

    started_at: Option<f64>,
    live_phrases: Vec<LivePhrase>,
    live_partials: HashMap<String, (String, String)>,
    finishing_progress: Option<(f64, Option<u64>)>,

    search: String,
    last_search: String,
    items: Vec<RecordingSummary>,
    selected_id: Option<String>,
    detail: Option<RecordingDetail>,
    title_draft: String,
    editing_title: bool,
    editing_detail_label: Option<String>,
    detail_label_draft: String,
    confirm_delete_open: bool,

    importing: Option<ImportProgress>,
    import_lines: Vec<crate::engine::transcript::TranscriptionSegment>,

    subtitling: HashMap<String, (f64, Option<u64>)>,

    theme_menu_open: bool,
    tour_open: bool,
    tour_step: usize,
    banner: Option<String>,
    toast: Option<Toast>,
    ram_mb: f64,
    last_ram_check: Instant,
    allow_close: bool,
    deck_hovered: bool,
    smoothed_audio_level: f32,
}

impl CronistaApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, String> {
        load_windows_fonts(&cc.egui_ctx);

        let (tx, rx) = mpsc::channel();
        let ctx_clone = cc.egui_ctx.clone();
        let repaint = Arc::new(move || ctx_clone.request_repaint());
        let ctrl = AppController::new(tx, repaint)?;

        let settings = ctrl.load_settings();
        let (microphones, win_default) = ctrl.microphones();
        let selected_mic = ctrl.default_microphone(&microphones, win_default.as_deref());
        let lang = settings.language.unwrap_or_else(|| "es".to_string());
        let system_dark = cc
            .egui_ctx
            .system_theme()
            .map(|t| t == egui::Theme::Dark)
            .unwrap_or(true);

        let palette = palette_for(&settings.theme, system_dark);
        apply_to_egui(&cc.egui_ctx, &palette);

        let items = ctrl.recordings("");
        let banner = if !ctrl.recorder_error.is_empty() {
            Some(ctrl.recorder_error.clone())
        } else {
            None
        };

        let model_status = ctrl.initial_model_status();
        let canary_state = ctrl.canary_status();
        let transcription = ctrl.active_transcription_quality();

        Ok(Self {
            ctrl,
            rx,
            lang,
            theme_id: settings.theme,
            system_dark,
            phase: Phase::Idle,
            model_status,
            canary_state,
            transcription,
            microphones,
            selected_mic,
            quality: settings.quality,
            system_audio: settings.system_audio,
            record_mode: settings.record_mode,
            live_transcription: settings.live_transcription,
            auto_subtitles: settings.subtitles,
            custom_labels: settings.labels,
            editing_chip: None,
            chip_draft: String::new(),
            started_at: None,
            live_phrases: Vec::new(),
            live_partials: HashMap::new(),
            finishing_progress: None,
            search: String::new(),
            last_search: String::new(),
            items,
            selected_id: None,
            detail: None,
            title_draft: String::new(),
            editing_title: false,
            editing_detail_label: None,
            detail_label_draft: String::new(),
            confirm_delete_open: false,
            importing: None,
            import_lines: Vec::new(),
            subtitling: HashMap::new(),
            theme_menu_open: false,
            tour_open: !settings.tour_seen,
            tour_step: 0,
            banner,
            toast: None,
            ram_mb: current_ram_mb(),
            last_ram_check: Instant::now(),
            allow_close: false,
            deck_hovered: false,
            smoothed_audio_level: 0.0,
        })
    }

    fn show_toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some(Toast {
            message: msg.into(),
            is_error,
            until: Instant::now() + Duration::from_millis(if is_error { 4500 } else { 2600 }),
        });
    }

    fn current_labels(&self) -> HashMap<String, String> {
        let mut labels = default_labels(Some(&self.lang));
        for (k, v) in &self.custom_labels {
            labels.insert(k.clone(), v.clone());
        }
        labels
    }

    fn refresh_list(&mut self) {
        self.items = self.ctrl.recordings(&self.search);
    }

    fn select_recording(&mut self, id: &str) {
        if let Some(rec) = self.ctrl.recording(id) {
            self.selected_id = Some(id.to_string());
            self.title_draft = rec.title.clone();
            self.editing_title = false;
            self.detail = Some(rec);
            self.editing_detail_label = None;
        }
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.rx.try_recv() {
            let s = strings(&self.lang);
            match ev {
                UiEvent::Model(status) => {
                    if let ModelStatus::Error(ref msg) = status {
                        self.banner = Some(msg.clone());
                    } else if matches!(status, ModelStatus::Ready | ModelStatus::Sleeping)
                        && self.ctrl.recorder_error.is_empty()
                    {
                        self.banner = None;
                    }
                    self.model_status = status;
                }
                UiEvent::Phase(p) => {
                    if p == Phase::Idle {
                        self.started_at = None;
                        self.finishing_progress = None;
                    }
                    self.phase = p;
                }
                UiEvent::RecordingStarted { started_at } => {
                    self.started_at = Some(started_at);
                }
                UiEvent::Partial {
                    source,
                    committed,
                    pending,
                } => {
                    if committed.is_empty() && pending.is_empty() {
                        self.live_partials.remove(&source);
                    } else {
                        self.live_partials.insert(source, (committed, pending));
                    }
                }
                UiEvent::Phrase {
                    start,
                    text,
                    source,
                } => {
                    self.live_partials.remove(&source);
                    self.live_phrases.push(LivePhrase { start, text, source });
                }
                UiEvent::Finalizing { progress, eta } => {
                    self.phase = Phase::Finishing;
                    if let Some(p) = progress {
                        self.finishing_progress = Some((p, eta));
                    }
                }
                UiEvent::Saved { id } => {
                    self.finishing_progress = None;
                    self.refresh_list();
                    self.select_recording(&id);
                    self.show_toast(s.saved, false);
                }
                UiEvent::Canary {
                    state,
                    transcription,
                } => {
                    let was_dl = matches!(self.canary_state, CanaryState::Downloading { .. });
                    if let CanaryState::Error(ref msg) = state {
                        self.show_toast(msg.clone(), true);
                    } else if matches!(state, CanaryState::Ready) && was_dl && transcription == "high" {
                        self.show_toast(s.high_ready, false);
                    }
                    self.canary_state = state;
                    self.transcription = transcription;
                }
                UiEvent::Subtitling { id, progress, eta } => {
                    self.subtitling.insert(id, (progress, eta));
                }
                UiEvent::Subtitled { id, .. } => {
                    self.subtitling.remove(&id);
                    self.show_toast(s.subtitled_saved, false);
                }
                UiEvent::Import(prog) => {
                    let same_file = self
                        .importing
                        .as_ref()
                        .map(|i| i.name == prog.name)
                        .unwrap_or(false);
                    if !same_file {
                        self.import_lines.clear();
                    }
                    if !prog.new_lines.is_empty() {
                        self.import_lines.extend(prog.new_lines.iter().cloned());
                        while self.import_lines.len() > 6 {
                            self.import_lines.remove(0);
                        }
                        if let Some(cur) = self.importing.as_mut() {
                            cur.waiting = prog.waiting;
                        }
                    } else {
                        self.importing = Some(prog);
                    }
                }
                UiEvent::Imported { id, .. } => {
                    self.importing = None;
                    self.import_lines.clear();
                    self.refresh_list();
                    self.select_recording(&id);
                    self.show_toast(s.transcribed, false);
                }
                UiEvent::ImportCancelled { .. } => {
                    self.importing = None;
                    self.import_lines.clear();
                    self.show_toast(s.import_cancelled, false);
                }
                UiEvent::ImportError { name, message } => {
                    self.importing = None;
                    self.import_lines.clear();
                    let msg = if name.is_empty() {
                        message
                    } else {
                        format!("{name}: {message}")
                    };
                    self.show_toast(msg, true);
                }
                UiEvent::Notice(msg) => self.show_toast(msg, false),
                UiEvent::Error {
                    message,
                    recording_id,
                } => {
                    self.show_toast(message, true);
                    if let Some(id) = recording_id {
                        self.subtitling.remove(&id);
                    }
                }
                UiEvent::CloseApproved => {
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn draw_header(&mut self, ctx: &egui::Context, p: &ThemePalette) {
        let s = strings(&self.lang);
        let is_maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));

        egui::TopBottomPanel::top("header")
            .exact_height(52.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.surface).inner_margin(Margin::symmetric(16, 0)))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                // Edge-to-edge subtle bottom hairline (accounts for 16px horizontal margin)
                ui.painter().line_segment(
                    [
                        pos2(rect.left() - 16.0, rect.bottom()),
                        pos2(rect.right() + 16.0, rect.bottom()),
                    ],
                    Stroke::new(1.0_f32, p.border),
                );

                // Dragging empty header space moves the frameless window; double-click maximizes
                let drag_resp = ui.interact(rect, egui::Id::new("titlebar_drag"), Sense::click_and_drag());
                if drag_resp.drag_started_by(egui::PointerButton::Primary) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if drag_resp.double_clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
                }

                ui.horizontal_centered(|ui| {
                    // Brand mark + title
                    let (logo_r, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                    paint_cronista_logo(ui.painter(), logo_r.center(), 10.5, p);
                    ui.add_space(2.0);
                    ui.label(RichText::new("Cronista").font(semibold(15.0)).color(p.text));
                    ui.add_space(8.0);

                    // Exact-centered Status pill (26px height, aligned with logo & right controls)
                    let (dot_color, pill_bg, pill_fg, pill_text) = self.status_pill(p);
                    {
                        let font = FontId::proportional(12.0);
                        let galley = ui
                            .painter()
                            .layout_no_wrap(pill_text.clone(), font.clone(), pill_fg);
                        let sz = vec2(10.0 + 7.0 + 7.0 + galley.size().x + 11.0, 26.0);
                        let (pr, _) = ui.allocate_exact_size(sz, Sense::hover());
                        ui.painter().rect(
                            pr,
                            CornerRadius::same(13),
                            pill_bg,
                            Stroke::new(1.0_f32, p.border),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().circle_filled(pos2(pr.left() + 13.5, pr.center().y), 3.4, dot_color);
                        ui.painter().text(
                            pos2(pr.left() + 23.0, pr.center().y),
                            egui::Align2::LEFT_CENTER,
                            &pill_text,
                            font,
                            pill_fg,
                        );
                    }

                    // Exact-centered RAM pill (click to sleep model when loaded)
                    if self.ram_mb > 0.0 {
                        let loaded = self.ctrl.is_model_loaded();
                        let ram_text = format!("{:.0} MB", self.ram_mb);
                        let font = FontId::proportional(11.5);
                        let col = if loaded { p.warn } else { p.text3 };
                        let galley = ui.painter().layout_no_wrap(ram_text.clone(), font.clone(), col);
                        let sz = vec2(galley.size().x + 20.0, 26.0);
                        let (rr, resp) = ui.allocate_exact_size(sz, Sense::click());
                        let resp = if loaded {
                            resp.on_hover_cursor(egui::CursorIcon::PointingHand)
                        } else {
                            resp
                        }
                        .on_hover_text(if loaded { s.sleep_tip } else { s.sleeping });
                        ui.painter().rect(
                            rr,
                            CornerRadius::same(13),
                            if resp.hovered() && loaded { p.surface2 } else { Color32::TRANSPARENT },
                            Stroke::new(1.0_f32, p.border),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            rr.center(),
                            egui::Align2::CENTER_CENTER,
                            &ram_text,
                            font,
                            col,
                        );
                        if resp.clicked() && loaded && self.phase == Phase::Idle && self.importing.is_none() {
                            self.ctrl.sleep_model_now();
                        }
                    }

                    // Right-side modern internal window controls + ghost nav + EN/ES
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;

                        // Modern internal Close / Maximize / Minimize buttons
                        if window_control_button(ui, Icon::WinClose, p, true).clicked() {
                            if self.ctrl.request_close() {
                                self.allow_close = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                        if window_control_button(
                            ui,
                            if is_maximized { Icon::WinRestore } else { Icon::WinMaximize },
                            p,
                            false,
                        )
                        .clicked()
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
                        }
                        if window_control_button(ui, Icon::WinMinimize, p, false).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }

                        // Subtle vertical hairline between app actions and window controls
                        ui.add_space(4.0);
                        let (div_r, _) = ui.allocate_exact_size(vec2(1.0, 18.0), Sense::hover());
                        ui.painter().line_segment(
                            [div_r.center_top(), div_r.center_bottom()],
                            Stroke::new(1.0_f32, p.border_strong),
                        );
                        ui.add_space(4.0);

                        if ghost_button(ui, Some(Icon::Folder), s.open_folder, p, false).clicked() {
                            self.ctrl.open_recordings_folder();
                        }
                        if ghost_button(ui, Some(Icon::Help), s.help, p, false).clicked() {
                            self.tour_step = 0;
                            self.tour_open = true;
                        }
                        if ghost_button(ui, Some(Icon::Palette), s.theme, p, false).clicked() {
                            self.theme_menu_open = !self.theme_menu_open;
                        }
                        ui.add_space(6.0);

                        // Exact-centered EN / ES segmented pill (30px height, perfectly aligned!)
                        let (seg_r, seg_resp) = ui.allocate_exact_size(vec2(68.0, 30.0), Sense::click());
                        let seg_resp = seg_resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                        ui.painter().rect(
                            seg_r,
                            CornerRadius::same(8),
                            p.surface2,
                            Stroke::new(1.0_f32, p.border),
                            egui::StrokeKind::Inside,
                        );
                        let half_w = (seg_r.width() - 6.0) * 0.5;
                        let en_r = Rect::from_min_size(pos2(seg_r.left() + 3.0, seg_r.top() + 3.0), vec2(half_w, 24.0));
                        let es_r = Rect::from_min_size(pos2(en_r.right(), seg_r.top() + 3.0), vec2(half_w, 24.0));
                        for (code, r) in [("en", en_r), ("es", es_r)] {
                            let active = self.lang == code;
                            if active {
                                ui.painter().rect_filled(r, CornerRadius::same(6), p.surface3);
                            }
                            ui.painter().text(
                                r.center(),
                                egui::Align2::CENTER_CENTER,
                                code.to_uppercase(),
                                semibold(11.5),
                                if active { p.text } else { p.text3 },
                            );
                        }
                        if seg_resp.clicked() {
                            if let Some(pos) = seg_resp.interact_pointer_pos() {
                                let chosen = if pos.x < seg_r.center().x { "en" } else { "es" };
                                if self.lang != chosen {
                                    self.lang = chosen.to_string();
                                    self.ctrl.save_preferences(
                                        Some(chosen), None, None, None, None, None, None,
                                    );
                                }
                            }
                        }
                    });
                });
            });
    }

    fn status_pill(&self, p: &ThemePalette) -> (Color32, Color32, Color32, String) {
        let s = strings(&self.lang);
        if !self.ctrl.recorder_error.is_empty() {
            return (p.danger, p.danger_soft, p.danger, s.cannot_record.into());
        }
        match self.phase {
            Phase::Recording => (
                p.rec,
                p.rec_soft,
                p.rec_text,
                format!("{} {}", s.recording, format_clock(self.elapsed_secs())),
            ),
            Phase::Listening => (p.rec, p.rec_soft, p.rec_text, s.starting.into()),
            Phase::Finishing => {
                let txt = match self.finishing_progress {
                    Some((pct, _)) => format!("{} {}%", s.finishing, (pct * 100.0) as u32),
                    None => s.finishing.into(),
                };
                (p.warn, p.surface2, p.text2, txt)
            }
            Phase::Idle => {
                if let Some(imp) = &self.importing {
                    let pct = (imp.progress * 100.0).round() as u32;
                    return (p.warn, p.surface2, p.text2, format!("{} {pct}%", s.transcription));
                }
                match &self.model_status {
                    ModelStatus::Loading => (p.warn, p.surface2, p.text2, s.loading.into()),
                    ModelStatus::Downloading { done, total } => {
                        let pct = if *total > 0 {
                            (*done as f64 / *total as f64 * 100.0) as u32
                        } else {
                            0
                        };
                        (p.warn, p.surface2, p.text2, format!("{} {pct}%", s.setup_downloading))
                    }
                    ModelStatus::Error(_) => (p.danger, p.danger_soft, p.danger, s.model_error.into()),
                    ModelStatus::Missing { .. } => (p.warn, p.surface2, p.text2, s.model_missing.into()),
                    ModelStatus::Sleeping => (p.ok, p.surface2, p.text2, s.ready.into()),
                    ModelStatus::Ready => (p.ok, p.surface2, p.text2, s.ready.into()),
                }
            }
        }
    }

    fn draw_sidebar(&mut self, ctx: &egui::Context, p: &ThemePalette) {
        let s = strings(&self.lang);
        egui::SidePanel::left("history_sidebar")
            .exact_width(304.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.surface).inner_margin(Margin::symmetric(14, 14)))
            .show(ctx, |ui| {
                // Crisp outer panel divider placed at the exact right edge (outside the 14px margin)
                let full = ui.max_rect();
                let edge_x = full.right() + 13.5;
                ui.painter().line_segment(
                    [pos2(edge_x, full.top() - 14.0), pos2(edge_x, full.bottom() + 14.0)],
                    Stroke::new(1.0_f32, p.border),
                );

                // Recessed search bar with vector magnifying glass
                Frame::new()
                    .fill(p.bg)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(10, 7))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (ic_r, _) = ui.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
                            paint_icon(ui.painter(), ic_r, Icon::Search, p.text3);
                            ui.add(
                                TextEdit::singleline(&mut self.search)
                                    .hint_text(RichText::new(s.search).color(p.text3))
                                    .frame(false)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                    });
                if self.search != self.last_search {
                    self.last_search = self.search.clone();
                    self.refresh_list();
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(s.history.to_uppercase())
                            .font(semibold(10.5))
                            .color(p.text3),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(4.0);
                        let count_label = if !self.search.trim().is_empty() {
                            if s.is_es {
                                format!("{} RESULTADOS", self.items.len())
                            } else {
                                format!("{} MATCHES", self.items.len())
                            }
                        } else if !self.items.is_empty() {
                            if s.is_es {
                                format!("{} GRABACIONES", self.items.len())
                            } else {
                                format!("{} RECORDINGS", self.items.len())
                            }
                        } else {
                            String::new()
                        };
                        ui.label(
                            RichText::new(count_label)
                                .font(semibold(10.5))
                                .color(p.text3),
                        );
                    });
                });
                ui.add_space(8.0);

                // Scrollable history list (leaves generous room for footer drop tile)
                let list_h = (ui.available_height() - 78.0).max(90.0);
                let sb_scroll = ScrollArea::vertical()
                    .max_height(list_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(imp) = &self.importing {
                            let pct = (imp.progress * 100.0).round() as u32;
                            let card = Frame::new()
                                .fill(p.surface2)
                                .stroke(Stroke::new(1.0_f32, p.accent.gamma_multiply(0.45)))
                                .corner_radius(CornerRadius::same(11))
                                .inner_margin(Margin::symmetric(12, 10))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(
                                        RichText::new(&imp.name)
                                            .font(semibold(13.0))
                                            .color(p.text),
                                    );
                                    ui.label(
                                        RichText::new(format!("{} · {pct}%", s.transcription))
                                            .size(11.5)
                                            .color(p.accent_text),
                                    );
                                    ui.add_space(5.0);
                                    ui.add(
                                        egui::ProgressBar::new(imp.progress as f32)
                                            .desired_height(3.0),
                                    );
                                });
                            paint_card_specular(ui.painter(), card.response.rect, 11.0, p);
                            ui.add_space(6.0);
                        }

                        if self.items.is_empty() {
                            ui.add_space(12.0);
                            Frame::new()
                                .fill(p.bg)
                                .stroke(Stroke::new(1.0_f32, p.border))
                                .corner_radius(CornerRadius::same(11))
                                .inner_margin(Margin::symmetric(14, 14))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(
                                        RichText::new(if self.search.trim().is_empty() {
                                            s.no_recordings
                                        } else {
                                            s.nothing_matches
                                        })
                                        .size(12.5)
                                        .color(p.text3),
                                    );
                                });
                        }

                        let mut clicked_id = None;
                        for item in &self.items {
                            let selected = self.selected_id.as_deref() == Some(item.id.as_str());
                            let is_audio_only = item.kind == "audio_recording" || item.kind == "audio";
                            let resp = Frame::new()
                                .fill(if selected { p.surface2 } else { p.bg.gamma_multiply(0.55) })
                                .stroke(Stroke::new(
                                    1.0_f32,
                                    if selected { p.border_strong } else { p.border },
                                ))
                                .corner_radius(CornerRadius::same(11))
                                .inner_margin(Margin::symmetric(11, 10))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        // Crisp media-type icon badge
                                        let (badge_r, _) =
                                            ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                                        let (b_bg, b_fg, b_ic) = if is_audio_only {
                                            (p.computer_bg, p.computer, Icon::Waveform)
                                        } else {
                                            (p.accent_soft, p.accent_text, Icon::Monitor)
                                        };
                                        ui.painter().rect_filled(badge_r, CornerRadius::same(8), b_bg);
                                        paint_icon(
                                            ui.painter(),
                                            Rect::from_center_size(badge_r.center(), Vec2::splat(14.0)),
                                            b_ic,
                                            b_fg,
                                        );
                                        ui.add_space(4.0);
                                        ui.vertical(|ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&item.title)
                                                        .font(semibold(13.0))
                                                        .color(p.text),
                                                )
                                                .truncate(),
                                            );
                                            ui.add_space(2.0);
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 5.0;
                                                Frame::new()
                                                    .fill(p.surface3)
                                                    .corner_radius(CornerRadius::same(4))
                                                    .inner_margin(Margin::symmetric(5, 1))
                                                    .show(ui, |ui| {
                                                        ui.label(
                                                            RichText::new(format_clock(item.duration_seconds))
                                                                .font(FontId::monospace(10.5))
                                                                .color(p.text2),
                                                        );
                                                    });
                                                ui.label(
                                                    RichText::new(format!(
                                                        "{} · {}",
                                                        short_date(&item.created_at),
                                                        format_size(item.size_bytes)
                                                    ))
                                                    .size(11.0)
                                                    .color(p.text3),
                                                );
                                            });
                                        });
                                    });
                                    ui.add_space(6.0);
                                    let snip = if item.snippet.trim().is_empty() {
                                        s.no_speech
                                    } else {
                                        item.snippet.as_str()
                                    };
                                    render_snippet(ui, snip, p);
                                });

                            let r = resp.response.rect;
                            if selected {
                                paint_card_specular(ui.painter(), r, 11.0, p);
                                let bar = Rect::from_min_max(
                                    pos2(r.left() + 1.0, r.top() + 10.0),
                                    pos2(r.left() + 3.6, r.bottom() - 10.0),
                                );
                                ui.painter().rect_filled(bar, CornerRadius::same(2), p.accent);
                            }

                            let click = resp
                                .response
                                .interact(Sense::click())
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if click.hovered() && !selected {
                                ui.painter().rect_stroke(
                                    r,
                                    CornerRadius::same(11),
                                    Stroke::new(1.0_f32, p.border_strong),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            if click.clicked() {
                                clicked_id = Some(item.id.clone());
                            }
                            ui.add_space(6.0);
                        }
                        if let Some(id) = clicked_id {
                            self.select_recording(&id);
                        }
                    });
                if sb_scroll.state.offset.y > 2.0 {
                    paint_scroll_fade(ui.painter(), sb_scroll.inner_rect, p.surface, 20.0, true);
                }
                if sb_scroll.state.offset.y + sb_scroll.inner_rect.height()
                    < sb_scroll.content_size.y - 2.0
                {
                    paint_scroll_fade(ui.painter(), sb_scroll.inner_rect, p.surface, 24.0, false);
                }

                ui.add_space(10.0);
                let busy = self.phase != Phase::Idle;
                if dashed_import_tile(ui, s.import_title, s.import_hint, !busy, p).clicked() {
                    if let Some(files) = rfd::FileDialog::new()
                        .add_filter("Audio & Video", MEDIA_EXTENSIONS)
                        .pick_files()
                    {
                        if let Err(err) = self.ctrl.import_files(files) {
                            self.show_toast(err, true);
                        }
                    }
                }
            });
    }

    fn draw_main(&mut self, ctx: &egui::Context, p: &ThemePalette) {
        let time = ctx.input(|i| i.time);
        let is_rec = matches!(self.phase, Phase::Recording | Phase::Listening);
        egui::CentralPanel::default()
            .frame(Frame::new().fill(p.bg).inner_margin(Margin::symmetric(28, 16)))
            .show(ctx, |ui| {
                // Architectural dot-grid + ambient studio aura behind the workspace
                paint_studio_backdrop(ui.painter(), ui.max_rect().expand(28.0), p, is_rec, time);

                let main_scroll = ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let col_w = ui.available_width().min(860.0);
                    ui.vertical_centered(|ui| {
                        ui.set_max_width(col_w);
                        ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                            if let Some(banner) = &self.banner {
                                Frame::new()
                                    .fill(p.danger_soft)
                                    .stroke(Stroke::new(1.0_f32, p.danger.gamma_multiply(0.4)))
                                    .corner_radius(CornerRadius::same(10))
                                    .inner_margin(Margin::symmetric(14, 10))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.label(RichText::new(banner).size(13.0).color(p.danger));
                                    });
                                ui.add_space(14.0);
                            }

                            // Minimalist 1-click setup card when Parakeet / ONNX Runtime is not installed yet
                            match self.model_status.clone() {
                                ModelStatus::Missing { .. } => {
                                    let s = strings(&self.lang);
                                    Frame::new()
                                        .fill(p.surface)
                                        .stroke(Stroke::new(1.0_f32, p.border_strong))
                                        .corner_radius(CornerRadius::same(12))
                                        .inner_margin(Margin::symmetric(18, 14))
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.horizontal(|ui| {
                                                ui.vertical(|ui| {
                                                    ui.label(
                                                        RichText::new(s.setup_title)
                                                            .font(semibold(13.5))
                                                            .color(p.text),
                                                    );
                                                    ui.add_space(2.0);
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(s.setup_body)
                                                                .size(12.0)
                                                                .color(p.text2),
                                                        )
                                                        .wrap(),
                                                    );
                                                });
                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        if action_button(
                                                            ui,
                                                            Icon::Download,
                                                            s.setup_download,
                                                            true,
                                                            None,
                                                            p,
                                                        )
                                                        .clicked()
                                                        {
                                                            self.ctrl.download_parakeet_and_runtime();
                                                        }
                                                    },
                                                );
                                            });
                                        });
                                    ui.add_space(14.0);
                                }
                                ModelStatus::Downloading { done, total } => {
                                    let s = strings(&self.lang);
                                    let pct = if total > 0 {
                                        (done as f32 / total as f32).clamp(0.0, 1.0)
                                    } else {
                                        0.0
                                    };
                                    Frame::new()
                                        .fill(p.surface)
                                        .stroke(Stroke::new(1.0_f32, p.border_strong))
                                        .corner_radius(CornerRadius::same(12))
                                        .inner_margin(Margin::symmetric(18, 14))
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new(format!(
                                                        "{} · {}% ({:.0} / {:.0} MB)",
                                                        s.setup_downloading,
                                                        (pct * 100.0) as u32,
                                                        done as f64 / (1024.0 * 1024.0),
                                                        total as f64 / (1024.0 * 1024.0)
                                                    ))
                                                    .font(semibold(13.0))
                                                    .color(p.text),
                                                );
                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        if ghost_button(ui, None, s.cancel, p, false)
                                                            .clicked()
                                                        {
                                                            self.ctrl.cancel_parakeet_download();
                                                        }
                                                    },
                                                );
                                            });
                                            ui.add_space(8.0);
                                            ui.add(
                                                egui::ProgressBar::new(pct).desired_height(4.0),
                                            );
                                        });
                                    ui.add_space(14.0);
                                }
                                _ => {}
                            }

                            self.draw_recorder_card(ui, p);
                            ui.add_space(12.0);

                            if self.importing.is_some() && self.phase == Phase::Idle {
                                self.draw_importing_card(ui, p);
                                ui.add_space(12.0);
                            }

                            if self.phase != Phase::Idle {
                                self.draw_live_card(ui, p);
                            } else if self.detail.is_some() {
                                self.draw_detail_card(ui, p);
                            } else if self.importing.is_none() {
                                self.draw_empty_state(ui, p);
                            }
                        });
                    });
                });
                if main_scroll.state.offset.y > 2.0 {
                    paint_scroll_fade(ui.painter(), main_scroll.inner_rect, p.bg, 22.0, true);
                }
                if main_scroll.state.offset.y + main_scroll.inner_rect.height()
                    < main_scroll.content_size.y - 2.0
                {
                    paint_scroll_fade(ui.painter(), main_scroll.inner_rect, p.bg, 28.0, false);
                }
            });
    }

    fn draw_recorder_card(&mut self, ui: &mut egui::Ui, p: &ThemePalette) {
        let s = strings(&self.lang);
        let busy = self.phase != Phase::Idle;
        let time = ui.input(|i| i.time);
        let active = matches!(self.phase, Phase::Listening | Phase::Recording);

        let deck_stroke = if active {
            Stroke::new(
                1.0_f32,
                Color32::from_rgba_unmultiplied(p.rec.r(), p.rec.g(), p.rec.b(), 95),
            )
        } else {
            Stroke::new(1.0_f32, p.border_strong)
        };

        let deck = Frame::new()
            .fill(p.surface)
            .stroke(deck_stroke)
            .corner_radius(CornerRadius::same(18))
            .inner_margin(Margin::symmetric(20, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                // ── Top Studio Deck Header: Icon Mode Switcher (left) + Live Telemetry Pill (right)
                ui.horizontal(|ui| {
                    Frame::new()
                        .fill(p.bg)
                        .stroke(Stroke::new(1.0_f32, p.border))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::same(3))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 3.0;
                                for (mode, icon, label) in [
                                    (RecordMode::ScreenAndAudio, Icon::Monitor, s.mode_screen),
                                    (RecordMode::AudioOnly, Icon::Waveform, s.mode_audio),
                                ] {
                                    let is_sel = self.record_mode == mode;
                                    let font = semibold(12.5);
                                    let galley = ui.painter().layout_no_wrap(
                                        label.to_string(),
                                        font.clone(),
                                        p.text,
                                    );
                                    let sz = vec2(15.0 + 7.0 + galley.size().x + 22.0, 28.0);
                                    let (rect, resp) = ui.allocate_exact_size(
                                        sz,
                                        if busy { Sense::hover() } else { Sense::click() },
                                    );
                                    let resp = if !busy {
                                        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
                                    } else {
                                        resp
                                    };
                                    let hov = !busy && resp.hovered();
                                    let fill = if is_sel {
                                        p.surface3
                                    } else if hov {
                                        p.surface2
                                    } else {
                                        Color32::TRANSPARENT
                                    };
                                    let stroke = if is_sel {
                                        Stroke::new(1.0_f32, p.border_strong)
                                    } else {
                                        Stroke::NONE
                                    };
                                    ui.painter().rect(
                                        rect,
                                        CornerRadius::same(7),
                                        fill,
                                        stroke,
                                        egui::StrokeKind::Inside,
                                    );
                                    let ic_col = if is_sel {
                                        p.accent_text
                                    } else if hov {
                                        p.text2
                                    } else {
                                        p.text3
                                    };
                                    let txt_col = if is_sel {
                                        p.text
                                    } else if hov {
                                        p.text2
                                    } else {
                                        p.text3
                                    };
                                    paint_icon(
                                        ui.painter(),
                                        Rect::from_center_size(
                                            pos2(rect.left() + 18.0, rect.center().y),
                                            Vec2::splat(14.0),
                                        ),
                                        icon,
                                        ic_col,
                                    );
                                    ui.painter().text(
                                        pos2(rect.left() + 31.0, rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        label,
                                        font,
                                        txt_col,
                                    );
                                    if !busy && resp.clicked() && !is_sel {
                                        self.record_mode = mode;
                                        self.ctrl.save_preferences(
                                            None, None, None, None, None, Some(mode), None,
                                        );
                                    }
                                }
                            });
                        });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (badge_dot, badge_bg, badge_fg, badge_txt) = match self.phase {
                            Phase::Recording => (
                                p.rec,
                                p.rec_soft,
                                p.rec_text,
                                if s.is_es { "EN GRABACIÓN" } else { "ON AIR · REC" },
                            ),
                            Phase::Listening => (
                                p.warn,
                                p.surface2,
                                p.warn,
                                if s.is_es { "INICIANDO" } else { "ARMING" },
                            ),
                            Phase::Finishing => (
                                p.warn,
                                p.surface2,
                                p.warn,
                                if s.is_es { "PROCESANDO" } else { "FINALIZING" },
                            ),
                            Phase::Idle => {
                                if self.importing.is_some() {
                                    (
                                        p.accent,
                                        p.accent_soft,
                                        p.accent_text,
                                        if s.is_es { "TRANSCRIBIENDO" } else { "TRANSCRIBING" },
                                    )
                                } else {
                                    (
                                        p.ok,
                                        p.bg,
                                        p.text3,
                                        if s.is_es { "ESTUDIO LOCAL" } else { "LOCAL STUDIO" },
                                    )
                                }
                            }
                        };
                        Frame::new()
                            .fill(badge_bg)
                            .stroke(Stroke::new(1.0_f32, p.border))
                            .corner_radius(CornerRadius::same(99))
                            .inner_margin(Margin::symmetric(10, 4))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    let (dr, _) =
                                        ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                                    ui.painter().circle_filled(dr.center(), 3.2, badge_dot);
                                    ui.label(
                                        RichText::new(badge_txt)
                                            .font(FontId::monospace(10.5))
                                            .color(badge_fg),
                                    );
                                });
                            });
                    });
                });

                ui.add_space(12.0);

                // ── Center Hero Stage: Machined Record Trigger + Studio Timecode + Acoustic Waveform
                let hero_avail = ui.available_width();
                let show_wave = hero_avail >= 560.0;
                ui.horizontal(|ui| {
                    let can_start = self.phase == Phase::Idle
                        && self.importing.is_none()
                        && self.ctrl.recorder_error.is_empty()
                        && !matches!(self.model_status, ModelStatus::Loading);
                    let can_stop = matches!(self.phase, Phase::Listening | Phase::Recording);
                    let enabled = can_start || can_stop;

                    if record_button(ui, active, enabled, self.smoothed_audio_level, time, p).clicked()
                        && enabled
                    {
                        if self.phase == Phase::Idle {
                            self.live_phrases.clear();
                            self.live_partials.clear();
                            if let Err(err) = self.ctrl.start_recording(
                                self.selected_mic.clone(),
                                self.quality.clone(),
                                self.system_audio,
                                self.record_mode,
                                self.live_transcription,
                                Some(&self.lang),
                                self.current_labels(),
                                self.auto_subtitles,
                            ) {
                                self.show_toast(err, true);
                            }
                        } else {
                            self.ctrl.stop_recording();
                        }
                    }

                    ui.add_space(18.0);
                    let text_max_w = if show_wave {
                        (hero_avail - 94.0 - 18.0 - 216.0).max(200.0)
                    } else {
                        (hero_avail - 94.0 - 24.0).max(200.0)
                    };
                    ui.allocate_ui_with_layout(
                        vec2(text_max_w, 94.0),
                        Layout::top_down(Align::LEFT),
                        |ui| {
                            ui.add_space(10.0);
                            let timer_str = if self.phase == Phase::Recording {
                                format_clock(self.elapsed_secs())
                            } else {
                                "00:00".to_string()
                            };
                            ui.label(
                                RichText::new(timer_str)
                                    .font(semibold(48.0))
                                    .color(p.text),
                            );
                            ui.add_space(2.0);
                            let hint = match self.phase {
                                Phase::Idle => {
                                    if self.importing.is_some() {
                                        s.hint_importing.to_string()
                                    } else {
                                        match (self.record_mode, self.system_audio) {
                                            (RecordMode::ScreenAndAudio, true) => {
                                                s.hint_idle_screen_computer.into()
                                            }
                                            (RecordMode::ScreenAndAudio, false) => {
                                                s.hint_idle_screen.into()
                                            }
                                            (RecordMode::AudioOnly, true) => {
                                                s.hint_idle_audio_computer.into()
                                            }
                                            (RecordMode::AudioOnly, false) => {
                                                s.hint_idle_audio.into()
                                            }
                                        }
                                    }
                                }
                                Phase::Listening => s.hint_starting.into(),
                                Phase::Recording => s.hint_recording.into(),
                                Phase::Finishing => match self.finishing_progress {
                                    Some((pct, Some(eta))) => {
                                        format!(
                                            "{} · {}% · ~{}s",
                                            s.hint_finishing,
                                            (pct * 100.0) as u32,
                                            eta
                                        )
                                    }
                                    Some((pct, None)) => {
                                        format!("{} · {}%", s.hint_finishing, (pct * 100.0) as u32)
                                    }
                                    None => s.hint_finishing.into(),
                                },
                            };
                            ui.add(
                                egui::Label::new(RichText::new(hint).size(13.0).color(p.text2))
                                    .wrap(),
                            );
                        },
                    );

                    // Right-aligned live organic center-outward LED matrix (wakes on hover & voice!)
                    if show_wave {
                        let hover_mix = ui.ctx().animate_bool_with_time(
                            egui::Id::new("deck_matrix_hover"),
                            self.deck_hovered,
                            0.45,
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_space(4.0);
                            studio_waveform(
                                ui,
                                active,
                                self.importing.is_some(),
                                hover_mix,
                                self.smoothed_audio_level,
                                time,
                                p,
                            );
                        });
                    }
                });

                ui.add_space(12.0);

                // ── Bottom Recessed Studio Console Dock (Capture + Intelligence controls)
                Frame::new()
                    .fill(p.bg)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        // Console Row 1: Microphone + Video Quality + Computer Audio
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);

                            let current_mic_name = self
                                .selected_mic
                                .as_deref()
                                .and_then(|id| self.microphones.iter().find(|m| m.id == id))
                                .map(|m| m.name.as_str())
                                .unwrap_or(s.no_mic);

                            ui.add_enabled_ui(!busy, |ui| {
                                let cb = egui::ComboBox::from_id_salt("mic_select")
                                    .width(235.0)
                                    .selected_text(RichText::new(current_mic_name).size(12.5))
                                    .show_ui(ui, |ui| {
                                        for m in &self.microphones {
                                            let selected =
                                                self.selected_mic.as_deref() == Some(&m.id);
                                            if ui
                                                .selectable_label(selected, &m.name)
                                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                .clicked()
                                            {
                                                self.selected_mic = Some(m.id.clone());
                                            }
                                        }
                                    });
                                if !busy && cb.response.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                            });

                            if self.record_mode == RecordMode::ScreenAndAudio {
                                let q_label = |q: &str| match q {
                                    "best" => s.quality_best,
                                    "smallest" => s.quality_smallest,
                                    _ => s.quality_balanced,
                                };
                                ui.add_enabled_ui(!busy, |ui| {
                                    let cb = egui::ComboBox::from_id_salt("quality_select")
                                        .width(116.0)
                                        .selected_text(
                                            RichText::new(q_label(&self.quality)).size(12.5),
                                        )
                                        .show_ui(ui, |ui| {
                                            for &q in QUALITIES {
                                                if ui
                                                    .selectable_label(self.quality == q, q_label(q))
                                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                    .clicked()
                                                {
                                                    self.quality = q.to_string();
                                                }
                                            }
                                        });
                                    if !busy && cb.response.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                });
                            }

                            toggle_pill(ui, &mut self.system_audio, s.computer, !busy, false, p);
                        });

                        ui.add_space(8.0);

                        // Subtle horizontal hairline separating capture & transcription controls
                        let sep = ui.available_rect_before_wrap();
                        ui.painter().line_segment(
                            [pos2(sep.left(), sep.top()), pos2(sep.right(), sep.top())],
                            Stroke::new(1.0_f32, p.border),
                        );
                        ui.add_space(8.0);

                        // Console Row 2: Speaker labels + STT Quality + Live + Subtitles
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                            ui.label(RichText::new(s.voices).size(12.0).color(p.text3));
                            let labels = self.current_labels();
                            for (src, fg, bg) in [
                                ("mic", p.mic, p.mic_bg),
                                ("computer", p.computer, p.computer_bg),
                            ] {
                                if src == "computer" && !self.system_audio {
                                    continue;
                                }
                                if self.editing_chip.as_deref() == Some(src) {
                                    let ed = ui.add(
                                        TextEdit::singleline(&mut self.chip_draft)
                                            .desired_width(88.0),
                                    );
                                    if ed.lost_focus()
                                        || ui.input(|i| i.key_pressed(egui::Key::Enter))
                                    {
                                        let val = self.chip_draft.trim().to_string();
                                        if val.is_empty() {
                                            self.custom_labels.remove(src);
                                        } else {
                                            self.custom_labels.insert(src.to_string(), val);
                                        }
                                        self.ctrl.save_preferences(
                                            None,
                                            None,
                                            Some(&self.custom_labels),
                                            None,
                                            None,
                                            None,
                                            None,
                                        );
                                        self.editing_chip = None;
                                    }
                                } else {
                                    let name =
                                        labels.get(src).map(|x| x.as_str()).unwrap_or(src);
                                    if speaker_pill(ui, name, fg, bg, !busy, true).clicked() {
                                        self.editing_chip = Some(src.to_string());
                                        self.chip_draft = name.to_string();
                                    }
                                }
                            }

                            ui.add_space(6.0);
                            ui.label(RichText::new(s.transcription).size(12.0).color(p.text3));
                            let downloading =
                                matches!(self.canary_state, CanaryState::Downloading { .. });
                            let high_text = match &self.canary_state {
                                CanaryState::Downloading { done, total, .. } => {
                                    let pct = if *total > 0 {
                                        (*done as f64 / *total as f64 * 100.0) as u32
                                    } else {
                                        0
                                    };
                                    format!("{pct}%")
                                }
                                CanaryState::Ready => s.high.to_string(),
                                _ => s.high_download.to_string(),
                            };

                            Frame::new()
                                .fill(p.surface2)
                                .stroke(Stroke::new(1.0_f32, p.border))
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(Margin::same(2))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 2.0;
                                        let std_active =
                                            self.transcription != "high" && !downloading;
                                        if ui
                                            .add_enabled(
                                                !busy,
                                                egui::Button::new(
                                                    RichText::new(s.standard)
                                                        .font(semibold(12.0))
                                                        .color(if std_active {
                                                            p.text
                                                        } else {
                                                            p.text3
                                                        }),
                                                )
                                                .fill(if std_active {
                                                    p.surface3
                                                } else {
                                                    Color32::TRANSPARENT
                                                })
                                                .stroke(Stroke::NONE)
                                                .corner_radius(CornerRadius::same(6))
                                                .min_size(vec2(0.0, 24.0)),
                                            )
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .clicked()
                                        {
                                            if let Ok(tr) = self.ctrl.set_transcription("standard")
                                            {
                                                self.transcription = tr;
                                            }
                                        }

                                        let high_active =
                                            self.transcription == "high" || downloading;
                                        if ui
                                            .add_enabled(
                                                !busy && !downloading,
                                                egui::Button::new(
                                                    RichText::new(high_text)
                                                        .font(semibold(12.0))
                                                        .color(if high_active {
                                                            p.text
                                                        } else {
                                                            p.text3
                                                        }),
                                                )
                                                .fill(if high_active {
                                                    p.surface3
                                                } else {
                                                    Color32::TRANSPARENT
                                                })
                                                .stroke(Stroke::NONE)
                                                .corner_radius(CornerRadius::same(6))
                                                .min_size(vec2(0.0, 24.0)),
                                            )
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .clicked()
                                        {
                                            match self.ctrl.set_transcription("high") {
                                                Ok(tr) if tr == "high" => self.transcription = tr,
                                                Ok(_) => {}
                                                Err(e) => self.show_toast(e, true),
                                            }
                                        }
                                    });
                                });

                            if toggle_pill(
                                ui,
                                &mut self.live_transcription,
                                s.live_toggle,
                                !busy,
                                true,
                                p,
                            )
                            .on_hover_text(s.live_toggle_tip)
                            .changed()
                            {
                                self.ctrl.save_preferences(
                                    None,
                                    None,
                                    None,
                                    None,
                                    None,
                                    None,
                                    Some(self.live_transcription),
                                );
                            }

                            if self.record_mode == RecordMode::ScreenAndAudio
                                && toggle_pill(
                                    ui,
                                    &mut self.auto_subtitles,
                                    s.subtitles_switch,
                                    !busy,
                                    true,
                                    p,
                                )
                                .changed()
                            {
                                self.ctrl.save_preferences(
                                    None,
                                    None,
                                    None,
                                    Some(self.auto_subtitles),
                                    None,
                                    None,
                                    None,
                                );
                            }
                        });
                    });
            });

        // Track hover smoothly across the entire Studio Deck for organic LED wake-up
        self.deck_hovered = ui.rect_contains_pointer(deck.response.rect);

        // Subtle studio specular top-edge sheen on the deck
        paint_card_specular(ui.painter(), deck.response.rect, 18.0, p);
    }

    fn draw_importing_card(&mut self, ui: &mut egui::Ui, p: &ThemePalette) {
        let Some(imp) = self.importing.clone() else {
            return;
        };
        let s = strings(&self.lang);
        Frame::new()
            .fill(p.surface)
            .stroke(Stroke::new(1.0_f32, p.border))
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::symmetric(20, 16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let time = ui.input(|i| i.time);
                    Frame::new()
                        .fill(p.bg)
                        .stroke(Stroke::new(1.0_f32, p.border))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::same(0))
                        .show(ui, |ui| {
                            dot_matrix_loader(ui, 5, 5, 38.0, time, p.accent, p);
                        });
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&imp.name).font(semibold(14.0)).color(p.text));
                        let pct = (imp.progress * 100.0).round() as u32;
                        let meta = if imp.stage == "reading" {
                            format!("Reading · {pct}%")
                        } else if let Some(eta) = imp.eta {
                            format!("{} / {} · ~{eta}s", format_clock(imp.done), format_clock(imp.total))
                        } else {
                            format!("{pct}%")
                        };
                        ui.label(RichText::new(meta).size(12.0).color(p.text3));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ghost_button(ui, None, s.cancel, p, false).clicked() {
                            self.ctrl.cancel_import();
                        }
                        if imp.waiting > 0 {
                            ui.label(RichText::new(format!("+{}", imp.waiting)).size(12.0).color(p.text2));
                        }
                    });
                });
                ui.add_space(10.0);
                ui.add(egui::ProgressBar::new(imp.progress as f32).desired_height(4.0));
                if !self.import_lines.is_empty() {
                    ui.add_space(10.0);
                    for seg in &self.import_lines {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format_clock(seg.start))
                                    .size(12.0)
                                    .color(p.text3),
                            );
                            ui.add_space(8.0);
                            ui.label(RichText::new(&seg.text).size(13.5).color(p.text));
                        });
                    }
                }
            });
    }

    fn draw_live_card(&mut self, ui: &mut egui::Ui, p: &ThemePalette) {
        let s = strings(&self.lang);
        let labels = self.current_labels();
        Frame::new()
            .fill(p.surface)
            .stroke(Stroke::new(1.0_f32, p.border))
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::symmetric(20, 16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.6, p.rec);
                    ui.label(
                        RichText::new(s.live.to_uppercase())
                            .font(semibold(11.0))
                            .color(p.text3),
                    );
                });
                ui.add_space(10.0);
                for ph in &self.live_phrases {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format_clock(ph.start))
                                .size(12.0)
                                .color(p.text3),
                        );
                        ui.add_space(8.0);
                        if self.system_audio {
                            let (fg, bg) = if ph.source == "computer" {
                                (p.computer, p.computer_bg)
                            } else {
                                (p.mic, p.mic_bg)
                            };
                            let who = labels.get(&ph.source).map(|x| x.as_str()).unwrap_or(&ph.source);
                            Frame::new()
                                .fill(bg)
                                .corner_radius(CornerRadius::same(5))
                                .inner_margin(Margin::symmetric(6, 1))
                                .show(ui, |ui| {
                                    ui.label(RichText::new(who).font(semibold(11.5)).color(fg));
                                });
                        }
                        ui.label(RichText::new(&ph.text).size(13.5).color(p.text));
                    });
                    ui.add_space(3.0);
                }
                for (src, (committed, pending)) in &self.live_partials {
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(48.0);
                        if self.system_audio {
                            let who = labels.get(src).map(|x| x.as_str()).unwrap_or(src);
                            ui.label(RichText::new(format!("{who}:")).italics().color(p.text3));
                        }
                        ui.label(RichText::new(committed).italics().color(p.text2));
                        ui.label(RichText::new(pending).italics().color(p.text3));
                    });
                }
            });
    }

    fn draw_detail_card(&mut self, ui: &mut egui::Ui, p: &ThemePalette) {
        let Some(rec) = self.detail.clone() else {
            return;
        };
        let s = strings(&self.lang);

        let card = Frame::new()
            .fill(p.surface)
            .stroke(Stroke::new(1.0_f32, p.border_strong))
            .corner_radius(CornerRadius::same(18))
            .inner_margin(Margin::symmetric(24, 22))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                // Title row (borderless clean title + ghost Rename & Delete)
                ui.horizontal(|ui| {
                    if self.editing_title {
                        let resp = ui.add(
                            TextEdit::singleline(&mut self.title_draft)
                                .font(semibold(20.0))
                                .desired_width((ui.available_width() - 210.0).max(160.0)),
                        );
                        if resp.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            let trimmed = self.title_draft.trim().to_string();
                            if !trimmed.is_empty() && trimmed != rec.title {
                                match self.ctrl.rename(&rec.id, &trimmed) {
                                    Ok(()) => {
                                        self.show_toast(s.renamed, false);
                                        self.refresh_list();
                                        self.select_recording(&rec.id);
                                    }
                                    Err(e) => self.show_toast(e, true),
                                }
                            } else if trimmed.is_empty() {
                                self.title_draft = rec.title.clone();
                            }
                            self.editing_title = false;
                        }
                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            self.title_draft = rec.title.clone();
                            self.editing_title = false;
                        }
                    } else {
                        let title_resp = ui
                            .label(RichText::new(&rec.title).font(semibold(22.0)).color(p.text))
                            .interact(Sense::click())
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if title_resp.clicked() {
                            self.editing_title = true;
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ghost_button(ui, Some(Icon::Trash), s.delete, p, true).clicked() {
                            self.confirm_delete_open = true;
                        }
                        if ghost_button(ui, Some(Icon::Pencil), s.rename, p, false).clicked() {
                            self.editing_title = true;
                        }
                    });
                });

                ui.add_space(4.0);

                // Metadata
                ui.horizontal(|ui| {
                    if rec.kind == "audio_recording" {
                        Frame::new()
                            .fill(p.computer_bg)
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(Margin::symmetric(7, 2))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(s.kind_audio_rec)
                                        .font(semibold(11.0))
                                        .color(p.computer),
                                );
                            });
                    } else if rec.is_imported() {
                        let tag = if rec.kind == "video" {
                            s.kind_video
                        } else {
                            s.kind_audio
                        };
                        Frame::new()
                            .fill(p.accent_soft)
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(Margin::symmetric(7, 2))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(format!("{tag} · {}", s.imported))
                                        .font(semibold(11.0))
                                        .color(p.accent_text),
                                );
                            });
                    }
                    ui.label(
                        RichText::new(format!(
                            "{} · {} · {}",
                            short_date(&rec.created_at),
                            format_clock(rec.duration_seconds),
                            format_size(rec.size_bytes)
                        ))
                        .size(12.5)
                        .color(p.text3),
                    );
                });

                ui.add_space(16.0);

                // Action bar with primary coral Play button + tactile studio tool buttons
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    if action_button_styled(ui, Icon::Play, s.play, true, true, None, p).clicked() {
                        if let Err(e) = self.ctrl.play(&rec.id) {
                            self.show_toast(e, true);
                        }
                    }
                    if action_button(ui, Icon::Folder, s.reveal, true, None, p).clicked() {
                        if let Err(e) = self.ctrl.reveal(&rec.id) {
                            self.show_toast(e, true);
                        }
                    }
                    if action_button(ui, Icon::Copy, s.copy, true, None, p).clicked() {
                        match self.ctrl.copy_text(&rec.id) {
                            Ok(_) => self.show_toast(s.copied, false),
                            Err(e) => self.show_toast(e, true),
                        }
                    }
                    if action_button(ui, Icon::Doc, s.word, true, None, p).clicked() {
                        match self.ctrl.export_word(&rec.id, Some(&self.lang)) {
                            Ok(_) => self.show_toast(s.word_saved, false),
                            Err(e) => self.show_toast(e, true),
                        }
                    }
                    if action_button(ui, Icon::Pdf, s.pdf, true, None, p).clicked() {
                        match self.ctrl.export_pdf(&rec.id, Some(&self.lang)) {
                            Ok(_) => self.show_toast(s.pdf_saved, false),
                            Err(e) => self.show_toast(e, true),
                        }
                    }
                    if rec.has_video() {
                        let job = self.subtitling.get(&rec.id).copied();
                        let label = match job {
                            Some((pct, Some(eta))) if pct > 0.0 => {
                                format!("{}% · ~{eta}s", (pct * 100.0) as u32)
                            }
                            Some(_) => s.subtitling.to_string(),
                            None => s.subtitled.to_string(),
                        };
                        let prog = job.map(|(pct, _)| pct as f32);
                        if action_button(ui, Icon::Captions, &label, job.is_none(), prog, p).clicked() {
                            if let Err(e) = self.ctrl.start_subtitling(&rec.id, Some(&self.lang), true) {
                                self.show_toast(e, true);
                            }
                        }
                    }
                });

                ui.add_space(16.0);
                // Crisp hairline separator
                let sep_rect = ui.available_rect_before_wrap();
                ui.painter().line_segment(
                    [pos2(sep_rect.left(), sep_rect.top()), pos2(sep_rect.right(), sep_rect.top())],
                    Stroke::new(1.0_f32, p.border),
                );
                ui.add_space(14.0);

                // Transcript lines with aligned monospace timecode badges
                if rec.segments.is_empty() {
                    ui.label(RichText::new(s.no_speech_here).size(13.5).color(p.text3));
                } else {
                    let mut merged_labels = default_labels(Some(&self.lang));
                    for (k, v) in &rec.labels {
                        merged_labels.insert(k.clone(), v.clone());
                    }
                    let names = speakers(&rec.segments, Some(&merged_labels));

                    for seg in &rec.segments {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            Frame::new()
                                .fill(p.bg)
                                .stroke(Stroke::new(1.0_f32, p.border))
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(Margin::symmetric(7, 2))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(format_clock(seg.start))
                                            .font(FontId::monospace(11.5))
                                            .color(p.text3),
                                    );
                                });
                            if let (Some(map), Some(src)) = (names.as_ref(), seg.source.as_deref()) {
                                if let Some(who) = map.get(src) {
                                    let (fg, bg) = if src == "computer" {
                                        (p.computer, p.computer_bg)
                                    } else {
                                        (p.mic, p.mic_bg)
                                    };
                                    if self.editing_detail_label.as_deref() == Some(src) {
                                        let ed = ui.add(
                                            TextEdit::singleline(&mut self.detail_label_draft)
                                                .desired_width(80.0),
                                        );
                                        if ed.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                            let val = self.detail_label_draft.trim().to_string();
                                            if !val.is_empty() && val != *who {
                                                match self.ctrl.rename_label(&rec.id, src, &val) {
                                                    Ok(_) => {
                                                        self.show_toast(s.label_renamed, false);
                                                        self.select_recording(&rec.id);
                                                    }
                                                    Err(e) => self.show_toast(e, true),
                                                }
                                            }
                                            self.editing_detail_label = None;
                                        }
                                    } else {
                                        let badge = Frame::new()
                                            .fill(bg)
                                            .corner_radius(CornerRadius::same(6))
                                            .inner_margin(Margin::symmetric(8, 2))
                                            .show(ui, |ui| {
                                                ui.label(RichText::new(who).font(semibold(11.5)).color(fg));
                                            })
                                            .response
                                            .interact(Sense::click())
                                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                                        if badge.clicked() {
                                            self.editing_detail_label = Some(src.to_string());
                                            self.detail_label_draft = who.clone();
                                        }
                                    }
                                }
                            }
                            ui.label(RichText::new(&seg.text).size(14.0).color(p.text));
                        });
                        ui.add_space(6.0);
                    }
                }
            });
        paint_card_specular(ui.painter(), card.response.rect, 18.0, p);
    }

    fn draw_empty_state(&mut self, ui: &mut egui::Ui, p: &ThemePalette) {
        let s = strings(&self.lang);

        let w = ui.available_width();
        let (card_rect, resp) = ui.allocate_exact_size(vec2(w, 214.0), Sense::click());
        let resp = if self.phase == Phase::Idle {
            resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            resp
        };
        let hovered = resp.hovered();
        let painter = ui.painter();

        // Crisp elevated studio drop canvas with smooth rounded dashed border (subtle hover tint)
        let fill = if hovered {
            p.surface2
        } else {
            p.surface
        };
        painter.rect_filled(card_rect, CornerRadius::same(18), fill);
        if hovered {
            painter.rect_filled(
                card_rect,
                CornerRadius::same(18),
                Color32::from_rgba_unmultiplied(
                    p.accent.r(),
                    p.accent.g(),
                    p.accent.b(),
                    if p.is_dark { 10 } else { 7 },
                ),
            );
        }
        paint_dashed_rect(
            painter,
            card_rect,
            18.0,
            Stroke::new(
                1.2_f32,
                if hovered { p.accent } else { p.border_strong },
            ),
            7.0,
            5.0,
        );

        // Centered content inside the drop canvas with balanced top & bottom breathing room
        ui.allocate_new_ui(
            egui::UiBuilder::new()
                .max_rect(card_rect.shrink2(vec2(28.0, 14.0)))
                .layout(Layout::top_down(Align::Center)),
            |ui| {
                ui.add_space(2.0);
                // Floating glowing studio emblem
                let (emb_r, _) = ui.allocate_exact_size(Vec2::splat(52.0), Sense::hover());
                let c = emb_r.center();
                ui.painter().circle_filled(
                    c,
                    26.0,
                    Color32::from_rgba_unmultiplied(
                        p.accent.r(),
                        p.accent.g(),
                        p.accent.b(),
                        if hovered { 34 } else { 18 },
                    ),
                );
                let tile = Rect::from_center_size(c, Vec2::splat(42.0));
                ui.painter().rect(
                    tile,
                    CornerRadius::same(13),
                    p.surface2,
                    Stroke::new(
                        1.0_f32,
                        if hovered { p.accent } else { p.border_strong },
                    ),
                    egui::StrokeKind::Inside,
                );
                paint_cronista_logo(ui.painter(), c, 12.5, p);

                ui.add_space(9.0);
                ui.label(
                    RichText::new(s.empty_title)
                        .font(semibold(17.0))
                        .color(p.text),
                );
                ui.add_space(3.0);
                ui.label(
                    RichText::new(if s.is_es {
                        "O arrastra cualquier video o audio aquí para transcribirlo al instante en tu PC."
                    } else {
                        "Or drop any video or audio file here to transcribe it locally in seconds."
                    })
                    .size(13.0)
                    .color(p.text2),
                );

                ui.add_space(10.0);
                // Perfectly centered format pill row (measured from actual text widths)
                let fmts = ["MP4", "MP3", "WAV", "M4A", "MKV", "MOV", "OGG", "FLAC"];
                let pill_font = FontId::monospace(10.5);
                let gap = 7.0_f32;
                let pill_widths: Vec<f32> = fmts
                    .iter()
                    .map(|f| {
                        ui.painter()
                            .layout_no_wrap((*f).to_string(), pill_font.clone(), p.text3)
                            .size()
                            .x
                            + 18.0
                    })
                    .collect();
                let total_row_w: f32 =
                    pill_widths.iter().sum::<f32>() + gap * (fmts.len().saturating_sub(1)) as f32;
                let (row_rect, _) =
                    ui.allocate_exact_size(vec2(total_row_w.min(ui.available_width()), 24.0), Sense::hover());
                let mut x = row_rect.center().x - total_row_w * 0.5;
                for (fmt, pw) in fmts.iter().zip(pill_widths.iter()) {
                    let r = Rect::from_min_size(pos2(x, row_rect.top()), vec2(*pw, 22.0));
                    ui.painter().rect(
                        r,
                        CornerRadius::same(6),
                        p.surface2,
                        Stroke::new(1.0_f32, p.border),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        *fmt,
                        pill_font.clone(),
                        p.text2,
                    );
                    x += *pw + gap;
                }

                ui.add_space(8.0);
                // Subtle privacy / capability caption with comfortable breathing room from bottom border
                ui.label(
                    RichText::new(if s.is_es {
                        "100% Privado y sin nube  ·  Exporta a SRT, Word y PDF  ·  Haz clic para elegir un archivo"
                    } else {
                        "100% Private & offline  ·  Export SRT, Word & PDF  ·  Click to browse a file"
                    })
                    .size(11.5)
                    .color(if hovered { p.accent_text } else { p.text3 }),
                );
            },
        );

        if resp.clicked() && self.phase == Phase::Idle {
            if let Some(files) = rfd::FileDialog::new()
                .add_filter("Audio & Video", MEDIA_EXTENSIONS)
                .pick_files()
            {
                if let Err(err) = self.ctrl.import_files(files) {
                    self.show_toast(err, true);
                }
            }
        }
    }

    fn draw_overlays(&mut self, ctx: &egui::Context, p: &ThemePalette) {
        let s = strings(&self.lang);

        // Theme picker popover with mini color swatches
        if self.theme_menu_open {
            let mut open = self.theme_menu_open;
            egui::Window::new(s.theme)
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::RIGHT_TOP, vec2(-18.0, 62.0))
                .show(ctx, |ui| {
                    egui::Grid::new("themes_grid")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            for (idx, &id) in THEMES.iter().enumerate() {
                                let preview = palette_for(id, self.system_dark);
                                let selected = self.theme_id == id;
                                let (rect, resp) =
                                    ui.allocate_exact_size(vec2(160.0, 36.0), Sense::click());
                                let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                                let hovered = resp.hovered();
                                let painter = ui.painter();
                                let row_bg = if selected || hovered {
                                    p.surface2
                                } else {
                                    Color32::TRANSPARENT
                                };
                                let row_stroke = if selected {
                                    Stroke::new(1.0_f32, p.accent)
                                } else if hovered {
                                    Stroke::new(1.0_f32, p.border)
                                } else {
                                    Stroke::NONE
                                };
                                painter.rect(
                                    rect,
                                    CornerRadius::same(9),
                                    row_bg,
                                    row_stroke,
                                    egui::StrokeKind::Inside,
                                );

                                // Mini swatch card
                                let sw = Rect::from_min_size(
                                    pos2(rect.left() + 6.0, rect.center().y - 12.0),
                                    vec2(36.0, 24.0),
                                );
                                painter.rect(
                                    sw,
                                    CornerRadius::same(6),
                                    preview.bg,
                                    Stroke::new(1.0_f32, p.border_strong),
                                    egui::StrokeKind::Inside,
                                );
                                painter.rect_filled(
                                    Rect::from_min_size(pos2(sw.left() + 5.0, sw.top() + 5.0), vec2(17.0, 14.0)),
                                    CornerRadius::same(3),
                                    preview.surface,
                                );
                                painter.circle_filled(
                                    pos2(sw.right() - 8.0, sw.center().y),
                                    4.0,
                                    preview.accent,
                                );

                                painter.text(
                                    pos2(sw.right() + 10.0, rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    theme_name(&self.lang, id),
                                    FontId::proportional(13.0),
                                    if selected || hovered { p.text } else { p.text2 },
                                );

                                if resp.clicked() {
                                    self.theme_id = id.to_string();
                                    let pal = palette_for(&self.theme_id, self.system_dark);
                                    apply_to_egui(ctx, &pal);
                                    self.ctrl.save_preferences(
                                        None, None, None, None, Some(id), None, None,
                                    );
                                }
                                if idx % 2 == 1 {
                                    ui.end_row();
                                }
                            }
                        });
                });
            self.theme_menu_open = open;
        }

        // Delete confirmation dialog
        if self.confirm_delete_open {
            if let Some(rec) = self.detail.clone() {
                egui::Window::new(s.delete)
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.label(
                            RichText::new(if rec.is_imported() {
                                s.confirm_delete_file
                            } else {
                                s.confirm_delete
                            })
                            .size(13.5)
                            .color(p.text2),
                        );
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            if ghost_button(ui, None, s.cancel, p, false).clicked() {
                                self.confirm_delete_open = false;
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(s.confirm_yes)
                                            .font(semibold(13.0))
                                            .color(Color32::WHITE),
                                    )
                                    .fill(p.danger)
                                    .stroke(Stroke::NONE)
                                    .corner_radius(CornerRadius::same(8)),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                let was_file = rec.is_imported();
                                if let Err(e) = self.ctrl.delete(&rec.id) {
                                    self.show_toast(e, true);
                                } else {
                                    self.detail = None;
                                    self.selected_id = None;
                                    self.refresh_list();
                                    self.show_toast(
                                        if was_file { s.deleted_file } else { s.deleted },
                                        false,
                                    );
                                }
                                self.confirm_delete_open = false;
                            }
                        });
                    });
            }
        }

        // Guided tour modal
        if self.tour_open {
            let steps = tour_steps(&self.lang);
            let idx = self.tour_step.min(steps.len().saturating_sub(1));
            let (title, body) = steps[idx];
            egui::Window::new(format!("{} · {} {} {}", s.help, idx + 1, s.of, steps.len()))
                .collapsible(false)
                .resizable(false)
                .default_width(380.0)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new(title).font(semibold(16.0)).color(p.text));
                    ui.add_space(6.0);
                    ui.label(RichText::new(body).size(13.5).color(p.text2));
                    ui.add_space(14.0);
                    ui.add(
                        egui::ProgressBar::new((idx + 1) as f32 / steps.len() as f32)
                            .desired_height(3.0),
                    );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if idx + 1 < steps.len() && ghost_button(ui, None, s.skip, p, false).clicked() {
                            self.tour_open = false;
                            self.ctrl
                                .save_preferences(None, Some(true), None, None, None, None, None);
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let next_label = if idx + 1 == steps.len() { s.done } else { s.next };
                            let next_btn = egui::Button::new(
                                RichText::new(next_label).font(semibold(13.0)).color(p.bg),
                            )
                            .fill(p.text)
                            .stroke(Stroke::NONE)
                            .corner_radius(CornerRadius::same(8));
                            if ui
                                .add(next_btn)
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                if idx + 1 == steps.len() {
                                    self.tour_open = false;
                                    self.ctrl.save_preferences(
                                        None, Some(true), None, None, None, None, None,
                                    );
                                } else {
                                    self.tour_step += 1;
                                }
                            }
                            if idx > 0 && ghost_button(ui, None, s.back, p, false).clicked() {
                                self.tour_step -= 1;
                            }
                        });
                    });
                });
        }

        // Drag-and-drop overlay (dead-center vertically & horizontally)
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if hovering {
            let busy = self.phase != Phase::Idle;
            let screen = ctx.screen_rect();
            egui::Area::new(egui::Id::new("dropzone"))
                .order(egui::Order::Foreground)
                .fixed_pos(screen.min)
                .show(ctx, |ui| {
                    let painter = ui.painter();
                    let scrim = Color32::from_rgba_unmultiplied(p.bg.r(), p.bg.g(), p.bg.b(), 238);
                    painter.rect_filled(screen, CornerRadius::ZERO, scrim);
                    let frame_rect = screen.shrink(18.0);
                    painter.rect_stroke(
                        frame_rect,
                        CornerRadius::same(20),
                        Stroke::new(1.5_f32, if busy { p.border_strong } else { p.accent }),
                        egui::StrokeKind::Inside,
                    );

                    // Center the content block explicitly around screen.center()
                    let card_rect = Rect::from_center_size(
                        screen.center(),
                        vec2(frame_rect.width().min(540.0), 188.0),
                    );
                    ui.allocate_new_ui(
                        egui::UiBuilder::new()
                            .max_rect(card_rect)
                            .layout(Layout::top_down(Align::Center)),
                        |ui| {
                            let (badge, _) =
                                ui.allocate_exact_size(Vec2::splat(68.0), Sense::hover());
                            ui.painter()
                                .rect_filled(badge, CornerRadius::same(20), p.accent_soft);
                            paint_icon(
                                ui.painter(),
                                Rect::from_center_size(badge.center(), Vec2::splat(28.0)),
                                Icon::Upload,
                                p.accent_text,
                            );
                            ui.add_space(16.0);
                            ui.label(
                                RichText::new(if busy { s.drop_busy_title } else { s.drop_title })
                                    .font(semibold(25.0))
                                    .color(p.text),
                            );
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(if busy { s.drop_busy_body } else { s.drop_body })
                                    .size(14.0)
                                    .color(p.text2),
                            );
                            if !busy {
                                ui.add_space(12.0);
                                ui.label(
                                    RichText::new(s.drop_formats).size(11.5).color(p.text3),
                                );
                            }
                        },
                    );
                });
        }

        // Handle dropped files
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            if let Err(err) = self.ctrl.import_files(dropped) {
                self.show_toast(err, true);
            }
        }

        // Toast pill
        if let Some(toast) = &self.toast {
            if Instant::now() < toast.until {
                egui::Area::new(egui::Id::new("toast"))
                    .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -22.0))
                    .show(ctx, |ui| {
                        Frame::new()
                            .fill(p.surface3)
                            .stroke(Stroke::new(
                                1.0_f32,
                                if toast.is_error { p.danger } else { p.border_strong },
                            ))
                            .corner_radius(CornerRadius::same(99))
                            .inner_margin(Margin::symmetric(16, 9))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let (ic, _) = ui.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
                                    paint_icon(
                                        ui.painter(),
                                        ic,
                                        if toast.is_error { Icon::Alert } else { Icon::Check },
                                        if toast.is_error { p.danger } else { p.ok },
                                    );
                                    ui.label(RichText::new(&toast.message).size(13.0).color(p.text));
                                });
                            });
                    });
            } else {
                self.toast = None;
            }
        }
    }

    fn elapsed_secs(&self) -> f64 {
        let Some(start) = self.started_at else {
            return 0.0;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(start);
        (now - start).max(0.0)
    }
}

impl eframe::App for CronistaApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events(ctx);

        if self.last_ram_check.elapsed() >= Duration::from_secs(2) {
            self.ram_mb = current_ram_mb();
            self.last_ram_check = Instant::now();
        }

        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            if !self.ctrl.request_close() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }

        // Smoothly follow live audio RMS during recording (fast attack, organic release)
        let target_lvl = if matches!(self.phase, Phase::Recording | Phase::Listening) {
            self.ctrl.live_audio_level()
        } else {
            0.0
        };
        let coeff = if target_lvl > self.smoothed_audio_level { 0.42 } else { 0.14 };
        self.smoothed_audio_level += (target_lvl - self.smoothed_audio_level) * coeff;
        if self.smoothed_audio_level < 0.004 {
            self.smoothed_audio_level = 0.0;
        }

        let palette = palette_for(&self.theme_id, self.system_dark);
        self.draw_header(ctx, &palette);
        self.draw_sidebar(ctx, &palette);
        self.draw_main(ctx, &palette);
        self.draw_overlays(ctx, &palette);
        handle_frameless_resize(ctx, &palette);

        let is_animating_fast = matches!(
            self.phase,
            Phase::Recording | Phase::Listening | Phase::Finishing
        ) || self.importing.is_some()
            || self.deck_hovered
            || matches!(
                self.model_status,
                ModelStatus::Loading | ModelStatus::Downloading { .. }
            );
        if is_animating_fast {
            ctx.request_repaint_after(Duration::from_millis(33));
        } else {
            // Very gentle low-CPU tick (~12 fps) for the subtle 4.6s resting heartbeat
            ctx.request_repaint_after(Duration::from_millis(85));
        }
    }
}

/// Enables native Windows 11 DWM rounded corners (DWMWCP_ROUND) and centers the
/// window inside the active monitor's usable work area (above the taskbar).
#[cfg(windows)]
fn apply_windows_rounded_corners_once() {
    use std::sync::Once;
    static APPLIED: Once = Once::new();
    APPLIED.call_once(|| {
        use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
        use windows::Win32::Graphics::Dwm::{
            DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        };
        use windows::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
        };
        use windows::Win32::System::Threading::GetCurrentThreadId;
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumThreadWindows, GetWindowRect, IsWindowVisible, SetWindowPos, SWP_NOACTIVATE,
            SWP_NOZORDER,
        };

        unsafe extern "system" fn cb(hwnd: HWND, _lparam: LPARAM) -> BOOL {
            let pref = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const _ as *const std::ffi::c_void,
                std::mem::size_of_val(&pref) as u32,
            );

            if IsWindowVisible(hwnd).as_bool() {
                let mut wr = RECT::default();
                if GetWindowRect(hwnd, &mut wr).is_ok() {
                    let cur_w = wr.right - wr.left;
                    let cur_h = wr.bottom - wr.top;
                    if cur_w >= 400 && cur_h >= 300 {
                        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY);
                        let mut mi = MONITORINFO {
                            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                            ..Default::default()
                        };
                        if GetMonitorInfoW(hmon, &mut mi).as_bool() {
                            let work = mi.rcWork;
                            let work_w = work.right - work.left;
                            let work_h = work.bottom - work.top;
                            let w = cur_w.min(work_w);
                            let h = cur_h.min(work_h);
                            let x = work.left + (work_w - w) / 2;
                            let y = work.top + (work_h - h) / 2;
                            let _ = SetWindowPos(
                                hwnd,
                                None,
                                x,
                                y,
                                w,
                                h,
                                SWP_NOZORDER | SWP_NOACTIVATE,
                            );
                        }
                    }
                }
            }
            BOOL(1)
        }

        unsafe {
            let tid = GetCurrentThreadId();
            let _ = EnumThreadWindows(tid, Some(cb), LPARAM(0));
        }
    });
}

/// Enables smooth edge & corner resizing and draws a crisp 1px rounded outer border on frameless windows.
fn handle_frameless_resize(ctx: &egui::Context, p: &ThemePalette) {
    #[cfg(windows)]
    apply_windows_rounded_corners_once();

    let is_max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    if is_max {
        return;
    }
    let screen = ctx.screen_rect();
    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("window_outline"),
    ))
    .rect_stroke(
        screen,
        CornerRadius::same(8),
        Stroke::new(1.0_f32, p.border_strong),
        egui::StrokeKind::Inside,
    );

    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let edge = 5.0;
    let left = pos.x <= screen.left() + edge;
    let right = pos.x >= screen.right() - edge;
    let top = pos.y <= screen.top() + edge;
    let bottom = pos.y >= screen.bottom() - edge;

    use egui::{CursorIcon, ResizeDirection, ViewportCommand};
    let dir = match (left, right, top, bottom) {
        (true, _, true, _) => Some((ResizeDirection::NorthWest, CursorIcon::ResizeNwSe)),
        (_, true, true, _) => Some((ResizeDirection::NorthEast, CursorIcon::ResizeNeSw)),
        (true, _, _, true) => Some((ResizeDirection::SouthWest, CursorIcon::ResizeNeSw)),
        (_, true, _, true) => Some((ResizeDirection::SouthEast, CursorIcon::ResizeNwSe)),
        (true, _, _, _) => Some((ResizeDirection::West, CursorIcon::ResizeHorizontal)),
        (_, true, _, _) => Some((ResizeDirection::East, CursorIcon::ResizeHorizontal)),
        (_, _, true, _) => Some((ResizeDirection::North, CursorIcon::ResizeVertical)),
        (_, _, _, true) => Some((ResizeDirection::South, CursorIcon::ResizeVertical)),
        _ => None,
    };

    if let Some((resize_dir, cursor)) = dir {
        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(resize_dir));
        }
    }
}

fn render_snippet(ui: &mut egui::Ui, snippet: &str, p: &ThemePalette) {
    let mut job = egui::text::LayoutJob::default();
    let normal = egui::TextFormat {
        font_id: FontId::proportional(12.0),
        color: p.text2,
        ..Default::default()
    };
    let highlighted = egui::TextFormat {
        font_id: FontId::proportional(12.0),
        color: p.text,
        background: p.mark,
        ..Default::default()
    };

    let mut rest = snippet;
    while let Some(open) = rest.find('[') {
        if open > 0 {
            job.append(&rest[..open], 0.0, normal.clone());
        }
        if let Some(close) = rest[open + 1..].find(']') {
            let inner = &rest[open + 1..open + 1 + close];
            job.append(inner, 0.0, highlighted.clone());
            rest = &rest[open + 2 + close..];
        } else {
            job.append(&rest[open..], 0.0, normal.clone());
            rest = "";
            break;
        }
    }
    if !rest.is_empty() {
        job.append(rest, 0.0, normal);
    }
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = false;
    ui.add(egui::Label::new(job).truncate());
}

fn load_windows_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_string());
    let fonts_dir = std::path::PathBuf::from(&windir).join("Fonts");

    if let Ok(bytes) = std::fs::read(fonts_dir.join("segoeui.ttf")) {
        fonts
            .font_data
            .insert("segoe_ui".to_owned(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "segoe_ui".to_owned());
    }

    let sb_bytes = std::fs::read(fonts_dir.join("seguisb.ttf"))
        .or_else(|_| std::fs::read(fonts_dir.join("segoeuib.ttf")));
    if let Ok(bytes) = sb_bytes {
        fonts
            .font_data
            .insert("segoe_sb".to_owned(), Arc::new(FontData::from_owned(bytes)));
        let mut chain = vec!["segoe_sb".to_owned()];
        if let Some(prop) = fonts.families.get(&FontFamily::Proportional) {
            chain.extend(prop.iter().cloned());
        }
        fonts
            .families
            .insert(FontFamily::Name("semibold".into()), chain);
    } else if let Some(prop) = fonts.families.get(&FontFamily::Proportional).cloned() {
        fonts
            .families
            .insert(FontFamily::Name("semibold".into()), prop);
    }

    ctx.set_fonts(fonts);
}

fn format_clock(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 * 1024 {
        format!("{} KB", (bytes / 1024).max(1))
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn short_date(iso: &str) -> String {
    if iso.len() >= 16 {
        format!("{} {}", &iso[5..10], &iso[11..16])
    } else {
        iso.to_string()
    }
}
