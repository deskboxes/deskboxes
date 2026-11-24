//! Atomic, versioned persistence (docs/ARCHITECTURE.md §8). Files live under the user's
//! %APPDATA% / %LOCALAPPDATA% folders, never in a Desktop folder.

use crate::model::{Layout, MonitorLayout, Settings, LAYOUT_SCHEMA};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Write `bytes` to `path` atomically: temp file, flush to disk, rename over the old file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path) // MoveFileExW(MOVEFILE_REPLACE_EXISTING) on Windows
}

/// A file that is there but cannot be read as this version reads it (damaged, or written by a
/// newer version) is copied aside before the defaults used instead are ever saved over it:
/// `<name>.unreadable-<unix time>.bak` next to it. Returns where it went.
pub fn keep_unreadable(path: &Path) -> Option<PathBuf> {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let name = path.file_name()?.to_string_lossy().into_owned();
    let to = path.with_file_name(format!("{name}.unreadable-{secs}.bak"));
    std::fs::copy(path, &to).ok()?;
    Some(to)
}

pub fn load_settings(path: &Path) -> Settings {
    let mut s: Settings = match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|_| {
            keep_unreadable(path);
            Settings::default()
        }),
        Err(_) => Settings::default(),
    };
    if s.icons.fx_v < 2 {
        s.icons.tint_strength = 1.0;
        s.icons.fx_v = 2;
    }
    s
}

pub fn save_settings(path: &Path, s: &Settings) -> std::io::Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(s).unwrap_or_default())
}

pub fn layout_path(dir: &Path, topology: &str) -> PathBuf {
    dir.join(format!("{topology}.json"))
}

/// Load the layout for a topology; None when there is none or it cannot be read.
pub fn load_layout(dir: &Path, topology: &str) -> Option<MonitorLayout> {
    load_layout_checked(dir, topology).ok().flatten()
}

/// Load the layout for a topology: Ok(None) there is none yet; Err when there is one this
/// version cannot read (damaged, or an unknown schema: not guessed at).
pub fn load_layout_checked(dir: &Path, topology: &str) -> Result<Option<MonitorLayout>, String> {
    let b = match std::fs::read(layout_path(dir, topology)) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let l: Layout = serde_json::from_slice(&b).map_err(|e| e.to_string())?;
    if l.schema != LAYOUT_SCHEMA {
        return Err(format!("schema {} (this version reads {LAYOUT_SCHEMA})", l.schema));
    }
    let mut m = l.monitor;
    for b in &mut m.boxes {
        b.own_folders();
    }
    Ok(Some(m))
}

pub fn save_layout(dir: &Path, topology: &str, m: &MonitorLayout) -> std::io::Result<()> {
    let l = Layout { schema: LAYOUT_SCHEMA, topology: topology.to_string(), monitor: m.clone() };
    write_atomic(&layout_path(dir, topology), &serde_json::to_vec_pretty(&l).unwrap_or_default())
}

/// Civil date (UTC) from days since 1970-01-01 (H. Hinnant's algorithm).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// First change of each day: copy the current layout files to backups/YYYY-MM-DD/ and keep the
/// newest 7 days. The copies are made in a folder of their own and only become the day's backup
/// when every one went through (a failed attempt is tried again at the next save); older days
/// are only let go after that.
pub fn backup_layouts_daily(layouts: &Path, backups: &Path) -> std::io::Result<bool> {
    let day = backups.join(today());
    if day.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(backups)?;
    let part = backups.join(format!("{}.part", today()));
    let _ = std::fs::remove_dir_all(&part);
    std::fs::create_dir_all(&part)?;
    let copied = (|| -> std::io::Result<()> {
        match std::fs::read_dir(layouts) {
            Ok(rd) => {
                for e in rd {
                    let e = e?;
                    if e.path().extension().map(|x| x == "json").unwrap_or(false) {
                        std::fs::copy(e.path(), part.join(e.file_name()))?;
                    }
                }
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    })();
    if let Err(e) = copied.and_then(|_| std::fs::rename(&part, &day)) {
        let _ = std::fs::remove_dir_all(&part);
        return Err(e);
    }
    // Only finished days count (a `.part` folder left by a crash goes too).
    let mut days: Vec<PathBuf> = vec![];
    for e in std::fs::read_dir(backups)?.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        if p.extension().map(|x| x == "part").unwrap_or(false) {
            let _ = std::fs::remove_dir_all(&p);
        } else {
            days.push(p);
        }
    }
    days.sort();
    while days.len() > 7 {
        let _ = std::fs::remove_dir_all(days.remove(0));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn civil() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_776), (2026, 11, 19));
    }
    #[test]
    fn layout_roundtrip() {
        let dir = std::env::temp_dir().join(format!("dbx-core-test-{}", std::process::id()));
        let mut m = MonitorLayout::default();
        crate::layout::create_box(&mut m, crate::Rect::new(1.0, 2.0, 3.0, 4.0), "Pliki", Default::default());
        save_layout(&dir, "sig", &m).unwrap();
        assert_eq!(load_layout(&dir, "sig"), Some(m));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
