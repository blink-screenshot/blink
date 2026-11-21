//! The clip window (`Blink --clip <folder>`): a recording after it was made. Play it, trim the
//! start and end, switch the pointer, its highlight, the click rings and the keys on or off,
//! and save it as MP4 or GIF, with an estimate of the size first.
//!
//! Its own process, like the editor, and deliberately not part of the image editor: a video
//! is a different thing to edit. The preview is drawn by the same code as the export
//! (`rec::export::Clip::compose`), so what you see is what you get.

use crate::config::{Config, VideoFormat, GIF_FPS};
use crate::i18n::{t, tf};
use crate::platform;
use crate::rec::compose::Overlays;
use crate::rec::export::{self, human_duration, human_size, Clip, Job, Trim};
use crate::rec::keys::KeyMode;
use crate::rec::store::{Rec, Spec};
use crate::rec::yuv::Nv12;
use crate::theme::{self, pal};
use eframe::egui::{self, vec2, RichText, Sense};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Widest preview frame (pixels); larger videos are scaled down for the screen only.
const PREVIEW_MAX_W: u32 = 1600;

pub fn run(dir: &Path) -> Result<(), String> {
    let rec = Rec::at(dir);
    if !rec.master().exists() {
        return Err(t("This recording is no longer available. Blink keeps recordings for editing for a day.").into());
    }
    let spec = rec.read_spec()?;
    let clip = Arc::new(rec.clip()?);
    let (w, h) = (clip.log.header.w, clip.log.header.h);
    // Opened on the player's thread: Media Foundation puts the thread that opens it in COM's
    // multithreaded mode, and this (the window's) thread must stay free for the file dialog.
    let (player, duration) = Player::start(rec.master(), (w, h), clip.clone())?;
    let duration = duration.max(rec.read_result().map_or(0, |o| o.duration_ms)).max(1);

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(t("Blink recording"))
        .with_inner_size([980.0, 720.0])
        .with_min_inner_size([620.0, 480.0])
        .with_app_id("blink-clip");
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/blink.png")) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, centered: true, ..Default::default() };
    eframe::run_native(
        "Blink clip",
        options,
        Box::new(move |cc| {
            crate::editor::setup_theme(&cc.egui_ctx);
            crate::editor::style_title_bar(cc);
            let s = &spec.settings;
            let overlays = Overlays {
                pointer: true,
                pointer_highlight: s.pointer_highlight,
                clicks: s.click_highlight,
                keys: clip.log.header.keys != KeyMode::Off,
            };
            player.set_ctx(cc.egui_ctx.clone());
            let mut app = ClipApp {
                dir: rec.dir.clone(),
                spec: spec.clone(),
                clip,
                duration,
                player,
                tex: None,
                pos: 0,
                requested: None,
                playing: None,
                trim: Trim::whole(duration),
                overlays,
                // The clip window is mostly for making a GIF to share.
                format: VideoFormat::Gif,
                estimate: Estimate::new(rec.dir.clone(), cc.egui_ctx.clone()),
                export: None,
                saved: None,
                folder: Config::load().recording.output_dir,
                message: None,
                drag: None,
            };
            app.request();
            app.estimate.ask(&app.job_for(app.format), app.format, app.gif_width());
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}

// ---- preview -------------------------------------------------------------------------------

/// Decodes frames on a thread of its own: the UI asks for a time, and gets the frame shown
/// then, with the overlays drawn on it.
struct Player {
    want: Arc<(Mutex<Option<(u32, Overlays)>>, Condvar)>,
    rx: Receiver<(u32, egui::ColorImage)>,
    ctx: Arc<Mutex<Option<egui::Context>>>,
}

impl Player {
    /// Opens the video on a thread of its own and returns once it is open, with its length.
    fn start(master: PathBuf, size: (u32, u32), clip: Arc<Clip>) -> Result<(Self, u32), String> {
        let want: Arc<(Mutex<Option<(u32, Overlays)>>, Condvar)> = Arc::new((Mutex::new(None), Condvar::new()));
        let (tx, rx): (Sender<(u32, egui::ColorImage)>, _) = std::sync::mpsc::channel();
        let (open_tx, open_rx) = std::sync::mpsc::channel::<Result<u32, String>>();
        let ctx: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));
        let (w2, ctx2) = (want.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut src = match platform::open_video(&master, Some(size)) {
                Ok(s) => {
                    let _ = open_tx.send(Ok(s.duration()));
                    s
                }
                Err(e) => {
                    let _ = open_tx.send(Err(e));
                    return;
                }
            };
            let fonts = crate::capture_bar::fonts();
            let mut cur: Option<(u32, Nv12)> = None;
            let mut ahead: Option<(u32, Nv12)> = None;
            let mut work = Nv12::default();
            let mut ended = false;
            loop {
                let (t, overlays) = {
                    let (lock, cv) = &*w2;
                    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
                    while g.is_none() {
                        g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
                    }
                    g.take().expect("checked")
                };
                // Backwards, or far ahead: seek (to the key frame before it) and read on.
                let behind = cur.as_ref().is_none_or(|(ct, _)| t < *ct || t > ct + 2000);
                if behind {
                    if src.seek(t).is_err() {
                        continue;
                    }
                    cur = None;
                    ahead = None;
                    ended = false;
                }
                loop {
                    if ahead.is_none() && !ended {
                        match src.next() {
                            Ok(Some(f)) => ahead = Some(f),
                            _ => ended = true,
                        }
                    }
                    match &ahead {
                        Some((ft, _)) if *ft <= t || cur.is_none() => cur = ahead.take(),
                        _ => break,
                    }
                }
                let Some((_, frame)) = &cur else { continue };
                work.clone_from(frame);
                clip.compose(&mut work, t, &overlays, fonts);
                // Straight to the size it is shown at: converting a whole 4K frame would cost
                // more than it shows.
                let (pw, ph) = export::output_size(work.w, work.h, Some(PREVIEW_MAX_W));
                let img = work.to_rgba_scaled(pw, ph);
                let size = [img.width() as usize, img.height() as usize];
                let ci = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                if tx.send((t, ci)).is_err() {
                    break;
                }
                if let Some(c) = ctx2.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                    c.request_repaint();
                }
            }
        });
        let duration = open_rx.recv().map_err(|_| t("The recording could not be opened.").to_string())??;
        Ok((Player { want, rx, ctx }, duration))
    }

    fn set_ctx(&self, c: egui::Context) {
        *self.ctx.lock().unwrap_or_else(|e| e.into_inner()) = Some(c);
    }

    fn ask(&self, t: u32, overlays: Overlays) {
        let (lock, cv) = &*self.want;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = Some((t, overlays));
        cv.notify_one();
    }
}

// ---- size estimate -------------------------------------------------------------------------

/// Estimates the size of a GIF on a thread (it encodes a few stretches); an MP4 is estimated
/// from its bitrate right away.
struct Estimate {
    dir: PathBuf,
    ctx: egui::Context,
    generation: Arc<AtomicU64>,
    result: Arc<Mutex<Option<(u64, Result<u64, String>)>>>,
}

impl Estimate {
    fn new(dir: PathBuf, ctx: egui::Context) -> Self {
        Estimate { dir, ctx, generation: Arc::new(AtomicU64::new(0)), result: Arc::new(Mutex::new(None)) }
    }

    fn ask(&self, job: &Job, format: VideoFormat, max_width: Option<u32>) {
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let result = self.result.clone();
        if format == VideoFormat::Mp4 {
            return;
        }
        let (dir, job, ctx, generation) = (self.dir.clone(), *job, self.ctx.clone(), self.generation.clone());
        std::thread::spawn(move || {
            // Wait a moment: the trim handles may still be moving.
            std::thread::sleep(Duration::from_millis(350));
            if generation.load(Ordering::SeqCst) != gen {
                return;
            }
            let rec = Rec::at(&dir);
            let r = (|| {
                let clip = rec.clip()?;
                let (w, h) = (clip.log.header.w, clip.log.header.h);
                let mut src = platform::open_video(&rec.master(), Some((w, h)))?;
                let cancel = || generation.load(Ordering::SeqCst) != gen;
                export::estimate_gif(src.as_mut(), &clip, &job, max_width, crate::capture_bar::fonts(), &cancel)
            })();
            if generation.load(Ordering::SeqCst) == gen {
                *result.lock().unwrap_or_else(|e| e.into_inner()) = Some((gen, r));
                ctx.request_repaint();
            }
        });
    }

    /// The GIF estimate for the latest question, once it is there.
    fn gif(&self) -> Option<Result<u64, String>> {
        let g = self.result.lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref().filter(|(gen, _)| *gen == self.generation.load(Ordering::SeqCst)).map(|(_, r)| r.clone())
    }
}

// ---- export --------------------------------------------------------------------------------

struct Exporting {
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    rx: Receiver<Result<(PathBuf, u64), String>>,
}

// ---- the window ----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    Start,
    End,
    Playhead,
}

struct ClipApp {
    dir: PathBuf,
    spec: Spec,
    clip: Arc<Clip>,
    duration: u32,
    player: Player,
    tex: Option<egui::TextureHandle>,
    /// Where the playhead is (ms).
    pos: u32,
    /// The last frame asked for, with its overlays.
    requested: Option<(u32, Overlays)>,
    /// Playing since (instant, position then).
    playing: Option<(Instant, u32)>,
    trim: Trim,
    overlays: Overlays,
    format: VideoFormat,
    estimate: Estimate,
    export: Option<Exporting>,
    saved: Option<PathBuf>,
    /// Where Save puts it (Settings > Recording).
    folder: PathBuf,
    message: Option<(String, bool)>,
    drag: Option<Drag>,
}

impl ClipApp {
    fn job_for(&self, format: VideoFormat) -> Job {
        let fps = self.clip.log.header.fps;
        match format {
            VideoFormat::Mp4 => Job { trim: self.trim, overlays: self.overlays, fps },
            VideoFormat::Gif => Job { trim: self.trim, overlays: self.overlays, fps: GIF_FPS },
        }
    }

    fn gif_width(&self) -> Option<u32> {
        Some(self.spec.settings.gif_max_width.max(64))
    }

    fn request(&mut self) {
        let want = (self.pos, self.overlays);
        if self.requested != Some(want) {
            self.requested = Some(want);
            self.player.ask(want.0, want.1);
        }
    }

    fn settings_changed(&mut self) {
        self.estimate.ask(&self.job_for(self.format), self.format, self.gif_width());
        self.request();
    }

    fn toggle_play(&mut self) {
        if self.playing.take().is_none() {
            if self.pos >= self.trim.end.saturating_sub(50) {
                self.pos = self.trim.start;
            }
            self.playing = Some((Instant::now(), self.pos));
        }
    }

    /// Saves to the recordings folder (Settings > Recording), or with `ask` where you choose.
    fn start_export(&mut self, ask: bool) {
        let fmt = self.format;
        let c = Config::load();
        let dir = c.recording.output_dir.clone();
        std::fs::create_dir_all(&dir).ok();
        let suggested = crate::config::next_free_path(&dir, &c.filename_pattern, fmt.ext());
        let path = if ask {
            let name = suggested.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let filter = match fmt {
                VideoFormat::Mp4 => (t("MP4 video"), "mp4"),
                VideoFormat::Gif => (t("GIF animation"), "gif"),
            };
            let picked = rfd::FileDialog::new()
                .set_title(t("Save recording as"))
                .set_directory(&dir)
                .set_file_name(name)
                .add_filter(filter.0, &[filter.1])
                .save_file();
            let Some(path) = picked else { return };
            path
        } else {
            suggested
        };
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let (p2, c2) = (progress.clone(), cancel.clone());
        let (rdir, overlays, trim, gw) = (self.dir.clone(), self.overlays, self.trim, self.spec.settings.gif_max_width);
        std::thread::spawn(move || {
            let r = platform::export_recording(&rdir, &path, fmt, overlays, trim, gw, &mut |p| {
                p2.store((p * 1000.0) as u32, Ordering::Relaxed);
                !c2.load(Ordering::Relaxed)
            });
            let _ = tx.send(r.map(|b| (path, b)));
        });
        self.message = None;
        self.export = Some(Exporting { progress, cancel, rx });
    }

    fn poll_export(&mut self) {
        let Some(ex) = &self.export else { return };
        if let Ok(r) = ex.rx.try_recv() {
            self.export = None;
            match r {
                Ok((path, bytes)) => {
                    self.message = Some((tf("Saved as {0} ({1})", &[&path.display(), &human_size(bytes)]), false));
                    self.saved = Some(path);
                }
                Err(e) if e == export::CANCELLED => self.message = None,
                Err(e) => self.message = Some((tf("Not saved: {0}", &[&e]), true)),
            }
        }
    }

    /// The timeline: the whole recording, the kept part between two handles, and the playhead.
    fn timeline(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 44.0), Sense::click_and_drag());
        let p = ui.painter_at(rect.expand(8.0));
        let track = egui::Rect::from_min_max(rect.left_top() + vec2(8.0, 12.0), rect.right_bottom() - vec2(8.0, 12.0));
        let x_of = |ms: u32| track.left() + track.width() * ms as f32 / self.duration as f32;
        let ms_of = |x: f32| (((x - track.left()) / track.width()).clamp(0.0, 1.0) * self.duration as f32) as u32;
        let r = theme::radius(track.height()) as f32;
        p.rect_filled(track, r, pal().control);
        let kept = egui::Rect::from_x_y_ranges(x_of(self.trim.start)..=x_of(self.trim.end), track.y_range());
        p.rect_filled(kept, r, theme::ACCENT.gamma_multiply(0.35));
        // The handles.
        let handle =
            |x: f32| egui::Rect::from_center_size(egui::pos2(x, track.center().y), vec2(10.0, track.height() + 14.0));
        let (hs, he) = (handle(x_of(self.trim.start)), handle(x_of(self.trim.end)));
        for hr in [hs, he] {
            p.rect_filled(hr, 3.0, theme::ACCENT);
            p.line_segment(
                [hr.center() - vec2(0.0, 5.0), hr.center() + vec2(0.0, 5.0)],
                egui::Stroke::new(1.5f32, theme::INK),
            );
        }
        // The playhead.
        let px = x_of(self.pos);
        p.line_segment(
            [egui::pos2(px, rect.top()), egui::pos2(px, rect.bottom())],
            egui::Stroke::new(2.0f32, pal().text),
        );
        p.circle_filled(egui::pos2(px, rect.top() + 2.0), 4.0, pal().text);

        if resp.drag_started() || resp.clicked() {
            if let Some(m) = resp.interact_pointer_pos() {
                self.drag = Some(if hs.expand(4.0).contains(m) {
                    Drag::Start
                } else if he.expand(4.0).contains(m) {
                    Drag::End
                } else {
                    Drag::Playhead
                });
                self.playing = None;
            }
        }
        if let (Some(d), Some(m)) = (self.drag, resp.interact_pointer_pos()) {
            let ms = ms_of(m.x);
            match d {
                Drag::Start => {
                    self.trim.start = ms.min(self.trim.end.saturating_sub(export::MIN_CLIP_MS));
                    self.pos = self.trim.start;
                }
                Drag::End => {
                    self.trim.end = ms.max(self.trim.start + export::MIN_CLIP_MS).min(self.duration);
                    self.pos = self.trim.end.saturating_sub(1);
                }
                Drag::Playhead => self.pos = ms,
            }
        }
        if resp.drag_stopped() || resp.clicked() {
            if matches!(self.drag, Some(Drag::Start | Drag::End)) {
                self.settings_changed();
            }
            self.drag = None;
        }
        let hover = resp.hover_pos().map(|m| hs.expand(4.0).contains(m) || he.expand(4.0).contains(m)).unwrap_or(false);
        if hover || matches!(self.drag, Some(Drag::Start | Drag::End)) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
    }
}

fn semibold(text: &str, size: f32) -> RichText {
    RichText::new(text).size(size).family(egui::FontFamily::Name("ui_semibold".into()))
}

impl eframe::App for ClipApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_export();
        // The newest decoded frame.
        while let Ok((_, img)) = self.player.rx.try_recv() {
            match &mut self.tex {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => self.tex = Some(ctx.load_texture("clip-frame", img, egui::TextureOptions::LINEAR)),
            }
        }
        if let Some((since, from)) = self.playing {
            let now = from + since.elapsed().as_millis() as u32;
            if now >= self.trim.end {
                self.pos = self.trim.end.saturating_sub(1);
                self.playing = None;
            } else {
                self.pos = now;
                ctx.request_repaint_after(Duration::from_millis(15));
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) && self.export.is_none() {
            self.toggle_play();
        }
        self.request();

        egui::TopBottomPanel::bottom("controls")
            .frame(egui::Frame::NONE.fill(pal().panel).inner_margin(egui::Margin::symmetric(16, 12)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let label = if self.playing.is_some() { t("Pause") } else { t("Play") };
                    if ui.add(egui::Button::new(semibold(label, 14.0)).min_size(vec2(76.0, 30.0))).clicked() {
                        self.toggle_play();
                    }
                    ui.label(
                        RichText::new(format!("{} / {}", human_duration(self.pos), human_duration(self.duration)))
                            .monospace()
                            .color(pal().text),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(tf(
                                "Keeps {0} ({1} to {2})",
                                &[&human_duration(self.trim.len()), &human_duration(self.trim.start), &human_duration(self.trim.end)],
                            ))
                            .color(pal().muted),
                        );
                    });
                });
                ui.add_space(6.0);
                self.timeline(ui);
                ui.label(RichText::new(t("Drag the handles to trim the start and the end.")).small().color(pal().muted));
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);

                ui.horizontal_wrapped(|ui| {
                    ui.label(semibold(t("Show"), 13.0).color(pal().text));
                    let before = self.overlays;
                    ui.checkbox(&mut self.overlays.pointer, t("Mouse pointer"));
                    ui.checkbox(&mut self.overlays.pointer_highlight, t("Pointer highlight"));
                    ui.checkbox(&mut self.overlays.clicks, t("Clicks"));
                    let logged = self.clip.log.header.keys != KeyMode::Off;
                    ui.add_enabled_ui(logged, |ui| {
                        ui.checkbox(&mut self.overlays.keys, t("Keys"));
                    })
                    .response
                    .on_disabled_hover_text(t("Keys were not recorded. Switch on \"Show keys and clicks\" in Settings > Recording before you record."));
                    if self.overlays != before {
                        self.settings_changed();
                    }
                });
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.label(semibold(t("Save as"), 13.0).color(pal().text));
                    let before = self.format;
                    for f in VideoFormat::ALL {
                        ui.selectable_value(&mut self.format, f, f.name());
                    }
                    if self.format != before {
                        self.settings_changed();
                    }
                    let estimate = match self.format {
                        VideoFormat::Mp4 => {
                            let (w, h) = (self.clip.log.header.w, self.clip.log.header.h);
                            let fps = self.clip.log.header.fps;
                            tf("about {0}", &[&human_size(export::estimate_mp4(w, h, fps, self.trim.len()))])
                        }
                        VideoFormat::Gif => match self.estimate.gif() {
                            Some(Ok(b)) => tf("about {0}", &[&human_size(b)]),
                            Some(Err(_)) => String::new(),
                            None => t("estimating size...").to_string(),
                        },
                    };
                    ui.label(RichText::new(estimate).color(pal().muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(ex) = &self.export {
                            if ui.button(t("Cancel")).clicked() {
                                ex.cancel.store(true, Ordering::Relaxed);
                            }
                            let p = ex.progress.load(Ordering::Relaxed) as f32 / 1000.0;
                            ui.add(egui::ProgressBar::new(p).desired_width(180.0).show_percentage());
                            ctx.request_repaint_after(Duration::from_millis(100));
                        } else {
                            // Right to left: Save (in the recordings folder) is the main button.
                            let save = egui::Button::new(semibold(t("Save"), 14.0).color(theme::INK))
                                .fill(theme::ACCENT)
                                .min_size(vec2(90.0, 30.0));
                            let folder = self.folder.clone();
                            if ui.add(save).on_hover_text(tf("Saves in {0}", &[&folder.display()])).clicked() {
                                self.playing = None;
                                self.start_export(false);
                            }
                            if ui.add(egui::Button::new(t("Save as...")).min_size(vec2(90.0, 30.0))).clicked() {
                                self.playing = None;
                                self.start_export(true);
                            }
                        }
                    });
                });
                if let Some((msg, error)) = self.message.clone() {
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(msg).color(if error { pal().error } else { pal().ok }));
                        if let (false, Some(path)) = (error, self.saved.clone()) {
                            if ui.button(t("Copy")).on_hover_text(t("Copies the file, to paste in a chat, an e-mail or a folder.")).clicked() {
                                match platform::copy_files(std::slice::from_ref(&path)) {
                                    Ok(()) => self.message = Some((t("Copied. Paste it in a chat, an e-mail or a folder.").into(), false)),
                                    Err(e) => self.message = Some((e, true)),
                                }
                            }
                            if ui.button(t("Show in folder")).clicked() {
                                if let Some(parent) = path.parent() {
                                    platform::open_folder(parent);
                                }
                            }
                        }
                    });
                }
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(pal().canvas).inner_margin(egui::Margin::same(12)))
            .show(ctx, |ui| {
                let avail = ui.available_size();
                match self.tex.clone() {
                    Some(tex) => {
                        // At most 1:1 on screen, smaller to fit the window.
                        let (vw, vh) = (self.clip.log.header.w as f32, self.clip.log.header.h as f32);
                        let s = (avail.x / vw).min(avail.y / vh).min(1.0 / ctx.pixels_per_point());
                        let size = vec2(vw * s, vh * s);
                        let mut clicked = false;
                        ui.centered_and_justified(|ui| {
                            let img = egui::Image::new(&tex).fit_to_exact_size(size);
                            clicked = ui.add(img.sense(Sense::click())).clicked();
                        });
                        if clicked {
                            self.toggle_play();
                        }
                    }
                    None => {
                        ui.centered_and_justified(|ui| {
                            ui.label(RichText::new(t("Loading...")).color(pal().muted));
                        });
                    }
                }
            });
    }
}
