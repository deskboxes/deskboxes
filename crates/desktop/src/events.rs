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
pub const WM_APP_RENAME_END: u32 = WM_APP + 3;

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
