# Developing Blink

## Building

```
cargo test
cargo build --release            # on Windows: target\release\Blink.exe
makensis /DVERSION=1.1.3 /DEXE=..\target\release\Blink.exe installer\blink.nsi
```

Cross-compiling from Linux works too (needs mingw-w64 and NSIS):

```
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
makensis -DVERSION=1.1.3 -DEXE=../target/x86_64-pc-windows-gnu/release/Blink.exe installer/blink.nsi
```

The editor and the settings window also run on Linux (`Blink --open image.png`), which is handy for development. Capturing, the tray and hotkeys are Windows-only. All tests run on Linux, and CI runs them on both Windows and Linux.

Useful preview commands (they write PNGs, no Windows needed):

| Command | Writes |
|---|---|
| `BLINK_GALLERY=<dir> cargo test gallery -- --ignored` | every device mockup and background |
| `BLINK_BAR_PREVIEW=<dir> cargo test capture_bar -- --nocapture` | the mode bar of the capture overlay, in every mode and language |
| `BLINK_BAR_PREVIEW=<dir> cargo test rec::bar -- --nocapture` | the recording bar and the countdown |

`src/golden.rs` holds digests of the output of the image pipelines (render, redact, export, frame, stitch, the images handed to OCR): a change that alters one output pixel fails `cargo test`. If a change is meant to alter the output, record the new digests and say so in the pull request. `cargo test --release golden::timing -- --ignored --nocapture` times the same pipelines at 4K; run it before and after a change that should not make them slower.

Memory use on Windows: `powershell -ExecutionPolicy Bypass -File tools\memory.ps1` prints the private bytes and the working set of every running Blink.exe (tray, editor, settings, ...) every second, optionally to a CSV (`-Csv file.csv`). Compare versions with the same steps.

On Windows, `Blink --record-test` records three seconds of the monitor under the mouse and saves it as set in Settings; `%LOCALAPPDATA%\Blink\recordings\recorder.log` says which capture method and encoder were used and what failed.

## Releasing

`main` holds every version; each version has a tag (`v1.0.0`, ...) and a GitHub release with the installer. [CHANGELOG.md](../CHANGELOG.md) describes every version, grouped in *New features*, *Improvements* and *Bug fixes*.

1. Set `version` in `Cargo.toml` (new features: `x.y+1.0`, only fixes: `x.y.z+1`).
2. Add a section `## [x.y.z] - date` at the top of `CHANGELOG.md`.
3. Merge to `main`. The Build workflow publishes `vx.y.z` as a **pre-release**: no user gets it yet.
4. Test the pre-release: download `Blink-Setup-x.y.z.exe` from its release page, run it over the previous version (also silently, as the updater does: `Blink-Setup-x.y.z.exe /S /RESTART`), and check that Blink starts again and works.
5. **Actions > Promote > Run workflow** (leave the version empty for the newest pre-release). From then on the in-app updater offers it to everyone.

In step 3 the Build workflow sees a version in `CHANGELOG.md` without a tag, tags `vx.y.z` on `main`, builds the installer and the portable exe, and publishes the pre-release with the notes from `CHANGELOG.md` (users see them in the update window and in Settings > Updates > What's new). Each release has the installer twice: `Blink-Setup-x.y.z.exe` and `Blink-Setup.exe`, so `releases/latest/download/Blink-Setup.exe` always points to the newest promoted version. After only editing the text of `CHANGELOG.md`: **Actions > Build > Run workflow** on `main`; that updates the notes but never changes which release is latest.

**Rolling back.** If a promoted version turns out bad: **Actions > Promote** with the previous version and *rollback* checked. The previous version becomes latest again and the bad one a pre-release, so nobody else gets it. Users who already updated keep it (the updater never downgrades): fix it in a new patch version.

The product page in `site/` is published to GitHub Pages by `.github/workflows/pages.yml` whenever `site/` changes on `main`.

After changing dependencies, run `python tools/third_party_notices.py` to regenerate `THIRD-PARTY-NOTICES.md`; CI fails when it is out of date.

## How updates work

Once a day (and via the tray icon > *Check for updates*) Blink asks the GitHub API for the latest release of `update::REPO` (pre-releases are never offered). If there is a newer one, a notification shows; clicking it shows what is new, with **Install**. Blink downloads the installer, checks the SHA-256 that GitHub reports for the asset, and runs it silently over the current version (`Blink-Setup-x.y.z.exe /S /RESTART`). The installer is started from the download thread: Windows scans a new, unsigned exe before it starts, which can take seconds, and on the UI thread that made the window show "Not responding". Settings and the "start with Windows" choice stay as they were. *Skip this version* skips one version; the check can be switched off in Settings.

The repository is public, so the check needs no token or account. GitHub allows 60 unauthenticated API requests per hour per IP address; one check a day per PC stays far below that, also behind a shared office connection.

## Design decisions

- **Scaling.** The process is *per-monitor DPI aware (v2)*. All coordinates are physical pixels, a screenshot is a 1:1 copy, and the editor at 100% shows exactly one image pixel per screen pixel. (Tools that mix scaled and physical coordinates produce oddly scaled screenshots on mixed-DPI setups.)
- **Window capture.** Window edges come from `DWMWA_EXTENDED_FRAME_BOUNDS` (the visible edge, without the invisible resize border and shadow of Windows 10/11), and the window is cut out of a frozen copy of the screen. What you see is what you get, also for GPU-rendered apps where `PrintWindow` often returns black.
- **Blur that holds.** A plain Gaussian blur with a small radius keeps the shape of letters and can be partly reversed. Blink first reduces the area to blocks, then blurs three times with a radius that scales with the area, and adds noise. For passwords and API keys, the **black box** is the only option that is safe by definition; the editor says so.
- **Text recognition with the engine Windows has.** `Windows.Media.Ocr` ships with Windows 10 and 11, works offline and adds nothing to the download; Tesseract or an ONNX model would add 10 to 40 MB and a native build. Its weak spot is small text, so images up to about 2.6 megapixels are enlarged twice before reading, images taller than the engine's maximum are read in overlapping bands, and images with a large dark area (dark mode) are read a second time inverted, because the engine is made for dark text on a light background; per line the reading with the most text is kept, because the engine silently skips words it is unsure of, often names and random strings (`ocr::read_with`). The capture overlay reads each monitor separately. *Find private info* only suggests: every finding is shown and can be turned off, and checks (IBAN mod 97, Luhn, elfproef) keep false alarms down where a format has one.
- **Lightweight.** The always-running part (tray, hotkeys, capture, overlay) is plain Win32 with no UI framework. Large images are not copied without need: the screenshot stays in the GDI bitmap it was captured into and the overlay shows that bitmap, blur works in place, every font file is held once per process (the tray maps its fonts from disk), and the editor's textures and exports are made in one copy. The editor starts as a separate process when needed (egui/OpenGL), so an editor crash never takes the tray down, and several editors can run side by side.
- **WYSIWYG.** The editor shows the output of the same software renderer that is used for export.
- **Screen recording with what Windows has.** DXGI Desktop Duplication delivers the frames of one monitor (GPU, no yellow border, only when something changed), with GDI `BitBlt` at 15 fps as the fallback (rotated monitors, a duplication that fails). Media Foundation's Sink Writer encodes H.264 in MP4 at a constant frame rate, the Source Reader decodes; Blink adds no codec to the download. Frames are NV12 throughout (`rec::yuv`): Blink converts the screen to NV12 itself on every core (Media Foundation's own RGB converter uses one core, too slow for 4K), the encoder runs on its own thread with a few reused buffers (a frame is left out when none is free, rather than holding up capture), and on screens at 200% or more the video is half size unless *Full resolution* is on. Media Foundation is loaded with `LoadLibrary`: Windows N editions lack it, and a normal import would stop Blink from starting at all there.
- **The pointer is drawn in afterwards.** The video is recorded without the pointer into a clean master (`%LOCALAPPDATA%\Blink\recordings\<id>`), next to an event log (JSON Lines: pointer position and shape, clicks, keys, with times on the video's timeline). Saving draws the pointer, its highlight, click rings and keys from that log (`rec::compose`), so each can be switched off in the clip window. The price is a second encode when you stop: a short wait, shown on the recording bar. The working folders are deleted after a day (the newest five).
- **Keys are filtered before they are written.** With *Shortcuts and clicks only*, typing (Shift + letter, AltGr characters, Space) never reaches the event log; only *All keys* looks up what a key types. The keyboard hook is not even installed when keys are off.
- **The recorder is its own process** (`Blink --record <dir>`), like the editor: a crash costs one recording, never the tray, and the tray never loads Direct3D or Media Foundation. The tray only picks the area (the overlay's Record mode), starts the recorder, shows a red dot and passes "stop" on.

## Code overview

| File | Contents |
|---|---|
| `src/platform/win/resident.rs` | tray icon, hotkeys, menus, what happens after a capture |
| `src/platform/win/overlay.rs` | selection overlay over all monitors, window snapping, magnifier |
| `src/capture_bar.rs` | the mode bar of the overlay (portable, previewable) |
| `src/platform/win/capture.rs` | screen capture (GDI), window enumeration, DWM bounds |
| `src/platform/win/scroll.rs` | scrolling capture |
| `src/stitch.rs` | stitching scroll frames (tested) |
| `src/ocr.rs`, `src/platform/win/ocr.rs` | reading text: the Windows OCR engine, enlarging small text, reading tall images in bands, selecting words / lines / paragraphs (tested) |
| `src/sensitive.rs` | finding private info in the read text for *Find private info* (tested) |
| `src/qr.rs` | finding and reading QR codes on the frozen screen for the overlay (`rqrr`, tested) |
| `src/history.rs` | the tray menu's History: the latest screenshots in `%LOCALAPPDATA%\Blink\history` (tested) |
| `src/editor/textsel.rs` | the editor's text tool and *Find private info* |
| `src/render.rs` | drawing annotations, blur/pixelate (tested) |
| `src/frame.rs` | backgrounds, device mockups, shadow, composing the frame (tested) |
| `src/editor/` | editor UI (egui), background/mockup side panel, remembered settings |
| `src/platform/win/kbhook.rs` | overriding hotkeys another app already has |
| `src/rec/` | screen recording, portable and tested: the event log (`events`), which keys are shown (`keys`), drawing the pointer, clicks and keys (`compose`), trimming and export (`export`), the GIF encoder (`gif`), the recording bar and countdown (`bar`), working folders (`store`) |
| `src/platform/win/rec/` | screen recording on Windows: the recorder process and its windows (`recorder`), DXGI and GDI capture (`dxgi`), Media Foundation (`mf`), mouse and keyboard hooks (`hooks`), files on the clipboard |
| `src/clip_ui.rs` | the clip window: play, trim, overlays on and off, save as MP4 or GIF |
| `src/platform/win/toast.rs`, `src/toast_xml.rs` | notifications with buttons |
| `src/platform/win/tray_icons.rs` | icons in the tray menu |
| `src/theme.rs` | all colours of the app (contrast is tested) |
| `src/settings_ui.rs` | settings window |
| `src/welcome_ui.rs` | welcome window on first start |
| `src/i18n.rs`, `src/i18n_*.rs` | translations: English is the key, plus tables for Dutch, German, French, Spanish and Portuguese (tests check every text is translated in every table) |
| `src/emoji.rs`, `assets/emoji/` | emoji stamps (Twemoji, CC-BY 4.0), rendered with resvg |
| `src/update.rs`, `src/update_ui.rs`, `src/changelog.rs` | updates via GitHub Releases, What's new |
| `src/platform/win/http.rs` | HTTPS via WinHTTP (system proxy and certificates) |
| `src/platform/other.rs` | stand-ins for non-Windows builds |
| `installer/blink.nsi` | NSIS installer |
| `tools/make_icon.py` | builds the app icon and tray icon |
| `tools/third_party_notices.py` | generates `THIRD-PARTY-NOTICES.md` |
| `site/` | the product page (GitHub Pages) |

## Visual identity: hot pink

Near-black ("ink") with one hot pink accent, taken from the logo. Every colour of the app is defined in one place, `src/theme.rs`: the editor, the settings, welcome and update windows and the capture overlay all use it.

| Role | Colour | Value |
|---|---|---|
| Ink (base, the icon's eyes) | near black | `#101114` |
| Accent (active, selected, primary button, icon tile) | hot pink | `#FF2E93` |
| Accent dim (text on dark surfaces) | muted pink | `#E04C95` |
| Accent deep (text on light surfaces) | deep pink | `#C41A63` |
| Signal (scrolling capture only) | cyan | `#22D3EE` |
| Success / warning / error (dark theme) | green / amber / coral | `#4ADE80` / `#FBBF24` / `#FF7A6B` |

Rules: text and icons on the accent are always ink (ink on hot pink is 5.5:1, above WCAG AA); the accent means "active or primary", never decoration, so success is green and errors are coral; scrolling capture is cyan so the two capture modes never look alike. Tests in `theme.rs` guard the contrast. Annotation colours are functional and separate from the brand.

**Shapes.** The icon tile has a corner radius of 58/256 of its side, and its corner marks run parallel to that edge. The interface follows the same rule: `theme::radius()` gives every element that ratio, and the crop selection and capture overlay use the same four rounded corner marks as the logo.

**Icon.** `python tools/make_icon.py` (needs `pip install cairosvg pillow`) builds `assets/blink.svg`, `.png` and `.ico`, and the tray glyph `assets/blink-tray.svg` and `.ico`, with a hand-tuned drawing per size below 56 px.
