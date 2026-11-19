//! Emoji stamps: a curated set of Twemoji graphics (CC-BY 4.0, by Twitter and contributors,
//! https://github.com/jdecked/twemoji), embedded as SVG and rasterized with resvg, so they are
//! sharp at any size and look the same on every PC.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tiny_skia::{Pixmap, Transform};

/// (id = Twemoji file name, English and Dutch description, SVG).
pub const EMOJI: &[(&str, &str, &str, &[u8])] = &[
    ("1f44d", "thumbs up", "duim omhoog", include_bytes!("../assets/emoji/1f44d.svg")),
    ("1f44e", "thumbs down", "duim omlaag", include_bytes!("../assets/emoji/1f44e.svg")),
    ("2705", "check", "vinkje", include_bytes!("../assets/emoji/2705.svg")),
    ("274c", "cross", "kruis", include_bytes!("../assets/emoji/274c.svg")),
    ("26a0", "warning", "waarschuwing", include_bytes!("../assets/emoji/26a0.svg")),
    ("2757", "exclamation", "uitroepteken", include_bytes!("../assets/emoji/2757.svg")),
    ("2753", "question", "vraagteken", include_bytes!("../assets/emoji/2753.svg")),
    ("1f4a1", "idea", "idee", include_bytes!("../assets/emoji/1f4a1.svg")),
    ("1f525", "fire", "vuur", include_bytes!("../assets/emoji/1f525.svg")),
    ("2b50", "star", "ster", include_bytes!("../assets/emoji/2b50.svg")),
    ("2764", "heart", "hart", include_bytes!("../assets/emoji/2764.svg")),
    ("1f389", "party", "feest", include_bytes!("../assets/emoji/1f389.svg")),
    ("1f600", "smile", "lach", include_bytes!("../assets/emoji/1f600.svg")),
    ("1f602", "laughing", "tranen van het lachen", include_bytes!("../assets/emoji/1f602.svg")),
    ("1f60d", "love", "verliefd", include_bytes!("../assets/emoji/1f60d.svg")),
    ("1f914", "thinking", "nadenken", include_bytes!("../assets/emoji/1f914.svg")),
    ("1f62e", "surprised", "verrast", include_bytes!("../assets/emoji/1f62e.svg")),
    ("1f622", "sad", "verdrietig", include_bytes!("../assets/emoji/1f622.svg")),
    ("1f621", "angry", "boos", include_bytes!("../assets/emoji/1f621.svg")),
    ("1f64f", "please", "alsjeblieft", include_bytes!("../assets/emoji/1f64f.svg")),
    ("1f44f", "applause", "applaus", include_bytes!("../assets/emoji/1f44f.svg")),
    ("1f440", "eyes", "ogen", include_bytes!("../assets/emoji/1f440.svg")),
    ("1f449", "point right", "wijs rechts", include_bytes!("../assets/emoji/1f449.svg")),
    ("1f448", "point left", "wijs links", include_bytes!("../assets/emoji/1f448.svg")),
    ("1f446", "point up", "wijs omhoog", include_bytes!("../assets/emoji/1f446.svg")),
    ("1f447", "point down", "wijs omlaag", include_bytes!("../assets/emoji/1f447.svg")),
    ("1f44c", "ok", "oké", include_bytes!("../assets/emoji/1f44c.svg")),
    ("270b", "stop hand", "stophand", include_bytes!("../assets/emoji/270b.svg")),
    ("1f680", "rocket", "raket", include_bytes!("../assets/emoji/1f680.svg")),
    ("1f41b", "bug", "bug", include_bytes!("../assets/emoji/1f41b.svg")),
    ("1f512", "locked", "op slot", include_bytes!("../assets/emoji/1f512.svg")),
    ("1f511", "key", "sleutel", include_bytes!("../assets/emoji/1f511.svg")),
    ("1f4cc", "pin", "punaise", include_bytes!("../assets/emoji/1f4cc.svg")),
    ("1f4cd", "location", "locatie", include_bytes!("../assets/emoji/1f4cd.svg")),
    ("1f4dd", "note", "notitie", include_bytes!("../assets/emoji/1f4dd.svg")),
    ("1f50d", "search", "zoeken", include_bytes!("../assets/emoji/1f50d.svg")),
    ("23f0", "alarm", "wekker", include_bytes!("../assets/emoji/23f0.svg")),
    ("1f4ac", "speech", "tekstballon", include_bytes!("../assets/emoji/1f4ac.svg")),
    ("1f6d1", "stop sign", "stopbord", include_bytes!("../assets/emoji/1f6d1.svg")),
    ("1f6ab", "prohibited", "verboden", include_bytes!("../assets/emoji/1f6ab.svg")),
    ("2728", "sparkles", "sterretjes", include_bytes!("../assets/emoji/2728.svg")),
    ("1f4af", "hundred", "honderd", include_bytes!("../assets/emoji/1f4af.svg")),
    ("1f3af", "target", "doel", include_bytes!("../assets/emoji/1f3af.svg")),
    ("1f3c6", "trophy", "trofee", include_bytes!("../assets/emoji/1f3c6.svg")),
    ("1f4c8", "chart up", "grafiek omhoog", include_bytes!("../assets/emoji/1f4c8.svg")),
    ("1f4c9", "chart down", "grafiek omlaag", include_bytes!("../assets/emoji/1f4c9.svg")),
    ("31-20e3", "one", "één", include_bytes!("../assets/emoji/31-20e3.svg")),
    ("32-20e3", "two", "twee", include_bytes!("../assets/emoji/32-20e3.svg")),
    ("33-20e3", "three", "drie", include_bytes!("../assets/emoji/33-20e3.svg")),
    ("1f195", "new", "nieuw", include_bytes!("../assets/emoji/1f195.svg")),
    ("2139", "info", "info", include_bytes!("../assets/emoji/2139.svg")),
    ("1f534", "red circle", "rode cirkel", include_bytes!("../assets/emoji/1f534.svg")),
    ("1f7e2", "green circle", "groene cirkel", include_bytes!("../assets/emoji/1f7e2.svg")),
    ("1f7e1", "yellow circle", "gele cirkel", include_bytes!("../assets/emoji/1f7e1.svg")),
];

/// The emoji a new stamp uses until the user picks another.
pub const DEFAULT: &str = "1f44d";

/// Description of emoji `id` in the interface language.
pub fn name(id: &str) -> &'static str {
    let e = EMOJI.iter().find(|e| e.0 == id);
    match (e, crate::i18n::language()) {
        (Some(e), crate::i18n::Language::Dutch) => e.2,
        (Some(e), _) => e.1,
        _ => "",
    }
}

pub fn exists(id: &str) -> bool {
    EMOJI.iter().any(|e| e.0 == id)
}

fn tree(id: &str) -> Option<Arc<resvg::usvg::Tree>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<resvg::usvg::Tree>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(t) = cache.lock().ok()?.get(id) {
        return Some(t.clone());
    }
    let svg = EMOJI.iter().find(|e| e.0 == id)?.3;
    let t = Arc::new(resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default()).ok()?);
    cache.lock().ok()?.insert(id.to_string(), t.clone());
    Some(t)
}

/// Draws emoji `id` into `pm`, filling the square of side `side` with its top-left at (x, y).
pub fn draw(pm: &mut Pixmap, id: &str, x: f32, y: f32, side: f32) {
    let Some(t) = tree(id) else { return };
    let s = t.size();
    let k = side / s.width().max(s.height());
    let ts = Transform::from_scale(k, k).post_translate(x, y);
    resvg::render(&t, ts, &mut pm.as_mut());
}

/// The emoji as a premultiplied square pixmap of `px` pixels.
pub fn raster(id: &str, px: u32) -> Option<Pixmap> {
    let mut pm = Pixmap::new(px, px)?;
    draw(&mut pm, id, 0.0, 0.0, px as f32);
    Some(pm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_emoji_renders() {
        for (id, ..) in EMOJI {
            let pm = raster(id, 32).unwrap_or_else(|| panic!("{id}"));
            assert!(pm.data().chunks(4).any(|p| p[3] > 0), "{id} is empty");
        }
        assert!(exists(DEFAULT));
    }
}
