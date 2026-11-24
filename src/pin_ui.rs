//! A pinned screenshot (`Blink --pin-raw <file>`): the screenshot in a small window without a
//! frame that stays above other windows, to keep something in view while you work (a
//! value to type over, a design to compare with, instructions to follow).
//!
//! The bar on top: stay on top (remembered for the next pin), Copy, Save as, Edit, the
//! opacity (Windows) and close. Drag anywhere to move it, `Ctrl` + scroll or the corner
//! grip to resize, double-click for 100%. Like the editor it is its own process: every pin
//! is independent, and closing one never touches the tray.

use crate::config::{config_dir, Config};
use crate::i18n::{t, tf};
use crate::imageio;
use crate::platform;
use crate::theme::{self, pal};
use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, Rect, RichText, Sense, Stroke};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

/// The height of the bar, in points.
const BAR: f32 = 34.0;
/// The height of the bar on a screen with `ppp` pixels per point: a whole number of pixels,
/// so the image under it starts on a pixel and 100% is sharp.
fn bar_height(ppp: f32) -> f32 {
    (BAR * ppp).round() / ppp
}

/// Opacities offered (Windows only).
const OPACITIES: [u8; 4] = [100, 80, 60, 40];

/// What carries over to the next pin. Stored as JSON in %APPDATA%\Blink\pin.json (not in
/// config.json, which the tray and the settings window write).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PinPrefs {
    pub on_top: bool,
}

impl Default for PinPrefs {
    fn default() -> Self {
        Self { on_top: true }
    }
}

fn prefs_path() -> std::path::PathBuf {
    config_dir().join("pin.json")
}

impl PinPrefs {
    pub fn load() -> Self {
        std::fs::read_to_string(prefs_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Best effort: a failed write only means the next pin starts on top.
    pub fn save(&self) {
        let Ok(json) = serde_json::to_string_pretty(self) else { return };
        if std::fs::create_dir_all(config_dir()).is_err() {
            return;
        }
        let tmp = prefs_path().with_extension(format!("json.{}.tmp", std::process::id()));
        if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, prefs_path()).is_err() {
            std::fs::remove_file(&tmp).ok();
        }
    }
}

/// The window size (points) that shows an image of `w` x `h` pixels at 100% under a bar,
/// shrunk to fit `max` (points) if it is bigger, and never narrower than the bar needs.
/// `ppp`: pixels per point of the screen. Returns the size and the zoom.
pub fn initial_size(w: u32, h: u32, ppp: f32, max: egui::Vec2, min_w: f32) -> (egui::Vec2, f32) {
    let img = vec2(w as f32, h as f32) / ppp;
    let bar = bar_height(ppp);
    let fit = ((max.x / img.x).min((max.y - bar) / img.y)).min(1.0);
    let shown = img * fit;
    (vec2(shown.x.max(min_w), shown.y + bar), fit)
}

pub fn run(img: RgbaImage, at: Option<(i32, i32)>) -> Result<(), String> {
    if img.width() == 0 || img.height() == 0 {
        return Err(t("The image is empty.").into());
    }
    let mon = at.and_then(|(x, y)| platform::monitor_info_at(x, y)).or_else(platform::cursor_monitor);
    let ppp = mon.map_or(1.0, |m| m.scale);
    let max = mon.map_or(vec2(1600.0, 1000.0), |m| vec2(m.work_width as f32, m.work_height as f32) / ppp * 0.9);
    let (size, _) = initial_size(img.width(), img.height(), ppp, max, min_width());
    let prefs = PinPrefs::load();
    let level = if prefs.on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(tf("Pinned screenshot {0} \u{d7} {1}", &[&img.width(), &img.height()]))
        .with_inner_size(size)
        .with_min_inner_size([min_width(), BAR + 40.0])
        .with_decorations(false)
        .with_resizable(true)
        .with_window_level(level)
        .with_app_id("blink-pin");
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/blink.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, centered: at.is_none(), ..Default::default() };
    eframe::run_native(
        "Blink pin",
        options,
        Box::new(move |cc| {
            crate::editor::setup_theme(&cc.egui_ctx);
            let hwnd = window_handle(cc);
            if let Some(h) = hwnd {
                platform::place_pin_window(h, at, (bar_height(ppp) * ppp).round() as i32);
            }
            let tex = cc.egui_ctx.load_texture(
                "pin",
                egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()),
                egui::TextureOptions::LINEAR,
            );
            Ok(Box::new(PinApp { img, tex, prefs, hwnd, opacity: 100, toast: None }))
        }),
    )
    .map_err(|e| e.to_string())
}

/// Narrowest window: room for the bar.
fn min_width() -> f32 {
    if cfg!(windows) {
        360.0
    } else {
        300.0
    }
}

fn window_handle(cc: &eframe::CreationContext<'_>) -> Option<isize> {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(h) = cc.window_handle() {
            if let RawWindowHandle::Win32(w) = h.as_raw() {
                return Some(w.hwnd.get());
            }
        }
    }
    let _ = cc;
    None
}

struct PinApp {
    img: RgbaImage,
    tex: egui::TextureHandle,
    prefs: PinPrefs,
    hwnd: Option<isize>,
    /// Percent.
    opacity: u8,
    toast: Option<(String, Instant, bool)>,
}

impl PinApp {
    fn set_on_top(&mut self, ctx: &egui::Context, on: bool) {
        self.prefs.on_top = on;
        let level = if on { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
        // The next pin starts the same way.
        self.prefs.save();
    }

    fn set_opacity(&mut self, pct: u8) {
        self.opacity = pct;
        if let Some(h) = self.hwnd {
            platform::set_window_opacity(h, (pct as u32 * 255 / 100) as u8);
        }
    }

    fn say(&mut self, text: String, error: bool) {
        self.toast = Some((text, Instant::now(), error));
    }

    fn copy(&mut self) {
        match imageio::copy_to_clipboard(&self.img) {
            Ok(()) => self.say(t("Copied to clipboard").into(), false),
            Err(e) => self.say(tf("Copy failed: {0}", &[&e]), true),
        }
    }

    fn save_as(&mut self) {
        let c = Config::load();
        let Some(mut path) = imageio::ask_save_path(&c) else { return };
        if path.extension().is_none() {
            path.set_extension(c.format.ext());
        }
        let fmt = imageio::format_from_path(&path, c.format);
        match imageio::save_image(&path, &self.img, fmt, c.jpg_quality) {
            Ok(()) => self.say(tf("Saved {0}", &[&path.display()]), false),
            Err(e) => self.say(e, true),
        }
    }

    /// Opens the screenshot in the editor; the pin stays.
    fn edit(&mut self) {
        let path = imageio::temp_raw_path();
        let result = imageio::write_raw(&path, &self.img).map_err(|e| e.to_string()).and_then(|()| {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            std::process::Command::new(exe).arg("--edit-raw").arg(&path).spawn().map(|_| ()).map_err(|e| e.to_string())
        });
        if let Err(e) = result {
            std::fs::remove_file(&path).ok();
            self.say(tf("Could not open the editor: {0}", &[&e]), true);
        }
    }

    /// Resizes the window to show the image at `zoom` (1 = one image pixel per screen pixel).
    fn zoom_to(&self, ctx: &egui::Context, zoom: f32) {
        let ppp = ctx.pixels_per_point();
        let img = vec2(self.img.width() as f32, self.img.height() as f32) / ppp * zoom.clamp(0.05, 8.0);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(img.x.max(min_width()), img.y + bar_height(ppp))));
    }

    /// Where the image is drawn in `area`: as large as fits, never stretched.
    fn image_rect(&self, area: Rect, ppp: f32) -> Rect {
        let (w, h) = (self.img.width() as f32, self.img.height() as f32);
        let k = (area.width() / w).min(area.height() / h);
        let r = Rect::from_center_size(area.center(), vec2(w * k, h * k));
        // Starting on a whole screen pixel, so 100% stays sharp.
        let snap = |v: f32| (v * ppp).round() / ppp;
        r.translate(vec2(snap(r.min.x) - r.min.x, snap(r.min.y) - r.min.y))
    }

    fn bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, zoom: f32) {
        let rect = ui.max_rect();
        // Dragging the empty part of the bar moves the window.
        let drag = ui.interact(rect, ui.id().with("bar_drag"), Sense::click_and_drag());
        if drag.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 4.0))), |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let mut on = self.prefs.on_top;
                if ui
                    .checkbox(&mut on, t("On top"))
                    .on_hover_text(t("Keep this screenshot above other windows (Ctrl+T). New pins start the same way."))
                    .changed()
                {
                    self.set_on_top(ctx, on);
                }
                if ui.button(t("Copy")).on_hover_text(t("Copy to clipboard (Ctrl+C)")).clicked() {
                    self.copy();
                }
                if ui.button(t("Save as...")).on_hover_text(t("Save as (Ctrl+S)")).clicked() {
                    self.save_as();
                }
                if ui.button(t("Edit")).on_hover_text(t("Open in the editor (Ctrl+E)")).clicked() {
                    self.edit();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // A drawn cross: not every UI font has the glyph.
                    let (r, close) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                    if close.hovered() {
                        ui.painter().rect_filled(r, CornerRadius::same(6), pal().control_hover);
                    }
                    let c = if close.hovered() { pal().text } else { pal().muted };
                    let d = 4.5;
                    for (a, b) in [(vec2(-d, -d), vec2(d, d)), (vec2(-d, d), vec2(d, -d))] {
                        ui.painter().line_segment([r.center() + a, r.center() + b], Stroke::new(1.5_f32, c));
                    }
                    if close.on_hover_text(t("Close (Esc)")).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if cfg!(windows) {
                        egui::ComboBox::from_id_salt("opacity")
                            .width(58.0)
                            .selected_text(format!("{}%", self.opacity))
                            .show_ui(ui, |ui| {
                                for pct in OPACITIES {
                                    if ui.selectable_label(self.opacity == pct, format!("{pct}%")).clicked() {
                                        self.set_opacity(pct);
                                    }
                                }
                            })
                            .response
                            .on_hover_text(t("Opacity: see through the screenshot to what is below it"));
                    }
                    let z = ui
                        .add(
                            egui::Label::new(RichText::new(format!("{:.0}%", zoom * 100.0)).color(pal().muted))
                                .sense(Sense::click()),
                        )
                        .on_hover_text(t("Zoom (Ctrl + scroll). Click for 100%."));
                    if z.clicked() {
                        self.zoom_to(ctx, 1.0);
                    }
                });
            });
        });
    }

    fn keys(&mut self, ctx: &egui::Context, zoom: f32) {
        let (esc, copy, save, edit, top, reset, scroll) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            (
                i.key_pressed(egui::Key::Escape),
                cmd && i.key_pressed(egui::Key::C),
                cmd && i.key_pressed(egui::Key::S),
                cmd && i.key_pressed(egui::Key::E),
                cmd && i.key_pressed(egui::Key::T),
                cmd && i.key_pressed(egui::Key::Num0),
                if cmd { i.raw_scroll_delta.y } else { 0.0 },
            )
        });
        // Ctrl+C arrives as a Copy event in egui, not always as a key.
        let copy = copy || ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        if esc {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if copy {
            self.copy();
        }
        if save {
            self.save_as();
        }
        if edit {
            self.edit();
        }
        if top {
            let on = !self.prefs.on_top;
            self.set_on_top(ctx, on);
        }
        if reset {
            self.zoom_to(ctx, 1.0);
        }
        if scroll != 0.0 {
            let step = if scroll > 0.0 { 1.1 } else { 1.0 / 1.1 };
            self.zoom_to(ctx, zoom * step);
        }
    }
}

impl eframe::App for PinApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        pal().canvas.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let screen = ctx.screen_rect();
        let bar = bar_height(ctx.pixels_per_point());
        let area = Rect::from_min_max(pos2(screen.left(), screen.top() + bar), screen.max);
        let shown = self.image_rect(area, ctx.pixels_per_point());
        let zoom = shown.width() * ctx.pixels_per_point() / self.img.width() as f32;
        self.keys(ctx, zoom);
        let focused = ctx.input(|i| i.focused);

        egui::TopBottomPanel::top("pin_bar")
            .exact_height(bar)
            .frame(egui::Frame::NONE.fill(pal().panel))
            .show_separator_line(false)
            .show(ctx, |ui| self.bar(ui, ctx, zoom));

        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(pal().canvas)).show(ctx, |ui| {
            let r = ui.interact(area, ui.id().with("pin_image"), Sense::click_and_drag());
            ui.painter().image(
                self.tex.id(),
                shown,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            if r.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if r.double_clicked() {
                self.zoom_to(ctx, 1.0);
            }
            r.context_menu(|ui| {
                if ui.button(t("Copy")).clicked() {
                    self.copy();
                    ui.close_menu();
                }
                if ui.button(t("Save as...")).clicked() {
                    ui.close_menu();
                    self.save_as();
                }
                if ui.button(t("Open in editor")).clicked() {
                    self.edit();
                    ui.close_menu();
                }
                ui.separator();
                let mut on = self.prefs.on_top;
                if ui.checkbox(&mut on, t("On top")).changed() {
                    self.set_on_top(ctx, on);
                }
                if ui.button(t("Actual size (100%)")).clicked() {
                    self.zoom_to(ctx, 1.0);
                    ui.close_menu();
                }
                ui.separator();
                if ui.button(t("Close")).clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

            // The resize grip in the bottom right corner.
            let grip = Rect::from_min_max(screen.max - vec2(16.0, 16.0), screen.max);
            let g = ui.interact(grip, ui.id().with("grip"), Sense::drag());
            if g.hovered() || g.dragged() {
                ctx.set_cursor_icon(egui::CursorIcon::ResizeSouthEast);
            }
            if g.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(egui::viewport::ResizeDirection::SouthEast));
            }
            let c = if g.hovered() { theme::ACCENT } else { pal().muted };
            for i in 1..=3 {
                let d = i as f32 * 4.0;
                ui.painter().line_segment(
                    [pos2(screen.right() - d, screen.bottom() - 2.0), pos2(screen.right() - 2.0, screen.bottom() - d)],
                    Stroke::new(1.0_f32, c),
                );
            }
        });

        if let Some((text, at, error)) = &self.toast {
            if at.elapsed().as_secs_f32() > 2.5 {
                self.toast = None;
            } else {
                let color = if *error { pal().error } else { pal().text };
                egui::Area::new(egui::Id::new("pin_toast"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(pos2(screen.center().x, screen.bottom() - 12.0))
                    .pivot(egui::Align2::CENTER_BOTTOM)
                    .interactable(false)
                    .show(ctx, |ui| {
                        egui::Frame::NONE
                            .fill(pal().panel_raised)
                            .stroke(Stroke::new(1.0_f32, pal().border))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(egui::Margin::symmetric(10, 5))
                            .show(ui, |ui| ui.label(RichText::new(text.as_str()).color(color)));
                    });
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }

        // A thin edge, so the pin never looks like part of the screen below it: the accent
        // while it has the focus.
        let edge = if focused { theme::ACCENT } else { pal().border };
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("pin_edge"))).rect_stroke(
            screen.shrink(0.5),
            CornerRadius::ZERO,
            Stroke::new(1.0_f32, edge),
            egui::StrokeKind::Inside,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_screenshots_open_at_full_size() {
        let (size, zoom) = initial_size(800, 400, 1.0, vec2(1600.0, 1000.0), 300.0);
        assert_eq!(zoom, 1.0);
        assert_eq!(size, vec2(800.0, 400.0 + bar_height(1.0)));
        // On a 150% screen the same pixels are fewer points.
        let (size, _) = initial_size(900, 600, 1.5, vec2(1600.0, 1000.0), 300.0);
        assert_eq!(size, vec2(600.0, 400.0 + bar_height(1.5)));
    }

    #[test]
    fn large_screenshots_are_shrunk_and_tiny_ones_get_room_for_the_bar() {
        let (size, zoom) = initial_size(3200, 1000, 1.0, vec2(1600.0, 1000.0), 300.0);
        assert_eq!(zoom, 0.5);
        assert_eq!(size, vec2(1600.0, 500.0 + bar_height(1.0)));
        let (size, zoom) = initial_size(80, 40, 1.0, vec2(1600.0, 1000.0), 300.0);
        assert_eq!(zoom, 1.0);
        assert_eq!(size.x, 300.0);
    }

    #[test]
    fn prefs_default_to_on_top_and_load_old_files() {
        assert!(PinPrefs::default().on_top);
        let p: PinPrefs = serde_json::from_str("{}").unwrap();
        assert!(p.on_top);
        let p: PinPrefs = serde_json::from_str(r#"{"on_top":false}"#).unwrap();
        assert!(!p.on_top);
    }
}
