//! Game covers for the Covers box view: Steam's library art (600 × 900) from its local cache
//! (`appcache\librarycache\<app id>\…\library_600x900.jpg`), read and scaled on a thread of its
//! own. Nothing is downloaded: a game Steam has no art for yet, and any other game, gets a
//! cover-sized tile with its icon.

use crate::app::App;
use crate::events::{post, Ev};
use deskboxes_core::geom::Rect;
use deskboxes_core::snapshot::IconImage;
use deskboxes_core::*;
use deskboxes_render::Bitmap;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A cover's size in DIPs (Steam's 2:3) and its corners.
pub const COVER_W: f32 = 100.0;
pub const COVER_H: f32 = 150.0;
pub const RADIUS: f32 = 6.0;

/// The covers read so far.
#[derive(Default)]
pub struct Covers {
    /// Steam app id -> its cover (None: there is none).
    got: HashMap<u32, Option<Bitmap>>,
    pending: HashSet<u32>,
}

/// Where Steam is (its own registry value), if it is installed.
fn steam_root() -> Option<PathBuf> {
    deskboxes_shell::reg::get_string(r"Software\Valve\Steam", "SteamPath").map(PathBuf::from).filter(|p| p.is_dir())
}

/// The cover Steam keeps for game `id`: in the game's folder of the library cache or one folder
/// further down (newer Steam), or next to the others (older Steam).
pub fn find_cover(root: &Path, id: u32) -> Option<PathBuf> {
    let cache = root.join("appcache").join("librarycache");
    let dir = cache.join(id.to_string());
    for name in ["library_600x900.jpg", "library_capsule.jpg"] {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path().join(name);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    let flat = cache.join(format!("{id}_library_600x900.jpg"));
    flat.is_file().then_some(flat)
}

/// Read and scale the covers of games `ids` to `px` pixels wide, then hand them to the UI thread.
fn load(ids: Vec<u32>, px: u32) {
    let spawned = std::thread::Builder::new().name("covers".into()).spawn(move || {
        let init = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED) };
        let root = steam_root();
        let t = std::time::Instant::now();
        let out: Vec<(u32, Option<IconImage>)> = ids
            .into_iter()
            .map(|id| {
                let img = root.as_deref().and_then(|r| find_cover(r, id)).and_then(|p| {
                    deskboxes_render::decode_image_cpu(&p.to_string_lossy(), |w, _| px as f32 / w.max(1) as f32)
                        .map_err(|e| log::warn!("cover of {id}: {} unreadable: {e}", p.display()))
                        .ok()
                });
                (id, img.map(|(i, _)| i))
            })
            .collect();
        log::info!("covers: {} of {} found in {} ms", out.iter().filter(|c| c.1.is_some()).count(), out.len(), t.elapsed().as_millis());
        if init.is_ok() {
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
        post(Ev::Covers(out));
    });
    if let Err(e) = spawned {
        log::warn!("covers: no thread: {e}");
    }
}

impl App {
    /// The cover of `it`, once read (None: not a Steam game, not read yet, or none).
    pub fn cover_of(&self, it: &Item) -> Option<&Bitmap> {
        let id = rules::steam_app_id(&it.link)?;
        self.views.covers.got.get(&id)?.as_ref()
    }

    /// Ask for the covers of `keys` not read yet (once each).
    pub fn request_covers<'a>(&mut self, keys: impl Iterator<Item = &'a ItemKey>) {
        let mut ids = vec![];
        for k in keys {
            let Some(id) = self.items.get(k).and_then(|it| rules::steam_app_id(&it.link)) else { continue };
            if !self.views.covers.got.contains_key(&id) && self.views.covers.pending.insert(id) {
                ids.push(id);
            }
        }
        if !ids.is_empty() {
            load(ids, (COVER_W * self.scale.max(1.0)).round() as u32);
        }
    }

    /// Covers read (see `request_covers`).
    pub fn apply_covers(&mut self, covers: Vec<(u32, Option<IconImage>)>) {
        for (id, img) in covers {
            self.views.covers.pending.remove(&id);
            let b = img.and_then(|i| self.gpu.bitmap(&i).map_err(|e| log::warn!("cover upload: {e}")).ok());
            self.views.covers.got.insert(id, b);
        }
        self.invalidate();
    }

    /// Where a Covers cell's cover goes and its name under it.
    pub fn cover_parts(&self, key: &ItemKey, cell: Rect, full: bool) -> (Rect, Rect) {
        let cover = Rect::new(cell.x + (cell.w - COVER_W) / 2.0, cell.y + 6.0, COVER_W, COVER_H);
        let lw = cell.w - 4.0;
        let lh = self.label_height(key, lw, full);
        (cover, Rect::new(cell.x + 2.0, cover.bottom() + 5.0, lw, lh))
    }
}

/// A Covers cell (the cover, its name in two lines under it).
pub fn cell_size(line_h: f32) -> (f32, f32) {
    (COVER_W + 16.0, 6.0 + COVER_H + 5.0 + 2.0 * line_h + 6.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_covers_in_each_layout() {
        let root = std::env::temp_dir().join(format!("dbx-covers-{}", std::process::id()));
        let cache = root.join("appcache").join("librarycache");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(cache.join("10")).unwrap();
        std::fs::write(cache.join("10").join("library_600x900.jpg"), b"x").unwrap();
        std::fs::create_dir_all(cache.join("20").join("abc123")).unwrap();
        std::fs::write(cache.join("20").join("abc123").join("library_600x900.jpg"), b"x").unwrap();
        std::fs::create_dir_all(cache.join("30")).unwrap();
        std::fs::write(cache.join("30").join("library_capsule.jpg"), b"x").unwrap();
        std::fs::write(cache.join("40_library_600x900.jpg"), b"x").unwrap();
        assert!(find_cover(&root, 10).unwrap().ends_with("10/library_600x900.jpg"));
        assert!(find_cover(&root, 20).unwrap().ends_with("abc123/library_600x900.jpg"));
        assert!(find_cover(&root, 30).unwrap().ends_with("library_capsule.jpg"));
        assert!(find_cover(&root, 40).unwrap().ends_with("40_library_600x900.jpg"));
        assert!(find_cover(&root, 50).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
