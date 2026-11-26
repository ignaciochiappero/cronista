//! Bespoke "Audio Studio" native widgets for Cronista:
//! layered specular glass cards, machined concentric record trigger, live acoustic waveform,
//! architectural dot-grid backdrop, tactile pills, and crisp frameless window controls.

use crate::ui::icons::{paint_icon, Icon};
use crate::ui::theme::ThemePalette;
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, FontId, Painter, Pos2, Rect, Response,
    Sense, Stroke, Ui, Vec2,
};
use std::f32::consts::TAU;

/// Paints a silky GPU-interpolated radial gradient (`egui::Mesh` triangle fan) with zero hard edges.
fn paint_radial_glow(painter: &Painter, center: Pos2, radius: f32, color: Color32, center_alpha: u8) {
    let steps = 48usize;
    let mut mesh = egui::Mesh::default();
    let c_col = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), center_alpha);
    let edge_col = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 0);

    // Center vertex (index 0)
    mesh.colored_vertex(center, c_col);
    // Mid-falloff ring at 45% radius for photographic Gaussian-like falloff (indices 1..=steps)
    let mid_col = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), center_alpha / 3);
    for i in 0..steps {
        let ang = (i as f32 / steps as f32) * TAU;
        let p_mid = pos2(center.x + ang.cos() * radius * 0.45, center.y + ang.sin() * radius * 0.45);
        mesh.colored_vertex(p_mid, mid_col);
    }
    // Outer ring at 100% radius fading smoothly to 0 alpha (indices steps+1..=2*steps)
    for i in 0..steps {
        let ang = (i as f32 / steps as f32) * TAU;
        let p_out = pos2(center.x + ang.cos() * radius, center.y + ang.sin() * radius);
        mesh.colored_vertex(p_out, edge_col);
    }

    for i in 0..steps {
        let next = (i + 1) % steps;
        let m0 = (1 + i) as u32;
        let m1 = (1 + next) as u32;
        let o0 = (1 + steps + i) as u32;
        let o1 = (1 + steps + next) as u32;
        mesh.add_triangle(0, m0, m1);
        mesh.add_triangle(m0, o0, o1);
        mesh.add_triangle(m0, o1, m1);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Paints a subtle architectural dot-grid and silky radial studio aura behind the main workspace.
pub fn paint_studio_backdrop(
    painter: &Painter,
    rect: Rect,
    p: &ThemePalette,
    recording: bool,
    time: f64,
) {
    // 1. Silky GPU mesh radial studio aura (zero hard circles!)
    let center = pos2(rect.center().x - 40.0, rect.top() + 140.0);
    let glow_color = if recording { p.rec } else { p.accent };
    let pulse = if recording {
        1.0 + 0.12 * ((time * 2.6).sin() as f32)
    } else {
        1.0
    };
    let primary_alpha = if p.is_dark {
        if recording { 38 } else { 24 }
    } else if recording {
        26
    } else {
        15
    };
    paint_radial_glow(painter, center, 440.0 * pulse, glow_color, primary_alpha);

    // Subtle cool counter-aura on the right
    let cool_alpha = if p.is_dark { 18 } else { 10 };
    paint_radial_glow(
        painter,
        pos2(rect.right() - 120.0, rect.top() + 310.0),
        380.0,
        p.computer,
        cool_alpha,
    );

    // 2. Crisp architectural dot grid (24px pitch, ultra-low opacity)
    let step = 24.0_f32;
    let dot_alpha = if p.is_dark { 15 } else { 18 };
    let dot_col = Color32::from_rgba_unmultiplied(p.text3.r(), p.text3.g(), p.text3.b(), dot_alpha);
    let start_x = ((rect.left() / step).floor() * step) + step * 0.5;
    let start_y = ((rect.top() / step).floor() * step) + step * 0.5;
    let mut y = start_y;
    while y < rect.bottom() {
        let mut x = start_x;
        while x < rect.right() {
            painter.rect_filled(
                Rect::from_center_size(pos2(x, y), Vec2::splat(1.3)),
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
    resp.on_hover_cursor(CursorIcon::PointingHand)
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
    resp.on_hover_cursor(CursorIcon::PointingHand)
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
    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        resp
    }
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
    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        resp
    }
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
    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        resp
    }
}

fn mix_rgb(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn c32_to_f32(c: Color32) -> [f32; 3] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
}

/// Arc UI (`@uiarc/voice-orb`) & ElevenLabs-inspired 3D Fresnel liquid Aura Voice Orb trigger.
/// Rendered with a high-resolution polar `egui::Mesh` so internal plasma gradients, Fresnel rim
/// lighting, and atmospheric glow blend seamlessly on GPU with zero hard concentric steps.
pub fn record_button(
    ui: &mut Ui,
    active: bool,
    enabled: bool,
    audio_level: f32,
    time: f64,
    p: &ThemePalette,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::splat(94.0),
        if enabled { Sense::click() } else { Sense::hover() },
    );
    let hovered = enabled && resp.hovered();
    let hover_mix = ui.ctx().animate_bool_with_time(
        egui::Id::new("voice_orb_hover"),
        hovered,
        0.45,
    );
    let active_mix = ui.ctx().animate_bool_with_time(
        egui::Id::new("voice_orb_active"),
        active,
        0.38,
    );

    let painter = ui.painter();
    let c = rect.center();
    let t = time as f32;
    let lvl = if active { audio_level.clamp(0.0, 1.0) } else { 0.0 };

    // 1. Wide, dreamy red atmospheric mist aura behind the orb
    let aura_col = p.rec;
    let aura_alpha = if !enabled {
        10
    } else {
        (26.0 + hover_mix * 36.0 + active_mix * (32.0 + lvl * 46.0)).min(115.0) as u8
    };
    let aura_radius = 48.0 + hover_mix * 7.0 + active_mix * (5.0 + lvl * 12.0);
    paint_radial_glow(painter, c, aura_radius, aura_col, aura_alpha);

    // 2. High-resolution polar 3D misty red aurora orb (translucent & softly feathered edges)
    let rings = 16usize;
    let sectors = 56usize;
    let base_r = 37.5 + hover_mix * 1.8 + active_mix * (1.5 + lvl * 3.8);

    // Deep smoky crimson -> vivid red nebula -> luminous scarlet ember
    let mist_void = [0.09, 0.012, 0.016];
    let red_mid = c32_to_f32(p.rec);
    let red_deep = [
        (red_mid[0] * 0.72).clamp(0.0, 1.0),
        (red_mid[1] * 0.16).clamp(0.0, 1.0),
        (red_mid[2] * 0.18).clamp(0.0, 1.0),
    ];
    let red_ember = [
        1.0,
        (red_mid[1] * 1.18 + 0.16).min(1.0),
        (red_mid[2] * 0.92 + 0.10).min(1.0),
    ];
    let rim_red = [
        1.0,
        (red_mid[1] * 0.62 + 0.22).min(1.0),
        (red_mid[2] * 0.55 + 0.18).min(1.0),
    ];

    // Constant, slow, meditative mist drift (zero phase jump on hover!)
    let flow = t * 0.22;
    let ca = (flow * 0.35).cos();
    let sa = (flow * 0.35).sin();

    let glow_boost = (0.42 + hover_mix * 0.48 + active_mix * (0.40 + lvl * 0.30)).clamp(0.0, 1.2);

    let shade_vertex = |nx: f32, ny: f32, r_norm: f32| -> Color32 {
        let sz = (1.0 - (r_norm * r_norm).min(1.0)).sqrt();
        let rx = nx * ca - ny * sa;
        let ry = nx * sa + ny * ca;

        // Domain-warped 2-octave red smoke/nebula plumes (organic wispy clouds!)
        let w1 = organic_noise(rx * 1.75 + flow * 0.52, ry * 1.75 - flow * 0.41, sz * 1.5 + flow * 0.32);
        let w2 = organic_noise(
            rx * 2.85 + (w1 - 0.5) * 1.65 - flow * 0.38,
            ry * 2.85 - (w1 - 0.5) * 1.65 + flow * 0.34,
            sz * 2.1 - flow * 0.26,
        );
        let plume = (0.42 * w1 + 0.58 * w2).clamp(0.0, 1.0);
        // Sculpted smoky clouds with deep dark-red hollows & bright red wisps
        let cloud = smoothstep((plume - 0.18) / 0.68);
        let wisp = smoothstep((plume - 0.54) / 0.38);

        // Soft misty radial fade so the red nebula stays airy & translucent
        let soft_edge = (1.0 - smoothstep((r_norm - 0.50) / 0.50)).clamp(0.0, 1.0);
        let edge_noise = (0.60 + 0.55 * w2).clamp(0.0, 1.15);
        let base_translucency = 0.62 + 0.16 * hover_mix + 0.18 * active_mix;
        let mist_alpha =
            (soft_edge * (0.26 + 0.74 * cloud) * edge_noise * base_translucency).clamp(0.0, 0.80);

        // Soft glowing crimson Fresnel veil right inside the misty perimeter
        let veil_ring = (-(r_norm - 0.76).powi(2) / (2.0 * 0.13 * 0.13)).exp()
            * (0.16 + 0.34 * hover_mix + 0.28 * active_mix)
            * (0.55 + 0.45 * w1);

        let mut rgb = mix_rgb(mist_void, red_deep, 0.35 + 0.65 * cloud);
        rgb = mix_rgb(rgb, red_mid, (cloud * glow_boost).clamp(0.0, 1.0));
        // Luminous warm ember filaments glowing from inside when hovered or recording
        let inner_lum = (wisp * sz.powf(0.55) * (0.22 + hover_mix * 0.62 + active_mix * (0.58 + lvl * 0.40)))
            .clamp(0.0, 1.0);
        rgb = mix_rgb(rgb, red_ember, inner_lum * 0.88);
        rgb = mix_rgb(rgb, rim_red, (veil_ring * 0.72).clamp(0.0, 0.82));

        let final_alpha = (mist_alpha + veil_ring * 0.42).clamp(0.0, 0.84)
            * if enabled { 1.0 } else { 0.40 };

        Color32::from_rgba_unmultiplied(
            (rgb[0] * 255.0).clamp(0.0, 255.0) as u8,
            (rgb[1] * 255.0).clamp(0.0, 255.0) as u8,
            (rgb[2] * 255.0).clamp(0.0, 255.0) as u8,
            (final_alpha * 255.0).clamp(0.0, 255.0) as u8,
        )
    };

    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(c, shade_vertex(0.0, 0.0, 0.0));

    for ri in 1..=rings {
        let r_norm = ri as f32 / rings as f32;
        for si in 0..sectors {
            let ang = (si as f32 / sectors as f32) * TAU;
            let ca_s = ang.cos();
            let sa_s = ang.sin();

            // Gentle organic misty edge undulation (stronger with live voice)
            let wn = organic_noise(ca_s * 1.4 + flow * 0.9, sa_s * 1.4 - flow * 0.8, flow * 0.6) - 0.5;
            let wobble = wn * (1.8 + hover_mix * 1.2 + active_mix * (1.8 + lvl * 4.5)) * r_norm;

            let px_r = r_norm * (base_r + wobble);
            let pos = pos2(c.x + ca_s * px_r, c.y + sa_s * px_r);
            let col = if ri == rings {
                Color32::from_rgba_unmultiplied(p.rec.r(), p.rec.g(), p.rec.b(), 0)
            } else {
                shade_vertex(ca_s * r_norm, sa_s * r_norm, r_norm)
            };
            mesh.colored_vertex(pos, col);
        }
    }

    for si in 0..sectors {
        let next = (si + 1) % sectors;
        mesh.add_triangle(0, (1 + si) as u32, (1 + next) as u32);
    }
    for ri in 1..rings {
        let row0 = 1 + (ri - 1) * sectors;
        let row1 = 1 + ri * sectors;
        for si in 0..sectors {
            let next = (si + 1) % sectors;
            let a = (row0 + si) as u32;
            let b = (row0 + next) as u32;
            let d = (row1 + si) as u32;
            let e = (row1 + next) as u32;
            mesh.add_triangle(a, d, e);
            mesh.add_triangle(a, e, b);
        }
    }
    painter.add(egui::Shape::mesh(mesh));

    // 3. Subtle circular glass button border so it reads unmistakably as a tactile button
    let border_r = base_r - 1.5;
    let border_a = if !enabled {
        24
    } else {
        (46.0 + hover_mix * 68.0 + active_mix * 64.0).min(155.0) as u8
    };
    let border_col = Color32::from_rgba_unmultiplied(
        (p.rec.r() as f32 * 0.82 + 45.0).min(255.0) as u8,
        (p.rec.g() as f32 * 0.78 + 38.0).min(255.0) as u8,
        (p.rec.b() as f32 * 0.78 + 38.0).min(255.0) as u8,
        border_a,
    );
    painter.circle_stroke(c, border_r, Stroke::new(1.0_f32, border_col));

    // 4. Minimalist frosted stop glyph ONLY while actively recording
    if active_mix > 0.05 {
        let sq_sz = 14.0 + 2.0 * active_mix;
        let sq = Rect::from_center_size(c, Vec2::splat(sq_sz));
        let a = (active_mix * 235.0) as u8;
        painter.rect_filled(
            sq,
            CornerRadius::same(4),
            Color32::from_rgba_unmultiplied(255, 255, 255, a),
        );
    }

    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Fast deterministic 3D value noise (`0.0..=1.0`) for organic, non-repetitive LED patterns.
fn hash3(ix: i32, iy: i32, iz: i32) -> f32 {
    let mut n = ix
        .wrapping_mul(374_761_393)
        .wrapping_add(iy.wrapping_mul(668_265_263))
        .wrapping_add(iz.wrapping_mul(2_147_483_647));
    n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
    let u = (n ^ (n >> 16)) as u32;
    (u & 0x00ff_ffff) as f32 / 16_777_215.0
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn organic_noise(x: f32, y: f32, z: f32) -> f32 {
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let iz = z.floor() as i32;
    let fx = smoothstep(x - ix as f32);
    let fy = smoothstep(y - iy as f32);
    let fz = smoothstep(z - iz as f32);

    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let c000 = hash3(ix, iy, iz);
    let c100 = hash3(ix + 1, iy, iz);
    let c010 = hash3(ix, iy + 1, iz);
    let c110 = hash3(ix + 1, iy + 1, iz);
    let c001 = hash3(ix, iy, iz + 1);
    let c101 = hash3(ix + 1, iy, iz + 1);
    let c011 = hash3(ix, iy + 1, iz + 1);
    let c111 = hash3(ix + 1, iy + 1, iz + 1);

    let x00 = lerp(c000, c100, fx);
    let x10 = lerp(c010, c110, fx);
    let x01 = lerp(c001, c101, fx);
    let x11 = lerp(c011, c111, fx);
    lerp(lerp(x00, x10, fy), lerp(x01, x11, fy), fz)
}

/// Organic double-beat cardiac/acoustic ring (`lub-dub`) travelling outward from center (`dist = 0 -> 1`).
fn heartbeat_ring(dist: f32, time: f32, period: f32, ring_width: f32) -> f32 {
    let phase = (time % period) / period; // 0..1
    // Primary beat ("lub"): expands from 0.0 -> 1.15 over first 52% of cycle
    let r1 = phase * 2.15;
    let env1 = (1.0 - phase * 1.45).clamp(0.0, 1.0);
    let b1 = (-(dist - r1).powi(2) / (2.0 * ring_width * ring_width)).exp() * env1;

    // Secondary softer echo beat ("dub"): starts slightly later at phase 0.16
    let p2 = (phase - 0.16).max(0.0);
    let r2 = p2 * 1.95;
    let env2 = if phase > 0.16 {
        (1.0 - p2 * 1.65).clamp(0.0, 1.0) * 0.65
    } else {
        0.0
    };
    let b2 = (-(dist - r2).powi(2) / (2.0 * (ring_width * 1.15).powi(2))).exp() * env2;

    (b1 + b2).clamp(0.0, 1.0)
}

/// Arc UI (`@uiarc/dot-matrix-loader`) inspired animated 5x5 LED dot-matrix loader.
/// Emits soft center-outward heartbeat ripples with organic noise; unlit dots dissolve at edges.
pub fn dot_matrix_loader(
    ui: &mut Ui,
    cols: usize,
    rows: usize,
    size: f32,
    time: f64,
    color: Color32,
    p: &ThemePalette,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    let pad = size * 0.14;
    let inner = rect.shrink(pad);
    let step_x = if cols > 1 { inner.width() / (cols - 1) as f32 } else { 0.0 };
    let step_y = if rows > 1 { inner.height() / (rows - 1) as f32 } else { 0.0 };
    let dot_r = (step_x.min(step_y) * 0.27).clamp(1.2, 2.3);
    let t = time as f32;
    let half_c = (cols as f32 - 1.0).max(1.0) * 0.5;
    let half_r = (rows as f32 - 1.0).max(1.0) * 0.5;

    for r in 0..rows {
        for c in 0..cols {
            let cx = inner.left() + c as f32 * step_x;
            let cy = inner.top() + r as f32 * step_y;

            let nx = (c as f32 - half_c) / half_c;
            let ny = (r as f32 - half_r) / half_r;
            let r_norm = (nx * nx + ny * ny).sqrt();
            let edge_fade = (1.0 - (r_norm * 0.82).powi(2)).clamp(0.0, 1.0).powf(1.5);

            // Organic center-outward heartbeat + subtle random shimmer
            let n = organic_noise(c as f32 * 0.65, r as f32 * 0.65, t * 1.4);
            let pulse = heartbeat_ring((r_norm * 0.85 + (n - 0.5) * 0.14).max(0.0), t, 1.9, 0.26);
            let intensity = (pulse * (0.65 + 0.45 * n) * edge_fade).clamp(0.0, 1.0);

            let unlit_alpha = ((if p.is_dark { 7.0 } else { 10.0 }) * edge_fade).round() as u8;
            if unlit_alpha >= 2 {
                let base_dot = Color32::from_rgba_unmultiplied(
                    p.text3.r(),
                    p.text3.g(),
                    p.text3.b(),
                    unlit_alpha,
                );
                painter.circle_filled(pos2(cx, cy), dot_r * 0.72, base_dot);
            }

            if intensity > 0.06 {
                let a = (intensity * 235.0) as u8;
                if a >= 4 {
                    let lit = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a);
                    if intensity > 0.55 {
                        let halo_a = ((intensity - 0.5) * 85.0 * edge_fade) as u8;
                        if halo_a >= 2 {
                            painter.circle_filled(
                                pos2(cx, cy),
                                dot_r * 1.85,
                                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), halo_a),
                            );
                        }
                    }
                    painter.circle_filled(pos2(cx, cy), dot_r * (0.68 + 0.38 * intensity), lit);
                }
            }
        }
    }
}

/// Organic center-outward LED Dot-Matrix studio indicator (`19x5`):
/// - Idle (unhovered): whisper-faint core with a rare, soft heartbeat every ~4.6s; edges vanish into bg.
/// - Hover: wakes smoothly with organic center-outward ripples and soft ember noise.
/// - Recording: reacts directly to real-time microphone/PC audio RMS (`audio_level`), radiating
///   living acoustic heartbeat pulses outward from the center as you speak.
/// - Transcribing: calm center-outward double-beat radial pulse loader.
pub fn studio_waveform(
    ui: &mut Ui,
    active: bool,
    busy_import: bool,
    hover_mix: f32,
    audio_level: f32,
    time: f64,
    p: &ThemePalette,
) {
    let cols = 19usize;
    let rows = 5usize;
    let pitch_x = 10.0_f32;
    let pitch_y = 9.5_f32;
    let total_w = (cols - 1) as f32 * pitch_x + 12.0;
    let total_h = (rows - 1) as f32 * pitch_y + 12.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total_w, total_h), Sense::hover());
    let painter = ui.painter();
    let t = time as f32;
    let center_col = (cols as f32 - 1.0) * 0.5;
    let center_row = (rows as f32 - 1.0) * 0.5;
    let dot_r = 2.05_f32;

    for c in 0..cols {
        let nx = (c as f32 - center_col) / center_col.max(1.0); // -1..1
        let h_fade = (1.0 - nx * nx).clamp(0.0, 1.0).powf(1.55);

        let x = rect.left() + 6.0 + c as f32 * pitch_x;
        for r in 0..rows {
            let y = rect.top() + 6.0 + r as f32 * pitch_y;
            let ny = (r as f32 - center_row) / center_row.max(1.0); // -1..1
            let v_fade = (1.0 - (ny * 0.92).powi(2)).clamp(0.0, 1.0).powf(1.1);
            // Elliptical vignette so edges & corners dissolve completely into the card background
            let edge_fade = (h_fade * v_fade).clamp(0.0, 1.0);

            // Normalized elliptical distance from center (0.0 at center -> ~1.0 at perimeter)
            let radial = (nx * nx * 0.85 + ny * ny * 0.65).sqrt();

            // Unlit dot socket: nearly invisible even in center, 0% at edges
            let unlit_base = if active {
                9.0
            } else {
                5.5 + 4.5 * hover_mix
            };
            let unlit_alpha = (unlit_base * edge_fade).round() as u8;
            if unlit_alpha >= 2 {
                let dim_dot = Color32::from_rgba_unmultiplied(
                    p.text3.r(),
                    p.text3.g(),
                    p.text3.b(),
                    unlit_alpha,
                );
                painter.circle_filled(pos2(x, y), dot_r * 0.68, dim_dot);
            }

            // Two-octave smooth 3D organic noise (gives natural non-mechanical variation)
            let n_slow = organic_noise(c as f32 * 0.34, r as f32 * 0.55, t * 0.55);
            let n_med = organic_noise(c as f32 * 0.62 + 11.3, r as f32 * 0.82 + 4.7, t * 1.45);
            let jitter = (n_slow - 0.5) * 0.22 + (n_med - 0.5) * 0.10;
            let org_radial = (radial + jitter).max(0.0);

            let lit_strength = if active {
                // Live recording: soft core heartbeat + real voice-driven center-outward acoustic bloom!
                let base_heart = heartbeat_ring(org_radial, t, 1.75, 0.20) * 0.42;
                let voice_reach = 0.16 + audio_level * 0.98;
                let voice_bloom = (1.0 - (org_radial / voice_reach).powf(1.8)).clamp(0.0, 1.0)
                    * (0.28 + 0.72 * audio_level);
                let voice_ripple = heartbeat_ring(org_radial, t * 1.35, 1.15, 0.22)
                    * audio_level
                    * 0.65;
                let organic_mod = 0.70 + 0.45 * n_med;
                ((base_heart + voice_bloom + voice_ripple) * organic_mod * edge_fade).clamp(0.0, 1.0)
            } else if busy_import {
                // Transcribing: calm center-outward double heartbeat pulse
                let heart = heartbeat_ring(org_radial, t, 2.0, 0.22);
                let core = (1.0 - radial * 1.6).clamp(0.0, 1.0) * 0.22;
                ((heart * (0.68 + 0.45 * n_med) + core) * edge_fade).clamp(0.0, 1.0)
            } else {
                // Idle: whisper-faint rare resting heartbeat; smoothly wakes on hover!
                let rest_heart = heartbeat_ring(org_radial, t, 4.6, 0.18) * 0.26;
                let rest_core = (1.0 - radial * 2.1).clamp(0.0, 1.0) * (0.10 + 0.08 * n_slow);
                let rest = (rest_heart + rest_core) * (0.65 + 0.35 * n_slow);

                let hover_heart = heartbeat_ring(org_radial, t, 2.1, 0.22) * 0.78;
                let hover_embers = ((n_med - 0.46) * 1.4).clamp(0.0, 1.0)
                    * (1.0 - radial * 0.95).clamp(0.0, 1.0)
                    * 0.42;
                let hover_val = hover_heart * (0.72 + 0.40 * n_slow) + hover_embers;

                ((rest * (1.0 - hover_mix) + hover_val * hover_mix) * edge_fade).clamp(0.0, 1.0)
            };

            if lit_strength > 0.04 {
                let base_col = if active {
                    p.rec
                } else if busy_import || radial < 0.42 {
                    p.accent
                } else {
                    p.accent_text
                };
                let max_a = if active {
                    242.0
                } else if busy_import {
                    220.0
                } else {
                    135.0 + 75.0 * hover_mix
                };
                let a = (lit_strength * max_a) as u8;
                if a >= 3 {
                    let col =
                        Color32::from_rgba_unmultiplied(base_col.r(), base_col.g(), base_col.b(), a);

                    if lit_strength > 0.52 {
                        let halo_a = ((lit_strength - 0.48)
                            * if active { 92.0 } else { 54.0 }
                            * edge_fade) as u8;
                        if halo_a >= 2 {
                            painter.circle_filled(
                                pos2(x, y),
                                dot_r * 2.0,
                                Color32::from_rgba_unmultiplied(
                                    base_col.r(),
                                    base_col.g(),
                                    base_col.b(),
                                    halo_a,
                                ),
                            );
                        }
                    }
                    painter.circle_filled(
                        pos2(x, y),
                        dot_r * (0.65 + 0.40 * lit_strength),
                        col,
                    );
                }
            }
        }
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
    if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Draws a continuous dashed rounded rectangle including smooth rounded corner arcs.
pub fn paint_dashed_rect(painter: &Painter, rect: Rect, r: f32, stroke: Stroke, dash: f32, gap: f32) {
    let rect = rect.shrink(stroke.width * 0.5);
    let r = r.min(rect.width() * 0.5).min(rect.height() * 0.5).max(0.0);
    let arc_steps = 8usize;
    let mut pts: Vec<Pos2> = Vec::with_capacity(arc_steps * 4 + 5);

    let mut push_arc = |center: Pos2, start_ang: f32, end_ang: f32| {
        for i in 0..=arc_steps {
            let t = i as f32 / arc_steps as f32;
            let a = start_ang + (end_ang - start_ang) * t;
            pts.push(pos2(center.x + a.cos() * r, center.y + a.sin() * r));
        }
    };

    let pi = std::f32::consts::PI;
    // Top-right -> Bottom-right -> Bottom-left -> Top-left (clockwise)
    push_arc(pos2(rect.right() - r, rect.top() + r), -pi * 0.5, 0.0);
    push_arc(pos2(rect.right() - r, rect.bottom() - r), 0.0, pi * 0.5);
    push_arc(pos2(rect.left() + r, rect.bottom() - r), pi * 0.5, pi);
    push_arc(pos2(rect.left() + r, rect.top() + r), pi, pi * 1.5);
    if let Some(&first) = pts.first() {
        pts.push(first);
    }

    // Walk along the polyline by arc-length and emit clean dashes around corners too!
    let period = (dash + gap).max(1.0);
    let mut dist_in_cycle = 0.0_f32;
    let mut current_dash: Vec<Pos2> = Vec::new();

    for seg in pts.windows(2) {
        let a = seg[0];
        let b = seg[1];
        let delta = b - a;
        let seg_len = delta.length();
        if seg_len <= 1e-4 {
            continue;
        }
        let dir = delta / seg_len;
        let mut cursor = 0.0_f32;

        while cursor < seg_len {
            let in_dash = dist_in_cycle < dash;
            let remaining_in_phase = if in_dash {
                dash - dist_in_cycle
            } else {
                period - dist_in_cycle
            };
            let step = remaining_in_phase.min(seg_len - cursor);
            let p0 = a + dir * cursor;
            let p1 = a + dir * (cursor + step);

            if in_dash {
                if current_dash.is_empty() {
                    current_dash.push(p0);
                }
                current_dash.push(p1);
            }

            cursor += step;
            dist_in_cycle += step;

            if dist_in_cycle >= dash - 1e-4 && in_dash {
                if current_dash.len() >= 2 {
                    painter.add(egui::Shape::line(std::mem::take(&mut current_dash), stroke));
                } else {
                    current_dash.clear();
                }
            }
            if dist_in_cycle >= period - 1e-4 {
                dist_in_cycle = 0.0;
            }
        }
    }
    if current_dash.len() >= 2 {
        painter.add(egui::Shape::line(current_dash, stroke));
    }
}
