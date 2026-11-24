//! Saved layouts (Settings › Saved layouts; Deskboxes › Saved layouts): keep how the boxes are
//! arranged now and go back to it later, or back to one of the daily backups. Only our layout
//! files are written (never a Desktop folder); restoring is one step that Undo takes back.

use crate::app::App;
use deskboxes_core::*;
use std::path::PathBuf;

/// A layout that can be restored: its id (YYYYMMDDHHMMSS, or YYYYMMDD for a daily backup), when
/// it is from (as shown) and how many boxes it has.
pub struct SavedLayout {
    pub id: u64,
    pub when: String,
    pub boxes: usize,
}

fn local_stamp() -> u64 {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    ((t.wYear as u64 * 100 + t.wMonth as u64) * 100 + t.wDay as u64) * 1_000_000 + (t.wHour as u64 * 100 + t.wMinute as u64) * 100 + t.wSecond as u64
}

/// Until 0.1.3 the layout file was named after the main screen's size ("primary-2560x1440-96");
/// now one file ("desktop") holds every screen's pages. The first start after that copies the
/// saved layouts across (the old ones stay where they were, for an older version).
pub fn copy_legacy_saved(paths: &crate::Paths, legacy: &str, now: &str) {
    let root = paths.layouts.parent().map(|p| p.join("saved")).unwrap_or_else(|| paths.layouts.join("saved"));
    let (from, to) = (root.join(legacy), root.join(now));
    if to.exists() || !from.is_dir() {
        return;
    }
    let mut n = 0;
    if std::fs::create_dir_all(&to).is_ok() {
        for e in std::fs::read_dir(&from).into_iter().flatten().flatten() {
            if e.path().extension().is_some_and(|x| x == "json") && std::fs::copy(e.path(), to.join(e.file_name())).is_ok() {
                n += 1;
            }
        }
    }
    log::info!("saved layouts: {n} copied from {legacy} to {now}");
}

/// "2026-11-20 04:15" (saved) or "2026-11-20" (a day's backup).
pub fn shown(id: u64) -> String {
    if id >= 10_000_000_000_000 {
        let (d, t) = (id / 1_000_000, id % 1_000_000);
        format!("{:04}-{:02}-{:02} {:02}:{:02}", d / 10_000, d / 100 % 100, d % 100, t / 10_000, t / 100 % 100)
    } else {
        format!("{:04}-{:02}-{:02}", id / 10_000, id / 100 % 100, id % 100)
    }
}

impl App {
    fn saved_dir(&self) -> PathBuf {
        self.paths.layouts.parent().map(|p| p.join("saved")).unwrap_or_else(|| self.paths.layouts.join("saved")).join(&self.topology)
    }

    /// The newest `n` saved layouts' ids, from their file names alone (nothing read: for the
    /// menu, which shows only when each was saved).
    pub fn newest_saved_ids(&self, n: usize) -> Vec<u64> {
        let mut v: Vec<u64> = std::fs::read_dir(self.saved_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| e.path().file_stem()?.to_str()?.parse().ok())
            .collect();
        v.sort_unstable_by(|a, b| b.cmp(a));
        v.truncate(n);
        v
    }

    /// Saved layouts of this screen setup, newest first (each read, for its box count).
    pub fn saved_layouts(&self) -> Vec<SavedLayout> {
        let mut v: Vec<SavedLayout> = std::fs::read_dir(self.saved_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let id: u64 = e.path().file_stem()?.to_str()?.parse().ok()?;
                let l = persist::load_layout(&self.saved_dir(), &id.to_string())?;
                Some(SavedLayout { id, when: shown(id), boxes: l.boxes.len() })
            })
            .collect();
        v.sort_by(|a, b| b.id.cmp(&a.id));
        v
    }

    /// The daily backups that have this screen setup's layout, newest first.
    pub fn backup_layouts(&self) -> Vec<SavedLayout> {
        let mut v: Vec<SavedLayout> = std::fs::read_dir(&self.paths.backups)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let day = e.file_name().to_str()?.to_string();
                let id: u64 = day.replace('-', "").parse().ok()?;
                let l = self.backup_of(&e.path())?;
                Some(SavedLayout { id, when: shown(id), boxes: l.boxes.len() })
            })
            .collect();
        v.sort_by(|a, b| b.id.cmp(&a.id));
        v
    }

    /// The layout in a day's backup folder (a backup from before 0.1.4 has it under the old name).
    fn backup_of(&self, day: &std::path::Path) -> Option<MonitorLayout> {
        persist::load_layout(day, &self.topology).or_else(|| persist::load_layout(day, &self.legacy_topology))
    }

    pub fn save_layout_now(&mut self) {
        let id = local_stamp();
        if let Err(e) = persist::save_layout(&self.saved_dir(), &id.to_string(), &self.layout) {
            log::warn!("save layout {id}: {e}");
        }
        self.saved_changed();
    }

    /// Go back to a saved layout (or a day's backup): one step, Undo takes it back.
    pub fn restore_layout(&mut self, id: u64) {
        let m = if id >= 10_000_000_000_000 {
            persist::load_layout(&self.saved_dir(), &id.to_string())
        } else {
            let day = shown(id);
            self.backup_of(&self.paths.backups.join(day))
        };
        let Some(m) = m else {
            log::warn!("restore layout {id}: not found");
            return;
        };
        let present: Vec<ItemKey> = self.items.keys().cloned().collect();
        self.scroll.clear();
        self.change_layout(|l, g| {
            *l = m;
            layout::reconcile(l, &present, g);
        });
        log::info!("restored layout {id}");
    }

    pub fn delete_saved_layout(&mut self, id: u64) {
        let _ = std::fs::remove_file(persist::layout_path(&self.saved_dir(), &id.to_string()));
        self.saved_changed();
    }

    /// The list in Settings changed.
    fn saved_changed(&mut self) {
        if self.host.is_some() {
            self.send_settings(false, None);
        }
    }
}
