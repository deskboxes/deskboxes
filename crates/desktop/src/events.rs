//! Worker → UI thread events: a queue plus one posted message to wake the UI thread.

use deskboxes_core::snapshot::IconImage;
use deskboxes_core::Item;
use deskboxes_shell::view::NativeView;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_APP_EVENTS: u32 = WM_APP + 1;
pub const WM_APP_DRAW: u32 = WM_APP + 2;
/// Give the focus back to a name field that just opened (see WM_KILLFOCUS).
pub const WM_APP_REFOCUS: u32 = WM_APP + 3;
/// The wallpaper settings in the registry changed (see `watch_wallpaper`).
pub const WM_APP_WALLPAPER: u32 = WM_APP + 4;

/// Tell `hwnd` whenever HKCU\Control Panel\Desktop changes (the wallpaper, its layout, a
/// slideshow): Windows does not always broadcast WM_SETTINGCHANGE for a new wallpaper.
/// Another window came to the front (see `watch_foreground`); lParam = that window.
pub const WM_APP_FOREGROUND: u32 = WM_APP + 5;

static FG_TARGET: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn on_foreground(_: windows::Win32::UI::Accessibility::HWINEVENTHOOK, _: u32, hwnd: HWND, _: i32, _: i32, _: u32, _: u32) {
    let _ = PostMessageW(Some(HWND(FG_TARGET.load(Ordering::SeqCst) as _)), WM_APP_FOREGROUND, WPARAM(0), LPARAM(hwnd.0 as isize));
}

/// Tell `hwnd` (WM_APP_FOREGROUND) whenever the foreground window changes. Out of context, on
/// this (the UI) thread's message loop.
pub fn watch_foreground(hwnd: HWND) {
    use windows::Win32::UI::Accessibility::SetWinEventHook;
    use windows::Win32::UI::WindowsAndMessaging::{EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS};
    FG_TARGET.store(hwnd.0 as isize, Ordering::SeqCst);
    unsafe {
        let h = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_foreground), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
        if h.is_invalid() {
            log::warn!("foreground hook failed");
        }
    }
}

pub fn watch_wallpaper(hwnd: HWND) {
    let h = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        use windows::Win32::System::Registry::*;
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, windows::core::w!(r"Control Panel\Desktop"), None, KEY_NOTIFY, &mut key).is_err() {
            return;
        }
        loop {
            if RegNotifyChangeKeyValue(key, false, REG_NOTIFY_CHANGE_LAST_SET, None, false).is_err() {
                return;
            }
            let _ = PostMessageW(Some(HWND(h as _)), WM_APP_WALLPAPER, WPARAM(0), LPARAM(0));
        }
    });
}

pub enum Ev {
    Scanned {
        items: Vec<Item>,
        icons: Vec<(u32, IconImage)>,
        native: Option<NativeView>,
        ms: f64,
        first: bool,
        resolved: usize,
    },
    /// Something changed on disk (watcher) or after one of our operations.
    Rescan,
    HostReady(deskboxes_ipc::HostClient),
    HostMsg(serde_json::Value),
    HostGone,
    /// The updater's status changed (Settings shows it).
    Update,
    /// Explorer's tooltip text for an item (empty: none).
    InfoTip {
        key: deskboxes_core::ItemKey,
        text: String,
    },
}

static QUEUE: Mutex<Vec<Ev>> = Mutex::new(Vec::new());
static TARGET: AtomicIsize = AtomicIsize::new(0);

pub fn set_target(h: HWND) {
    TARGET.store(h.0 as isize, Ordering::SeqCst);
}

pub fn post(ev: Ev) {
    let was_empty = {
        let mut q = QUEUE.lock().unwrap_or_else(|p| p.into_inner());
        q.push(ev);
        q.len() == 1
    };
    if was_empty {
        unsafe {
            let _ = PostMessageW(Some(HWND(TARGET.load(Ordering::SeqCst) as _)), WM_APP_EVENTS, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn take() -> Vec<Ev> {
    std::mem::take(&mut *QUEUE.lock().unwrap_or_else(|p| p.into_inner()))
}
