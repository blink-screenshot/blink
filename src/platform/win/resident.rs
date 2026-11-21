//! The always-running part: tray icon, global hotkeys and capture orchestration.
//! Pure Win32 with no UI framework, so it idles at a few MB of RAM and reacts instantly.

use super::capture::{
    app_window, class_name, foreground_app_window, grab, grab_pointer, grab_rgba, is_cloaked, top_level_windows,
    virtual_screen, window_bounds, IRect,
};
use super::kbhook::{self, FORCED_HOTKEY_MSG};
use super::tray_icons::{self, Icon};
use super::{cursor_pos, monitor_at, overlay, scroll, toast, wide, CAPTURE_MESSAGE, RELOAD_MESSAGE, RESIDENT_CLASS};
use crate::capture_bar::Mode;
use crate::config::{config_path, Config, Hotkey, HotkeyState, HotkeyStatus, HotkeyStatusEntry};
use crate::i18n::{t, tf};
use crate::imageio::{self, Pointer};
use crate::ocr::{self, OcrError};
use crate::toast_xml::Button;
use core::ffi::c_void;
use eframe::egui::Color32;
use image::RgbaImage;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows::UI::Notifications::ToastNotification;
use windows_sys::Win32::Foundation::{GetLastError, BOOL, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmFlush, DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED};
use windows_sys::Win32::Graphics::Gdi::DeleteObject;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId};
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_NONE, NIIF_WARNING, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DispatchMessageW, EnumWindows, FindWindowW, GetForegroundWindow, GetMenuItemCount, GetMessageW, GetSystemMetrics,
    GetWindowThreadProcessId, IsWindowVisible, LoadImageW, PeekMessageW, PostMessageW, PostQuitMessage,
    RegisterClassExW, RegisterWindowMessageW, SetForegroundWindow, SetMenuInfo, SetMenuItemInfoW, SetWindowPos,
    ShowWindow, SystemParametersInfoW, TrackPopupMenu, TranslateMessage, ASFW_ANY, EVENT_SYSTEM_FOREGROUND, IMAGE_ICON,
    LR_DEFAULTCOLOR, MENUINFO, MENUITEMINFOW, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MIIM_BITMAP, MIM_STYLE,
    MNS_CHECKORBMP, MSG, PM_REMOVE, SM_CXSMICON, SM_CYSMICON, SPI_GETSELECTIONFADE, SPI_SETSELECTIONFADE,
    SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_HOTKEY, WM_LBUTTONUP,
    WM_NULL, WM_RBUTTONUP, WM_TIMER, WM_USER, WNDCLASSEXW,
};

const TRAY_MSG: u32 = WM_APP + 1;
const OPEN_SETTINGS_MSG: u32 = WM_APP + 2;
/// Posted by the update checker thread when a new version is out (see `UPDATE_AVAILABLE`).
const UPDATE_FOUND_MSG: u32 = WM_APP + 4;
/// Posted when a button of the capture toast is pressed: `wParam` is the action, `lParam` the
/// toast it came from (see `CAPTURE_TOAST`).
const TOAST_MSG: u32 = WM_APP + 5;
/// Tray notification: the user clicked the balloon.
const NIN_BALLOONUSERCLICK: u32 = WM_USER + 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Region = 1,
    Fullscreen,
    ActiveWindow,
    Scrolling,
    Picker,
    AllScreens,
    OpenFile,
    OpenClipboard,
    OpenFolder,
    Settings,
    Exit,
    Update,
    /// Opens Settings on Updates, scrolled to What's new.
    WhatsNew,
    /// Opens the screenshot of the current notification in the editor (not a menu item).
    EditCapture,
    /// Saves the screenshot of the current notification to the output folder (not a menu item).
    SaveCapture,
    /// Asks where to save the screenshot of the current notification (not a menu item).
    SaveCaptureAs,
    /// Copies the text you point at (the overlay in text mode).
    Text,
    /// Opens the capture of the "text copied" notification in the editor, with its text
    /// selection tool (not a menu item).
    EditCaptureText,
    /// Opens the language settings of Windows, to add text recognition (not a menu item).
    OcrLanguages,
    /// A screenshot picked in the History submenu (which one: `HISTORY_PICK`).
    HistoryItem,
}

/// Menu ids of the History submenu: `HISTORY_BASE + index`.
const HISTORY_BASE: usize = 1000;

impl Action {
    fn from_id(id: usize) -> Option<Action> {
        use Action::*;
        [
            Region,
            Fullscreen,
            ActiveWindow,
            Scrolling,
            Picker,
            AllScreens,
            OpenFile,
            OpenClipboard,
            OpenFolder,
            Settings,
            Exit,
            Update,
            WhatsNew,
            Text,
        ]
        .into_iter()
        .find(|a| *a as usize == id)
    }
}

thread_local! {
    static CONFIG: RefCell<Config> = RefCell::new(Config::load());
    static BUSY: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    static RELOAD: Cell<u32> = const { Cell::new(0) };
    static CAPTURE_REQUEST: Cell<u32> = const { Cell::new(0) };
    /// The last app window that was in the foreground (not the taskbar, tray or Blink).
    static LAST_APP_WINDOW: Cell<isize> = const { Cell::new(0) };
    /// What clicking the current balloon does.
    static BALLOON_CLICK: Cell<Option<Action>> = const { Cell::new(None) };
    /// The screenshot the current balloon is about (a raw temp file), for `EditCapture`.
    static BALLOON_IMAGE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// The toast with the Edit / Save buttons that is on screen (or in the notification
    /// center), with its number. Presses of any other toast are stale and ignored.
    static CAPTURE_TOAST: RefCell<Option<(u32, ToastNotification)>> = const { RefCell::new(None) };
    static TOAST_COUNTER: Cell<u32> = const { Cell::new(0) };
    /// The history entry chosen in the menu, for `Action::HistoryItem`.
    static HISTORY_PICK: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Newer version found by the background check (shared with that thread).
static UPDATE_AVAILABLE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn update_available() -> Option<String> {
    UPDATE_AVAILABLE.lock().ok().and_then(|v| v.clone())
}

/// Checks GitHub shortly after start and then once a day, in the background.
fn start_update_checks(hwnd: HWND) {
    let target = hwnd as isize;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(45));
        loop {
            let c = Config::load();
            if c.check_updates {
                if let Ok(crate::update::Check::Available(r)) = crate::update::check() {
                    let v = r.version.to_string();
                    let new =
                        UPDATE_AVAILABLE.lock().map(|mut a| a.replace(v.clone()) != Some(v.clone())).unwrap_or(false);
                    if new && v != c.skipped_version {
                        unsafe {
                            PostMessageW(target as HWND, UPDATE_FOUND_MSG, 0, 0);
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(24 * 3600));
        }
    });
}

/// Removes expired History screenshots now (and those beyond the limits), in the background.
fn prune_history() {
    let limits = crate::history::Limits::from_config(&cfg());
    std::thread::spawn(move || crate::history::prune(limits));
}

/// Removes expired History screenshots at start and every ten minutes after, so they are
/// gone from the disk on time, not only when the menu opens.
fn start_history_cleanup() {
    std::thread::spawn(|| loop {
        crate::history::prune(crate::history::Limits::from_config(&Config::load()));
        std::thread::sleep(Duration::from_secs(600));
    });
}

fn cfg() -> Config {
    CONFIG.with(|c| c.borrow().clone())
}

pub fn run_resident() -> Result<(), String> {
    unsafe {
        let mutex_name = wide("Local\\BlinkScreenshotResident");
        let mutex = CreateMutexW(std::ptr::null(), 1, mutex_name.as_ptr());
        if mutex.is_null() || GetLastError() == 183 {
            // Already running: open its settings instead of starting twice.
            let class = wide(RESIDENT_CLASS);
            let other = FindWindowW(class.as_ptr(), std::ptr::null());
            if !other.is_null() {
                AllowSetForegroundWindow(ASFW_ANY);
                PostMessageW(other, OPEN_SETTINGS_MSG, 0, 0);
            }
            return Ok(());
        }

        crate::imageio::remove_stale_raw_files();
        // Read the overlay's fonts now, so the first capture opens as fast as the others.
        std::thread::spawn(|| {
            crate::capture_bar::fonts();
        });
        let first_run = !config_path().exists();
        if first_run {
            // The installer decides about autostart; a portable copy starts without it.
            let c = Config { start_with_windows: super::autostart_enabled(), ..Config::default() };
            c.save().ok();
            CONFIG.with(|cell| *cell.borrow_mut() = c);
        }
        if cfg().start_with_windows {
            super::set_autostart(true).ok(); // keeps the path right if the exe was moved
        }

        let hinst = GetModuleHandleW(std::ptr::null());
        let class = wide(RESIDENT_CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);
        TASKBAR_CREATED.with(|c| c.set(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr())));
        RELOAD.with(|c| c.set(RegisterWindowMessageW(wide(RELOAD_MESSAGE).as_ptr())));
        CAPTURE_REQUEST.with(|c| c.set(RegisterWindowMessageW(wide(CAPTURE_MESSAGE).as_ptr())));
        // A hidden top-level window (not message-only: it must receive TaskbarCreated).
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return Err(t("Could not create the Blink tray window.").into());
        }
        add_tray_icon(hwnd);
        toast::init();
        allow_dark_menus();
        // Remember the app window the user worked in last: clicking the tray icon makes the
        // taskbar (and then Blink) the foreground window, so "active window" needs this.
        if let Some(w) = foreground_app_window() {
            LAST_APP_WINDOW.with(|c| c.set(w as isize));
        }
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            std::ptr::null_mut(),
            Some(on_foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        // The first start opens the welcome window (hotkey, Print Screen fix, a test capture)
        // instead of the "Blink is running" balloon.
        register_hotkeys(hwnd, true);
        if first_run {
            spawn_self(&["--welcome"]);
        }
        let version = crate::update::CURRENT;
        if cfg().last_version != version {
            // An earlier version ran before (0.1.0 did not record it): say it was updated.
            let updated = !first_run;
            if updated && BALLOON_CLICK.with(|c| c.get()).is_none() {
                balloon(hwnd, &tf("Blink updated to {0}", &[&version]), t("Click here to see what is new."), false);
                BALLOON_CLICK.with(|c| c.set(Some(Action::WhatsNew)));
            }
            CONFIG.with(|c| {
                let mut c = c.borrow_mut();
                c.last_version = version.to_string();
                c.updated_at = if updated { now_secs() } else { 0 };
                c.save().ok();
            });
        }
        start_update_checks(hwnd);
        start_history_cleanup();

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        remove_tray_icon(hwnd);
        discard_balloon_image();
        hide_capture_toast();
    }
    Ok(())
}

// ---- tray --------------------------------------------------------------------------------

fn copy_wstr(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

unsafe fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid
}

unsafe fn add_tray_icon(hwnd: HWND) {
    let mut nid = tray_data(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = TRAY_MSG;
    // Icon resource 2 is the tray glyph (one eye inside four corners, no tile, see tools/make_icon.py);
    // resource 1 is the app icon with the tile, used by Explorer, the taskbar and the installer.
    nid.hIcon = LoadImageW(
        GetModuleHandleW(std::ptr::null()),
        2 as _,
        IMAGE_ICON,
        GetSystemMetrics(SM_CXSMICON),
        GetSystemMetrics(SM_CYSMICON),
        LR_DEFAULTCOLOR,
    ) as _;
    copy_wstr(&mut nid.szTip, t("Blink screenshot"));
    Shell_NotifyIconW(NIM_ADD, &nid);
    // Version 3 behavior: the shell reports clicks on balloons (NIN_BALLOONUSERCLICK).
    // Mouse clicks still arrive as WM_RBUTTONUP / WM_LBUTTONUP.
    nid.Anonymous.uVersion = 3;
    Shell_NotifyIconW(NIM_SETVERSION, &nid);
}

unsafe fn remove_tray_icon(hwnd: HWND) {
    let nid = tray_data(hwnd);
    Shell_NotifyIconW(NIM_DELETE, &nid);
}

/// Shows a notification. Only warnings get an icon; the others are just the text.
fn balloon(hwnd: HWND, title: &str, text: &str, warning: bool) {
    BALLOON_CLICK.with(|c| c.set(None));
    discard_balloon_image();
    hide_capture_toast();
    unsafe {
        let mut nid = tray_data(hwnd);
        nid.uFlags = NIF_INFO;
        copy_wstr(&mut nid.szInfoTitle, title);
        copy_wstr(&mut nid.szInfo, text);
        nid.dwInfoFlags = if warning { NIIF_WARNING } else { NIIF_NONE };
        Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Removes Blink's notification from the screen (an empty text does that).
fn hide_balloon(hwnd: HWND) {
    BALLOON_CLICK.with(|c| c.set(None));
    discard_balloon_image();
    hide_capture_toast();
    unsafe {
        let mut nid = tray_data(hwnd);
        nid.uFlags = NIF_INFO;
        Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Shows the "screenshot taken" notification as a toast, which can have buttons: Edit opens
/// the screenshot in the editor, Save saves it to the output folder (left out when it was
/// saved already), Save as asks where. Clicking the toast itself opens the editor. False if
/// Windows refused it.
fn show_capture_toast(hwnd: HWND, title: &str, text: &str, saved: bool) -> bool {
    let edit = Action::EditCapture as u32;
    let mut buttons = vec![Button { label: t("Edit"), action: edit }];
    if !saved {
        buttons.push(Button { label: t("Save"), action: Action::SaveCapture as u32 });
    }
    buttons.push(Button { label: t("Save as"), action: Action::SaveCaptureAs as u32 });
    show_toast(hwnd, title, text, edit, &buttons)
}

/// A notification about the capture in `BALLOON_IMAGE`: clicking it does `click`. False if
/// Windows refused it.
fn show_toast(hwnd: HWND, title: &str, text: &str, click: u32, buttons: &[Button]) -> bool {
    BALLOON_CLICK.with(|c| c.set(None));
    discard_balloon_image();
    hide_capture_toast();
    let edit = click;
    let token = TOAST_COUNTER.with(|c| {
        c.set(c.get().wrapping_add(1));
        c.get()
    });
    match toast::show(hwnd, TOAST_MSG, token, title, text, edit, buttons) {
        Ok(t) => {
            CAPTURE_TOAST.with(|c| *c.borrow_mut() = Some((token, t)));
            true
        }
        Err(_) => false,
    }
}

fn hide_capture_toast() {
    if let Some((_, t)) = CAPTURE_TOAST.with(|c| c.borrow_mut().take()) {
        toast::hide(&t);
    }
}

fn discard_balloon_image() {
    if let Some(path) = BALLOON_IMAGE.with(|c| c.borrow_mut().take()) {
        std::fs::remove_file(path).ok();
    }
}

// ---- hotkeys -----------------------------------------------------------------------------

fn bindings(c: &Config) -> [(Action, Hotkey, &'static str, &'static str); 6] {
    // In the order of `Hotkeys::list`.
    let actions =
        [Action::Region, Action::Fullscreen, Action::ActiveWindow, Action::Scrolling, Action::Text, Action::Picker];
    let list = c.hotkeys.list();
    std::array::from_fn(|i| (actions[i], list[i].2, list[i].1, list[i].0))
}

/// Registers all hotkeys, records per-hotkey results for the settings window, and warns
/// about conflicts. `startup`: show the conflict balloon (on reloads the settings window,
/// which caused the reload, shows the status itself).
unsafe fn register_hotkeys(hwnd: HWND, startup: bool) {
    let c = cfg();
    let mut failed = Vec::new();
    let mut forced = Vec::new();
    let mut status = HotkeyStatus::default();
    for (action, hk, name, id) in bindings(&c) {
        UnregisterHotKey(hwnd, action as i32);
        let state = if !hk.is_enabled() {
            HotkeyState::Disabled
        } else if hk.force {
            forced.push((action as usize, hk));
            HotkeyState::Overriding
        } else {
            let mut mods = MOD_NOREPEAT;
            if hk.ctrl {
                mods |= MOD_CONTROL;
            }
            if hk.alt {
                mods |= MOD_ALT;
            }
            if hk.shift {
                mods |= MOD_SHIFT;
            }
            if hk.win {
                mods |= MOD_WIN;
            }
            if RegisterHotKey(hwnd, action as i32, mods, hk.vk) != 0 {
                HotkeyState::Active
            } else {
                failed.push(format!("{} ({name})", hk.label()));
                HotkeyState::InUse
            }
        };
        status.entries.push(HotkeyStatusEntry { id: id.to_string(), hotkey: hk, state });
    }
    kbhook::set(hwnd, forced);
    status.save();
    if !failed.is_empty() && startup {
        balloon(
            hwnd,
            t("Some Blink hotkeys are in use"),
            &tf(
                "Taken by another app or by Windows: {0}. Click here to see them in Settings and override them.",
                &[&failed.join(", ")],
            ),
            true,
        );
        BALLOON_CLICK.with(|c| c.set(Some(Action::Settings)));
    }
}

// ---- foreground tracking -----------------------------------------------------------------

unsafe extern "system" fn on_foreground(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if id_object == 0 {
        if let Some(w) = app_window(hwnd) {
            LAST_APP_WINDOW.with(|c| c.set(w as isize));
        }
    }
}

/// The window "Active window" means: the foreground app window, or (when the tray icon or
/// the capture menu took the foreground) the last one before that.
fn target_app_window() -> Option<HWND> {
    foreground_app_window().or_else(|| app_window(LAST_APP_WINDOW.with(|c| c.get()) as HWND))
}

// ---- after a menu ---------------------------------------------------------------------------

unsafe extern "system" fn find_transient(hwnd: HWND, lp: LPARAM) -> BOOL {
    if IsWindowVisible(hwnd) != 0 {
        match class_name(hwnd).as_str() {
            "SysFader" => {
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, &mut pid);
                if pid == GetCurrentProcessId() {
                    // Our menu's fade-out: it only advances while this thread handles
                    // messages, which it does not while capturing. Remove it right away,
                    // without the DWM hide animation (that would leave a fading ghost), and
                    // off the screen first in case the hide itself is still animated.
                    let off: BOOL = 1;
                    DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_TRANSITIONS_FORCEDISABLED as u32,
                        &off as *const BOOL as *const c_void,
                        std::mem::size_of::<BOOL>() as u32,
                    );
                    SetWindowPos(
                        hwnd,
                        std::ptr::null_mut(),
                        -32000,
                        -32000,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    ShowWindow(hwnd, SW_HIDE);
                } else {
                    *(lp as *mut bool) = true;
                }
            }
            "#32768" => *(lp as *mut bool) = true,
            _ => {}
        }
    }
    1
}

/// Waits until the closed menu is really gone from the screen. Windows fades the clicked
/// item out in a separate "SysFader" window, which otherwise ends up half-transparent in
/// the screenshot. Our menus normally have that fade turned off (`NoSelectionFade`); this
/// is the fallback for when that did not work. The fade is driven by timers of this
/// thread, and the fader may only appear on the first timer tick, so for a minimum time
/// timers keep being handled and every fader of ours that shows up is removed. Also
/// closes the tray overflow flyout (the "^" area) if the menu was opened from there, so it
/// is not captured either.
fn wait_for_menu_to_vanish() {
    const MIN_WAIT: Duration = Duration::from_millis(60);
    const MAX_WAIT: Duration = Duration::from_millis(900);
    let start = Instant::now();
    let mut hid_flyout = false;
    loop {
        let mut busy = false;
        unsafe {
            EnumWindows(Some(find_transient), &mut busy as *mut bool as LPARAM);
            for class in ["NotifyIconOverflowWindow", "TopLevelWindowForOverflowXamlIsland"] {
                let w = FindWindowW(wide(class).as_ptr(), std::ptr::null());
                if !w.is_null() && IsWindowVisible(w) != 0 && !is_cloaked(w) {
                    ShowWindow(w, SW_HIDE);
                    hid_flyout = true;
                }
            }
        }
        let waited = start.elapsed();
        if (!busy && waited >= MIN_WAIT) || waited > MAX_WAIT {
            break;
        }
        unsafe { pump_timers() };
        std::thread::sleep(Duration::from_millis(15));
    }
    // Give the compositor a few frames to show the screen without them.
    std::thread::sleep(Duration::from_millis(if hid_flyout { 150 } else { 50 }));
    unsafe {
        DwmFlush();
    }
}

/// Handles pending timer messages only (WM_TIMER and the system timers that drive menu
/// animations), without running anything else re-entrantly.
unsafe fn pump_timers() {
    const WM_SYSTIMER: u32 = 0x118;
    let mut msg: MSG = std::mem::zeroed();
    for kind in [WM_TIMER, WM_SYSTIMER] {
        while PeekMessageW(&mut msg, std::ptr::null_mut(), kind, kind, PM_REMOVE) != 0 {
            DispatchMessageW(&msg);
        }
    }
}

// ---- menus -------------------------------------------------------------------------------

/// Lets popup menus follow the Windows dark/light app theme (Windows 10 1809+). Uses the
/// same uxtheme entry points as Explorer and Notepad++; skipped on older builds.
fn allow_dark_menus() {
    unsafe {
        let build =
            reg_string(HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", "CurrentBuildNumber")
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(0);
        if build < 17763 {
            return;
        }
        let ux = LoadLibraryW(wide("uxtheme.dll").as_ptr());
        if ux.is_null() {
            return;
        }
        // #135: SetPreferredAppMode(AllowDark) (1809: AllowDarkModeForApp(TRUE)), #136: FlushMenuThemes.
        if let Some(f) = GetProcAddress(ux, 135 as _) {
            let f: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            f(1);
        }
        if let Some(f) = GetProcAddress(ux, 136 as _) {
            let f: unsafe extern "system" fn() = std::mem::transmute(f);
            f();
        }
        DARK_MENUS_SUPPORTED.with(|c| c.set(true));
    }
}

thread_local! {
    static DARK_MENUS_SUPPORTED: Cell<bool> = const { Cell::new(false) };
}

unsafe fn reg_string(root: windows_sys::Win32::System::Registry::HKEY, key: &str, name: &str) -> Option<String> {
    let (k, n) = (wide(key), wide(name));
    let mut buf = [0u16; 64];
    let mut len = (buf.len() * 2) as u32;
    let r = RegGetValueW(
        root,
        k.as_ptr(),
        n.as_ptr(),
        RRF_RT_REG_SZ,
        std::ptr::null_mut(),
        buf.as_mut_ptr().cast(),
        &mut len,
    );
    (r == 0).then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
}

fn menus_are_dark() -> bool {
    if !DARK_MENUS_SUPPORTED.with(|c| c.get()) {
        return false;
    }
    let (k, n) = (wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"), wide("AppsUseLightTheme"));
    let mut v: u32 = 1;
    let mut len = 4u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            n.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut v as *mut u32).cast(),
            &mut len,
        );
    }
    v == 0
}

// ---- menus -------------------------------------------------------------------------------

fn menu_label(text: &str, hk: Hotkey) -> String {
    if hk.is_enabled() {
        format!("{text}\t{}", hk.label())
    } else {
        text.to_string()
    }
}

/// A popup menu with icons, shown at the mouse pointer.
struct PopupMenu {
    menu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU,
    bitmaps: Vec<windows_sys::Win32::Graphics::Gdi::HBITMAP>,
    icon_px: u32,
    color: Color32,
}

impl PopupMenu {
    unsafe fn new() -> Self {
        let menu = CreatePopupMenu();
        // Icons share the column with check marks instead of adding another one.
        let mut info: MENUINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MENUINFO>() as u32;
        info.fMask = MIM_STYLE;
        info.dwStyle = MNS_CHECKORBMP;
        SetMenuInfo(menu, &info);
        let scale = monitor_at(cursor_pos()).map_or(1.0, |m| m.2);
        let color = if menus_are_dark() { Color32::from_rgb(232, 232, 238) } else { Color32::from_rgb(48, 48, 56) };
        Self { menu, bitmaps: Vec::new(), icon_px: (16.0 * scale).round() as u32, color }
    }

    unsafe fn add(&mut self, id: usize, icon: Icon, text: &str) {
        let w = wide(text);
        AppendMenuW(self.menu, MF_STRING, id, w.as_ptr());
        let bmp = tray_icons::bitmap(icon, self.icon_px, self.color);
        if !bmp.is_null() {
            let mut mii: MENUITEMINFOW = std::mem::zeroed();
            mii.cbSize = std::mem::size_of::<MENUITEMINFOW>() as u32;
            mii.fMask = MIIM_BITMAP;
            mii.hbmpItem = bmp;
            SetMenuItemInfoW(self.menu, id as u32, 0, &mii);
            self.bitmaps.push(bmp);
        }
    }

    /// An item with a picture instead of an icon (the History previews).
    unsafe fn add_bitmap(&mut self, id: usize, bmp: windows_sys::Win32::Graphics::Gdi::HBITMAP, text: &str) {
        let w = wide(text);
        AppendMenuW(self.menu, MF_STRING, id, w.as_ptr());
        if !bmp.is_null() {
            let mut mii: MENUITEMINFOW = std::mem::zeroed();
            mii.cbSize = std::mem::size_of::<MENUITEMINFOW>() as u32;
            mii.fMask = MIIM_BITMAP;
            mii.hbmpItem = bmp;
            SetMenuItemInfoW(self.menu, id as u32, 0, &mii);
            self.bitmaps.push(bmp);
        }
    }

    /// Adds `sub` as a submenu. Its bitmaps are freed with this menu.
    unsafe fn submenu(&mut self, icon: Icon, text: &str, sub: PopupMenu) {
        let w = wide(text);
        let pos = GetMenuItemCount(self.menu);
        AppendMenuW(self.menu, MF_POPUP, sub.menu as usize, w.as_ptr());
        self.bitmaps.extend(sub.bitmaps);
        let bmp = tray_icons::bitmap(icon, self.icon_px, self.color);
        if !bmp.is_null() && pos >= 0 {
            let mut mii: MENUITEMINFOW = std::mem::zeroed();
            mii.cbSize = std::mem::size_of::<MENUITEMINFOW>() as u32;
            mii.fMask = MIIM_BITMAP;
            mii.hbmpItem = bmp;
            SetMenuItemInfoW(self.menu, pos as u32, 1, &mii);
            self.bitmaps.push(bmp);
        }
    }

    unsafe fn title(&mut self, text: &str) {
        let w = wide(text);
        AppendMenuW(self.menu, MF_STRING | MF_GRAYED, 0, w.as_ptr());
    }

    unsafe fn separator(&mut self) {
        AppendMenuW(self.menu, MF_SEPARATOR, 0, std::ptr::null());
    }

    /// Shows the menu at the mouse pointer; returns the chosen id (0 = dismissed).
    unsafe fn track(self, hwnd: HWND) -> usize {
        let pt = cursor_pos();
        SetForegroundWindow(hwnd);
        let _no_fade = NoSelectionFade::new();
        let cmd = TrackPopupMenu(
            self.menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            0,
            hwnd,
            std::ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(self.menu);
        for b in self.bitmaps {
            DeleteObject(b);
        }
        cmd as usize
    }
}

/// Turns off the Windows selection fade while one of our menus is open. With it on, the
/// clicked item stays on screen after the menu closes and fades out over a few hundred ms in a
/// "SysFader" window, right when the screenshot is taken. The setting is changed for this
/// session only (not written to the user profile, no broadcast) and restored as soon as the
/// menu returns. The fade is decided inside `TrackPopupMenu`, and no other menu can be open
/// at the same time, so other apps are not affected in practice.
struct NoSelectionFade(bool);

impl NoSelectionFade {
    unsafe fn new() -> Self {
        let mut on: BOOL = 0;
        let was_on =
            SystemParametersInfoW(SPI_GETSELECTIONFADE, 0, &mut on as *mut BOOL as *mut c_void, 0) != 0 && on != 0;
        // For this setting the new value is passed as the pointer itself (FALSE = null).
        Self(was_on && SystemParametersInfoW(SPI_SETSELECTIONFADE, 0, std::ptr::null_mut(), 0) != 0)
    }
}

impl Drop for NoSelectionFade {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                SystemParametersInfoW(SPI_SETSELECTIONFADE, 0, 1 as *mut c_void, 0);
            }
        }
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Whether Blink was updated to this version less than a day ago.
fn recently_updated(c: &Config) -> bool {
    let age = now_secs() - c.updated_at;
    c.updated_at > 0 && c.last_version == crate::update::CURRENT && (0..24 * 60 * 60).contains(&age)
}

unsafe fn show_menu(hwnd: HWND, full: bool) -> Option<Action> {
    let c = cfg();
    let mut m = PopupMenu::new();
    if full {
        if recently_updated(&c) {
            let text = tf("Blink {0} - See what's new", &[&crate::update::CURRENT]);
            m.add(Action::WhatsNew as usize, Icon::WhatsNew, &text);
        } else {
            m.title(&format!("Blink {}", crate::update::CURRENT));
        }
        m.separator();
        if let Some(v) = update_available() {
            m.add(Action::Update as usize, Icon::Update, &tf("&Update to Blink {0}...", &[&v]));
            m.separator();
        }
    }
    m.add(Action::Region as usize, Icon::Region, &menu_label(t("&Region or window"), c.hotkeys.region));
    m.add(Action::Fullscreen as usize, Icon::Fullscreen, &menu_label(t("&Full screen"), c.hotkeys.fullscreen));
    m.add(Action::AllScreens as usize, Icon::AllScreens, t("&All screens"));
    m.add(Action::ActiveWindow as usize, Icon::Window, &menu_label(t("Active &window"), c.hotkeys.active_window));
    m.add(Action::Scrolling as usize, Icon::Scrolling, &menu_label(t("&Scrolling page"), c.hotkeys.scrolling));
    m.add(Action::Text as usize, Icon::Text, &menu_label(t("Copy &text from the screen"), c.hotkeys.text));
    m.separator();
    let mut ids = Vec::new();
    if let Some((sub, list)) = if full && c.history_size > 0 { history_menu(&m) } else { None } {
        m.submenu(Icon::History, t("&History"), sub);
        ids = list;
    }
    m.add(Action::OpenFile as usize, Icon::OpenFile, t("&Open image from file..."));
    m.add(Action::OpenClipboard as usize, Icon::Clipboard, t("Open image from &clipboard"));
    if full {
        m.separator();
        m.add(Action::OpenFolder as usize, Icon::Folder, t("Open screenshots &folder"));
        m.add(Action::Settings as usize, Icon::Settings, t("S&ettings"));
        if update_available().is_none() {
            m.add(Action::Update as usize, Icon::Update, t("Check for &updates..."));
        }
        m.separator();
        m.add(Action::Exit as usize, Icon::Exit, t("E&xit"));
    }
    let cmd = m.track(hwnd);
    if let Some(id) = cmd.checked_sub(HISTORY_BASE).and_then(|i| ids.get(i)) {
        HISTORY_PICK.with(|p| p.set(Some(*id)));
        return Some(Action::HistoryItem);
    }
    Action::from_id(cmd)
}

/// "Today 14:32", "Yesterday 09:05" or "2026-11-19 16:40".
fn history_label(e: &crate::history::Entry) -> String {
    let Some(time) = e.time() else { return String::new() };
    let today = chrono::Local::now().date_naive();
    let clock = time.format("%H:%M").to_string();
    let when = if time.date_naive() == today {
        tf("Today {0}", &[&clock])
    } else if today.pred_opt() == Some(time.date_naive()) {
        tf("Yesterday {0}", &[&clock])
    } else {
        time.format("%Y-%m-%d %H:%M").to_string()
    };
    format!("{when}\t{} \u{d7} {}", e.width, e.height)
}

/// The History submenu with a preview of every screenshot, and the entry ids in menu order.
/// Windows makes a menu that is taller than the screen scroll.
unsafe fn history_menu(parent: &PopupMenu) -> Option<(PopupMenu, Vec<u64>)> {
    let entries = crate::history::list(crate::history::Limits::from_config(&cfg()));
    let mut sub = PopupMenu::new();
    if entries.is_empty() {
        sub.title(t("No screenshots yet"));
        return Some((sub, Vec::new()));
    }
    let scale = parent.icon_px as f32 / 16.0;
    let (w, h) = ((64.0 * scale).round() as u32, (40.0 * scale).round() as u32);
    let mut ids = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let bmp = crate::history::thumbnail(e.id)
            .map_or(std::ptr::null_mut(), |img| tray_icons::thumbnail(&img, w, h, parent.color));
        sub.add_bitmap(HISTORY_BASE + i, bmp, &history_label(e));
        ids.push(e.id);
    }
    Some((sub, ids))
}

/// What to do with a screenshot ("Ask me what to do").
#[derive(Clone, Copy, PartialEq, Eq)]
enum AfterCapture {
    Edit = 1,
    Copy,
    Save,
}

/// Small menu at the mouse pointer to choose what happens with a new screenshot.
/// `None`: dismissed (Esc or a click elsewhere), the screenshot is discarded.
unsafe fn ask_after_capture(hwnd: HWND, img: &RgbaImage) -> Option<AfterCapture> {
    let mut m = PopupMenu::new();
    m.title(&tf("Screenshot {0} \u{d7} {1}", &[&img.width(), &img.height()]));
    m.separator();
    m.add(AfterCapture::Edit as usize, Icon::Edit, t("Open in &editor"));
    m.add(AfterCapture::Copy as usize, Icon::Clipboard, t("&Copy to clipboard"));
    m.add(AfterCapture::Save as usize, Icon::Save, t("&Save to screenshots folder"));
    match m.track(hwnd) {
        1 => Some(AfterCapture::Edit),
        2 => Some(AfterCapture::Copy),
        3 => Some(AfterCapture::Save),
        _ => None,
    }
}

// ---- actions -----------------------------------------------------------------------------

fn spawn_self(args: &[&str]) {
    if let Ok(exe) = std::env::current_exe() {
        unsafe {
            AllowSetForegroundWindow(ASFW_ANY);
        }
        let _ = std::process::Command::new(exe).args(args).spawn();
    }
}

fn monitor_under_cursor() -> IRect {
    monitor_at(cursor_pos()).map(|(m, _, _)| IRect::from_rect(m)).unwrap_or_else(virtual_screen)
}

fn capture(hwnd: HWND, action: Action, menu_was_open: bool) {
    if BUSY.with(|b| b.replace(true)) {
        return;
    }
    if menu_was_open {
        wait_for_menu_to_vanish();
    }
    let c = cfg();
    // The pointer where it is now. From a menu it would only show where that menu was.
    let pointer = if c.capture_pointer && !menu_was_open { grab_pointer() } else { None };
    // The screen rectangle the screenshot shows, to place the pointer in it.
    let mut shown: Option<IRect> = None;
    let img: Option<RgbaImage> = match action {
        Action::Region | Action::Scrolling | Action::Text => {
            let windows = top_level_windows();
            let mode = match action {
                Action::Scrolling => Mode::Scroll,
                Action::Text => Mode::Text,
                _ => Mode::Region,
            };
            match grab(virtual_screen()) {
                Some(shot) => {
                    // Notifications stay above every normal window, the overlay included, and
                    // would take the clicks there. Ours is in the frozen screenshot already,
                    // so it can go now. (Other apps' notifications cannot be removed.)
                    hide_balloon(hwnd);
                    match overlay::select(&shot, windows, mode, c.show_magnifier) {
                        Some(out) if out.qr.is_some() => {
                            drop(shot);
                            if let Some((content, open)) = out.qr {
                                deliver_qr(hwnd, &content, open);
                            }
                            None
                        }
                        Some(out) if out.text.is_some() => {
                            let img = shot.crop_rgba(out.rect);
                            drop(shot);
                            deliver_text(hwnd, out.text.unwrap_or(Ok(String::new())), img);
                            None
                        }
                        Some(out) if out.scroll => {
                            drop(shot);
                            std::thread::sleep(Duration::from_millis(120));
                            scroll::capture(out.rect, &c)
                        }
                        Some(out) => {
                            shown = Some(out.rect.intersect(&shot.rect));
                            shot.crop_rgba(out.rect)
                        }
                        None => None,
                    }
                }
                None => None,
            }
        }
        Action::Fullscreen => {
            let r = monitor_under_cursor();
            shown = Some(r);
            grab_rgba(r)
        }
        Action::AllScreens => {
            shown = Some(virtual_screen());
            grab_rgba(virtual_screen())
        }
        Action::ActiveWindow => {
            let target = target_app_window();
            if let Some(w) = target {
                let fg = unsafe { GetForegroundWindow() };
                if app_window(fg) != Some(w) {
                    // Picked from the tray / capture menu: bring the window back to the front
                    // so nothing covers it, and give it a moment to repaint.
                    unsafe {
                        SetForegroundWindow(w);
                    }
                    std::thread::sleep(Duration::from_millis(220));
                }
            }
            let r = target.and_then(window_bounds).map(|r| r.intersect(&virtual_screen())).filter(|r| !r.is_empty());
            let r = r.unwrap_or_else(monitor_under_cursor);
            shown = Some(r);
            grab_rgba(r)
        }
        _ => None,
    };
    if let Some(img) = img {
        let pointer = pointer.zip(shown).and_then(|((p, x, y), r)| {
            let p = Pointer { img: p, x: x - r.x, y: y - r.y };
            p.overlaps(img.width(), img.height()).then_some(p)
        });
        deliver(hwnd, img, pointer, &c);
    }
    BUSY.with(|b| b.set(false));
}

fn deliver(hwnd: HWND, img: RgbaImage, pointer: Option<Pointer>, c: &Config) {
    if c.after_capture_ask {
        let Some(choice) = (unsafe { ask_after_capture(hwnd, &img) }) else { return };
        let chosen = Config {
            after_capture_ask: false,
            after_capture_open_editor: choice == AfterCapture::Edit,
            after_capture_copy: choice == AfterCapture::Copy,
            after_capture_save: choice == AfterCapture::Save,
            ..c.clone()
        };
        return deliver(hwnd, img, pointer, &chosen);
    }
    if c.history_size > 0 {
        // PNG takes a moment for a big screenshot: not on the thread that answers hotkeys.
        let (copy, ptr, limits) = (img.clone(), pointer.clone(), crate::history::Limits::from_config(c));
        std::thread::spawn(move || crate::history::add(&copy, ptr.as_ref(), limits).ok());
    }
    // Copy and Save get the pointer drawn in; the editor gets it as a separate object.
    let flat = pointer.as_ref().map(|p| {
        let mut f = img.clone();
        imageio::flatten(&mut f, p);
        f
    });
    let out = flat.as_ref().unwrap_or(&img);
    let mut lines = Vec::new();
    let mut warning = false;
    let mut copied = false;
    let mut saved = false;
    if c.after_capture_copy {
        match imageio::copy_to_clipboard(out) {
            Ok(()) => copied = true,
            Err(e) => {
                lines.push(tf("Copy failed: {0}", &[&e]));
                warning = true;
            }
        }
    }
    if c.after_capture_save {
        let path = c.next_output_path();
        match imageio::save_image(&path, out, c.format, c.jpg_quality) {
            Ok(()) => {
                lines.push(tf("Saved as {0}", &[&path.display()]));
                saved = true;
            }
            Err(e) => {
                lines.push(tf("Save failed: {0}", &[&e]));
                warning = true;
            }
        }
    }
    let path = imageio::temp_raw_path();
    let raw = imageio::write_capture(&path, &img, pointer.as_ref());
    let open = c.after_capture_open_editor || (!c.after_capture_copy && !c.after_capture_save);
    if open {
        match raw {
            Ok(()) => spawn_self(&["--edit-raw", &path.display().to_string()]),
            Err(e) => {
                std::fs::remove_file(&path).ok();
                balloon(hwnd, t("Screenshot taken"), &tf("Could not open the editor: {0}", &[&e]), true);
            }
        }
        return;
    }
    let title = match (copied, saved) {
        (true, true) => t("Screenshot copied and saved"),
        (true, false) => t("Screenshot copied to clipboard"),
        (false, true) => t("Screenshot saved"),
        (false, false) => t("Screenshot taken"),
    };
    if raw.is_err() {
        std::fs::remove_file(&path).ok();
        balloon(hwnd, title, &lines.join("\n"), warning);
        return;
    }
    if show_capture_toast(hwnd, title, &lines.join("\n"), saved) {
        BALLOON_IMAGE.with(|c| *c.borrow_mut() = Some(path));
        return;
    }
    // No toast possible: the balloon can only be clicked, which opens the editor.
    // First: the text is cut off at 255 characters, and a saved path can be long.
    lines.insert(0, t("Click to open it in the editor.").to_string());
    balloon(hwnd, title, &lines.join("\n"), warning);
    BALLOON_CLICK.with(|c| c.set(Some(Action::EditCapture)));
    BALLOON_IMAGE.with(|c| *c.borrow_mut() = Some(path));
}

/// Text mode: puts the text on the clipboard and says so with its first words. The capture
/// goes along with the notification, so "Edit" can open it to select the text exactly.
fn deliver_text(hwnd: HWND, text: Result<String, OcrError>, img: Option<RgbaImage>) {
    let raw = img.and_then(|img| {
        let path = imageio::temp_raw_path();
        match imageio::write_raw(&path, &img) {
            Ok(()) => Some(path),
            Err(_) => {
                std::fs::remove_file(&path).ok();
                None
            }
        }
    });
    let (title, body) = match text {
        Ok(text) if !text.trim().is_empty() => match imageio::copy_text_to_clipboard(&text) {
            Ok(()) => (t("Text copied"), ocr::preview(&text, 160)),
            Err(e) => {
                discard(raw);
                return balloon(hwnd, t("Text not copied"), &tf("Copy failed: {0}", &[&e]), true);
            }
        },
        Ok(_) => (t("No text found"), t("Blink could not read any text there.").to_string()),
        Err(e) => {
            discard(raw);
            balloon(hwnd, t("Text recognition is not available"), &e.message(), true);
            if e == OcrError::NoLanguage {
                BALLOON_CLICK.with(|c| c.set(Some(Action::OcrLanguages)));
            }
            return;
        }
    };
    let Some(path) = raw else {
        return balloon(hwnd, title, &body, false);
    };
    let edit = Action::EditCaptureText as u32;
    if show_toast(hwnd, title, &body, edit, &[Button { label: t("Open in editor"), action: edit }]) {
        BALLOON_IMAGE.with(|c| *c.borrow_mut() = Some(path));
        return;
    }
    balloon(hwnd, title, &format!("{}\n{body}", t("Click to open it in the editor.")), false);
    BALLOON_CLICK.with(|c| c.set(Some(Action::EditCaptureText)));
    BALLOON_IMAGE.with(|c| *c.borrow_mut() = Some(path));
}

/// A click on a QR code in the overlay: copies what it says, or opens its link (Ctrl+click;
/// only http and https links get that far).
fn deliver_qr(hwnd: HWND, content: &str, open: bool) {
    if open && crate::qr::is_web_link(content) {
        super::open_url(content);
        return;
    }
    match imageio::copy_text_to_clipboard(content) {
        Ok(()) => {
            let title = if crate::qr::is_web_link(content) { t("Link copied") } else { t("QR code copied") };
            balloon(hwnd, title, &ocr::preview(content, 160), false);
        }
        Err(e) => balloon(hwnd, t("QR code not copied"), &tf("Copy failed: {0}", &[&e]), true),
    }
}

fn discard(raw: Option<PathBuf>) {
    if let Some(p) = raw {
        std::fs::remove_file(p).ok();
    }
}

/// A screenshot picked in History: copied again, with the same notification as a new
/// capture (Edit, Save, Save as). It is not added to the history a second time.
fn from_history(hwnd: HWND, id: u64) {
    let (img, pointer) = match crate::history::load(id) {
        Ok(v) => v,
        Err(e) => return balloon(hwnd, t("Screenshot not found"), &e, true),
    };
    let mut flat = img.clone();
    if let Some(p) = &pointer {
        imageio::flatten(&mut flat, p);
    }
    let copied = imageio::copy_to_clipboard(&flat);
    drop(flat);
    let path = imageio::temp_raw_path();
    let (title, body) = match &copied {
        Ok(()) => (t("Screenshot copied to clipboard"), t("From your history.").to_string()),
        Err(e) => (t("Screenshot taken"), tf("Copy failed: {0}", &[e])),
    };
    if imageio::write_capture(&path, &img, pointer.as_ref()).is_err() {
        std::fs::remove_file(&path).ok();
        return balloon(hwnd, title, &body, copied.is_err());
    }
    if show_capture_toast(hwnd, title, &body, false) {
        BALLOON_IMAGE.with(|c| *c.borrow_mut() = Some(path));
        return;
    }
    balloon(hwnd, title, &format!("{}\n{body}", t("Click to open it in the editor.")), copied.is_err());
    BALLOON_CLICK.with(|c| c.set(Some(Action::EditCapture)));
    BALLOON_IMAGE.with(|c| *c.borrow_mut() = Some(path));
}

/// The "Save" button of the capture notification: saves that screenshot like "save after
/// capture" does. If that fails the screenshot opens in the editor, so it is not lost.
fn save_capture(hwnd: HWND) {
    // Taken first: showing a notification discards the screenshot that is still in there.
    let Some(path) = BALLOON_IMAGE.with(|c| c.borrow_mut().take()) else { return };
    let c = cfg();
    let result = imageio::read_capture_flat(&path).map_err(|e| e.to_string()).and_then(|img| {
        let out = c.next_output_path();
        imageio::save_image(&out, &img, c.format, c.jpg_quality).map(|()| out)
    });
    match result {
        Ok(out) => {
            std::fs::remove_file(&path).ok();
            balloon(hwnd, t("Screenshot saved"), &tf("Saved as {0}", &[&out.display()]), false);
        }
        Err(e) => {
            spawn_self(&["--edit-raw", &path.display().to_string()]);
            balloon(hwnd, t("Screenshot not saved"), &tf("{0}\nIt is open in the editor instead.", &[&e]), true);
        }
    }
}

fn run_action(hwnd: HWND, action: Action, from_menu: bool) {
    match action {
        Action::Picker => {
            if let Some(a) = unsafe { show_menu(hwnd, false) } {
                run_action(hwnd, a, true);
            }
        }
        Action::Region
        | Action::Fullscreen
        | Action::ActiveWindow
        | Action::Scrolling
        | Action::AllScreens
        | Action::Text => capture(hwnd, action, from_menu),
        Action::EditCaptureText => {
            if let Some(path) = BALLOON_IMAGE.with(|c| c.borrow_mut().take()) {
                spawn_self(&["--edit-raw", &path.display().to_string(), "--select-text"]);
            }
            hide_capture_toast();
        }
        Action::OcrLanguages => super::open_language_settings(),
        Action::OpenFile => spawn_self(&["--open-dialog"]),
        Action::OpenClipboard => spawn_self(&["--clipboard"]),
        Action::OpenFolder => {
            let dir = cfg().output_dir;
            std::fs::create_dir_all(&dir).ok();
            super::open_folder(&dir);
        }
        Action::Settings => spawn_self(&["--settings"]),
        Action::WhatsNew => spawn_self(&["--settings", "whats-new"]),
        Action::Update => spawn_self(&["--update"]),
        Action::EditCapture => {
            // The editor deletes the file once it has read it.
            if let Some(path) = BALLOON_IMAGE.with(|c| c.borrow_mut().take()) {
                spawn_self(&["--edit-raw", &path.display().to_string()]);
            }
            hide_capture_toast();
        }
        Action::SaveCapture => save_capture(hwnd),
        Action::HistoryItem => {
            if let Some(id) = HISTORY_PICK.with(|p| p.take()) {
                from_history(hwnd, id);
            }
        }
        Action::SaveCaptureAs => {
            // A separate process shows the file dialog, so the tray keeps answering hotkeys.
            // It deletes the file when it is done with it.
            if let Some(path) = BALLOON_IMAGE.with(|c| c.borrow_mut().take()) {
                spawn_self(&["--save-raw-as", &path.display().to_string()]);
            }
            hide_capture_toast();
        }
        Action::Exit => unsafe {
            remove_tray_icon(hwnd);
            discard_balloon_image();
            hide_capture_toast();
            PostQuitMessage(0);
        },
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == TASKBAR_CREATED.with(|c| c.get()) {
        add_tray_icon(hwnd); // Explorer restarted
        return 0;
    }
    if msg == CAPTURE_REQUEST.with(|c| c.get()) {
        run_action(hwnd, Action::Region, false);
        return 0;
    }
    if msg == RELOAD.with(|c| c.get()) {
        CONFIG.with(|c| *c.borrow_mut() = Config::load());
        cfg().apply_globals();
        prune_history();
        register_hotkeys(hwnd, false);
        return 0;
    }
    match msg {
        WM_HOTKEY | FORCED_HOTKEY_MSG => {
            if let Some(a) = Action::from_id(wp) {
                run_action(hwnd, a, false);
            }
            0
        }
        TRAY_MSG => {
            let event = (lp & 0xFFFF) as u32;
            if event == NIN_BALLOONUSERCLICK {
                if let Some(a) = BALLOON_CLICK.with(|c| c.take()) {
                    run_action(hwnd, a, false);
                }
            } else if event == WM_RBUTTONUP || event == WM_LBUTTONUP || event == WM_CONTEXTMENU {
                if let Some(a) = show_menu(hwnd, true) {
                    run_action(hwnd, a, true);
                }
            }
            0
        }
        OPEN_SETTINGS_MSG => {
            run_action(hwnd, Action::Settings, false);
            0
        }
        TOAST_MSG => {
            let current = CAPTURE_TOAST.with(|t| t.borrow().as_ref().map(|(token, _)| *token));
            if current == Some(lp as u32) {
                match wp {
                    a if a == Action::EditCapture as usize => run_action(hwnd, Action::EditCapture, false),
                    a if a == Action::SaveCapture as usize => run_action(hwnd, Action::SaveCapture, false),
                    a if a == Action::SaveCaptureAs as usize => run_action(hwnd, Action::SaveCaptureAs, false),
                    a if a == Action::EditCaptureText as usize => run_action(hwnd, Action::EditCaptureText, false),
                    _ => {}
                }
            }
            0
        }
        UPDATE_FOUND_MSG => {
            if let Some(v) = update_available() {
                balloon(
                    hwnd,
                    &tf("Blink {0} is available", &[&v]),
                    t("Click here to see what is new and install it."),
                    false,
                );
                BALLOON_CLICK.with(|c| c.set(Some(Action::Update)));
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
