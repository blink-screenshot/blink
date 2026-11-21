//! The small bar that floats next to the area while it is being recorded: a red dot and the
//! time, pause / resume, stop and discard, and a grip to move it. While the recording is being
//! saved it shows how far that is. Also the 3-2-1 countdown before recording starts.
//!
//! Drawn here with tiny-skia like the capture overlay's mode bar (`capture_bar`), so the
//! Windows side only blends a picture, and it can be tested and previewed anywhere.

use crate::capture_bar::{draw_text, fill, rrect, shadow, stroke, text_width, Area, Fonts};
use crate::i18n::{t, tf};
use crate::theme::{self, DARK};
use eframe::egui::Color32;
use tiny_skia::{PathBuilder, Pixmap};

/// What the bar shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct State {
    /// Length of the recording so far.
    pub ms: u32,
    pub paused: bool,
    /// The bin was pressed once: a second press throws the recording away.
    pub confirm_discard: bool,
    /// Saving: how far (0..1).
    pub saving: Option<f32>,
}

/// What is under the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Grip,
    Pause,
    Stop,
    Discard,
}

// Sizes in points.
const H: f32 = 40.0;
const PAD: f32 = 4.0;
const BTN: f32 = 32.0;
const GRIP_W: f32 = 18.0;
const TIME_EM: f32 = 14.0;
const TEXT_EM: f32 = 12.5;

pub struct Layout {
    pub scale: f32,
    /// Size of the panel.
    pub w: i32,
    pub h: i32,
    /// Room for the shadow around the panel.
    pub margin: i32,
    grip: Area,
    dot: (f32, f32),
    time_x: f32,
    pause: Area,
    stop: Area,
    discard: Area,
    saving: bool,
}

fn time_text(ms: u32) -> String {
    super::export::human_duration(ms.saturating_sub(ms % 1000))
}

impl Layout {
    pub fn new(state: &State, scale: f32, fonts: &Fonts) -> Self {
        let sc = scale.max(0.5);
        let semi = &fonts.semibold;
        let grip = Area::new(0.0, 0.0, GRIP_W * sc, H * sc);
        let cy = H * sc / 2.0;
        let dot = (grip.w + 14.0 * sc, cy);
        let time_x = dot.0 + 12.0 * sc;
        // Room for the longest time of this length, so the buttons do not shift each second.
        let widest = if state.ms >= 3_600_000 { "0:00:00" } else { "00:00" };
        let tw = text_width(semi, TIME_EM * sc, &widest.replace('0', "8"));
        let mut x = time_x + tw + 12.0 * sc;
        let btn = |x: f32, w: f32| Area::new(x, PAD * sc, w, BTN * sc);
        if let Some(p) = state.saving {
            let label = tf("Saving... {0}%", &[&((p * 100.0) as u32)]);
            let w = text_width(&fonts.regular, TEXT_EM * sc, &label) + 18.0 * sc;
            let a = btn(x, w);
            x += w + PAD * sc;
            return Layout {
                scale: sc,
                w: x.round() as i32,
                h: (H * sc).round() as i32,
                margin: (14.0 * sc).ceil() as i32,
                grip,
                dot,
                time_x,
                pause: a,
                stop: Area::new(-100.0, 0.0, 0.0, 0.0),
                discard: Area::new(-100.0, 0.0, 0.0, 0.0),
                saving: true,
            };
        }
        let pause = btn(x, BTN * sc);
        x += BTN * sc + 2.0 * sc;
        let stop = btn(x, BTN * sc);
        x += BTN * sc + 2.0 * sc;
        let discard_w =
            if state.confirm_discard { text_width(semi, TEXT_EM * sc, t("Discard?")) + 20.0 * sc } else { BTN * sc };
        let discard = btn(x, discard_w);
        x += discard_w + PAD * sc;
        Layout {
            scale: sc,
            w: x.round() as i32,
            h: (H * sc).round() as i32,
            margin: (14.0 * sc).ceil() as i32,
            grip,
            dot,
            time_x,
            pause,
            stop,
            discard,
            saving: false,
        }
    }

    /// What is at `(x, y)` relative to the panel's top-left.
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if self.grip.contains(x, y) {
            return Some(Hit::Grip);
        }
        if self.saving {
            return None;
        }
        [(self.pause, Hit::Pause), (self.stop, Hit::Stop), (self.discard, Hit::Discard)]
            .into_iter()
            .find(|(a, _)| a.contains(x, y))
            .map(|(_, h)| h)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= 0.0 && y >= 0.0 && x < self.w as f32 && y < self.h as f32
    }

    /// The bar, with the panel's top-left at `(margin, margin)` (premultiplied RGBA).
    pub fn render(&self, state: &State, hover: Option<Hit>, fonts: &Fonts) -> Option<Pixmap> {
        let sc = self.scale;
        let m = self.margin as f32;
        let mut pix = Pixmap::new((self.w + 2 * self.margin) as u32, (self.h + 2 * self.margin) as u32)?;
        let at = |a: Area| Area { x: a.x + m, y: a.y + m, ..a };
        let panel = Area { x: m, y: m, w: self.w as f32, h: self.h as f32 };
        let r = theme::radius(BTN) as f32 * sc + PAD * sc;
        let line = sc.round().max(1.0);
        shadow(&mut pix, panel, r, sc);
        fill(&mut pix, rrect(panel, [r; 4]), DARK.panel, 1.0);
        stroke(
            &mut pix,
            rrect(panel.inset(line / 2.0, line / 2.0, line / 2.0, line / 2.0), [r - line / 2.0; 4]),
            DARK.border,
            line,
        );

        // Grip: two columns of dots, like the mode bar.
        let grip = at(self.grip);
        let dots = if hover == Some(Hit::Grip) { DARK.text } else { DARK.muted };
        for col in [-0.5f32, 0.5] {
            for row in -1..=1 {
                let mut pb = PathBuilder::new();
                pb.push_circle(
                    grip.x + grip.w / 2.0 + 2.0 * sc + col * 6.0 * sc,
                    grip.cy() + row as f32 * 6.0 * sc,
                    1.4 * sc,
                );
                fill(&mut pix, pb.finish(), dots, 1.0);
            }
        }

        // The red dot (gray while paused) and the time.
        let mut pb = PathBuilder::new();
        pb.push_circle(self.dot.0 + m, self.dot.1 + m, 5.0 * sc);
        let dot_c = if state.paused { DARK.muted } else { theme::RECORD };
        fill(&mut pix, pb.finish(), dot_c, 1.0);
        let time_c = if state.paused { DARK.muted } else { DARK.text };
        draw_text(
            &mut pix,
            &fonts.semibold,
            TIME_EM * sc,
            self.time_x + m,
            self.dot.1 + m,
            time_c,
            &time_text(state.ms),
        );

        let seg_r = theme::radius(BTN) as f32 * sc;
        if self.saving {
            let a = at(self.pause);
            let p = state.saving.unwrap_or(0.0).clamp(0.0, 1.0);
            fill(&mut pix, rrect(a, [seg_r; 4]), DARK.control, 1.0);
            fill(
                &mut pix,
                rrect(Area { w: (a.w * p).max(seg_r * 2.0).min(a.w), ..a }, [seg_r; 4]),
                theme::RECORD,
                0.35,
            );
            let label = tf("Saving... {0}%", &[&((p * 100.0) as u32)]);
            draw_text(&mut pix, &fonts.regular, TEXT_EM * sc, a.x + 9.0 * sc, a.cy(), DARK.text, &label);
            return Some(pix);
        }

        for (area, which) in [(self.pause, Hit::Pause), (self.stop, Hit::Stop), (self.discard, Hit::Discard)] {
            let a = at(area);
            let hot = hover == Some(which);
            let confirm = which == Hit::Discard && state.confirm_discard;
            if confirm {
                fill(&mut pix, rrect(a, [seg_r; 4]), DARK.error, if hot { 1.0 } else { 0.85 });
            } else if hot {
                fill(&mut pix, rrect(a, [seg_r; 4]), DARK.control_hover, 1.0);
            }
            let c = if hot { DARK.text } else { Color32::from_rgb(0xD0, 0xD3, 0xDA) };
            let (cx, cy) = (a.x + a.w / 2.0, a.cy());
            let u = sc;
            match which {
                Hit::Pause if state.paused => {
                    // Resume: a triangle.
                    let mut pb = PathBuilder::new();
                    pb.move_to(cx - 4.0 * u, cy - 6.0 * u);
                    pb.line_to(cx + 6.0 * u, cy);
                    pb.line_to(cx - 4.0 * u, cy + 6.0 * u);
                    pb.close();
                    fill(&mut pix, pb.finish(), c, 1.0);
                }
                Hit::Pause => {
                    for dx in [-3.5f32, 3.5] {
                        let bar = Area { x: cx + dx * u - 1.75 * u, y: cy - 6.0 * u, w: 3.5 * u, h: 12.0 * u };
                        fill(&mut pix, rrect(bar, [1.0 * u; 4]), c, 1.0);
                    }
                }
                Hit::Stop => {
                    let sq = Area { x: cx - 6.0 * u, y: cy - 6.0 * u, w: 12.0 * u, h: 12.0 * u };
                    fill(&mut pix, rrect(sq, [2.5 * u; 4]), if hot { theme::RECORD } else { c }, 1.0);
                }
                Hit::Discard if confirm => {
                    let tw = text_width(&fonts.semibold, TEXT_EM * sc, t("Discard?"));
                    draw_text(&mut pix, &fonts.semibold, TEXT_EM * sc, cx - tw / 2.0, cy, theme::INK, t("Discard?"));
                }
                Hit::Discard => {
                    // A bin: lid, handle and can.
                    let w = 1.6 * u;
                    let mut pb = PathBuilder::new();
                    pb.move_to(cx - 6.5 * u, cy - 4.5 * u);
                    pb.line_to(cx + 6.5 * u, cy - 4.5 * u);
                    pb.move_to(cx - 2.5 * u, cy - 4.5 * u);
                    pb.line_to(cx - 2.5 * u, cy - 6.5 * u);
                    pb.line_to(cx + 2.5 * u, cy - 6.5 * u);
                    pb.line_to(cx + 2.5 * u, cy - 4.5 * u);
                    pb.move_to(cx - 4.8 * u, cy - 4.5 * u);
                    pb.line_to(cx - 4.0 * u, cy + 6.5 * u);
                    pb.line_to(cx + 4.0 * u, cy + 6.5 * u);
                    pb.line_to(cx + 4.8 * u, cy - 4.5 * u);
                    stroke(&mut pix, pb.finish(), c, w);
                }
                Hit::Grip => {}
            }
        }
        Some(pix)
    }
}

/// The countdown: a big number in a dark circle with a red ring, and "Esc: cancel" below it.
/// Premultiplied RGBA, exactly as large as needed.
pub fn countdown(n: u32, scale: f32, fonts: &Fonts) -> Option<Pixmap> {
    let sc = scale.max(0.5);
    let d = (120.0 * sc).round();
    let hint = t("Esc to cancel");
    let hint_em = 13.0 * sc;
    let hint_w = text_width(&fonts.regular, hint_em, hint) + 20.0 * sc;
    let w = d.max(hint_w).ceil();
    let h = (d + 40.0 * sc).ceil();
    let mut pix = Pixmap::new(w as u32, h as u32)?;
    let cx = w / 2.0;
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, d / 2.0, d / 2.0 - 2.0 * sc);
    let circle = pb.finish();
    fill(&mut pix, circle.clone(), theme::INK, 0.85);
    stroke(&mut pix, circle, theme::RECORD, 4.0 * sc);
    let text = n.to_string();
    let em = 64.0 * sc;
    let tw = text_width(&fonts.semibold, em, &text);
    draw_text(&mut pix, &fonts.semibold, em, cx - tw / 2.0, d / 2.0, Color32::WHITE, &text);
    let pill = Area::new((w - hint_w) / 2.0, d + 10.0 * sc, hint_w, 26.0 * sc);
    fill(&mut pix, rrect(pill, [pill.h / 2.0; 4]), theme::INK, 0.85);
    draw_text(&mut pix, &fonts.regular, hint_em, pill.x + 10.0 * sc, pill.cy(), DARK.text, hint);
    Some(pix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State { ms: 12_400, paused: false, confirm_discard: false, saving: None }
    }

    #[test]
    fn buttons_are_where_they_are_hit() {
        let fonts = crate::capture_bar::fonts();
        for scale in [1.0, 1.5, 2.0] {
            let l = Layout::new(&state(), scale, fonts);
            for (a, h) in [(l.pause, Hit::Pause), (l.stop, Hit::Stop), (l.discard, Hit::Discard)] {
                assert_eq!(l.hit(a.x + a.w / 2.0, a.cy()), Some(h));
                assert!(a.x + a.w <= l.w as f32);
            }
            assert_eq!(l.hit(2.0, 2.0), Some(Hit::Grip));
            assert_eq!(l.hit(l.time_x + 2.0, l.dot.1), None, "the time is not a button");
            assert!(l.pause.x > l.time_x);
            assert!(l.render(&state(), Some(Hit::Stop), fonts).is_some());
        }
    }

    #[test]
    fn the_layout_does_not_jump_every_second() {
        let fonts = crate::capture_bar::fonts();
        let a = Layout::new(&State { ms: 1_000, ..state() }, 1.0, fonts);
        let b = Layout::new(&State { ms: 51_000, ..state() }, 1.0, fonts);
        assert_eq!(a.w, b.w);
        assert_eq!(a.stop, b.stop);
    }

    #[test]
    fn discard_asks_once_more_and_saving_has_no_buttons() {
        let fonts = crate::capture_bar::fonts();
        let plain = Layout::new(&state(), 1.0, fonts);
        let asking = Layout::new(&State { confirm_discard: true, ..state() }, 1.0, fonts);
        assert!(asking.discard.w > plain.discard.w);
        assert!(asking.render(&State { confirm_discard: true, ..state() }, None, fonts).is_some());
        let saving = State { saving: Some(0.4), ..state() };
        let l = Layout::new(&saving, 1.0, fonts);
        assert_eq!(l.hit(l.pause.x + 4.0, l.pause.cy()), None);
        assert_eq!(l.hit(2.0, 2.0), Some(Hit::Grip));
        assert!(l.render(&saving, None, fonts).is_some());
    }

    #[test]
    fn the_dot_is_red_while_recording() {
        let fonts = crate::capture_bar::fonts();
        let l = Layout::new(&state(), 1.0, fonts);
        let pix = l.render(&state(), None, fonts).unwrap();
        let p = pix.pixel((l.dot.0 + l.margin as f32) as u32, (l.dot.1 + l.margin as f32) as u32).unwrap();
        assert_eq!((p.red(), p.green(), p.blue()), (theme::RECORD.r(), theme::RECORD.g(), theme::RECORD.b()));
        let paused = State { paused: true, ..state() };
        let pix = l.render(&paused, None, fonts).unwrap();
        let p = pix.pixel((l.dot.0 + l.margin as f32) as u32, (l.dot.1 + l.margin as f32) as u32).unwrap();
        assert_ne!(p.red(), theme::RECORD.r());
    }

    #[test]
    fn countdown_draws() {
        let fonts = crate::capture_bar::fonts();
        let p = countdown(3, 1.5, fonts).unwrap();
        assert!(p.width() >= 180 && p.height() > p.width() / 2);
    }

    /// `BLINK_BAR_PREVIEW=<dir> cargo test rec::bar -- --nocapture` writes PNGs of the bar and
    /// the countdown.
    #[test]
    fn preview() {
        let Some(dir) = std::env::var_os("BLINK_BAR_PREVIEW") else { return };
        let fonts = crate::capture_bar::fonts();
        let states = [
            ("recording", state(), Some(Hit::Stop)),
            ("paused", State { paused: true, ..state() }, Some(Hit::Pause)),
            ("discard", State { confirm_discard: true, ..state() }, Some(Hit::Discard)),
            ("saving", State { saving: Some(0.45), ..state() }, None),
        ];
        for (name, s, hover) in states {
            for scale in [1.0, 2.0] {
                let l = Layout::new(&s, scale, fonts);
                let bar = l.render(&s, hover, fonts).unwrap();
                let mut pix = Pixmap::new(bar.width(), bar.height()).unwrap();
                pix.fill(tiny_skia::Color::from_rgba8(200, 205, 215, 255));
                pix.draw_pixmap(
                    0,
                    0,
                    bar.as_ref(),
                    &tiny_skia::PixmapPaint::default(),
                    tiny_skia::Transform::identity(),
                    None,
                );
                pix.save_png(std::path::Path::new(&dir).join(format!("rec-bar-{name}-{scale}x.png"))).unwrap();
            }
        }
        countdown(3, 2.0, fonts).unwrap().save_png(std::path::Path::new(&dir).join("rec-countdown-2x.png")).unwrap();
    }
}
