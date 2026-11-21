//! Mouse clicks and keys during a recording, from low-level hooks.
//!
//! The hooks run on their own thread with a message loop (Windows drops hooks that answer
//! slowly) and only pass what they see on to the recorder through a channel. The keyboard hook
//! is only installed when keys are shown at all, and the text a key types is only looked up in
//! "all keys" mode. What may be shown is decided by `rec::keys::filter` before anything is
//! written down.

use crate::rec::events::{Button, Mods};
use crate::rec::keys::{KeyMode, KeyPress};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::Instant;
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, GetKeyboardLayout, MapVirtualKeyExW, ToUnicodeEx, MAPVK_VK_TO_CHAR, VK_CAPITAL,
    VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
    MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// What the hooks saw.
#[derive(Clone, Debug)]
pub enum Input {
    Down { b: Button, x: i32, y: i32, mods: Mods, at: Instant },
    Up { b: Button, at: Instant },
    Key { key: KeyPress, at: Instant },
}

static SENDER: Mutex<Option<Sender<Input>>> = Mutex::new(None);
static KEYS: AtomicU8 = AtomicU8::new(0);
/// Keys that are down, so auto-repeat counts once.
static HELD: Mutex<[bool; 256]> = Mutex::new([false; 256]);
static RUNNING: AtomicBool = AtomicBool::new(false);

fn send(i: Input) {
    if let Ok(s) = SENDER.lock() {
        if let Some(tx) = s.as_ref() {
            let _ = tx.send(i);
        }
    }
}

fn down(vk: u16) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

fn mods() -> Mods {
    Mods { ctrl: down(VK_CONTROL), alt: down(VK_MENU), shift: down(VK_SHIFT), win: down(VK_LWIN) || down(VK_RWIN) }
}

/// The running hooks; dropping it removes them.
pub struct Hooks {
    thread: u32,
    handle: Option<std::thread::JoinHandle<()>>,
}

/// Starts the hooks: always the mouse, the keyboard only when `keys` shows any.
pub fn start(keys: KeyMode, tx: Sender<Input>) -> Hooks {
    *SENDER.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    KEYS.store(keys as u8, Ordering::SeqCst);
    *HELD.lock().unwrap_or_else(|e| e.into_inner()) = [false; 256];
    let (id_tx, id_rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || unsafe {
        let _ = id_tx.send(GetCurrentThreadId());
        RUNNING.store(true, Ordering::SeqCst);
        let module = GetModuleHandleW(std::ptr::null());
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
        let keyboard = if keys != KeyMode::Off {
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_proc), module, 0)
        } else {
            std::ptr::null_mut()
        };
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if !mouse.is_null() {
            UnhookWindowsHookEx(mouse);
        }
        if !keyboard.is_null() {
            UnhookWindowsHookEx(keyboard);
        }
        RUNNING.store(false, Ordering::SeqCst);
    });
    let thread = id_rx.recv().unwrap_or(0);
    Hooks { thread, handle: Some(handle) }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        // The hook thread may not have its message queue yet right after it started.
        for _ in 0..50 {
            if unsafe { PostThreadMessageW(self.thread, WM_QUIT, 0, 0) } != 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        *SENDER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let m = &*(lp as *const MSLLHOOKSTRUCT);
        let at = Instant::now();
        let (x, y) = (m.pt.x, m.pt.y);
        match wp as u32 {
            WM_LBUTTONDOWN => send(Input::Down { b: Button::Left, x, y, mods: mods(), at }),
            WM_RBUTTONDOWN => send(Input::Down { b: Button::Right, x, y, mods: mods(), at }),
            WM_MBUTTONDOWN => send(Input::Down { b: Button::Middle, x, y, mods: mods(), at }),
            WM_LBUTTONUP => send(Input::Up { b: Button::Left, at }),
            WM_RBUTTONUP => send(Input::Up { b: Button::Right, at }),
            WM_MBUTTONUP => send(Input::Up { b: Button::Middle, at }),
            _ => {}
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
}

/// The keyboard layout of the app being typed in.
unsafe fn layout() -> windows_sys::Win32::UI::Input::KeyboardAndMouse::HKL {
    let fg = GetForegroundWindow();
    let thread = if fg.is_null() { 0 } else { GetWindowThreadProcessId(fg, std::ptr::null_mut()) };
    GetKeyboardLayout(thread)
}

/// The text `vk` types with the modifiers that are down. Flag 4: do not change the keyboard
/// state (Windows 10 1607+), so dead keys in the app keep working.
unsafe fn typed(vk: u32, scan: u32) -> Option<String> {
    let mut state = [0u8; 256];
    for k in [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU] {
        if down(k) {
            state[k as usize] = 0x80;
        }
    }
    if GetKeyState(VK_CAPITAL as i32) & 1 != 0 {
        state[VK_CAPITAL as usize] = 0x01;
    }
    let mut buf = [0u16; 8];
    let n = ToUnicodeEx(vk, scan, state.as_ptr(), buf.as_mut_ptr(), buf.len() as i32, 4, layout());
    (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
}

unsafe extern "system" fn key_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let k = &*(lp as *const KBDLLHOOKSTRUCT);
        let vk = k.vkCode;
        let msg = wp as u32;
        if k.flags & LLKHF_INJECTED == 0 && vk < 256 {
            if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                let first = HELD.lock().map(|mut h| !std::mem::replace(&mut h[vk as usize], true)).unwrap_or(true);
                if first {
                    let mode = match KEYS.load(Ordering::SeqCst) {
                        x if x == KeyMode::All as u8 => KeyMode::All,
                        x if x == KeyMode::Shortcuts as u8 => KeyMode::Shortcuts,
                        _ => KeyMode::Off,
                    };
                    // AltGr is right Alt with a Ctrl that Windows adds (left Ctrl). Treated as
                    // typing, so "@" and "€" never show as shortcuts. A real Left Ctrl + Right
                    // Alt is rare and then simply not shown: the safe side.
                    let altgr = down(VK_RMENU) && down(VK_LCONTROL) && !down(VK_LMENU) && !down(VK_RCONTROL);
                    let ch = MapVirtualKeyExW(vk, MAPVK_VK_TO_CHAR, layout()) & 0x7FFF;
                    let key = KeyPress {
                        vk,
                        mods: mods(),
                        altgr,
                        key_char: char::from_u32(ch).filter(|c| *c != '\0'),
                        typed: if mode == KeyMode::All { typed(vk, k.scanCode) } else { None },
                    };
                    send(Input::Key { key, at: Instant::now() });
                }
            } else if msg == WM_KEYUP || msg == WM_SYSKEYUP {
                if let Ok(mut h) = HELD.lock() {
                    h[vk as usize] = false;
                }
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
}
