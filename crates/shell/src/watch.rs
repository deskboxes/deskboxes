//! Folder watchers (docs/ARCHITECTURE.md §2 FS thread). One blocking `ReadDirectoryChangesW` thread
//! per folder; each change batch calls `on_change`. The consumer coalesces and re-scans with the fast
//! path, so no per-record parsing is needed in Phase 1.

use crate::{pcw, wide};
use std::path::PathBuf;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;

pub fn watch(dirs: Vec<PathBuf>, on_change: impl Fn() + Send + Sync + Clone + 'static) {
    for d in dirs {
        let cb = on_change.clone();
        std::thread::Builder::new()
            .name("fs-watch".into())
            .spawn(move || unsafe {
                let p = wide(&d.display().to_string());
                let h = match CreateFileW(
                    pcw(&p),
                    FILE_LIST_DIRECTORY.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS,
                    None,
                ) {
                    Ok(h) => h,
                    Err(e) => {
                        log::warn!("watch {}: {e}", d.display());
                        return;
                    }
                };
                let mut buf = vec![0u8; 64 * 1024];
                let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_ATTRIBUTES | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE;
                loop {
                    let mut n = 0u32;
                    let r = ReadDirectoryChangesW(h, buf.as_mut_ptr() as _, buf.len() as u32, false, filter, Some(&mut n), None, None);
                    if r.is_err() {
                        log::warn!("watch {} ended: {r:?}", d.display());
                        let _ = CloseHandle(h);
                        return;
                    }
                    cb();
                }
            })
            .expect("spawn watcher");
    }
}
