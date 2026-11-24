//! Moving to another PC (Settings › Saved layouts): everything — settings, the boxes, pages and
//! rules of every screen, the saved layouts — into one file, and back from it.
//!
//! Icons are known by their file IDs, which another PC does not share, so the file also keeps
//! each icon's file name: importing matches icons by name, moves paths under the old user folder
//! to this PC's, and the old main screen to this one's. Importing keeps what there was under
//! Saved layouts first, then starts Deskboxes again on the imported files.

use crate::app::App;
use deskboxes_core::screens as scr;
use deskboxes_core::*;
use deskboxes_i18n::{t, S};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const FORMAT: &str = "deskboxes-export";
pub const EXT: &str = "deskboxes";

/// Started again after an import, without the watchdog to do it (a test window).
static RESPAWN: AtomicBool = AtomicBool::new(false);

/// The export file.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Export {
    pub format: String,
    pub version: u32,
    /// The Deskboxes version that wrote it.
    pub app: String,
    /// When (local time, "2026-11-24 15:30").
    pub exported: String,
    /// The user folder it was exported from (`C:\Users\name`).
    pub profile: String,
    /// The main screen there (its id in the layout).
    pub main_screen: String,
    /// Icon key -> its file name, for matching icons on another PC.
    pub items: BTreeMap<String, ExportItem>,
    pub settings: Value,
    pub layout: Value,
    /// Saved layouts by id.
    #[serde(default)]
    pub saved: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ExportItem {
    /// The file's name, lowercase.
    pub name: String,
    /// On the public desktop (all users).
    #[serde(default)]
    pub public: bool,
}

/// What changes from the exporting PC to this one, string by string (icon keys, the main
/// screen, paths under the user folder). Each string is mapped once: no chain of replacements.
#[derive(Default, Debug)]
pub struct Remap {
    pub keys: HashMap<String, String>,
    pub screen: Option<(String, String)>,
    pub profile: Option<(String, String)>,
}

impl Remap {
    pub fn string(&self, s: &str) -> String {
        if let Some(n) = self.keys.get(s) {
            return n.clone();
        }
        if let Some((o, n)) = &self.screen {
            if s == o {
                return n.clone();
            }
        }
        if let Some((o, n)) = &self.profile {
            // A path ("C:\Users\old\Documents\x"), or a path key ("p:c:\users\old\desktop\x").
            let (pre, rest) = match s.strip_prefix("p:") {
                Some(r) => ("p:", r),
                None => ("", s),
            };
            let head = rest.get(..o.len());
            let tail = rest.get(o.len()..).unwrap_or("");
            if head.is_some_and(|h| h.eq_ignore_ascii_case(o)) && (tail.is_empty() || tail.starts_with('\\')) {
                let n = if pre.is_empty() { n.clone() } else { n.to_lowercase() };
                return format!("{pre}{n}{tail}");
            }
        }
        s.to_string()
    }

    /// Every string in `v`, object keys too.
    pub fn value(&self, v: Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.string(&s)),
            Value::Array(a) => Value::Array(a.into_iter().map(|x| self.value(x)).collect()),
            Value::Object(m) => Value::Object(m.into_iter().map(|(k, x)| (self.string(&k), self.value(x))).collect()),
            other => other,
        }
    }
}

/// The exported icons' keys -> the keys of the icons here with the same file name (on the same
/// desktop, the user's or the public one, where both have one). Each icon here is taken once.
pub fn match_keys(old: &BTreeMap<String, ExportItem>, here: &[(String, String, bool)]) -> HashMap<String, String> {
    let mut by_name: HashMap<&str, Vec<(&str, bool)>> = HashMap::new();
    for (k, name, public) in here {
        by_name.entry(name.as_str()).or_default().push((k.as_str(), *public));
    }
    let mut taken: HashSet<&str> = HashSet::new();
    let mut out = HashMap::new();
    // Same desktop first, then any.
    for same in [true, false] {
        for (k, it) in old {
            if out.contains_key(k) {
                continue;
            }
            let Some(c) = by_name.get(it.name.as_str()) else { continue };
            if let Some((nk, _)) = c.iter().find(|(nk, p)| !taken.contains(nk) && (!same || *p == it.public)) {
                taken.insert(nk);
                out.insert(k.clone(), nk.to_string());
            }
        }
    }
    out.retain(|k, v| k != v);
    out
}

pub fn read(path: &Path) -> Result<Export, String> {
    let b = std::fs::read(path).map_err(|e| e.to_string())?;
    let e: Export = serde_json::from_slice(&b).map_err(|e| e.to_string())?;
    if e.format != FORMAT || e.version != 1 {
        return Err(format!("format {} {}", e.format, e.version));
    }
    serde_json::from_value::<MonitorLayout>(e.layout.clone()).map_err(|e| format!("layout: {e}"))?;
    Ok(e)
}

/// Start Deskboxes again after an import when there is no watchdog to (a test window).
pub fn respawn_if_wanted() {
    if !RESPAWN.load(Ordering::Acquire) {
        return;
    }
    let args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--first-run-template").collect();
    match std::env::current_exe().map(|exe| std::process::Command::new(exe).args(&args).spawn()) {
        Ok(Ok(_)) => log::info!("import: started again"),
        Ok(Err(e)) | Err(e) => log::warn!("import: cannot start again: {e}"),
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

impl App {
    fn main_screen_id(&self) -> String {
        match self.screens.first() {
            Some(s) => s.mon.id.clone(),
            None => scr::current(&self.layout).to_string(),
        }
    }

    /// Everything into the file at `path`.
    pub fn export_all(&mut self, path: &Path) -> Result<(), String> {
        let items = self
            .items
            .iter()
            .filter(|(_, it)| matches!(it.source, Source::User | Source::Public | Source::Fixture))
            .filter_map(|(k, it)| {
                let name = Path::new(&it.parsing_name).file_name()?.to_string_lossy().to_lowercase();
                Some((k.0.clone(), ExportItem { name, public: it.source == Source::Public }))
            })
            .collect();
        let mut saved = BTreeMap::new();
        for e in std::fs::read_dir(self.saved_dir()).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") {
                let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                if let Some(m) = persist::load_layout(&self.saved_dir(), &id) {
                    saved.insert(id, serde_json::to_value(&m).map_err(|e| e.to_string())?);
                }
            }
        }
        let e = Export {
            format: FORMAT.into(),
            version: 1,
            app: env!("CARGO_PKG_VERSION").into(),
            exported: crate::saved::shown(crate::saved::local_stamp()),
            profile: std::env::var("USERPROFILE").unwrap_or_default(),
            main_screen: self.main_screen_id(),
            items,
            settings: serde_json::to_value(&self.settings).map_err(|e| e.to_string())?,
            layout: serde_json::to_value(&self.layout).map_err(|e| e.to_string())?,
            saved,
        };
        let bytes = serde_json::to_vec_pretty(&e).map_err(|e| e.to_string())?;
        std::fs::write(path, bytes).map_err(|e| e.to_string())?;
        log::info!("exported to {}: {} icons, {} saved layouts", path.display(), e.items.len(), e.saved.len());
        Ok(())
    }

    /// Settings › Export: where to, then the file, then what happened.
    pub fn exported(&mut self, path: &Path) {
        let l = self.lang;
        let (title, body) = match self.export_all(path) {
            Ok(()) => (t(l, S::ExportDoneTitle).to_string(), t(l, S::ExportDoneBody).replace("{file}", &file_name(path))),
            Err(e) => {
                log::warn!("export to {}: {e}", path.display());
                (t(l, S::ExportPickTitle).to_string(), t(l, S::ExportFailed).replace("{error}", &e))
            }
        };
        self.info_dialog(&title, &body);
    }

    /// Settings › Import: the file chosen; ask first.
    pub fn ask_import(&mut self, path: PathBuf) {
        let l = self.lang;
        match read(&path) {
            Ok(e) => {
                let body = serde_json::json!({
                    "kind": "confirm",
                    "title": t(l, S::ImportTitle).replace("{file}", &file_name(&path)),
                    "body": t(l, S::ImportBody).replace("{when}", &e.exported),
                    "ok": t(l, S::ImportOk),
                    "cancel": t(l, S::Cancel),
                });
                self.open_dialog(crate::pages::Ask::Import(path), body);
            }
            Err(e) => {
                log::warn!("import {}: {e}", path.display());
                self.info_dialog(t(l, S::ImportPickTitle), t(l, S::ImportBad));
            }
        }
    }

    /// A message with only OK.
    pub fn info_dialog(&mut self, title: &str, body: &str) {
        let body = serde_json::json!({ "kind": "confirm", "title": title, "body": body, "ok": t(self.lang, S::Ok), "cancel": "" });
        self.open_dialog(crate::pages::Ask::Info, body);
    }

    /// Import the file at `path` (asked): what there is now goes to Saved layouts, the files
    /// are written, and Deskboxes starts again on them.
    pub fn import_all(&mut self, path: &Path) {
        let e = match read(path) {
            Ok(e) => e,
            Err(err) => {
                log::warn!("import {}: {err}", path.display());
                return self.info_dialog(t(self.lang, S::ImportPickTitle), t(self.lang, S::ImportBad));
            }
        };
        self.save_layout_now();
        let here: Vec<(String, String, bool)> = self
            .items
            .iter()
            .filter(|(_, it)| matches!(it.source, Source::User | Source::Public | Source::Fixture))
            .filter_map(|(k, it)| Some((k.0.clone(), Path::new(&it.parsing_name).file_name()?.to_string_lossy().to_lowercase(), it.source == Source::Public)))
            .collect();
        let profile_now = std::env::var("USERPROFILE").unwrap_or_default();
        let remap = Remap {
            keys: match_keys(&e.items, &here),
            screen: Some((e.main_screen.clone(), self.main_screen_id())).filter(|(o, n)| !o.is_empty() && o != n),
            profile: Some((e.profile.clone(), profile_now)).filter(|(o, n)| !o.is_empty() && !n.is_empty() && !o.eq_ignore_ascii_case(n)),
        };
        log::info!("import {} (from {}, {}): {} of {} icons matched by name", path.display(), e.app, e.exported, remap.keys.len(), e.items.len());
        let layout: MonitorLayout = match serde_json::from_value(remap.value(e.layout)) {
            Ok(l) => l,
            Err(err) => {
                log::warn!("import layout: {err}");
                return self.info_dialog(t(self.lang, S::ImportPickTitle), t(self.lang, S::ImportBad));
            }
        };
        let mut settings: Settings = serde_json::from_value(e.settings).unwrap_or_else(|_| self.settings.clone());
        // (This PC's own: Deskboxes is on here.)
        settings.enabled = self.settings.enabled;
        let dir = self.saved_dir();
        for (id, v) in e.saved {
            if persist::layout_path(&dir, &id).exists() {
                continue;
            }
            match serde_json::from_value::<MonitorLayout>(remap.value(v)) {
                Ok(m) => {
                    let _ = std::fs::create_dir_all(&dir);
                    if let Err(err) = persist::save_layout(&dir, &id, &m) {
                        log::warn!("import saved layout {id}: {err}");
                    }
                }
                Err(err) => log::warn!("import saved layout {id}: {err}"),
            }
        }
        self.settings = settings;
        self.settings_dirty = false;
        if let Err(err) = persist::save_settings(&self.paths.settings, &self.settings) {
            log::warn!("import settings: {err}");
        }
        self.layout = layout;
        if let Err(err) = persist::save_layout(&self.paths.layouts, &self.topology, &self.layout) {
            log::warn!("import layout: {err}");
        }
        // Start again on the imported files: the watchdog does it (as after an Explorer restart
        // with nothing on screen); a test window starts itself.
        if self.args.watchdog_pid.is_none() {
            RESPAWN.store(true, Ordering::Release);
        }
        let code = if self.args.watchdog_pid.is_some() { 4 } else { 0 };
        log::info!("import: starting again (exit {code})");
        unsafe { windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(code) };
    }
}

/// Windows' Save dialog for an export file, over `owner`.
pub fn pick_save(owner: windows::Win32::Foundation::HWND, title: &str, kind: &str, name: &str) -> Option<PathBuf> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::*;
    unsafe {
        let d: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let (n, spec) = (HSTRING::from(kind), HSTRING::from(format!("*.{EXT}")));
        let _ = d.SetFileTypes(&[COMDLG_FILTERSPEC { pszName: PCWSTR(n.as_ptr()), pszSpec: PCWSTR(spec.as_ptr()) }]);
        let _ = d.SetDefaultExtension(&HSTRING::from(EXT));
        let _ = d.SetFileName(&HSTRING::from(name));
        let _ = d.SetTitle(&HSTRING::from(title));
        let _ = d.SetOptions(d.GetOptions().unwrap_or_default() | FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM);
        d.Show(Some(owner)).ok()?;
        item_path(&d.GetResult().ok()?)
    }
}

/// Windows' Open dialog for an export file, over `owner`.
pub fn pick_open(owner: windows::Win32::Foundation::HWND, title: &str, kind: &str) -> Option<PathBuf> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::*;
    unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let (n, spec) = (HSTRING::from(kind), HSTRING::from(format!("*.{EXT}")));
        let _ = d.SetFileTypes(&[COMDLG_FILTERSPEC { pszName: PCWSTR(n.as_ptr()), pszSpec: PCWSTR(spec.as_ptr()) }]);
        let _ = d.SetTitle(&HSTRING::from(title));
        let _ = d.SetOptions(d.GetOptions().unwrap_or_default() | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM);
        d.Show(Some(owner)).ok()?;
        item_path(&d.GetResult().ok()?)
    }
}

unsafe fn item_path(item: &windows::Win32::UI::Shell::IShellItem) -> Option<PathBuf> {
    let p = item.GetDisplayName(windows::Win32::UI::Shell::SIGDN_FILESYSPATH).ok()?;
    let s = p.to_string().ok();
    windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _));
    s.map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, public: bool) -> ExportItem {
        ExportItem { name: name.into(), public }
    }

    #[test]
    fn icons_match_by_name_and_desktop() {
        let mut old = BTreeMap::new();
        old.insert("f:1:a".to_string(), item("notes.txt", false));
        old.insert("f:1:b".to_string(), item("game.lnk", true));
        old.insert("f:1:c".to_string(), item("gone.txt", false));
        old.insert("f:1:d".to_string(), item("same.lnk", false));
        old.insert("f:1:e".to_string(), item("same.lnk", true));
        let here = vec![
            ("f:2:a".to_string(), "notes.txt".to_string(), false),
            ("f:2:b".to_string(), "game.lnk".to_string(), true),
            ("f:2:e".to_string(), "same.lnk".to_string(), true),
            ("f:2:d".to_string(), "same.lnk".to_string(), false),
        ];
        let m = match_keys(&old, &here);
        assert_eq!(m.get("f:1:a").map(String::as_str), Some("f:2:a"));
        assert_eq!(m.get("f:1:b").map(String::as_str), Some("f:2:b"));
        assert_eq!(m.get("f:1:d").map(String::as_str), Some("f:2:d"));
        assert_eq!(m.get("f:1:e").map(String::as_str), Some("f:2:e"));
        assert!(!m.contains_key("f:1:c"));
    }

    #[test]
    fn remap_is_one_pass() {
        let mut r = Remap::default();
        // a -> b and b -> c: "a" becomes "b", not "c".
        r.keys.insert("f:a".into(), "f:b".into());
        r.keys.insert("f:b".into(), "f:c".into());
        r.screen = Some(("DISPLAY-OLD".into(), "DISPLAY-NEW".into()));
        r.profile = Some((r"C:\Users\Old".into(), r"C:\Users\new".into()));
        let v = serde_json::json!({
            "loose": { "f:a": [1, 2], "f:b": [3, 4] },
            "folder": r"C:\Users\old\Documents\Games",
            "other": r"C:\Users\Oldest\x",
            "key": r"p:c:\users\old\desktop\x.txt",
            "shown": { "DISPLAY-OLD": 2 }
        });
        let out = r.value(v);
        assert_eq!(out["loose"]["f:b"], serde_json::json!([1, 2]));
        assert_eq!(out["loose"]["f:c"], serde_json::json!([3, 4]));
        assert_eq!(out["folder"], serde_json::json!(r"C:\Users\new\Documents\Games"));
        assert_eq!(out["other"], serde_json::json!(r"C:\Users\Oldest\x"));
        assert_eq!(out["key"], serde_json::json!(r"p:c:\users\new\desktop\x.txt"));
        assert_eq!(out["shown"]["DISPLAY-NEW"], serde_json::json!(2));
    }
}
