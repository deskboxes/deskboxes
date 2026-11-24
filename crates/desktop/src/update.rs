//! Updates (Settings › General › Updates). An installed copy looks once a day for
//! `latest.json` in the newest release of the public `deskboxes/releases` repository:
//!
//! ```json
//! { "version": "0.2.0", "url": "https://github.com/deskboxes/releases/releases/download/v0.2.0/DeskboxesSetup.exe",
//!   "sha256": "…64 hex…", "size": 22000000 }
//! ```
//!
//! A newer version is downloaded in the background into `%LOCALAPPDATA%\Deskboxes\update`, and
//! kept only if its size and SHA-256 match. The next time Deskboxes starts (or at once with
//! "Install now") the core starts that installer with `--quiet --update` and exits; the installer
//! closes what is left of Deskboxes, replaces the files and starts it again. A failed install is
//! tried twice at most. Copies that are not installed (a build folder) never update themselves.
//! Only HTTPS to github.com (redirects to GitHub's download hosts are followed by WinHTTP);
//! `DBX_UPDATE_FEED` points tests at a local feed (plain http only on 127.0.0.1).
//!
//! Settings › Updates also shows what changed: the newest release's notes, read from GitHub's
//! API with each check (best effort, kept in `state.json` for offline use; `DBX_UPDATE_NOTES`
//! points tests at a local copy).

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const FEED: &str = "https://github.com/deskboxes/releases/releases/latest/download/latest.json";
/// The newest release (its notes): GitHub's REST API.
const NOTES: &str = "https://api.github.com/repos/deskboxes/releases/releases/latest";
const DAY: u64 = 24 * 3600;
const MAX_SETUP: u64 = 200 * 1024 * 1024;

#[derive(Deserialize)]
struct Feed {
    version: String,
    url: String,
    sha256: String,
    size: u64,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct State {
    /// When the feed was last read successfully (Unix seconds).
    last_check: u64,
    /// A downloaded, verified installer waiting for the next start.
    pending: Option<Pending>,
    /// Install attempts of the pending version (2 at most).
    attempts: u32,
    /// The newest release's notes, from the last check that could read them.
    notes: Option<Notes>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct Notes {
    version: String,
    body: String,
    /// "2026-11-19" (from GitHub's published_at).
    date: String,
}

/// GitHub's release object: only what is shown.
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Pending {
    version: String,
    file: String,
    sha256: String,
}

/// What Settings shows. `state`: "" (not checked yet), "checking", "uptodate", "downloading",
/// "ready", "error", "dev" (not an installed copy).
#[derive(Clone, Default, Serialize, PartialEq, Debug)]
pub struct Status {
    pub state: String,
    pub latest: String,
    pub checked: u64,
    pub error: String,
    /// While downloading: 0..1.
    pub progress: f64,
    /// What changed in the newest release (Markdown as written on GitHub), its version and date.
    pub notes: String,
    pub notes_version: String,
    pub notes_date: String,
}

static STATUS: Mutex<Option<Status>> = Mutex::new(None);
static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
static AUTO: AtomicBool = AtomicBool::new(true);
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// "0.10.2" > "0.9.9": numbers compared one by one.
pub fn newer(a: &str, than: &str) -> bool {
    let p = |s: &str| s.trim_start_matches('v').split(['.', '-']).map(|x| x.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (x, y) = (p(a), p(than));
    for i in 0..x.len().max(y.len()) {
        let (u, v) = (x.get(i).copied().unwrap_or(0), y.get(i).copied().unwrap_or(0));
        if u != v {
            return u > v;
        }
    }
    false
}

const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Deskboxes";

fn feed_url() -> String {
    std::env::var("DBX_UPDATE_FEED").unwrap_or_else(|_| FEED.to_string())
}

/// Runs from the folder the installer put it in (or a test feed is set).
pub fn installed_copy() -> bool {
    if std::env::var_os("DBX_UPDATE_FEED").is_some() {
        return true;
    }
    let Some(loc) = deskboxes_shell::reg::get_string(UNINSTALL_KEY, "InstallLocation") else { return false };
    let me = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_default();
    me.to_string_lossy().trim_end_matches('\\').eq_ignore_ascii_case(loc.trim_end_matches('\\'))
}

pub fn status() -> Status {
    STATUS.lock().unwrap_or_else(|p| p.into_inner()).clone().unwrap_or_default()
}

fn set_status(f: impl FnOnce(&mut Status)) {
    {
        let mut s = STATUS.lock().unwrap_or_else(|p| p.into_inner());
        f(s.get_or_insert_with(Status::default));
    }
    crate::events::post(crate::events::Ev::Update);
}

fn load_state(dir: &Path) -> State {
    std::fs::read(dir.join("state.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_state(dir: &Path, s: &State) {
    let _ = std::fs::create_dir_all(dir);
    let tmp = dir.join("state.json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(s).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join("state.json"));
    }
}

/// Start the background checks (installed copies only).
pub fn start(dir: PathBuf, auto: bool) {
    AUTO.store(auto, Ordering::SeqCst);
    if !installed_copy() {
        set_status(|s| s.state = "dev".into());
        return;
    }
    *DIR.lock().unwrap() = Some(dir.clone());
    let st = load_state(&dir);
    set_status(|s| {
        s.checked = st.last_check;
        if let Some(n) = &st.notes {
            s.notes = n.body.clone();
            s.notes_version = n.version.clone();
            s.notes_date = n.date.clone();
        }
        if let Some(p) = st.pending.as_ref().filter(|p| newer(&p.version, current()) && dir.join(&p.file).is_file()) {
            s.state = "ready".into();
            s.latest = p.version.clone();
        } else if st.last_check > 0 {
            // The last check found nothing newer, or found this version (now installed).
            s.state = "uptodate".into();
        }
    });
    let first = std::env::var("DBX_UPDATE_DELAY").ok().and_then(|v| v.parse().ok()).unwrap_or(120u64);
    std::thread::Builder::new()
        .name("update".into())
        .spawn(move || {
            let mut forced = wait(Duration::from_secs(first));
            loop {
                let due = AUTO.load(Ordering::SeqCst) && now().saturating_sub(load_state(&dir).last_check) >= DAY;
                if due || forced {
                    check(&dir);
                }
                forced = wait(Duration::from_secs(3600));
            }
        })
        .ok();
}

/// Sleep up to `d`; true when "Check now" woke us.
fn wait(d: Duration) -> bool {
    let (m, cv) = &WAKE;
    let g = m.lock().unwrap_or_else(|p| p.into_inner());
    let (mut g, _) = cv.wait_timeout_while(g, d, |w| !*w).unwrap_or_else(|p| p.into_inner());
    std::mem::replace(&mut *g, false)
}

pub fn set_auto(on: bool) {
    AUTO.store(on, Ordering::SeqCst);
}

pub fn check_now() {
    if DIR.lock().unwrap().is_none() {
        return;
    }
    let (m, cv) = &WAKE;
    *m.lock().unwrap_or_else(|p| p.into_inner()) = true;
    cv.notify_all();
}

fn check(dir: &Path) {
    set_status(|s| {
        s.state = "checking".into();
        s.error.clear();
        s.progress = 0.0;
    });
    match check_inner(dir) {
        Ok(()) => {}
        Err(e) => {
            log::warn!("update check failed: {e}");
            // A download that is already here stays ready; otherwise say what kind of trouble it was.
            // (Only one whose file is really there: a "ready" without an installer is no use.)
            let pending = load_state(dir).pending.filter(|p| newer(&p.version, current()) && dir.join(&p.file).is_file());
            set_status(|s| match pending {
                Some(p) => {
                    s.state = "ready".into();
                    s.latest = p.version;
                    s.error.clear();
                }
                None => {
                    s.state = "error".into();
                    s.error = error_kind(&e).into();
                }
            });
        }
    }
}

/// What Settings says about a failed check (the details go to the log): "net" (no connection),
/// "none" (nothing published yet), "server" (an HTTP error), "file" (the feed or the download
/// did not check out) or "other".
fn error_kind(e: &str) -> &'static str {
    if e.contains("(404)") {
        "none"
    } else if e.starts_with("HTTP ") {
        "server"
    } else if e.contains("0x80072E") || e.contains("0x80072F") || e.starts_with("connect ") {
        "net"
    } else if e.contains("SHA-256") || e.starts_with("size ") || e.starts_with("latest.json") || e.contains("larger than") {
        "file"
    } else {
        "other"
    }
}

fn check_inner(dir: &Path) -> Result<(), String> {
    let mut body = Vec::new();
    http::get(&feed_url(), 64 * 1024, &mut |b| {
        body.extend_from_slice(b);
        Ok(())
    })?;
    let f: Feed = serde_json::from_slice(&body).map_err(|e| format!("latest.json: {e}"))?;
    let mut st = load_state(dir);
    st.last_check = now();
    log::info!("update feed: newest {} (this is {})", f.version, current());
    read_notes(&mut st);
    if !newer(&f.version, current()) {
        clear_pending(dir, &mut st);
        save_state(dir, &st);
        set_status(|s| {
            s.state = "uptodate".into();
            s.latest = f.version.clone();
            s.checked = st.last_check;
        });
        return Ok(());
    }
    let want = f.sha256.to_ascii_lowercase();
    if want.len() != 64 || f.size == 0 || f.size > MAX_SETUP {
        return Err("latest.json: bad sha256 or size".into());
    }
    let file = format!("DeskboxesSetup-{}.exe", f.version.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.').collect::<String>());
    let path = dir.join(&file);
    // Already here from an earlier check?
    let have = st.pending.as_ref().map(|p| p.version == f.version && p.sha256 == want && sha256_file(&path).as_deref() == Some(want.as_str())).unwrap_or(false);
    if !have {
        set_status(|s| {
            s.state = "downloading".into();
            s.latest = f.version.clone();
        });
        // An installer downloaded before stays until this one is verified and recorded.
        let _ = std::fs::create_dir_all(dir);
        let part = dir.join(format!("{file}.part"));
        let mut out = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut h = sha256::Sha256::new();
        let mut n = 0u64;
        let mut shown = 0u64;
        let r = http::get(&f.url, f.size + 1, &mut |b| {
            n += b.len() as u64;
            // Settings' progress bar: whole percents.
            let pct = (n * 100 / f.size).min(100);
            if pct != shown {
                shown = pct;
                set_status(|s| s.progress = pct as f64 / 100.0);
            }
            h.update(b);
            out.write_all(b).map_err(|e| e.to_string())
        });
        drop(out);
        let got = sha256::hex(&h.finish());
        if let Err(e) = r.and_then(|_| if n != f.size { Err(format!("size {n}, expected {}", f.size)) } else if got != want { Err("SHA-256 does not match".into()) } else { Ok(()) }) {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
        std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
        if let Some(old) = st.pending.take().filter(|p| !p.file.eq_ignore_ascii_case(&file)) {
            let _ = std::fs::remove_file(dir.join(&old.file));
        }
        st.pending = Some(Pending { version: f.version.clone(), file, sha256: want });
        st.attempts = 0;
        log::info!("update {} downloaded and verified ({n} bytes)", f.version);
    }
    save_state(dir, &st);
    set_status(|s| {
        s.state = "ready".into();
        s.latest = f.version.clone();
        s.checked = st.last_check;
    });
    Ok(())
}

/// The newest release's notes into `st` and Settings (best effort: a check does not fail for them).
fn read_notes(st: &mut State) {
    let url = match (std::env::var("DBX_UPDATE_NOTES"), std::env::var_os("DBX_UPDATE_FEED")) {
        (Ok(u), _) => u,
        (Err(_), Some(_)) => return, // a test feed and no test notes
        (Err(_), None) => NOTES.to_string(),
    };
    let mut body = Vec::new();
    let r = http::get(&url, 1 << 20, &mut |b| {
        body.extend_from_slice(b);
        Ok(())
    })
    .and_then(|_| serde_json::from_slice::<Release>(&body).map_err(|e| format!("release: {e}")));
    match r {
        Ok(rel) => {
            let n = Notes {
                version: rel.tag_name.trim_start_matches('v').to_string(),
                body: rel.body.unwrap_or_default().chars().take(20_000).collect(),
                date: rel.published_at.unwrap_or_default().chars().take(10).collect(),
            };
            set_status(|s| {
                s.notes = n.body.clone();
                s.notes_version = n.version.clone();
                s.notes_date = n.date.clone();
            });
            st.notes = Some(n);
        }
        Err(e) => log::info!("release notes not read: {e}"),
    }
}

fn clear_pending(dir: &Path, st: &mut State) {
    if let Some(p) = st.pending.take() {
        let _ = std::fs::remove_file(dir.join(&p.file));
    }
    st.attempts = 0;
}

/// A verified, newer installer is waiting: start it (`--quiet --update`, and `--launch` when
/// Deskboxes should come back afterwards) and return true; the caller then exits.
pub fn install_pending(dir: &Path, relaunch: bool) -> bool {
    let mut st = load_state(dir);
    let Some(p) = st.pending.clone() else { return false };
    if !newer(&p.version, current()) {
        log::info!("update {} is installed: removing the downloaded installer", p.version);
        clear_pending(dir, &mut st);
        save_state(dir, &st);
        return false;
    }
    // The installer only updates a registered install (even with a test feed).
    if !installed_copy() || deskboxes_shell::reg::get_string(UNINSTALL_KEY, "InstallLocation").is_none() {
        return false;
    }
    if st.attempts >= 2 {
        log::warn!("update {}: installing failed twice; waiting for a newer one", p.version);
        return false;
    }
    let path = dir.join(&p.file);
    if sha256_file(&path).as_deref() != Some(p.sha256.as_str()) {
        log::warn!("update {}: the downloaded installer changed; downloading again later", p.version);
        clear_pending(dir, &mut st);
        save_state(dir, &st);
        return false;
    }
    st.attempts += 1;
    save_state(dir, &st);
    let mut cmd = std::process::Command::new(&path);
    cmd.arg("--quiet").arg("--update");
    if relaunch {
        cmd.arg("--launch");
    }
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x8;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
    match cmd.current_dir(dir).creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP).spawn() {
        Ok(c) => {
            log::info!("installing update {} (installer pid {}, relaunch={relaunch})", p.version, c.id());
            true
        }
        Err(e) => {
            log::warn!("cannot start the update installer: {e}");
            false
        }
    }
}

fn sha256_file(p: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut h = sha256::Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(sha256::hex(&h.finish()))
}

/// WinHTTP GET: the body in pieces to `sink`, at most `max` bytes.
mod http {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Networking::WinHttp::*;

    struct H(*mut core::ffi::c_void);
    impl Drop for H {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }

    pub fn get(url: &str, max: u64, sink: &mut dyn FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
        let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("http://") {
            (false, r)
        } else {
            return Err("not an http(s) address".into());
        };
        let (hostport, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().map_err(|_| "bad port")?),
            None => (hostport, if secure { 443 } else { 80 }),
        };
        let local = host == "127.0.0.1" || host.eq_ignore_ascii_case("localhost");
        if !secure && !local {
            return Err("updates are only downloaded over HTTPS".into());
        }
        if !local && !(host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("api.github.com") || host.to_ascii_lowercase().ends_with(".githubusercontent.com")) {
            return Err(format!("not a GitHub address: {host}"));
        }
        unsafe {
            let ua = HSTRING::from(format!("Deskboxes-Updater/{}", super::current()));
            let s = H(WinHttpOpen(PCWSTR(ua.as_ptr()), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
            if s.0.is_null() {
                return Err(format!("WinHttpOpen: {}", windows::core::Error::from_thread()));
            }
            let _ = WinHttpSetTimeouts(s.0, 15_000, 15_000, 30_000, 60_000);
            let hh = HSTRING::from(host);
            let c = H(WinHttpConnect(s.0, PCWSTR(hh.as_ptr()), port, 0));
            if c.0.is_null() {
                return Err(format!("connect {host}: {}", windows::core::Error::from_thread()));
            }
            let hp = HSTRING::from(path.as_str());
            let r = H(WinHttpOpenRequest(c.0, windows::core::w!("GET"), PCWSTR(hp.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), if secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) }));
            if r.0.is_null() {
                return Err(format!("request: {}", windows::core::Error::from_thread()));
            }
            WinHttpSendRequest(r.0, None, None, 0, 0, 0).map_err(|e| format!("send: {e}"))?;
            WinHttpReceiveResponse(r.0, std::ptr::null_mut()).map_err(|e| format!("receive: {e}"))?;
            let mut code = 0u32;
            let mut len = 4u32;
            WinHttpQueryHeaders(r.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut code as *mut u32 as *mut _), &mut len, std::ptr::null_mut()).map_err(|e| format!("status: {e}"))?;
            if code != 200 {
                return Err(if code == 404 { "no release published yet (404)".into() } else { format!("HTTP {code}") });
            }
            let mut total = 0u64;
            let mut buf = vec![0u8; 1 << 16];
            loop {
                let mut n = 0u32;
                WinHttpReadData(r.0, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut n).map_err(|e| format!("read: {e}"))?;
                if n == 0 {
                    break;
                }
                total += n as u64;
                if total > max {
                    return Err("download larger than expected".into());
                }
                sink(&buf[..n as usize])?;
            }
            Ok(())
        }
    }
}

/// SHA-256 (FIPS 180-4), small and dependency-free.
mod sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    pub struct Sha256 {
        h: [u32; 8],
        buf: Vec<u8>,
        len: u64,
    }

    impl Sha256 {
        pub fn new() -> Self {
            Sha256 { h: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19], buf: Vec::with_capacity(64), len: 0 }
        }

        pub fn update(&mut self, mut d: &[u8]) {
            self.len += d.len() as u64;
            if !self.buf.is_empty() {
                let take = (64 - self.buf.len()).min(d.len());
                self.buf.extend_from_slice(&d[..take]);
                d = &d[take..];
                if self.buf.len() == 64 {
                    let b: [u8; 64] = self.buf[..].try_into().unwrap();
                    self.block(&b);
                    self.buf.clear();
                }
            }
            while d.len() >= 64 {
                self.block(d[..64].try_into().unwrap());
                d = &d[64..];
            }
            self.buf.extend_from_slice(d);
        }

        pub fn finish(mut self) -> [u8; 32] {
            let bits = self.len.wrapping_mul(8);
            let mut pad = vec![0x80u8];
            while (self.buf.len() + pad.len()) % 64 != 56 {
                pad.push(0);
            }
            pad.extend_from_slice(&bits.to_be_bytes());
            let len = self.len;
            self.update(&pad);
            self.len = len;
            let mut out = [0u8; 32];
            for (i, w) in self.h.iter().enumerate() {
                out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
            }
            out
        }

        fn block(&mut self, b: &[u8; 64]) {
            let mut w = [0u32; 64];
            for i in 0..16 {
                w[i] = u32::from_be_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
            }
            let [mut a, mut bb, mut c, mut d, mut e, mut f, mut g, mut h] = self.h;
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ (!e & g);
                let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & bb) ^ (a & c) ^ (bb & c);
                let t2 = s0.wrapping_add(maj);
                h = g;
                g = f;
                f = e;
                e = d.wrapping_add(t1);
                d = c;
                c = bb;
                bb = a;
                a = t1.wrapping_add(t2);
            }
            for (x, y) in self.h.iter_mut().zip([a, bb, c, d, e, f, g, h]) {
                *x = x.wrapping_add(y);
            }
        }
    }

    pub fn hex(d: &[u8]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_vectors() {
        let h = |s: &[u8]| {
            let mut x = sha256::Sha256::new();
            x.update(s);
            sha256::hex(&x.finish())
        };
        assert_eq!(h(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(h(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(h(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        // In pieces of odd sizes, as a download arrives.
        let data: Vec<u8> = (0..100_000u32).map(|i| (i * 7 + 3) as u8).collect();
        let mut x = sha256::Sha256::new();
        for c in data.chunks(997) {
            x.update(c);
        }
        assert_eq!(sha256::hex(&x.finish()), h(&data));
    }

    #[test]
    fn versions() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("v1.0", "0.99.99"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1", "0.1.0"));
        assert!(!newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn error_kinds() {
        assert_eq!(error_kind("send: 0x80072EFD"), "net");
        assert_eq!(error_kind("connect github.com: 0x80072EE7"), "net");
        assert_eq!(error_kind("no release published yet (404)"), "none");
        assert_eq!(error_kind("HTTP 500"), "server");
        assert_eq!(error_kind("SHA-256 does not match"), "file");
        assert_eq!(error_kind("latest.json: expected value"), "file");
        assert_eq!(error_kind("something odd"), "other");
    }
}
