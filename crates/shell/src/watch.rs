//! Folder watchers (docs/ARCHITECTURE.md §2 FS thread). One blocking `ReadDirectoryChangesW` thread
//! per folder; each change batch calls `on_change`. The consumer coalesces and re-scans with the fast
//! path, so no per-record parsing is needed in Phase 1.

use crate::{pcw, wide};
use std::path::PathBuf;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;

/// A folder watcher that can be stopped: dropping it (or `cancel`) ends its thread and closes
/// the folder (the blocking read is cancelled).
pub struct Watch {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The folder's handle while the thread has it open (0: none).
    h: std::sync::Arc<std::sync::Mutex<isize>>,
}

impl Watch {
    pub fn cancel(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let h = self.h.lock().unwrap();
        if *h != 0 {
            unsafe {
                let _ = windows::Win32::System::IO::CancelIoEx(HANDLE(*h as _), None);
            }
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Watch one folder until the returned `Watch` is dropped.
pub fn watch_one(dir: PathBuf, on_change: impl Fn() + Send + 'static) -> Watch {
    use std::sync::atomic::Ordering;
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hm = std::sync::Arc::new(std::sync::Mutex::new(0isize));
    let w = Watch { stop: stop.clone(), h: hm.clone() };
    let r = std::thread::Builder::new().name("fs-watch".into()).spawn(move || unsafe {
        let p = wide(&dir.display().to_string());
        let h = match CreateFileW(pcw(&p), FILE_LIST_DIRECTORY.0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, None) {
            Ok(h) => h,
            Err(e) => {
                log::warn!("watch {}: {e}", dir.display());
                return;
            }
        };
        *hm.lock().unwrap() = h.0 as isize;
        let mut buf = vec![0u8; 64 * 1024];
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_ATTRIBUTES | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE;
        while !stop.load(Ordering::SeqCst) {
            let mut n = 0u32;
            let r = ReadDirectoryChangesW(h, buf.as_mut_ptr() as _, buf.len() as u32, false, filter, Some(&mut n), None, None);
            if stop.load(Ordering::SeqCst) {
                break;
            }
            if r.is_err() {
                log::warn!("watch {} ended: {r:?}", dir.display());
                break;
            }
            on_change();
        }
        let mut g = hm.lock().unwrap();
        *g = 0;
        let _ = CloseHandle(h);
    });
    if let Err(e) = r {
        log::warn!("watch: no thread: {e}");
    }
    w
}

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
