//! Update window (separate process, `Blink --update`): checks GitHub, shows what is new,
//! downloads and installs the update.

use crate::config::Config;
use crate::i18n::{t, tf};
use crate::theme::{self, pal};
use crate::update::{self, Check, Release, Version};
use eframe::egui::{self, Color32, RichText};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

pub fn run() -> Result<(), String> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(t("Blink update"))
        .with_inner_size([460.0, 190.0])
        .with_min_inner_size([400.0, 170.0])
        .with_app_id("blink-update");
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/blink.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, centered: true, ..Default::default() };
    eframe::run_native(
        "Blink update",
        options,
        Box::new(|cc| {
            crate::editor::setup_theme(&cc.egui_ctx);
            crate::editor::style_title_bar(cc);
            let mut app = UpdateApp::new(cc.egui_ctx.clone());
            app.start_check();
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}

enum State {
    Checking,
    UpToDate,
    Available(Release),
    Downloading { release: Release, done: u64, total: u64 },
    Installing,
    Error(String),
}

enum Msg {
    Checked(Result<Check, String>),
    Progress(u64, u64),
    Downloaded(Result<PathBuf, String>),
    /// Starting the installer failed (on success the installer closes this window).
    InstallFailed(String),
}

struct UpdateApp {
    ctx: egui::Context,
    state: State,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    /// The window size asked for last: compact unless there are release notes to show.
    size: egui::Vec2,
}

const ACCENT: Color32 = theme::ACCENT;

impl UpdateApp {
    fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        Self {
            ctx,
            state: State::Checking,
            tx,
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            size: egui::vec2(460.0, 190.0),
        }
    }

    fn start_check(&mut self) {
        self.state = State::Checking;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let r = update::check();
            tx.send(Msg::Checked(r)).ok();
            ctx.request_repaint();
        });
    }

    fn start_download(&mut self, release: Release) {
        self.cancel.store(false, Ordering::Relaxed);
        let (tx, ctx, cancel) = (self.tx.clone(), self.ctx.clone(), self.cancel.clone());
        let rel = release.clone();
        std::thread::spawn(move || {
            let mut last = 0u64;
            let r = update::download(
                &rel,
                &mut |done, total| {
                    if done - last > 64 * 1024 || done == total {
                        last = done;
                        tx.send(Msg::Progress(done, total)).ok();
                        ctx.request_repaint();
                    }
                },
                &cancel,
            );
            let path = r.as_ref().ok().cloned();
            tx.send(Msg::Downloaded(r)).ok();
            ctx.request_repaint();
            // Start the installer here, not on the UI thread: Windows (Defender, SmartScreen)
            // scans a freshly downloaded, unsigned exe before CreateProcess returns, which can
            // take several seconds. On the UI thread the window stopped pumping messages in
            // that time and Windows marked it "Not responding".
            if let Some(path) = path {
                if let Err(e) = update::install(&path) {
                    tx.send(Msg::InstallFailed(e)).ok();
                    ctx.request_repaint();
                }
            }
        });
        let total = release.asset.size;
        self.state = State::Downloading { release, done: 0, total };
    }

    fn receive(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Checked(Ok(Check::UpToDate)) => self.state = State::UpToDate,
                Msg::Checked(Ok(Check::Available(r))) => self.state = State::Available(r),
                Msg::Checked(Err(e)) => self.state = State::Error(e),
                Msg::Progress(d, t) => {
                    if let State::Downloading { done, total, .. } = &mut self.state {
                        *done = d;
                        *total = t;
                    }
                }
                Msg::Downloaded(Ok(_)) => self.state = State::Installing,
                Msg::InstallFailed(e) => self.state = State::Error(e),
                Msg::Downloaded(Err(e)) => {
                    // A cancelled download goes back to the offer.
                    self.state = match std::mem::replace(&mut self.state, State::Checking) {
                        State::Downloading { release, .. } if self.cancel.load(Ordering::Relaxed) => {
                            State::Available(release)
                        }
                        _ => State::Error(e),
                    };
                }
            }
        }
    }
}

fn mb(b: u64) -> String {
    format!("{:.1} MB", b as f64 / 1_048_576.0)
}

/// Release notes are Markdown (from CHANGELOG.md): shows headings and bullets, without the markup.
pub(crate) fn release_notes(ui: &mut egui::Ui, notes: &str) {
    for line in notes.lines() {
        let line = line.trim_end().replace("**", "").replace('`', "");
        if let Some(h) = line.strip_prefix("### ").or_else(|| line.strip_prefix("## ")) {
            ui.add_space(4.0);
            ui.label(RichText::new(h).family(egui::FontFamily::Name("ui_semibold".into())).color(pal().text));
        } else if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            ui.horizontal_top(|ui| {
                ui.label("\u{2022}");
                ui.add(egui::Label::new(item).wrap());
            });
        } else if !line.trim().is_empty() {
            ui.add(egui::Label::new(line).wrap());
        }
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(18.0).family(egui::FontFamily::Name("ui_semibold".into())).color(pal().text));
    ui.add_space(6.0);
}

fn primary(ui: &mut egui::Ui, text: &str) -> bool {
    ui.add(egui::Button::new(RichText::new(text).strong().color(theme::INK)).fill(ACCENT)).clicked()
}

/// Whether this exe is the installed copy (an update replaces that one, not a portable exe).
fn running_installed_copy() -> bool {
    match (crate::platform::install_dir(), std::env::current_exe()) {
        (Some(dir), Ok(exe)) => exe.parent().is_some_and(|p| p.eq(dir.as_path())),
        _ => false,
    }
}

impl eframe::App for UpdateApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive();
        let want = match &self.state {
            State::Available(r) if !r.notes.is_empty() => egui::vec2(560.0, 460.0),
            State::Available(_) => egui::vec2(520.0, 280.0),
            State::Error(_) => egui::vec2(480.0, 230.0),
            _ => egui::vec2(460.0, 190.0),
        };
        if want != self.size {
            self.size = want;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(want));
        }
        let current = Version::current();
        let mut action: Option<Box<dyn FnOnce(&mut Self)>> = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(pal().panel_raised).inner_margin(egui::Margin::same(20)))
            .show(ctx, |ui| match &self.state {
                State::Checking => {
                    heading(ui, t("Checking for updates"));
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(t("Asking GitHub for the latest version...")).color(pal().muted));
                    });
                }
                State::UpToDate => {
                    heading(ui, t("Blink is up to date"));
                    ui.label(tf("You have the latest version, {0}.", &[&current]));
                    ui.add_space(16.0);
                    if ui.button(t("Close")).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                State::Available(rel) => {
                    heading(ui, &tf("Blink {0} is available", &[&rel.version]));
                    ui.label(RichText::new(tf("You have {0}.  {1}", &[&current, &rel.name])).color(pal().muted));
                    ui.add_space(8.0);
                    if !rel.notes.is_empty() {
                        egui::Frame::NONE
                            .fill(pal().input_bg)
                            .corner_radius(theme::radius(48.0))
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical().max_height(170.0).auto_shrink([false, true]).show(
                                    ui,
                                    |ui| {
                                        release_notes(ui, &rel.notes);
                                    },
                                );
                            });
                        ui.add_space(8.0);
                    }
                    ui.label(
                        RichText::new(t(
                            "Installing closes Blink, including open editor windows (save your work first), \
                             installs the update and starts Blink again.",
                        ))
                        .color(pal().muted)
                        .small(),
                    );
                    if !running_installed_copy() {
                        ui.label(
                            RichText::new(t(
                                "This copy of Blink is not the installed one: the update installs Blink to \
                                 %LOCALAPPDATA%\\Programs\\Blink and starts that copy.",
                            ))
                            .color(pal().muted)
                            .small(),
                        );
                    }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if primary(ui, &tf("Install {0}", &[&rel.version])) {
                            let r = rel.clone();
                            action = Some(Box::new(move |app: &mut Self| app.start_download(r)));
                        }
                        if ui.button(t("Later")).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button(t("Skip this version")).on_hover_text(t("No reminders for this version")).clicked()
                        {
                            let mut c = Config::load();
                            c.skipped_version = rel.version.to_string();
                            c.save().ok();
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if !rel.page.is_empty() && ui.link(t("Release page")).clicked() {
                            ctx.open_url(egui::OpenUrl::new_tab(&rel.page));
                        }
                    });
                }
                State::Downloading { release, done, total } => {
                    heading(ui, &tf("Downloading Blink {0}", &[&release.version]));
                    let frac = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
                    ui.add(egui::ProgressBar::new(frac).show_percentage().fill(ACCENT));
                    ui.label(RichText::new(tf("{0} of {1}", &[&mb(*done), &mb(*total)])).color(pal().muted).small());
                    ui.add_space(12.0);
                    if ui.button(t("Cancel")).clicked() {
                        self.cancel.store(true, Ordering::Relaxed);
                    }
                }
                State::Installing => {
                    heading(ui, t("Installing"));
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t("Blink closes now and starts again in a few seconds."));
                    });
                }
                State::Error(e) => {
                    heading(ui, t("Could not update"));
                    ui.label(RichText::new(e).color(pal().error));
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if primary(ui, t("Try again")) {
                            action = Some(Box::new(|app: &mut Self| app.start_check()));
                        }
                        if ui.button(t("Open settings")).clicked() {
                            if let Ok(exe) = std::env::current_exe() {
                                let _ = std::process::Command::new(exe).arg("--settings").spawn();
                            }
                        }
                        if ui.button(t("Close")).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                }
            });
        if let Some(a) = action {
            a(self);
        }
    }
}
