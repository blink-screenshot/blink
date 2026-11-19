//! Finds private information in recognized text, for the "Find private info" suggestions in
//! the editor: e-mail addresses, IBANs, card numbers, phone numbers, IP addresses, keys and
//! tokens, passwords and Dutch citizen service numbers (BSN).
//!
//! Every finding is a suggestion the user confirms, never a promise that a screenshot is
//! clean: OCR misreads characters, and nothing can know every format of every secret. The
//! detectors prefer precision where a check exists (IBAN mod 97, Luhn for cards, the
//! "elfproef" for BSN) and look at the context where it does not (a password follows a
//! label such as "Password:").

use crate::i18n::t;
use crate::ocr::{Bounds, Line, Page};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Email,
    Iban,
    Card,
    Phone,
    Ip,
    Secret,
    Password,
    Bsn,
}

impl Kind {
    pub const ALL: [Kind; 8] =
        [Kind::Email, Kind::Phone, Kind::Iban, Kind::Card, Kind::Bsn, Kind::Ip, Kind::Secret, Kind::Password];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Email => t("E-mail address"),
            Kind::Iban => t("IBAN"),
            Kind::Card => t("Card number"),
            Kind::Phone => t("Phone number"),
            Kind::Ip => t("IP address"),
            Kind::Secret => t("Key or token"),
            Kind::Password => t("Password"),
            Kind::Bsn => t("BSN"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub kind: Kind,
    pub text: String,
    /// Where it is in the image (one line of text).
    pub rect: Bounds,
}

/// What a click at `(x, y)` hides in the redaction tool: a private item that is there (also
/// when it spans several words, like an IBAN in groups of four), or else the word there.
/// `slack` is how far from a word still counts, in image pixels.
pub fn target_at(page: &Page, found: &[Finding], x: f32, y: f32, slack: f32) -> Option<(Bounds, Option<Kind>)> {
    if let Some(f) = found.iter().find(|f| f.rect.expand(slack).contains(x, y)) {
        return Some((f.rect, Some(f.kind)));
    }
    let i = page.word_at(x, y, slack)?;
    let r = page.word(i)?.rect;
    // Only on the word itself (with the slack): not anywhere on its line.
    if !r.expand(slack).contains(x, y) {
        return None;
    }
    Some((r.expand((r.h * 0.12).max(1.0)), None))
}

/// Everything that looks private on `page`.
pub fn scan(page: &Page) -> Vec<Finding> {
    let mut out = Vec::new();
    for line in &page.lines {
        let (text, spans) = line_text(line);
        for (start, end, kind) in find_in(&text) {
            if let Some(rect) = rect_of(line, &spans, start, end) {
                out.push(Finding { kind, text: text[start..end].to_string(), rect });
            }
        }
    }
    out
}

/// The text of a line with the byte range of every word in it (words joined by one space).
fn line_text(line: &Line) -> (String, Vec<(usize, usize)>) {
    let mut s = String::new();
    let mut spans = Vec::new();
    for (i, w) in line.words.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        spans.push((s.len(), s.len() + w.text.len()));
        s.push_str(&w.text);
    }
    (s, spans)
}

/// The image area of the text `start..end` (bytes) of a line: whole words, and the part of a
/// word estimated from the share of its characters (the engine only gives word boxes).
fn rect_of(line: &Line, spans: &[(usize, usize)], start: usize, end: usize) -> Option<Bounds> {
    let mut out: Option<Bounds> = None;
    for (w, &(ws, we)) in line.words.iter().zip(spans) {
        if we <= start || ws >= end {
            continue;
        }
        let chars = w.text.chars().count().max(1) as f32;
        let from = w.text[..start.saturating_sub(ws).min(w.text.len())].chars().count() as f32;
        let to = w.text[..end.min(we) - ws].chars().count() as f32;
        let r = w.rect;
        let part = Bounds::new(r.x + r.w * from / chars, r.y, r.w * (to - from) / chars, r.h);
        out = Some(out.map_or(part, |o| o.union(&part)));
    }
    // A little room: OCR boxes hug the letters, and a redaction should cover them fully.
    out.map(|r| r.expand((r.h * 0.12).max(1.0)))
}

/// The private things in one line of text, as byte ranges, most specific first; a range
/// never overlaps an earlier one.
pub fn find_in(s: &str) -> Vec<(usize, usize, Kind)> {
    let mut found: Vec<(usize, usize, Kind)> = Vec::new();
    let add = |list: Vec<(usize, usize)>, kind: Kind, found: &mut Vec<(usize, usize, Kind)>| {
        for (a, b) in list {
            if a < b && !found.iter().any(|&(fa, fb, _)| a < fb && fa < b) {
                found.push((a, b, kind));
            }
        }
    };
    add(emails(s), Kind::Email, &mut found);
    add(ibans(s), Kind::Iban, &mut found);
    add(cards(s), Kind::Card, &mut found);
    add(known_secrets(s), Kind::Secret, &mut found);
    for kind in [Kind::Password, Kind::Secret] {
        let values = labelled(s).into_iter().filter(|v| v.2 == kind).map(|v| (v.0, v.1)).collect();
        add(values, kind, &mut found);
    }
    add(bsns(s), Kind::Bsn, &mut found);
    add(ips(s), Kind::Ip, &mut found);
    add(phones(s), Kind::Phone, &mut found);
    add(generic_secrets(s), Kind::Secret, &mut found);
    found.sort_by_key(|f| f.0);
    found
}

// ---- tokens ------------------------------------------------------------------------------

/// Whitespace-separated tokens with their byte ranges, without the punctuation around them
/// (quotes, brackets, a trailing comma or full stop).
fn tokens(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices().chain(std::iter::once((s.len(), ' '))) {
        if c.is_whitespace() {
            if let Some(a) = start.take() {
                out.push(trim_token(s, a, i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    out.into_iter().filter(|(a, b)| a < b).collect()
}

fn trim_token(s: &str, mut a: usize, mut b: usize) -> (usize, usize) {
    const LEAD: &[char] = &['(', '[', '{', '<', '"', '\'', '`', '\u{201c}', '\u{2018}', '\u{ab}'];
    const TRAIL: &[char] =
        &[')', ']', '}', '>', '"', '\'', '`', ',', ';', ':', '.', '!', '?', '\u{201d}', '\u{2019}', '\u{bb}'];
    while a < b {
        let c = s[a..b].chars().next().unwrap();
        if !LEAD.contains(&c) {
            break;
        }
        a += c.len_utf8();
    }
    while a < b {
        let c = s[a..b].chars().next_back().unwrap();
        if !TRAIL.contains(&c) {
            break;
        }
        b -= c.len_utf8();
    }
    (a, b)
}

// ---- e-mail --------------------------------------------------------------------------------

fn emails(s: &str) -> Vec<(usize, usize)> {
    tokens(s).into_iter().filter_map(|(a, b)| email_in(s, a, b)).collect()
}

/// The e-mail address in token `a..b` ("mailto:" and a label before it are left out).
fn email_in(s: &str, a: usize, b: usize) -> Option<(usize, usize)> {
    let tok = &s[a..b];
    let at = tok.find('@')?;
    let local_ok = |c: char| c.is_alphanumeric() || "._%+-'".contains(c);
    let local_start = tok[..at].char_indices().rev().take_while(|(_, c)| local_ok(*c)).last().map(|(i, _)| i)?;
    let local = &tok[local_start..at];
    let domain_end = tok[at + 1..]
        .char_indices()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '.' || *c == '-'))
        .map_or(tok.len(), |(i, _)| at + 1 + i);
    let domain = tok[at + 1..domain_end].trim_end_matches(['.', '-']);
    let tld = domain.rsplit('.').next()?;
    let ok = !local.is_empty()
        && !local.starts_with('.')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.contains("..")
        && tld.chars().count() >= 2
        && tld.chars().all(char::is_alphabetic);
    ok.then(|| (a + local_start, a + at + 1 + domain.len()))
}

// ---- IBAN --------------------------------------------------------------------------------

/// Characters of `s` from `start` while they are letters/digits or a single space between
/// groups: (the compact characters, the byte where each one ends).
fn grouped(s: &str, start: usize, allowed: impl Fn(char) -> bool, max: usize) -> (String, Vec<usize>) {
    let mut compact = String::new();
    let mut ends = Vec::new();
    let mut prev_space = false;
    for (i, c) in s[start..].char_indices() {
        if allowed(c) {
            compact.push(c);
            ends.push(start + i + c.len_utf8());
            prev_space = false;
            if compact.len() >= max {
                break;
            }
        } else if (c == ' ' || c == '-') && !prev_space && !compact.is_empty() {
            prev_space = true;
        } else {
            break;
        }
    }
    (compact, ends)
}

fn boundary_before(s: &str, i: usize) -> bool {
    s[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric())
}

fn boundary_after(s: &str, i: usize) -> bool {
    s[i..].chars().next().is_none_or(|c| !c.is_alphanumeric())
}

fn iban_valid(iban: &str) -> bool {
    if !(15..=34).contains(&iban.len()) {
        return false;
    }
    let b = iban.as_bytes();
    if !(b[0].is_ascii_uppercase() && b[1].is_ascii_uppercase() && b[2].is_ascii_digit() && b[3].is_ascii_digit()) {
        return false;
    }
    let rearranged = iban[4..].chars().chain(iban[..4].chars());
    let mut rem: u32 = 0;
    for c in rearranged {
        let v = match c {
            '0'..='9' => c as u32 - '0' as u32,
            'A'..='Z' => c as u32 - 'A' as u32 + 10,
            _ => return false,
        };
        for d in if v >= 10 { vec![v / 10, v % 10] } else { vec![v] } {
            rem = (rem * 10 + d) % 97;
        }
    }
    rem == 1
}

fn ibans(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if s.is_char_boundary(i) && boundary_before(s, i) {
            let (compact, ends) = grouped(s, i, |c| c.is_ascii_digit() || c.is_ascii_uppercase(), 34);
            // The longest prefix that is a valid IBAN (text may follow it on the line).
            if let Some(n) =
                (15..=compact.len()).rev().find(|&n| iban_valid(&compact[..n]) && boundary_after(s, ends[n - 1]))
            {
                out.push((i, ends[n - 1]));
                i = ends[n - 1];
                continue;
            }
        }
        i += 1;
    }
    out
}

// ---- digits: cards, phones, BSN, IP --------------------------------------------------------

fn luhn(digits: &str) -> bool {
    let mut sum = 0;
    for (i, c) in digits.chars().rev().enumerate() {
        let mut d = c.to_digit(10).unwrap_or(0);
        if i % 2 == 1 {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
    }
    sum % 10 == 0
}

/// Runs of digits with single spaces or dashes between groups, starting at a word boundary.
fn digit_runs(s: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if s.is_char_boundary(i) && s[i..].starts_with(|c: char| c.is_ascii_digit()) && boundary_before(s, i) {
            let (compact, ends) = grouped(s, i, |c| c.is_ascii_digit(), 64);
            let end = *ends.last().unwrap();
            if boundary_after(s, end) {
                out.push((i, end, compact));
            }
            i = end;
            continue;
        }
        i += 1;
    }
    out
}

fn cards(s: &str) -> Vec<(usize, usize)> {
    digit_runs(s)
        .into_iter()
        .filter(|(_, _, d)| (13..=19).contains(&d.len()) && luhn(d) && !d.chars().all(|c| c == '0'))
        .map(|(a, b, _)| (a, b))
        .collect()
}

/// Dutch citizen service number: 9 digits that pass the "elfproef", only on a line that says
/// it is one (any 9-digit number passes the check one time in eleven).
fn bsns(s: &str) -> Vec<(usize, usize)> {
    let lower = s.to_lowercase();
    if !["bsn", "burgerservice", "sofi"].iter().any(|k| lower.contains(k)) {
        return Vec::new();
    }
    let elf = |d: &str| {
        let v: Vec<u32> = d.chars().filter_map(|c| c.to_digit(10)).collect();
        let sum: i32 = v.iter().take(8).enumerate().map(|(i, x)| (9 - i as i32) * *x as i32).sum::<i32>() - v[8] as i32;
        sum % 11 == 0 && sum != 0
    };
    let mut out = Vec::new();
    for (a, b) in tokens(s) {
        let d: String = s[a..b].chars().filter(|c| *c != '.').collect();
        if d.len() == 9 && d.chars().all(|c| c.is_ascii_digit()) && elf(&d) {
            out.push((a, b));
        }
    }
    out
}

fn phones(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    let b = s.as_bytes();
    while i < s.len() {
        let c = b[i];
        let starts = (c == b'+' || c == b'(' || c == b'0') && boundary_before(s, i);
        if !starts {
            i += 1;
            continue;
        }
        // Digits, single separators (space, dash, dot, slash) and one pair of brackets.
        let mut j = i;
        let mut digits = 0;
        let mut last_digit_end = i;
        let mut prev_sep = false;
        while j < s.len() {
            let ch = b[j];
            if ch.is_ascii_digit() {
                digits += 1;
                last_digit_end = j + 1;
                prev_sep = false;
            } else if (ch == b'+' && j == i) || ch == b'(' || ch == b')' {
                prev_sep = false;
            } else if matches!(ch, b' ' | b'-' | b'.' | b'/') && !prev_sep {
                prev_sep = true;
            } else {
                break;
            }
            j += 1;
        }
        let text = &s[i..last_digit_end];
        let international = c == b'+';
        let ok = if international { (8..=15).contains(&digits) } else { (10..=11).contains(&digits) };
        // "0.0000123456" and "0,12" are numbers, not phone numbers: a phone number has no
        // decimal point right after its first digit.
        let decimal = text.starts_with("0.") || text.starts_with("0,");
        if ok && !decimal && boundary_after(s, last_digit_end) {
            out.push((i, last_digit_end));
            i = last_digit_end;
        } else {
            i = j.max(i + 1);
        }
    }
    out
}

fn ips(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (a, b) in tokens(s) {
        let tok = &s[a..b];
        // An address with a port or a network size ("10.0.0.1:8080", "10.0.0.0/24").
        let ip_end = tok.find([':', '/']).unwrap_or(tok.len());
        let parts: Vec<&str> = tok[..ip_end].split('.').collect();
        let ok = parts.len() == 4
            && parts.iter().all(|p| (1..=3).contains(&p.len()) && p.chars().all(|c| c.is_ascii_digit()))
            && parts.iter().all(|p| p.parse::<u32>().is_ok_and(|v| v <= 255));
        if ok {
            out.push((a, a + ip_end));
        }
    }
    out
}

// ---- keys, tokens, passwords ---------------------------------------------------------------

/// Prefixes of well-known API keys and tokens.
const KEY_PREFIXES: &[&str] = &[
    "sk-",
    "sk_live_",
    "sk_test_",
    "rk_live_",
    "rk_test_",
    "pk_live_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxs-",
    "xapp-",
    "AKIA",
    "ASIA",
    "AIza",
    "SG.",
    "npm_",
    "hf_",
    "shpat_",
    "dop_v1_",
    "eyJ",
];

fn token_chars(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-_./+=".contains(c)
}

fn known_secrets(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (a, b) in tokens(s) {
        // A label glued to the value: "key=sk-...", "token:ghp_...".
        let tok = &s[a..b];
        let start = tok.rfind(['=', ':']).map_or(0, |i| i + 1);
        let value = &tok[start..];
        let known = KEY_PREFIXES.iter().any(|p| value.starts_with(p) && value.len() >= p.len() + 12);
        if known && value.chars().all(token_chars) {
            out.push((a + start, b));
        }
    }
    out
}

/// Bits of information per character: random keys are high, words and paths are low.
fn entropy(s: &str) -> f32 {
    let mut counts = std::collections::HashMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0u32) += 1;
    }
    let n = s.chars().count() as f32;
    counts
        .values()
        .map(|&k| {
            let p = k as f32 / n;
            -p * p.log2()
        })
        .sum()
}

/// Long random-looking strings: keys and tokens without a known prefix, and long hex
/// strings (32+ characters: API keys, but also commit hashes, which are left to the user).
fn generic_secrets(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (a, b) in tokens(s) {
        let tok = &s[a..b];
        let start = tok.rfind(['=', ':']).map_or(0, |i| i + 1);
        let value = &tok[start..];
        if value.len() < 20 || !value.chars().all(token_chars) || value.contains("://") {
            continue;
        }
        let hex = value.len() >= 32 && value.chars().all(|c| c.is_ascii_hexdigit());
        let lower = value.chars().any(|c| c.is_ascii_lowercase());
        let upper = value.chars().any(|c| c.is_ascii_uppercase());
        let digit = value.chars().any(|c| c.is_ascii_digit());
        // Paths and dotted names ("com.example.app", "/usr/local/bin") are not secrets.
        let separators = value.chars().filter(|c| matches!(c, '/' | '.')).count();
        let random = digit && (lower as u8 + upper as u8) >= 1 && (lower && upper || hex) && entropy(value) >= 3.6;
        if (hex || random) && separators <= 2 {
            out.push((a + start, b));
        }
    }
    out
}

/// The value after a label like "Password:" or "token =": the next token, or what follows
/// the separator in the same token ("password=hunter2"). A password label gives a password,
/// a key or token label a key or token.
fn labelled(s: &str) -> Vec<(usize, usize, Kind)> {
    const LABELS: &[(&str, Kind)] = &[
        ("password", Kind::Password),
        ("passwd", Kind::Password),
        ("pwd", Kind::Password),
        ("passphrase", Kind::Password),
        ("wachtwoord", Kind::Password),
        ("pincode", Kind::Password),
        ("pin", Kind::Password),
        ("wifi key", Kind::Password),
        ("wi-fi key", Kind::Password),
        ("secret", Kind::Secret),
        ("token", Kind::Secret),
        ("api key", Kind::Secret),
        ("api-key", Kind::Secret),
        ("apikey", Kind::Secret),
        ("api_key", Kind::Secret),
        ("access key", Kind::Secret),
        ("client secret", Kind::Secret),
    ];
    let lower = s.to_lowercase();
    // The lower-case text has the same byte offsets only for ASCII; skip the rest.
    if lower.len() != s.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for &(label, kind) in LABELS {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(label) {
            let at = from + pos;
            from = at + label.len();
            if !boundary_before(s, at) {
                continue;
            }
            // Only a label when a separator follows: "Password: x", "password = x".
            let rest = &s[from..];
            let sep = rest.char_indices().find(|(_, c)| !c.is_whitespace());
            let Some((si, sc)) = sep else { continue };
            if sc != ':' && sc != '=' {
                continue;
            }
            let after = from + si + 1;
            let value_start = s[after..].char_indices().find(|(_, c)| !c.is_whitespace()).map(|(i, _)| after + i);
            let Some(vs) = value_start else { continue };
            let ve = s[vs..].char_indices().find(|(_, c)| c.is_whitespace()).map_or(s.len(), |(i, _)| vs + i);
            // Only quotes and brackets around it: a password can end in "!" or ".".
            let quote = |c: char| "\"'`()[]<>\u{201c}\u{201d}".contains(c);
            let vs = vs + s[vs..ve].chars().take_while(|c| quote(*c)).map(char::len_utf8).sum::<usize>();
            let ve = ve - s[vs..ve].chars().rev().take_while(|c| quote(*c)).map(char::len_utf8).sum::<usize>();
            // Masked values ("••••••", "******") have nothing left to hide.
            let masked = s[vs..ve].chars().all(|c| "*•●·.x".contains(c));
            if ve > vs && !masked {
                out.push((vs, ve, kind));
            }
        }
    }
    out.sort_by_key(|v| (v.0, v.1));
    out.dedup_by_key(|v| (v.0, v.1));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::tests::{line_at, page};

    fn kinds(s: &str) -> Vec<(String, Kind)> {
        find_in(s).into_iter().map(|(a, b, k)| (s[a..b].to_string(), k)).collect()
    }

    fn one(s: &str) -> (String, Kind) {
        let k = kinds(s);
        assert_eq!(k.len(), 1, "{s:?}: {k:?}");
        k[0].clone()
    }

    #[test]
    fn emails() {
        assert_eq!(
            one("Contact: jan.de-vries+test@example.co.uk."),
            ("jan.de-vries+test@example.co.uk".into(), Kind::Email)
        );
        assert_eq!(one("<info@shop.nl>"), ("info@shop.nl".into(), Kind::Email));
        assert_eq!(one("mailto:a@b.io"), ("a@b.io".into(), Kind::Email));
        assert!(kinds("@username and a@b").is_empty());
        assert!(kinds("user@localhost").is_empty());
    }

    #[test]
    fn ibans_are_checked() {
        assert_eq!(one("IBAN: NL91 ABNA 0417 1643 00 t.n.v. X"), ("NL91 ABNA 0417 1643 00".into(), Kind::Iban));
        assert_eq!(one("NL91ABNA0417164300"), ("NL91ABNA0417164300".into(), Kind::Iban));
        assert_eq!(one("DE89 3704 0044 0532 0130 00").1, Kind::Iban);
        // One digit off: not an IBAN.
        assert!(kinds("NL91 ABNA 0417 1643 01").iter().all(|k| k.1 != Kind::Iban));
    }

    #[test]
    fn cards_pass_luhn() {
        assert_eq!(one("Visa 4111 1111 1111 1111 exp 12/29"), ("4111 1111 1111 1111".into(), Kind::Card));
        assert_eq!(one("5500-0000-0000-0004").1, Kind::Card);
        assert!(kinds("Order 4111 1111 1111 1112").iter().all(|k| k.1 != Kind::Card));
    }

    #[test]
    fn phones() {
        assert_eq!(one("Bel +31 6 1234 5678 vandaag"), ("+31 6 1234 5678".into(), Kind::Phone));
        assert_eq!(one("tel: 06-12345678"), ("06-12345678".into(), Kind::Phone));
        assert_eq!(one("(020) 123 4567").1, Kind::Phone);
        assert_eq!(one("+1 (555) 123-4567").1, Kind::Phone);
        // Not phone numbers: amounts, dates, short numbers, decimals.
        for s in ["Total 1234567890", "0.000123456789", "2024-01-15 10:00", "Order 0123", "v0.12.3"] {
            assert!(kinds(s).iter().all(|k| k.1 != Kind::Phone), "{s}");
        }
    }

    #[test]
    fn ip_addresses() {
        assert_eq!(one("Server 192.168.1.20 is down"), ("192.168.1.20".into(), Kind::Ip));
        assert_eq!(one("http 10.0.0.1:8080").0, "10.0.0.1");
        assert!(kinds("version 1.2.3 or 300.1.1.1 or 1.2.3.4.5").is_empty());
    }

    #[test]
    fn keys_and_tokens() {
        assert_eq!(one("OPENAI_API_KEY=sk-proj-abcdefghijklmnopqrstuv").0, "sk-proj-abcdefghijklmnopqrstuv");
        assert_eq!(one("token ghp_16C7e42F292c6912E7710c838347Ae178B4a").1, Kind::Secret);
        assert_eq!(one("AKIAIOSFODNN7EXAMPLE").1, Kind::Secret);
        assert_eq!(one("sha 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b").1, Kind::Secret);
        // After a label, also when it does not look random.
        assert_eq!(one("x-api-key: abc123").1, Kind::Secret);
        assert_eq!(one("API key: sk-proj-a8F3kL9qZx72mNbV4tR1").1, Kind::Secret);
        assert_eq!(one("aZ3kP9qL2mX7vB8nR4tY6wE1uI5oA0sD").1, Kind::Secret);
        // Ordinary long words, paths and URLs are not.
        for s in [
            "internationalization",
            "/usr/local/share/applications/blink.desktop",
            "https://example.com/some/long/path/to/a/page",
            "com.example.application.MainActivity",
            "Supercalifragilisticexpialidocious",
        ] {
            assert!(kinds(s).is_empty(), "{s}: {:?}", kinds(s));
        }
    }

    #[test]
    fn passwords_after_a_label() {
        assert_eq!(one("Password: hunter2"), ("hunter2".into(), Kind::Password));
        assert_eq!(one("wachtwoord = Zomer2024!"), ("Zomer2024!".into(), Kind::Password));
        assert_eq!(one("DB_PASSWORD=s3cr3t"), ("s3cr3t".into(), Kind::Password));
        assert!(kinds("Forgot your password?").is_empty());
        assert!(kinds("Password: ••••••••").is_empty());
    }

    #[test]
    fn bsn_needs_its_label() {
        assert_eq!(one("BSN: 111222333"), ("111222333".into(), Kind::Bsn));
        assert!(kinds("Ordernummer 111222333").is_empty());
        assert!(kinds("BSN: 111222334").is_empty());
    }

    #[test]
    fn a_click_hides_the_whole_item_or_one_word() {
        let p = page(&[("IBAN NL91 ABNA 0417 1643 00", 0.0, 0.0), ("just words here", 0.0, 40.0)]);
        let found = scan(&p);
        // On "0417" (third group): the whole IBAN, not only that group.
        let w = p.lines[0].words[3].rect;
        let (r, kind) = target_at(&p, &found, w.cx(), w.cy(), 2.0).unwrap();
        assert_eq!(kind, Some(Kind::Iban));
        assert!(r.x < p.lines[0].words[1].rect.x + 1.0 && r.right() > p.lines[0].words[5].rect.right() - 1.0);
        // On an ordinary word: that word, a little bigger.
        let w = p.lines[1].words[1].rect;
        let (r, kind) = target_at(&p, &found, w.cx(), w.cy(), 2.0).unwrap();
        assert_eq!(kind, None);
        assert!(r.x < w.x && r.right() > w.right() && r.w < w.w + 6.0);
        // Next to the text: nothing (a drag there makes a box as usual).
        assert_eq!(target_at(&p, &found, 400.0, 46.0, 2.0), None);
        assert_eq!(target_at(&p, &found, 10.0, 25.0, 2.0), None);
    }

    #[test]
    fn findings_have_image_positions() {
        // "mail" "jan@x.nl" "now": the e-mail is the second word.
        let p = page(&[("mail jan@x.nl now", 10.0, 100.0)]);
        let f = scan(&p);
        assert_eq!(f.len(), 1);
        let w = &p.lines[0].words[1].rect;
        assert!(f[0].rect.x <= w.x && f[0].rect.right() >= w.right());
        assert!(f[0].rect.x > p.lines[0].words[0].rect.right() - 3.0);

        // Part of a word: "pwd=secret1" -> only the value, estimated from the characters.
        let p = Page { lines: vec![line_at("pwd=secret1", 0.0, 0.0, 12.0)] };
        let f = scan(&p);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].text, "secret1");
        // 4 of 11 characters are the label: the box starts about 40 px in (10 px each).
        assert!((f[0].rect.x - 40.0).abs() < 3.0, "{:?}", f[0].rect);

        // Spread over several words: an IBAN.
        let p = page(&[("IBAN NL91 ABNA 0417 1643 00", 0.0, 0.0)]);
        let f = scan(&p);
        assert_eq!(f.len(), 1);
        assert!(f[0].rect.x > 30.0 && f[0].rect.w > 150.0);
    }
}
