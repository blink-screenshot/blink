//! The XML of a Windows toast notification, and the arguments its buttons send back.
//! Kept free of any Windows API so it can be tested anywhere.

pub struct Button<'a> {
    pub label: &'a str,
    /// Sent back to Blink when the button is pressed.
    pub action: u32,
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Control characters are not allowed in XML (a newline in the text is).
            c if (c as u32) < 0x20 && c != '\n' => {}
            c => out.push(c),
        }
    }
    out
}

/// What a click sends back: which toast (`token`, so a click on an old toast can be ignored)
/// and which action.
pub fn arguments(token: u32, action: u32) -> String {
    format!("{token};{action}")
}

pub fn parse_arguments(s: &str) -> Option<(u32, u32)> {
    let (token, action) = s.split_once(';')?;
    Some((token.parse().ok()?, action.parse().ok()?))
}

/// A toast with a title, a text, and buttons. Clicking the toast itself does `click_action`.
pub fn toast(token: u32, title: &str, body: &str, click_action: u32, buttons: &[Button]) -> String {
    let mut xml = format!(
        r#"<toast launch="{}" activationType="foreground"><visual><binding template="ToastGeneric"><text>{}</text>"#,
        escape(&arguments(token, click_action)),
        escape(title),
    );
    if !body.is_empty() {
        xml.push_str(&format!("<text>{}</text>", escape(body)));
    }
    xml.push_str("</binding></visual>");
    if !buttons.is_empty() {
        xml.push_str("<actions>");
        for b in buttons {
            xml.push_str(&format!(
                r#"<action content="{}" arguments="{}" activationType="foreground"/>"#,
                escape(b.label),
                escape(&arguments(token, b.action)),
            ));
        }
        xml.push_str("</actions>");
    }
    xml.push_str("</toast>");
    xml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_roundtrip() {
        assert_eq!(parse_arguments(&arguments(7, 3)), Some((7, 3)));
        assert_eq!(parse_arguments("garbage"), None);
        assert_eq!(parse_arguments("1;x"), None);
    }

    #[test]
    fn toast_has_buttons_and_escapes_text() {
        let xml = toast(
            4,
            "Saved",
            r#"C:\Pictures\A & B <1>"#,
            1,
            &[Button { label: "Open in editor", action: 1 }, Button { label: "Save", action: 2 }],
        );
        assert!(xml.contains(r#"launch="4;1""#));
        assert!(xml.contains("C:\\Pictures\\A &amp; B &lt;1&gt;"));
        assert!(xml.contains(r#"<action content="Open in editor" arguments="4;1" activationType="foreground"/>"#));
        assert!(xml.contains(r#"<action content="Save" arguments="4;2" activationType="foreground"/>"#));
    }

    #[test]
    fn no_buttons_no_actions_element() {
        assert!(!toast(1, "Hi", "", 1, &[]).contains("<actions>"));
    }
}
