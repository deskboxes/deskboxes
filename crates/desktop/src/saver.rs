//! One thread writes the layout and the icon snapshot, in the order they were made: the UI thread
//! hands over what to write (the layout copied, the snapshot's icons shared, not copied) and goes
//! on. A newer layout or snapshot waiting replaces an older one still waiting (only the newest is
//! written), and one writer means an older one can never be written over a newer one. Each write
//! is atomic (`persist::write_atomic`); the day's layout backup is made before the layout is
//! written over; `flush` waits for all of it (on quitting).

use deskboxes_core::model::MonitorLayout;
use deskboxes_core::persist;
use deskboxes_core::snapshot::SnapshotParts;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub struct LayoutJob {
    pub layouts: PathBuf,
    pub backups: PathBuf,
    pub topology: String,
    pub layout: MonitorLayout,
}

pub struct SnapshotJob {
    pub path: PathBuf,
    pub snap: SnapshotParts,
}

struct Queue {
    layout: Option<LayoutJob>,
    snapshot: Option<SnapshotJob>,
    /// The writer is writing what it took.
    busy: bool,
    started: bool,
    /// What each layout file was last written with (a save of the same layout writes nothing).
    written: Vec<(PathBuf, Vec<u8>)>,
}

static Q: Mutex<Queue> = Mutex::new(Queue { layout: None, snapshot: None, busy: false, started: false, written: Vec::new() });
/// Work handed over, or the writer gone idle.
static CV: Condvar = Condvar::new();

fn lock() -> MutexGuard<'static, Queue> {
    Q.lock().unwrap_or_else(|p| p.into_inner())
}

/// Write this layout (after any older one waiting, which it replaces).
pub fn layout(job: LayoutJob) {
    hand_over(|q| q.layout = Some(job));
}

/// Write this snapshot (after any older one waiting, which it replaces).
pub fn snapshot(job: SnapshotJob) {
    hand_over(|q| q.snapshot = Some(job));
}

fn hand_over(f: impl FnOnce(&mut Queue)) {
    let mut q = lock();
    f(&mut q);
    if !q.started {
        q.started = true;
        if let Err(e) = std::thread::Builder::new().name("saver".into()).spawn(run) {
            // No thread: written here and now.
            log::warn!("saver: no thread: {e}");
            q.started = false;
            let (l, s) = (q.layout.take(), q.snapshot.take());
            drop(q);
            write(l, s);
            return;
        }
    }
    CV.notify_all();
}

fn run() {
    loop {
        let (l, s) = {
            let mut q = lock();
            while q.layout.is_none() && q.snapshot.is_none() {
                if q.busy {
                    q.busy = false;
                    CV.notify_all();
                }
                q = CV.wait(q).unwrap_or_else(|p| p.into_inner());
            }
            q.busy = true;
            (q.layout.take(), q.snapshot.take())
        };
        write(l, s);
    }
}

fn write(l: Option<LayoutJob>, s: Option<SnapshotJob>) {
    if let Some(j) = l {
        let path = persist::layout_path(&j.layouts, &j.topology);
        let bytes = persist::layout_bytes(&j.topology, &j.layout);
        let same = lock().written.iter().any(|(p, b)| *p == path && *b == bytes);
        if !same {
            let _ = persist::backup_layouts_daily(&j.layouts, &j.backups);
            match persist::write_atomic(&path, &bytes) {
                Ok(()) => {
                    let mut q = lock();
                    q.written.retain(|(p, _)| *p != path);
                    q.written.push((path, bytes));
                }
                Err(e) => log::warn!("save layout: {e}"),
            }
        }
    }
    if let Some(j) = s {
        if let Err(e) = persist::write_atomic(&j.path, &j.snap.encode()) {
            log::warn!("save snapshot: {e}");
        }
    }
}

/// Wait until everything handed over is written (at most `limit`).
pub fn flush(limit: Duration) {
    let until = Instant::now() + limit;
    let mut q = lock();
    while q.started && (q.busy || q.layout.is_some() || q.snapshot.is_some()) {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            log::warn!("saver: still writing after {} ms", limit.as_millis());
            return;
        }
        q = CV.wait_timeout(q, left).unwrap_or_else(|p| p.into_inner()).0;
    }
}

/// A layout file was written by someone else (an import): the next save writes it, same or not.
pub fn forget_written() {
    lock().written.clear();
}
