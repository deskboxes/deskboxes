//! Spike (a): can an UNPACKAGED, non-elevated process activate the IExplorerCommand
//! objects that packaged apps register for File Explorer's modern context menu,
//! and read Title/Icon/State/Flags/SubCommands (and Invoke) them?
//!
//! Usage:
//!   spike-explorer-command <CLSID> <path> [<path>...] [--invoke <title-substring>]
//!
//! Prints one line per command (recursively through EnumSubCommands).
//! Throwaway code: results are recorded in REPORT.md.

use std::time::Instant;
use windows::core::{GUID, HSTRING, PCWSTR};
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: <CLSID> <path>... [--invoke <title-substring>]");
        std::process::exit(2);
    }
    let mut invoke: Option<String> = None;
    let mut paths = Vec::new();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--invoke" {
            invoke = args.get(i + 1).cloned();
            i += 2;
        } else {
            paths.push(args[i].clone());
            i += 1;
        }
    }
    let clsid = GUID::try_from(args[0].trim_matches(|c| c == '{' || c == '}')).expect("bad CLSID");

    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
        println!("elevated={}", is_elevated());

        // Build an IShellItemArray from the paths.
        let pidls: Vec<*mut Common::ITEMIDLIST> = paths
            .iter()
            .map(|p| {
                let mut pidl = std::ptr::null_mut();
                SHParseDisplayName(&HSTRING::from(p.as_str()), None, &mut pidl, 0, None)
                    .unwrap_or_else(|e| panic!("SHParseDisplayName({p}): {e}"));
                pidl
            })
            .collect();
        let consts: Vec<*const Common::ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
        let items: IShellItemArray = SHCreateShellItemArrayFromIDLists(&consts).expect("item array");

        let t0 = Instant::now();
        let cmd: windows::core::Result<IExplorerCommand> =
            CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER | CLSCTX_INPROC_SERVER);
        let cmd = match cmd {
            Ok(c) => c,
            Err(e) => {
                println!("ACTIVATION FAILED {:?} {e} after {:?}", e.code(), t0.elapsed());
                std::process::exit(1);
            }
        };
        println!("activated in {:?}", t0.elapsed());
        dump(&cmd, &items, 0, invoke.as_deref());

        for p in pidls {
            CoTaskMemFree(Some(p as _));
        }
        CoUninitialize();
    }
}

unsafe fn dump(cmd: &IExplorerCommand, items: &IShellItemArray, depth: usize, invoke: Option<&str>) {
    let pad = "  ".repeat(depth);
    let t = Instant::now();
    let title = cmd.GetTitle(items).map(|p| take(p)).unwrap_or_else(|e| format!("<GetTitle {e}>"));
    let icon = cmd.GetIcon(items).map(|p| take(p)).unwrap_or_else(|e| format!("<GetIcon {:?}>", e.code()));
    let tip = cmd.GetToolTip(items).map(|p| take(p)).unwrap_or_else(|e| format!("<none {:?}>", e.code()));
    let state = cmd.GetState(items, true).map(|s| s as i64).unwrap_or(-1);
    let state_slow = cmd.GetState(items, false).map(|s| s as i64).unwrap_or(-1);
    let flags = cmd.GetFlags().map(|f| f as i64).unwrap_or(-1);
    let canon = cmd.GetCanonicalName().map(|g| format!("{g:?}")).unwrap_or_default();
    println!(
        "{pad}- title={title:?} state(okBeSlow=1)={state} state(0)={state_slow} flags=0x{flags:x} icon={icon:?} tip={tip:?} canon={canon} ({:?})",
        t.elapsed()
    );
    if let (Some(want), false) = (invoke, title.is_empty()) {
        if title.to_lowercase().contains(&want.to_lowercase()) {
            let r = cmd.Invoke(items, None);
            println!("{pad}  INVOKE -> {r:?}");
            // Many handlers finish asynchronously (worker threads / message posts): keep the
            // apartment alive and pump messages like a long-lived host would.
            pump(std::time::Duration::from_secs(6));
        }
    }
    if flags >= 0 && (flags as u32 & ECF_HASSUBCOMMANDS.0 as u32) != 0 {
        match cmd.EnumSubCommands() {
            Ok(en) => loop {
                let mut out = [None];
                let mut fetched = 0u32;
                if en.Next(&mut out, Some(&mut fetched)).is_err() || fetched == 0 {
                    break;
                }
                if let Some(sub) = out[0].take() {
                    dump(&sub, items, depth + 1, invoke);
                }
            },
            Err(e) => println!("{pad}  <EnumSubCommands {e}>"),
        }
    }
}

unsafe fn pump(d: std::time::Duration) {
    use windows::Win32::UI::WindowsAndMessaging::*;
    let end = Instant::now() + d;
    while Instant::now() < end {
        let mut m = MSG::default();
        while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

unsafe fn take(p: windows::core::PWSTR) -> String {
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as _));
    s
}

fn is_elevated() -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::*;
    use windows::Win32::System::Threading::*;
    unsafe {
        let mut tok = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0;
        let ok = GetTokenInformation(
            tok,
            TokenElevation,
            Some(&mut e as *mut _ as _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(tok);
        ok && e.TokenIsElevated != 0
    }
}

#[allow(dead_code)]
fn _unused(_: PCWSTR) {}
