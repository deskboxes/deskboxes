//! deskboxes.exe: the Deskboxes core (docs/ARCHITECTURE.md §1–§10).
//!
//! ```text
//! deskboxes                         run on the real desktop (child surface of Progman)
//! deskboxes --desktop-dir DIR       fixture mode: show DIR instead of the desktop folders
//!           [--public-dir DIR2]       second fixture folder standing in for the Public desktop
//!           [--window]              in a normal top-level test window (native icons untouched)
//!           [--no-hide]             do not hide Explorer's icons
//!           [--profile DIR]         settings/layout/snapshot location (fixture mode default:
//!                                   %LOCALAPPDATA%\Deskboxes\fixture-profile)
//!           [--exit-after MS]       quit MS after the first full reconciliation (measurements)
//!           [--stats-out FILE]      write startup measurements as JSON at exit
//!           [--scene-out FILE]      write item/box screen rects as JSON after each change (tests)
//!           [--no-ui-ops]           FOF_NO_UI for file operations (fixture tests)
//!           [--no-host]             do not start the shell host
//!           [--host EXE]            shell host executable
//!           [--watchdog-pid PID]    started by the watchdog
//!           [--screens 1200x800,900x700]  (with --window) test windows standing in for
//!                                   several monitors; closing one is unplugging it
//!           [--diagnostics]         show the diagnostics overlay (also Ctrl+Shift+F12)
//!           [--enable]              turn Deskboxes back on if "Disable Deskboxes" turned it off
//!                                   (the Start menu shortcut and setup's "Open Deskboxes now";
//!                                   the autostart entry does not pass it, so off stays off)
//! deskboxes --restore-icons         panic command: stop Deskboxes, show Explorer's icons
//! deskboxes --uninstall             restore icons and remove the autostart entry
//! deskboxes --install-autostart     add the HKCU Run entry (starts the watchdog)
//! ```

#![windows_subsystem = "windows"]
#![recursion_limit = "256"]

mod anim;
mod app;
mod az;
mod covers;
mod draw;
mod droptarget;
mod events;
mod iconfx;
mod logger;
mod pages;
mod saved;
mod saver;
mod screens;
mod above;
mod folderbox;
mod update;
mod search;
mod textedit;
mod tuck;
mod template;
mod transfer;
mod uia;
mod views;
mod wallcache;
mod worker;

use deskboxes_shell::hide;
use std::path::PathBuf;
use std::time::Instant;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Debug, Default)]
pub struct Args {
    pub desktop_dir: Option<PathBuf>,
    /// Fixture mode: a second folder standing in for the Public desktop (duplicate names).
    pub public_dir: Option<PathBuf>,
    pub window: bool,
    /// Only the first monitor's screen (measurements that must not depend on the others).
    pub one_screen: bool,
    pub no_hide: bool,
    pub profile: Option<PathBuf>,
    pub exit_after: Option<u32>,
    pub stats_out: Option<PathBuf>,
    pub scene_out: Option<PathBuf>,
    pub no_ui_ops: bool,
    pub no_host: bool,
    pub host: Option<PathBuf>,
    pub watchdog_pid: Option<u32>,
    pub restore_icons: bool,
    pub uninstall: bool,
    pub install_autostart: bool,
    pub diagnostics: bool,
    pub enable: bool,
    /// Sort the icons into the default boxes after the first scan, as a fresh install does
    /// (for fixture tests).
    pub first_run_template: bool,
    /// Open Settings once the shell host is ready (for checks of the Settings window).
    pub open_settings: bool,
    /// Fixture monitors (`--screens 1200x800,900x700`, with --window).
    pub screens: Option<String>,
}

fn parse_args() -> Args {
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut a = Args::default();
    let mut i = 0;
    let next = |i: &mut usize| {
        *i += 1;
        v.get(*i).cloned().unwrap_or_default()
    };
    while i < v.len() {
        match v[i].as_str() {
            "--desktop-dir" => a.desktop_dir = Some(next(&mut i).into()),
            "--public-dir" => a.public_dir = Some(next(&mut i).into()),
            "--window" => a.window = true,
            "--one-screen" => a.one_screen = true,
            "--no-hide" => a.no_hide = true,
            "--profile" => a.profile = Some(next(&mut i).into()),
            "--exit-after" => a.exit_after = next(&mut i).parse().ok(),
            "--stats-out" => a.stats_out = Some(next(&mut i).into()),
            "--scene-out" => a.scene_out = Some(next(&mut i).into()),
            "--no-ui-ops" => a.no_ui_ops = true,
            "--no-host" => a.no_host = true,
            "--host" => a.host = Some(next(&mut i).into()),
            "--watchdog-pid" => a.watchdog_pid = next(&mut i).parse().ok(),
            "--restore-icons" => a.restore_icons = true,
            "--uninstall" => a.uninstall = true,
            "--install-autostart" => a.install_autostart = true,
            "--diagnostics" => a.diagnostics = true,
            "--enable" => a.enable = true,
            "--open-settings" => a.open_settings = true,
            "--first-run-template" => a.first_run_template = true,
            "--screens" => a.screens = Some(next(&mut i)),
            other => eprintln!("unknown argument {other}"),
        }
        i += 1;
    }
    a
}

pub struct Paths {
    pub settings: PathBuf,
    pub layouts: PathBuf,
    pub backups: PathBuf,
    pub snapshot: PathBuf,
    pub logs: PathBuf,
    /// Downloaded updates (update.rs).
    pub update: PathBuf,
}

fn env_dir(var: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Deskboxes")
}

impl Paths {
    fn new(a: &Args) -> Paths {
        let logs = env_dir("LOCALAPPDATA").join("logs");
        let (roam, local) = match (&a.profile, &a.desktop_dir) {
            (Some(p), _) => (p.clone(), p.clone()),
            (None, Some(_)) => {
                let p = env_dir("LOCALAPPDATA").join("fixture-profile");
                (p.clone(), p)
            }
            (None, None) => (env_dir("APPDATA"), env_dir("LOCALAPPDATA")),
        };
        Paths {
            settings: roam.join("settings.json"),
            layouts: roam.join("layouts"),
            backups: roam.join("backups"),
            snapshot: local.join("snapshot.bin"),
            update: local.join("update"),
            logs,
        }
    }
}

pub const RUN_VALUE: &str = "Deskboxes";

/// Stop the watchdog and the core (politely, then forcibly), then show Explorer's icons.
fn restore_icons_cmd() -> i32 {
    deskboxes_shell::com_sta();
    unsafe {
        for (class, wait) in [(w!("DeskboxesWatchdog"), 2000u32), (w!("DeskboxesCore"), 2000u32)] {
            while let Ok(h) = FindWindowW(class, None) {
                let mut pid = 0u32;
                GetWindowThreadProcessId(h, Some(&mut pid));
                let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
                if let Ok(p) = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid) {
                    if WaitForSingleObject(p, wait) != WAIT_OBJECT_0 {
                        log::warn!("pid {pid} did not exit in {wait} ms; terminating");
                        let _ = TerminateProcess(p, 1);
                        let _ = WaitForSingleObject(p, 2000);
                    }
                    let _ = CloseHandle(p);
                } else {
                    break;
                }
            }
        }
    }
    let s = hide::restore();
    log::info!("--restore-icons: {s:?}");
    println!("native icons visible={} ({s:?})", s.visible());
    if s.visible() {
        0
    } else {
        2
    }
}

fn token_elevated() -> bool {
    use windows::Win32::Security::*;
    unsafe {
        let mut tok = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut n = 0u32;
        let ok = GetTokenInformation(tok, TokenElevation, Some(&mut e as *mut _ as _), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut n).is_ok();
        let _ = CloseHandle(tok);
        ok && e.TokenIsElevated != 0
    }
}

fn main() {
    // Never a crash dialog or a "drive not ready" box on the user's screen (the watchdog handles crashes).
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::*;
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    }
    let t0 = Instant::now();
    let args = parse_args();
    let paths = Paths::new(&args);
    logger::init(&paths.logs, t0);
    log::info!(
        "deskboxes {} start args={:?} elevated={} ({:.0} ms after the process was created)",
        env!("CARGO_PKG_VERSION"),
        std::env::args().skip(1).collect::<Vec<_>>(),
        token_elevated(),
        ms_since_process_created(t0)
    );
    if args.restore_icons {
        std::process::exit(restore_icons_cmd());
    }
    if args.uninstall {
        let code = restore_icons_cmd();
        deskboxes_shell::autostart::disable();
        deskboxes_shell::turnon::remove_verb();
        log::info!("--uninstall: autostart removed");
        std::process::exit(code);
    }
    if args.install_autostart {
        let exe = std::env::current_exe().unwrap_or_default();
        let wd = exe.with_file_name("deskboxes-watchdog.exe");
        let r = deskboxes_shell::autostart::enable(&wd);
        log::info!("--install-autostart: {r:?}");
        std::process::exit(if r.is_ok() { 0 } else { 1 });
    }
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = windows::Win32::System::Ole::OleInitialize(None);
    }
    allow_dark_dialogs();
    let code = app::run(args, paths, t0);
    transfer::respawn_if_wanted();
    log::logger().flush();
    std::process::exit(code);
}

/// Time from the process's creation to `t0` (the start of `main`): loading the program and the
/// DLLs it imports, before any of its code runs.
fn ms_since_process_created(t0: Instant) -> f64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).is_err() {
            return 0.0;
        }
        let now = windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime();
        let ft = |f: FILETIME| ((f.dwHighDateTime as i64) << 32) | f.dwLowDateTime as i64;
        (ft(now) - ft(c)) as f64 / 1e4 - t0.elapsed().as_secs_f64() * 1000.0
    }
}

/// Windows' own dialogs shown from this process (copy progress, delete and replace confirmations,
/// properties) follow the system's dark mode only when the process opts in, as Explorer does:
/// uxtheme's SetPreferredAppMode (ordinal 135; 1 = follow the system theme), then FlushMenuThemes
/// (ordinal 136). Without it they were light on a dark desktop (2026-11-20).
fn allow_dark_dialogs() {
    use windows::core::{w, PCSTR};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
    unsafe {
        let Ok(ux) = LoadLibraryExW(w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) else { return };
        if let Some(f) = GetProcAddress(ux, PCSTR(135 as *const u8)) {
            let set: extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set(1);
        }
        if let Some(f) = GetProcAddress(ux, PCSTR(136 as *const u8)) {
            let flush: extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}
