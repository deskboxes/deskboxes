//! Folder boxes: a box that shows a folder's contents (Documents, Downloads, a project folder)
//! instead of desktop icons. Its files stay where they are; the box only shows them. A folder in
//! it opens inside the box, and the back arrow in its title (or Backspace) goes up again, never
//! above the folder the box was made for.
//!
//! The worker lists the folders shown now along with the desktop (`Source::Folder` items), and
//! each box's list is filled from them, folders first, then by name. Desktop icons never go into
//! a folder box as layout: dropping one there moves the file into the folder, as dropping it on
//! the folder's icon would.

use crate::app::{header_h, App};
use crate::worker::Req;
use deskboxes_core::*;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// The folders shown now (lowercase): a watched folder that is no longer shown asks no rescans.
static SHOWN: Mutex<Vec<String>> = Mutex::new(Vec::new());
static WATCHED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

const DIR: u32 = 0x10;
/// Cloud files not on this PC (OFFLINE, RECALL_ON_OPEN, RECALL_ON_DATA_ACCESS): never listed
/// inside a box (that could download them); they open in File Explorer instead.
const NOT_LOCAL: u32 = 0x1000 | 0x40000 | 0x400000;

pub fn parent_of(path: &str) -> &str {
    path.rsplit_once('\\').map(|(p, _)| p).unwrap_or("")
}

fn same(a: &str, b: &str) -> bool {
    a.trim_end_matches('\\').eq_ignore_ascii_case(b.trim_end_matches('\\'))
}

impl App {
    pub fn is_folder_box(&self, id: u32) -> bool {
        self.box_by_id(id).map(|b| b.is_folder()).unwrap_or(false)
    }

    /// An item from a folder a folder box shows (not a desktop icon).
    pub fn is_folder_item(&self, k: &ItemKey) -> bool {
        self.items.get(k).map(|i| i.source == Source::Folder).unwrap_or(false)
    }

    /// A folder that may be shown in a box: a local folder on disk, not one of the desktop's own
    /// folders (their icons are on the desktop already).
    pub fn can_show_folder(&self, path: &str) -> bool {
        let Ok(md) = std::fs::symlink_metadata(path) else { return false };
        use std::os::windows::fs::MetadataExt;
        let a = md.file_attributes();
        a & DIR != 0 && a & NOT_LOCAL == 0 && !self.desk_dirs.iter().any(|d| same(d, path))
    }

    /// Tell the worker which folders to list (and watch them) when that changed, and fill the
    /// folder boxes from what is known.
    pub fn folder_sync(&mut self) {
        let mut want: Vec<String> = vec![];
        for b in self.layout.boxes.iter().filter(|b| b.is_folder()) {
            let f = b.folder_now().to_string();
            if !want.iter().any(|w| same(w, &f)) {
                want.push(f);
            }
        }
        if want != self.folders_sent {
            *SHOWN.lock().unwrap() = want.iter().map(|w| w.to_lowercase()).collect();
            let mut watched = WATCHED.lock().unwrap();
            let watched = watched.get_or_insert_with(HashSet::new);
            for w in &want {
                let lw = w.to_lowercase();
                if watched.insert(lw.clone()) {
                    deskboxes_shell::watch::watch(vec![w.into()], move || {
                        if SHOWN.lock().map(|s| s.contains(&lw)).unwrap_or(false) {
                            crate::events::post(crate::events::Ev::Rescan);
                        }
                    });
                }
            }
            log::info!("folder boxes show {} folder(s)", want.len());
            self.folders_sent = want.clone();
            let _ = self.worker.send(Req::Folders(want));
            let _ = self.worker.send(Req::Scan);
        }
        self.fill_folder_boxes();
    }

    /// Each folder box lists what is in its folder now: folders first, then by name.
    pub fn fill_folder_boxes(&mut self) {
        if !self.layout.boxes.iter().any(|b| b.is_folder()) {
            return;
        }
        let mut by_dir: HashMap<String, Vec<&Item>> = HashMap::new();
        for it in self.items.values().filter(|i| i.source == Source::Folder) {
            by_dir.entry(parent_of(&it.parsing_name).to_lowercase()).or_default().push(it);
        }
        for v in by_dir.values_mut() {
            v.sort_by(|a, b| b.is_folder().cmp(&a.is_folder()).then_with(|| natural(&a.display_name, &b.display_name)));
        }
        for b in self.layout.boxes.iter_mut().filter(|b| b.is_folder()) {
            let keys: Vec<ItemKey> = by_dir.get(&b.folder_now().trim_end_matches('\\').to_lowercase()).map(|v| v.iter().map(|i| i.key.clone()).collect()).unwrap_or_default();
            b.tabs.truncate(1);
            b.active_tab = 0;
            if let Some(t) = b.tabs.first_mut() {
                t.items = keys;
            }
        }
    }

    /// Open folder `path` inside folder box `id`. False: not a folder that can be shown there
    /// (the caller opens it in File Explorer).
    pub fn folder_open(&mut self, id: u32, path: &str) -> bool {
        if !self.can_show_folder(path) {
            return false;
        }
        let Some(b) = self.layout.boxes.iter_mut().find(|b| b.id == id && b.is_folder()) else { return false };
        b.folder_at = if same(path, &b.folder) { String::new() } else { path.to_string() };
        self.folder_moved(id);
        true
    }

    /// Up one folder (not above the box's own folder).
    pub fn folder_back(&mut self, id: u32) {
        let Some(b) = self.layout.boxes.iter_mut().find(|b| b.id == id && b.is_folder()) else { return };
        if b.folder_at.is_empty() {
            return;
        }
        let up = parent_of(&b.folder_at).to_string();
        b.folder_at = if up.is_empty() || same(&up, &b.folder) || up.len() < b.folder.len() { String::new() } else { up };
        self.folder_moved(id);
    }

    fn folder_moved(&mut self, id: u32) {
        self.scroll.remove(&id);
        self.sel.set.clear();
        self.sel.focus = None;
        self.hover = None;
        self.folder_sync();
        self.layout_changed();
        self.invalidate();
    }

    /// The back arrow in a folder box's title (shown while it is in a subfolder).
    pub fn folder_back_rect(&self, b: &BoxModel) -> Option<deskboxes_core::geom::Rect> {
        if !b.is_folder() || b.folder_at.is_empty() {
            return None;
        }
        let r = self.shown_rect(b);
        Some(deskboxes_core::geom::Rect::new(r.x + 6.0, r.y + 4.0, 26.0, header_h() - 8.0))
    }

    /// The folder box whose back arrow is at `p`.
    pub fn folder_back_at(&self, p: Pt) -> Option<u32> {
        for b in self.layout.boxes.iter().rev() {
            if self.shown_rect(b).contains(p) {
                return self.folder_back_rect(b).filter(|r| r.inflate(2.0).contains(p)).map(|_| b.id);
            }
        }
        None
    }

    /// What a folder box's title says: its own title in its folder, the subfolder's name below.
    pub fn folder_title(&self, b: &BoxModel) -> Option<String> {
        if !b.is_folder() || b.folder_at.is_empty() {
            return None;
        }
        let name = b.folder_at.trim_end_matches('\\').rsplit('\\').next().unwrap_or("").to_string();
        Some(name)
    }

    /// A new folder box for `path` at `at` (4 × 3 icons), named after the folder.
    pub fn new_folder_box(&mut self, path: &str, at: Pt) {
        if !self.can_show_folder(path) {
            log::info!("folder box refused for {path}");
            return;
        }
        let title = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
        let r = deskboxes_core::geom::Rect::new(at.x, at.y, self.grid.cell_w * 4.0 + 2.0 * crate::app::PAD, header_h() + self.grid.cell_h * 3.0 + 8.0);
        let bounds = deskboxes_core::geom::Rect::new(0.0, 0.0, self.size_dip.0, self.size_dip.1);
        let r = r.clamp_into(&self.work.clamp_into(&bounds));
        let style = self.settings.default_style.clone();
        let p = path.to_string();
        self.change_layout(|l, _| {
            layout::create_folder_box(l, r, &title, style, &p);
        });
        self.folder_sync();
        log::info!("folder box for {path}");
    }

    /// The folder box a selection's item is in, when it is a folder that opens inside it.
    pub fn folder_to_open(&self, k: &ItemKey) -> Option<(u32, String)> {
        let it = self.items.get(k)?;
        if it.source != Source::Folder || !it.is_folder() {
            return None;
        }
        let id = self.layout.boxes.iter().find(|b| b.is_folder() && b.contains(k).is_some())?.id;
        Some((id, it.parsing_name.clone()))
    }

    /// A drop of `keys` onto folder box `id`: the files move into its folder (Ctrl: copied), as
    /// a drop on that folder's icon. Items already in that folder stay as they are.
    pub fn drop_into_folder_box(&mut self, keys: &[ItemKey], id: u32, copy: bool) {
        let Some(target) = self.box_by_id(id).map(|b| b.folder_now().to_string()) else { return };
        let paths: Vec<String> = keys
            .iter()
            .filter_map(|k| self.items.get(k))
            .filter(|i| !same(parent_of(&i.parsing_name), &target))
            .map(|i| i.parsing_name.clone())
            .collect();
        if paths.is_empty() {
            return;
        }
        log::info!("drop {} item(s) into folder box {id}", paths.len());
        let _ = self.worker.send(Req::DropOnItem { target, paths, copy });
    }

    /// Hit for a menu or drop: the folder a folder box shows at `p`.
    pub fn folder_box_at(&self, p: Pt) -> Option<String> {
        self.drop_box(p).and_then(|id| self.box_by_id(id)).filter(|b| b.is_folder()).map(|b| b.folder_now().to_string())
    }
}

/// "Choose folder…" for a new folder box (`change` None, made at `at`) or "Change folder…" for
/// box `change`: the folder picker opens once the menu is gone (it is modal: it runs outside
/// the App's borrow, from a posted message).
#[derive(Clone, Copy)]
pub struct PickReq {
    pub change: Option<u32>,
    pub at: Pt,
}

impl App {
    pub fn ask_folder(&mut self, change: Option<u32>, at: Pt) {
        self.pick_req = Some(PickReq { change, at });
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(Some(self.msg_hwnd), crate::events::WM_APP_PICKFOLDER, windows::Win32::Foundation::WPARAM(0), windows::Win32::Foundation::LPARAM(0));
        }
    }

    /// Where the picker starts: the box's folder, else Documents.
    pub fn pick_start(&self, r: &PickReq) -> String {
        r.change
            .and_then(|id| self.box_by_id(id))
            .map(|b| b.folder.clone())
            .or_else(|| deskboxes_shell::items::known_folders().into_iter().find(|(n, _)| *n == "documents").map(|(_, p)| p.display().to_string()))
            .unwrap_or_default()
    }

    /// The folder was picked: a new folder box for it, or the box shows it from now on.
    pub fn apply_pick(&mut self, r: PickReq, path: &str) {
        if !self.can_show_folder(path) {
            log::info!("folder box: {path} can't be shown (a desktop folder, or not on this PC)");
            return;
        }
        match r.change {
            None => self.new_folder_box(path, r.at),
            Some(id) => {
                let title = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
                let p = path.to_string();
                self.change_layout(|l, _| {
                    if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id && b.is_folder()) {
                        // Its title follows the folder when it was the old folder's name.
                        let old = std::path::Path::new(&b.folder).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        if let Some(t) = b.tabs.first_mut() {
                            if t.title == old || t.title.is_empty() {
                                t.title = title.clone();
                            }
                        }
                        b.folder = p.clone();
                        b.folder_at = String::new();
                    }
                });
                self.folder_moved(id);
                log::info!("folder box {id} shows {path}");
            }
        }
    }
}

/// Windows' folder picker, over `owner`, starting in `start`.
pub fn pick_folder(owner: windows::Win32::Foundation::HWND, title: &str, start: &str) -> Option<String> {
    use windows::core::HSTRING;
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::*;
    unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let _ = d.SetOptions(FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST);
        let _ = d.SetTitle(&HSTRING::from(title));
        if !start.is_empty() {
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(start), None) {
                let _ = d.SetFolder(&item);
            }
        }
        d.Show(Some(owner)).ok()?;
        let item = d.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

/// Names compared as Explorer sorts them: numbers by value ("2" before "10"), case ignored.
pub fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n = String::new();
                while let Some(c) = x.peek().copied().filter(|c| c.is_ascii_digit()) {
                    n.push(c);
                    x.next();
                }
                let mut m = String::new();
                while let Some(d) = y.peek().copied().filter(|d| d.is_ascii_digit()) {
                    m.push(d);
                    y.next();
                }
                let (n2, m2) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let o = n2.len().cmp(&m2.len()).then_with(|| n2.cmp(m2));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["File 10", "file 2", "File 1", "a", "B"];
        v.sort_by(|a, b| natural(a, b));
        assert_eq!(v, vec!["a", "B", "File 1", "file 2", "File 10"]);
    }

    #[test]
    fn parents() {
        assert_eq!(parent_of(r"C:\a\b\c.txt"), r"C:\a\b");
        assert!(same(r"C:\A\b\", r"c:\a\B"));
    }
}
