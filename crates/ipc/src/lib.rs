//! Core side of the shell-host pipe (docs/ARCHITECTURE.md §11).
//! Frames: u32 little-endian length + UTF-8 JSON `{ "v": 1, "t": type, "id": n, ... }`.
//! The handle is overlapped so the reader thread and writers never serialize on each other.
//! Writes go through a queue and a thread of their own (`send` never waits for the host to read):
//! in order, at most `MAX_BACKLOG` bytes waiting, a menu prefetch not yet written replaced by a
//! newer one (or dropped for a menu shown).

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Pipes::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::IO::*;

pub const PROTO: u32 = 1;

struct Handle(HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Overlapped I/O on `h`, waiting for completion. Returns bytes transferred.
unsafe fn io(h: HANDLE, buf: *mut u8, len: u32, write: bool) -> windows::core::Result<u32> {
    let ev = CreateEventW(None, true, false, None)?;
    let mut ov = OVERLAPPED { hEvent: ev, ..Default::default() };
    let r = if write {
        WriteFile(h, Some(std::slice::from_raw_parts(buf, len as usize)), None, Some(&mut ov))
    } else {
        ReadFile(h, Some(std::slice::from_raw_parts_mut(buf, len as usize)), None, Some(&mut ov))
    };
    let mut n = 0u32;
    let res = match r {
        Ok(()) => GetOverlappedResult(h, &ov, &mut n, true),
        Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => GetOverlappedResult(h, &ov, &mut n, true),
        Err(e) => Err(e),
    };
    let _ = CloseHandle(ev);
    res.map(|_| n)
}

unsafe fn io_all(h: HANDLE, buf: &mut [u8], write: bool) -> windows::core::Result<()> {
    let mut done = 0usize;
    while done < buf.len() {
        let n = io(h, buf.as_mut_ptr().add(done), (buf.len() - done) as u32, write)?;
        if n == 0 {
            return Err(ERROR_BROKEN_PIPE.to_hresult().into());
        }
        done += n as usize;
    }
    Ok(())
}

/// Bytes waiting to be written, at most: past it the host is taken to be stuck (`send` fails).
const MAX_BACKLOG: usize = 64 << 20;

/// Frames waiting for the writer thread.
#[derive(Default)]
struct OutQ {
    /// (type, frame, when it was handed over)
    frames: VecDeque<(String, Vec<u8>, Instant)>,
    bytes: usize,
    /// A write failed: the pipe is gone.
    broken: bool,
    /// Every client is gone: the writer writes what is left and ends.
    closed: bool,
}

struct Out {
    q: Mutex<OutQ>,
    cv: Condvar,
}

impl Out {
    fn lock(&self) -> std::sync::MutexGuard<'_, OutQ> {
        self.q.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Dropped with the last client: the writer thread ends.
struct Life(Arc<Out>);
impl Drop for Life {
    fn drop(&mut self) {
        self.0.lock().closed = true;
        self.0.cv.notify_all();
    }
}

#[derive(Clone)]
pub struct HostClient {
    out: Arc<Out>,
    _life: Arc<Life>,
    next: Arc<AtomicU32>,
}

/// The writer thread: each frame in order, until the pipe breaks or every client is gone.
fn write_loop(h: Arc<Handle>, out: Arc<Out>) {
    loop {
        let (t, mut frame, at) = {
            let mut q = out.lock();
            loop {
                if let Some(f) = q.frames.pop_front() {
                    q.bytes -= f.1.len();
                    break f;
                }
                if q.closed || q.broken {
                    return;
                }
                q = out.cv.wait(q).unwrap_or_else(|p| p.into_inner());
            }
        };
        let waited = at.elapsed();
        if let Err(e) = unsafe { io_all(h.0, &mut frame, true) } {
            log::warn!("ipc: writing '{t}': {e}");
            let mut q = out.lock();
            q.broken = true;
            q.frames.clear();
            q.bytes = 0;
            return;
        }
        // (How long it waited for the host to read, apart from how long anything took to handle.)
        let ms = at.elapsed().as_millis();
        if ms >= 250 {
            log::warn!("ipc: '{t}' written {ms} ms after it was sent (waited {} ms)", waited.as_millis());
        }
    }
}

impl HostClient {
    /// Connect to `\\.\pipe\<name>`, retrying until `timeout` (the host creates the pipe).
    /// `on_msg` runs on the reader thread for every message; `None` = disconnected.
    pub fn connect(name: &str, timeout: Duration, on_msg: impl Fn(Option<Value>) + Send + 'static) -> windows::core::Result<HostClient> {
        let path = HSTRING::from(format!(r"\\.\pipe\{name}"));
        let end = Instant::now() + timeout;
        let h = loop {
            let r = unsafe {
                CreateFileW(PCWSTR(path.as_ptr()), (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_NONE, None, OPEN_EXISTING, FILE_FLAG_OVERLAPPED, None)
            };
            match r {
                Ok(h) => break h,
                Err(e) if Instant::now() < end => {
                    if e.code() == ERROR_PIPE_BUSY.to_hresult() {
                        unsafe {
                            let _ = WaitNamedPipeW(PCWSTR(path.as_ptr()), 200);
                        }
                    } else {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                }
                Err(e) => return Err(e),
            }
        };
        unsafe {
            let mode = PIPE_READMODE_BYTE;
            let _ = SetNamedPipeHandleState(h, Some(&mode), None, None);
        }
        let h = Arc::new(Handle(h));
        let out = Arc::new(Out { q: Mutex::new(OutQ::default()), cv: Condvar::new() });
        let c = HostClient { out: out.clone(), _life: Arc::new(Life(out.clone())), next: Arc::new(AtomicU32::new(1)) };
        let hw = h.clone();
        std::thread::Builder::new().name("ipc-writer".into()).spawn(move || write_loop(hw, out)).expect("spawn ipc writer");
        let hr = h;
        std::thread::Builder::new()
            .name("ipc-reader".into())
            .spawn(move || loop {
                let mut len = [0u8; 4];
                if unsafe { io_all(hr.0, &mut len, false) }.is_err() {
                    on_msg(None);
                    return;
                }
                let n = u32::from_le_bytes(len) as usize;
                if n > 16 << 20 {
                    on_msg(None);
                    return;
                }
                let mut b = vec![0u8; n];
                if unsafe { io_all(hr.0, &mut b, false) }.is_err() {
                    on_msg(None);
                    return;
                }
                match serde_json::from_slice::<Value>(&b) {
                    Ok(v) => on_msg(Some(v)),
                    Err(e) => log::warn!("ipc: bad frame: {e}"),
                }
            })
            .expect("spawn ipc reader");
        Ok(c)
    }

    /// Send a message of type `t` with the fields of `body` (an object). Returns its id. Handed
    /// to the writer thread (written in order, without waiting here); fails when the pipe is gone
    /// or the host has not read `MAX_BACKLOG` bytes.
    pub fn send(&self, t: &str, body: Value) -> windows::core::Result<u32> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut o = json!({ "v": PROTO, "t": t, "id": id });
        if let (Some(dst), Value::Object(src)) = (o.as_object_mut(), body) {
            dst.extend(src);
        }
        let s = serde_json::to_vec(&o).unwrap_or_default();
        let mut frame = Vec::with_capacity(4 + s.len());
        frame.extend_from_slice(&(s.len() as u32).to_le_bytes());
        frame.extend_from_slice(&s);
        let mut q = self.out.lock();
        if q.broken {
            return Err(ERROR_BROKEN_PIPE.to_hresult().into());
        }
        // A menu prefetch not written yet is of no use any more: a newer one replaces it, and a
        // menu shown goes first (the host drops its prefetch on every show anyway).
        if t == "prefetch" || t == "show" {
            let OutQ { frames, bytes, .. } = &mut *q;
            frames.retain(|(ft, f, _)| {
                let keep = ft != "prefetch";
                if !keep {
                    *bytes -= f.len();
                }
                keep
            });
        }
        if q.bytes + frame.len() > MAX_BACKLOG {
            log::warn!("ipc: the host has not read {} bytes; '{t}' not sent", q.bytes);
            return Err(ERROR_NOT_ENOUGH_QUOTA.to_hresult().into());
        }
        q.bytes += frame.len();
        q.frames.push_back((t.to_string(), frame, Instant::now()));
        drop(q);
        self.out.cv.notify_all();
        Ok(id)
    }
}

/// Spawn the shell host serving `pipe_name`, at below-normal priority.
/// (At normal priority: a menu is waited for. Below normal, a busy slow PC kept a right-click
/// waiting; the host's own background work, the package scan, runs below normal itself.)
pub fn spawn_host(exe: &std::path::Path, pipe_name: &str) -> std::io::Result<std::process::Child> {
    std::process::Command::new(exe).arg("--pipe").arg(pipe_name).spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host that does not read for a while: sending does not wait for it; then everything
    /// comes in order, the prefetches superseded before they were written left out.
    #[test]
    fn sends_do_not_wait_for_a_slow_reader() {
        let name = format!("dbx-ipc-test-{}", std::process::id());
        let path = HSTRING::from(format!(r"\\.\pipe\{name}"));
        let server = unsafe {
            CreateNamedPipeW(PCWSTR(path.as_ptr()), PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT, 1, 4096, 4096, 0, None)
        };
        assert!(!server.is_invalid(), "{:?}", unsafe { GetLastError() });
        let server = Handle(server);
        let c = HostClient::connect(&name, Duration::from_secs(5), |_| {}).unwrap();
        unsafe {
            let _ = ConnectNamedPipe(server.0, None);
        }
        // Far more than the pipe's buffer, with no one reading.
        let big = "x".repeat(256 * 1024);
        let t = Instant::now();
        c.send("settings", json!({ "n": 1, "pad": big })).unwrap();
        c.send("prefetch", json!({ "n": 2 })).unwrap();
        c.send("prefetch", json!({ "n": 3 })).unwrap();
        c.send("settings", json!({ "n": 4, "pad": big })).unwrap();
        c.send("prefetch", json!({ "n": 5 })).unwrap();
        c.send("show", json!({ "n": 6 })).unwrap();
        c.send("quit", json!({ "n": 7 })).unwrap();
        assert!(t.elapsed() < Duration::from_millis(500), "sending waited {:?}", t.elapsed());
        std::thread::sleep(Duration::from_millis(300));
        let mut got = vec![];
        while got.last() != Some(&7) {
            let mut len = [0u8; 4];
            unsafe { io_all(server.0, &mut len, false) }.unwrap();
            let mut b = vec![0u8; u32::from_le_bytes(len) as usize];
            unsafe { io_all(server.0, &mut b, false) }.unwrap();
            let v: Value = serde_json::from_slice(&b).unwrap();
            got.push(v["n"].as_i64().unwrap());
        }
        // (The prefetch being written when the next came may still arrive; the rest in order.)
        assert!(got.starts_with(&[1]));
        assert!(got.ends_with(&[4, 6, 7]), "{got:?}");
        assert!(!got.contains(&5), "{got:?}");
        assert!(got.windows(2).all(|w| w[0] < w[1]), "{got:?}");
    }
}
