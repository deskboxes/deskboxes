//! Shell integration for the core (docs/ARCHITECTURE.md §5, §6). Everything here is a thin,
//! documented wrapper over Windows' own APIs: nothing is written into Desktop folders, files are
//! opened with `FILE_READ_ATTRIBUTES` only (no OneDrive hydration), and file changes go through
//! `IFileOperation`.

pub mod autostart;
pub mod fileops;
pub mod hide;
pub mod icons;
pub mod items;
pub mod reg;
pub mod turnon;
pub mod view;
pub mod wallpaper;
pub mod watch;

use windows::core::{HSTRING, PCWSTR};

pub fn wide(s: &str) -> HSTRING {
    HSTRING::from(s)
}

pub(crate) fn pcw(h: &HSTRING) -> PCWSTR {
    PCWSTR(h.as_ptr())
}

/// Initialize COM (STA) on the calling thread for its lifetime.
pub fn com_sta() {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED | windows::Win32::System::Com::COINIT_DISABLE_OLE1DDE);
    }
}

/// Open the item with its default verb (what a double-click does), on a fresh STA thread so the
/// UI thread never blocks on a shell call.
pub fn open_default(parsing_names: Vec<String>, owner: isize) {
    std::thread::spawn(move || unsafe {
        use windows::Win32::UI::Shell::*;
        com_sta();
        for p in parsing_names {
            let h = wide(&p);
            let mut pidl = std::ptr::null_mut();
            if SHParseDisplayName(pcw(&h), None, &mut pidl, 0, None).is_err() {
                log::warn!("open: cannot parse {p}");
                continue;
            }
            let mut sei = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_INVOKEIDLIST,
                hwnd: windows::Win32::Foundation::HWND(owner as _),
                lpIDList: pidl as _,
                nShow: windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL.0,
                ..Default::default()
            };
            let r = ShellExecuteExW(&mut sei);
            log::info!("open {p} -> {r:?}");
            windows::Win32::System::Com::CoTaskMemFree(Some(pidl as _));
        }
    });
}
