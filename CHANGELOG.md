# Changelog

All notable changes to Blink, per version. Every version is on [GitHub](https://github.com/blink-screenshot/blink/releases) with its installer; the release notes are taken from this file.

## [1.1.1] - 2026-11-19

### Improvements

- **Text in dark mode is read much better.** Light text on a dark background (dark themes, code editors, terminals) often came back as *No text found*. Blink now also reads dark areas inverted, so copying text and *Find private info* work there too.
- **Find private info also finds passwords without a label.** A word like `Winter2026!` or `P@ssw0rd` (upper and lower case, a digit and a symbol) is now suggested even when there is no *Password:* in front of it. Ordinary text and code are left alone.

## [1.1.0] - 2026-11-19

### New features

- **Copy text from the screen.** The capture overlay has a new mode, *Text* (press `T`, or the new hotkey `Ctrl + Alt + Print Screen`, or *Copy text from the screen* in the tray menu). Blink reads the screen while you aim, and the text it found lights up. Click a line to copy it, `Ctrl` + click to copy its whole paragraph, drag to copy all text in a region, or click a window without text under the mouse to copy the text of that window. The label next to the selection shows what will be copied. A notification shows the start of the copied text, with *Open in editor* to select more precisely.
- **Select text (OCR)** in the editor (`X`): drag over text to select it like in a document, or drag a box around it. Double-click selects a line, triple-click a paragraph, `Ctrl + A` everything. Copy it with `Ctrl + C` or the *Copy text* button next to the selection, or hide it with *Hide*, which covers it in the mode of the redaction tool. *Copy all* copies all text in the (cropped) image.
- **Hide text with one click** in the redaction tool: the word under the mouse lights up, or the whole item when it is private info (an e-mail address, a key, an IBAN over several words, ...), and a click hides it. Dragging still hides any area, for things that are not text.
- **Find private info** (in the redaction tool and the text tool): Blink reads the screenshot and suggests what to hide: e-mail addresses, phone numbers, IBANs, card numbers, IP addresses, keys and tokens, passwords and Dutch citizen service numbers (BSN). Click a suggestion to leave it out, pick blur, pixelate or black box, and hide the rest in one step (one undo). It is a suggestion, not a guarantee: check the screenshot yourself.

Text recognition uses the OCR engine built into Windows 10 and 11. It runs on your PC, needs no download and no internet, and reads the languages installed in Windows (Settings > Time & language > Language & region).

## [1.0.0] - 2026-11-19

### New features

- **The first public release of Blink**, a free, open-source screenshot tool for Windows 10 and 11.
- **Capture** a region, a window, an inner pane (Ctrl + click), the screen under the mouse, all screens, the active window, or a whole **scrolling page**.
- **Annotate** with arrows, lines, shapes, freehand, highlighter, text, speech bubbles, numbered steps and emoji stamps, and crop.
- **Redact** with blur, pixelate or a black box. The blur cannot be undone to reveal the text.
- **Present** your screenshot on a background or in a device mockup, with padding, rounded corners and shadow.
- **Save** as PNG, JPEG, WebP or BMP, or copy it. A notification after each screenshot offers Edit, Save and Save as.
- Configurable hotkeys, start with Windows, English and Dutch, dark and light theme.
- **Updates**: Blink checks GitHub once a day (you can turn this off) and installs an update with one click, after verifying its checksum. Settings > Updates shows what changed in each version.
