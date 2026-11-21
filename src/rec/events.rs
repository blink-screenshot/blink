//! The event log of a screen recording: where the mouse pointer was and what it looked like,
//! the mouse clicks and the keys, with their time on the video's timeline.
//!
//! The video itself is recorded without the pointer. The pointer, click highlights and keys
//! are drawn from this log when the recording is exported, so they can be switched on and off
//! afterwards.
//!
//! The log is a text file with one JSON object per line (JSON Lines): a header first, then
//! the events in time order. A crash halfway leaves a log that can still be read up to the
//! last complete line. Times are milliseconds on the video's timeline (paused time is left
//! out); positions are physical pixels relative to the top-left of the recorded area.
//!
//! Keys are filtered before they are written (see `keys`): in "shortcuts only" mode an
//! ordinary letter never reaches this file.

use super::keys::KeyMode;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

/// Version of the file format.
pub const VERSION: u32 = 1;

/// The first line of the log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub v: u32,
    /// Size of the video in pixels.
    pub w: u32,
    pub h: u32,
    pub fps: u32,
    /// Display scale of the recorded monitor (1.5 at 150%), to size the highlights.
    pub scale: f32,
    /// Video pixels per screen pixel: 1, unless the area was too large for the encoder and
    /// the video was made smaller. Positions in the log are in video pixels already.
    #[serde(default = "one")]
    pub zoom: f32,
    /// Which keys were logged.
    pub keys: KeyMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Button {
    #[serde(rename = "l")]
    Left,
    #[serde(rename = "r")]
    Right,
    #[serde(rename = "m")]
    Middle,
}

/// Modifier keys held at the time of an event. Stored as letters ("cs" is Ctrl + Shift).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl Mods {
    pub fn any(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.win
    }
    fn letters(&self) -> String {
        [(self.ctrl, 'c'), (self.alt, 'a'), (self.shift, 's'), (self.win, 'w')]
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, c)| *c)
            .collect()
    }
    fn from_letters(s: &str) -> Self {
        Mods { ctrl: s.contains('c'), alt: s.contains('a'), shift: s.contains('s'), win: s.contains('w') }
    }
}

impl Serialize for Mods {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.letters())
    }
}

impl<'de> Deserialize<'de> for Mods {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Mods::from_letters(&s))
    }
}

fn one() -> f32 {
    1.0
}

fn no_mods(m: &Mods) -> bool {
    !m.any()
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// What happened.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "lowercase")]
pub enum Kind {
    /// The pointer moved to `(x, y)`.
    Pos {
        x: i32,
        y: i32,
    },
    /// The pointer changed shape: picture `id` (see `cursors/<id>.png`) with its hotspot.
    /// `id` 0: the pointer is hidden.
    Cursor {
        id: u32,
        #[serde(default)]
        hx: i32,
        #[serde(default)]
        hy: i32,
    },
    /// A mouse button went down at `(x, y)`.
    Down {
        b: Button,
        x: i32,
        y: i32,
        #[serde(default, skip_serializing_if = "no_mods")]
        mods: Mods,
    },
    /// A mouse button went up.
    Up {
        b: Button,
    },
    /// A key or key combination to show. `typed`: ordinary text (only in "all keys" mode),
    /// shown together with the text typed just before it.
    Key {
        label: String,
        #[serde(default, skip_serializing_if = "is_false")]
        typed: bool,
    },
    /// The recording was paused here, and went on here. For information; the timeline
    /// already leaves the pause out.
    Pause,
    Resume,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Milliseconds on the video's timeline.
    pub t: u32,
    #[serde(flatten)]
    pub kind: Kind,
}

/// A whole log, read from disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Log {
    pub header: Header,
    /// In time order.
    pub events: Vec<Event>,
}

/// Reads a log. A broken last line (the recording stopped while it was being written) is
/// skipped; a broken line anywhere else, or a missing or newer header, is an error.
pub fn parse(text: &str) -> Result<Log, String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty()).peekable();
    let first = lines.next().ok_or("The event log is empty.")?;
    let header: Header = serde_json::from_str(first).map_err(|e| format!("Event log header: {e}"))?;
    if header.v > VERSION {
        return Err(format!("Event log version {} is newer than this Blink understands.", header.v));
    }
    let mut events = Vec::new();
    while let Some(line) = lines.next() {
        match serde_json::from_str::<Event>(line) {
            Ok(e) => events.push(e),
            Err(_) if lines.peek().is_none() => break,
            Err(e) => return Err(format!("Event log: {e}")),
        }
    }
    // Written in order, but a sort costs nothing and keeps lookups right whatever happens.
    events.sort_by_key(|e| e.t);
    Ok(Log { header, events })
}

pub fn read(path: &Path) -> Result<Log, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse(&text)
}

/// Writes a log line by line, as the recording goes.
pub struct Writer<W: Write> {
    out: W,
}

impl<W: Write> Writer<W> {
    pub fn new(mut out: W, header: &Header) -> std::io::Result<Self> {
        serde_json::to_writer(&mut out, header).map_err(std::io::Error::other)?;
        out.write_all(b"\n")?;
        Ok(Self { out })
    }

    pub fn push(&mut self, e: &Event) -> std::io::Result<()> {
        serde_json::to_writer(&mut self.out, e).map_err(std::io::Error::other)?;
        self.out.write_all(b"\n")
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }

    #[cfg(test)]
    pub fn into_inner(self) -> W {
        self.out
    }
}

/// The recording's clock: milliseconds since the start, without the time it was paused.
/// Takes the current time as an argument (any monotonic milliseconds), so it can be tested.
#[derive(Clone, Copy, Debug)]
pub struct Timeline {
    start: u64,
    paused_total: u64,
    paused_at: Option<u64>,
}

impl Timeline {
    pub fn start(now: u64) -> Self {
        Self { start: now, paused_total: 0, paused_at: None }
    }

    pub fn pause(&mut self, now: u64) {
        if self.paused_at.is_none() {
            self.paused_at = Some(now);
        }
    }

    pub fn resume(&mut self, now: u64) {
        if let Some(p) = self.paused_at.take() {
            self.paused_total += now.saturating_sub(p);
        }
    }

    pub fn paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Where `now` falls on the video's timeline. While paused: the moment it was paused.
    pub fn at(&self, now: u64) -> u32 {
        let now = self.paused_at.unwrap_or(now);
        now.saturating_sub(self.start).saturating_sub(self.paused_total).min(u32::MAX as u64) as u32
    }
}

// ---- what is on screen at a given time ---------------------------------------------------

/// How long a click highlight lasts.
pub const CLICK_MS: u32 = 450;
/// How long a key stays fully visible, and how long it then takes to fade out.
pub const KEY_SHOW_MS: u32 = 1400;
pub const KEY_FADE_MS: u32 = 400;
/// Typed text within this time of the previous key joins it.
const TYPED_JOIN_MS: u32 = 1000;
/// At most this many key labels are shown at once.
const MAX_KEYS: usize = 3;

/// The pointer at a given time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pointer {
    pub x: i32,
    pub y: i32,
    /// Picture id (0: hidden) and its hotspot.
    pub id: u32,
    pub hx: i32,
    pub hy: i32,
}

/// A click being highlighted: where, which button, and how long ago it happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Click {
    pub x: i32,
    pub y: i32,
    pub button: Button,
    pub age: u32,
}

/// A key label being shown, with how long ago it was (last) pressed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyLabel {
    pub label: String,
    pub age: u32,
}

impl KeyLabel {
    /// Opacity: 1 while it is shown, then fading to 0.
    pub fn alpha(&self) -> f32 {
        if self.age <= KEY_SHOW_MS {
            1.0
        } else {
            1.0 - ((self.age - KEY_SHOW_MS) as f32 / KEY_FADE_MS as f32).min(1.0)
        }
    }
}

/// Everything to draw over one frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub pointer: Option<Pointer>,
    pub clicks: Vec<Click>,
    pub keys: Vec<KeyLabel>,
}

impl Log {
    /// Index of the first event after `t`.
    fn upto(&self, t: u32) -> usize {
        self.events.partition_point(|e| e.t <= t)
    }

    /// What is on screen at `t` (ms on the video's timeline).
    pub fn frame_at(&self, t: u32) -> Frame {
        let end = self.upto(t);
        let past = &self.events[..end];

        let mut pos = None;
        let mut shape = None;
        for e in past.iter().rev() {
            match &e.kind {
                Kind::Pos { x, y } if pos.is_none() => pos = Some((*x, *y)),
                Kind::Down { x, y, .. } if pos.is_none() => pos = Some((*x, *y)),
                Kind::Cursor { id, hx, hy } if shape.is_none() => shape = Some((*id, *hx, *hy)),
                _ => {}
            }
            if pos.is_some() && shape.is_some() {
                break;
            }
        }
        let pointer = match (pos, shape) {
            (Some((x, y)), Some((id, hx, hy))) if id != 0 => Some(Pointer { x, y, id, hx, hy }),
            _ => None,
        };

        let since = |ms: u32| self.events[..end].partition_point(|e| e.t + ms < t);
        let clicks = past[since(CLICK_MS)..]
            .iter()
            .filter_map(|e| match e.kind {
                Kind::Down { x, y, b, .. } => Some(Click { x, y, button: b, age: t - e.t }),
                _ => None,
            })
            .collect();

        // Keys: typed text joins into one label; a repeated combination shows once.
        let mut keys: Vec<(KeyLabel, bool, u32)> = Vec::new();
        for e in &past[since(KEY_SHOW_MS + KEY_FADE_MS)..] {
            let Kind::Key { label, typed } = &e.kind else { continue };
            let age = t - e.t;
            if let Some((last, last_typed, last_t)) = keys.last_mut() {
                if *typed && *last_typed && e.t - *last_t <= TYPED_JOIN_MS {
                    last.label.push_str(label);
                    last.age = age;
                    *last_t = e.t;
                    continue;
                }
                if !*typed && !*last_typed && last.label == *label {
                    last.age = age;
                    *last_t = e.t;
                    continue;
                }
            }
            keys.push((KeyLabel { label: label.clone(), age }, *typed, e.t));
        }
        let skip = keys.len().saturating_sub(MAX_KEYS);
        let keys = keys.into_iter().skip(skip).map(|(k, _, _)| k).filter(|k| k.alpha() > 0.0).collect();

        Frame { pointer, clicks, keys }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header { v: VERSION, w: 640, h: 360, fps: 30, scale: 1.5, zoom: 1.0, keys: KeyMode::Shortcuts }
    }

    fn ev(t: u32, kind: Kind) -> Event {
        Event { t, kind }
    }

    fn sample() -> Vec<Event> {
        vec![
            ev(0, Kind::Cursor { id: 1, hx: 2, hy: 3 }),
            ev(0, Kind::Pos { x: 10, y: 20 }),
            ev(500, Kind::Pos { x: 40, y: 50 }),
            ev(
                600,
                Kind::Down { b: Button::Left, x: 41, y: 51, mods: Mods { ctrl: true, shift: true, ..Mods::default() } },
            ),
            ev(700, Kind::Up { b: Button::Left }),
            ev(900, Kind::Key { label: "Ctrl + C".into(), typed: false }),
            ev(1000, Kind::Pause),
            ev(1000, Kind::Resume),
            ev(1200, Kind::Cursor { id: 0, hx: 0, hy: 0 }),
        ]
    }

    #[test]
    fn writes_and_reads_back() {
        let mut w = Writer::new(Vec::new(), &header()).unwrap();
        for e in sample() {
            w.push(&e).unwrap();
        }
        let text = String::from_utf8(w.into_inner()).unwrap();
        // Short and readable: one object per line, modifiers as letters.
        assert!(text.lines().count() == 1 + sample().len());
        assert!(text.contains(r#"{"t":600,"k":"down","b":"l","x":41,"y":51,"mods":"cs"}"#), "{text}");
        assert!(text.contains(r#"{"t":700,"k":"up","b":"l"}"#));
        let log = parse(&text).unwrap();
        assert_eq!(log.header, header());
        assert_eq!(log.events, sample());
    }

    #[test]
    fn a_broken_last_line_is_skipped_but_not_a_broken_middle() {
        let mut w = Writer::new(Vec::new(), &header()).unwrap();
        for e in sample() {
            w.push(&e).unwrap();
        }
        let text = String::from_utf8(w.into_inner()).unwrap();
        let cut = &text[..text.len() - 12];
        let log = parse(cut).unwrap();
        assert_eq!(log.events.len(), sample().len() - 1);

        let mut lines: Vec<&str> = text.lines().collect();
        lines[3] = "{\"t\":5,";
        assert!(parse(&lines.join("\n")).is_err());
        assert!(parse("").is_err());
        let newer = serde_json::to_string(&Header { v: VERSION + 1, ..header() }).unwrap();
        assert!(parse(&newer).is_err());
    }

    #[test]
    fn timeline_leaves_out_pauses() {
        let mut tl = Timeline::start(1000);
        assert_eq!(tl.at(1500), 500);
        tl.pause(2000);
        assert!(tl.paused());
        assert_eq!(tl.at(5000), 1000, "stands still while paused");
        tl.resume(6000);
        assert_eq!(tl.at(6000), 1000);
        assert_eq!(tl.at(6250), 1250);
        tl.resume(7000); // a second resume changes nothing
        assert_eq!(tl.at(7000), 2000);
    }

    #[test]
    fn frame_at_follows_the_pointer_clicks_and_keys() {
        let log = Log { header: header(), events: sample() };
        let f = log.frame_at(100);
        assert_eq!(f.pointer, Some(Pointer { x: 10, y: 20, id: 1, hx: 2, hy: 3 }));
        assert!(f.clicks.is_empty() && f.keys.is_empty());

        let f = log.frame_at(650);
        assert_eq!(f.pointer.map(|p| (p.x, p.y)), Some((41, 51)), "a click moves the pointer too");
        assert_eq!(f.clicks, vec![Click { x: 41, y: 51, button: Button::Left, age: 50 }]);

        // The click is gone after CLICK_MS, the key shows.
        let f = log.frame_at(600 + CLICK_MS + 1);
        assert!(f.clicks.is_empty());
        let f = log.frame_at(1100);
        assert_eq!(f.keys, vec![KeyLabel { label: "Ctrl + C".into(), age: 200 }]);
        assert_eq!(f.keys[0].alpha(), 1.0);
        // Fading, then gone.
        let f = log.frame_at(900 + KEY_SHOW_MS + KEY_FADE_MS / 2);
        assert!((f.keys[0].alpha() - 0.5).abs() < 0.01);
        assert!(log.frame_at(900 + KEY_SHOW_MS + KEY_FADE_MS + 1).keys.is_empty());
        // A hidden pointer is not drawn.
        assert_eq!(log.frame_at(1300).pointer, None);
        // Before anything happened: nothing.
        assert_eq!(Log { header: header(), events: vec![] }.frame_at(10), Frame::default());
    }

    #[test]
    fn typed_text_joins_and_repeats_show_once() {
        let key = |t, label: &str, typed| ev(t, Kind::Key { label: label.into(), typed });
        let events = vec![
            key(0, "h", true),
            key(100, "i", true),
            key(300, "Ctrl + S", false),
            key(400, "Ctrl + S", false),
            key(500, "o", true),
            key(600, "k", true),
        ];
        let log = Log { header: header(), events };
        let labels: Vec<String> = log.frame_at(700).keys.into_iter().map(|k| k.label).collect();
        assert_eq!(labels, ["hi", "Ctrl + S", "ok"]);
        // A pause in typing starts a new label.
        let events = vec![key(0, "a", true), key(TYPED_JOIN_MS + 1, "b", true)];
        let log = Log { header: header(), events };
        let labels: Vec<String> = log.frame_at(TYPED_JOIN_MS + 2).keys.into_iter().map(|k| k.label).collect();
        assert_eq!(labels, ["a", "b"]);
        // At most MAX_KEYS labels.
        let events = (0..6).map(|i| key(i * 10, &format!("F{i}"), false)).collect();
        let log = Log { header: header(), events };
        let labels: Vec<String> = log.frame_at(100).keys.into_iter().map(|k| k.label).collect();
        assert_eq!(labels, ["F3", "F4", "F5"]);
    }
}
