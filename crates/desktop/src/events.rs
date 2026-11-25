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
/// The icon name field is done: wParam 1 commit (Enter), 0 cancel (Esc), 2 it lost the focus.
pub const WM_APP_RENAME_END: u32 = WM_APP + 6;
/// Open the folder picker asked for (folder boxes), outside the App's borrow.
pub const WM_APP_PICKFOLDER: u32 = WM_APP + 7;
/// Settings › Export (wParam 0) or Import (1): the file dialog, outside the App's borrow.
pub const WM_APP_TRANSFER: u32 = WM_APP + 9;
/// The wheel turned while something is dragged over a box (from the drag's mouse hook).
pub const WM_APP_DRAGWHEEL: u32 = WM_APP + 8;
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
    /// The update installer asked for from Settings: started (Deskboxes exits for it), or not.
    Installing {
        started: bool,
        relaunch: bool,
    },
    Scanned {
        items: Vec<Item>,
        icons: Vec<(u32, IconImage)>,
        native: Option<NativeView>,
        ms: f64,
        first: bool,
        resolved: usize,
        /// Icon ids the worker may hand out again without their image (shared icons it keeps
        /// by system icon index, at every size): never let go of.
        keep: Vec<u32>,
    },
    /// Something changed on disk (watcher) or after one of our operations.
    Rescan,
    /// Something changed in this folder (its watcher): only it needs listing again.
    DirChanged(String),
    /// (With the host's number, `App::host_count`: one replaced since is let go.)
    HostReady(u32, deskboxes_ipc::HostClient),
    HostMsg(serde_json::Value),
    HostGone(u32),
    /// The updater's status changed (Settings shows it).
    Update,
    /// Explorer's tooltip text for an item (empty: none).
    InfoTip {
        key: deskboxes_core::ItemKey,
        text: String,
    },
    /// A new file operation went through (for undo), with what really happened.
    FileDone {
        op: deskboxes_core::layout::FileOp,
    },
    /// An undo (`undo`) or redo of a file operation finished: what really went through (None:
    /// nothing did).
    FileStepDone {
        op: Option<deskboxes_core::layout::FileOp>,
        undo: bool,
    },
    /// Icons made at another size for boxes (see `views`): (desktop icon id, its id at `px`).
    IconsAt {
        px: u32,
        map: Vec<(u32, u32)>,
        icons: Vec<(u32, IconImage)>,
    },
    /// Game covers read (see `covers`): Steam app id, its cover if there is one.
    Covers(Vec<(u32, Option<IconImage>)>),
    /// Pictures are being made after a scan (true) or all are made (false): while they are,
    /// the items in view are asked for first (`Req::PicturesFirst`).
    PicturesPending(bool),
    /// Pictures made after the scan that listed their items (`worker::spawn_pictures`):
    /// (parsing name, its icon id now), and the new icons.
    Pictures {
        items: Vec<(String, u32)>,
        icons: Vec<(u32, IconImage)>,
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
