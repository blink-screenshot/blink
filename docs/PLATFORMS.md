# Blink on macOS and Linux

**Status: Windows only.** Blink runs on Windows 10 and 11 (64-bit). This page explains why, and what a macOS or Linux version needs. If you want to help, open an issue first so the work can be split up.

## What is portable today

About two thirds of Blink is platform-independent Rust and already builds and runs on Linux (and should on macOS):

- the editor and all its tools (egui/eframe, OpenGL)
- the renderer, blur/pixelate, stitching, backgrounds and device mockups (tiny-skia, resvg)
- settings, translations, the changelog, image import/export and the clipboard (arboard)

CI runs the test suite on Linux for this reason.

## What is Windows-only

Everything in `src/platform/win/` (about 3,300 lines), behind the interface in `src/platform/mod.rs`:

| Part | Windows implementation | What macOS needs | What Linux needs |
|---|---|---|---|
| Screen capture | GDI `BitBlt` of the whole desktop | ScreenCaptureKit (macOS 12.3+), with the Screen Recording permission | X11: `XGetImage` / XShm. Wayland: only via the `xdg-desktop-portal` Screenshot portal, which shows its own dialog |
| Window list and snapping | `EnumWindows` + DWM frame bounds | ScreenCaptureKit / `CGWindowListCopyWindowInfo` | X11: `_NET_CLIENT_LIST` + frame extents. Wayland: not available to apps |
| Selection overlay | layered Win32 windows on every monitor | borderless full-screen windows per display (AppKit or winit) | X11: override-redirect windows. Wayland: layer-shell, compositor-specific |
| Global hotkeys | `RegisterHotKey` + low-level keyboard hook | Carbon `RegisterEventHotKey` (the `global-hotkey` crate does this) | X11: `XGrabKey` (`global-hotkey`). Wayland: the GlobalShortcuts portal, not on every desktop |
| Tray icon and menu | `Shell_NotifyIcon` | `NSStatusItem` (the `tray-icon` crate) | StatusNotifierItem / AppIndicator (`tray-icon`) |
| Scrolling capture | synthetic wheel events (`SendInput`) | `CGEvent` scroll events, needs the Accessibility permission | X11: XTest. Wayland: not possible without privileged tools |
| Notifications with buttons | WinRT toasts | `UserNotifications` | D-Bus notifications (`notify-rust`) |
| Start at login, updates | `Run` registry key, NSIS installer | Login item, DMG or Homebrew cask; updater must replace the `.app` | XDG autostart; AppImage, Flatpak or distro packages |

A good part of this can share code across macOS and Linux (and could replace parts of the Windows code later): `xcap` for capture, `global-hotkey` and `tray-icon` for hotkeys and the tray, and an egui or winit window for the overlay. Blink's Windows-specific quality (exact window edges, 1:1 pixels on mixed DPI, capture of GPU-rendered apps) has to be re-earned on each platform.

## Distribution costs

- **macOS:** an unsigned app is blocked by Gatekeeper; users have to right-click > Open or clear the quarantine flag. A proper release needs an Apple Developer account (USD 99 per year), code signing and notarization. Builds are needed for Apple Silicon and Intel (or a universal binary). Screen Recording and Accessibility permissions have to be requested and explained on first use.
- **Linux:** no signing cost, but many targets. An AppImage for X11 is the cheapest start. On Wayland (the default on GNOME and KDE today) a screenshot tool can only capture through the portal: no live window snapping, no own overlay on top of everything, no simulated scrolling. Flameshot and others live with the same limits.

## Suggested order

1. **Linux on X11**, because the editor already runs there and it can be tested in CI with Xvfb. Ship it as an AppImage, clearly marked as X11-only.
2. **macOS**, once someone with a Mac can test it, and once there is a decision on the Apple Developer account.
3. **Linux on Wayland** through the Screenshot portal, with a reduced feature set (no window snapping, no scrolling capture).

Rough size: each platform is several weeks of part-time work, most of it testing on real machines rather than writing code.
