//! deskboxes-watchdog (docs/ARCHITECTURE.md §1, §5): the autostart target.
//!
//! `deskboxes-watchdog --launch [core args...]`
//! - starts `deskboxes.exe --watchdog-pid <own pid> [core args]` next to itself,
//! - exit code 0 from the core = clean exit (quit, "Disable Deskboxes"): the watchdog exits too,
//! - crash, abnormal exit or a hang (no answer to WM_NULL within 6 s, checked every 3 s): native
//!   icons are restored at once, then the core is restarted, at most 3 times in 60 s,
//! - WM_CLOSE to its "DeskboxesWatchdog" window (sent by `deskboxes --restore-icons`): stop without
//!   restarting.
//! Log: %LOCALAPPDATA%\Deskboxes\logs\watchdog.log.

#![windows_subsystem = "windows"]

use std::io::Write;
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

static mut STOP: bool = false;
/// Windows is signing out or shutting down: the core ending now is not a crash. The core itself
/// decides whether Explorer's icons come back (they stay hidden when it starts again at sign-in,
/// so Explorer never draws them before Deskboxes).
static mut ENDING: bool = false;

fn log(msg: &str) {
    let dir = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Deskboxes").join("logs");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("watchdog.log")) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "{t} pid={} {msg}", std::process::id());
    }
}

extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if m == WM_CLOSE {
            STOP = true;
            return LRESULT(0);
        }
        if m == WM_QUERYENDSESSION {
            ENDING = true;
            return LRESULT(1);
        }
        if m == WM_ENDSESSION {
            // wParam 0: the end was called off.
            ENDING = wp.0 != 0;
            return LRESULT(0);
        }
        DefWindowProcW(h, m, wp, lp)
    }
}

/// The core's message window, if it exists (owned by `pid`).
fn core_window(pid: u32) -> Option<HWND> {
    unsafe {
        let mut h = FindWindowExW(None, None, w!("DeskboxesCore"), None).ok()?;
        loop {
            let mut p = 0u32;
            GetWindowThreadProcessId(h, Some(&mut p));
            if p == pid {
                return Some(h);
            }
            h = FindWindowExW(None, Some(h), w!("DeskboxesCore"), None).ok()?;
        }
    }
}

fn pump() {
    unsafe {
        let mut m = MSG::default();
        while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
            DispatchMessageW(&m);
        }
    }
}

enum Outcome {
    Clean,
    Crashed(u32),
    Hung,
    Stopped,
    SessionEnd,
}

fn supervise(child: &mut std::process::Child) -> Outcome {
    use std::os::windows::io::AsRawHandle;
    let ph = HANDLE(child.as_raw_handle());
    let pid = child.id();
    let mut last_check = Instant::now();
    loop {
        let r = unsafe { MsgWaitForMultipleObjects(Some(&[ph]), false, 500, QS_ALLINPUT) };
        pump();
        if unsafe { STOP } {
            return Outcome::Stopped;
        }
        if r == WAIT_OBJECT_0 && unsafe { ENDING } {
            return Outcome::SessionEnd;
        }
        if r == WAIT_OBJECT_0 {
            let code = child.wait().map(|s| s.code().unwrap_or(1) as u32).unwrap_or(1);
            return if code == 0 { Outcome::Clean } else { Outcome::Crashed(code) };
        }
        // (While the session ends the core is busy saving: not a hang.)
        if last_check.elapsed() >= Duration::from_secs(3) && !unsafe { ENDING } {
            last_check = Instant::now();
            if let Some(h) = core_window(pid) {
                let ok = unsafe { SendMessageTimeoutW(h, WM_NULL, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG | SMTO_BLOCK, 6000, None) };
                if ok.0 == 0 {
                    return Outcome::Hung;
                }
            }
        }
    }
}

fn main() {
    // Never a crash dialog on the user's screen.
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::*;
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a.as_str()) != Some("--launch") {
        log("usage: deskboxes-watchdog --launch [core args]");
        std::process::exit(1);
    }
    // Started inside our menu package (from Windows' desktop menu): start again through Explorer,
    // outside it (a packaged core's HKCU writes are virtualized, and its shell host could hang on
    // the menu's `dllhost`, leaving context menus dead). Once only.
    let relaunched = args.iter().any(|a| a == "--relaunched");
    let core_args: Vec<String> = args[1..].iter().filter(|a| *a != "--relaunched").cloned().collect();
    if !relaunched && deskboxes_shell::launch::has_package_identity() {
        deskboxes_shell::com_sta();
        let exe = std::env::current_exe().unwrap_or_default();
        let dir = exe.parent().map(|d| d.to_path_buf()).unwrap_or_default();
        let mut again: Vec<String> = args.iter().map(|a| quote_arg(a)).collect();
        again.push("--relaunched".into());
        let again = again.join(" ");
        match deskboxes_shell::launch::via_explorer(&exe, &again, &dir) {
            Ok(()) => {
                log("started inside the menu package: started again through Explorer");
                return;
            }
            Err(e) => log(&format!("started inside the menu package; through Explorer failed ({e}): going on")),
        }
    }
    unsafe {
        let _mutex = CreateMutexW(None, true, w!(r"Local\DeskboxesWatchdog"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            log("another watchdog is running; exiting");
            return;
        }
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: w!("DeskboxesWatchdog"), ..Default::default() };
        RegisterClassW(&wc);
        let _ = CreateWindowExW(WINDOW_EX_STYLE(0), w!("DeskboxesWatchdog"), w!("Deskboxes watchdog"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst.into()), None);
        deskboxes_shell::com_sta();
    }
    let core = std::env::current_exe().unwrap_or_default().with_file_name("deskboxes.exe");
    let mut starts: Vec<Instant> = vec![];
    loop {
        starts.retain(|t| t.elapsed() < Duration::from_secs(60));
        if starts.len() >= 4 {
            log("core failed 3 times within 60 s: giving up with native icons restored");
            deskboxes_shell::hide::restore();
            break;
        }
        starts.push(Instant::now());
        let mut child = match std::process::Command::new(&core).arg("--watchdog-pid").arg(std::process::id().to_string()).args(&core_args).spawn() {
            Ok(c) => c,
            Err(e) => {
                log(&format!("cannot start {}: {e}; restoring icons", core.display()));
                deskboxes_shell::hide::restore();
                break;
            }
        };
        log(&format!("core started pid={}", child.id()));
        match supervise(&mut child) {
            Outcome::Clean => {
                log("core exited cleanly");
                break;
            }
            Outcome::Stopped => {
                log("stop requested (WM_CLOSE); not restarting");
                break;
            }
            Outcome::SessionEnd => {
                log("the session is ending: the core is gone; Explorer's icons left as it left them");
                break;
            }
            Outcome::Crashed(code) => {
                log(&format!("core exited with {code:#x}: restoring icons, restarting"));
                deskboxes_shell::hide::restore();
            }
            Outcome::Hung => {
                log("core hung (no answer to WM_NULL within 6 s): terminating, restoring icons, restarting");
                let _ = child.kill();
                let _ = child.wait();
                deskboxes_shell::hide::restore();
            }
        }
    }
}

/// One argument for a Windows command line, quoted so `CommandLineToArgvW` (and the Rust
/// runtime) give it back unchanged: spaces and quotes inside, backslashes before a quote.
fn quote_arg(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '\n', '"']) {
        return a.to_string();
    }
    let mut out = String::from("\"");
    let mut slashes = 0usize;
    for c in a.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::quote_arg;

    #[test]
    fn quoting() {
        assert_eq!(quote_arg("--window"), "--window");
        assert_eq!(quote_arg(r"C:\Test Fixtures\Desktop"), r#""C:\Test Fixtures\Desktop""#);
        assert_eq!(quote_arg(r"C:\Test Fixtures\"), r#""C:\Test Fixtures\\""#);
        assert_eq!(quote_arg(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote_arg(""), r#""""#);
    }
}
