//! Screenshot editor window (egui). One process per editor window, launched on demand by
//! the resident tray process, so the always-running part stays tiny.

mod canvas;
mod frame_ui;
mod prefs;
mod ui;

use crate::annot::{Annotation, Kind, RedactMode, ShapeKind, Style};
use crate::config::{Config, PreviewQuality};
use crate::frame::{self, FrameSettings, Layout};
use crate::i18n::{t, tf};
use crate::imageio;
use crate::platform;
use crate::render::{crop_pixmap, unpremultiply, Fonts, Renderer};
use eframe::egui::{self, Color32, Pos2, Rect, TextureHandle, Vec2};
use image::RgbaImage;
use prefs::EditorPrefs;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Rect,
    Ellipse,
    Shape,
    Line,
    Arrow,
    Pen,
    Highlight,
    Text,
    Callout,
    Step,
    Emoji,
    Redact,
    Crop,
}

/// Number of tools.
pub const TOOLS: usize = 14;

impl Tool {
    /// In toolbar order.
    pub const ALL: [Tool; TOOLS] = [
        Tool::Select,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Shape,
        Tool::Line,
        Tool::Arrow,
        Tool::Pen,
        Tool::Highlight,
        Tool::Text,
        Tool::Callout,
        Tool::Step,
        Tool::Emoji,
        Tool::Redact,
        Tool::Crop,
    ];
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap()
    }
    /// The toolbar group: a divider is drawn between groups.
    pub fn group(self) -> u8 {
        match self {
            Tool::Select => 0,
            Tool::Rect | Tool::Ellipse | Tool::Shape | Tool::Line | Tool::Arrow => 1,
            Tool::Pen | Tool::Highlight => 2,
            Tool::Text | Tool::Callout | Tool::Step | Tool::Emoji => 3,
            Tool::Redact => 4,
            Tool::Crop => 5,
        }
    }
    /// Full name, for tooltips (in the interface language).
    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => t("Select / move"),
            Tool::Rect => t("Rectangle"),
            Tool::Ellipse => t("Ellipse"),
            Tool::Shape => t("Shapes"),
            Tool::Line => t("Line"),
            Tool::Arrow => t("Arrow"),
            Tool::Pen => t("Freehand"),
            Tool::Highlight => t("Highlighter"),
            Tool::Text => t("Text"),
            Tool::Callout => t("Callout"),
            Tool::Step => t("Step number"),
            Tool::Emoji => t("Emoji stamp"),
            Tool::Redact => t("Blur / redact"),
            Tool::Crop => t("Crop"),
        }
    }
    /// Short name, for the label under the icon.
    pub fn short_name(self) -> &'static str {
        match self {
            Tool::Select => t("Select"),
            Tool::Rect => t("Rectangle"),
            Tool::Ellipse => t("Ellipse"),
            Tool::Shape => t("Shapes"),
            Tool::Line => t("Line"),
            Tool::Arrow => t("Arrow"),
            Tool::Pen => t("Pen"),
            Tool::Highlight => t("Marker"),
            Tool::Text => t("Text"),
            Tool::Callout => t("Callout"),
            Tool::Step => t("Step"),
            Tool::Emoji => t("Emoji"),
            Tool::Redact => t("Redact"),
            Tool::Crop => t("Crop"),
        }
    }
    /// Stable name for the preferences file.
    pub fn id(self) -> &'static str {
        match self {
            Tool::Select => "select",
            Tool::Rect => "rect",
            Tool::Ellipse => "ellipse",
            Tool::Shape => "shape",
            Tool::Line => "line",
            Tool::Arrow => "arrow",
            Tool::Pen => "pen",
            Tool::Highlight => "highlight",
            Tool::Text => "text",
            Tool::Callout => "callout",
            Tool::Step => "step",
            Tool::Emoji => "emoji",
            Tool::Redact => "redact",
            Tool::Crop => "crop",
        }
    }
    pub fn from_id(id: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.id() == id)
    }
    pub fn key(self) -> egui::Key {
        use egui::Key;
        match self {
            Tool::Select => Key::V,
            Tool::Rect => Key::R,
            Tool::Ellipse => Key::E,
            Tool::Shape => Key::S,
            Tool::Line => Key::L,
            Tool::Arrow => Key::A,
            Tool::Pen => Key::P,
            Tool::Highlight => Key::H,
            Tool::Text => Key::T,
            Tool::Callout => Key::O,
            Tool::Step => Key::N,
            Tool::Emoji => Key::M,
            Tool::Redact => Key::B,
            Tool::Crop => Key::C,
        }
    }
    /// The tool that creates annotations of this kind.
    pub fn for_kind(kind: &Kind) -> Tool {
        match kind {
            Kind::Rect => Tool::Rect,
            Kind::Ellipse => Tool::Ellipse,
            Kind::Arrow => Tool::Arrow,
            Kind::Line => Tool::Line,
            Kind::Freehand(_) => Tool::Pen,
            Kind::Text(_) => Tool::Text,
            Kind::Highlight => Tool::Highlight,
            Kind::Redact(_) => Tool::Redact,
            Kind::Step(_) => Tool::Step,
            Kind::Shape(_) => Tool::Shape,
            Kind::Callout { .. } => Tool::Callout,
            Kind::Emoji(_) => Tool::Emoji,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc {
    pub anns: Vec<Annotation>,
    pub crop: Option<Rect>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Handle {
    /// Rect corners/edges: (x side, y side) with -1 = min, 0 = none, 1 = max.
    Rect(i8, i8),
    SegA,
    SegB,
    /// The pointer of a callout.
    Tail,
}

pub enum Drag {
    None,
    Create { id: u64, start: Pos2 },
    Move { id: u64, last: Pos2, moved: bool },
    Handle { id: u64, handle: Handle, moved: bool },
    Crop { start: Pos2, cur: Pos2 },
    Pan,
}

pub struct TextEditing {
    pub id: u64,
    pub focus_set: bool,
    pub created: bool,
}

/// Part of the rendered image uploaded as one texture (GPUs limit texture size, and long
/// scrolling captures easily exceed it).
pub struct Tile {
    pub rect: Rect,
    pub tex: TextureHandle,
}

pub struct Toast {
    pub text: String,
    pub at: Instant,
    pub error: bool,
}

/// Which color the eyedropper sets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeTarget {
    pub tool: Tool,
    pub fill: bool,
}

pub struct EditorApp {
    renderer: Renderer,
    fonts: Arc<Fonts>,
    config: Config,
    pub doc: Doc,
    undo: Vec<Doc>,
    redo: Vec<Doc>,
    next_id: u64,
    pub tool: Tool,
    pub styles: [Style; TOOLS],
    pub redact_mode: RedactMode,
    pub shape: ShapeKind,
    pub emoji: String,
    /// Emoji rasterized for the picker and the live preview.
    emoji_tex: HashMap<String, TextureHandle>,
    pub selected: Option<u64>,
    drag: Drag,
    tiles: Vec<Tile>,
    /// Preview of the framed output (background / mockup), the layout it was made for, and
    /// its scale (preview pixels per output pixel).
    frame_tex: Option<(TextureHandle, Layout, f32)>,
    /// The annotated, cropped screenshot, reused while only frame options change:
    /// (annotation generation, pixels) at full size, and (generation, width, pixels) shrunk
    /// to the size it has in the preview.
    frame_full: Option<(u64, tiny_skia::Pixmap)>,
    frame_src: Option<(u64, u32, tiny_skia::Pixmap)>,
    /// Bumped whenever the annotations or the crop change.
    ann_gen: u64,
    pub frame: FrameSettings,
    frame_thumbs: Vec<(frame::Background, TextureHandle)>,
    checker: Option<TextureHandle>,
    tex_dirty: bool,
    tex_skip: Option<u64>,
    /// Image pixels -> screen points. `None` = fit to window.
    zoom: Option<f32>,
    pan: Vec2,
    last_zoom: f32,
    text_edit: Option<TextEditing>,
    toast: Option<Toast>,
    /// Changes since the image was last copied or saved.
    unsaved: bool,
    confirm_close: bool,
    /// Esc pressed: "Close the editor?" is open, with its "don't ask again" box.
    confirm_escape: bool,
    dont_ask_escape: bool,
    allow_close: bool,
    /// When the image was last copied (the Copy button says "Copied" for a moment).
    copied_at: Option<Instant>,
    /// The eyedropper is active: the next click on the image picks a color for this.
    pub eyedropper: Option<EyeTarget>,
    /// The annotated screenshot the eyedropper reads from.
    pick_src: Option<tiny_skia::Pixmap>,
    /// The background / mockup panel is folded away.
    pub panel_collapsed: bool,
    pub show_shortcuts: bool,
    /// Small copy of the screenshot for the background previews.
    shot_thumb: Option<tiny_skia::Pixmap>,
    thumb_crop: Option<Rect>,
    last_saved: Option<PathBuf>,
    space_down: bool,
    /// Ctrl is held: the mouse acts as the select/move tool.
    pub ctrl_select: bool,
    pub hover: Option<Pos2>,
    prefs_saved: EditorPrefs,
    prefs_pending: Option<Instant>,
}

pub fn run(img: RgbaImage, source: Option<PathBuf>) -> Result<(), String> {
    if img.width() == 0 || img.height() == 0 {
        return Err(t("The image is empty.").into());
    }
    let mon = platform::cursor_monitor();
    let scale = mon.map_or(1.0, |m| m.scale);
    let (work_w, work_h) =
        mon.map_or((1600.0, 1000.0), |m| (m.work_width as f32 / scale, m.work_height as f32 / scale));
    // Chrome around the canvas: toolbar + property bar + status bar.
    let chrome = Vec2::new(32.0, 130.0);
    let want = Vec2::new(img.width() as f32 / scale, img.height() as f32 / scale) + chrome;
    let max = Vec2::new(work_w * 0.94, work_h * 0.94);
    let maximize = want.x > max.x || want.y > max.y;
    let size = want.min(max).max(Vec2::new(900.0, 480.0)).min(Vec2::new(work_w, work_h));

    let title = match &source {
        Some(p) => format!("Blink - {}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        None => format!("Blink - {} x {}", img.width(), img.height()),
    };
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/blink.png")).ok();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size(size)
        .with_min_inner_size([640.0, 360.0])
        .with_app_id("blink-editor");
    let light = !crate::theme::pal().dark;
    if let Some(icon) = icon {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, centered: true, ..Default::default() };
    let phys = ((size.x * scale) as i32, (size.y * scale) as i32);
    eframe::run_native(
        "Blink",
        options,
        Box::new(move |cc| {
            #[cfg(windows)]
            {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Ok(h) = cc.window_handle() {
                    if let RawWindowHandle::Win32(w) = h.as_raw() {
                        platform::set_title_bar_theme(w.hwnd.get(), !light);
                        platform::place_editor_window(w.hwnd.get(), phys.0, phys.1, maximize);
                    }
                }
            }
            #[cfg(not(windows))]
            let _ = (phys, maximize, light);
            Ok(Box::new(EditorApp::new(cc, img)))
        }),
    )
    .map_err(|e| tf("Could not open the editor window: {0}", &[&e]))
}

fn default_styles() -> [Style; TOOLS] {
    let base = Style::default();
    let mut s = [base; TOOLS];
    s[Tool::Arrow.index()].width = 5.0;
    s[Tool::Highlight.index()].stroke = Color32::from_rgb(255, 232, 26);
    s[Tool::Text.index()].width = 0.0;
    let callout = &mut s[Tool::Callout.index()];
    callout.width = 3.0;
    callout.fill = Color32::WHITE;
    callout.font_size = 22.0;
    callout.bold = false;
    s[Tool::Emoji.index()].font_size = 72.0;
    s
}

impl EditorApp {
    fn new(cc: &eframe::CreationContext<'_>, img: RgbaImage) -> Self {
        let fonts = Fonts::load();
        ui::setup_style(&cc.egui_ctx, &fonts);
        let config = Config::load();
        let prefs = EditorPrefs::load();
        let mut styles = default_styles();
        for t in Tool::ALL {
            if let Some(s) = prefs.styles.get(t.id()) {
                styles[t.index()] = *s;
            }
        }
        let mut app = Self {
            renderer: Renderer::new(&img, fonts.clone()),
            fonts,
            config,
            doc: Doc::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            next_id: 1,
            tool: prefs.tool.as_deref().and_then(Tool::from_id).filter(|t| *t != Tool::Crop).unwrap_or(Tool::Arrow),
            styles,
            redact_mode: prefs.redact_mode.unwrap_or(RedactMode::Blur),
            shape: prefs.shape.unwrap_or(ShapeKind::Star),
            emoji: prefs
                .emoji
                .clone()
                .filter(|e| crate::emoji::exists(e))
                .unwrap_or_else(|| crate::emoji::DEFAULT.into()),
            emoji_tex: HashMap::new(),
            selected: None,
            drag: Drag::None,
            tiles: Vec::new(),
            frame_tex: None,
            frame_full: None,
            frame_src: None,
            ann_gen: 0,
            frame: prefs.frame.clone().with_known_background(),
            frame_thumbs: Vec::new(),
            checker: None,
            tex_dirty: true,
            tex_skip: None,
            zoom: None,
            pan: Vec2::ZERO,
            last_zoom: 1.0,
            text_edit: None,
            toast: None,
            unsaved: false,
            confirm_close: false,
            confirm_escape: false,
            dont_ask_escape: false,
            allow_close: false,
            copied_at: None,
            eyedropper: None,
            pick_src: None,
            panel_collapsed: prefs.panel_collapsed,
            show_shortcuts: false,
            shot_thumb: None,
            thumb_crop: None,
            last_saved: None,
            space_down: false,
            ctrl_select: false,
            hover: None,
            prefs_saved: EditorPrefs::default(),
            prefs_pending: None,
        };
        app.prefs_saved = app.prefs_snapshot();
        app
    }

    fn prefs_snapshot(&self) -> EditorPrefs {
        EditorPrefs {
            tool: Some(self.tool.id().to_string()),
            styles: Tool::ALL.into_iter().map(|t| (t.id().to_string(), self.styles[t.index()])).collect(),
            redact_mode: Some(self.redact_mode),
            frame: self.frame.clone(),
            shape: Some(self.shape),
            emoji: Some(self.emoji.clone()),
            panel_collapsed: self.panel_collapsed,
        }
    }

    /// Remembers the tool settings for the next editor (debounced).
    fn persist_prefs(&mut self, ctx: &egui::Context, force: bool) {
        let now = self.prefs_snapshot();
        if now == self.prefs_saved {
            self.prefs_pending = None;
            return;
        }
        let since = *self.prefs_pending.get_or_insert_with(Instant::now);
        if force || since.elapsed().as_secs_f32() > 0.6 {
            now.save();
            self.prefs_saved = now;
            self.prefs_pending = None;
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(700));
        }
    }

    /// Layout of the framed output for the current view, if a frame is on.
    pub fn frame_layout(&self) -> Option<Layout> {
        let v = self.view_rect();
        frame::layout(&self.frame, v.width(), v.height())
    }

    /// The tool the mouse acts as right now.
    pub fn effective_tool(&self) -> Tool {
        if self.ctrl_select {
            Tool::Select
        } else {
            self.tool
        }
    }

    pub fn image_rect(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(self.renderer.width() as f32, self.renderer.height() as f32))
    }

    /// The visible part of the image (crop or everything).
    pub fn view_rect(&self) -> Rect {
        self.doc.crop.unwrap_or_else(|| self.image_rect())
    }

    /// Records the current state for undo. Call before modifying `doc`.
    pub fn checkpoint(&mut self) {
        self.undo.push(self.doc.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.unsaved = true;
    }

    /// The image changed (annotations, crop): re-render everything.
    pub fn invalidate(&mut self) {
        self.tex_dirty = true;
        self.ann_gen += 1;
    }

    /// Only the frame options changed: the annotated screenshot can be reused.
    pub fn invalidate_frame(&mut self) {
        self.tex_dirty = true;
    }

    /// Preview pixels per output pixel for the current zoom. The preview only has to match the
    /// screen, which is much less work than the full output resolution. Fitted to the window
    /// it is exactly the screen size (one texel per screen pixel: sharp); when zoomed by hand
    /// it is rounded up to steps of sqrt(2) so zooming does not re-render all the time.
    fn preview_scale(&self, l: &Layout, ppp: f32, canvas: Vec2) -> f32 {
        let screen = self.screen_scale(l, ppp, canvas);
        match self.config.preview_quality {
            PreviewQuality::Fast => (screen * 0.5).max(0.02),
            PreviewQuality::Balanced => screen,
            PreviewQuality::Full => 1.0,
        }
    }

    fn screen_scale(&self, l: &Layout, ppp: f32, canvas: Vec2) -> f32 {
        match self.zoom {
            Some(z) => 2f32.powf(((z * ppp).max(0.01).log2() * 2.0).ceil() / 2.0).min(1.0),
            None => {
                // Same fit as the canvas uses (see canvas::show).
                let fit = ((canvas.x - 40.0) / l.size.x).min((canvas.y - 40.0) / l.size.y);
                (fit.max(0.01).min(1.0 / ppp) * ppp).min(1.0)
            }
        }
    }

    fn undo(&mut self) {
        self.finish_text_edit();
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.doc, prev));
            self.after_history_change();
        }
    }

    fn redo(&mut self) {
        self.finish_text_edit();
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.doc, next));
            self.after_history_change();
        }
    }

    fn after_history_change(&mut self) {
        if let Some(id) = self.selected {
            if self.find(id).is_none() {
                self.selected = None;
            }
        }
        self.unsaved = true;
        self.invalidate();
    }

    pub fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn find(&self, id: u64) -> Option<usize> {
        self.doc.anns.iter().position(|a| a.id == id)
    }

    pub fn get(&self, id: u64) -> Option<&Annotation> {
        self.doc.anns.iter().find(|a| a.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Annotation> {
        self.doc.anns.iter_mut().find(|a| a.id == id)
    }

    pub fn text_size(&self) -> impl Fn(&Annotation) -> Vec2 + '_ {
        |a| crate::render::text_box_size(&self.fonts, a, self.renderer.width() as f32)
    }

    pub fn delete_selected(&mut self) {
        if let Some(id) = self.selected.take() {
            if let Some(i) = self.find(id) {
                self.checkpoint();
                self.doc.anns.remove(i);
                self.invalidate();
            }
        }
    }

    /// Ends in-place text editing; removes the annotation if it ended up empty.
    pub fn finish_text_edit(&mut self) {
        if let Some(te) = self.text_edit.take() {
            if let Some(i) = self.find(te.id) {
                if matches!(&self.doc.anns[i].kind, Kind::Text(t) if t.trim().is_empty()) {
                    self.doc.anns.remove(i);
                    if te.created {
                        // Nothing was added: drop the checkpoint taken on creation.
                        self.undo.pop();
                    }
                    self.selected = None;
                }
            }
            self.tex_skip = None;
            self.invalidate();
        }
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.finish_text_edit();
        self.tool = tool;
        if tool != Tool::Select {
            // Keep the selection only if it belongs to the new tool (to tweak its style).
            if let Some(a) = self.selected.and_then(|id| self.get(id)) {
                if Tool::for_kind(&a.kind) != tool {
                    self.selected = None;
                }
            }
        }
    }

    pub fn style_for(&self, tool: Tool) -> Style {
        self.styles[tool.index()]
    }

    /// Applies a style edit to the current tool defaults and to the selection.
    pub fn apply_style(&mut self, tool: Tool, f: impl Fn(&mut Style)) {
        f(&mut self.styles[tool.index()]);
        if let Some(id) = self.selected {
            let before = self.get(id).map(|a| a.style);
            if let Some(a) = self.get_mut(id) {
                f(&mut a.style);
            }
            if before != self.get(id).map(|a| a.style) {
                self.unsaved = true;
                self.invalidate();
            }
        }
    }

    // ---- output ----

    fn export(&self) -> RgbaImage {
        let Some(l) = self.frame_layout() else {
            return self.renderer.export(&self.doc.anns, self.doc.crop);
        };
        let shot = crop_pixmap(&self.renderer.render(&self.doc.anns, None), self.doc.crop);
        let img = match frame::compose(&shot, &l, &self.frame, &self.fonts, 1.0) {
            Some(pm) => unpremultiply(&pm),
            None => unpremultiply(&shot),
        };
        // Fixed output sizes (social media, slides): exactly that many pixels.
        match self.frame.aspect.fixed_size() {
            Some((w, h)) if self.frame.background_on && (img.width(), img.height()) != (w, h) => {
                image::imageops::resize(&img, w, h, image::imageops::FilterType::Lanczos3)
            }
            _ => img,
        }
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some(Toast { text: text.into(), at: Instant::now(), error });
    }

    /// "Copied" shows on the Copy button.
    pub fn just_copied(&self) -> bool {
        self.copied_at.is_some()
    }

    pub fn copy(&mut self, ctx: &egui::Context) {
        self.finish_text_edit();
        match imageio::copy_to_clipboard(&self.export()) {
            Ok(()) => {
                self.unsaved = false;
                self.copied_at = Some(Instant::now());
                self.toast(t("Copied to clipboard"), false);
                if self.config.close_editor_after_copy {
                    self.close_now(ctx);
                }
            }
            Err(e) => self.toast(tf("Copy failed: {0}", &[&e]), true),
        }
    }

    pub fn save_quick(&mut self, ctx: &egui::Context) {
        self.finish_text_edit();
        let path = self.config.next_output_path();
        self.save_to(ctx, path);
    }

    pub fn save_as(&mut self, ctx: &egui::Context) {
        self.finish_text_edit();
        if let Some(path) = imageio::ask_save_path(&self.config) {
            self.save_to(ctx, path);
        }
    }

    fn save_to(&mut self, ctx: &egui::Context, mut path: PathBuf) {
        if path.extension().is_none() {
            path.set_extension(self.config.format.ext());
        }
        let fmt = imageio::format_from_path(&path, self.config.format);
        match imageio::save_image(&path, &self.export(), fmt, self.config.jpg_quality) {
            Ok(()) => {
                self.unsaved = false;
                self.toast(tf("Saved {0}", &[&path.display()]), false);
                self.last_saved = Some(path);
                if self.config.close_editor_after_save {
                    self.close_now(ctx);
                }
            }
            Err(e) => self.toast(e, true),
        }
    }

    fn close_now(&mut self, ctx: &egui::Context) {
        self.persist_prefs(ctx, true);
        self.allow_close = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn request_close(&mut self, ctx: &egui::Context) {
        if self.unsaved && !self.doc.anns.is_empty() {
            self.confirm_close = true;
        } else {
            self.close_now(ctx);
        }
    }

    /// Esc with nothing to cancel: close, after asking (unless the user turned that off).
    fn escape_close(&mut self, ctx: &egui::Context) {
        if self.unsaved && !self.doc.anns.is_empty() {
            // The unsaved-changes question already asks.
            self.confirm_close = true;
        } else if self.config.confirm_escape_close {
            self.dont_ask_escape = false;
            self.confirm_escape = true;
        } else {
            self.close_now(ctx);
        }
    }

    /// Answer to "Close the editor?". Remembers "don't ask again" in the settings.
    fn answer_escape(&mut self, ctx: &egui::Context, close: bool) {
        self.confirm_escape = false;
        if self.dont_ask_escape {
            let mut c = Config::load();
            c.confirm_escape_close = false;
            c.save().ok();
            self.config.confirm_escape_close = false;
        }
        if close {
            self.close_now(ctx);
        }
    }

    /// Starts the eyedropper for the color of `target`.
    pub fn start_eyedropper(&mut self, target: EyeTarget) {
        self.finish_text_edit();
        self.pick_src = Some(self.renderer.render(&self.doc.anns, None));
        self.eyedropper = Some(target);
    }

    /// The color of the annotated screenshot at image position `p`.
    pub fn color_at(&self, p: Pos2) -> Option<Color32> {
        let pm = self.pick_src.as_ref()?;
        if p.x < 0.0 || p.y < 0.0 {
            return None;
        }
        let px = pm.pixel(p.x as u32, p.y as u32)?.demultiply();
        Some(Color32::from_rgb(px.red(), px.green(), px.blue()))
    }

    /// Applies the picked color and ends the eyedropper.
    pub fn pick_color(&mut self, p: Pos2) {
        let (Some(target), Some(c)) = (self.eyedropper, self.color_at(p)) else { return };
        if target.fill {
            self.apply_style(target.tool, |s| s.fill = c);
        } else {
            self.apply_style(target.tool, |s| s.stroke = c);
        }
        self.cancel_eyedropper();
    }

    pub fn cancel_eyedropper(&mut self) {
        self.eyedropper = None;
        self.pick_src = None;
    }

    /// The emoji as a texture (for the picker and the live preview).
    pub fn emoji_texture(&mut self, ctx: &egui::Context, id: &str) -> Option<egui::TextureId> {
        if let Some(t) = self.emoji_tex.get(id) {
            return Some(t.id());
        }
        let pm = crate::emoji::raster(id, 96)?;
        let img = egui::ColorImage::from_rgba_premultiplied([96, 96], pm.data());
        let tex = ctx.load_texture(format!("emoji_{id}"), img, egui::TextureOptions::LINEAR);
        let tid = tex.id();
        self.emoji_tex.insert(id.to_string(), tex);
        Some(tid)
    }

    /// A texture made earlier by `emoji_texture` (the canvas cannot create textures).
    pub fn emoji_tex_id(&self, id: &str) -> Option<egui::TextureId> {
        self.emoji_tex.get(id).map(|t| t.id())
    }

    /// Small copy of the screenshot (cropped) for the background previews.
    pub fn shot_thumb(&mut self) -> Option<&tiny_skia::Pixmap> {
        if self.shot_thumb.is_none() {
            let full = crop_pixmap(self.renderer.base(), self.doc.crop);
            self.shot_thumb = frame::shrink_to_width(&full, 72.0).or(Some(full));
        }
        self.shot_thumb.as_ref()
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.confirm_escape || self.confirm_close || self.show_shortcuts {
            let (esc, enter) = ctx.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Enter)));
            if self.confirm_escape {
                if esc {
                    self.answer_escape(ctx, false);
                } else if enter {
                    self.answer_escape(ctx, true);
                }
            } else if esc {
                self.confirm_close = false;
                self.show_shortcuts = false;
            }
            return;
        }
        // Esc on an open popup (color picker, stamps) only closes that popup.
        let popup = ctx.memory(|m| m.any_popup_open());
        let typing = self.text_edit.is_some() || ctx.wants_keyboard_input() || popup;
        self.space_down = !typing && ctx.input(|i| i.key_down(egui::Key::Space));
        // Use the modifiers carried by each key event, not the live modifier state, so
        // quick key combinations are never missed.
        let mut presses: Vec<(egui::Key, egui::Modifiers)> = Vec::new();
        let mut copy = false;
        ctx.input(|i| {
            for e in &i.events {
                match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => presses.push((*key, *modifiers)),
                    egui::Event::Copy => copy = true,
                    _ => {}
                }
            }
        });

        if self.text_edit.is_some() {
            if presses.iter().any(|(k, m)| *k == egui::Key::Escape || (m.command && *k == egui::Key::Enter)) {
                self.finish_text_edit();
            }
            return;
        }
        if typing {
            return;
        }
        if copy {
            self.copy(ctx);
        }
        for (key, m) in presses {
            use egui::Key;
            if m.command {
                match key {
                    Key::Z if m.shift => self.redo(),
                    Key::Z => self.undo(),
                    Key::Y => self.redo(),
                    Key::S if m.shift => self.save_as(ctx),
                    Key::S => self.save_quick(ctx),
                    Key::Num0 => {
                        self.zoom = None;
                        self.pan = Vec2::ZERO;
                    }
                    Key::Num1 => self.zoom = Some(1.0 / ctx.pixels_per_point()),
                    Key::B if self.frame.active() => self.panel_collapsed = !self.panel_collapsed,
                    Key::Plus | Key::Equals => self.zoom = Some(self.last_zoom * 1.25),
                    Key::Minus => self.zoom = Some(self.last_zoom / 1.25),
                    _ => {}
                }
                continue;
            }
            match key {
                Key::Delete | Key::Backspace => self.delete_selected(),
                Key::F1 | Key::Questionmark => self.show_shortcuts = true,
                Key::Escape => self.escape(ctx),
                Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown => {
                    let step = if m.shift { 10.0 } else { 1.0 };
                    let d = match key {
                        Key::ArrowLeft => Vec2::new(-step, 0.0),
                        Key::ArrowRight => Vec2::new(step, 0.0),
                        Key::ArrowUp => Vec2::new(0.0, -step),
                        _ => Vec2::new(0.0, step),
                    };
                    if let Some(id) = self.selected {
                        self.checkpoint();
                        if let Some(a) = self.get_mut(id) {
                            a.translate(d);
                        }
                        self.invalidate();
                    }
                }
                _ => {
                    if let Some(t) = Tool::ALL.into_iter().find(|t| t.key() == key) {
                        if !m.any() || m.shift_only() {
                            self.set_tool(t);
                        }
                    }
                }
            }
        }
    }

    fn escape(&mut self, ctx: &egui::Context) {
        if self.eyedropper.is_some() {
            self.cancel_eyedropper();
            return;
        }
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Create { id, .. } => {
                if let Some(i) = self.find(id) {
                    self.doc.anns.remove(i);
                    self.undo.pop();
                }
                self.selected = None;
                self.tex_skip = None;
                self.invalidate();
            }
            Drag::Crop { .. } => {}
            Drag::None if self.selected.is_some() => self.selected = None,
            Drag::None => self.escape_close(ctx),
            other => self.drag = other,
        }
    }

    fn update_texture(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        // What is left after the toolbars and the side panel: the canvas of this frame.
        let canvas = ctx.available_rect().size();
        let layout = self.frame_layout();
        if let (Some((_, _, have)), Some(l)) = (&self.frame_tex, &layout) {
            let want = self.preview_scale(l, ppp, canvas);
            let redo = match self.zoom {
                // Zoomed in past the preview's detail, or far out.
                Some(_) => want > *have * 1.01 || want < *have / 2.5,
                // Window resized.
                None => (want - *have).abs() > *have * 0.02,
            };
            if redo {
                self.tex_dirty = true;
            }
        }
        if !self.tex_dirty && (!self.tiles.is_empty() || self.frame_tex.is_some()) {
            return;
        }
        self.tex_dirty = false;
        let max_side = ctx.input(|i| i.max_texture_side).clamp(1024, 8192);
        let opts = egui::TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Linear,
            wrap_mode: egui::TextureWrapMode::ClampToEdge,
            mipmap_mode: Some(egui::TextureFilter::Linear),
        };
        if let Some(l) = layout {
            // The framed output, rendered by the same code as the export, at screen size.
            let cap =
                if self.config.preview_quality == PreviewQuality::Full { max_side } else { max_side.min(4096) } as f32;
            let scale = self.preview_scale(&l, ppp, canvas).min(cap / l.size.x.max(l.size.y));
            // The screenshot at the size it has in the preview (annotations drawn, cropped).
            let width = (l.shot.width() * scale).round().max(1.0) as u32;
            let fresh = matches!(&self.frame_src, Some((g, w, _)) if *g == self.ann_gen && *w == width);
            if !fresh {
                if !matches!(&self.frame_full, Some((g, _)) if *g == self.ann_gen) {
                    let full = crop_pixmap(&self.renderer.render(&self.doc.anns, self.tex_skip), self.doc.crop);
                    self.frame_full = Some((self.ann_gen, full));
                }
                let full = &self.frame_full.as_ref().expect("set above").1;
                let shot = if (width as f32) < full.width() as f32 * 0.7 {
                    frame::shrink_to_width(full, width as f32)
                } else {
                    None
                };
                self.frame_src = Some((self.ann_gen, width, shot.unwrap_or_else(|| full.clone())));
            }
            let shot = &self.frame_src.as_ref().expect("set above").2;
            if let Some(out) = frame::compose(shot, &l, &self.frame, &self.fonts, scale) {
                let img = egui::ColorImage::from_rgba_premultiplied(
                    [out.width() as usize, out.height() as usize],
                    out.data(),
                );
                let opts = egui::TextureOptions { magnification: egui::TextureFilter::Linear, ..opts };
                match &mut self.frame_tex {
                    Some((t, lay, sc)) => {
                        t.set(img, opts);
                        *lay = l;
                        *sc = scale;
                    }
                    None => self.frame_tex = Some((ctx.load_texture("framed", img, opts), l, scale)),
                }
            }
            self.tiles.clear();
            return;
        }
        self.frame_tex = None;
        self.frame_full = None;
        self.frame_src = None;
        let pm = self.renderer.render(&self.doc.anns, self.tex_skip);
        let (w, h) = (pm.width() as usize, pm.height() as usize);
        let mut tiles = Vec::new();
        let mut reuse = std::mem::take(&mut self.tiles).into_iter();
        for ty in (0..h).step_by(max_side) {
            for tx in (0..w).step_by(max_side) {
                let (tw, th) = ((w - tx).min(max_side), (h - ty).min(max_side));
                let mut data = Vec::with_capacity(tw * th * 4);
                for y in ty..ty + th {
                    let s = (y * w + tx) * 4;
                    data.extend_from_slice(&pm.data()[s..s + tw * 4]);
                }
                let img = egui::ColorImage::from_rgba_premultiplied([tw, th], &data);
                let rect = Rect::from_min_size(Pos2::new(tx as f32, ty as f32), Vec2::new(tw as f32, th as f32));
                let tex = match reuse.next() {
                    Some(mut t) if t.rect == rect => {
                        t.tex.set(img, opts);
                        t.tex
                    }
                    _ => ctx.load_texture(format!("shot{tx}_{ty}"), img, opts),
                };
                tiles.push(Tile { rect, tex });
            }
        }
        self.tiles = tiles;
    }
}

impl eframe::App for EditorApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        crate::theme::pal().canvas.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request_close(ctx);
        }
        self.ctrl_select = self.text_edit.is_none() && ctx.input(|i| i.modifiers.ctrl);
        self.handle_shortcuts(ctx);
        ui::toolbar(self, ctx);
        ui::property_bar(self, ctx);
        ui::status_bar(self, ctx);
        if self.frame.active() {
            frame_ui::panel(self, ctx);
        }
        if self.doc.crop != self.thumb_crop {
            // The background previews show the (cropped) screenshot.
            self.thumb_crop = self.doc.crop;
            self.shot_thumb = None;
            self.frame_thumbs.clear();
        }
        // The live preview of a stamp needs its texture (only made here, not while drawing).
        let mut ids: Vec<String> = self
            .doc
            .anns
            .iter()
            .filter_map(|a| if let Kind::Emoji(e) = &a.kind { Some(e.clone()) } else { None })
            .collect();
        ids.push(self.emoji.clone());
        for id in ids {
            self.emoji_texture(ctx, &id);
        }
        self.update_texture(ctx);
        canvas::show(self, ctx);
        if self.confirm_close {
            ui::confirm_close_dialog(self, ctx);
        }
        if self.confirm_escape {
            ui::confirm_escape_dialog(self, ctx);
        }
        if self.show_shortcuts {
            ui::shortcuts_window(self, ctx);
        }
        ui::toast_overlay(self, ctx);
        if let Some(at) = self.copied_at {
            if at.elapsed().as_secs_f32() > 1.6 {
                self.copied_at = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        if let Some(t) = &self.toast {
            if t.at.elapsed().as_secs_f32() > 4.0 {
                self.toast = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        self.persist_prefs(ctx, false);
    }
}

/// Shared look & feel (also used by the settings, update and welcome windows).
pub fn setup_theme(ctx: &egui::Context) {
    ui::setup_style(ctx, &Fonts::load());
}

pub use ui::{brand_header, logo_texture};

/// Matches the title bar of a window to the theme (Windows).
pub fn style_title_bar(cc: &eframe::CreationContext<'_>) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(h) = cc.window_handle() {
            if let RawWindowHandle::Win32(w) = h.as_raw() {
                platform::set_title_bar_theme(w.hwnd.get(), crate::theme::pal().dark);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = cc;
}
