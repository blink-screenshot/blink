//! Hotkey override: a low-level keyboard hook that sees key presses before Windows hands
//! them to whichever app registered the combination with RegisterHotKey (the Snipping Tool's
//! Print Screen, another screenshot tool, ...). Only used for hotkeys the user chose to override.
//!
//! The hook runs on its own thread with its own message loop. Windows drops hooks that do
//! not answer quickly, and the main thread is busy for seconds during a scrolling capture.

use crate::config::Hotkey;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL, VK_LWIN,
    VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostMessageW, SetWindowsHookExW, TranslateMessage, HC_ACTION,
    KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// Posted to the tray window with the action id in `wParam`.
pub const FORCED_HOTKEY_MSG: u32 = WM_APP + 3;

static FORCED: Mutex<Vec<(usize, Hotkey)>> = Mutex::new(Vec::new());
static TARGET: AtomicIsize = AtomicIsize::new(0);
/// Key whose press we swallowed, so its release is swallowed too.
static SWALLOWING: AtomicU32 = AtomicU32::new(0);
static STARTED: OnceLock<()> = OnceLock::new();

/// Sets the overridden hotkeys (action id, hotkey). Starts the hook the first time it is needed.
pub fn set(hwnd: HWND, list: Vec<(usize, Hotkey)>) {
    TARGET.store(hwnd as isize, Ordering::SeqCst);
    let needed = !list.is_empty();
    *FORCED.lock().unwrap_or_else(|e| e.into_inner()) = list;
    if needed {
        STARTED.get_or_init(|| {
            std::thread::spawn(|| unsafe {
                let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), GetModuleHandleW(std::ptr::null()), 0);
                if hook.is_null() {
                    return;
                }
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            });
        });
    }
}

fn down(vk: u16) -> bool {
    unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

/// Taps an unassigned key so that releasing Alt or Win does not open a menu / Start.
unsafe fn mask_modifier_release() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: 0xE8, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
}

unsafe extern "system" fn hook_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let k = &*(lp as *const KBDLLHOOKSTRUCT);
        if k.flags & LLKHF_INJECTED == 0 {
            let msg = wp as u32;
            let vk = k.vkCode;
            if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                let (ctrl, alt, shift) = (down(VK_CONTROL), down(VK_MENU), down(VK_SHIFT));
                let win = down(VK_LWIN) || down(VK_RWIN);
                let hit = FORCED.lock().ok().and_then(|list| {
                    list.iter()
                        .find(|(_, h)| h.vk == vk && (h.ctrl, h.alt, h.shift, h.win) == (ctrl, alt, shift, win))
                        .map(|(id, _)| *id)
                });
                if let Some(id) = hit {
                    // Key repeat sends more key-downs: act once per press.
                    if SWALLOWING.swap(vk, Ordering::SeqCst) != vk {
                        if alt || win {
                            mask_modifier_release();
                        }
                        PostMessageW(TARGET.load(Ordering::SeqCst) as HWND, FORCED_HOTKEY_MSG, id, 0);
                    }
                    return 1;
                }
            } else if (msg == WM_KEYUP || msg == WM_SYSKEYUP) && SWALLOWING.load(Ordering::SeqCst) == vk && vk != 0 {
                SWALLOWING.store(0, Ordering::SeqCst);
                return 1;
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
}
