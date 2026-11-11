//! Tema e componentes visuais do DiscoLab. Os ícones são desenhados com vetores
//! porque as fontes embutidas (e as do Android) não têm emojis/símbolos.

use discolab_core::health::Severity;
use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Shape, Stroke, TextStyle, Vec2, pos2, vec2};

pub const BG: Color32 = Color32::from_rgb(13, 15, 20);
pub const PANEL: Color32 = Color32::from_rgb(19, 22, 29);
pub const CARD: Color32 = Color32::from_rgb(25, 29, 38);
pub const CARD_HI: Color32 = Color32::from_rgb(33, 38, 50);
pub const BORDER: Color32 = Color32::from_rgb(40, 46, 60);
pub const TRACK: Color32 = Color32::from_rgb(48, 54, 70);
pub const TEXT: Color32 = Color32::from_rgb(230, 234, 242);
pub const MUTED: Color32 = Color32::from_rgb(140, 148, 166);
pub const ACCENT: Color32 = Color32::from_rgb(64, 196, 255);
pub const GREEN: Color32 = Color32::from_rgb(52, 211, 153);
pub const YELLOW: Color32 = Color32::from_rgb(250, 204, 21);
pub const ORANGE: Color32 = Color32::from_rgb(251, 146, 60);
pub const RED: Color32 = Color32::from_rgb(248, 87, 87);
pub const BLUE: Color32 = Color32::from_rgb(96, 165, 250);

pub fn apply_theme(ctx: &egui::Context, mobile: bool) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(18);
    v.menu_corner_radius = CornerRadius::same(12);
    v.extreme_bg_color = Color32::from_rgb(10, 12, 16);
    v.faint_bg_color = CARD_HI;
    v.code_bg_color = CARD_HI;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.override_text_color = None;
    v.slider_trailing_fill = true;
    for (w, fill) in [
        (&mut v.widgets.noninteractive, CARD),
        (&mut v.widgets.inactive, CARD_HI),
        (&mut v.widgets.hovered, Color32::from_rgb(42, 49, 64)),
        (&mut v.widgets.active, Color32::from_rgb(50, 58, 76)),
        (&mut v.widgets.open, Color32::from_rgb(42, 49, 64)),
    ] {
        w.corner_radius = CornerRadius::same(10);
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.6));
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        let k = if mobile { 1.08 } else { 1.0 };
        s.text_styles.insert(TextStyle::Heading, FontId::proportional(22.0 * k));
        s.text_styles.insert(TextStyle::Body, FontId::proportional(14.5 * k));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(14.5 * k));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(11.5 * k));
        s.text_styles.insert(TextStyle::Monospace, FontId::monospace(12.5 * k));
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(12.0, if mobile { 9.0 } else { 6.0 });
        s.spacing.interact_size.y = if mobile { 38.0 } else { 26.0 };
        s.spacing.slider_width = if mobile { 110.0 } else { 160.0 };
        s.spacing.combo_width = 120.0;
        s.spacing.scroll.floating = true;
        s.spacing.scroll.bar_width = 6.0;
    });
}

pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(16)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

pub fn section_title(ui: &mut egui::Ui, t: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(t.to_uppercase()).small().strong().color(MUTED));
    ui.add_space(4.0);
}

pub fn pill(ui: &mut egui::Ui, t: &str, c: Color32) {
    egui::Frame::new()
        .fill(c.gamma_multiply(0.16))
        .corner_radius(20)
        .inner_margin(egui::Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(t).small().strong().color(c));
        });
}

pub fn legend_dot(ui: &mut egui::Ui, c: Color32, t: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), 4.5, c);
    ui.label(RichText::new(t).small().color(MUTED));
}

pub fn stat_grid(ui: &mut egui::Ui, tiles: &[(&str, String, Color32)], cols: usize) {
    let gap = 8.0;
    let w = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
    for row in tiles.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (label, value, color) in row {
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(14)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(w - 26.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(*label).small().color(MUTED));
                            ui.add(egui::Label::new(RichText::new(value).size(18.0).strong().color(*color)).truncate());
                        });
                    });
            }
        });
        ui.add_space(gap - 6.0);
    }
}

pub fn kv_card(ui: &mut egui::Ui, id: &str, rows: &[(&str, String, Option<Color32>)]) {
    card(ui, |ui| {
        egui::Grid::new(id).num_columns(2).striped(true).spacing([24.0, 6.0]).show(ui, |ui| {
            for (k, v, c) in rows {
                ui.label(RichText::new(*k).color(MUTED));
                ui.label(RichText::new(v).strong().color(c.unwrap_or(TEXT)));
                ui.end_row();
            }
        });
    });
    ui.add_space(10.0);
}

pub fn sev_color(s: Severity) -> Color32 {
    match s {
        Severity::Critical => RED,
        Severity::Warning => ORANGE,
        Severity::Notice => YELLOW,
        Severity::Ok => GREEN,
        Severity::Info => MUTED,
    }
}

pub fn finding_card(ui: &mut egui::Ui, sev: Severity, title: &str, detail: &str, penalty: f32) {
    let c = sev_color(sev);
    let r = egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(14)
        .inner_margin(egui::Margin { left: 18, right: 14, top: 12, bottom: 12 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                pill(ui, sev.label(), c);
                ui.label(RichText::new(title).strong());
                if penalty > 0.0 {
                    ui.label(RichText::new(format!("-{penalty:.0} pts")).small().color(MUTED));
                }
            });
            ui.add_space(2.0);
            ui.label(RichText::new(detail).color(MUTED));
        })
        .response;
    let stripe = Rect::from_min_max(r.rect.left_top() + vec2(0.0, 10.0), pos2(r.rect.left() + 4.0, r.rect.bottom() - 10.0));
    ui.painter().rect_filled(stripe, 2, c);
}

pub fn empty_state(ui: &mut egui::Ui, icon: Icon, title: &str, text: &str) {
    card(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(12.0);
            let (r, _) = ui.allocate_exact_size(Vec2::splat(56.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 28.0, ACCENT.gamma_multiply(0.12));
            paint_icon(ui.painter(), r.shrink(14.0), icon, ACCENT);
            ui.add_space(8.0);
            ui.label(RichText::new(title).size(17.0).strong());
            ui.label(RichText::new(text).color(MUTED));
            ui.add_space(12.0);
        });
    });
}

pub fn brand(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::splat(30.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 9, ACCENT);
    paint_icon(ui.painter(), r.shrink(6.0), Icon::Disk, BG);
    ui.label(RichText::new("DiscoLab").size(19.0).strong().color(TEXT));
}

fn button(ui: &mut egui::Ui, text: &str, fill: Color32, fg: Color32, enabled: bool, full: bool) -> egui::Response {
    let b = egui::Button::new(RichText::new(text).strong().color(fg)).fill(fill).corner_radius(12).min_size(vec2(0.0, 40.0));
    if full {
        ui.add_enabled(enabled, b.min_size(vec2(ui.available_width(), 44.0)))
    } else {
        ui.add_enabled(enabled, b)
    }
}

pub fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let full = ui.available_width() < 520.0;
    button(ui, text, ACCENT, BG, enabled, full)
}

pub fn danger_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let full = ui.available_width() < 520.0;
    button(ui, text, RED, TEXT, true, full)
}

pub fn secondary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    button(ui, text, CARD_HI, TEXT, enabled, false)
}

pub fn icon_button(ui: &mut egui::Ui, icon: Icon, tip: &str, busy: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(38.0), egui::Sense::click());
    let fill = if resp.hovered() { CARD_HI } else { CARD };
    ui.painter().rect_filled(r, 10, fill);
    if busy {
        egui::Spinner::new().size(18.0).color(ACCENT).paint_at(ui, r.shrink(10.0));
    } else {
        paint_icon(ui.painter(), r.shrink(10.0), icon, TEXT);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip));
    resp.on_hover_text(tip)
}

pub fn tab_chip(ui: &mut egui::Ui, icon: Icon, label: &str, selected: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(14.5), TEXT);
    let size = vec2(galley.size().x + 52.0, 38.0);
    let (r, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let (fill, fg) = if selected {
        (ACCENT.gamma_multiply(0.18), ACCENT)
    } else if resp.hovered() {
        (CARD, TEXT)
    } else {
        (Color32::TRANSPARENT, MUTED)
    };
    ui.painter().rect_filled(r, 12, fill);
    paint_icon(ui.painter(), Rect::from_center_size(pos2(r.left() + 22.0, r.center().y), Vec2::splat(16.0)), icon, fg);
    ui.painter().text(pos2(r.left() + 38.0, r.center().y), egui::Align2::LEFT_CENTER, label, FontId::proportional(14.5), fg);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, label));
    resp
}

pub fn nav_item(ui: &mut egui::Ui, w: f32, icon: Icon, label: &str, selected: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(w, 58.0), egui::Sense::click());
    let fg = if selected { ACCENT } else { MUTED };
    let ic = Rect::from_center_size(pos2(r.center().x, r.top() + 20.0), vec2(48.0, 28.0));
    if selected {
        ui.painter().rect_filled(ic, 14, ACCENT.gamma_multiply(0.18));
    }
    paint_icon(ui.painter(), Rect::from_center_size(ic.center(), Vec2::splat(18.0)), icon, fg);
    ui.painter().text(pos2(r.center().x, r.top() + 46.0), egui::Align2::CENTER_CENTER, label, FontId::proportional(11.5), fg);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, label));
    resp
}

pub fn bucket_color(b: u8) -> Color32 {
    match b {
        0 => Color32::from_rgb(70, 80, 98),
        1 => Color32::from_rgb(105, 116, 138),
        2 => Color32::from_rgb(150, 160, 180),
        3 => GREEN,
        4 => ORANGE,
        5 => RED,
        6 => Color32::from_rgb(168, 85, 247),
        _ => Color32::from_rgb(32, 37, 48),
    }
}

// ------------------------------------------------------------------ ícones

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Heart,
    List,
    Speed,
    Grid,
    Doc,
    Disk,
    Ssd,
    Usb,
    Chip,
    Refresh,
}

/// Arco de `t0` a `t1` (frações de volta, 0 = topo, sentido horário).
pub fn arc(p: &egui::Painter, c: Pos2, r: f32, t0: f32, t1: f32, stroke: Stroke) {
    let n = ((t1 - t0).abs() * 96.0).max(2.0) as usize;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let t = t0 + (t1 - t0) * i as f32 / n as f32;
            let a = -std::f32::consts::FRAC_PI_2 + t * std::f32::consts::TAU;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    p.add(Shape::line(pts, stroke));
}

pub fn paint_icon(p: &egui::Painter, r: Rect, icon: Icon, c: Color32) {
    let w = r.width();
    let s = Stroke::new((w * 0.11).max(1.6), c);
    let at = |x: f32, y: f32| pos2(r.left() + x * w, r.top() + y * r.height());
    match icon {
        Icon::Heart => {
            let rad = w * 0.25;
            p.circle_filled(at(0.29, 0.36), rad, c);
            p.circle_filled(at(0.71, 0.36), rad, c);
            p.add(Shape::convex_polygon(vec![at(0.06, 0.45), at(0.94, 0.45), at(0.5, 0.92)], c, Stroke::NONE));
        }
        Icon::List => {
            for (i, y) in [0.2, 0.5, 0.8].iter().enumerate() {
                p.circle_filled(at(0.1, *y), w * 0.08, c);
                p.line_segment([at(0.3, *y), at(if i == 1 { 0.8 } else { 0.95 }, *y)], s);
            }
        }
        Icon::Speed => {
            let ctr = at(0.5, 0.68);
            let n = 24;
            let pts: Vec<Pos2> = (0..=n)
                .map(|i| {
                    let a = std::f32::consts::PI + std::f32::consts::PI * i as f32 / n as f32;
                    ctr + vec2(a.cos(), a.sin()) * w * 0.46
                })
                .collect();
            p.add(Shape::line(pts, s));
            p.line_segment([ctr, at(0.78, 0.36)], s);
            p.circle_filled(ctr, w * 0.09, c);
        }
        Icon::Grid => {
            let g = w * 0.27;
            for i in 0..3 {
                for j in 0..3 {
                    let rr = Rect::from_min_size(at(0.0, 0.0) + vec2(i as f32 * (g + w * 0.095), j as f32 * (g + w * 0.095)), Vec2::splat(g));
                    p.rect_filled(rr, 2, c);
                }
            }
        }
        Icon::Doc => {
            let rr = Rect::from_min_max(at(0.15, 0.02), at(0.85, 0.98));
            p.rect_stroke(rr, 3, s, egui::StrokeKind::Middle);
            for y in [0.32, 0.52, 0.72] {
                p.line_segment([at(0.3, y), at(0.7, y)], Stroke::new(s.width * 0.8, c));
            }
        }
        Icon::Disk => {
            let rr = Rect::from_min_max(at(0.08, 0.02), at(0.92, 0.98));
            p.rect_stroke(rr, 4, s, egui::StrokeKind::Middle);
            p.circle_stroke(at(0.5, 0.42), w * 0.22, s);
            p.circle_filled(at(0.5, 0.42), w * 0.06, c);
            p.line_segment([at(0.5, 0.42), at(0.76, 0.82)], Stroke::new(s.width * 0.8, c));
        }
        Icon::Ssd => {
            let rr = Rect::from_min_max(at(0.0, 0.22), at(1.0, 0.78));
            p.rect_stroke(rr, 3, s, egui::StrokeKind::Middle);
            for x in [0.18, 0.42] {
                p.rect_filled(Rect::from_min_max(at(x, 0.36), at(x + 0.16, 0.64)), 1, c);
            }
            for x in [0.72, 0.8, 0.88] {
                p.line_segment([at(x, 0.36), at(x, 0.64)], Stroke::new(s.width * 0.6, c));
            }
        }
        Icon::Usb => {
            p.line_segment([at(0.5, 0.12), at(0.5, 0.86)], s);
            p.add(Shape::convex_polygon(vec![at(0.5, 0.0), at(0.38, 0.18), at(0.62, 0.18)], c, Stroke::NONE));
            p.line_segment([at(0.5, 0.6), at(0.22, 0.42)], s);
            p.line_segment([at(0.22, 0.42), at(0.22, 0.3)], s);
            p.circle_filled(at(0.22, 0.26), w * 0.08, c);
            p.line_segment([at(0.5, 0.52), at(0.78, 0.36)], s);
            p.rect_filled(Rect::from_center_size(at(0.78, 0.3), Vec2::splat(w * 0.15)), 1, c);
            p.circle_filled(at(0.5, 0.9), w * 0.1, c);
        }
        Icon::Chip => {
            let rr = Rect::from_min_max(at(0.2, 0.2), at(0.8, 0.8));
            p.rect_stroke(rr, 3, s, egui::StrokeKind::Middle);
            p.rect_filled(rr.shrink(w * 0.17), 1, c);
            for t in [0.35, 0.5, 0.65] {
                let thin = Stroke::new(s.width * 0.7, c);
                p.line_segment([at(t, 0.0), at(t, 0.2)], thin);
                p.line_segment([at(t, 0.8), at(t, 1.0)], thin);
                p.line_segment([at(0.0, t), at(0.2, t)], thin);
                p.line_segment([at(0.8, t), at(1.0, t)], thin);
            }
        }
        Icon::Refresh => {
            let ctr = r.center();
            arc(p, ctr, w * 0.4, 0.08, 0.86, s);
            let a = -std::f32::consts::FRAC_PI_2 + 0.08 * std::f32::consts::TAU;
            let tip = ctr + vec2(a.cos(), a.sin()) * w * 0.4;
            p.add(Shape::convex_polygon(vec![tip + vec2(w * 0.2, -w * 0.02), tip + vec2(-w * 0.08, -w * 0.2), tip + vec2(-w * 0.04, w * 0.16)], c, Stroke::NONE));
        }
    }
}
