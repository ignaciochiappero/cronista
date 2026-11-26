//! Crisp, thin-stroke vector icons and the circular Cronista mark painted natively via `egui::Painter`.

use crate::ui::theme::ThemePalette;
use egui::{pos2, vec2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke};
use std::f32::consts::PI;

#[derive(Debug, Clone, Copy)]
pub enum Icon {
    Search,
    Folder,
    Play,
    Copy,
    Doc,
    Pdf,
    Captions,
    Trash,
    Pencil,
    Palette,
    Help,
    Mic,
    Gauge,
    Upload,
    Monitor,
    Waveform,
    Check,
    Alert,
    WinMinimize,
    WinMaximize,
    WinRestore,
    WinClose,
    Download,
}

/// Paints the exact circular Cronista logo: left coral-red semicircle + 4 clean right bars
/// all cleanly clipped inside a true circle.
pub fn paint_cronista_logo(painter: &Painter, center: Pos2, radius: f32, p: &ThemePalette) {
    // Left half-circle (coral red)
    let segments = 24;
    let mut left_pts = Vec::with_capacity(segments + 2);
    let split_x = center.x - radius * 0.04;
    for i in 0..=segments {
        let angle = PI * 0.5 + (i as f32 / segments as f32) * PI; // top -> left -> bottom
        let pt = pos2(center.x + radius * angle.cos(), center.y + radius * angle.sin());
        left_pts.push(pos2(pt.x.min(split_x), pt.y));
    }
    painter.add(Shape::convex_polygon(left_pts, p.rec, Stroke::NONE));

    // Right 4 transcript bars trimmed to circular arc
    let bar_x0 = center.x + radius * 0.14;
    let bar_h = radius * 0.28;
    let ys = [-0.72_f32, -0.24, 0.24, 0.72];
    for &ny in &ys {
        let cy = center.y + ny * radius;
        let max_dx = (radius * radius - (ny * radius).powi(2)).max(0.0).sqrt();
        let x1 = (center.x + max_dx).max(bar_x0 + 2.0);
        let rect = Rect::from_min_max(pos2(bar_x0, cy - bar_h * 0.5), pos2(x1, cy + bar_h * 0.5));
        painter.rect_filled(rect, CornerRadius::same((bar_h * 0.35).max(1.0) as u8), p.text);
    }
}

/// Paints a crisp 1.4px vector icon centered inside `rect`.
pub fn paint_icon(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    let o = rect.center() - vec2(s * 0.5, s * 0.5);
    let pt = |nx: f32, ny: f32| pos2(o.x + nx * s, o.y + ny * s);
    let stroke = Stroke::new((s * 0.095).clamp(1.2, 1.8), color);

    match icon {
        Icon::Search => {
            painter.circle_stroke(pt(0.45, 0.45), s * 0.27, stroke);
            painter.line_segment([pt(0.66, 0.66), pt(0.86, 0.86)], stroke);
        }
        Icon::Folder => {
            let pts = vec![
                pt(0.14, 0.28),
                pt(0.40, 0.28),
                pt(0.49, 0.38),
                pt(0.86, 0.38),
                pt(0.86, 0.78),
                pt(0.14, 0.78),
                pt(0.14, 0.28),
            ];
            painter.add(Shape::line(pts, stroke));
        }
        Icon::Play => {
            let pts = vec![pt(0.32, 0.22), pt(0.78, 0.50), pt(0.32, 0.78), pt(0.32, 0.22)];
            painter.add(Shape::line(pts, stroke));
        }
        Icon::Copy => {
            let back = Rect::from_min_max(pt(0.18, 0.18), pt(0.64, 0.64));
            let front = Rect::from_min_max(pt(0.36, 0.36), pt(0.82, 0.82));
            painter.rect_stroke(back, CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
            painter.rect_stroke(front, CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
        }
        Icon::Doc => {
            let pts = vec![
                pt(0.24, 0.14),
                pt(0.60, 0.14),
                pt(0.78, 0.32),
                pt(0.78, 0.86),
                pt(0.24, 0.86),
                pt(0.24, 0.14),
            ];
            painter.add(Shape::line(pts, stroke));
            painter.line_segment([pt(0.36, 0.52), pt(0.66, 0.52)], stroke);
            painter.line_segment([pt(0.36, 0.68), pt(0.58, 0.68)], stroke);
        }
        Icon::Pdf => {
            let pts = vec![
                pt(0.24, 0.14),
                pt(0.60, 0.14),
                pt(0.78, 0.32),
                pt(0.78, 0.86),
                pt(0.24, 0.86),
                pt(0.24, 0.14),
            ];
            painter.add(Shape::line(pts, stroke));
            painter.line_segment([pt(0.50, 0.44), pt(0.50, 0.72)], stroke);
            painter.line_segment([pt(0.40, 0.62), pt(0.50, 0.72)], stroke);
            painter.line_segment([pt(0.60, 0.62), pt(0.50, 0.72)], stroke);
        }
        Icon::Captions => {
            let box_r = Rect::from_min_max(pt(0.14, 0.24), pt(0.86, 0.76));
            painter.rect_stroke(box_r, CornerRadius::same(3), stroke, egui::StrokeKind::Inside);
            painter.line_segment([pt(0.28, 0.46), pt(0.44, 0.46)], stroke);
            painter.line_segment([pt(0.52, 0.46), pt(0.72, 0.46)], stroke);
            painter.line_segment([pt(0.28, 0.60), pt(0.50, 0.60)], stroke);
            painter.line_segment([pt(0.58, 0.60), pt(0.72, 0.60)], stroke);
        }
        Icon::Trash => {
            painter.line_segment([pt(0.20, 0.28), pt(0.80, 0.28)], stroke);
            painter.line_segment([pt(0.38, 0.18), pt(0.62, 0.18)], stroke);
            let pts = vec![pt(0.28, 0.28), pt(0.32, 0.84), pt(0.68, 0.84), pt(0.72, 0.28)];
            painter.add(Shape::line(pts, stroke));
        }
        Icon::Pencil => {
            let pts = vec![
                pt(0.20, 0.80),
                pt(0.25, 0.62),
                pt(0.68, 0.19),
                pt(0.81, 0.32),
                pt(0.38, 0.75),
                pt(0.20, 0.80),
            ];
            painter.add(Shape::line(pts, stroke));
        }
        Icon::Palette => {
            painter.circle_stroke(pt(0.50, 0.50), s * 0.35, stroke);
            for (nx, ny) in [(0.36, 0.46), (0.48, 0.33), (0.64, 0.42)] {
                painter.circle_filled(pt(nx, ny), (s * 0.055).max(1.0), color);
            }
        }
        Icon::Help => {
            painter.circle_stroke(pt(0.50, 0.50), s * 0.36, stroke);
            let q = vec![pt(0.40, 0.39), pt(0.45, 0.32), pt(0.56, 0.33), pt(0.59, 0.42), pt(0.50, 0.52), pt(0.50, 0.58)];
            painter.add(Shape::line(q, stroke));
            painter.circle_filled(pt(0.50, 0.70), (s * 0.055).max(1.0), color);
        }
        Icon::Mic => {
            let cap = Rect::from_min_max(pt(0.36, 0.14), pt(0.64, 0.58));
            painter.rect_stroke(cap, CornerRadius::same((s * 0.14) as u8), stroke, egui::StrokeKind::Inside);
            // U arc
            let mut arc = Vec::new();
            for i in 0..=12 {
                let a = (i as f32 / 12.0) * PI;
                arc.push(pos2(
                    o.x + (0.50 + 0.25 * a.cos()) * s,
                    o.y + (0.48 + 0.24 * a.sin()) * s,
                ));
            }
            painter.add(Shape::line(arc, stroke));
            painter.line_segment([pt(0.50, 0.72), pt(0.50, 0.88)], stroke);
        }
        Icon::Gauge => {
            let mut arc = Vec::new();
            for i in 0..=14 {
                let a = PI + (i as f32 / 14.0) * PI;
                arc.push(pos2(
                    o.x + (0.50 + 0.33 * a.cos()) * s,
                    o.y + (0.68 + 0.33 * a.sin()) * s,
                ));
            }
            painter.add(Shape::line(arc, stroke));
            painter.line_segment([pt(0.50, 0.68), pt(0.66, 0.46)], stroke);
        }
        Icon::Upload => {
            painter.line_segment([pt(0.50, 0.64), pt(0.50, 0.20)], stroke);
            painter.line_segment([pt(0.33, 0.36), pt(0.50, 0.20)], stroke);
            painter.line_segment([pt(0.67, 0.36), pt(0.50, 0.20)], stroke);
            let tray = vec![pt(0.18, 0.62), pt(0.18, 0.82), pt(0.82, 0.82), pt(0.82, 0.62)];
            painter.add(Shape::line(tray, stroke));
        }
        Icon::Monitor => {
            let scr = Rect::from_min_max(pt(0.14, 0.20), pt(0.86, 0.68));
            painter.rect_stroke(scr, CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
            painter.line_segment([pt(0.50, 0.68), pt(0.50, 0.82)], stroke);
            painter.line_segment([pt(0.32, 0.82), pt(0.68, 0.82)], stroke);
        }
        Icon::Waveform => {
            for (nx, h) in [(0.18, 0.16), (0.34, 0.42), (0.50, 0.66), (0.66, 0.46), (0.82, 0.22)] {
                painter.line_segment([pt(nx, 0.50 - h * 0.5), pt(nx, 0.50 + h * 0.5)], stroke);
            }
        }
        Icon::Check => {
            let pts = vec![pt(0.20, 0.52), pt(0.42, 0.74), pt(0.82, 0.30)];
            painter.add(Shape::line(pts, stroke));
        }
        Icon::Alert => {
            painter.circle_stroke(pt(0.50, 0.50), s * 0.36, stroke);
            painter.line_segment([pt(0.50, 0.32), pt(0.50, 0.54)], stroke);
            painter.circle_filled(pt(0.50, 0.68), (s * 0.055).max(1.0), color);
        }
        Icon::WinMinimize => {
            painter.line_segment([pt(0.14, 0.52), pt(0.86, 0.52)], stroke);
        }
        Icon::WinMaximize => {
            let r = Rect::from_min_max(pt(0.16, 0.16), pt(0.84, 0.84));
            painter.rect_stroke(r, CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
        }
        Icon::WinRestore => {
            let front = Rect::from_min_max(pt(0.14, 0.30), pt(0.72, 0.86));
            painter.rect_stroke(front, CornerRadius::same(2), stroke, egui::StrokeKind::Inside);
            let back = vec![pt(0.30, 0.30), pt(0.30, 0.14), pt(0.86, 0.14), pt(0.86, 0.72), pt(0.72, 0.72)];
            painter.add(Shape::line(back, stroke));
        }
        Icon::WinClose => {
            painter.line_segment([pt(0.18, 0.18), pt(0.82, 0.82)], stroke);
            painter.line_segment([pt(0.82, 0.18), pt(0.18, 0.82)], stroke);
        }
        Icon::Download => {
            painter.line_segment([pt(0.50, 0.20), pt(0.50, 0.64)], stroke);
            painter.line_segment([pt(0.33, 0.48), pt(0.50, 0.64)], stroke);
            painter.line_segment([pt(0.67, 0.48), pt(0.50, 0.64)], stroke);
            let tray = vec![pt(0.18, 0.66), pt(0.18, 0.82), pt(0.82, 0.82), pt(0.82, 0.66)];
            painter.add(Shape::line(tray, stroke));
        }
    }
}
