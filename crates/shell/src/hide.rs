//! Hide / restore Explorer's native desktop icons (docs/ARCHITECTURE.md §5, verified in spike c).
//! `IFolderView2::SetCurrentFolderFlags(FWF_NOICONS)` on Explorer's desktop view: idempotent, live,
//! and Explorer persists it as `HideIcons` itself (no boot flash). The DefView `WM_COMMAND 0x7402`
//! toggle is the last-resort restore path only.

use crate::reg;
use windows::core::{w, Interface};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub fn progman() -> HWND {
    unsafe { FindWindowW(w!("Progman"), None).unwrap_or_default() }
}

pub fn defview() -> HWND {
    unsafe {
        let p = progman();
        let mut dv = FindWindowExW(Some(p), None, w!("SHELLDLL_DefView"), None).unwrap_or_default();
        if dv.0.is_null() {
            let mut ww = FindWindowExW(None, None, w!("WorkerW"), None).unwrap_or_default();
            while !ww.0.is_null() && dv.0.is_null() {
                dv = FindWindowExW(Some(ww), None, w!("SHELLDLL_DefView"), None).unwrap_or_default();
                ww = FindWindowExW(None, Some(ww), w!("WorkerW"), None).unwrap_or_default();
            }
        }
        dv
    }
}

pub fn listview() -> HWND {
    unsafe { FindWindowExW(Some(defview()), None, w!("SysListView32"), None).unwrap_or_default() }
}

/// Explorer's desktop `IFolderView2` (needs COM initialized on the calling thread).
pub fn desktop_folder_view() -> windows::core::Result<IFolderView2> {
    unsafe {
        let sw: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL)?;
        let mut loc = VARIANT::default();
        (*loc.Anonymous.Anonymous).vt = VT_I4;
        (*loc.Anonymous.Anonymous).Anonymous.lVal = CSIDL_DESKTOP as i32;
        let root = VARIANT::default();
        let mut hwnd = 0i32;
        let disp = sw.FindWindowSW(&loc, &root, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)?;
        let sp: IServiceProvider = disp.cast()?;
        let browser: IShellBrowser = sp.QueryService(&SID_STopLevelBrowser)?;
        browser.QueryActiveShellView()?.cast()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconState {
    pub reg_hidden: Option<bool>,
    pub view_hidden: Option<bool>,
    pub listview_visible: bool,
}

impl IconState {
    /// Native icons are visible to the user.
    pub fn visible(&self) -> bool {
        self.listview_visible && self.view_hidden != Some(true)
    }
}

pub fn state() -> IconState {
    let view_hidden = desktop_folder_view().and_then(|fv| unsafe { fv.GetCurrentFolderFlags() }).ok().map(|f| f & FWF_NOICONS.0 as u32 != 0);
    IconState {
        reg_hidden: reg::get_dword(reg::ADVANCED, "HideIcons").map(|v| v != 0),
        view_hidden,
        listview_visible: unsafe { IsWindowVisible(listview()).as_bool() },
    }
}

fn set_view(hidden: bool) -> windows::core::Result<()> {
    let fv = desktop_folder_view()?;
    unsafe { fv.SetCurrentFolderFlags(FWF_NOICONS.0 as u32, if hidden { FWF_NOICONS.0 as u32 } else { 0 }) }
}

fn toggle() {
    unsafe {
        let _ = SendMessageTimeoutW(defview(), WM_COMMAND, WPARAM(0x7402), LPARAM(0), SMTO_ABORTIFHUNG, 2000, None);
    }
}

/// Hide or show the native icons and verify. Falls back to the toggle when the view call did not
/// take effect. Returns the final state.
pub fn set_hidden(hidden: bool) -> IconState {
    let r = set_view(hidden);
    let mut s = state();
    for _ in 0..20 {
        if s.view_hidden == Some(hidden) && s.listview_visible != hidden {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
        s = state();
    }
    // The toggle flips whatever Explorer has: only when its icons are on screen the other way
    // than wanted. (Hiding while Explorer's desktop view was not there yet, at sign-in with its
    // icons already hidden, it showed them.)
    let on_screen_wrong = if hidden { s.listview_visible } else { !s.listview_visible && !listview().is_invalid() };
    if (s.view_hidden != Some(hidden) || s.listview_visible == hidden) && on_screen_wrong {
        log::warn!("hide({hidden}): view call {r:?} left {s:?}; using the DefView toggle");
        toggle();
        std::thread::sleep(std::time::Duration::from_millis(300));
        s = state();
    } else if s.view_hidden != Some(hidden) {
        log::warn!("hide({hidden}): view call {r:?} left {s:?}; Explorer's icons are {} (no toggle)", if s.listview_visible { "on screen" } else { "not on screen" });
    }
    log::info!("native icons hidden={hidden} -> {s:?}");
    s
}

/// The safety net: native icons visible, whatever state we are in.
pub fn restore() -> IconState {
    set_hidden(false)
}
