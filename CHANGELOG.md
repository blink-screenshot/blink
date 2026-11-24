# Changelog

All notable changes to Blink, per version. Every version is on [GitHub](https://github.com/blink-screenshot/blink/releases) with its installer; the release notes are taken from this file.

## [1.6.0] - 2026-11-24

### New features

- **Name your screenshots, and sort them into folders.** In Settings > Output, *File name and folders* now takes variables such as `{date}`, `{time}`, `{app}` (the program you captured, like `chrome`), `{mode}`, `{width}`, `{user}` and `{n}` (a number that counts up). Type `\` to make a folder: `{year}-{month}\{app}_{time}` puts every month in a folder of its own. Pick a ready-made pattern (one folder, a folder per month, per app, or both, or numbered), or click a variable to insert it; the example below shows what your next file will be called. Recordings follow the same pattern. Your current names stay exactly as they were. Suggested by @bqoun in #17.
- **Opening your screenshots folder shows the newest one.** *Open screenshots folder* in the tray menu and *Open* in Settings select your newest screenshot in Explorer, also when it is in one of the new folders, so you do not have to scroll down to find it.

### Improvements

- **Pins appear at once.** Pinning a screenshot no longer takes a moment: the pin opens the instant you press it. Drag an edge to resize it (it keeps its shape), and right-click it for every option.
- **A push pin instead of the word.** The notification after a capture and the editor's toolbar show a pin icon for pinning; the *Ask me what to do* menu says *Pin*.
- **More room in the pin's bar**, so its buttons no longer sit cramped together.

### Bug fixes

- **The editor's toolbar in a small window.** The tools now move to a second row instead of running under Copy, Save and the other buttons on the right, and every group of buttons lines up and sits in the middle of the bar.

## [1.5.0] - 2026-11-24

### New features

- **Pin a screenshot to the screen.** Choose *Pin* on the notification after a capture, in the *Ask me what to do* menu, with the Pin button in the editor (`Ctrl` + `P`), or turn on *Pin to the screen* in Settings > General > After capture. The screenshot opens in a small window without a frame that stays above your other windows, right where you took it, so you can keep a value, a design or instructions in view while you work. Its bar has *On top* (Blink remembers your choice for the next pin), *Copy*, *Save as*, *Edit* and the opacity, to see through it to what is below. Drag it anywhere to move it, resize it with `Ctrl` + scroll or the corner, double-click for 100%, `Esc` closes it.
- **Copy tables as tables.** When the text you copy from the screen or select in the editor is a table, Blink keeps its rows and columns: paste it into Excel and every value lands in its own cell; Word, Outlook and Teams make a real table of it. The label in the capture overlay and the notification say when a table was found. In the editor, *As text* next to *Copy table* copies it as plain text instead.

## [1.4.0] - 2026-11-23

### New features

- **Screenshots of HDR screens (experimental).** With HDR on in Windows, bright parts of HDR games, videos and photos came out as flat white in a screenshot. Turn on *Tone map HDR screens* in Settings > Capture and Blink brings them back with their detail and colour. It only does something on screens that have HDR on; on other screens nothing changes. Ordinary apps look the same, though white can come out a little less bright when there is HDR content on the same screen. Not for scrolling captures or recordings. It is off by default: if a screenshot looks wrong with it, turn it off and let us know.

## [1.3.3] - 2026-11-21

### Improvements

- **Blink uses less memory**, most of all with large screenshots and on 4K screens: in the editor (also while you blur or pixelate a large area), in the settings and other windows, while the capture overlay is open and during a scrolling capture. Nothing looks different: every screenshot comes out pixel for pixel the same.
- **Blurring a large area is faster** in the editor: about twice as fast when you blur most of a 4K screenshot.

## [1.3.2] - 2026-11-21

### Improvements

- **Smooth recordings on 4K screens.** Recording kept up badly on high-resolution screens, so videos played back at a few frames per second. Blink now prepares each frame on all processor cores and hands it to the video encoder separately, so capturing never waits for encoding.
- **Half size on sharp screens.** On a screen at 200% scaling or more, recordings are made at half size, which looks like the same screen at 100%: text stays readable, files are four times smaller, and recording and saving are much faster. Choose *Full resolution* in Settings > Recording to keep every pixel.
- **Saving is much faster**, after a recording and in the clip window: the pointer, clicks and keys are now drawn only where they are, instead of converting every whole frame twice.
- **The GIF size estimate is ready sooner.**
- **The clip window starts on GIF** and has a **Save** button that saves straight to your recordings folder, next to *Save as...*.

## [1.3.1] - 2026-11-21

### Bug fixes

- **Screen recording works.** In 1.3.0 the recorder closed as soon as the countdown appeared, and the notification only said it stopped unexpectedly. If a recording ever fails now, the notification and `recorder.log` say why.

## [1.3.0] - 2026-11-21

### New features

- **Record your screen as MP4 or GIF.** Press `Ctrl` + `Alt` + `Shift` + `Print Screen`, or choose *Record* in the capture overlay (key `R`), and pick a region, a window or a screen. After a 3-2-1 countdown a small bar next to the area shows the time, with pause, stop and discard; the tray icon turns into a red dot. Stop with the same hotkey, the stop button or the tray menu. Recordings go to Videos\Blink.
- **A notification after each recording**, like after a screenshot: *Recording saved (0:12, 3.4 MB)* with *Edit*, *As GIF* and *Copy*. Copy puts the file itself on the clipboard, so you can paste it straight into Teams, Slack, an e-mail or a folder.
- **The clip window.** *Edit* opens the recording: play it, drag the handles to cut off the start and the end, switch the mouse pointer, its highlight, the click rings and the keys on or off, and save as MP4 or GIF. Before you save, it shows roughly how large the file will be.
- **Clicks and keys in your recordings.** A ring shows where you click (orange for the left button, blue for the right), and a soft circle can follow the pointer. In Settings > Recording you can also show keys: *Shortcuts and clicks only* shows combinations like `Ctrl` + `C`, keys like `F5` or `Enter` and `Ctrl` + click, but never what you type, so passwords stay out of the video. Off by default.
- **Settings > Recording**: MP4 or GIF, 15 or 30 frames per second, the largest GIF width, the countdown, the highlights, the keys and the folder.

### Improvements

- Blink uses the video encoder that is part of Windows, so the download hardly grew. On Windows N editions recording needs the Media Feature Pack; Blink tells you if it is missing.

## [1.2.0] - 2026-11-21

### New features

- **History.** Your latest screenshots are under *History* in the tray menu, each with a small preview. Click one to copy it again: the same notification as after a capture appears, with *Edit*, *Save* and *Save as*. By default Blink keeps the last 10 screenshots for 24 hours and then deletes them. In Settings > Capture you choose how many it keeps (0 turns History off) and for how many hours or days, or without a time limit, and you can clear it there. They are stored only on your PC.
- **Read QR codes.** In the capture overlay, point at a QR code: it lights up and the label shows what it says. Click to copy it, or `Ctrl` + click to open a link in your browser (only web links open; anything else is copied).
- **Mouse pointer in screenshots.** Turn on *Include the mouse pointer* in Settings > Capture. In the editor the pointer is a separate object you can move, resize or delete; Copy and Save draw it into the screenshot.
- **Deutsch, Français, Español and Português.** Blink now also speaks German, French, Spanish and Brazilian Portuguese (Settings > General > Language).

### Improvements

- **A hand over handles you can drag.** In the editor the pointer turns into a hand over the round end points of arrows and lines and the pointer of a speech bubble, and closes while you drag one, so you can see you are in the right spot.

## [1.1.3] - 2026-11-19

### Improvements

- **Find private info finds shorter keys.** A random-looking run of 12 or more letters and digits (upper case, lower case and digits mixed, like `Xk9mP2vL7qR4tZ` or the end of `pk_test_51H8aBcD2eFgH3iJ`) is now suggested as a key; before, it took 20. Licence names, version numbers and ordinary words with digits are still left alone.

## [1.1.2] - 2026-11-19

### Improvements

- **More text is read in dark mode.** Blink already read dark screens a second time with the colours inverted, but kept only lines the first reading had missed entirely. Now it keeps, line by line, the reading that found the most text, so words that one reading skips (often names) are less likely to go missing when you copy text or use *Find private info*.

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
