//! Refined 10-theme palette system using pre-blended hairline borders and soft surface tints
//! so native `egui` renders with the same razor-sharp minimalism as Linear / Raycast / Cronista.

use egui::{Color32, CornerRadius, Stroke, Style, Visuals};

#[derive(Debug, Clone, Copy)]
pub struct ThemePalette {
    pub id: &'static str,
    pub is_dark: bool,
    pub bg: Color32,
    pub surface: Color32,
    pub surface2: Color32,
    pub surface3: Color32,
    /// Ultra-subtle 1px hairline border (pre-blended, never harsh gray).
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text2: Color32,
    pub text3: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub accent_text: Color32,
    pub rec: Color32,
    pub rec_soft: Color32,
    pub rec_text: Color32,
    pub danger: Color32,
    pub danger_soft: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub mic: Color32,
    pub mic_bg: Color32,
    pub computer: Color32,
    pub computer_bg: Color32,
    pub mark: Color32,
}

/// Blends foreground RGB over background RGB by alpha `0.0..=1.0` into an opaque `Color32`.
const fn mix(bg: (u8, u8, u8), fg: (u8, u8, u8), alpha_pct: u16) -> Color32 {
    let inv = 100 - alpha_pct;
    let r = ((bg.0 as u16 * inv + fg.0 as u16 * alpha_pct) / 100) as u8;
    let g = ((bg.1 as u16 * inv + fg.1 as u16 * alpha_pct) / 100) as u8;
    let b = ((bg.2 as u16 * inv + fg.2 as u16 * alpha_pct) / 100) as u8;
    Color32::from_rgb(r, g, b)
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub fn palette_for(id: &str, system_dark: bool) -> ThemePalette {
    let dark_base = ThemePalette {
        id: "dark",
        is_dark: true,
        bg: rgb(0x0b, 0x0c, 0x0f),
        surface: rgb(0x11, 0x12, 0x16),
        surface2: rgb(0x17, 0x18, 0x1d),
        surface3: rgb(0x1f, 0x20, 0x27),
        border: mix((0x11, 0x12, 0x16), (255, 255, 255), 7),
        border_strong: mix((0x11, 0x12, 0x16), (255, 255, 255), 13),
        text: rgb(0xec, 0xec, 0xef),
        text2: rgb(0xa3, 0xa4, 0xad),
        text3: rgb(0x6e, 0x6f, 0x79),
        accent: rgb(0xff, 0x5a, 0x4e),
        accent_soft: mix((0x11, 0x12, 0x16), (0xff, 0x5a, 0x4e), 13),
        accent_text: rgb(0xff, 0x8a, 0x7f),
        rec: rgb(0xff, 0x5a, 0x4e),
        rec_soft: mix((0x11, 0x12, 0x16), (0xff, 0x5a, 0x4e), 14),
        rec_text: rgb(0xff, 0x8a, 0x7f),
        danger: rgb(0xff, 0x7b, 0x72),
        danger_soft: mix((0x11, 0x12, 0x16), (0xff, 0x5a, 0x4e), 13),
        ok: rgb(0x3d, 0xdc, 0x97),
        warn: rgb(0xf5, 0xb7, 0x4f),
        mic: rgb(0xff, 0x8a, 0x7f),
        mic_bg: mix((0x11, 0x12, 0x16), (0xff, 0x5a, 0x4e), 14),
        computer: rgb(0x8a, 0xb4, 0xff),
        computer_bg: mix((0x11, 0x12, 0x16), (0x6e, 0xa0, 0xff), 15),
        mark: mix((0x17, 0x18, 0x1d), (0xff, 0xc8, 0x57), 28),
    };

    let light_base = ThemePalette {
        id: "light",
        is_dark: false,
        bg: rgb(0xf6, 0xf6, 0xf7),
        surface: rgb(0xff, 0xff, 0xff),
        surface2: rgb(0xf1, 0xf1, 0xf3),
        surface3: rgb(0xe8, 0xe8, 0xec),
        border: mix((0xff, 0xff, 0xff), (0x0c, 0x0d, 0x12), 8),
        border_strong: mix((0xff, 0xff, 0xff), (0x0c, 0x0d, 0x12), 14),
        text: rgb(0x11, 0x12, 0x16),
        text2: rgb(0x57, 0x58, 0x64),
        text3: rgb(0x8b, 0x8c, 0x96),
        accent: rgb(0xf2, 0x3b, 0x4c),
        accent_soft: mix((0xff, 0xff, 0xff), (0xf2, 0x3b, 0x4c), 10),
        accent_text: rgb(0xd8, 0x32, 0x3f),
        rec: rgb(0xf2, 0x3b, 0x4c),
        rec_soft: mix((0xff, 0xff, 0xff), (0xf2, 0x3b, 0x4c), 10),
        rec_text: rgb(0xd8, 0x32, 0x3f),
        danger: rgb(0xc4, 0x2b, 0x36),
        danger_soft: mix((0xff, 0xff, 0xff), (0xf2, 0x3b, 0x4c), 10),
        ok: rgb(0x1e, 0xa8, 0x6b),
        warn: rgb(0xd9, 0x8a, 0x12),
        mic: rgb(0xd8, 0x32, 0x3f),
        mic_bg: mix((0xff, 0xff, 0xff), (0xf2, 0x3b, 0x4c), 10),
        computer: rgb(0x2f, 0x63, 0xc9),
        computer_bg: mix((0xff, 0xff, 0xff), (0x2f, 0x63, 0xc9), 10),
        mark: mix((0xf1, 0xf1, 0xf3), (0xff, 0xc4, 0x00), 34),
    };

    match id {
        "system" => {
            if system_dark {
                ThemePalette { id: "system", ..dark_base }
            } else {
                ThemePalette { id: "system", ..light_base }
            }
        }
        "dark" => dark_base,
        "light" => light_base,
        "midnight" => ThemePalette {
            id: "midnight",
            bg: rgb(0x07, 0x0b, 0x17),
            surface: rgb(0x0c, 0x12, 0x22),
            surface2: rgb(0x12, 0x1a, 0x2e),
            surface3: rgb(0x19, 0x22, 0x3a),
            border: mix((0x0c, 0x12, 0x22), (0x8c, 0xaa, 0xff), 10),
            border_strong: mix((0x0c, 0x12, 0x22), (0x8c, 0xaa, 0xff), 19),
            text: rgb(0xe6, 0xec, 0xff),
            text2: rgb(0x9a, 0xa6, 0xc4),
            text3: rgb(0x66, 0x71, 0x8f),
            accent: rgb(0x5b, 0x8c, 0xff),
            accent_soft: mix((0x0c, 0x12, 0x22), (0x5b, 0x8c, 0xff), 16),
            accent_text: rgb(0x8f, 0xb0, 0xff),
            mic: rgb(0xff, 0x9a, 0x9a),
            mic_bg: mix((0x0c, 0x12, 0x22), (0xff, 0x6e, 0x6e), 15),
            computer: rgb(0x7c, 0xc4, 0xff),
            computer_bg: mix((0x0c, 0x12, 0x22), (0x5a, 0xaa, 0xff), 17),
            ..dark_base
        },
        "ocean" => ThemePalette {
            id: "ocean",
            bg: rgb(0x05, 0x13, 0x16),
            surface: rgb(0x0a, 0x1c, 0x20),
            surface2: rgb(0x0f, 0x26, 0x2b),
            surface3: rgb(0x16, 0x32, 0x38),
            border: mix((0x0a, 0x1c, 0x20), (0x78, 0xe6, 0xdc), 9),
            border_strong: mix((0x0a, 0x1c, 0x20), (0x78, 0xe6, 0xdc), 18),
            text: rgb(0xe3, 0xf6, 0xf4),
            text2: rgb(0x95, 0xb9, 0xb6),
            text3: rgb(0x5f, 0x85, 0x82),
            accent: rgb(0x2d, 0xd4, 0xbf),
            accent_soft: mix((0x0a, 0x1c, 0x20), (0x2d, 0xd4, 0xbf), 15),
            accent_text: rgb(0x6e, 0xe7, 0xd8),
            mic: rgb(0xff, 0xa9, 0x8a),
            mic_bg: mix((0x0a, 0x1c, 0x20), (0xff, 0x8c, 0x64), 15),
            computer: rgb(0x7d, 0xd3, 0xfc),
            computer_bg: mix((0x0a, 0x1c, 0x20), (0x7d, 0xd3, 0xfc), 15),
            ..dark_base
        },
        "forest" => ThemePalette {
            id: "forest",
            bg: rgb(0x0a, 0x12, 0x0d),
            surface: rgb(0x0f, 0x1a, 0x13),
            surface2: rgb(0x15, 0x23, 0x1a),
            surface3: rgb(0x1c, 0x2d, 0x22),
            border: mix((0x0f, 0x1a, 0x13), (0xa0, 0xe6, 0xb4), 8),
            border_strong: mix((0x0f, 0x1a, 0x13), (0xa0, 0xe6, 0xb4), 17),
            text: rgb(0xe6, 0xf2, 0xe9),
            text2: rgb(0x9b, 0xb8, 0xa3),
            text3: rgb(0x64, 0x80, 0x6c),
            accent: rgb(0x4a, 0xde, 0x80),
            accent_soft: mix((0x0f, 0x1a, 0x13), (0x4a, 0xde, 0x80), 14),
            accent_text: rgb(0x86, 0xef, 0xac),
            mic: rgb(0xfc, 0xa5, 0xa5),
            mic_bg: mix((0x0f, 0x1a, 0x13), (0xf8, 0x71, 0x71), 15),
            computer: rgb(0x93, 0xc5, 0xfd),
            computer_bg: mix((0x0f, 0x1a, 0x13), (0x93, 0xc5, 0xfd), 15),
            ..dark_base
        },
        "grape" => ThemePalette {
            id: "grape",
            bg: rgb(0x11, 0x0c, 0x1a),
            surface: rgb(0x17, 0x10, 0x24),
            surface2: rgb(0x1f, 0x16, 0x30),
            surface3: rgb(0x28, 0x1d, 0x3d),
            border: mix((0x17, 0x10, 0x24), (0xd2, 0xaa, 0xff), 9),
            border_strong: mix((0x17, 0x10, 0x24), (0xd2, 0xaa, 0xff), 18),
            text: rgb(0xf1, 0xe9, 0xff),
            text2: rgb(0xb3, 0xa3, 0xcc),
            text3: rgb(0x7a, 0x6b, 0x93),
            accent: rgb(0xe8, 0x79, 0xf9),
            accent_soft: mix((0x17, 0x10, 0x24), (0xe8, 0x79, 0xf9), 15),
            accent_text: rgb(0xf0, 0xab, 0xfc),
            mic: rgb(0xfd, 0xa4, 0xaf),
            mic_bg: mix((0x17, 0x10, 0x24), (0xfb, 0x71, 0x85), 16),
            computer: rgb(0xa5, 0xb4, 0xfc),
            computer_bg: mix((0x17, 0x10, 0x24), (0xa5, 0xb4, 0xfc), 16),
            ..dark_base
        },
        "sunset" => ThemePalette {
            id: "sunset",
            bg: rgb(0x14, 0x0d, 0x0a),
            surface: rgb(0x1c, 0x13, 0x0f),
            surface2: rgb(0x24, 0x19, 0x14),
            surface3: rgb(0x2e, 0x20, 0x1a),
            border: mix((0x1c, 0x13, 0x0f), (0xff, 0xbe, 0x96), 9),
            border_strong: mix((0x1c, 0x13, 0x0f), (0xff, 0xbe, 0x96), 18),
            text: rgb(0xfb, 0xee, 0xe6),
            text2: rgb(0xc4, 0xa8, 0x98),
            text3: rgb(0x8a, 0x70, 0x64),
            accent: rgb(0xff, 0x9a, 0x52),
            accent_soft: mix((0x1c, 0x13, 0x0f), (0xff, 0x9a, 0x52), 15),
            accent_text: rgb(0xff, 0xbd, 0x8a),
            mic: rgb(0xff, 0x9a, 0x9a),
            mic_bg: mix((0x1c, 0x13, 0x0f), (0xff, 0x6e, 0x6e), 16),
            computer: rgb(0x93, 0xc5, 0xfd),
            computer_bg: mix((0x1c, 0x13, 0x0f), (0x93, 0xc5, 0xfd), 15),
            ..dark_base
        },
        "paper" => ThemePalette {
            id: "paper",
            bg: rgb(0xf4, 0xee, 0xe3),
            surface: rgb(0xfb, 0xf8, 0xf1),
            surface2: rgb(0xef, 0xe7, 0xd8),
            surface3: rgb(0xe6, 0xdc, 0xc9),
            border: mix((0xfb, 0xf8, 0xf1), (0x3c, 0x28, 0x14), 10),
            border_strong: mix((0xfb, 0xf8, 0xf1), (0x3c, 0x28, 0x14), 19),
            text: rgb(0x2b, 0x21, 0x18),
            text2: rgb(0x6b, 0x5a, 0x48),
            text3: rgb(0x9a, 0x8a, 0x77),
            accent: rgb(0xc2, 0x41, 0x0c),
            accent_soft: mix((0xfb, 0xf8, 0xf1), (0xc2, 0x41, 0x0c), 11),
            accent_text: rgb(0xb3, 0x36, 0x0a),
            mic: rgb(0xb3, 0x36, 0x0a),
            mic_bg: mix((0xfb, 0xf8, 0xf1), (0xc2, 0x41, 0x0c), 11),
            computer: rgb(0x1d, 0x5f, 0xa8),
            computer_bg: mix((0xfb, 0xf8, 0xf1), (0x1d, 0x5f, 0xa8), 11),
            mark: mix((0xef, 0xe7, 0xd8), (0xff, 0xc4, 0x00), 36),
            ..light_base
        },
        "contrast" => ThemePalette {
            id: "contrast",
            bg: Color32::BLACK,
            surface: Color32::BLACK,
            surface2: rgb(0x0d, 0x0d, 0x0d),
            surface3: rgb(0x1c, 0x1c, 0x1c),
            border: rgb(0x6a, 0x6a, 0x6a),
            border_strong: Color32::WHITE,
            text: Color32::WHITE,
            text2: rgb(0xf2, 0xf2, 0xf2),
            text3: rgb(0xcf, 0xcf, 0xcf),
            accent: rgb(0xff, 0xd6, 0x0a),
            accent_soft: mix((0, 0, 0), (0xff, 0xd6, 0x0a), 20),
            accent_text: rgb(0xff, 0xe4, 0x5c),
            mic: rgb(0xff, 0xd6, 0x0a),
            mic_bg: mix((0, 0, 0), (0xff, 0xd6, 0x0a), 18),
            computer: rgb(0x7c, 0xf0, 0xff),
            computer_bg: mix((0, 0, 0), (0x7c, 0xf0, 0xff), 18),
            mark: mix((0x0d, 0x0d, 0x0d), (0xff, 0xd6, 0x0a), 45),
            ..dark_base
        },
        _ => dark_base,
    }
}

pub fn apply_to_egui(ctx: &egui::Context, p: &ThemePalette) {
    let mut visuals = if p.is_dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    visuals.panel_fill = p.bg;
    visuals.window_fill = p.surface;
    visuals.extreme_bg_color = p.surface2;
    visuals.faint_bg_color = p.surface2;
    visuals.window_stroke = Stroke::new(1.0_f32, p.border_strong);
    visuals.window_corner_radius = CornerRadius::same(14);
    visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 14],
        blur: 32,
        spread: 0,
        color: Color32::from_black_alpha(if p.is_dark { 120 } else { 32 }),
    };
    visuals.popup_shadow = visuals.window_shadow;
    visuals.override_text_color = Some(p.text);

    visuals.widgets.noninteractive.bg_fill = p.surface;
    visuals.widgets.noninteractive.weak_bg_fill = p.surface;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text2);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(8);

    visuals.widgets.inactive.bg_fill = p.surface2;
    visuals.widgets.inactive.weak_bg_fill = p.surface2;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(8);

    visuals.widgets.hovered.bg_fill = p.surface3;
    visuals.widgets.hovered.weak_bg_fill = p.surface3;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.border_strong);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, p.text);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(8);

    visuals.widgets.active.bg_fill = p.surface3;
    visuals.widgets.active.weak_bg_fill = p.surface3;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, p.text);
    visuals.widgets.active.corner_radius = CornerRadius::same(8);

    visuals.widgets.open.bg_fill = p.surface3;
    visuals.widgets.open.weak_bg_fill = p.surface3;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, p.border_strong);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, p.text);
    visuals.widgets.open.corner_radius = CornerRadius::same(8);

    visuals.selection.bg_fill = p.accent_soft;
    visuals.selection.stroke = Stroke::new(1.0_f32, p.accent);

    let mut style = Style::default();
    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.combo_height = 240.0;
    style.spacing.scroll = egui::style::ScrollStyle {
        bar_width: 6.0,
        floating: true,
        ..egui::style::ScrollStyle::thin()
    };
    ctx.set_style(style);
}
