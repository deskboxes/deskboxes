//! What setup does to the PC. Everything is per user (HKCU, the user's own folders); only the
//! desktop-menu package asks for administrator rights (once, and it is optional), and nothing
//! outside these is touched:
//!
//! | What | Where |
//! |---|---|
//! | Program files | `%LOCALAPPDATA%\Programs\Deskboxes` (or the folder chosen), listed in `install.json` |
//! | Uninstaller | `uninstall.exe` there (this program without its payload) |
//! | Apps list entry | `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Deskboxes` |
//! | Autostart | Task Scheduler logon task `Deskboxes` for this user (no delay, above-normal priority, normal rights), else the `HKCU\...\Run` value `Deskboxes` (deskboxes_shell::autostart, as the app's own Settings switch does) |
//! | Desktop menu | the sparse package `Deskboxes.DesktopMenu` (files in the install folder): "Turn on Deskboxes" in Windows 11's desktop menu while Deskboxes is off. Unsigned, so registering it needs admin (one prompt; skipped if refused) |
//! | Start menu | `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Deskboxes.lnk` |
//!
//! The user's data (`%APPDATA%\Deskboxes`, `%LOCALAPPDATA%\Deskboxes`) is only removed when the
//! user asks for it in the uninstaller.

use crate::payload::Payload;
use deskboxes_shell::{autostart, hide, reg};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::core::{w, Interface, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Deskboxes";
pub const RUN_VALUE: &str = "Deskboxes";
pub const MANIFEST: &str = "install.json";
pub const UNINSTALLER: &str = "uninstall.exe";
pub const WATCHDOG: &str = "deskboxes-watchdog.exe";
pub const CORE: &str = "deskboxes.exe";
pub const SHELLHOST: &str = "deskboxes-shellhost.exe";

// ---------------------------------------------------------------------------------------------
// Log: %TEMP%\deskboxes-setup.log (the install folder and the data folders may be going away).

pub fn log(msg: &str) {
    use std::io::Write;
    let p = std::env::temp_dir().join("deskboxes-setup.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "{t} pid={} {msg}", std::process::id());
    }
}

// ---------------------------------------------------------------------------------------------
// Where things are.

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s.map(PathBuf::from)
    }
}

/// `%LOCALAPPDATA%\Programs\Deskboxes`.
pub fn default_dir() -> PathBuf {
    known_folder(&FOLDERID_UserProgramFiles)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Programs")))
        .unwrap_or_else(std::env::temp_dir)
        .join("Deskboxes")
}

pub fn start_menu_link() -> Option<PathBuf> {
    known_folder(&FOLDERID_Programs).map(|p| p.join("Deskboxes.lnk"))
}

/// The user's Deskboxes data: roaming (settings, layouts, backups) and local (snapshot, logs).
/// `root` (tests): `<root>\Roaming\Deskboxes` and `<root>\Local\Deskboxes` instead.
pub fn data_dirs(root: Option<&Path>) -> Vec<PathBuf> {
    match root {
        Some(r) => vec![r.join("Roaming").join("Deskboxes"), r.join("Local").join("Deskboxes")],
        None => ["APPDATA", "LOCALAPPDATA"].iter().filter_map(std::env::var_os).map(|p| PathBuf::from(p).join("Deskboxes")).collect(),
    }
}

/// Folder the user picked → where Deskboxes goes: `<picked>\Deskboxes`, unless the picked folder
/// is already called Deskboxes.
pub fn dir_for_picked(p: &Path) -> PathBuf {
    if p.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case("Deskboxes")).unwrap_or(false) {
        p.to_path_buf()
    } else {
        p.join("Deskboxes")
    }
}

#[derive(Clone, Debug)]
pub struct Installed {
    pub dir: PathBuf,
    pub version: String,
}

pub fn installed() -> Option<Installed> {
    let dir = reg::get_string(UNINSTALL_KEY, "InstallLocation")?;
    let version = reg::get_string(UNINSTALL_KEY, "DisplayVersion").unwrap_or_default();
    Some(Installed { dir: PathBuf::from(dir), version })
}

pub fn autostart_on() -> bool {
    autostart::is_on()
}

// ---------------------------------------------------------------------------------------------
// The desktop-menu package ("Turn on Deskboxes" in Windows 11's main desktop menu).

pub use deskboxes_shell::turnon::{MENU_PACKAGE, MENU_PFN};
use deskboxes_shell::turnon::MENU_PREFS;
pub const MENU_MSIX: &str = "deskboxes-menu.msix";

fn powershell(script: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new("powershell.exe");
    c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]).creation_flags(0x0800_0000);
    c
}

/// Where the menu package's files are registered from, if it is registered for this user.
pub fn menu_package_location() -> Option<PathBuf> {
    deskboxes_shell::turnon::menu_package_dir()
}

/// Register the menu package from `dir` (Windows 11 only). It is unsigned, which Windows accepts
/// only from an administrator: one UAC prompt (`ask`), skipped if refused. Returns whether it is
/// registered from `dir` afterwards.
pub fn register_menu_package(dir: &Path, ask: bool) -> bool {
    if windows_build() < 22000 || !dir.join(MENU_MSIX).exists() {
        return false;
    }
    if menu_package_location().map(|l| same_dir(&l, dir)).unwrap_or(false) {
        log("menu package: already registered here");
        show_menu_on_top();
        return true;
    }
    if !ask {
        log("menu package: not registered; not asking now");
        return false;
    }
    let q = |p: &Path| p.display().to_string().replace('\'', "''");
    let script = format!(
        "try {{ Get-AppxPackage -Name {MENU_PACKAGE} | Remove-AppxPackage -ErrorAction SilentlyContinue; Add-AppxPackage -Path '{}' -ExternalLocation '{}' -AllowUnsigned -ErrorAction Stop; exit 0 }} catch {{ Add-Content -Path '{}' -Value ('menu package (elevated): ' + $_); exit 1 }}",
        q(&dir.join(MENU_MSIX)),
        q(dir),
        q(&std::env::temp_dir().join("deskboxes-setup.log"))
    );
    let code = if token_elevated() { powershell(&script).status().ok().and_then(|s| s.code()) } else { run_elevated_powershell(&script) };
    log(&format!("menu package: elevated registration exit {code:?}"));
    let ok = menu_package_location().map(|l| same_dir(&l, dir)).unwrap_or(false);
    if ok {
        show_menu_on_top();
    }
    ok
}

fn windows_build() -> u32 {
    reg_hklm_string(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuildNumber").and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn reg_hklm_string(key: &str, value: &str) -> Option<String> {
    let (k, v) = (HSTRING::from(key), HSTRING::from(value));
    let mut buf = [0u16; 64];
    let mut sz = (buf.len() * 2) as u32;
    unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, PCWSTR(k.as_ptr()), PCWSTR(v.as_ptr()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut sz)).ok().ok()? };
    let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buf[..n]))
}

fn show_menu_on_top() {
    deskboxes_shell::turnon::pin_modern_verb();
}

/// powershell.exe `script` as administrator (a UAC prompt), hidden; its exit code, or None if the
/// prompt was refused or it did not finish within two minutes.
fn run_elevated_powershell(script: &str) -> Option<i32> {
    let params = HSTRING::from(format!("-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -Command \"{}\"", script.replace('"', "\\\"")));
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("runas"),
        lpFile: w!("powershell.exe"),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut sei) {
            log(&format!("menu package: no administrator rights ({e})"));
            return None;
        }
        if sei.hProcess.is_invalid() {
            return None;
        }
        let w = WaitForSingleObject(sei.hProcess, 120_000);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(sei.hProcess, &mut code);
        let _ = CloseHandle(sei.hProcess);
        (w == WAIT_OBJECT_0).then_some(code as i32)
    }
}

/// Remove the menu package (one's own registration needs no admin to remove).
pub fn unregister_menu_package() {
    if menu_package_location().is_some() {
        let r = powershell(&format!("Get-AppxPackage -Name {MENU_PACKAGE} | Remove-AppxPackage")).status();
        log(&format!("menu package removed: {r:?}"));
    }
    delete_key(&format!(r"{MENU_PREFS}\extensions\{MENU_PFN}"));
}

#[derive(Serialize, Deserialize, Default)]
struct Manifest {
    version: String,
    files: Vec<String>,
}

fn read_manifest(dir: &Path) -> Option<Manifest> {
    serde_json::from_slice(&std::fs::read(dir.join(MANIFEST)).ok()?).ok()
}

// ---------------------------------------------------------------------------------------------
// Progress reports from the worker thread to the window.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Closing,
    Copying,
    Registering,
    Starting,
    Restoring,
    Cleanup,
    Removing,
}

#[derive(Clone, Debug)]
pub enum Report {
    Step(Step),
    /// Copy progress: files done of total.
    Copy { files: usize, files_total: usize },
    /// The whole job's progress, 0..1.
    Fraction(f32),
    /// Finished; whether Deskboxes was started.
    Done(bool),
    Failed(Failure),
}

#[derive(Clone, Debug)]
pub enum Failure {
    NoPayload,
    StillRunning,
    Location(String),
    Other(String),
}

// ---------------------------------------------------------------------------------------------
// Processes.

fn image_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])))
    }
}

/// Whether two paths name the same folder, however spelled (letter case, short names, a
/// trailing separator, a link to it). Paths that do not exist are compared as text.
pub fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x.to_string_lossy().eq_ignore_ascii_case(&y.to_string_lossy()),
        _ => a.to_string_lossy().trim_end_matches(['\\', '/']).eq_ignore_ascii_case(b.to_string_lossy().trim_end_matches(['\\', '/'])),
    }
}

/// The programs Deskboxes runs from `dir`: the ones its install.json lists and its own known
/// ones (a folder without install.json: an older or a development copy). Relative to `dir`.
fn our_programs(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = read_manifest(dir).map(|m| m.files).unwrap_or_default();
    names.extend([CORE, WATCHDOG, SHELLHOST, UNINSTALLER].map(String::from));
    names.retain(|f| f.to_lowercase().ends_with(".exe"));
    names.sort_by_key(|f| f.to_lowercase());
    names.dedup_by_key(|f| f.to_lowercase());
    names
}

/// Running Deskboxes programs from `dir` (only those: another program kept in the same folder
/// is never stopped).
fn processes_in(dir: &Path) -> Vec<u32> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    let ours: Vec<PathBuf> = our_programs(dir).iter().map(|f| dir.join(f)).collect();
    let names: std::collections::HashSet<String> = ours.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().to_lowercase()).collect();
    let canon: Vec<String> = ours.iter().filter_map(|p| std::fs::canonicalize(p).ok()).map(|p| p.to_string_lossy().to_lowercase()).collect();
    let mut out = vec![];
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let name = String::from_utf16_lossy(&e.szExeFile[..e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0)]).to_lowercase();
            if e.th32ProcessID != std::process::id() && names.contains(&name) {
                if let Some(p) = image_path(e.th32ProcessID) {
                    let pl = std::fs::canonicalize(&p).map(|c| c.to_string_lossy().to_lowercase()).unwrap_or_else(|_| p.to_string_lossy().to_lowercase());
                    if canon.contains(&pl) {
                        out.push(e.th32ProcessID);
                    }
                }
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Is Deskboxes running (from anywhere)?
pub fn app_running() -> bool {
    unsafe { FindWindowW(w!("DeskboxesWatchdog"), None).is_ok() || FindWindowW(w!("DeskboxesCore"), None).is_ok() }
}

fn wait_or_kill(pid: u32, wait_ms: u32) -> bool {
    unsafe {
        let Ok(p) = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid) else { return false };
        let mut ok = WaitForSingleObject(p, wait_ms) == WAIT_OBJECT_0;
        if !ok {
            log(&format!("pid {pid} did not exit in {wait_ms} ms; terminating"));
            let _ = TerminateProcess(p, 1);
            ok = WaitForSingleObject(p, 3000) == WAIT_OBJECT_0;
        }
        let _ = CloseHandle(p);
        ok
    }
}

/// Stop Deskboxes the way `deskboxes --restore-icons` does: WM_CLOSE to the watchdog (so it does
/// not restart the core), then to the core (it restores the icons on a clean exit), terminating
/// what does not exit. Then Deskboxes' other programs still running from `dir` (the shell host).
/// Finally make sure Explorer's icons are visible. Returns whether Deskboxes was running.
pub fn stop_app(dir: Option<&Path>) -> Result<bool, Failure> {
    let mut was = false;
    unsafe {
        for class in [w!("DeskboxesWatchdog"), w!("DeskboxesCore")] {
            for _ in 0..8 {
                let Ok(h) = FindWindowW(class, None) else { break };
                was = true;
                let mut pid = 0u32;
                GetWindowThreadProcessId(h, Some(&mut pid));
                let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
                if !wait_or_kill(pid, 3000) {
                    log(&format!("could not stop pid {pid}"));
                    return Err(Failure::StillRunning);
                }
            }
            if FindWindowW(class, None).is_ok() {
                return Err(Failure::StillRunning);
            }
        }
    }
    if let Some(d) = dir {
        let t0 = Instant::now();
        loop {
            let left = processes_in(d);
            if left.is_empty() {
                break;
            }
            was = true;
            if t0.elapsed() > Duration::from_secs(2) {
                for pid in &left {
                    if !wait_or_kill(*pid, 0) {
                        return Err(Failure::StillRunning);
                    }
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let s = hide::restore();
    log(&format!("stop_app: was running={was}, icons {s:?}"));
    Ok(was)
}

// ---------------------------------------------------------------------------------------------
// Files.

/// Move `p` aside as `<name>.old~N`: a file in use cannot be replaced or deleted, but it can be
/// renamed (deleted at the end, or by the next setup run). Returns where it went.
fn move_aside(p: &Path) -> std::io::Result<PathBuf> {
    let mut last = std::io::Error::new(std::io::ErrorKind::AlreadyExists, "no free .old~N name");
    for n in 0..100 {
        let old = PathBuf::from(format!("{}.old~{n}", p.display()));
        if old.exists() && std::fs::remove_file(&old).is_err() {
            continue;
        }
        match std::fs::rename(p, &old) {
            Ok(()) => return Ok(old),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Files an earlier setup moved aside (see `move_aside`): `<file>.old~N` for the files Deskboxes
/// installs (`files`, relative to `dir`), nothing else.
fn remove_old_leftovers(dir: &Path, files: &[String]) {
    let mut by_dir: std::collections::HashMap<PathBuf, std::collections::HashSet<String>> = Default::default();
    for f in files {
        let p = dir.join(f);
        if let (Some(parent), Some(name)) = (p.parent(), p.file_name()) {
            by_dir.entry(parent.to_path_buf()).or_default().insert(name.to_string_lossy().to_lowercase());
        }
    }
    for (d, names) in by_dir {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let Some((base, n)) = name.rsplit_once(".old~") else { continue };
            if !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) && names.contains(base) && e.path().is_file() {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Folders a setup unpacked into (`.deskboxes-setup-<pid>`) and did not get to remove.
fn remove_old_staging(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(pid) = name.strip_prefix(STAGING) {
            if !pid.is_empty() && pid.bytes().all(|c| c.is_ascii_digit()) && e.path().is_dir() {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

const STAGING: &str = ".deskboxes-setup-";

/// Put the unpacked files (`rel`, under `stage`) in place of the installed ones. Each installed
/// file is moved aside first; if any step fails, everything done so far is put back. Returns the
/// files moved aside (to delete once the install is through).
fn replace_files(stage: &Path, dir: &Path, rel: &[String]) -> std::io::Result<Vec<PathBuf>> {
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = vec![];
    let undo = |done: &[(PathBuf, Option<PathBuf>)]| {
        for (dst, aside) in done.iter().rev() {
            let _ = std::fs::remove_file(dst);
            if let Some(a) = aside {
                if let Err(e) = std::fs::rename(a, dst) {
                    log(&format!("could not put back {}: {e}", dst.display()));
                }
            }
        }
    };
    for f in rel {
        let (src, dst) = (stage.join(f), dir.join(f));
        let step = (|| -> std::io::Result<Option<PathBuf>> {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let aside = if dst.exists() { Some(move_aside(&dst)?) } else { None };
            if let Err(e) = std::fs::rename(&src, &dst) {
                if let Some(a) = &aside {
                    let _ = std::fs::rename(a, &dst);
                }
                return Err(e);
            }
            Ok(aside)
        })();
        match step {
            Ok(aside) => done.push((dst, aside)),
            Err(e) => {
                log(&format!("replacing {} failed ({e}); putting the installed files back", dst.display()));
                undo(&done);
                return Err(e);
            }
        }
    }
    Ok(done.into_iter().filter_map(|(_, a)| a).collect())
}

/// Remove `dir` and its parents up to (not including) `stop` while they are empty.
fn remove_empty_dirs(dir: &Path, stop: &Path) {
    let mut d = dir.to_path_buf();
    while d.starts_with(stop) && d != stop {
        if std::fs::remove_dir(&d).is_err() {
            break;
        }
        match d.parent() {
            Some(p) => d = p.to_path_buf(),
            None => break,
        }
    }
}

/// Can we write in `dir`? (Creates it.)
fn check_location(dir: &Path) -> Result<(), Failure> {
    let probe = dir.join(".deskboxes-setup-probe");
    std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&probe, b"ok")).map_err(|e| Failure::Location(e.to_string()))?;
    let _ = std::fs::remove_file(probe);
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Registry, shortcut.

fn set_value(key: &str, name: &str, ty: REG_VALUE_TYPE, data: &[u8]) -> windows::core::Result<()> {
    let (k, n) = (HSTRING::from(key), HSTRING::from(name));
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr()), PCWSTR(n.as_ptr()), ty.0, Some(data.as_ptr() as _), data.len() as u32).ok() }
}

fn set_sz(key: &str, name: &str, s: &str) -> windows::core::Result<()> {
    let d: Vec<u8> = s.encode_utf16().chain(std::iter::once(0)).flat_map(|c| c.to_le_bytes()).collect();
    set_value(key, name, REG_SZ, &d)
}

fn set_dword(key: &str, name: &str, v: u32) -> windows::core::Result<()> {
    set_value(key, name, REG_DWORD, &v.to_le_bytes())
}

fn delete_key(key: &str) {
    let k = HSTRING::from(key);
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr()));
        let _ = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(k.as_ptr()));
    }
}

fn today() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}{:02}{:02}", t.wYear, t.wMonth, t.wDay)
}

fn register_uninstall(dir: &Path, version: &str, size_bytes: u64) -> windows::core::Result<()> {
    let d = dir.display().to_string();
    let un = dir.join(UNINSTALLER).display().to_string();
    set_sz(UNINSTALL_KEY, "DisplayName", "Deskboxes")?;
    set_sz(UNINSTALL_KEY, "DisplayVersion", version)?;
    set_sz(UNINSTALL_KEY, "DisplayIcon", &format!("{},0", dir.join(CORE).display()))?;
    set_sz(UNINSTALL_KEY, "Publisher", "Deskboxes")?;
    set_sz(UNINSTALL_KEY, "InstallLocation", &d)?;
    set_sz(UNINSTALL_KEY, "UninstallString", &format!("\"{un}\" --uninstall"))?;
    set_sz(UNINSTALL_KEY, "QuietUninstallString", &format!("\"{un}\" --uninstall --quiet"))?;
    set_sz(UNINSTALL_KEY, "InstallDate", &today())?;
    set_dword(UNINSTALL_KEY, "EstimatedSize", (size_bytes / 1024) as u32)?;
    set_dword(UNINSTALL_KEY, "NoModify", 1)?;
    set_dword(UNINSTALL_KEY, "NoRepair", 1)?;
    Ok(())
}

fn create_shortcut(link: &Path, target: &Path, args: &str, dir: &Path, icon: &Path, desc: &str) -> windows::core::Result<()> {
    unsafe {
        let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        sl.SetPath(&HSTRING::from(target.as_os_str()))?;
        sl.SetArguments(&HSTRING::from(args))?;
        sl.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        sl.SetIconLocation(&HSTRING::from(icon.as_os_str()), 0)?;
        sl.SetDescription(&HSTRING::from(desc))?;
        let pf: IPersistFile = sl.cast()?;
        if let Some(p) = link.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        pf.Save(&HSTRING::from(link.as_os_str()), true)?;
        SHChangeNotify(SHCNE_CREATE, SHCNF_PATHW, Some(HSTRING::from(link.as_os_str()).as_ptr() as _), None);
    }
    Ok(())
}

fn remove_shortcut_if_ours(dir: Option<&Path>) {
    let Some(link) = start_menu_link() else { return };
    if !link.exists() {
        return;
    }
    // Ours if it points into our folder (or we don't know the folder), or its target is gone.
    let target = shortcut_target(&link);
    let ours = match (&target, dir) {
        (Some(t), Some(d)) => t.starts_with(d) || !t.exists(),
        _ => true,
    };
    if ours {
        let _ = std::fs::remove_file(&link);
        unsafe { SHChangeNotify(SHCNE_DELETE, SHCNF_PATHW, Some(HSTRING::from(link.as_os_str()).as_ptr() as _), None) };
    }
}

fn shortcut_target(link: &Path) -> Option<PathBuf> {
    unsafe {
        let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let pf: IPersistFile = sl.cast().ok()?;
        pf.Load(&HSTRING::from(link.as_os_str()), STGM_READ).ok()?;
        let mut buf = [0u16; 1024];
        sl.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    }
}

/// Is the Run value ours (points into `dir`, or at a watchdog that no longer exists)?
fn run_value_ours(dir: &Path) -> bool {
    let Some(t) = autostart::target().or_else(|| reg::get_string(reg::RUN, RUN_VALUE)) else { return false };
    let p = PathBuf::from(t.trim_start_matches('"').split('"').next().unwrap_or(""));
    p.starts_with(dir) || !p.exists()
}

// ---------------------------------------------------------------------------------------------
// Starting Deskboxes as the user, never elevated.

pub fn token_elevated() -> bool {
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

/// Start `exe args` in `cwd`. When setup runs elevated ("Run as administrator"), Deskboxes must
/// not: it is started through Explorer's desktop (`IShellDispatch2::ShellExecute`), so it runs as
/// everything else the user starts.
pub fn launch(exe: &Path, args: &str, cwd: &Path) -> Result<(), String> {
    if token_elevated() {
        log("elevated: starting through Explorer");
        return deskboxes_shell::launch::via_explorer(exe, args, cwd).map_err(|e| e.to_string());
    }
    let mut c = std::process::Command::new(exe);
    for a in args.split_whitespace() {
        c.arg(a);
    }
    c.current_dir(cwd).spawn().map(|_| ()).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------------
// Install.

#[derive(Clone, Debug)]
pub struct InstallOptions {
    pub dir: PathBuf,
    pub autostart: bool,
    pub start_menu: bool,
    /// Start Deskboxes when done (always, if it was running before, except for an update).
    pub launch: bool,
    /// An update started by Deskboxes itself: the copy it closed was exiting anyway, so start
    /// Deskboxes again only when `launch` says so.
    pub update: bool,
    /// May ask for administrator rights (the desktop-menu package): the interactive installer
    /// only, never an update or a quiet install.
    pub ask_admin: bool,
    pub shortcut_desc: String,
}

pub fn install(p: &Payload, o: &InstallOptions, report: &dyn Fn(Report)) -> Result<bool, Failure> {
    log(&format!("install {} to {} (autostart={}, start menu={}, launch={})", p.header.app_version, o.dir.display(), o.autostart, o.start_menu, o.launch));
    let old = installed();
    // 1. Close Deskboxes (it may be running from here, from an older folder, or a development build).
    report(Report::Step(Step::Closing));
    report(Report::Fraction(0.0));
    let was_running = stop_app(Some(&o.dir))?;
    // (The same folder spelled differently is not another install.)
    let moved = old.as_ref().is_some_and(|old| !same_dir(&old.dir, &o.dir));
    if let Some(old) = old.as_ref().filter(|_| moved) {
        let _ = stop_app(Some(&old.dir));
    }
    check_location(&o.dir)?;
    let mut ours: Vec<String> = read_manifest(&o.dir).map(|m| m.files).unwrap_or_default();
    ours.extend(p.header.files.iter().map(|f| f.path.clone()));
    ours.push(UNINSTALLER.into());
    remove_old_leftovers(&o.dir, &ours);
    remove_old_staging(&o.dir);
    report(Report::Fraction(0.04));

    // 2. Files: all unpacked and checked in a folder beside the install first; only then do they
    // replace the installed ones (put back if that fails part way). A damaged setup file or a
    // full disk leaves the installed version as it was.
    report(Report::Step(Step::Copying));
    let total = p.header.total().max(1);
    let files_total = p.header.files.len();
    let mut files_done = 0usize;
    let stage = o.dir.join(format!("{STAGING}{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    let unpacked = (|| -> Result<(), String> {
        std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
        p.extract(
            &stage,
            |path, data| std::fs::write(path, data),
            |bytes| {
                files_done += 1;
                report(Report::Copy { files: files_done.min(files_total), files_total });
                report(Report::Fraction(0.04 + 0.86 * bytes as f32 / total as f32));
            },
        )
        .map_err(|e| e.to_string())?;
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        let un = crate::payload::program_bytes(&me).map_err(|e| e.to_string())?;
        std::fs::write(stage.join(UNINSTALLER), &un).map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(e) = unpacked {
        let _ = std::fs::remove_dir_all(&stage);
        log(&format!("unpacking failed: {e}; the installed files are unchanged"));
        return Err(Failure::Other(e));
    }
    let un_len = std::fs::metadata(stage.join(UNINSTALLER)).map(|m| m.len()).unwrap_or(0);
    let mut rel: Vec<String> = p.header.files.iter().map(|f| f.path.clone()).collect();
    rel.push(UNINSTALLER.into());
    let replaced = replace_files(&stage, &o.dir, &rel);
    let _ = std::fs::remove_dir_all(&stage);
    let leftovers = replaced.map_err(|e| Failure::Other(e.to_string()))?;
    report(Report::Fraction(0.91));

    // Files of the previous version that this one no longer has.
    let new_files: std::collections::HashSet<String> = p.header.files.iter().map(|f| f.path.to_lowercase()).collect();
    if let Some(m) = read_manifest(&o.dir) {
        for f in m.files.iter().filter(|f| !new_files.contains(&f.to_lowercase())) {
            let fp = o.dir.join(f);
            if std::fs::remove_file(&fp).is_ok() {
                if let Some(parent) = fp.parent() {
                    remove_empty_dirs(parent, &o.dir);
                }
            }
        }
    }
    let m = Manifest { version: p.header.app_version.clone(), files: p.header.files.iter().map(|f| f.path.clone()).collect() };
    std::fs::write(o.dir.join(MANIFEST), serde_json::to_vec_pretty(&m).unwrap_or_default()).map_err(|e| Failure::Other(e.to_string()))?;
    for l in leftovers {
        let _ = std::fs::remove_file(l);
    }
    // Moved to another folder: the old one goes (only the files it listed).
    if let Some(old) = old.as_ref().filter(|_| moved) {
        if old.dir.join(MANIFEST).exists() {
            log(&format!("removing the previous install at {}", old.dir.display()));
            remove_files(&old.dir);
        }
    }
    report(Report::Fraction(0.93));

    // 3. Windows integration.
    report(Report::Step(Step::Registering));
    let wd = o.dir.join(WATCHDOG);
    if o.autostart {
        let how = autostart::enable(&wd).map_err(|e| Failure::Other(e.to_string()))?;
        log(&format!("autostart: {how}"));
    } else if run_value_ours(&o.dir) || old.as_ref().map(|i| run_value_ours(&i.dir)).unwrap_or(false) {
        autostart::disable();
    }
    let menu = register_menu_package(&o.dir, o.ask_admin);
    log(&format!("desktop menu package registered: {menu}"));
    if o.start_menu {
        if let Some(link) = start_menu_link() {
            // `--enable`: opening Deskboxes from Start turns it back on after "Disable Deskboxes".
            create_shortcut(&link, &wd, "--launch --enable", &o.dir, &o.dir.join(CORE), &o.shortcut_desc).map_err(|e| Failure::Other(format!("Start menu shortcut: {e}")))?;
        }
    } else {
        remove_shortcut_if_ours(Some(&o.dir));
    }
    let size: u64 = p.header.total() + un_len;
    register_uninstall(&o.dir, &p.header.app_version, size).map_err(|e| Failure::Other(e.to_string()))?;
    report(Report::Fraction(0.97));

    // 4. Start it.
    let start = o.launch || (was_running && !o.update);
    if start {
        report(Report::Step(Step::Starting));
        launch(&wd, "--launch --enable", &o.dir).map_err(Failure::Other)?;
        // Give the core a moment so "Done" shows when the desktop has switched.
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(4) && !app_running() {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    report(Report::Fraction(1.0));
    log("install done");
    Ok(start)
}

/// Remove the files `dir`'s install.json lists, the manifest and the uninstaller (unless it is
/// the running exe), then the folders they leave empty.
fn remove_files(dir: &Path) -> Vec<PathBuf> {
    let mut failed = vec![];
    let me = std::env::current_exe().ok();
    let mut files: Vec<String> = read_manifest(dir).map(|m| m.files).unwrap_or_default();
    files.push(MANIFEST.into());
    files.push(UNINSTALLER.into());
    remove_old_staging(dir);
    for f in &files {
        let p = dir.join(f);
        if Some(&p) == me.as_ref() || !p.exists() {
            continue;
        }
        if std::fs::remove_file(&p).is_err() {
            // In use (Explorer keeps the desktop-menu DLL loaded for a while): a file in use can
            // still be moved; out of the folder it goes, to the temp folder Windows cleans.
            let aside = std::env::temp_dir().join(format!("deskboxes-removed-{}-{}", std::process::id(), f.replace(['\\', '/'], "_")));
            if std::fs::rename(&p, &aside).is_err() {
                failed.push(p.clone());
            }
        }
        if let Some(parent) = p.parent() {
            remove_empty_dirs(parent, dir);
        }
    }
    remove_old_leftovers(dir, &files);
    let _ = std::fs::remove_dir(dir);
    failed
}

// ---------------------------------------------------------------------------------------------
// Uninstall.

#[derive(Clone, Debug)]
pub struct UninstallOptions {
    pub dir: PathBuf,
    pub remove_data: bool,
    pub data_root: Option<PathBuf>,
}

pub fn uninstall(o: &UninstallOptions, report: &dyn Fn(Report)) -> Result<(), Failure> {
    log(&format!("uninstall from {} (remove data={})", o.dir.display(), o.remove_data));
    report(Report::Step(Step::Restoring));
    report(Report::Fraction(0.0));
    stop_app(Some(&o.dir))?;
    report(Report::Fraction(0.3));

    report(Report::Step(Step::Cleanup));
    if run_value_ours(&o.dir) {
        autostart::disable();
    }
    unregister_menu_package();
    deskboxes_shell::turnon::remove_verb();
    remove_shortcut_if_ours(Some(&o.dir));
    delete_key(UNINSTALL_KEY);
    report(Report::Fraction(0.45));

    report(Report::Step(Step::Removing));
    let failed = remove_files(&o.dir);
    report(Report::Fraction(0.85));
    if o.remove_data {
        for d in data_dirs(o.data_root.as_deref()) {
            if d.exists() {
                log(&format!("removing data {}", d.display()));
                if let Err(e) = std::fs::remove_dir_all(&d) {
                    log(&format!("could not remove {}: {e}", d.display()));
                }
            }
        }
    }
    report(Report::Fraction(1.0));
    if !failed.is_empty() {
        log(&format!("could not remove: {failed:?}"));
        return Err(Failure::Other(format!("{} file(s) could not be removed, e.g. {}", failed.len(), failed[0].display())));
    }
    log("uninstall done");
    Ok(())
}

/// The uninstaller runs from the folder it removes: after it exits, a hidden `cmd` deletes it and
/// the folder (if empty).
pub fn schedule_self_delete(dir: &Path) {
    let Ok(me) = std::env::current_exe() else { return };
    if !me.starts_with(dir) {
        return;
    }
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let script = format!("ping -n 3 127.0.0.1 >nul & del /f /q \"{}\" & rmdir \"{}\"", me.display(), dir.display());
    let r = std::process::Command::new("cmd.exe").args(["/d", "/c"]).raw_arg(&script).current_dir(std::env::temp_dir()).creation_flags(CREATE_NO_WINDOW).spawn();
    log(&format!("self-delete scheduled: {}", r.is_ok()));
}
