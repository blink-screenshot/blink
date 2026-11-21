//! Editor chrome: theme, toolbar with vector icons, property bar, status bar, dialogs.

use super::{EditorApp, EyeTarget, Tool};
use crate::annot::{arrow_geometry, shape_outline, ArrowHead, HeadShape, Kind, RedactMode, ShapeKind, Style};
use crate::i18n::{t, tf};
use crate::platform;
use crate::render::Fonts;
use crate::theme::{self, pal};
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Layout,
    Margin, Painter, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, WidgetInfo,
    WidgetType,
};
use std::sync::Arc;

/// Selected, active and primary things.
pub const ACCENT: Color32 = theme::ACCENT;
/// Text and icons on top of the accent color.
pub const ON_ACCENT: Color32 = theme::INK;

pub const PALETTE: [Color32; 10] = theme::ANNOTATION_COLORS;

/// Radius of a capsule that holds a row of tool buttons. A button inside it is `CAPSULE_MARGIN`
/// in from the edge, so its radius is the capsule's minus that margin: the same "parallel curve"
/// the corner marks in the logo follow inside the tile.
const CAPSULE_RADIUS: u8 = 13;
const CAPSULE_MARGIN: i8 = 3;
const BUTTON_RADIUS: f32 = (CAPSULE_RADIUS as i8 - CAPSULE_MARGIN) as f32;

pub fn setup_style(ctx: &egui::Context, fonts: &Arc<Fonts>) {
    let mut defs = FontDefinitions::default();
    defs.font_data.insert("annot".into(), Arc::new(FontData::from_static(fonts.regular_bytes)));
    defs.font_data.insert("annot_bold".into(), Arc::new(FontData::from_static(fonts.bold_bytes)));
    defs.font_data.insert("annot_italic".into(), Arc::new(FontData::from_static(fonts.italic_bytes)));
    defs.font_data.insert("annot_bold_italic".into(), Arc::new(FontData::from_static(fonts.bold_italic_bytes)));
    defs.font_data.insert("ui_semibold".into(), Arc::new(FontData::from_static(fonts.semibold_bytes)));
    for name in ["annot", "annot_bold", "annot_italic", "annot_bold_italic", "ui_semibold"] {
        defs.families.insert(FontFamily::Name(name.into()), vec![name.into()]);
    }
    // Use the system UI font (Segoe UI on Windows) for the interface as well.
    if cfg!(windows) {
        if let Some(list) = defs.families.get_mut(&FontFamily::Proportional) {
            list.insert(0, "annot".into());
        }
    }
    ctx.set_fonts(defs);

    let p = pal();
    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.panel;
    v.window_fill = p.panel_raised;
    v.extreme_bg_color = p.input_bg;
    v.faint_bg_color = p.group;
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0_f32, ON_ACCENT);
    v.hyperlink_color = if p.dark { theme::ACCENT } else { p.accent_text };
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_stroke = Stroke::new(1.0_f32, p.border);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(theme::radius(30.0));
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.inactive.weak_bg_fill = p.control;
    v.widgets.inactive.bg_fill = p.control_strong;
    v.widgets.hovered.weak_bg_fill = p.control_hover;
    v.widgets.hovered.bg_fill = p.control_hover;
    v.widgets.active.weak_bg_fill = p.control_active;
    v.widgets.active.bg_fill = p.control_active;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, p.text);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, p.text);
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = vec2(6.0, 6.0);
        s.spacing.button_padding = vec2(10.0, 5.0);
        s.spacing.slider_width = 110.0;
        s.spacing.interact_size.y = 26.0;
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(13.5));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(13.5));
        s.text_styles.insert(egui::TextStyle::Small, FontId::proportional(11.5));
    });
}

fn semibold() -> FontFamily {
    FontFamily::Name("ui_semibold".into())
}

// ---- toolbar ------------------------------------------------------------------------------

pub fn toolbar(app: &mut EditorApp, ctx: &egui::Context) {
    let labels = app.config.show_tool_labels;
    egui::TopBottomPanel::top("toolbar")
        .frame(egui::Frame::NONE.fill(pal().panel).inner_margin(Margin::symmetric(10, 7)))
        .show_separator_line(false)
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                // The tools get what the buttons on the right leave over, and wrap onto a second
                // row in a narrow window instead of running under those buttons.
                // The width of the buttons is measured (previous frame), so a longer translation or
                // the "Copied" label cannot make them run over the tools.
                let right_id = egui::Id::new("toolbar_right_width");
                let right = ctx.data(|d| d.get_temp::<f32>(right_id)).unwrap_or(380.0) + 12.0;
                let left_w = (ui.available_width() - right).max(120.0);
                // Room for the text labels of Background / Mockup next to the tools.
                let wide = left_w > if labels { 1000.0 } else { 820.0 };
                ui.allocate_ui_with_layout(
                    vec2(left_w, 32.0),
                    Layout::left_to_right(Align::Center).with_main_wrap(true),
                    |ui| {
                        ui.spacing_mut().item_spacing = vec2(3.0, 4.0);
                        // One rounded capsule per group of related tools.
                        let all = Tool::ALL;
                        let mut i = 0;
                        while i < all.len() {
                            let group = all[i].group();
                            let mut j = i;
                            while j < all.len() && all[j].group() == group {
                                j += 1;
                            }
                            let tools = &all[i..j];
                            i = j;
                            capsule(ui, |ui| {
                                for &tool in tools {
                                    let mut tip = format!("{}  ({})", tool.name(), tool.key().name());
                                    if tool == Tool::Select {
                                        tip.push('\n');
                                        tip.push_str(t("Hold Ctrl to use it temporarily with any tool."));
                                    }
                                    // While Ctrl is held the mouse acts as Select: show that with an outline.
                                    let temporary = app.ctrl_select && tool == Tool::Select && app.tool != Tool::Select;
                                    let label = labels.then(|| tool.short_name());
                                    let resp = icon_button(ui, app.tool == tool, label, tool.name(), |p, r, c| {
                                        draw_tool_icon(p, r, tool, c)
                                    });
                                    if temporary {
                                        ui.painter().rect_stroke(
                                            resp.rect,
                                            BUTTON_RADIUS,
                                            Stroke::new(1.5_f32, ACCENT),
                                            StrokeKind::Inside,
                                        );
                                    }
                                    if resp.on_hover_text(tip).clicked() {
                                        app.set_tool(tool);
                                    }
                                }
                            });
                        }
                        capsule(ui, |ui| {
                            let bg_on = app.frame.background_on;
                            let bg_label = (wide || labels).then(|| t("Background"));
                            if labeled_button(ui, bg_on, bg_label, labels, t("Background"), draw_background_icon)
                                .on_hover_text(t("Background: put the screenshot on a gradient with rounded corners"))
                                .clicked()
                            {
                                app.frame.background_on = !bg_on;
                                app.zoom = None;
                                app.invalidate();
                            }
                            let mock_on = app.frame.mockup_on;
                            let mock_label = (wide || labels).then(|| t("Mockup"));
                            if labeled_button(ui, mock_on, mock_label, labels, t("Device mockup"), draw_mockup_icon)
                                .on_hover_text(t(
                                    "Device mockup: show the screenshot on a phone, laptop, monitor or browser",
                                ))
                                .clicked()
                            {
                                app.frame.mockup_on = !mock_on;
                                app.zoom = None;
                                app.invalidate();
                            }
                        });
                    },
                );
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    let start_x = ui.max_rect().right();
                    ui.spacing_mut().item_spacing.x = 6.0;
                    // The most used action: say clearly that it worked.
                    let copy_text =
                        if app.just_copied() { format!("\u{2714} {}", t("Copied")) } else { t("Copy").to_string() };
                    let copy = ui.add(
                        egui::Button::new(RichText::new(copy_text).color(ON_ACCENT).strong())
                            .fill(ACCENT)
                            .min_size(vec2(84.0, 0.0)),
                    );
                    if copy.on_hover_text(t("Copy to clipboard (Ctrl+C)")).clicked() {
                        app.copy(ctx);
                    }
                    if ui
                        .button(t("Save"))
                        .on_hover_text(tf("Save to {0} (Ctrl+S)", &[&app.config.output_dir.display()]))
                        .clicked()
                    {
                        app.save_quick(ctx);
                    }
                    if ui.button(t("Save as...")).on_hover_text(t("Save as (Ctrl+Shift+S)")).clicked() {
                        app.save_as(ctx);
                    }
                    ui.add_space(6.0);
                    let can_redo = !app.redo.is_empty();
                    let can_undo = !app.undo.is_empty();
                    // `horizontal` inside this right-to-left row lays out right to left too, so redo first.
                    capsule(ui, |ui| {
                        if ui
                            .add_enabled_ui(can_redo, |ui| {
                                icon_button(ui, false, None, t("Redo"), |p, r, c| draw_undo(p, r, c, true))
                            })
                            .inner
                            .on_hover_text(t("Redo (Ctrl+Y)"))
                            .clicked()
                        {
                            app.redo();
                        }
                        if ui
                            .add_enabled_ui(can_undo, |ui| {
                                icon_button(ui, false, None, t("Undo"), |p, r, c| draw_undo(p, r, c, false))
                            })
                            .inner
                            .on_hover_text(t("Undo (Ctrl+Z)"))
                            .clicked()
                        {
                            app.undo();
                        }
                    });
                    let used = start_x - ui.min_rect().left();
                    let old = ctx.data(|d| d.get_temp::<f32>(right_id)).unwrap_or(0.0);
                    if (used - old).abs() > 0.5 {
                        ctx.data_mut(|d| d.insert_temp(right_id, used));
                        ctx.request_repaint();
                    }
                });
            });
        });
}

/// A rounded capsule around a row of related buttons.
fn capsule(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .fill(pal().panel_raised)
        .stroke(Stroke::new(1.0_f32, pal().border.gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(CAPSULE_RADIUS))
        .inner_margin(Margin::same(CAPSULE_MARGIN))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                add(ui);
            });
        });
}

fn accent_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(egui::Button::new(RichText::new(text).color(ON_ACCENT).strong()).fill(ACCENT))
}

/// Background of a toolbar button, and the color to draw its icon in.
fn button_colors(ui: &Ui, resp: &Response, selected: bool) -> (Color32, Color32) {
    let enabled = ui.is_enabled();
    let bg = if selected {
        ACCENT
    } else if (resp.hovered() || resp.has_focus()) && enabled {
        pal().control_hover
    } else {
        Color32::TRANSPARENT
    };
    let fg = if selected {
        ON_ACCENT
    } else if enabled {
        pal().text
    } else {
        pal().disabled
    };
    (bg, fg)
}

/// Gives a custom-drawn button a name for screen readers, and a focus ring for keyboard users.
fn accessible(ui: &Ui, resp: &Response, name: &str, selected: bool) {
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, selected, name));
    if resp.has_focus() {
        ui.painter().rect_stroke(
            resp.rect.expand(1.0),
            BUTTON_RADIUS + 1.0,
            Stroke::new(2.0_f32, pal().text),
            StrokeKind::Outside,
        );
    }
}

/// Toolbar button with a vector icon, and optionally a small label under it.
fn icon_button(
    ui: &mut Ui,
    selected: bool,
    label: Option<&str>,
    name: &str,
    draw: impl FnOnce(&Painter, Rect, Color32),
) -> Response {
    let size = if label.is_some() { vec2(52.0, 46.0) } else { vec2(34.0, 32.0) };
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let (bg, fg) = button_colors(ui, &resp, selected);
    ui.painter().rect_filled(rect, BUTTON_RADIUS, bg);
    match label {
        Some(l) => {
            let icon = Rect::from_center_size(pos2(rect.center().x, rect.top() + 15.0), vec2(18.0, 18.0));
            draw(ui.painter(), icon, fg);
            ui.painter().text(
                pos2(rect.center().x, rect.bottom() - 9.0),
                Align2::CENTER_CENTER,
                l,
                FontId::proportional(10.5),
                fg,
            );
        }
        None => draw(ui.painter(), rect.shrink(8.0), fg),
    }
    accessible(ui, &resp, name, selected);
    resp
}

/// Toolbar button with an icon and the text next to it (or under it, with tool labels on).
fn labeled_button(
    ui: &mut Ui,
    selected: bool,
    label: Option<&str>,
    stacked: bool,
    name: &str,
    draw: impl FnOnce(&Painter, Rect, Color32),
) -> Response {
    let Some(label) = label.filter(|_| !stacked) else {
        return icon_button(ui, selected, label, name, draw);
    };
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(13.0), pal().text);
    let size = vec2(34.0 + galley.size().x + 6.0, 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let (bg, fg) = button_colors(ui, &resp, selected);
    ui.painter().rect_filled(rect, BUTTON_RADIUS, bg);
    draw(ui.painter(), Rect::from_min_size(rect.min, vec2(34.0, 32.0)).shrink(8.0), fg);
    ui.painter().text(
        pos2(rect.left() + 32.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        fg,
    );
    accessible(ui, &resp, name, selected);
    resp
}

fn draw_tool_icon(p: &Painter, r: Rect, tool: Tool, c: Color32) {
    let s = Stroke::new(1.8_f32, c);
    let at = |x: f32, y: f32| pos2(r.min.x + x * r.width(), r.min.y + y * r.height());
    match tool {
        Tool::Select => {
            let pts = vec![
                at(0.2, 0.05),
                at(0.2, 0.85),
                at(0.42, 0.65),
                at(0.58, 0.98),
                at(0.7, 0.92),
                at(0.55, 0.6),
                at(0.82, 0.58),
            ];
            let mut pts = pts;
            pts.push(pts[0]);
            round_line(p, pts, 1.8, c);
        }
        Tool::Rect => {
            p.rect_stroke(Rect::from_min_max(at(0.05, 0.18), at(0.95, 0.82)), 3.5, s, StrokeKind::Middle);
        }
        Tool::Ellipse => {
            let pts: Vec<_> = (0..40)
                .map(|i| {
                    let t = i as f32 / 40.0 * std::f32::consts::TAU;
                    at(0.5 + 0.47 * t.cos(), 0.5 + 0.36 * t.sin())
                })
                .collect();
            p.add(Shape::closed_line(pts, s));
        }
        Tool::Shape => {
            let (pts, _) = shape_outline(ShapeKind::Star, r.expand(1.0));
            p.add(Shape::closed_line(pts, Stroke::new(1.6_f32, c)));
        }
        Tool::Arrow => {
            round_line(p, vec![at(0.08, 0.92), at(0.78, 0.22)], 1.8, c);
            p.add(Shape::convex_polygon(vec![at(0.95, 0.05), at(0.52, 0.18), at(0.82, 0.48)], c, Stroke::NONE));
        }
        Tool::Line => {
            round_line(p, vec![at(0.08, 0.92), at(0.92, 0.08)], 1.8, c);
        }
        Tool::Pen => {
            let pts: Vec<_> = (0..=24)
                .map(|i| {
                    let t = i as f32 / 24.0;
                    at(0.05 + t * 0.9, 0.55 + 0.28 * (t * 9.0).sin() * (1.0 - t * 0.4))
                })
                .collect();
            round_line(p, pts, 1.8, c);
        }
        Tool::Text => {
            p.text(
                r.center() + vec2(0.0, 1.0),
                Align2::CENTER_CENTER,
                "T",
                FontId::new(r.height() * 1.15, FontFamily::Name("annot_bold".into())),
                c,
            );
        }
        Tool::Callout => {
            let pts = crate::annot::callout_outline(
                Rect::from_min_max(at(0.0, 0.08), at(1.0, 0.66)),
                at(0.28, 0.98),
                r.height() * 0.16,
            );
            p.add(Shape::closed_line(pts, Stroke::new(1.6_f32, c)));
            round_line(p, vec![at(0.22, 0.3), at(0.78, 0.3)], 1.4, c);
            round_line(p, vec![at(0.22, 0.46), at(0.6, 0.46)], 1.4, c);
        }
        Tool::Highlight => {
            p.rect_filled(
                Rect::from_min_max(at(0.0, 0.58), at(1.0, 0.9)),
                2.0,
                Color32::from_rgb(250, 204, 21).gamma_multiply(0.85),
            );
            round_line(p, vec![at(0.3, 0.5), at(0.75, 0.05)], 4.0, c);
        }
        Tool::Redact => {
            let n = 4;
            for y in 0..n {
                for x in 0..n {
                    let a = if (x + y) % 2 == 0 { 1.0 } else { 0.45 };
                    let cell = Rect::from_min_size(at(x as f32 / n as f32, y as f32 / n as f32), r.size() / n as f32)
                        .shrink(0.6);
                    p.rect_filled(cell, 1.6, c.gamma_multiply(a));
                }
            }
        }
        Tool::Step => {
            p.circle_stroke(r.center(), r.width() * 0.48, s);
            p.text(
                r.center() + vec2(0.0, 0.5),
                Align2::CENTER_CENTER,
                "1",
                FontId::new(r.height() * 0.75, FontFamily::Name("annot_bold".into())),
                c,
            );
        }
        Tool::Emoji => {
            // A smiley, in the icon color (the real stamps are in color).
            let ctr = r.center();
            let rad = r.width() * 0.48;
            p.circle_stroke(ctr, rad, s);
            p.circle_filled(ctr + vec2(-rad * 0.35, -rad * 0.25), rad * 0.13, c);
            p.circle_filled(ctr + vec2(rad * 0.35, -rad * 0.25), rad * 0.13, c);
            let smile: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = 0.2 * std::f32::consts::PI + 0.6 * std::f32::consts::PI * i as f32 / 12.0;
                    ctr + vec2(a.cos(), a.sin()) * rad * 0.55
                })
                .collect();
            p.add(Shape::line(smile, Stroke::new(1.6_f32, c)));
        }
        Tool::Crop => {
            round_line(p, vec![at(0.25, 0.0), at(0.25, 0.75), at(1.0, 0.75)], 1.8, c);
            round_line(p, vec![at(0.0, 0.25), at(0.75, 0.25), at(0.75, 1.0)], 1.8, c);
        }
        Tool::SelectText => {
            // Four corners around a "T", like text mode on the capture bar.
            for (sx, sy) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                let dx = if sx == 0.0 { 0.28 } else { -0.28 };
                let dy = if sy == 0.0 { 0.28 } else { -0.28 };
                round_line(p, vec![at(sx, sy + dy), at(sx, sy), at(sx + dx, sy)], 1.7, c);
            }
            round_line(p, vec![at(0.3, 0.3), at(0.7, 0.3)], 1.8, c);
            round_line(p, vec![at(0.5, 0.3), at(0.5, 0.74)], 1.8, c);
        }
    }
}

/// Maps a point on a 24 x 24 icon grid (the Lucide / Fluent convention) into `r`.
fn grid(r: Rect) -> impl Fn(f32, f32) -> Pos2 {
    let s = r.width().min(r.height()) / 24.0;
    let o = r.center() - vec2(12.0 * s, 12.0 * s);
    move |x, y| pos2(o.x + x * s, o.y + y * s)
}

/// A polyline with round caps and joins (egui strokes are butt-capped).
pub fn round_line(p: &Painter, pts: Vec<Pos2>, width: f32, c: Color32) {
    for q in &pts {
        p.circle_filled(*q, width / 2.0, c);
    }
    p.add(Shape::line(pts, Stroke::new(width, c)));
}

/// Undo: arrow pointing back, curving around (like Lucide's "undo-2"). Redo is the mirror image.
fn draw_undo(p: &Painter, r: Rect, c: Color32, redo: bool) {
    let g = grid(r.expand(3.0));
    let at = |x: f32, y: f32| if redo { g(24.0 - x, y) } else { g(x, y) };
    let w = r.width() / 9.0;
    round_line(p, vec![at(9.0, 14.0), at(4.0, 9.0), at(9.0, 4.0)], w, c);
    let mut pts = vec![at(4.0, 9.0), at(14.5, 9.0)];
    for i in 1..=16 {
        let t = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 16.0;
        pts.push(at(14.5 + 5.5 * t.cos(), 14.5 + 5.5 * t.sin()));
    }
    pts.push(at(11.0, 20.0));
    round_line(p, pts, w, c);
}

/// A picture on a backdrop, in the icon color like every other icon (the button itself turns
/// accent when the background is on).
fn draw_background_icon(p: &Painter, r: Rect, c: Color32) {
    let g = grid(r.expand(2.0));
    let outer = Rect::from_min_max(g(2.0, 4.0), g(22.0, 20.0));
    p.rect_filled(outer, 3.0, c.gamma_multiply(0.28));
    p.rect_stroke(outer, 3.0, Stroke::new(1.5_f32, c), StrokeKind::Middle);
    let inner = Rect::from_min_max(g(6.5, 8.0), g(17.5, 16.0));
    p.rect_filled(inner, 1.5, c);
}

fn draw_mockup_icon(p: &Painter, r: Rect, c: Color32) {
    let g = grid(r.expand(2.0));
    let s = Stroke::new(1.7_f32, c);
    // Laptop with a phone in front.
    p.rect_stroke(Rect::from_min_max(g(3.0, 5.0), g(17.0, 15.0)), 1.5, s, StrokeKind::Middle);
    round_line(p, vec![g(1.5, 18.0), g(13.0, 18.0)], 1.7, c);
    let phone = Rect::from_min_max(g(15.0, 9.0), g(22.0, 21.0));
    // Knock out the laptop behind the phone with the button's own background.
    let behind = if c == ON_ACCENT { ACCENT } else { pal().panel_raised };
    p.rect_filled(phone, 2.0, behind);
    p.rect_stroke(phone, 2.0, s, StrokeKind::Middle);
    p.line_segment([g(17.5, 11.0), g(19.5, 11.0)], s);
}

// ---- property bar ---------------------------------------------------------------------------

pub fn property_bar(app: &mut EditorApp, ctx: &egui::Context) {
    egui::TopBottomPanel::top("props")
        .frame(egui::Frame::NONE.fill(pal().panel_raised).inner_margin(Margin::symmetric(12, 6)))
        .show_separator_line(false)
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                // The groups wrap onto a second row instead of being cut off in a narrow window.
                let w = (ui.available_width() - 40.0).max(100.0);
                ui.allocate_ui_with_layout(
                    vec2(w, 30.0),
                    Layout::left_to_right(Align::Center).with_main_wrap(true),
                    |ui| {
                        ui.set_min_height(30.0);
                        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                        properties(app, ctx, ui);
                    },
                );
                // The shortcuts overview, always in the same place at the right.
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    let r = ui.add(egui::Button::new(RichText::new("?").strong()).min_size(vec2(28.0, 28.0)));
                    if r.on_hover_text(t("Keyboard shortcuts (F1)")).clicked() {
                        app.show_shortcuts = true;
                    }
                });
            });
        });
}

fn properties(app: &mut EditorApp, ctx: &egui::Context, ui: &mut Ui) {
    // The selected annotation's settings win over the active tool's.
    let selected = app.selected.and_then(|id| app.get(id)).map(|a| (Tool::for_kind(&a.kind), a.style, a.kind.clone()));
    let (tool, style) = match &selected {
        Some((t, s, _)) => (*t, *s),
        None => (app.tool, app.style_for(app.tool)),
    };
    match tool {
        Tool::Select => {
            hint(ui, t("Click an object to select it, drag to move it. Press ? for all shortcuts."));
        }
        Tool::SelectText => text_props(ui, ctx, app),
        Tool::Crop => {
            hint(ui, t("Drag over the image to crop."));
            if app.doc.crop.is_some() && ui.button(t("Reset crop")).clicked() {
                app.checkpoint();
                app.doc.crop = None;
                app.zoom = None;
                app.invalidate();
            }
        }
        Tool::Rect | Tool::Ellipse => {
            group(ui, t("Line"), |ui| {
                color_row(ui, app, tool, style.stroke, false);
                width_slider(ui, app, tool, style);
            });
            group(ui, t("Fill"), |ui| fill_row(ui, app, tool, style.fill));
        }
        Tool::Shape => {
            let current = match &selected {
                Some((_, _, Kind::Shape(k))) => *k,
                _ => app.shape,
            };
            group(ui, t("Shape"), |ui| shape_picker(ui, app, current));
            group(ui, t("Line"), |ui| {
                color_row(ui, app, tool, style.stroke, false);
                width_slider(ui, app, tool, style);
            });
            group(ui, t("Fill"), |ui| fill_row(ui, app, tool, style.fill));
        }
        Tool::Arrow => {
            group(ui, t("Color"), |ui| color_row(ui, app, tool, style.stroke, false));
            group(ui, t("Width"), |ui| width_slider(ui, app, tool, style));
            group(ui, t("Head"), |ui| arrow_heads(ui, app, style));
            group(ui, t("Direction"), |ui| arrow_direction(ui, app, style));
        }
        Tool::Line | Tool::Pen => {
            group(ui, t("Color"), |ui| color_row(ui, app, tool, style.stroke, false));
            group(ui, t("Width"), |ui| width_slider(ui, app, tool, style));
        }
        Tool::Highlight => {
            group(ui, t("Color"), |ui| color_row(ui, app, tool, style.stroke, false));
        }
        Tool::Text => {
            group(ui, t("Color"), |ui| color_row(ui, app, tool, style.stroke, false));
            group(ui, t("Size"), |ui| size_slider(ui, app, tool, style));
            group(ui, t("Style"), |ui| text_style_buttons(ui, app, tool, style, true));
            group(ui, t("Background"), |ui| fill_row(ui, app, tool, style.fill));
        }
        Tool::Callout => {
            group(ui, t("Border"), |ui| {
                color_row(ui, app, tool, style.stroke, false);
                width_slider(ui, app, tool, style);
            });
            group(ui, t("Fill"), |ui| fill_row(ui, app, tool, style.fill));
            group(ui, t("Text"), |ui| {
                size_slider(ui, app, tool, style);
                text_style_buttons(ui, app, tool, style, false);
            });
            hint(ui, t("Drag the round handle to point the arrow."));
        }
        Tool::Step => {
            group(ui, t("Color"), |ui| color_row(ui, app, tool, style.stroke, false));
            group(ui, t("Size"), |ui| size_slider(ui, app, tool, style));
        }
        Tool::Emoji => {
            let current = match &selected {
                Some((_, _, Kind::Emoji(e))) => e.clone(),
                _ => app.emoji.clone(),
            };
            group(ui, t("Stamp"), |ui| emoji_picker(ui, ctx, app, &current));
            group(ui, t("Size"), |ui| stamp_size(ui, app, style));
        }
        Tool::Redact => redact_props(ui, ctx, app, style, selected.as_ref().map(|s| &s.2)),
    }
}

/// A labelled, boxed group of related controls.
pub fn group(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .fill(pal().group)
        .stroke(Stroke::new(1.0_f32, pal().border))
        .corner_radius(CornerRadius::same(theme::radius(36.0)))
        .inner_margin(Margin { left: 10, right: 8, top: 2, bottom: 2 })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(28.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(RichText::new(label).family(semibold()).color(pal().text));
                ui.add_space(6.0);
                add(ui);
            });
        });
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).color(pal().muted));
}

fn color_row(ui: &mut Ui, app: &mut EditorApp, tool: Tool, current: Color32, fill: bool) {
    for c in PALETTE {
        if swatch(ui, c, current == c).clicked() {
            app.apply_style(tool, |s| if fill { s.fill = c } else { s.stroke = c });
        }
    }
    custom_color(ui, app, tool, current, fill);
}

fn fill_row(ui: &mut Ui, app: &mut EditorApp, tool: Tool, current: Color32) {
    let none = current.a() == 0;
    if no_fill_swatch(ui, none).on_hover_text(t("No fill")).clicked() {
        app.apply_style(tool, |s| s.fill = Color32::TRANSPARENT);
    }
    color_row(ui, app, tool, current, true);
}

/// Round "custom color" button (a rainbow ring around the current color) with a popup picker,
/// and an eyedropper that takes a color from the screenshot.
fn custom_color(ui: &mut Ui, app: &mut EditorApp, tool: Tool, current: Color32, fill: bool) {
    let custom = !PALETTE.contains(&current) && current.a() > 0;
    let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
    let p = ui.painter();
    let c = rect.center();
    let n = 36;
    for i in 0..n {
        let a0 = std::f32::consts::TAU * i as f32 / n as f32;
        let a1 = std::f32::consts::TAU * (i + 1) as f32 / n as f32;
        let hue = egui::ecolor::Hsva::new(i as f32 / n as f32, 0.85, 1.0, 1.0);
        p.line_segment(
            [c + vec2(a0.cos(), a0.sin()) * 7.5, c + vec2(a1.cos(), a1.sin()) * 7.5],
            Stroke::new(3.0_f32, Color32::from(hue)),
        );
    }
    if custom {
        p.circle_filled(c, 4.5, current);
    }
    if custom || resp.hovered() {
        let ring = if custom { pal().text } else { pal().muted };
        p.circle_stroke(c, 10.0, Stroke::new(if custom { 2.0_f32 } else { 1.0_f32 }, ring));
    }
    accessible(ui, &resp, t("Custom color"), custom);
    let popup_id = ui.make_persistent_id(("custom_color", tool.id(), fill));
    if resp.clone().on_hover_text(t("Custom color")).clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }
    egui::popup_below_widget(ui, popup_id, &resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.set_min_width(220.0);
        let mut c = if current.a() == 0 { Color32::from_rgba_unmultiplied(255, 255, 255, 160) } else { current };
        let alpha = if fill { egui::color_picker::Alpha::OnlyBlend } else { egui::color_picker::Alpha::Opaque };
        if egui::color_picker::color_picker_color32(ui, &mut c, alpha) {
            app.apply_style(tool, |s| if fill { s.fill = c } else { s.stroke = c });
        }
        ui.add_space(4.0);
        if ui.button(format!("\u{1F4A7} {}", t("Pick from screenshot"))).clicked() {
            ui.memory_mut(|m| m.close_popup());
            app.start_eyedropper(EyeTarget { tool, fill });
        }
    });
    let picking = app.eyedropper == Some(EyeTarget { tool, fill });
    let (r, eye) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    let (bg, fg) = button_colors(ui, &eye, picking);
    ui.painter().rect_filled(r, 5.0, bg);
    draw_eyedropper(ui.painter(), r.shrink(4.0), fg);
    accessible(ui, &eye, t("Eyedropper"), picking);
    if eye.on_hover_text(t("Eyedropper: click in the screenshot to take a color (Esc cancels)")).clicked() {
        if picking {
            app.cancel_eyedropper();
        } else {
            app.start_eyedropper(EyeTarget { tool, fill });
        }
    }
}

fn draw_eyedropper(p: &Painter, r: Rect, c: Color32) {
    let g = grid(r);
    // A pipette: bulb at the top right, glass tube down to the bottom left.
    p.circle_filled(g(17.5, 6.5), r.width() * 0.17, c);
    round_line(p, vec![g(13.0, 8.0), g(16.0, 11.0)], r.width() / 7.0, c);
    round_line(p, vec![g(14.5, 9.5), g(5.5, 18.5)], r.width() / 10.0, c);
    p.circle_filled(g(4.5, 19.5), r.width() / 14.0, c);
}

/// Small square toggle button with a custom-drawn glyph.
fn glyph_toggle(ui: &mut Ui, on: bool, name: &str, draw: impl FnOnce(&Painter, Rect, Color32)) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 26.0), Sense::click());
    let bg = if on {
        ACCENT
    } else if resp.hovered() || resp.has_focus() {
        pal().control_active
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 5.0, bg);
    draw(ui.painter(), rect, if on { ON_ACCENT } else { pal().text });
    accessible(ui, &resp, name, on);
    resp
}

fn arrow_heads(ui: &mut Ui, app: &mut EditorApp, style: Style) {
    for head in ArrowHead::ALL {
        let name = t(head.name());
        let resp = glyph_toggle(ui, style.head == head, name, |p, r, c| {
            draw_arrow_sample(p, r.shrink2(vec2(5.0, 8.0)), c, head, false)
        });
        if resp.on_hover_text(name).clicked() {
            app.apply_style(Tool::Arrow, |s| s.head = head);
        }
    }
}

fn arrow_direction(ui: &mut Ui, app: &mut EditorApp, style: Style) {
    for (both, tip) in [(false, t("Head at the end only")), (true, t("Heads at both ends"))] {
        let resp = glyph_toggle(ui, style.both_ends == both, tip, |p, r, c| {
            draw_arrow_sample(p, r.shrink2(vec2(5.0, 8.0)), c, style.head, both)
        });
        if resp.on_hover_text(tip).clicked() {
            app.apply_style(Tool::Arrow, |s| s.both_ends = both);
        }
    }
}

fn draw_arrow_sample(p: &Painter, r: Rect, c: Color32, head: ArrowHead, both: bool) {
    let a = r.left_center();
    let b = r.right_center();
    // Same geometry as the real arrow, in a tiny size.
    let width = 1.6;
    let ((s0, s1), heads) = arrow_geometry(a, b, width, head, both);
    p.line_segment([s0, s1], Stroke::new(width, c));
    for h in heads {
        match h {
            HeadShape::Fill(pts) => {
                p.add(Shape::convex_polygon(pts, c, Stroke::new(0.8_f32, c)));
            }
            HeadShape::Stroke(pts) => {
                p.add(Shape::line(pts, Stroke::new(width, c)));
            }
            HeadShape::Dot(ctr, rad) => {
                p.circle_filled(ctr, rad, c);
            }
        }
    }
}

fn shape_picker(ui: &mut Ui, app: &mut EditorApp, current: ShapeKind) {
    for k in ShapeKind::ALL {
        let name = t(k.name());
        let resp = glyph_toggle(ui, current == k, name, |p, r, c| {
            let (pts, extra) = shape_outline(k, r.shrink2(vec2(6.0, 6.0)));
            p.add(Shape::closed_line(pts, Stroke::new(1.4_f32, c)));
            for e in extra {
                p.add(Shape::line(e, Stroke::new(1.4_f32, c)));
            }
        });
        if resp.on_hover_text(name).clicked() {
            app.shape = k;
            if let Some(id) = app.selected {
                if matches!(app.get(id).map(|a| &a.kind), Some(Kind::Shape(_))) {
                    app.checkpoint();
                    if let Some(a) = app.get_mut(id) {
                        a.kind = Kind::Shape(k);
                    }
                    app.invalidate();
                }
            }
        }
    }
}

/// The most used stamps inline, all of them in a popup.
fn emoji_picker(ui: &mut Ui, ctx: &egui::Context, app: &mut EditorApp, current: &str) {
    const INLINE: usize = 12;
    let all = crate::emoji::EMOJI;
    let current_pos = all.iter().position(|e| e.0 == current);
    for (i, (id, ..)) in all.iter().enumerate().take(INLINE) {
        emoji_button(ui, ctx, app, id, *id == current);
        // The current stamp stays visible even when it is not one of the first.
        if i == INLINE - 1 {
            if let Some(p) = current_pos.filter(|p| *p >= INLINE) {
                emoji_button(ui, ctx, app, all[p].0, true);
            }
        }
    }
    let resp = ui.add(egui::Button::new(t("More...")).min_size(vec2(0.0, 26.0)));
    let popup_id = ui.make_persistent_id("emoji_more");
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }
    egui::popup_below_widget(ui, popup_id, &resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.set_width(9.0 * 30.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for (id, ..) in all {
                if emoji_button(ui, ctx, app, id, *id == current) {
                    ui.memory_mut(|m| m.close_popup());
                }
            }
        });
    });
}

/// One stamp in the picker; true when clicked.
fn emoji_button(ui: &mut Ui, ctx: &egui::Context, app: &mut EditorApp, id: &str, selected: bool) -> bool {
    let Some(tex) = app.emoji_texture(ctx, id) else { return false };
    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    let bg = if selected {
        ACCENT
    } else if resp.hovered() || resp.has_focus() {
        pal().control_active
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 5.0, bg);
    ui.painter().image(tex, rect.shrink(3.0), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
    let name = crate::emoji::name(id);
    accessible(ui, &resp, name, selected);
    if !resp.on_hover_text(name).clicked() {
        return false;
    }
    app.emoji = id.to_string();
    if let Some(sel) = app.selected {
        if matches!(app.get(sel).map(|a| &a.kind), Some(Kind::Emoji(_))) {
            app.checkpoint();
            if let Some(a) = app.get_mut(sel) {
                a.kind = Kind::Emoji(id.to_string());
            }
            app.invalidate();
        }
    }
    true
}

/// Size of new stamps, and of the selected one (resized around its center).
fn stamp_size(ui: &mut Ui, app: &mut EditorApp, style: Style) {
    let sel = app
        .selected
        .and_then(|id| app.get(id))
        .filter(|a| matches!(a.kind, Kind::Emoji(_)))
        .map(|a| a.rect().width().min(a.rect().height()).round());
    let mut v = sel.unwrap_or(style.font_size);
    if ui.add(egui::Slider::new(&mut v, 16.0..=400.0).step_by(1.0).suffix(" px")).changed() {
        app.styles[Tool::Emoji.index()].font_size = v;
        if let Some(id) = app.selected.filter(|_| sel.is_some()) {
            if let Some(a) = app.get_mut(id) {
                let c = a.rect().center();
                a.a = c - Vec2::splat(v / 2.0);
                a.b = c + Vec2::splat(v / 2.0);
            }
            app.unsaved = true;
            app.invalidate();
        }
    }
}

fn text_style_buttons(ui: &mut Ui, app: &mut EditorApp, tool: Tool, style: Style, lines: bool) {
    let letter = |family: &str, text: &'static str| {
        let family = FontFamily::Name(family.into());
        move |p: &Painter, r: Rect, c: Color32| {
            p.text(r.center() + vec2(0.0, 0.5), Align2::CENTER_CENTER, text, FontId::new(16.0, family), c);
        }
    };
    if glyph_toggle(ui, style.bold, t("Bold"), letter("annot_bold", "B")).on_hover_text(t("Bold")).clicked() {
        app.apply_style(tool, |s| s.bold = !style.bold);
    }
    if glyph_toggle(ui, style.italic, t("Italic"), letter("annot_italic", "I")).on_hover_text(t("Italic")).clicked() {
        app.apply_style(tool, |s| s.italic = !style.italic);
    }
    if !lines {
        return;
    }
    let underline = |p: &Painter, r: Rect, c: Color32| {
        p.text(
            r.center() - vec2(0.0, 1.5),
            Align2::CENTER_CENTER,
            "U",
            FontId::new(15.0, FontFamily::Name("annot".into())),
            c,
        );
        p.line_segment([r.center() + vec2(-5.0, 7.0), r.center() + vec2(5.0, 7.0)], Stroke::new(1.5_f32, c));
    };
    if glyph_toggle(ui, style.underline, t("Underline"), underline).on_hover_text(t("Underline")).clicked() {
        app.apply_style(tool, |s| s.underline = !style.underline);
    }
    let strike = |p: &Painter, r: Rect, c: Color32| {
        p.text(
            r.center() + vec2(0.0, 0.5),
            Align2::CENTER_CENTER,
            "S",
            FontId::new(16.0, FontFamily::Name("annot".into())),
            c,
        );
        p.line_segment([r.center() + vec2(-6.5, 1.0), r.center() + vec2(6.5, 1.0)], Stroke::new(1.5_f32, c));
    };
    if glyph_toggle(ui, style.strike, t("Strikethrough"), strike).on_hover_text(t("Strikethrough")).clicked() {
        app.apply_style(tool, |s| s.strike = !style.strike);
    }
}

fn swatch(ui: &mut Ui, c: Color32, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
    let p = ui.painter();
    if selected {
        p.circle_stroke(rect.center(), 9.5, Stroke::new(2.0_f32, pal().text));
    } else if resp.hovered() || resp.has_focus() {
        p.circle_stroke(rect.center(), 9.5, Stroke::new(1.0_f32, pal().muted));
    }
    p.circle_filled(rect.center(), 7.0, c);
    // Swatches close to the panel color get an edge.
    let bg = pal().panel_raised;
    let close = (c.r() as i32 - bg.r() as i32).abs()
        + (c.g() as i32 - bg.g() as i32).abs()
        + (c.b() as i32 - bg.b() as i32).abs()
        < 90;
    if close {
        p.circle_stroke(rect.center(), 7.0, Stroke::new(1.0_f32, pal().control_strong));
    }
    let [r, g, b, _] = c.to_array();
    let name = format!("#{r:02X}{g:02X}{b:02X}");
    resp.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, selected, &name));
    resp
}

fn no_fill_swatch(ui: &mut Ui, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
    let p = ui.painter();
    if selected {
        p.circle_stroke(rect.center(), 9.5, Stroke::new(2.0_f32, pal().text));
    }
    p.circle_stroke(rect.center(), 6.5, Stroke::new(1.2_f32, pal().muted));
    p.line_segment(
        [rect.center() + vec2(-4.6, 4.6), rect.center() + vec2(4.6, -4.6)],
        Stroke::new(1.5_f32, Color32::from_rgb(239, 68, 68)),
    );
    resp.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, selected, t("No fill")));
    resp
}

fn width_slider(ui: &mut Ui, app: &mut EditorApp, tool: Tool, style: Style) {
    let mut w = style.width;
    if ui.add(egui::Slider::new(&mut w, 0.0..=30.0).step_by(1.0).suffix(" px")).changed() {
        app.apply_style(tool, |s| s.width = w);
    }
}

fn size_slider(ui: &mut Ui, app: &mut EditorApp, tool: Tool, style: Style) {
    let mut v = style.font_size;
    if ui.add(egui::Slider::new(&mut v, 8.0..=160.0).step_by(1.0).suffix(" px")).changed() {
        app.apply_style(tool, |s| s.font_size = v);
    }
}

fn redact_props(ui: &mut Ui, ctx: &egui::Context, app: &mut EditorApp, style: Style, selected_kind: Option<&Kind>) {
    let current = match selected_kind {
        Some(Kind::Redact(m)) => *m,
        _ => app.redact_mode,
    };
    group(ui, t("Mode"), |ui| {
        for (mode, label) in
            [(RedactMode::Blur, t("Blur")), (RedactMode::Pixelate, t("Pixelate")), (RedactMode::Solid, t("Black box"))]
        {
            if ui.selectable_label(current == mode, label).clicked() {
                app.redact_mode = mode;
                if let Some(id) = app.selected {
                    if matches!(app.get(id).map(|a| &a.kind), Some(Kind::Redact(_))) {
                        app.checkpoint();
                        if let Some(a) = app.get_mut(id) {
                            a.kind = Kind::Redact(mode);
                        }
                        app.invalidate();
                    }
                }
            }
        }
    });
    if current != RedactMode::Solid {
        group(ui, t("Strength"), |ui| {
            let mut v = style.strength;
            if ui.add(egui::Slider::new(&mut v, 1..=10)).changed() {
                app.apply_style(Tool::Redact, |s| s.strength = v);
            }
        });
    }
    find_private_button(ui, ctx, app);
    hint(ui, t("Click a word or key to hide it, drag to hide anything else."));
    hint(
        ui,
        if current == RedactMode::Solid {
            t("Fully removes the content.")
        } else {
            t("Content is destroyed, not just softened. For passwords/keys a black box is the 100% safe choice.")
        },
    );
}

fn find_private_button(ui: &mut Ui, ctx: &egui::Context, app: &mut EditorApp) {
    if ui
        .button(t("Find private info"))
        .on_hover_text(t(
            "Reads the text and suggests what to hide: e-mail addresses, phone numbers, IBANs, card numbers, \
             keys and passwords",
        ))
        .clicked()
    {
        app.find_private(ctx);
    }
}

/// The text tool: what it is doing, and buttons for the whole text.
fn text_props(ui: &mut Ui, ctx: &egui::Context, app: &mut EditorApp) {
    use super::textsel::OcrJob;
    match &app.ocr {
        OcrJob::Idle | OcrJob::Running(_) => {
            ui.add(egui::Spinner::new().size(16.0));
            hint(ui, t("Reading text\u{2026}"));
            return;
        }
        OcrJob::Failed(e) => {
            let msg = e.message();
            let no_language = *e == crate::ocr::OcrError::NoLanguage;
            ui.label(RichText::new(msg).color(pal().warn));
            if no_language && ui.button(t("Language settings")).clicked() {
                platform::open_language_settings();
            }
            return;
        }
        OcrJob::Ready(page) if page.is_empty() => {
            hint(ui, t("No text found in this image."));
            find_private_button(ui, ctx, app);
            return;
        }
        OcrJob::Ready(_) => {}
    }
    group(ui, t("Text"), |ui| {
        if ui.button(t("Copy all")).on_hover_text(t("Copy all text in the image")).clicked() {
            app.copy_all_text(ctx);
        }
        if ui.button(t("Select all")).on_hover_text("Ctrl+A").clicked() {
            app.select_all_text(ctx);
        }
        let any = !app.text_sel.is_empty();
        if ui.add_enabled(any, egui::Button::new(t("Copy selection"))).on_hover_text("Ctrl+C").clicked() {
            app.copy_selected_text();
        }
        if ui
            .add_enabled(any, egui::Button::new(t("Hide selection")))
            .on_hover_text(t("Cover the selected text with the redaction tool's mode"))
            .clicked()
        {
            app.redact_selected_text();
        }
    });
    find_private_button(ui, ctx, app);
    hint(ui, t("Drag over text to select it, or drag a box around it. Double-click: line, triple-click: paragraph."));
}

// ---- status bar ----------------------------------------------------------------------------

pub fn status_bar(app: &mut EditorApp, ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("status")
        .frame(egui::Frame::NONE.fill(pal().panel).inner_margin(Margin::symmetric(12, 3)))
        .show_separator_line(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let muted = pal().muted;
                let v = app.view_rect();
                let size = match (app.frame_layout(), app.frame.aspect.fixed_size()) {
                    (Some(_), Some((w, h))) if app.frame.background_on => tf("{0} x {1} px (framed)", &[&w, &h]),
                    (Some(l), _) => tf("{0} x {1} px (framed)", &[&l.size.x, &l.size.y]),
                    _ => format!("{} x {} px", v.width(), v.height()),
                };
                ui.label(RichText::new(size).color(muted).small());
                status_divider(ui);
                // Zoom looks like what it is: a button.
                let pct = app.last_zoom * ctx.pixels_per_point() * 100.0;
                let zoom_resp = ui
                    .add(egui::Button::new(RichText::new(format!("{pct:.0}%")).small()).min_size(vec2(48.0, 20.0)))
                    .on_hover_text(t("Click: toggle fit / 100%  (Ctrl+0 / Ctrl+1)"));
                if zoom_resp.clicked() {
                    app.zoom = if app.zoom.is_none() { Some(1.0 / ctx.pixels_per_point()) } else { None };
                    app.pan = Vec2::ZERO;
                }
                if let Some(h) = app.hover {
                    if app.image_rect().contains(h) {
                        status_divider(ui);
                        ui.label(RichText::new(format!("{:.0}, {:.0}", h.x.floor(), h.y.floor())).color(muted).small());
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let mut open_folder = None;
                    if let Some(p) = &app.last_saved {
                        if ui.link(RichText::new(t("Show in folder")).small()).clicked() {
                            open_folder = p.parent().map(|d| d.to_path_buf());
                        }
                    } else if app.unsaved {
                        ui.label(RichText::new(t("Not copied or saved yet")).color(muted).small());
                    }
                    if app.eyedropper.is_some() {
                        ui.label(
                            RichText::new(t("Click in the screenshot to take its color. Esc cancels."))
                                .color(pal().accent_text)
                                .small(),
                        );
                    }
                    if let Some(dir) = open_folder {
                        platform::open_folder(&dir);
                    }
                });
            });
        });
}

fn status_divider(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(1.0, 14.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, pal().border);
}

/// Messages (copied, saved, errors) as a pill at the bottom center of the canvas.
pub fn toast_overlay(app: &mut EditorApp, ctx: &egui::Context) {
    let Some(toast) = &app.toast else { return };
    let age = toast.at.elapsed().as_secs_f32();
    let alpha = if age > 3.4 { ((4.0 - age) / 0.6).clamp(0.0, 1.0) } else { 1.0 };
    let area = ctx.available_rect();
    let (fill, fg) = if toast.error { (pal().error, Color32::WHITE) } else { (theme::INK, Color32::WHITE) };
    egui::Area::new(egui::Id::new("toast"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -(ctx.screen_rect().bottom() - area.bottom()) - 18.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(fill.gamma_multiply(alpha))
                .stroke(Stroke::new(1.0_f32, if toast.error { fill } else { ACCENT }.gamma_multiply(alpha)))
                .corner_radius(CornerRadius::same(18))
                .inner_margin(Margin::symmetric(16, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if !toast.error {
                            ui.label(RichText::new("\u{2714}").color(ACCENT.gamma_multiply(alpha)));
                        }
                        ui.label(RichText::new(&toast.text).color(fg.gamma_multiply(alpha)));
                    });
                });
        });
}

// ---- dialogs ---------------------------------------------------------------------------------

fn dim(ctx: &egui::Context) {
    let screen = ctx.screen_rect();
    ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("dim"))).rect_filled(
        screen,
        0.0,
        Color32::from_black_alpha(120),
    );
}

/// A centered modal window. The dim layer and a default Window are both Order::Middle, and
/// egui may paint the dim layer on top: Foreground keeps the dialog above it.
fn dialog(ctx: &egui::Context, title: &str, add: impl FnOnce(&mut Ui)) {
    dim(ctx);
    egui::Window::new(title)
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, add);
}

pub fn confirm_close_dialog(app: &mut EditorApp, ctx: &egui::Context) {
    dialog(ctx, t("Close editor?"), |ui| {
        ui.label(t("Your annotations have not been copied or saved."));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if accent_button(ui, t("Copy & close")).clicked() {
                app.confirm_close = false;
                app.copy(ctx);
                app.close_now(ctx);
            }
            if ui.button(t("Save & close")).clicked() {
                app.confirm_close = false;
                app.save_quick(ctx);
                if !app.unsaved {
                    app.close_now(ctx);
                }
            }
            if ui.button(t("Discard")).clicked() {
                app.close_now(ctx);
            }
            if ui.button(t("Cancel")).clicked() {
                app.confirm_close = false;
            }
        });
    });
}

/// Asked when Esc would close the editor.
pub fn confirm_escape_dialog(app: &mut EditorApp, ctx: &egui::Context) {
    let mut answer = None;
    dialog(ctx, t("Close the editor?"), |ui| {
        ui.label(t("You pressed Esc. Do you want to close the editor?"));
        ui.add_space(8.0);
        ui.checkbox(&mut app.dont_ask_escape, t("Don't ask me again"));
        ui.label(
            RichText::new(t("You can turn this question back on in Settings > Editor.")).small().color(pal().muted),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if accent_button(ui, t("Close editor")).on_hover_text(t("Enter")).clicked() {
                answer = Some(true);
            }
            if ui.button(t("Keep editing")).on_hover_text(t("Esc")).clicked() {
                answer = Some(false);
            }
        });
    });
    if let Some(close) = answer {
        app.answer_escape(ctx, close);
    }
}

/// Overview of every keyboard shortcut (F1 or the ? button).
pub fn shortcuts_window(app: &mut EditorApp, ctx: &egui::Context) {
    let mut open = true;
    dim(ctx);
    egui::Window::new(t("Keyboard shortcuts"))
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            let section = |ui: &mut Ui, title: &str| {
                ui.add_space(6.0);
                ui.label(RichText::new(title).family(semibold()).color(pal().text));
            };
            ui.columns(2, |cols| {
                section(&mut cols[0], t("Tools"));
                egui::Grid::new("keys_tools").num_columns(2).spacing([14.0, 3.0]).show(&mut cols[0], |ui| {
                    for tool in Tool::ALL {
                        key_label(ui, tool.key().name());
                        ui.label(tool.name());
                        ui.end_row();
                    }
                });
                section(&mut cols[1], t("Editing"));
                let rows: [(&str, &str); 19] = [
                    ("Ctrl", t("Hold: select / move with any tool")),
                    ("Shift", t("While drawing: straight lines, squares")),
                    ("Del", t("Delete the selection")),
                    (t("Arrows"), t("Nudge the selection (Shift: 10 px)")),
                    ("Esc", t("Deselect, or close the editor")),
                    ("Ctrl+Z", t("Undo")),
                    ("Ctrl+Y", t("Redo")),
                    ("Ctrl+C", t("Copy to clipboard")),
                    ("Ctrl+A", t("Select all text (OCR tool)")),
                    ("Ctrl+S", t("Save to the screenshots folder")),
                    ("Ctrl+Shift+S", t("Save as")),
                    (t("Space+drag"), t("Pan")),
                    (t("Middle mouse"), t("Pan")),
                    (t("Ctrl+wheel"), t("Zoom")),
                    ("Ctrl+0", t("Fit to window")),
                    ("Ctrl+1", t("Actual size (100%)")),
                    ("Ctrl+B", t("Show or hide the background panel")),
                    (t("Double-click"), t("Edit text")),
                    ("F1", t("This overview")),
                ];
                egui::Grid::new("keys_edit").num_columns(2).spacing([14.0, 3.0]).show(&mut cols[1], |ui| {
                    for (k, what) in rows {
                        key_label(ui, k);
                        ui.label(what);
                        ui.end_row();
                    }
                });
            });
        });
    if !open {
        app.show_shortcuts = false;
    }
}

fn key_label(ui: &mut Ui, key: &str) {
    egui::Frame::NONE
        .fill(pal().control)
        .stroke(Stroke::new(1.0_f32, pal().border))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(key).monospace().small().color(pal().text));
        });
}

// ---- brand ---------------------------------------------------------------------------------

/// The app logo as a texture, rendered at twice the 44 points it is shown at so it stays sharp.
pub fn logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let img = image::load_from_memory(include_bytes!("../../assets/blink.png")).expect("embedded logo").to_rgba8();
    let img = image::imageops::resize(&img, 96, 96, image::imageops::FilterType::Lanczos3);
    let color = egui::ColorImage::from_rgba_unmultiplied([96, 96], img.as_raw());
    ctx.load_texture("blink_logo", color, egui::TextureOptions::LINEAR)
}

/// Logo, name and a line of text: the header of the settings and update windows.
pub fn brand_header(ui: &mut Ui, logo: &egui::TextureHandle, title: &str, subtitle: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(44.0, 44.0)));
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(RichText::new(title).family(semibold()).size(20.0).color(pal().text));
            ui.label(RichText::new(subtitle).color(pal().muted));
        });
    });
}
