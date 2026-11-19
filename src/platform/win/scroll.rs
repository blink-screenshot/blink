//! Scrolling ("full page") capture: scrolls the area under the selection with synthetic
//! mouse-wheel input, grabs a frame after each step and stitches the frames.
//!
//! This works with anything that scrolls on the mouse wheel (browsers, editors, chat apps,
//! Explorer). It cannot work for content that changes while scrolling (videos, animated
//! headers, lazy-loading placeholders); the stitcher stops cleanly when frames no longer
//! line up and returns what it has so far.

use super::capture::{grab_rgba, IRect};
use crate::config::Config;
use crate::stitch::{nearly_equal, Step, Stitcher};
use image::RgbaImage;
use std::time::{Duration, Instant};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_WHEEL, MOUSEINPUT, VK_ESCAPE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{SetCursorPos, WHEEL_DELTA};

fn wheel(notches: i32) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: (notches * WHEEL_DELTA as i32) as _,
                dwFlags: MOUSEEVENTF_WHEEL,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

fn escape_pressed() -> bool {
    unsafe { (GetAsyncKeyState(VK_ESCAPE as i32) as u16 & 0x8000) != 0 }
}

/// Grabs until two consecutive frames are the same (smooth scrolling / rendering done).
/// "The same" tolerates tiny changes such as a blinking text cursor, which would otherwise
/// keep every step waiting for the full timeout.
fn settled_frame(rect: IRect, delay: Duration) -> Option<RgbaImage> {
    let start = Instant::now();
    std::thread::sleep(delay);
    let mut prev = grab_rgba(rect)?;
    loop {
        std::thread::sleep(Duration::from_millis(40));
        let cur = grab_rgba(rect)?;
        if cur == prev || nearly_equal(&cur, &prev) || start.elapsed() > Duration::from_millis(1500) {
            return Some(cur);
        }
        prev = cur;
    }
}

pub fn capture(rect: IRect, cfg: &Config) -> Option<RgbaImage> {
    if rect.w < 40 || rect.h < 40 {
        return grab_rgba(rect);
    }
    let delay = Duration::from_millis(cfg.scroll_delay_ms.clamp(30, 2000) as u64);
    // Keep the pointer near the right edge (scrollbar area) to avoid hover effects.
    let px = rect.right() - (rect.w / 4).min(30);
    let py = rect.y + rect.h / 2;
    unsafe {
        SetCursorPos(px, py);
    }
    std::thread::sleep(Duration::from_millis(120));

    // Scroll to the top first.
    let mut before = grab_rgba(rect)?;
    for _ in 0..40 {
        if escape_pressed() {
            return Some(before);
        }
        for _ in 0..4 {
            wheel(10);
        }
        let after = settled_frame(rect, delay)?;
        if after == before || nearly_equal(&after, &before) {
            break;
        }
        before = after;
    }

    let mut st = Stitcher::new(cfg.scroll_max_height);
    st.push(before, None);
    let mut notches = 2;
    let mut px_per_notch: Option<f32> = None;
    let mut retried = false;
    for _ in 0..1000 {
        if escape_pressed() {
            break;
        }
        wheel(-notches);
        let Some(frame) = settled_frame(rect, delay) else { break };
        let hint = px_per_notch.map(|p| (p * notches as f32).round() as usize);
        match st.push(frame, hint) {
            Step::Appended(s) if s > 0 => {
                retried = false;
                let ppn = s as f32 / notches as f32;
                px_per_notch = Some(ppn);
                // Aim for ~45% of the view per step: fast, but with ample overlap.
                notches = ((rect.h as f32 * 0.45) / ppn).floor().clamp(1.0, 25.0) as i32;
            }
            Step::Appended(_) => {}
            Step::Mismatch if !retried => {
                // Often content that was still loading or animating: give it time, then
                // compare again. If the step was too large, scroll back half of it.
                retried = true;
                std::thread::sleep(delay * 3);
                let Some(frame) = settled_frame(rect, delay) else { break };
                match st.push(frame, hint) {
                    Step::Appended(_) => continue,
                    Step::Mismatch if notches > 1 => {
                        let back = notches / 2;
                        wheel(back);
                        notches -= back;
                        let Some(frame) = settled_frame(rect, delay) else { break };
                        if !matches!(st.push(frame, None), Step::Appended(_)) {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            Step::NoChange | Step::Mismatch | Step::Full => break,
        }
    }
    st.finish()
}
