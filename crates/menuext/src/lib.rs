//! "Turn on Deskboxes" in Windows 11's own desktop context menu, shown only while Deskboxes is
//! turned off ("Turn off Deskboxes" ends the core, so nothing of ours is running to add it).
//!
//! An `IExplorerCommand` that Explorer loads from the sparse package the installer registers
//! (`package/AppxManifest.xml`, unsigned; registering it needs administrator rights once).
//! `GetState` reads `%APPDATA%\Deskboxes\settings.json` and hides the item unless
//! `"enabled": false`; `Invoke` starts the watchdog next to this DLL with `--launch --enable` (as
//! the Start menu shortcut does), through Explorer: started from here directly, Deskboxes would
//! inherit the package's identity (`deskboxes_shell::launch`).

#![allow(non_snake_case)]

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Must match `package/AppxManifest.xml`.
pub const CLSID_TURN_ON: GUID = GUID::from_u128(0xE635630E_E581_4382_B31F_5CE245560EBD);

static MODULE: AtomicIsize = AtomicIsize::new(0);
static OBJECTS: AtomicUsize = AtomicUsize::new(0);

#[no_mangle]
extern "system" fn DllMain(inst: HINSTANCE, reason: u32, _: *mut c_void) -> BOOL {
    if reason == windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH {
        MODULE.store(inst.0 as isize, Ordering::Relaxed);
    }
    TRUE
}

/// The folder this DLL is in (the install folder).
fn here() -> Option<PathBuf> {
    let mut buf = [0u16; 1024];
    let n = unsafe { windows::Win32::System::LibraryLoader::GetModuleFileNameW(Some(HMODULE(MODULE.load(Ordering::Relaxed) as _)), &mut buf) } as usize;
    (n > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n]))).and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

fn settings() -> serde_json::Value {
    std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join("Deskboxes").join("settings.json"))
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn turned_off() -> bool {
    settings().get("enabled").and_then(|v| v.as_bool()) == Some(false)
}

fn co_string(s: &str) -> Result<PWSTR> {
    let w: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let p = CoTaskMemAlloc(w.len() * 2) as *mut u16;
        if p.is_null() {
            return Err(E_OUTOFMEMORY.into());
        }
        std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
        Ok(PWSTR(p))
    }
}

#[implement(IExplorerCommand)]
struct TurnOn;

impl TurnOn {
    fn new() -> Self {
        OBJECTS.fetch_add(1, Ordering::Relaxed);
        TurnOn
    }
}

impl Drop for TurnOn {
    fn drop(&mut self) {
        OBJECTS.fetch_sub(1, Ordering::Relaxed);
    }
}

impl IExplorerCommand_Impl for TurnOn_Impl {
    fn GetTitle(&self, _: Ref<IShellItemArray>) -> Result<PWSTR> {
        let setting = settings().get("language").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let lang = deskboxes_i18n::Lang::pick(&setting, unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() });
        co_string(deskboxes_i18n::t(lang, deskboxes_i18n::S::TurnOn))
    }
    fn GetIcon(&self, _: Ref<IShellItemArray>) -> Result<PWSTR> {
        let exe = here().map(|d| d.join("deskboxes.exe")).ok_or(Error::from(E_FAIL))?;
        co_string(&format!("{},0", exe.display()))
    }
    fn GetToolTip(&self, _: Ref<IShellItemArray>) -> Result<PWSTR> {
        Err(E_NOTIMPL.into())
    }
    fn GetCanonicalName(&self) -> Result<GUID> {
        Ok(CLSID_TURN_ON)
    }
    fn GetState(&self, _: Ref<IShellItemArray>, _: BOOL) -> Result<u32> {
        Ok(if turned_off() { ECS_ENABLED.0 as u32 } else { ECS_HIDDEN.0 as u32 })
    }
    fn Invoke(&self, _: Ref<IShellItemArray>, _: Ref<IBindCtx>) -> Result<()> {
        let dir = here().ok_or(Error::from(E_FAIL))?;
        let exe = dir.join("deskboxes-watchdog.exe");
        if deskboxes_shell::launch::via_explorer(&exe, "--launch --enable", &dir).is_ok() {
            return Ok(());
        }
        // (The watchdog leaves the package itself then: see its `main`.)
        let wd = HSTRING::from(exe.as_os_str());
        let d = HSTRING::from(dir.as_os_str());
        let r = unsafe { ShellExecuteW(None, w!("open"), &wd, w!("--launch --enable"), &d, SW_SHOWNORMAL) };
        if r.0 as isize <= 32 {
            return Err(E_FAIL.into());
        }
        Ok(())
    }
    fn GetFlags(&self) -> Result<u32> {
        Ok(ECF_DEFAULT.0 as u32)
    }
    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        Err(E_NOTIMPL.into())
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, iid: *const GUID, out: *mut *mut c_void) -> Result<()> {
        unsafe {
            *out = std::ptr::null_mut();
        }
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let cmd: IExplorerCommand = TurnOn::new().into();
        unsafe { cmd.query(iid, out).ok() }
    }
    fn LockServer(&self, lock: BOOL) -> Result<()> {
        if lock.as_bool() {
            OBJECTS.fetch_add(1, Ordering::Relaxed);
        } else {
            OBJECTS.fetch_sub(1, Ordering::Relaxed);
        }
        Ok(())
    }
}

#[no_mangle]
unsafe extern "system" fn DllGetClassObject(clsid: *const GUID, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    *out = std::ptr::null_mut();
    if *clsid != CLSID_TURN_ON {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let f: IClassFactory = Factory.into();
    f.query(iid, out)
}

/// Explorer frees the DLL when nothing of ours is alive (so an update or uninstall can replace it).
#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    if OBJECTS.load(Ordering::Relaxed) == 0 {
        S_OK
    } else {
        S_FALSE
    }
}
