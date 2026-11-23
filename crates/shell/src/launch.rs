//! Starting Deskboxes as if the user had started it: through Explorer's desktop
//! (`IShellDispatch2::ShellExecute`), so the new process is Explorer's child. It then runs with
//! the user's normal token (not an elevated setup's) and without the identity of our sparse menu
//! package (a process started from the menu extension's `dllhost` inherits it, and with it
//! virtualized HKCU writes and a shell host that could hang on that same `dllhost`).

use windows::core::{Interface, BSTR};
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Start `exe args` in `cwd` through Explorer's desktop window. COM must be initialized.
pub fn via_explorer(exe: &std::path::Path, args: &str, cwd: &std::path::Path) -> windows::core::Result<()> {
    unsafe {
        let sw: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
        let mut loc = VARIANT::default();
        (*loc.Anonymous.Anonymous).vt = VT_I4;
        (*loc.Anonymous.Anonymous).Anonymous.lVal = CSIDL_DESKTOP as i32;
        let root = VARIANT::default();
        let mut hwnd = 0i32;
        let disp = sw.FindWindowSW(&loc, &root, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)?;
        let sp: IServiceProvider = disp.cast()?;
        let browser: IShellBrowser = sp.QueryService(&SID_STopLevelBrowser)?;
        let view = browser.QueryActiveShellView()?;
        let bg: IDispatch = view.GetItemObject(SVGIO_BACKGROUND)?;
        let fv: IShellFolderViewDual = bg.cast()?;
        let app: IShellDispatch2 = fv.Application()?.cast()?;
        let v = |s: &str| VARIANT::from(BSTR::from(s));
        app.ShellExecute(&BSTR::from(exe.to_string_lossy().as_ref()), &v(args), &v(&cwd.to_string_lossy()), &v("open"), &VARIANT::from(SW_SHOWNORMAL.0))?;
    }
    Ok(())
}

/// This process runs with a package identity (started from the menu package's extension).
pub fn has_package_identity() -> bool {
    let mut len = 0u32;
    let r = unsafe { windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName(&mut len, None) };
    // APPMODEL_ERROR_NO_PACKAGE (15700): none. ERROR_INSUFFICIENT_BUFFER: there is one.
    r.0 != 15700
}
