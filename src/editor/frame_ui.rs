//! Side panel with the background and device mockup options.

use super::{ui, EditorApp};
use crate::frame::{self, Align, Aspect, Background, Finish, Fit, Group, Orientation, DEVICES, PRESETS};
use crate::i18n::{t, tf};
use crate::theme::pal;
use eframe::egui::{
    self, vec2, Color32, CornerRadius, FontFamily, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
};

pub fn panel(app: &mut EditorApp, ctx: &egui::Context) {
    let before = app.frame.clone();
    if app.panel_collapsed {
        // Folded away: a slim strip to bring it back.
        egui::SidePanel::right("frame_folded")
            .resizable(false)
            .exact_width(34.0)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(pal().panel).inner_margin(Margin::symmetric(4, 10)))
            .show(ctx, |ui| {
                if ui
                    .add(egui::Button::new("\u{2039}").min_size(vec2(26.0, 26.0)))
                    .on_hover_text(t("Show the background / mockup panel (Ctrl+B)"))
                    .clicked()
                {
                    app.panel_collapsed = false;
                }
            });
        return;
    }
    egui::SidePanel::right("frame")
        .resizable(false)
        .exact_width(316.0)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(pal().panel).inner_margin(Margin::symmetric(8, 8)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new("\u{203A}").min_size(vec2(26.0, 22.0)))
                        .on_hover_text(t("Hide this panel (Ctrl+B)"))
                        .clicked()
                    {
                        app.panel_collapsed = true;
                    }
                });
            });
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().slider_width = 150.0;
                card(ui, |ui| background_section(app, ctx, ui));
                ui.add_space(8.0);
                card(ui, |ui| mockup_section(app, ui));
                ui.add_space(10.0);
                let fixed = app.frame.aspect.fixed_size().filter(|_| app.frame.background_on);
                if let Some(l) = app.frame_layout() {
                    let (w, h) = fixed.unwrap_or((l.size.x as u32, l.size.y as u32));
                    ui.label(RichText::new(tf("Output: {0} x {1} px", &[&w, &h])).color(pal().muted).small());
                }
                ui.label(
                    RichText::new(t("Copy and Save include the frame. Options are remembered for next time."))
                        .color(pal().muted)
                        .small(),
                );
            });
        });
    if app.frame != before {
        if app.frame.active() != before.active()
            || app.frame.mockup_on != before.mockup_on
            || app.frame.background_on != before.background_on
        {
            app.zoom = None;
        }
        app.invalidate_frame();
    }
}

/// A rounded card around one section of the panel. The radius follows the logo tile's ratio.
fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .fill(pal().panel_raised)
        .stroke(Stroke::new(1.0_f32, pal().border))
        .corner_radius(CornerRadius::same(crate::theme::radius(72.0)))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn header(ui: &mut Ui, title: &str, on: &mut bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).family(FontFamily::Name("ui_semibold".into())).size(15.0).color(pal().text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            toggle_switch(ui, on);
        });
    });
    ui.add_space(4.0);
}

fn toggle_switch(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(34.0, 18.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let bg = pal().control_strong.lerp_to_gamma(ui::ACCENT, t);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, *on, ""));
    ui.painter().rect_filled(rect, 9.0, bg);
    let x = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, t);
    // The knob turns to ink on the pink of an active switch.
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 6.5, Color32::WHITE.lerp_to_gamma(ui::ON_ACCENT, t));
    resp
}

fn row_label(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).color(pal().muted).small());
}

/// Previews of every background with this screenshot on it, so you see the result, not a
/// generic swatch.
fn ensure_thumbs(app: &mut EditorApp, ctx: &egui::Context) {
    if !app.frame_thumbs.is_empty() {
        return;
    }
    let shot = app.shot_thumb().cloned();
    let mut list: Vec<Background> = PRESETS.iter().map(|p| Background::Preset(p.id.into())).collect();
    list.push(Background::Transparent);
    for bg in list {
        if let Some(mut pm) = frame::thumbnail(&bg, 96, 60) {
            if let Some(shot) = &shot {
                // Fit the screenshot in the middle, with room around it like the padding.
                let k = (70.0 / shot.width() as f32).min(42.0 / shot.height() as f32);
                let (w, h) = (shot.width() as f32 * k, shot.height() as f32 * k);
                let (x, y) = ((96.0 - w) / 2.0, (60.0 - h) / 2.0);
                let paint =
                    tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
                if let Some(r) = tiny_skia::Rect::from_xywh(x + 1.0, y + 2.0, w, h) {
                    // A soft shadow under it.
                    let mut sp = tiny_skia::Paint::default();
                    sp.set_color_rgba8(0, 0, 0, 60);
                    pm.fill_rect(r, &sp, tiny_skia::Transform::identity(), None);
                }
                pm.draw_pixmap(0, 0, shot.as_ref(), &paint, tiny_skia::Transform::from_row(k, 0.0, 0.0, k, x, y), None);
            }
            let img = egui::ColorImage::from_rgba_premultiplied([96, 60], pm.data());
            let tex = ctx.load_texture(format!("bg_{bg:?}"), img, egui::TextureOptions::LINEAR);
            app.frame_thumbs.push((bg, tex));
        }
    }
}

fn swatch_frame(ui: &mut Ui, selected: bool, tip: &str, paint: impl FnOnce(&egui::Painter, Rect)) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(48.0, 32.0), Sense::click());
    let inner = rect.shrink(2.0);
    paint(ui.painter(), inner);
    let stroke = if selected {
        Stroke::new(2.0_f32, Color32::WHITE)
    } else if resp.hovered() {
        Stroke::new(1.0_f32, pal().muted)
    } else {
        Stroke::new(1.0_f32, pal().border)
    };
    ui.painter().rect_stroke(rect.shrink(1.0), crate::theme::radius(32.0) as f32, stroke, StrokeKind::Middle);
    resp.on_hover_text(tip).clicked()
}

fn background_section(app: &mut EditorApp, ctx: &egui::Context, ui: &mut Ui) {
    header(ui, t("Background"), &mut app.frame.background_on);
    if !app.frame.background_on {
        ui.label(
            RichText::new(t("Puts the screenshot on a background with rounded corners.")).color(pal().muted).small(),
        );
        return;
    }
    ensure_thumbs(app, ctx);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for (bg, tex) in &app.frame_thumbs {
            let name = match bg {
                Background::Preset(id) => frame::preset(id).map_or("", |p| t(p.name)),
                _ => t("Transparent (PNG)"),
            };
            let selected = &app.frame.background == bg;
            let id = tex.id();
            if swatch_frame(ui, selected, name, |p, r| {
                p.add(
                    egui::epaint::RectShape::filled(r, CornerRadius::same(6), Color32::WHITE)
                        .with_texture(id, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))),
                );
            }) {
                app.frame.background = bg.clone();
            }
        }
        // Custom two-color gradient.
        let (a, b) = match &app.frame.background {
            Background::Custom { a, b, .. } => (*a, *b),
            _ => ([99, 102, 241], [236, 72, 153]),
        };
        let selected = matches!(app.frame.background, Background::Custom { .. });
        if swatch_frame(ui, selected, t("Custom gradient"), |p, r| {
            let ca = Color32::from_rgb(a[0], a[1], a[2]);
            let cb = Color32::from_rgb(b[0], b[1], b[2]);
            let mut m = egui::Mesh::default();
            let v = |pos, color| egui::epaint::Vertex { pos, uv: egui::epaint::WHITE_UV, color };
            m.vertices.extend([
                v(r.left_top(), ca),
                v(r.right_top(), ca),
                v(r.right_bottom(), cb),
                v(r.left_bottom(), cb),
            ]);
            m.indices.extend([0, 1, 2, 0, 2, 3]);
            p.add(m);
            p.text(r.center(), egui::Align2::CENTER_CENTER, "+", egui::FontId::proportional(16.0), Color32::WHITE);
        }) && !selected
        {
            app.frame.background = Background::Custom { a, b, angle: 135.0 };
        }
    });
    if let Background::Custom { a, b, angle } = &mut app.frame.background {
        ui.horizontal(|ui| {
            ui.label(RichText::new(t("Colors")).color(pal().muted).small());
            egui::color_picker::color_edit_button_srgb(ui, a);
            egui::color_picker::color_edit_button_srgb(ui, b);
            ui.add(egui::DragValue::new(angle).range(0.0..=360.0).speed(1.0).suffix("°"));
        });
    }

    let f = &mut app.frame;
    row_label(ui, t("Padding (how much background shows)"));
    let mut pct = f.padding * 100.0;
    if ui.add(egui::Slider::new(&mut pct, 0.0..=50.0).step_by(1.0).suffix(" %")).changed() {
        f.padding = pct / 100.0;
    }
    if !f.mockup_on {
        row_label(ui, t("Corner radius"));
        ui.add(egui::Slider::new(&mut f.radius, 0.0..=80.0).step_by(1.0).suffix(" px"));
    }
    shadow_slider(ui, f);
    ui.add_space(4.0);
    ui.checkbox(&mut f.grain, t("Film grain"));
    row_label(ui, t("Canvas shape"));
    ui.horizontal_wrapped(|ui| {
        for a in Aspect::ALL {
            ui.selectable_value(&mut f.aspect, a, t(a.name()));
        }
    });
    row_label(ui, t("Exact size for social media and slides"));
    let label = |a: Aspect| {
        let (w, h) = a.fixed_size().unwrap_or_default();
        format!("{}  ·  {w} x {h}", t(a.name()))
    };
    let current =
        if f.aspect.fixed_size().is_some() { label(f.aspect) } else { t("None (use the shape above)").to_string() };
    egui::ComboBox::from_id_salt("size_preset").width(260.0).selected_text(current).show_ui(ui, |ui| {
        let mut none = f.aspect.fixed_size().is_none();
        if ui.selectable_label(none, t("None (use the shape above)")).clicked() && !none {
            f.aspect = Aspect::Auto;
            none = true;
        }
        let _ = none;
        for a in Aspect::SIZES {
            ui.selectable_value(&mut f.aspect, a, label(a));
        }
    });
}

fn shadow_slider(ui: &mut Ui, f: &mut frame::FrameSettings) {
    row_label(ui, t("Shadow"));
    let mut pct = f.shadow * 100.0;
    if ui.add(egui::Slider::new(&mut pct, 0.0..=100.0).step_by(5.0).suffix(" %")).changed() {
        f.shadow = pct / 100.0;
    }
}

fn mockup_section(app: &mut EditorApp, ui: &mut Ui) {
    header(ui, t("Device mockup"), &mut app.frame.mockup_on);
    if !app.frame.mockup_on {
        ui.label(
            RichText::new(t("Shows the screenshot on a phone, tablet, laptop, monitor or in a browser window."))
                .color(pal().muted)
                .small(),
        );
        return;
    }
    let v = app.view_rect();
    let (iw, ih) = (v.width(), v.height());
    let (auto, _) = frame::auto_device(iw, ih);
    let f = &mut app.frame;
    let (current, rotated) = frame::resolve_device(f, iw, ih);

    row_label(ui, t("Device"));
    let selected_text = match &f.device {
        None => tf("Auto: {0}", &[&auto.display_name()]),
        Some(_) => current.display_name().to_string(),
    };
    egui::ComboBox::from_id_salt("device").width(260.0).height(420.0).selected_text(selected_text).show_ui(ui, |ui| {
        ui.selectable_value(&mut f.device, None, tf("Auto (best match: {0})", &[&auto.display_name()]));
        let mut group: Option<Group> = None;
        for d in DEVICES {
            if group != Some(d.group) {
                group = Some(d.group);
                ui.add_space(4.0);
                ui.label(RichText::new(t(d.group.name())).color(pal().muted).small());
            }
            ui.selectable_value(&mut f.device, Some(d.id.to_string()), d.label());
        }
    });

    if current.rotatable {
        row_label(ui, t("Orientation"));
        ui.horizontal(|ui| {
            for (o, name) in [
                (Orientation::Auto, t("Auto")),
                (Orientation::Portrait, t("Portrait")),
                (Orientation::Landscape, t("Landscape")),
            ] {
                ui.selectable_value(&mut f.orientation, o, name);
            }
        });
    }

    if !current.flexible() {
        row_label(ui, t("Screenshot in the screen"));
        ui.horizontal(|ui| {
            ui.selectable_value(&mut f.fit, Fit::Fill, t("Fill"))
                .on_hover_text(t("Fill the screen edge to edge, cropping a little if the shape differs"));
            ui.selectable_value(&mut f.fit, Fit::Fit, t("Fit"))
                .on_hover_text(t("Show the whole screenshot, black bars where the shape differs"));
        });
        if let Some(l) = frame::layout(f, iw, ih) {
            let vertical = (l.shot.height() - l.clip.height()).abs() > (l.shot.width() - l.clip.width()).abs();
            let differs =
                (l.shot.height() - l.clip.height()).abs() > 0.5 || (l.shot.width() - l.clip.width()).abs() > 0.5;
            if differs {
                let names =
                    if vertical { [t("Top"), t("Center"), t("Bottom")] } else { [t("Left"), t("Center"), t("Right")] };
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(if f.fit == Fit::Fill { t("Keep") } else { t("Place") })
                            .color(pal().muted)
                            .small(),
                    );
                    for (a, n) in [Align::Start, Align::Center, Align::End].into_iter().zip(names) {
                        ui.selectable_value(&mut f.align, a, n);
                    }
                });
                let shot = l.shot.width() * l.shot.height();
                let vis = l.shot.intersect(l.clip);
                let lost = 1.0 - vis.width() * vis.height() / shot.max(1.0);
                let note = if f.fit == Fit::Fill {
                    tf("{0}% of the screenshot is cropped to fill the screen.", &[&format!("{:.0}", lost * 100.0)])
                } else {
                    t("Black bars fill the rest of the screen.").to_string()
                };
                ui.label(RichText::new(note).color(pal().muted).small());
            }
            let (rw, rh) = if rotated { (current.res.1, current.res.0) } else { current.res };
            ui.label(
                RichText::new(tf(
                    "Screen {0} x {1}  ·  screenshot {2} x {3}",
                    &[&rw, &rh, &format!("{iw:.0}"), &format!("{ih:.0}")],
                ))
                .color(pal().muted)
                .small(),
            );
        }
    }

    row_label(ui, if current.group == Group::Window { t("Theme") } else { t("Finish") });
    ui.horizontal(|ui| {
        let (dark, light) =
            if current.group == Group::Window { (t("Dark"), t("Light")) } else { (t("Dark"), t("Silver")) };
        ui.selectable_value(&mut f.finish, Finish::Dark, dark);
        ui.selectable_value(&mut f.finish, Finish::Light, light);
    });
    if current.group == Group::Window {
        row_label(ui, if current.id == "browser" { t("Address bar text") } else { t("Window title") });
        ui.add(egui::TextEdit::singleline(&mut f.url).hint_text(t("optional")).desired_width(260.0));
    }
    if !f.background_on {
        shadow_slider(ui, f);
        // Most people want a background behind a device: offer it right here.
        ui.add_space(6.0);
        egui::Frame::NONE.fill(pal().group).corner_radius(8).inner_margin(Margin::same(8)).show(ui, |ui| {
            ui.label(
                RichText::new(t("Without a background the area around the device is transparent."))
                    .color(pal().muted)
                    .small(),
            );
            if ui
                .add(
                    egui::Button::new(RichText::new(t("Add a background")).strong().color(ui::ON_ACCENT))
                        .fill(ui::ACCENT),
                )
                .clicked()
            {
                f.background_on = true;
            }
        });
    }
}
