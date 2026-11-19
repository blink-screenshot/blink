//! The drawing surface: view transform, mouse interaction and live previews.

use super::textsel::{self, to_rect};
use super::{ui, Drag, EditorApp, Handle, TextEditing, Tool};
use crate::annot::{arrow_geometry, callout_outline, push_point, shape_outline, Annotation, HeadShape, Kind, Style};
use crate::render::{
    callout_box, callout_padding, callout_text_color, contrast_color, step_text_size, text_box_size, text_fixed_width,
    text_min_width, text_padding, text_wrap_width,
};
use crate::theme;
use eframe::egui::{
    self, pos2, vec2, Color32, CursorIcon, FontFamily, FontId, Painter, PointerButton, Pos2, Rect, Sense, Shape,
    Stroke, StrokeKind, Vec2,
};

const HANDLE_PX: f32 = 8.0;

struct View {
    origin: Pos2,
    zoom: f32,
    view: Rect,
}

impl View {
    fn to_screen(&self, p: Pos2) -> Pos2 {
        self.origin + (p - self.view.min) * self.zoom
    }
    fn to_img(&self, s: Pos2) -> Pos2 {
        self.view.min + (s - self.origin) / self.zoom
    }
    fn rect_to_screen(&self, r: Rect) -> Rect {
        Rect::from_two_pos(self.to_screen(r.min), self.to_screen(r.max))
    }
}

pub fn show(app: &mut EditorApp, ctx: &egui::Context) {
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(theme::pal().canvas)).show(ctx, |ui| {
        let avail = ui.available_rect_before_wrap();
        let (resp, painter) = ui.allocate_painter(avail.size(), Sense::click_and_drag());
        let canvas = resp.rect;
        let ppp = ctx.pixels_per_point();
        let view_rect = app.view_rect();
        // The "document" is what gets exported: the framed output, or the (cropped) image.
        let layout = app.frame_layout();
        let doc = layout.map_or(view_rect.size(), |l| l.size);
        let (shot_min, k) = match &layout {
            Some(l) => (l.shot.min.to_vec2(), l.scale(view_rect.width())),
            None => (Vec2::ZERO, 1.0),
        };

        // ---- zoom & pan (document pixels -> screen points) ----
        let fit = ((canvas.width() - 40.0) / doc.x).min((canvas.height() - 40.0) / doc.y).max(0.01);
        let mut zoom = app.zoom.unwrap_or_else(|| fit.min(1.0 / ppp)).clamp(0.02, 40.0);
        if app.zoom.is_none() {
            app.pan = Vec2::ZERO;
        }
        let mut origin = canvas.center() + app.pan - doc * zoom / 2.0;
        if resp.hovered() {
            let (zd, scroll, hover) = ctx.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.pointer.hover_pos()));
            if zd != 1.0 {
                if let Some(s) = hover {
                    let p = (s - origin) / zoom;
                    let nz = (zoom * zd).clamp(0.02, 40.0);
                    let new_origin = s - p * nz;
                    app.pan = new_origin - canvas.center() + doc * nz / 2.0;
                    app.zoom = Some(nz);
                    zoom = nz;
                    origin = new_origin;
                }
            } else if scroll != Vec2::ZERO {
                app.zoom = Some(zoom);
                app.pan += scroll;
                origin += scroll;
            }
        }
        // Snap to the physical pixel grid: at 100% every image pixel maps to exactly one
        // screen pixel, so screenshots look crisp instead of blurry/rescaled.
        origin = pos2((origin.x * ppp).round() / ppp, (origin.y * ppp).round() / ppp);
        app.last_zoom = zoom;
        let v = View { origin: origin + shot_min * zoom, zoom: zoom * k, view: view_rect };
        let doc_screen = Rect::from_min_size(origin, doc * zoom);

        // ---- image ----
        let img_screen = v.rect_to_screen(view_rect);
        match (&layout, app.frame_tex.as_ref().map(|(t, ..)| t.id())) {
            (Some(_), Some(tex)) => {
                checkerboard(app, ctx, &painter, doc_screen);
                painter.image(tex, doc_screen, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
            }
            _ => {
                painter.rect_filled(img_screen.expand(1.0), 0.0, Color32::from_black_alpha(90));
                for t in &app.tiles {
                    let part = t.rect.intersect(view_rect);
                    if !part.is_positive() {
                        continue;
                    }
                    let uv = Rect::from_min_max(
                        ((part.min - t.rect.min) / t.rect.size()).to_pos2(),
                        ((part.max - t.rect.min) / t.rect.size()).to_pos2(),
                    );
                    painter.image(t.tex.id(), v.rect_to_screen(part), uv, Color32::WHITE);
                }
            }
        }
        // Where live previews may draw: the visible screenshot area.
        let visible = match &layout {
            Some(l) => Rect::from_min_size(origin + l.clip.min.to_vec2() * zoom, l.clip.size() * zoom),
            None => img_screen,
        };

        // ---- interaction ----
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        app.hover = resp.hover_pos().map(|p| v.to_img(p));
        let shift = ctx.input(|i| i.modifiers.shift);
        let tool = app.effective_tool();
        if app.eyedropper.is_some() {
            eyedropper(app, ctx, &resp, &painter, &v);
            return;
        }
        let panning =
            resp.dragged_by(PointerButton::Middle) || (app.space_down && resp.dragged_by(PointerButton::Primary));
        if panning {
            app.zoom = Some(zoom);
            app.pan += resp.drag_delta();
            app.drag = Drag::Pan;
            ctx.set_cursor_icon(CursorIcon::Grabbing);
        } else if matches!(app.drag, Drag::Pan) {
            app.drag = Drag::None;
        } else {
            if resp.drag_started_by(PointerButton::Primary) {
                let start = ctx.input(|i| i.pointer.press_origin()).or(pointer).unwrap_or_default();
                begin_drag(app, &v, start, tool);
            }
            if resp.dragged_by(PointerButton::Primary) {
                if let Some(p) = pointer {
                    update_drag(app, v.to_img(p), shift);
                }
            }
            if resp.drag_stopped_by(PointerButton::Primary) {
                end_drag(app);
            }
            if resp.triple_clicked_by(PointerButton::Primary) {
                if let Some(p) = pointer {
                    triple_click(app, v.to_img(p), &v, tool);
                }
            } else if resp.double_clicked_by(PointerButton::Primary) {
                if let Some(p) = pointer {
                    double_click(app, v.to_img(p), zoom, &v, tool);
                }
            } else if resp.clicked_by(PointerButton::Primary) {
                if let Some(p) = pointer {
                    click(app, &v, p, tool);
                }
            }
            if resp.hovered() {
                set_cursor(app, ctx, &v, pointer, tool);
            }
        }

        // ---- overlays ----
        let clip = painter.with_clip_rect(visible.intersect(canvas));
        let edit_id = app.text_edit.as_ref().map(|t| t.id);
        if let Some(id) = app.tex_skip {
            if Some(id) != edit_id {
                if let Some(a) = app.get(id) {
                    paint_preview(&clip, app, a, &v);
                }
            }
        }
        if let Some(a) = app.selected.and_then(|id| app.get(id)) {
            if Some(a.id) != edit_id {
                paint_selection(&painter, app, a, &v);
            }
        }
        if let Drag::Crop { start, cur } = app.drag {
            paint_crop(
                &painter,
                img_screen,
                v.rect_to_screen(Rect::from_two_pos(start, cur)),
                Rect::from_two_pos(start, cur),
            );
        }
        paint_text_layer(app, &clip, &v);
        text_editor(app, ctx, &v);
        // Floating parts: reading, the private info panel, Copy / Hide at a selection.
        let busy = app.ocr_running() && (app.tool == Tool::SelectText || app.after_ocr.is_some());
        if busy {
            textsel::reading_pill(ctx, canvas);
        }
        if app.privacy.is_none() && app.tool == Tool::SelectText && matches!(app.drag, Drag::None) {
            let rects = app.page().map(|p| p.selection_rects(&app.text_sel)).unwrap_or_default();
            if let Some(all) = rects.iter().map(|r| v.rect_to_screen(to_rect(r))).reduce(|a, b| a.union(b)) {
                if canvas.intersects(all) {
                    textsel::selection_actions(app, ctx, all, canvas);
                }
            }
        }
    });
}

/// The read text (text tool): every word faintly, the selection clearly, the selection box;
/// and the suggestions of "Find private info" (with any tool).
fn paint_text_layer(app: &EditorApp, p: &Painter, v: &View) {
    // The redaction tool: what a click would hide.
    if app.tool == Tool::Redact && !app.ctrl_select && matches!(app.drag, Drag::None) {
        if let Some(h) = app.hover.filter(|h| hit_test(app, *h, v.zoom).is_none()) {
            if let Some((b, kind)) = app.redact_target(h, v.zoom) {
                let r = v.rect_to_screen(to_rect(&b));
                p.rect_filled(r, 2.0, theme::ACCENT.gamma_multiply(0.22));
                p.rect_stroke(r, 2.0, Stroke::new(1.5_f32, theme::ACCENT), StrokeKind::Outside);
                if let Some(kind) = kind {
                    let galley = p.layout_no_wrap(kind.name().to_string(), FontId::proportional(11.0), theme::INK);
                    let tag = Rect::from_min_size(
                        pos2(r.left() - 2.0, r.top() - galley.size().y - 6.0),
                        galley.size() + vec2(8.0, 3.0),
                    );
                    p.rect_filled(tag, 3.0, theme::ACCENT);
                    p.galley(tag.min + vec2(4.0, 1.5), galley, theme::INK);
                }
            }
        }
    }
    if let (Tool::SelectText, Some(page)) = (app.tool, app.page()) {
        for (i, _, w) in page.words() {
            if app.word_visible(i) {
                p.rect_filled(v.rect_to_screen(to_rect(&w.rect)).expand(1.0), 2.0, theme::ACCENT.gamma_multiply(0.10));
            }
        }
        for r in page.selection_rects(&app.text_sel) {
            p.rect_filled(v.rect_to_screen(to_rect(&r)).expand(1.5), 2.0, theme::ACCENT.gamma_multiply(0.38));
        }
        if let Drag::TextMarquee { start, cur } = app.drag {
            let r = v.rect_to_screen(Rect::from_two_pos(start, cur));
            p.rect_filled(r, 0.0, theme::ACCENT.gamma_multiply(0.06));
            p.rect_stroke(r, 0.0, Stroke::new(1.0_f32, theme::ACCENT), StrokeKind::Inside);
        }
    }
    if let Some(priv_) = &app.privacy {
        for (f, on) in priv_.findings.iter().zip(&priv_.on) {
            let r = v.rect_to_screen(to_rect(&f.rect));
            if *on {
                p.rect_filled(r, 2.0, theme::ACCENT.gamma_multiply(0.30));
                p.rect_stroke(r, 2.0, Stroke::new(2.0_f32, theme::ACCENT), StrokeKind::Outside);
            } else {
                p.rect_stroke(r, 2.0, Stroke::new(1.5_f32, Color32::from_gray(150)), StrokeKind::Outside);
            }
            // What it is, above the box, for the one under the mouse (labels on all of them
            // would cover the text around them).
            if !app.hover.is_some_and(|h| to_rect(&f.rect).contains(h)) {
                continue;
            }
            let font = FontId::proportional(11.0);
            let galley =
                p.layout_no_wrap(f.kind.name().to_string(), font, if *on { theme::INK } else { Color32::WHITE });
            let tag = Rect::from_min_size(
                pos2(r.left() - 2.0, r.top() - galley.size().y - 6.0),
                galley.size() + vec2(8.0, 3.0),
            );
            p.rect_filled(tag, 3.0, if *on { theme::ACCENT } else { Color32::from_gray(90) });
            p.galley(tag.min + vec2(4.0, 1.5), galley, Color32::WHITE);
        }
    }
}

// ---------------------------------------------------------------------------------------

fn handles(app: &EditorApp, a: &Annotation) -> Vec<(Handle, Pos2)> {
    if let Kind::Text(_) = a.kind {
        // Text boxes only change width; the height follows from the wrapped text.
        let r = a.bounds(app.text_size());
        return vec![(Handle::Rect(-1, 0), r.left_center()), (Handle::Rect(1, 0), r.right_center())];
    }
    if a.is_segment() {
        return vec![(Handle::SegA, a.a), (Handle::SegB, a.b)];
    }
    if !a.is_rect_like() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let r = match &a.kind {
        Kind::Callout { tail, .. } => {
            out.push((Handle::Tail, *tail));
            callout_box(&app.fonts, a, app.renderer.width() as f32)
        }
        _ => a.rect(),
    };
    for hy in -1i8..=1 {
        for hx in -1i8..=1 {
            if hx == 0 && hy == 0 {
                continue;
            }
            let x = match hx {
                -1 => r.min.x,
                0 => r.center().x,
                _ => r.max.x,
            };
            let y = match hy {
                -1 => r.min.y,
                0 => r.center().y,
                _ => r.max.y,
            };
            out.push((Handle::Rect(hx, hy), pos2(x, y)));
        }
    }
    out
}

fn handle_at(app: &EditorApp, v: &View, screen: Pos2) -> Option<(u64, Handle)> {
    let a = app.selected.and_then(|id| app.get(id))?;
    handles(app, a).into_iter().find(|(_, p)| (v.to_screen(*p) - screen).length() <= HANDLE_PX).map(|(h, _)| (a.id, h))
}

fn hit_test(app: &EditorApp, p: Pos2, zoom: f32) -> Option<u64> {
    let tol = 5.0 / zoom;
    let ts = app.text_size();
    app.doc.anns.iter().rev().find(|a| a.hit(p, tol, &ts)).map(|a| a.id)
}

/// Gray/white checkerboard behind the framed output, so transparency is visible.
fn checkerboard(app: &mut EditorApp, ctx: &egui::Context, p: &Painter, r: Rect) {
    let tex = app.checker.get_or_insert_with(|| {
        let (a, b) = (Color32::from_gray(58), Color32::from_gray(44));
        let img = egui::ColorImage { size: [2, 2], pixels: vec![a, b, b, a] };
        let opts = egui::TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Nearest,
            wrap_mode: egui::TextureWrapMode::Repeat,
            mipmap_mode: None,
        };
        ctx.load_texture("checker", img, opts)
    });
    let cell = 16.0;
    let uv = Rect::from_min_size(Pos2::ZERO, r.size() / (cell * 2.0));
    p.image(tex.id(), r, uv, Color32::WHITE);
}

fn begin_drag(app: &mut EditorApp, v: &View, start_screen: Pos2, tool: Tool) {
    app.finish_text_edit();
    let start = v.to_img(start_screen);
    if let Some((id, handle)) = handle_at(app, v, start_screen) {
        if let Some(a) = app.get_mut(id) {
            if a.is_rect_like() {
                let r = a.rect();
                a.a = r.min;
                a.b = r.max;
            }
        }
        // A callout's box is resized from the size it shows (grown to fit its text).
        if let Some(a) = app.get(id).filter(|a| matches!(a.kind, Kind::Callout { .. })) {
            let r = callout_box(&app.fonts, a, app.renderer.width() as f32);
            if let Some(a) = app.get_mut(id) {
                a.b = r.max;
            }
        }
        // Dragging a text edge fixes its width at what it is now.
        if let Some(a) = app.get(id).filter(|a| matches!(a.kind, Kind::Text(_)) && text_fixed_width(a).is_none()) {
            let w = text_box_size(&app.fonts, a, app.renderer.width() as f32).x;
            let (ax, ay) = (a.a.x, a.a.y);
            if let Some(a) = app.get_mut(id) {
                a.b = pos2(ax + w, ay);
            }
        }
        app.drag = Drag::Handle { id, handle, moved: false };
        app.tex_skip = Some(id);
        app.invalidate();
        return;
    }
    match tool {
        Tool::Select | Tool::Text => {
            let hit = hit_test(app, start, v.zoom);
            let hit = hit.filter(|id| tool == Tool::Select || app.get(*id).is_some_and(|a| a.kind.text().is_some()));
            match hit {
                Some(id) => {
                    app.selected = Some(id);
                    app.drag = Drag::Move { id, last: start, moved: false };
                    app.tex_skip = Some(id);
                    app.invalidate();
                }
                None => app.selected = None,
            }
        }
        Tool::Crop => app.drag = Drag::Crop { start, cur: start },
        Tool::SelectText => {
            let slack = 3.0 / v.zoom;
            let at = app.page().and_then(|p| p.word_at(start.x, start.y, slack)).filter(|i| app.word_visible(*i));
            match at {
                Some(i) => {
                    app.text_sel = vec![i];
                    app.drag = Drag::TextFlow { anchor: i };
                }
                None => {
                    app.text_sel.clear();
                    app.drag = Drag::TextMarquee { start, cur: start };
                }
            }
        }
        Tool::Step => {
            let id = add_step(app, start);
            app.drag = Drag::Move { id, last: start, moved: true };
            app.tex_skip = Some(id);
        }
        _ => {
            let kind = match tool {
                Tool::Rect => Kind::Rect,
                Tool::Ellipse => Kind::Ellipse,
                Tool::Arrow => Kind::Arrow,
                Tool::Line => Kind::Line,
                Tool::Pen => Kind::Freehand(vec![start]),
                Tool::Highlight => Kind::Highlight,
                Tool::Redact => Kind::Redact(app.redact_mode),
                Tool::Shape => Kind::Shape(app.shape),
                Tool::Callout => Kind::Callout { text: String::new(), tail: start },
                Tool::Emoji => Kind::Emoji(app.emoji.clone()),
                _ => return,
            };
            app.checkpoint();
            let id = app.new_id();
            let style = app.style_for(tool);
            app.doc.anns.push(Annotation { id, kind, a: start, b: start, style });
            app.selected = Some(id);
            app.drag = Drag::Create { id, start };
            app.tex_skip = Some(id);
            app.invalidate();
        }
    }
}

fn add_step(app: &mut EditorApp, at: Pos2) -> u64 {
    app.checkpoint();
    let n =
        app.doc.anns.iter().filter_map(|a| if let Kind::Step(n) = a.kind { Some(n) } else { None }).max().unwrap_or(0)
            + 1;
    let id = app.new_id();
    let style = app.style_for(Tool::Step);
    app.doc.anns.push(Annotation { id, kind: Kind::Step(n), a: at, b: at, style });
    app.selected = Some(id);
    app.invalidate();
    id
}

fn constrain(start: Pos2, cur: Pos2, segment: bool) -> Pos2 {
    let d = cur - start;
    if segment {
        let len = d.length();
        let step = std::f32::consts::FRAC_PI_4;
        let ang = (d.y.atan2(d.x) / step).round() * step;
        start + vec2(ang.cos(), ang.sin()) * len
    } else {
        let m = d.x.abs().max(d.y.abs());
        start + vec2(m * d.x.signum(), m * d.y.signum())
    }
}

fn update_drag(app: &mut EditorApp, cur: Pos2, shift: bool) {
    match app.drag {
        Drag::Create { id, start } => {
            if let Some(a) = app.get_mut(id) {
                match &mut a.kind {
                    Kind::Freehand(pts) => push_point(pts, cur, 1.5),
                    // Stamps stay square.
                    Kind::Emoji(_) => a.b = constrain(start, cur, false),
                    Kind::Callout { tail, .. } => {
                        a.b = if shift { constrain(start, cur, false) } else { cur };
                        *tail = start;
                    }
                    _ => {
                        let seg = a.is_segment();
                        a.b = if shift { constrain(start, cur, seg) } else { cur };
                    }
                }
            }
        }
        Drag::Move { id, last, moved } => {
            let d = cur - last;
            if d != Vec2::ZERO {
                if !moved {
                    app.checkpoint();
                }
                if let Some(a) = app.get_mut(id) {
                    a.translate(d);
                }
                app.drag = Drag::Move { id, last: cur, moved: true };
            }
        }
        Drag::Handle { id, handle, moved } => {
            if !moved {
                app.checkpoint();
                app.drag = Drag::Handle { id, handle, moved: true };
            }
            if let Some(a) = app.get_mut(id) {
                if let (Kind::Text(_), Handle::Rect(hx, _)) = (&a.kind, handle) {
                    let min_w = text_min_width(a.style.font_size);
                    if hx < 0 {
                        a.a.x = cur.x.min(a.b.x - min_w);
                    } else {
                        a.b.x = cur.x.max(a.a.x + min_w);
                    }
                    a.b.y = a.a.y;
                    return;
                }
                let square = matches!(a.kind, Kind::Emoji(_));
                match handle {
                    Handle::Tail => {
                        if let Kind::Callout { tail, .. } = &mut a.kind {
                            *tail = cur;
                        }
                    }
                    Handle::Rect(hx, hy) if square => {
                        // Resize from the opposite corner, keeping the stamp square.
                        let anchor = pos2(if hx < 0 { a.b.x } else { a.a.x }, if hy < 0 { a.b.y } else { a.a.y });
                        let d = cur - anchor;
                        let side = match (hx, hy) {
                            (0, _) => d.y.abs(),
                            (_, 0) => d.x.abs(),
                            _ => d.x.abs().max(d.y.abs()),
                        }
                        .max(8.0);
                        let sx = if hx < 0 { -1.0 } else { 1.0 };
                        let sy = if hy < 0 { -1.0 } else { 1.0 };
                        let other = anchor + vec2(side * sx, side * sy);
                        a.a = pos2(anchor.x.min(other.x), anchor.y.min(other.y));
                        a.b = pos2(anchor.x.max(other.x), anchor.y.max(other.y));
                    }
                    Handle::SegA => a.a = if shift { constrain(a.b, cur, true) } else { cur },
                    Handle::SegB => a.b = if shift { constrain(a.a, cur, true) } else { cur },
                    Handle::Rect(hx, hy) => {
                        if hx < 0 {
                            a.a.x = cur.x;
                        } else if hx > 0 {
                            a.b.x = cur.x;
                        }
                        if hy < 0 {
                            a.a.y = cur.y;
                        } else if hy > 0 {
                            a.b.y = cur.y;
                        }
                    }
                }
            }
        }
        Drag::Crop { start, .. } => app.drag = Drag::Crop { start, cur },
        Drag::TextFlow { anchor } => {
            if let Some(end) = app.page().and_then(|p| p.nearest_word(cur.x, cur.y)) {
                let (a, b) = (anchor.min(end), anchor.max(end));
                app.text_sel = (a..=b).filter(|i| app.word_visible(*i)).collect();
            }
        }
        Drag::TextMarquee { start, .. } => {
            let r = Rect::from_two_pos(start, cur);
            let b = crate::ocr::Bounds::new(r.min.x, r.min.y, r.width(), r.height());
            let words = app.page().map(|p| p.words_in(&b)).unwrap_or_default();
            app.text_sel = words.into_iter().filter(|i| app.word_visible(*i)).collect();
            app.drag = Drag::TextMarquee { start, cur };
        }
        Drag::None | Drag::Pan => {}
    }
}

fn end_drag(app: &mut EditorApp) {
    if matches!(app.drag, Drag::TextFlow { .. } | Drag::TextMarquee { .. }) {
        // The selection stays; nothing in the image changed.
        app.drag = Drag::None;
        return;
    }
    match std::mem::replace(&mut app.drag, Drag::None) {
        Drag::Create { id, start } => {
            if let Some(i) = app.find(id) {
                let a = &mut app.doc.anns[i];
                let small = a.rect().width() < 8.0 || a.rect().height() < 8.0;
                let degenerate = match &a.kind {
                    Kind::Freehand(p) => p.len() < 2,
                    Kind::Arrow | Kind::Line => (a.b - a.a).length() < 3.0,
                    // A tiny drag places a stamp / callout at its normal size.
                    Kind::Emoji(_) | Kind::Callout { .. } => false,
                    _ => a.rect().width() < 3.0 || a.rect().height() < 3.0,
                };
                if small && matches!(a.kind, Kind::Emoji(_)) {
                    let side = a.style.font_size;
                    a.a = start - vec2(side, side) / 2.0;
                    a.b = a.a + vec2(side, side);
                }
                if let Kind::Callout { tail, .. } = &mut a.kind {
                    if small {
                        let size = a.style.font_size;
                        a.a = start;
                        a.b = start + vec2(size * 9.0, size * 2.4);
                    }
                    let r = Rect::from_two_pos(a.a, a.b);
                    (a.a, a.b) = (r.min, r.max);
                    // Pointer down-left, out of the box.
                    *tail = pos2(r.min.x + r.width() * 0.25, r.max.y + (r.height() * 0.6).max(a.style.font_size * 1.6));
                }
                if degenerate {
                    app.doc.anns.remove(i);
                    app.undo.pop();
                    app.selected = None;
                } else if matches!(app.doc.anns[i].kind, Kind::Callout { .. }) {
                    // Type the text right away.
                    app.text_edit = Some(TextEditing { id, focus_set: false, created: true });
                    app.selected = Some(id);
                }
            }
        }
        Drag::Crop { start, cur } => {
            let r = Rect::from_two_pos(start, cur).intersect(app.view_rect());
            let r = Rect::from_min_max(r.min.round(), r.max.round());
            if r.width() >= 4.0 && r.height() >= 4.0 {
                app.checkpoint();
                app.doc.crop = Some(r);
                app.zoom = None;
                app.pan = Vec2::ZERO;
            }
        }
        _ => {}
    }
    // A new callout is typed into right away: the editor draws it meanwhile.
    app.tex_skip = app.text_edit.as_ref().map(|t| t.id);
    app.invalidate();
}

fn click(app: &mut EditorApp, v: &View, screen: Pos2, tool: Tool) {
    app.finish_text_edit();
    let p = v.to_img(screen);
    // A suggested redaction is turned off (or on again) by clicking it, with any tool.
    if app.toggle_finding_at(p) {
        return;
    }
    match tool {
        Tool::SelectText => {
            let at = app.page().and_then(|pg| pg.word_at(p.x, p.y, 3.0 / v.zoom)).filter(|i| app.word_visible(*i));
            app.text_sel = at.into_iter().collect();
        }
        Tool::Callout | Tool::Emoji => {
            let same = |a: &Annotation| Tool::for_kind(&a.kind) == tool;
            if let Some(id) = hit_test(app, p, v.zoom).filter(|id| app.get(*id).is_some_and(same)) {
                if tool == Tool::Callout {
                    start_text_edit(app, id, false);
                } else {
                    app.selected = Some(id);
                }
                return;
            }
            // A click places one at its normal size (the same as a tiny drag).
            app.checkpoint();
            let id = app.new_id();
            let style = app.style_for(tool);
            let kind = if tool == Tool::Emoji {
                Kind::Emoji(app.emoji.clone())
            } else {
                Kind::Callout { text: String::new(), tail: p }
            };
            app.doc.anns.push(Annotation { id, kind, a: p, b: p, style });
            app.drag = Drag::Create { id, start: p };
            end_drag(app);
        }
        Tool::Text => {
            if let Some(id) =
                hit_test(app, p, v.zoom).filter(|id| matches!(app.get(*id).map(|a| &a.kind), Some(Kind::Text(_))))
            {
                start_text_edit(app, id, false);
            } else {
                app.checkpoint();
                let id = app.new_id();
                let style = app.style_for(Tool::Text);
                let pad = text_padding(style.font_size);
                let at = p - vec2(pad, pad + style.font_size * 0.6);
                app.doc.anns.push(Annotation { id, kind: Kind::Text(String::new()), a: at, b: at, style });
                start_text_edit(app, id, true);
            }
        }
        Tool::Step => {
            add_step(app, p);
        }
        Tool::Redact => {
            // An existing redaction is selected; else the word or private item there is hidden.
            app.selected = hit_test(app, p, v.zoom);
            if app.selected.is_none() {
                if let Some((b, _)) = app.redact_target(p, v.zoom) {
                    app.redact_bounds(b);
                }
            }
        }
        _ => app.selected = hit_test(app, p, v.zoom),
    }
}

/// The words of the line (`paragraph`: of the paragraph) at `p`, in the text tool.
fn select_line_at(app: &mut EditorApp, p: Pos2, v: &View, paragraph: bool) {
    let Some(page) = app.page() else { return };
    let Some(li) = page.line_at(p.x, p.y, 3.0 / v.zoom) else { return };
    let lines = if paragraph { page.paragraph(li) } else { li..=li };
    let words: Vec<usize> = lines.flat_map(|l| page.line_words(l)).collect();
    app.text_sel = words.into_iter().filter(|i| app.word_visible(*i)).collect();
}

fn triple_click(app: &mut EditorApp, p: Pos2, v: &View, tool: Tool) {
    if tool == Tool::SelectText {
        select_line_at(app, p, v, true);
    }
}

fn double_click(app: &mut EditorApp, p: Pos2, zoom: f32, v: &View, tool: Tool) {
    if tool == Tool::SelectText {
        return select_line_at(app, p, v, false);
    }
    if let Some(id) = hit_test(app, p, zoom) {
        if app.get(id).is_some_and(|a| a.kind.text().is_some()) {
            app.finish_text_edit();
            start_text_edit(app, id, false);
        }
    }
}

fn start_text_edit(app: &mut EditorApp, id: u64, created: bool) {
    if !created {
        app.checkpoint();
    }
    app.selected = Some(id);
    app.text_edit = Some(TextEditing { id, focus_set: false, created });
    app.tex_skip = Some(id);
    app.invalidate();
}

fn set_cursor(app: &EditorApp, ctx: &egui::Context, v: &View, pointer: Option<Pos2>, tool: Tool) {
    let Some(p) = pointer else { return };
    if app.space_down {
        ctx.set_cursor_icon(CursorIcon::Grab);
        return;
    }
    if let Some((_, h)) = handle_at(app, v, p) {
        ctx.set_cursor_icon(match h {
            Handle::Rect(-1, -1) | Handle::Rect(1, 1) => CursorIcon::ResizeNwSe,
            Handle::Rect(1, -1) | Handle::Rect(-1, 1) => CursorIcon::ResizeNeSw,
            Handle::Rect(0, _) => CursorIcon::ResizeVertical,
            Handle::Rect(_, 0) => CursorIcon::ResizeHorizontal,
            Handle::Tail => CursorIcon::Move,
            _ => CursorIcon::Crosshair,
        });
        return;
    }
    let icon = match tool {
        Tool::Select => {
            if matches!(app.drag, Drag::Move { .. }) || hit_test(app, v.to_img(p), v.zoom).is_some() {
                CursorIcon::Move
            } else {
                CursorIcon::Default
            }
        }
        Tool::Text | Tool::Callout => CursorIcon::Text,
        Tool::SelectText => {
            let q = v.to_img(p);
            if app.ocr_running() {
                CursorIcon::Progress
            } else if app.page().and_then(|pg| pg.word_at(q.x, q.y, 3.0 / v.zoom)).is_some() {
                CursorIcon::Text
            } else {
                CursorIcon::Crosshair
            }
        }
        Tool::Redact => {
            let q = v.to_img(p);
            if hit_test(app, q, v.zoom).is_none() && app.redact_target(q, v.zoom).is_some() {
                CursorIcon::PointingHand
            } else {
                CursorIcon::Crosshair
            }
        }
        _ => CursorIcon::Crosshair,
    };
    ctx.set_cursor_icon(icon);
}

// ---- painting ---------------------------------------------------------------------------

fn ellipse_points(r: Rect) -> Vec<Pos2> {
    let n = 72;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            r.center() + vec2(t.cos() * r.width() / 2.0, t.sin() * r.height() / 2.0)
        })
        .collect()
}

pub fn text_family(bold: bool, italic: bool) -> FontFamily {
    FontFamily::Name(
        match (bold, italic) {
            (false, false) => "annot",
            (true, false) => "annot_bold",
            (false, true) => "annot_italic",
            (true, true) => "annot_bold_italic",
        }
        .into(),
    )
}

/// egui text layout matching a text annotation's style at `size` points, wrapped at `wrap` points.
fn text_job(text: &str, st: &Style, size: f32, color: Color32, wrap: f32) -> egui::text::LayoutJob {
    let line = Stroke::new((size * 0.065).max(1.0), color);
    let format = egui::TextFormat {
        font_id: FontId::new(size.max(1.0), text_family(st.bold, st.italic)),
        color,
        underline: if st.underline { line } else { Stroke::NONE },
        strikethrough: if st.strike { line } else { Stroke::NONE },
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), format);
    job.wrap.max_width = wrap;
    job
}

/// Approximate live preview of an annotation (used while it is being dragged/created;
/// everything else is shown from the exact software render).
fn paint_preview(p: &Painter, app: &EditorApp, a: &Annotation, v: &View) {
    let z = v.zoom;
    let st = &a.style;
    let stroke = Stroke::new((st.width * z).max(1.0), st.stroke);
    match &a.kind {
        Kind::Rect => {
            let r = v.rect_to_screen(a.rect());
            if st.fill.a() > 0 {
                p.rect_filled(r, 0.0, st.fill);
            }
            if st.width > 0.0 {
                p.rect_stroke(r, 0.0, stroke, StrokeKind::Middle);
            }
        }
        Kind::Ellipse => {
            let pts = ellipse_points(v.rect_to_screen(a.rect()));
            if st.fill.a() > 0 {
                p.add(Shape::convex_polygon(pts.clone(), st.fill, Stroke::NONE));
            }
            if st.width > 0.0 {
                p.add(Shape::closed_line(pts, stroke));
            }
        }
        Kind::Line => {
            p.line_segment([v.to_screen(a.a), v.to_screen(a.b)], stroke);
        }
        Kind::Arrow => {
            let ((s0, s1), heads) = arrow_geometry(a.a, a.b, st.width, st.head, st.both_ends);
            p.line_segment([v.to_screen(s0), v.to_screen(s1)], stroke);
            for h in heads {
                match h {
                    HeadShape::Fill(pts) => {
                        let pts: Vec<Pos2> = pts.iter().map(|q| v.to_screen(*q)).collect();
                        p.add(Shape::convex_polygon(pts, st.stroke, Stroke::NONE));
                    }
                    HeadShape::Stroke(pts) => {
                        p.add(Shape::line(pts.iter().map(|q| v.to_screen(*q)).collect(), stroke));
                    }
                    HeadShape::Dot(c, r) => {
                        p.circle_filled(v.to_screen(c), r * z, st.stroke);
                    }
                }
            }
        }
        Kind::Freehand(pts) => {
            let pts: Vec<Pos2> = pts.iter().map(|q| v.to_screen(*q)).collect();
            p.add(Shape::line(pts, stroke));
        }
        Kind::Highlight => {
            let [r, g, b, _] = st.stroke.to_array();
            p.rect_filled(v.rect_to_screen(a.rect()), 0.0, Color32::from_rgba_unmultiplied(r, g, b, 110));
        }
        Kind::Redact(_) => {
            let r = v.rect_to_screen(a.rect());
            p.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(110, 110, 120, 200));
            p.rect_stroke(r, 0.0, Stroke::new(1.0_f32, Color32::WHITE), StrokeKind::Inside);
        }
        Kind::Step(n) => {
            let c = v.to_screen(a.a);
            let r = a.step_radius();
            p.circle_filled(c, r * z, st.stroke);
            p.circle_stroke(c, r * z, Stroke::new((r * z * 0.12).max(1.0), Color32::WHITE));
            let em = step_text_size(r, *n);
            let (ascent, mid) = app.fonts.digit_metrics(em);
            let fg = contrast_color(st.stroke);
            let galley = p.layout_no_wrap(n.to_string(), FontId::new((em * z).max(1.0), text_family(true, false)), fg);
            let pos = c - vec2(galley.size().x / 2.0, (ascent - mid) * z);
            p.galley(pos, galley, fg);
        }
        Kind::Shape(k) => {
            let (outline, extra) = shape_outline(*k, a.rect());
            let pts: Vec<Pos2> = outline.iter().map(|q| v.to_screen(*q)).collect();
            if st.fill.a() > 0 && k.convex() {
                p.add(Shape::convex_polygon(pts.clone(), st.fill, Stroke::NONE));
            }
            if st.width > 0.0 || st.fill.a() > 0 {
                let line = if st.width > 0.0 { stroke } else { Stroke::new(1.0_f32, st.fill) };
                p.add(Shape::closed_line(pts, line));
                for e in extra {
                    p.add(Shape::line(e.iter().map(|q| v.to_screen(*q)).collect(), line));
                }
            }
        }
        Kind::Callout { text, tail } => {
            let r = callout_box(&app.fonts, a, app.renderer.width() as f32);
            paint_callout_box(p, a, r, *tail, v);
            let pad = callout_padding(st.font_size);
            let size = app.fonts.egui_size(st.font_size) * z;
            let wrap = text_wrap_width(a, app.renderer.width() as f32) * z;
            let color = callout_text_color(st);
            let galley = p.layout_job(text_job(text, st, size, color, wrap));
            p.galley(v.to_screen(r.min + Vec2::splat(pad)), galley, color);
        }
        Kind::Emoji(id) => {
            if let Some(tex) = app.emoji_tex_id(id) {
                let r = a.rect();
                let side = r.width().min(r.height());
                let sq = Rect::from_center_size(r.center(), Vec2::splat(side));
                p.image(tex, v.rect_to_screen(sq), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
            }
        }
        Kind::Text(t) => {
            let pad = text_padding(st.font_size);
            if st.fill.a() > 0 {
                let r = Rect::from_min_size(a.a, text_box_size(&app.fonts, a, app.renderer.width() as f32));
                p.rect_filled(v.rect_to_screen(r), pad * 0.8 * z, st.fill);
            }
            let size = app.fonts.egui_size(st.font_size) * z;
            let wrap = text_wrap_width(a, app.renderer.width() as f32) * z;
            let galley = p.layout_job(text_job(t, st, size, st.stroke, wrap));
            p.galley(v.to_screen(a.a + Vec2::splat(pad)), galley, st.stroke);
        }
    }
}

fn paint_selection(p: &Painter, app: &EditorApp, a: &Annotation, v: &View) {
    let accent = ui::ACCENT;
    if !a.is_segment() {
        let r = v.rect_to_screen(a.bounds(app.text_size())).expand(2.0);
        p.add(Shape::dashed_line(
            &[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()],
            Stroke::new(1.0_f32, accent),
            4.0,
            3.0,
        ));
    }
    for (h, pos) in handles(app, a) {
        let c = v.to_screen(pos);
        match h {
            Handle::Rect(..) => {
                let r = Rect::from_center_size(c, Vec2::splat(9.0));
                p.rect_filled(r, 2.0, Color32::WHITE);
                p.rect_stroke(r, 2.0, Stroke::new(1.5_f32, accent), StrokeKind::Inside);
            }
            Handle::Tail => {
                p.circle_filled(c, 6.0, accent);
                p.circle_stroke(c, 6.0, Stroke::new(1.5_f32, theme::INK));
            }
            _ => {
                p.circle_filled(c, 5.5, Color32::WHITE);
                p.circle_stroke(c, 5.5, Stroke::new(1.5_f32, accent));
            }
        }
    }
}

fn paint_crop(p: &Painter, img: Rect, sel: Rect, sel_img: Rect) {
    let shade = Color32::from_black_alpha(150);
    let sel = sel.intersect(img);
    p.rect_filled(Rect::from_min_max(img.min, pos2(img.max.x, sel.min.y)), 0.0, shade);
    p.rect_filled(Rect::from_min_max(pos2(img.min.x, sel.max.y), img.max), 0.0, shade);
    p.rect_filled(Rect::from_min_max(pos2(img.min.x, sel.min.y), pos2(sel.min.x, sel.max.y)), 0.0, shade);
    p.rect_filled(Rect::from_min_max(pos2(sel.max.x, sel.min.y), pos2(img.max.x, sel.max.y)), 0.0, shade);
    // A fine outline, and four square corner marks in the accent color.
    p.rect_stroke(sel, 0.0, Stroke::new(1.0_f32, Color32::from_white_alpha(150)), StrokeKind::Outside);
    corner_marks(p, sel, 3.0, theme::ACCENT);
    // Size label as a pill in the accent color, above the selection (inside when there is no room).
    let label = format!("{} x {}", sel_img.width().round(), sel_img.height().round());
    let galley = p.layout_no_wrap(label, FontId::proportional(12.0), theme::INK);
    let size = galley.size() + vec2(16.0, 7.0);
    let mut min = sel.left_top() - vec2(0.0, size.y + 8.0);
    if min.y < img.min.y - size.y {
        min.y = sel.min.y + 8.0;
    }
    let pill = Rect::from_min_size(min, size);
    p.rect_filled(pill, size.y / 2.0, theme::ACCENT);
    p.galley(pill.min + vec2(8.0, 3.5), galley, theme::INK);
}

/// The four corner marks around `r`: L-shapes with square corners (like the crop itself),
/// drawn just outside the rectangle so they never cover what is selected.
fn corner_marks(p: &Painter, r: Rect, width: f32, color: Color32) {
    let r = r.expand(width + 1.0);
    let arm = (r.width().min(r.height()) / 3.0).clamp(10.0, 26.0);
    for (corner, dx, dy) in [
        (r.left_top(), 1.0, 1.0),
        (r.right_top(), -1.0, 1.0),
        (r.right_bottom(), -1.0, -1.0),
        (r.left_bottom(), 1.0, -1.0),
    ] {
        for (w, h) in [(arm, width), (width, arm)] {
            let far = corner + vec2(dx * w, dy * h);
            p.rect_filled(Rect::from_two_pos(corner, far), 0.0, color);
        }
    }
}

/// In-place text editing with an egui TextEdit using the same font and scale as the renderer.
fn text_editor(app: &mut EditorApp, ctx: &egui::Context, v: &View) {
    let Some(te) = &app.text_edit else { return };
    let id = te.id;
    let focus_set = te.focus_set;
    let Some(i) = app.find(id) else {
        app.text_edit = None;
        return;
    };
    let a = app.doc.anns[i].clone();
    let st = a.style;
    if let Kind::Callout { .. } = a.kind {
        callout_editor(app, ctx, v, i, &a, focus_set);
        return;
    }
    let pad = text_padding(st.font_size);
    let size = app.fonts.egui_size(st.font_size) * v.zoom;
    let bound_w = app.renderer.width() as f32;
    let box_size = text_box_size(&app.fonts, &a, bound_w) * v.zoom;
    let wrap = text_wrap_width(&a, bound_w) * v.zoom;
    if st.fill.a() > 0 {
        let r = Rect::from_min_size(v.to_screen(a.a), box_size);
        ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("text_bg"))).rect_filled(
            r,
            pad * 0.8 * v.zoom,
            st.fill,
        );
    }
    let pos = v.to_screen(a.a + Vec2::splat(pad));
    // A box sized by hand is as wide as that; otherwise just wide enough for the text.
    let width = if text_fixed_width(&a).is_some() { wrap } else { (box_size.x.max(size) + size).min(wrap + size) };
    let mut changed = false;
    let mut lost = false;
    egui::Area::new(egui::Id::new(("text_edit", id))).fixed_pos(pos).order(egui::Order::Foreground).show(ctx, |ui| {
        if let Kind::Text(s) = &mut app.doc.anns[i].kind {
            let mut layouter = |ui: &egui::Ui, text: &str, _wrap: f32| {
                let job = text_job(text, &st, size, st.stroke, wrap);
                ui.fonts(|f| f.layout_job(job))
            };
            ui.visuals_mut().text_cursor.stroke =
                Stroke::new(2.0_f32, contrast_color(st.fill.gamma_multiply(1.0)).gamma_multiply(0.9));
            if st.fill.a() == 0 {
                ui.visuals_mut().text_cursor.stroke.color = st.stroke;
            }
            let out = egui::TextEdit::multiline(s)
                .frame(false)
                .margin(egui::Margin::ZERO)
                .desired_width(width)
                .desired_rows(1)
                .layouter(&mut layouter)
                .show(ui);
            if !focus_set {
                out.response.request_focus();
            }
            changed = out.response.changed();
            lost = out.response.lost_focus();
        }
    });
    if let Some(te) = &mut app.text_edit {
        te.focus_set = true;
    }
    if changed {
        app.unsaved = true;
    }
    if lost && focus_set {
        app.finish_text_edit();
    }
}

/// A callout's box and pointer (no text), for the live preview and while typing.
fn paint_callout_box(p: &Painter, a: &Annotation, r: Rect, tail: Pos2, v: &View) {
    let st = &a.style;
    let z = v.zoom;
    let outline: Vec<Pos2> = callout_outline(r, tail, a.callout_radius()).iter().map(|q| v.to_screen(*q)).collect();
    if st.fill.a() > 0 {
        // The outline is not convex (pointer): fill the box and the pointer separately.
        p.rect_filled(v.rect_to_screen(r), a.callout_radius() * z, st.fill);
        if !r.contains(tail) {
            let tip = v.to_screen(tail);
            let c = v.to_screen(r.center());
            let n = (tip - c).normalized().rot90() * (r.width().min(r.height()) * 0.15 * z);
            p.add(Shape::convex_polygon(vec![c + n, tip, c - n], st.fill, Stroke::NONE));
        }
    }
    if st.width > 0.0 {
        p.add(Shape::closed_line(outline, Stroke::new((st.width * z).max(1.0), st.stroke)));
    }
}

/// In-place editing of a callout's text: the box and pointer are drawn live, the text is typed
/// into it with the same font and wrapping as the renderer.
fn callout_editor(app: &mut EditorApp, ctx: &egui::Context, v: &View, i: usize, a: &Annotation, focus_set: bool) {
    let st = a.style;
    let id = a.id;
    let pad = callout_padding(st.font_size);
    let size = app.fonts.egui_size(st.font_size) * v.zoom;
    let wrap = text_wrap_width(a, app.renderer.width() as f32) * v.zoom;
    let color = callout_text_color(&st);
    let r = callout_box(&app.fonts, a, app.renderer.width() as f32);
    let Kind::Callout { tail, .. } = a.kind else { return };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("callout_bg")));
    paint_callout_box(&painter, a, r, tail, v);
    let pos = v.to_screen(r.min + Vec2::splat(pad));
    let mut changed = false;
    let mut lost = false;
    egui::Area::new(egui::Id::new(("text_edit", id))).fixed_pos(pos).order(egui::Order::Foreground).show(ctx, |ui| {
        if let Some(s) = app.doc.anns[i].kind.text_mut() {
            let mut layouter = |ui: &egui::Ui, text: &str, _wrap: f32| {
                let job = text_job(text, &st, size, color, wrap);
                ui.fonts(|f| f.layout_job(job))
            };
            ui.visuals_mut().text_cursor.stroke = Stroke::new(2.0_f32, color);
            let out = egui::TextEdit::multiline(s)
                .frame(false)
                .margin(egui::Margin::ZERO)
                .desired_width(wrap)
                .desired_rows(1)
                .layouter(&mut layouter)
                .show(ui);
            if !focus_set {
                out.response.request_focus();
            }
            changed = out.response.changed();
            lost = out.response.lost_focus();
        }
    });
    if let Some(te) = &mut app.text_edit {
        te.focus_set = true;
    }
    if changed {
        app.unsaved = true;
    }
    if lost && focus_set {
        app.finish_text_edit();
    }
}

/// Eyedropper: the cursor shows a loupe with the color under it; a click takes that color.
fn eyedropper(app: &mut EditorApp, ctx: &egui::Context, resp: &egui::Response, painter: &Painter, v: &View) {
    ctx.set_cursor_icon(CursorIcon::Crosshair);
    let Some(screen) = resp.hover_pos() else { return };
    let p = v.to_img(screen);
    let color = app.color_at(p);
    if let Some(c) = color {
        let center = screen + vec2(26.0, -26.0);
        painter.circle_filled(center, 17.0, theme::INK);
        painter.circle_filled(center, 14.0, c);
        painter.circle_stroke(center, 14.0, Stroke::new(2.0_f32, Color32::WHITE));
        let [r, g, b, _] = c.to_array();
        let label = format!("#{r:02X}{g:02X}{b:02X}");
        let pos = center + vec2(22.0, 0.0);
        let galley = painter.layout_no_wrap(label, FontId::proportional(12.0), Color32::WHITE);
        let bg = Rect::from_min_size(pos - vec2(4.0, galley.size().y / 2.0 + 2.0), galley.size() + vec2(8.0, 4.0));
        painter.rect_filled(bg, 4.0, theme::INK);
        painter.galley(pos - vec2(0.0, galley.size().y / 2.0), galley, Color32::WHITE);
    }
    if resp.clicked_by(PointerButton::Primary) && color.is_some() {
        app.pick_color(p);
    } else if resp.clicked_by(PointerButton::Secondary) {
        app.cancel_eyedropper();
    }
}
