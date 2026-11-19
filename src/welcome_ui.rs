//! First-start window (`Blink --welcome`, opened once by the tray process): the capture
//! hotkey, the Windows 11 Print Screen setting with a button to fix it, and a test capture.

use crate::config::{Config, VK_SNAPSHOT};
use crate::i18n::{t, tf, Language};
use crate::platform;
use crate::theme::{self, pal};
use eframe::egui::{self, vec2, RichText};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn run() -> Result<(), String> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(t("Welcome to Blink"))
        .with_inner_size([520.0, 470.0])
        .with_min_inner_size([460.0, 420.0])
        .with_app_id("blink-welcome");
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/blink.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, centered: true, ..Default::default() };
    eframe::run_native(
        "Blink welcome",
        options,
        Box::new(|cc| {
            crate::editor::setup_theme(&cc.egui_ctx);
            crate::editor::style_title_bar(cc);
            Ok(Box::new(WelcomeApp {
                cfg: Config::load(),
                checked: Instant::now(),
                snipping: platform::print_screen_opens_snipping(),
                logo: crate::editor::logo_texture(&cc.egui_ctx),
            }))
        }),
    )
    .map_err(|e| e.to_string())
}

struct WelcomeApp {
    cfg: Config,
    /// When the Print Screen setting was read last (the user may change it in Windows).
    checked: Instant,
    snipping: bool,
    logo: egui::TextureHandle,
}

fn step(ui: &mut egui::Ui, n: u32, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), egui::Sense::hover());
        ui.painter().circle_filled(r.center(), 12.0, theme::ACCENT);
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            n.to_string(),
            egui::FontId::new(13.0, egui::FontFamily::Name("ui_semibold".into())),
            theme::INK,
        );
        ui.vertical(|ui| {
            ui.label(
                RichText::new(title).family(egui::FontFamily::Name("ui_semibold".into())).size(15.0).color(pal().text),
            );
            add(ui);
        });
    });
    ui.add_space(12.0);
}

impl eframe::App for WelcomeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.checked.elapsed() > Duration::from_secs(1) {
            self.snipping = platform::print_screen_opens_snipping();
            self.checked = Instant::now();
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(pal().panel_raised).inner_margin(egui::Margin::same(24)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(40.0, 40.0)));
                    ui.label(
                        RichText::new(t("Welcome to Blink"))
                            .size(22.0)
                            .family(egui::FontFamily::Name("ui_semibold".into()))
                            .color(pal().text),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let before = self.cfg.language;
                        egui::ComboBox::from_id_salt("lang").selected_text(self.cfg.language.native_name()).show_ui(ui, |ui| {
                            for l in Language::ALL {
                                ui.selectable_value(&mut self.cfg.language, l, l.native_name());
                            }
                        });
                        if self.cfg.language != before {
                            // Saved right away: the tray menu and every window use it.
                            let mut c = Config::load();
                            c.language = self.cfg.language;
                            c.save().ok();
                            c.apply_globals();
                            platform::notify_resident_reload();
                        }
                    });
                });
                ui.label(RichText::new(t("Three things to know, then you are ready.")).color(pal().muted));
                ui.add_space(16.0);

                let hk = self.cfg.hotkeys.region;
                step(ui, 1, t("Your capture key"), |ui| {
                    ui.label(tf("Press {0} to select a region or window. Right-click the Blink icon in the taskbar corner for everything else.", &[&hk.label()]));
                });

                step(ui, 2, t("Print Screen on Windows 11"), |ui| {
                    if hk.vk != VK_SNAPSHOT {
                        ui.label(RichText::new(t("Your capture key is not Print Screen, so nothing to do here.")).color(pal().muted));
                    } else if self.snipping {
                        ui.label(RichText::new(t("Windows opens the Snipping Tool with Print Screen, so Blink does not get the key. Turn off \"Use the Print screen key to open screen capture\".")).color(pal().warn));
                        ui.horizontal(|ui| {
                            if ui
                                .add(egui::Button::new(RichText::new(t("Fix it")).strong().color(theme::INK)).fill(theme::ACCENT))
                                .on_hover_text(t("Opens the Windows setting"))
                                .clicked()
                            {
                                platform::open_keyboard_settings();
                            }
                            ui.label(RichText::new(t("After switching it off, restart Blink from the tray menu (Exit, then start it again).")).small().color(pal().muted));
                        });
                    } else {
                        ui.label(RichText::new(format!("\u{2714} {}", t("Print Screen goes to Blink. All set."))).color(pal().ok));
                    }
                });

                step(ui, 3, t("Try it"), |ui| {
                    ui.label(t("Take a capture now: drag over part of the screen, then draw on it in the editor and press Copy."));
                    ui.add_space(4.0);
                    if ui
                        .add(egui::Button::new(RichText::new(t("Take a test capture")).strong().color(theme::INK)).fill(theme::ACCENT))
                        .clicked()
                        && platform::request_capture()
                    {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.horizontal(|ui| {
                        if ui.button(t("Close")).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button(t("Open settings")).clicked() {
                            if let Ok(exe) = std::env::current_exe() {
                                let _ = std::process::Command::new(exe).arg("--settings").spawn();
                            }
                        }
                    });
                });
            });
    }
}
