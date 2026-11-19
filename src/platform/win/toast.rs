//! Windows toast notifications. The tray balloon cannot have buttons, a toast can.
//!
//! Blink is a plain exe (not packaged), so Windows needs an app id with a display name and an
//! icon registered for the user before it shows toasts under the name "Blink". That is done
//! at startup; it needs no installer. Button presses arrive on a thread-pool thread and are
//! posted to the tray window, which handles them like every other action.

use super::wide;
use crate::config::config_dir;
use crate::toast_xml::{self, Button};
use windows::core::{IInspectable, Interface, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::UI::Notifications::{ToastActivatedEventArgs, ToastNotification, ToastNotificationManager};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE,
    REG_SZ,
};
use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

/// The app's identity for Windows notifications (AppUserModelID). Windows keeps the user's
/// notification settings for Blink under this id: never change it, or every user's settings
/// for Blink notifications are reset.
const APP_ID: &str = "BlinkScreenshot.Blink";
const ICON: &[u8] = include_bytes!("../../../assets/blink.ico");

fn set_value(key: HKEY, name: &str, value: &str) -> bool {
    let name = wide(name);
    let value = wide(value);
    unsafe { RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, value.as_ptr() as *const u8, (value.len() * 2) as u32) == 0 }
}

/// Makes toasts show up as "Blink" with its icon. Call once, on the tray thread.
pub fn init() {
    unsafe {
        // Fails with RPC_E_CHANGED_MODE if the thread is already set up some other way; fine.
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
        let id = wide(APP_ID);
        SetCurrentProcessExplicitAppUserModelID(id.as_ptr());
    }
    // The icon has to be a file; the one in the exe is not addressable for this.
    let dir = config_dir();
    let icon = dir.join("blink.ico");
    if std::fs::read(&icon).map(|b| b != ICON).unwrap_or(true) {
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(&icon, ICON).ok();
    }
    let path = wide(&format!("Software\\Classes\\AppUserModelId\\{APP_ID}"));
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        let created = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        );
        if created == 0 {
            set_value(key, "DisplayName", "Blink");
            set_value(key, "IconUri", &icon.display().to_string());
            RegCloseKey(key);
        }
    }
}

/// Shows a toast with `buttons`; pressing one (or the toast itself, `click_action`) posts
/// `msg` to `hwnd` with the action as `wParam` and `token` as `lParam`.
/// Keep the result: hand it to `hide` to remove the toast again.
pub fn show(
    hwnd: HWND,
    msg: u32,
    token: u32,
    title: &str,
    body: &str,
    click_action: u32,
    buttons: &[Button],
) -> windows::core::Result<ToastNotification> {
    let xml = XmlDocument::new()?;
    xml.LoadXml(&HSTRING::from(toast_xml::toast(token, title, body, click_action, buttons)))?;
    let toast = ToastNotification::CreateToastNotification(&xml)?;
    let target = hwnd as isize;
    toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, args| {
        let arguments = args
            .as_ref()
            .and_then(|a| a.cast::<ToastActivatedEventArgs>().ok())
            .and_then(|a| a.Arguments().ok())
            .map(|s| s.to_string());
        if let Some((token, action)) = arguments.as_deref().and_then(toast_xml::parse_arguments) {
            unsafe {
                PostMessageW(target as HWND, msg, action as usize, token as isize);
            }
        }
        Ok(())
    }))?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?.Show(&toast)?;
    Ok(toast)
}

/// Removes the toast from the screen and the notification center.
pub fn hide(toast: &ToastNotification) {
    if let Ok(notifier) = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID)) {
        notifier.Hide(toast).ok();
    }
}
