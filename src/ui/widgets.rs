//! Bespoke "Audio Studio" native widgets for Cronista:
//! layered specular glass cards, machined concentric record trigger, live acoustic waveform,
//! architectural dot-grid backdrop, tactile pills, and crisp frameless window controls.

use crate::ui::icons::{paint_icon, Icon};
use crate::ui::theme::ThemePalette;
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Response, Sense,
    Stroke, Ui, Vec2,
};
use std::f32::consts::TAU;

/// Paints a subtle architectural dot-grid and soft radial studio aura behind the main workspace.
pub fn paint_studio_backdrop(
    painter: &Painter,
    rect: Rect,
    p: &ThemePalette,
    recording: bool,
    time: f64,
) {
    // 1. Subtle ambient radial studio glow behind the top deck
    let center = pos2(rect.center().x, rect.top() + 145.0);
    let glow_color = if recording { p.rec } else { p.accent };
    let pulse = if recording {
        1.0 + 0.14 * ((time * 2.6).sin() as f32)
    } else {
        1.0
    };
    let base_alpha = if p.is_dark {
        if recording { 16.0 } else { 11.0 }
    } else if recording {
        12.0
    } else {
        7.0
    };

    // Multi-ring soft radial bloom (fast, zero textures, smooth visual falloff)
    for (i, &(radius, weight)) in [
        (340.0_f32, 0.22_f32),
        (250.0, 0.34),
        (175.0, 0.48),
        (110.0, 0.62),
    ]
    .iter()
    .enumerate()
    {
        let a = ((base_alpha * weight * pulse) as u8).max(1);
        let col = Color32::from_rgba_unmultiplied(glow_color.r(), glow_color.g(), glow_color.b(), a);
        let shift_x = if i % 2 == 0 { -18.0 } else { 14.0 };
        painter.circle_filled(pos2(center.x + shift_x, center.y), radius * pulse, col);
    }

    // Secondary cool counter-glow on the far right for subtle depth
    let cool = p.computer;
    let cool_a = if p.is_dark { 6 } else { 4 };
    painter.circle_filled(
        pos2(rect.right() - 120.0, rect.top() + 260.0),
        220.0,
        Color32::from_rgba_unmultiplied(cool.r(), cool.g(), cool.b(), cool_a),
    );

    // 2. Crisp architectural dot grid (24px pitch, ultra-low opacity)
    let step = 24.0_f32;
    let dot_alpha = if p.is_dark { 14 } else { 18 };
    let dot_col = Color32::from_rgba_unmultiplied(p.text3.r(), p.text3.g(), p.text3.b(), dot_alpha);
    let start_x = ((rect.left() / step).floor() * step) + step * 0.5;
    let start_y = ((rect.top() / step).floor() * step) + step * 0.5;
    let mut y = start_y;
    while y < rect.bottom() {
        let mut x = start_x;
        while x < rect.right() {
            painter.rect_filled(
                Rect::from_center_size(pos2(x, y), Vec2::splat(1.4)),
                CornerRadius::ZERO,
                dot_col,
            );
            x += step;
        }
        y += step;
    }
}

/// Paints a crisp 1px top-edge specular light sheen along the top of an elevated card.
pub fn paint_card_specular(painter: &Painter, rect: Rect, radius: f32, p: &ThemePalette) {
    let inset = radius.max(10.0);
    if rect.width() <= inset * 2.0 + 4.0 {
        return;
    }
    let sheen = if p.is_dark {
        Color32::from_rgba_unmultiplied(255, 255, 255, 22)
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 180)
    };
    painter.line_segment(
        [
            pos2(rect.left() + inset, rect.top() + 1.0),
            pos2(rect.right() - inset, rect.top() + 1.0),
        ],
        Stroke::new(1.0_f32, sheen),
    );
}

/// Borderless ghost button with crisp icon + subtle label (used in the top header & title row).
pub fn ghost_button(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    p: &ThemePalette,
    danger: bool,
) -> Response {
    let font = FontId::proportional(13.0);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), p.text2);
    let icon_w = if icon.is_some() { 15.0 + 7.0 } else { 0.0 };
    let size = vec2(icon_w + galley.size().x + 18.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hovered = resp.hovered();

    let bg = if danger && hovered {
        p.danger_soft
    } else if hovered {
        p.surface2
    } else {
        Color32::TRANSPARENT
    };
    let fg = if danger {
        p.danger
    } else if hovered {
        p.text
    } else {
        p.text2
    };

    let painter = ui.painter();
    if bg != Color32::TRANSPARENT {
        painter.rect(
            rect,
            CornerRadius::same(8),
            bg,
            Stroke::new(1.0_f32, if danger { p.danger.gamma_multiply(0.35) } else { p.border }),
            egui::StrokeKind::Inside,
        );
    }

    let mut x = rect.left() + 9.0;
    if let Some(ic) = icon {
        let icon_rect = Rect::from_center_size(pos2(x + 7.5, rect.center().y), vec2(15.0, 15.0));
        paint_icon(painter, icon_rect, ic, fg);
        x += 22.0;
    }
    painter.text(
        pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        fg,
    );
    resp
}

/// Modern minimalist frameless window control button (Minimize / Maximize / Close).
pub fn window_control_button(
    ui: &mut Ui,
    icon: Icon,
    p: &ThemePalette,
    is_close: bool,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(38.0, 32.0), Sense::click());
    let hovered = resp.hovered();
    let bg = if hovered && is_close {
        p.danger_soft
    } else if hovered {
        p.surface2
    } else {
        Color32::TRANSPARENT
    };
    let fg = if hovered && is_close {
        p.danger
    } else if hovered {
        p.text
    } else {
        p.text2
    };
    let painter = ui.painter();
    if bg != Color32::TRANSPARENT {
        painter.rect_filled(rect, CornerRadius::same(8), bg);
    }
    let ic_rect = Rect::from_center_size(rect.center(), Vec2::splat(16.0));
    paint_icon(painter, ic_rect, icon, fg);
    resp
}

/// Tactile action button with crisp icon, specular rim, and optional primary accent style.
pub fn action_button(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    enabled: bool,
    progress: Option<f32>,
    p: &ThemePalette,
) -> Response {
    action_button_styled(ui, icon, label, enabled, false, progress, p)
}

pub fn action_button_styled(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    enabled: bool,
    primary: bool,
    progress: Option<f32>,
    p: &ThemePalette,
) -> Response {
    let font = FontId::new(
        12.5,
        if primary {
            egui::FontFamily::Name("semibold".into())
        } else {
            egui::FontFamily::Proportional
        },
    );
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), p.text);
    let size = vec2(15.0 + 7.0 + galley.size().x + 24.0, 33.0);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let hovered = enabled && resp.hovered();

    let (bg, border, fg, ic_col) = if primary && enabled {
        let bg = if hovered {
            Color32::from_rgb(
                p.accent.r().saturating_add(14),
                p.accent.g().saturating_add(10),
                p.accent.b().saturating_add(10),
            )
        } else {
            p.accent
        };
        (bg, p.accent, Color32::WHITE, Color32::WHITE)
    } else {
        let bg = if hovered { p.surface3 } else { p.surface2 };
        let border = if hovered { p.border_strong } else { p.border };
        let fg = if enabled || progress.is_some() { p.text } else { p.text3 };
        let ic = if hovered { p.text } else { p.text2 };
        (bg, border, fg, ic)
    };

    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(9),
        bg,
        Stroke::new(1.0_f32, border),
        egui::StrokeKind::Inside,
    );
    if !primary {
        paint_card_specular(painter, rect, 9.0, p);
    }
    if let Some(pct) = progress {
        let fill_w = rect.width() * pct.clamp(0.0, 1.0);
        if fill_w > 1.0 {
            let fill_rect = Rect::from_min_size(rect.min, vec2(fill_w, rect.height()));
            painter.rect_filled(fill_rect, CornerRadius::same(9), p.accent_soft);
        }
    }

    let icon_rect = Rect::from_center_size(
        pos2(rect.left() + 12.0 + 7.5, rect.center().y),
        vec2(15.0, 15.0),
    );
    paint_icon(painter, icon_rect, icon, ic_col);
    painter.text(
        pos2(rect.left() + 34.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        fg,
    );
    resp
}

/// Sleek studio pill toggle switch with subtle illuminated border when active.
pub fn toggle_pill(
    ui: &mut Ui,
    on: &mut bool,
    label: &str,
    enabled: bool,
    compact: bool,
    p: &ThemePalette,
) -> Response {
    let font = FontId::proportional(if compact { 12.5 } else { 13.0 });
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), p.text);
    let h = if compact { 29.0 } else { 32.0 };
    let pad_l = if compact { 7.0 } else { 9.0 };
    let pad_r = if compact { 11.0 } else { 13.0 };
    let track_w = 28.0_f32;
    let track_h = 16.0_f32;
    let gap = 9.0_f32;
    let size = vec2(pad_l + track_w + gap + galley.size().x + pad_r, h);

    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, mut resp) = ui.allocate_exact_size(size, sense);
    if enabled && resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }

    let hovered = enabled && resp.hovered();
    let painter = ui.painter();
    let container_bg = if *on && enabled {
        Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), if p.is_dark { 18 } else { 12 })
    } else if hovered {
        p.surface3
    } else {
        p.surface2
    };
    let container_stroke = if hovered {
        Stroke::new(1.0_f32, p.border_strong)
    } else if *on && enabled {
        Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), 65),
        )
    } else {
        Stroke::new(1.0_f32, p.border)
    };
    painter.rect(
        rect,
        CornerRadius::same(9),
        container_bg,
        container_stroke,
        egui::StrokeKind::Inside,
    );

    let track_rect = Rect::from_center_size(
        pos2(rect.left() + pad_l + track_w * 0.5, rect.center().y),
        vec2(track_w, track_h),
    );
    let track_fill = if *on {
        if enabled { p.accent } else { p.accent.gamma_multiply(0.5) }
    } else {
        p.surface3
    };
    let track_stroke = if *on {
        Stroke::NONE
    } else {
        Stroke::new(1.0_f32, p.border_strong)
    };
    painter.rect(
        track_rect,
        CornerRadius::same(99),
        track_fill,
        track_stroke,
        egui::StrokeKind::Inside,
    );

    let knob_x = if *on {
        track_rect.right() - 8.0
    } else {
        track_rect.left() + 8.0
    };
    let knob_color = if *on { Color32::WHITE } else { p.text2 };
    painter.circle_filled(pos2(knob_x, track_rect.center().y), 5.2, knob_color);

    let label_color = if !enabled {
        p.text3
    } else if *on {
        p.text
    } else {
        p.text2
    };
    painter.text(
        pos2(track_rect.right() + gap, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        label_color,
    );
    resp
}

/// Soft tinted speaker chip (`● Yo ✎` / `● Otros ✎`).
pub fn speaker_pill(
    ui: &mut Ui,
    label: &str,
    fg: Color32,
    bg: Color32,
    enabled: bool,
    show_pencil: bool,
) -> Response {
    let font = FontId::new(12.0, egui::FontFamily::Name("semibold".into()));
    let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), fg);
    let pencil_w = if show_pencil { 16.0 } else { 0.0 };
    let size = vec2(10.0 + 7.0 + 6.0 + galley.size().x + pencil_w + 10.0, 28.0);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let hovered = enabled && resp.hovered();

    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(99),
        bg,
        Stroke::new(
            1.0_f32,
            if hovered {
                fg.gamma_multiply(0.8)
            } else {
                fg.gamma_multiply(0.22)
            },
        ),
        egui::StrokeKind::Inside,
    );
    painter.circle_filled(pos2(rect.left() + 13.0, rect.center().y), 3.3, fg);
    painter.text(
        pos2(rect.left() + 22.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        fg,
    );
    if show_pencil {
        let p_rect = Rect::from_center_size(
            pos2(rect.right() - 13.0, rect.center().y),
            vec2(11.0, 11.0),
        );
        paint_icon(painter, p_rect, Icon::Pencil, fg.gamma_multiply(0.78));
    }
    resp
}

/// Precision machined hardware studio Record / Stop trigger with dial ticks, radial glow,
/// breathing halo while active, and subtle specular highlight on the core.
pub fn record_button(
    ui: &mut Ui,
    active: bool,
    enabled: bool,
    time: f64,
    p: &ThemePalette,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::splat(94.0),
        if enabled { Sense::click() } else { Sense::hover() },
    );
    let hovered = enabled && resp.hovered();
    let painter = ui.painter();
    let c = rect.center();

    let pulse = if active {
        0.5 + 0.5 * ((time * 3.2).sin() as f32)
    } else {
        0.0
    };

    // Animated expanding pulse halo when recording
    if active {
        let ring_r = 41.0 + pulse * 6.0;
        let ring_a = ((1.0 - pulse) * 90.0) as u8;
        painter.circle_stroke(
            c,
            ring_r,
            Stroke::new(
                1.5_f32,
                Color32::from_rgba_unmultiplied(p.rec.r(), p.rec.g(), p.rec.b(), ring_a),
            ),
        );
    }

    // Outer precision studio bezel with 12 delicate dial ticks
    let outer_r = 44.0_f32;
    let bezel_stroke = if active {
        Stroke::new(1.0_f32, p.rec.gamma_multiply(0.5))
    } else if hovered {
        Stroke::new(1.0_f32, p.border_strong)
    } else {
        Stroke::new(1.0_f32, p.border)
    };
    painter.circle_stroke(c, outer_r, bezel_stroke);

    let tick_col = if active {
        p.rec.gamma_multiply(0.55)
    } else {
        p.border_strong
    };
    for i in 0..12 {
        let ang = (i as f32 / 12.0) * TAU;
        let r0 = outer_r - 4.0;
        let r1 = outer_r - 1.2;
        painter.line_segment(
            [
                pos2(c.x + ang.cos() * r0, c.y + ang.sin() * r0),
                pos2(c.x + ang.cos() * r1, c.y + ang.sin() * r1),
            ],
            Stroke::new(1.0_f32, tick_col),
        );
    }

    // Recessed anodized well
    let well_r = 35.5_f32;
    let well_fill = if active {
        p.rec_soft
    } else if hovered {
        p.surface3
    } else {
        p.surface2
    };
    let well_stroke = if active {
        Stroke::new(1.2_f32, p.rec.gamma_multiply(0.75))
    } else if hovered {
        Stroke::new(1.2_f32, p.accent.gamma_multiply(0.55))
    } else {
        Stroke::new(1.0_f32, p.border_strong)
    };
    painter.circle(c, well_r, well_fill, well_stroke);

    // Multi-step radial coral glow inside well
    let dot_color = if enabled {
        p.rec
    } else {
        p.rec.gamma_multiply(0.45)
    };
    if enabled {
        let g1 = if active { 34 + (pulse * 22.0) as u8 } else if hovered { 34 } else { 18 };
        let g2 = if active { 56 + (pulse * 26.0) as u8 } else if hovered { 54 } else { 30 };
        painter.circle_filled(
            c,
            29.0,
            Color32::from_rgba_unmultiplied(p.rec.r(), p.rec.g(), p.rec.b(), g1),
        );
        painter.circle_filled(
            c,
            21.5,
            Color32::from_rgba_unmultiplied(p.rec.r(), p.rec.g(), p.rec.b(), g2),
        );
    }

    if active {
        let sq = Rect::from_center_size(c, Vec2::splat(22.0));
        painter.rect_filled(sq, CornerRadius::same(6), dot_color);
        // Subtle top specular highlight on stop square
        painter.line_segment(
            [pos2(sq.left() + 5.0, sq.top() + 2.0), pos2(sq.right() - 5.0, sq.top() + 2.0)],
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 75)),
        );
    } else {
        let core_r = if hovered { 15.2 } else { 14.0 };
        painter.circle_filled(c, core_r, dot_color);
        // Subtle tactile specular sheen on top-left of the button cap
        painter.circle_filled(
            pos2(c.x - 4.0, c.y - 4.5),
            4.2,
            Color32::from_rgba_unmultiplied(255, 255, 255, 52),
        );
    }

    resp
}

/// Decorative & live-reactive acoustic studio waveform visualizer ribbon.
pub fn studio_waveform(ui: &mut Ui, active: bool, busy_import: bool, time: f64, p: &ThemePalette) {
    let bars = 26usize;
    let bar_w = 3.2_f32;
    let gap = 4.2_f32;
    let total_w = bars as f32 * bar_w + (bars - 1) as f32 * gap;
    let total_h = 54.0_f32;
    let (rect, _) = ui.allocate_exact_size(vec2(total_w, total_h), Sense::hover());
    let painter = ui.painter();
    let cy = rect.center().y;
    let t = time as f32;

    // Subtle center horizon line
    painter.line_segment(
        [pos2(rect.left(), cy), pos2(rect.right(), cy)],
        Stroke::new(1.0_f32, p.border),
    );

    for i in 0..bars {
        let u = (i as f32) / ((bars - 1) as f32);
        // Smooth bell envelope so edges taper gracefully
        let env = (u * std::f32::consts::PI).sin().powf(0.85);
        let amp = if active {
            let w1 = (t * 5.2 + i as f32 * 0.55).sin().abs();
            let w2 = (t * 8.7 - i as f32 * 0.92).cos().abs();
            let w3 = (t * 3.1 + i as f32 * 0.28).sin().abs();
            0.18 + 0.82 * env * (0.45 * w1 + 0.35 * w2 + 0.20 * w3)
        } else if busy_import {
            let sweep = (t * 3.4 - u * 4.5).sin() * 0.5 + 0.5;
            0.14 + 0.56 * env * sweep
        } else {
            // Calm sculptural acoustic signature at rest
            let sculpt = 0.42
                + 0.36 * (i as f32 * 0.62).sin().abs()
                + 0.22 * (i as f32 * 1.15 + 0.8).cos().abs();
            0.12 + 0.42 * env * sculpt
        };

        let h = (amp * (total_h - 6.0)).max(4.0);
        let x = rect.left() + i as f32 * (bar_w + gap);
        let bar_rect = Rect::from_center_size(pos2(x + bar_w * 0.5, cy), vec2(bar_w, h));

        let color = if active {
            let alpha = (145.0 + 110.0 * env) as u8;
            Color32::from_rgba_unmultiplied(p.rec.r(), p.rec.g(), p.rec.b(), alpha)
        } else if busy_import {
            let alpha = (110.0 + 120.0 * env) as u8;
            Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), alpha)
        } else {
            let alpha = (40.0 + 75.0 * env) as u8;
            if i % 3 == 0 {
                Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), alpha)
            } else {
                Color32::from_rgba_unmultiplied(p.text3.r(), p.text3.g(), p.text3.b(), alpha)
            }
        };

        painter.rect_filled(bar_rect, CornerRadius::same(2), color);
    }
}

/// Hand-drawn dashed rounded drop/click tile for "Transcribe a file" in the sidebar footer.
pub fn dashed_import_tile(
    ui: &mut Ui,
    title: &str,
    subtitle: &str,
    enabled: bool,
    p: &ThemePalette,
) -> Response {
    let size = vec2(ui.available_width(), 56.0);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let hovered = enabled && resp.hovered();

    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius::same(12),
        if hovered { p.accent_soft } else { p.surface },
    );
    let dash_color = if hovered { p.accent } else { p.border_strong };
    paint_dashed_rect(painter, rect, 12.0, Stroke::new(1.0_f32, dash_color), 5.0, 4.0);

    // Left 34x34 icon badge
    let badge = Rect::from_center_size(pos2(rect.left() + 27.0, rect.center().y), vec2(34.0, 34.0));
    painter.rect_filled(
        badge,
        CornerRadius::same(9),
        if hovered { p.accent } else { p.surface2 },
    );
    let icon_rect = Rect::from_center_size(badge.center(), vec2(16.0, 16.0));
    paint_icon(
        painter,
        icon_rect,
        Icon::Upload,
        if hovered { Color32::WHITE } else { p.accent_text },
    );

    // Bold title + muted subtitle
    painter.text(
        pos2(badge.right() + 11.0, rect.center().y - 8.5),
        Align2::LEFT_CENTER,
        title,
        FontId::new(13.0, egui::FontFamily::Name("semibold".into())),
        if enabled { p.text } else { p.text3 },
    );
    painter.text(
        pos2(badge.right() + 11.0, rect.center().y + 8.5),
        Align2::LEFT_CENTER,
        subtitle,
        FontId::proportional(11.0),
        p.text3,
    );
    resp
}

pub fn paint_dashed_rect(painter: &Painter, rect: Rect, r: f32, stroke: Stroke, dash: f32, gap: f32) {
    let draw_edge = |a: Pos2, b: Pos2| {
        let delta = b - a;
        let len = delta.length();
        if len <= 0.1 {
            return;
        }
        let dir = delta / len;
        let mut t = 0.0;
        while t < len {
            let end = (t + dash).min(len);
            painter.line_segment([a + dir * t, a + dir * end], stroke);
            t += dash + gap;
        }
    };
    let r = r.min(rect.width() * 0.5).min(rect.height() * 0.5);
    draw_edge(pos2(rect.left() + r, rect.top()), pos2(rect.right() - r, rect.top()));
    draw_edge(pos2(rect.right(), rect.top() + r), pos2(rect.right(), rect.bottom() - r));
    draw_edge(pos2(rect.right() - r, rect.bottom()), pos2(rect.left() + r, rect.bottom()));
    draw_edge(pos2(rect.left(), rect.bottom() - r), pos2(rect.left(), rect.top() + r));
}
