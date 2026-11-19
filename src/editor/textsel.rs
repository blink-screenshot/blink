//! Text in the editor: the "Select text (OCR)" tool and "Find private info".
//!
//! The screenshot is read once, on a worker thread, the first time it is needed (the tool is
//! chosen, or private info is looked for); the image never changes in the editor, only the
//! annotations on top of it, so the result stays valid. Text is selected like in a document
//! (drag from a word) or with a box (drag from an empty spot), copied with Ctrl+C or the
//! floating Copy button, and can be hidden with the redaction tool's mode in one step.
//!
//! "Find private info" suggests redactions (see `crate::sensitive`): a box over each finding,
//! a click on it leaves it out, and one button hides the rest.

use super::{EditorApp, Tool};
use crate::annot::{Annotation, Kind, RedactMode};
use crate::i18n::{t, tf};
use crate::imageio;
use crate::ocr::{self, Bounds, OcrError, Page};
use crate::sensitive::{self, Finding};
use crate::theme::{self, pal};
use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, Margin, Pos2, Rect, RichText, Stroke};
use std::sync::mpsc::Receiver;

pub enum OcrJob {
    Idle,
    Running(Receiver<Result<Page, OcrError>>),
    Ready(Page),
    Failed(OcrError),
}

/// What to do once the text is read (the user asked before it was).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterOcr {
    CopyAll,
    SelectAll,
    FindPrivate,
}

/// Suggested redactions, and which of them are still on.
pub struct Privacy {
    pub findings: Vec<Finding>,
    pub on: Vec<bool>,
}

pub fn to_rect(b: &Bounds) -> Rect {
    Rect::from_min_size(pos2(b.x, b.y), vec2(b.w, b.h))
}

impl EditorApp {
    /// Starts reading the screenshot's text, unless that is done or under way.
    pub fn ensure_ocr(&mut self, ctx: &egui::Context) {
        if !matches!(self.ocr, OcrJob::Idle) {
            return;
        }
        let base = self.renderer.base();
        let (w, h) = (base.width(), base.height());
        // Premultiplied RGBA to BGRA. Transparent parts turn black, which is fine for reading.
        let mut bgra = base.data().to_vec();
        for p in bgra.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(ocr::read_bgra(&bgra, w, h));
            ctx.request_repaint();
        });
        self.ocr = OcrJob::Running(rx);
    }

    /// Picks up the result of the reading thread (call every frame).
    pub fn poll_ocr(&mut self, ctx: &egui::Context) {
        if let OcrJob::Running(rx) = &self.ocr {
            match rx.try_recv() {
                Ok(Ok(page)) => {
                    self.found = sensitive::scan(&page);
                    self.ocr = OcrJob::Ready(page);
                }
                Ok(Err(e)) => self.ocr = OcrJob::Failed(e),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(150));
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.ocr = OcrJob::Failed(OcrError::Failed("stopped".into()))
                }
            }
            match (self.after_ocr.take(), &self.ocr) {
                (Some(after), OcrJob::Ready(_)) => match after {
                    AfterOcr::CopyAll => self.copy_all_text(ctx),
                    AfterOcr::SelectAll => self.select_all_text(ctx),
                    AfterOcr::FindPrivate => self.find_private(ctx),
                },
                (Some(_), OcrJob::Failed(e)) => {
                    let msg = e.message();
                    self.toast(msg, true);
                }
                _ => {}
            }
        }
    }

    pub fn ocr_running(&self) -> bool {
        matches!(self.ocr, OcrJob::Running(_))
    }

    pub fn page(&self) -> Option<&Page> {
        match &self.ocr {
            OcrJob::Ready(p) => Some(p),
            _ => None,
        }
    }

    /// Whether word `i` is in the visible (cropped) part of the image.
    pub fn word_visible(&self, i: usize) -> bool {
        let view = self.view_rect();
        self.page().and_then(|p| p.word(i)).is_some_and(|w| view.contains(pos2(w.rect.cx(), w.rect.cy())))
    }

    fn visible_words(&self) -> Vec<usize> {
        let Some(page) = self.page() else { return Vec::new() };
        (0..page.word_count()).filter(|&i| self.word_visible(i)).collect()
    }

    /// Runs `what` now if the text is read, or once it is.
    fn when_read(&mut self, ctx: &egui::Context, what: AfterOcr) -> bool {
        match &self.ocr {
            OcrJob::Ready(_) => true,
            OcrJob::Failed(e) => {
                let msg = e.message();
                self.toast(msg, true);
                false
            }
            _ => {
                self.ensure_ocr(ctx);
                self.after_ocr = Some(what);
                false
            }
        }
    }

    fn put_text(&mut self, text: String) {
        match imageio::copy_text_to_clipboard(&text) {
            Ok(()) => {
                let n = text.chars().count();
                self.toast(tf("Copied {0} characters of text", &[&n]), false);
            }
            Err(e) => self.toast(tf("Copy failed: {0}", &[&e]), true),
        }
    }

    pub fn copy_selected_text(&mut self) {
        let Some(page) = self.page() else { return };
        let text = page.text_of(&self.text_sel);
        if !text.is_empty() {
            self.put_text(text);
        }
    }

    pub fn copy_all_text(&mut self, ctx: &egui::Context) {
        if !self.when_read(ctx, AfterOcr::CopyAll) {
            return;
        }
        let words = self.visible_words();
        let text = self.page().map(|p| p.text_of(&words)).unwrap_or_default();
        if text.is_empty() {
            self.toast(t("No text found in this image."), false);
        } else {
            self.put_text(text);
        }
    }

    pub fn select_all_text(&mut self, ctx: &egui::Context) {
        if self.when_read(ctx, AfterOcr::SelectAll) {
            self.text_sel = self.visible_words();
        }
    }

    /// Hides the selected text with the redaction tool's mode (one undo step).
    pub fn redact_selected_text(&mut self) {
        let Some(page) = self.page() else { return };
        let rects: Vec<Bounds> = page.selection_rects(&self.text_sel);
        if rects.is_empty() {
            return;
        }
        let rects: Vec<Bounds> = rects.iter().map(|r| r.expand((r.h * 0.12).max(1.0))).collect();
        self.add_redactions(&rects);
        self.text_sel.clear();
    }

    fn add_redactions(&mut self, rects: &[Bounds]) {
        self.finish_text_edit();
        self.checkpoint();
        let style = self.style_for(Tool::Redact);
        for r in rects {
            let id = self.new_id();
            let r = to_rect(r).intersect(self.image_rect());
            self.doc.anns.push(Annotation { id, kind: Kind::Redact(self.redact_mode), a: r.min, b: r.max, style });
        }
        self.selected = None;
        self.invalidate();
    }

    /// The redaction tool: what a click at image position `p` hides (a private item or a word),
    /// if the text is read and that spot is visible.
    pub fn redact_target(&self, p: Pos2, zoom: f32) -> Option<(Bounds, Option<sensitive::Kind>)> {
        let page = self.page()?;
        if !self.view_rect().contains(p) {
            return None;
        }
        sensitive::target_at(page, &self.found, p.x, p.y, 2.0 / zoom)
    }

    /// Hides one word or item (a click in the redaction tool).
    pub fn redact_bounds(&mut self, b: Bounds) {
        self.add_redactions(&[b]);
    }

    /// Looks for private info and shows what it found as suggestions.
    pub fn find_private(&mut self, ctx: &egui::Context) {
        if !self.when_read(ctx, AfterOcr::FindPrivate) {
            return;
        }
        let view = self.view_rect();
        let findings: Vec<Finding> = self.found.iter().filter(|f| view.intersects(to_rect(&f.rect))).cloned().collect();
        if findings.is_empty() {
            self.privacy = None;
            self.toast(t("No private info found. Do check the screenshot yourself."), false);
            return;
        }
        let on = vec![true; findings.len()];
        self.privacy = Some(Privacy { findings, on });
    }

    /// Hides the suggestions that are still on.
    pub fn apply_privacy(&mut self) {
        let Some(p) = self.privacy.take() else { return };
        let rects: Vec<Bounds> = p.findings.iter().zip(&p.on).filter(|(_, on)| **on).map(|(f, _)| f.rect).collect();
        if rects.is_empty() {
            return;
        }
        self.add_redactions(&rects);
        self.toast(tf("Hid {0} items", &[&rects.len()]), false);
    }

    /// A click at image position `p` on a suggestion turns it off (or on again).
    pub fn toggle_finding_at(&mut self, p: Pos2) -> bool {
        let Some(priv_) = &mut self.privacy else { return false };
        let hit = priv_.findings.iter().position(|f| to_rect(&f.rect).contains(p));
        match hit {
            Some(i) => {
                priv_.on[i] = !priv_.on[i];
                true
            }
            None => false,
        }
    }
}

/// A pill at the top of the canvas while the text is being read.
pub fn reading_pill(ctx: &egui::Context, canvas: Rect) {
    egui::Area::new(egui::Id::new("ocr_reading"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(canvas.center().x, canvas.top() + 14.0))
        .pivot(Align2::CENTER_TOP)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(theme::INK)
                .stroke(Stroke::new(1.0_f32, theme::ACCENT))
                .corner_radius(CornerRadius::same(16))
                .inner_margin(Margin::symmetric(14, 7))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0).color(theme::ACCENT));
                        ui.label(RichText::new(t("Reading text\u{2026}")).color(Color32::WHITE));
                    });
                });
        });
}

/// The private info that was found, in a bar under the property bar (so it never covers the
/// screenshot): what was found, the redaction mode, and Hide / Cancel.
pub fn privacy_bar(app: &mut EditorApp, ctx: &egui::Context) {
    let Some(p) = &app.privacy else { return };
    let total = p.findings.len();
    let on = p.on.iter().filter(|o| **o).count();
    let mut counts: Vec<(sensitive::Kind, usize)> = sensitive::Kind::ALL
        .into_iter()
        .map(|k| (k, p.findings.iter().filter(|f| f.kind == k).count()))
        .filter(|(_, n)| *n > 0)
        .collect();
    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let mut apply = false;
    let mut cancel = false;
    egui::TopBottomPanel::top("privacy")
        .frame(
            egui::Frame::NONE
                .fill(pal().panel)
                .stroke(Stroke::new(1.0_f32, theme::ACCENT.gamma_multiply(0.6)))
                .inner_margin(Margin::symmetric(12, 6)),
        )
        .show_separator_line(false)
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                ui.label(RichText::new(tf("Found {0} items that look private", &[&total])).strong().color(pal().text));
                let summary: Vec<String> = counts.iter().map(|(k, n)| format!("{n} \u{d7} {}", k.name())).collect();
                ui.label(RichText::new(summary.join(" \u{b7} ")).color(pal().muted));
                ui.separator();
                for (mode, label) in [
                    (RedactMode::Blur, t("Blur")),
                    (RedactMode::Pixelate, t("Pixelate")),
                    (RedactMode::Solid, t("Black box")),
                ] {
                    if ui.selectable_label(app.redact_mode == mode, label).clicked() {
                        app.redact_mode = mode;
                    }
                }
                let hide =
                    egui::Button::new(RichText::new(tf("Hide {0}", &[&on])).color(super::ui::ON_ACCENT).strong())
                        .fill(theme::ACCENT);
                if ui.add_enabled(on > 0, hide).clicked() {
                    apply = true;
                }
                if ui.button(t("Cancel")).on_hover_text("Esc").clicked() {
                    cancel = true;
                }
            });
            ui.label(
                RichText::new(t(
                    "Click a box to leave it out. Detection is not watertight: check the screenshot yourself.",
                ))
                .color(pal().muted)
                .small(),
            );
        });
    if apply {
        app.apply_privacy();
    } else if cancel {
        app.privacy = None;
    }
}

/// Copy and Hide next to a text selection (`sel`: its screen rectangle): to the right of it
/// where there is room, which is usually empty space next to text; otherwise under it.
pub fn selection_actions(app: &mut EditorApp, ctx: &egui::Context, sel: Rect, canvas: Rect) {
    const W: f32 = 190.0;
    let pos = if sel.right() + 10.0 + W < canvas.right() {
        pos2(sel.right() + 10.0, sel.top() - 4.0)
    } else {
        pos2(sel.left(), sel.bottom() + 8.0)
    };
    let pos = pos2(
        pos.x.clamp(canvas.left() + 8.0, canvas.right() - W),
        pos.y.clamp(canvas.top() + 8.0, canvas.bottom() - 44.0),
    );
    let mut copy = false;
    let mut hide = false;
    egui::Area::new(egui::Id::new("text_actions")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::NONE
            .fill(pal().panel)
            .stroke(Stroke::new(1.0_f32, pal().border))
            .corner_radius(CornerRadius::same(theme::radius(36.0)))
            .inner_margin(Margin::same(4))
            .shadow(egui::Shadow { offset: [0, 3], blur: 12, spread: 0, color: Color32::from_black_alpha(60) })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let b = egui::Button::new(RichText::new(t("Copy text")).color(super::ui::ON_ACCENT).strong())
                        .fill(theme::ACCENT);
                    copy = ui.add(b).on_hover_text("Ctrl+C").clicked();
                    hide = ui
                        .button(t("Hide"))
                        .on_hover_text(t("Cover the selected text with the redaction tool's mode"))
                        .clicked();
                });
            });
    });
    if copy {
        app.copy_selected_text();
    } else if hide {
        app.redact_selected_text();
    }
}
