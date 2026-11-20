//! Spike (c): hide / restore Explorer's native desktop icons.
//!
//! `spike-hide-icons state`            print registry HideIcons, FWF_NOICONS, list-view visibility
//! `spike-hide-icons fv <on|off>`      IFolderView2::SetCurrentFolderFlags(FWF_NOICONS)
//! `spike-hide-icons reg <on|off>`     write HKCU ...\Explorer\Advanced\HideIcons only
//! `spike-hide-icons toggle`           WM_COMMAND 0x7402 to SHELLDLL_DefView (the "Show desktop icons" command)
//! `spike-hide-icons select <path>..`  select existing desktop items (by full parsing path) in Explorer's
//!                                     desktop view; used by tools/menu-probe.ps1 (read-only probing)
//! `spike-hide-icons deselect`         clear the desktop selection
//!
//! Throwaway; results recorded in REPORT.md.

use windows::core::{w, Interface};
use windows::Win32::System::Variant::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const ADV: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced");

unsafe fn reg_get() -> Option<u32> {
    let mut v = 0u32;
    let mut sz = 4u32;
    let r = RegGetValueW(HKEY_CURRENT_USER, ADV, w!("HideIcons"), RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as _), Some(&mut sz));
    if r.is_ok() { Some(v) } else { None }
}
unsafe fn reg_set(v: u32) {
    let r = RegSetKeyValueW(HKEY_CURRENT_USER, ADV, w!("HideIcons"), REG_DWORD.0, Some(&v as *const _ as _), 4);
    println!("RegSetKeyValue HideIcons={v} -> {r:?}");
}

unsafe fn listview() -> (HWND, HWND) {
    let progman = FindWindowW(w!("Progman"), None).unwrap_or_default();
    let mut defview = FindWindowExW(Some(progman), None, w!("SHELLDLL_DefView"), None).unwrap_or_default();
    if defview.0.is_null() {
        // Older layout: DefView lives in a WorkerW.
        let mut ww = FindWindowExW(None, None, w!("WorkerW"), None).unwrap_or_default();
        while !ww.0.is_null() && defview.0.is_null() {
            defview = FindWindowExW(Some(ww), None, w!("SHELLDLL_DefView"), None).unwrap_or_default();
            ww = FindWindowExW(None, Some(ww), w!("WorkerW"), None).unwrap_or_default();
        }
    }
    let lv = FindWindowExW(Some(defview), None, w!("SysListView32"), None).unwrap_or_default();
    (defview, lv)
}

unsafe fn desktop_folder_view() -> windows::core::Result<IFolderView2> {
    let sw: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL)?;
    let mut loc = VARIANT::default();
    (*loc.Anonymous.Anonymous).vt = VT_I4;
    (*loc.Anonymous.Anonymous).Anonymous.lVal = CSIDL_DESKTOP as i32;
    let root = VARIANT::default();
    let mut hwnd = 0i32;
    let disp = sw.FindWindowSW(&loc, &root, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)?;
    let sp: IServiceProvider = disp.cast()?;
    let browser: IShellBrowser = sp.QueryService(&SID_STopLevelBrowser)?;
    let view = browser.QueryActiveShellView()?;
    view.cast()
}

unsafe fn state() {
    let (dv, lv) = listview();
    let flags = desktop_folder_view().and_then(|fv| fv.GetCurrentFolderFlags());
    println!(
        "STATE reg.HideIcons={:?} FWF_NOICONS={:?} defview={:?} listview={:?} lv_visible={}",
        reg_get(),
        flags.map(|f| f & FWF_NOICONS.0 as u32 != 0),
        dv.0,
        lv.0,
        IsWindowVisible(lv).as_bool()
    );
}

/// Selects the given items in Explorer's desktop view. Paths are compared with each view item's
/// SHGDN_FORPARSING name, so items of the user and the Public desktop both work.
/// The FIRST path is the primary item, as after a real right-click: it is selected last and
/// gets the focus (Explorer builds the menu for the most recently selected item).
unsafe fn select(paths: &[String]) -> windows::core::Result<()> {
    let fv = desktop_folder_view()?;
    let desk = SHGetDesktopFolder()?;
    let want: Vec<String> = paths.iter().map(|p| p.to_lowercase()).collect();
    let mut index: Vec<Option<i32>> = vec![None; want.len()];
    let n = fv.ItemCount(SVGIO_ALLVIEW)?;
    for i in 0..n {
        let pidl = fv.Item(i)?;
        let mut sr = Common::STRRET::default();
        let mut name = String::new();
        if desk.GetDisplayNameOf(pidl, SHGDN_FORPARSING, &mut sr).is_ok() {
            let mut p = windows::core::PWSTR::null();
            if StrRetToStrW(&mut sr, Some(pidl), &mut p).is_ok() {
                name = p.to_string().unwrap_or_default().to_lowercase();
                CoTaskMemFree(Some(p.0 as _));
            }
        }
        CoTaskMemFree(Some(pidl as _));
        if let Some(k) = want.iter().position(|w| *w == name) { index[k] = Some(i); }
    }
    if index.iter().any(|x| x.is_none()) {
        println!("not found: {:?}", paths.iter().zip(&index).filter(|(_, i)| i.is_none()).map(|(p, _)| p).collect::<Vec<_>>());
        return Err(windows::core::Error::from(E_INVALIDARG));
    }
    // SVSI_SELECT=1 DESELECTOTHERS=4 ENSUREVISIBLE=8 FOCUSED=16
    let count = index.len();
    for (k, i) in index.iter().enumerate().rev() {
        let flags = if k == count - 1 { 1 | 4 } else { 1 } | if k == 0 { 8 | 16 } else { 0 };
        fv.SelectItem(i.unwrap(), flags)?;
    }
    println!("selected {count} item(s), primary {}", paths[0]);
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
        match args.first().map(|s| s.as_str()) {
            Some("state") => state(),
            Some("fv") => {
                let on = args.get(1).map(|s| s == "on").unwrap_or(true);
                let t = std::time::Instant::now();
                let fv = desktop_folder_view().expect("desktop IFolderView2");
                let r = fv.SetCurrentFolderFlags(FWF_NOICONS.0 as u32, if on { FWF_NOICONS.0 as u32 } else { 0 });
                println!("SetCurrentFolderFlags(NOICONS={on}) -> {r:?} in {:?}", t.elapsed());
                std::thread::sleep(std::time::Duration::from_millis(400));
                state();
            }
            Some("reg") => {
                let on = args.get(1).map(|s| s == "on").unwrap_or(true);
                reg_set(on as u32);
                std::thread::sleep(std::time::Duration::from_millis(400));
                state();
            }
            Some("toggle") => {
                let (dv, _) = listview();
                let r = SendMessageTimeoutW(dv, WM_COMMAND, WPARAM(0x7402), LPARAM(0), SMTO_ABORTIFHUNG, 2000, None);
                println!("WM_COMMAND 0x7402 -> {r:?}");
                std::thread::sleep(std::time::Duration::from_millis(400));
                state();
            }
            Some("select") => {
                if let Err(e) = select(&args[1..]) { eprintln!("select failed: {e}"); std::process::exit(1); }
            }
            Some("deselect") => {
                let fv = desktop_folder_view().expect("desktop IFolderView2");
                // SVSI_DESELECTOTHERS on item 0 with SVSI_DESELECT (0) for item 0 itself.
                let r = fv.SelectItem(0, 4).and_then(|_| fv.SelectItem(0, 0));
                println!("deselect -> {r:?}");
            }
            _ => eprintln!("usage: state | fv on|off | reg on|off | toggle | select <path>.. | deselect"),
        }
        CoUninitialize();
    }
}
