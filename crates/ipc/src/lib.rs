//! Core side of the shell-host pipe (docs/ARCHITECTURE.md §11).
//! Frames: u32 little-endian length + UTF-8 JSON `{ "v": 1, "t": type, "id": n, ... }`.
//! The handle is overlapped so the reader thread and writers never serialize on each other.

use serde_json::{json, Value};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
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

#[derive(Clone)]
pub struct HostClient {
    h: Arc<Handle>,
    wlock: Arc<Mutex<()>>,
    next: Arc<AtomicU32>,
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
        let c = HostClient { h: Arc::new(Handle(h)), wlock: Arc::new(Mutex::new(())), next: Arc::new(AtomicU32::new(1)) };
        let hr = c.h.clone();
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

    /// Send a message of type `t` with the fields of `body` (an object). Returns its id.
    pub fn send(&self, t: &str, body: Value) -> windows::core::Result<u32> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut o = json!({ "v": PROTO, "t": t, "id": id });
        if let (Some(dst), Value::Object(src)) = (o.as_object_mut(), body) {
            dst.extend(src);
        }
        let s = serde_json::to_vec(&o).unwrap_or_default();
        let mut frame = (s.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&s);
        let _g = self.wlock.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { io_all(self.h.0, &mut frame, true)? };
        Ok(id)
    }
}

/// Spawn the shell host serving `pipe_name`, at below-normal priority.
pub fn spawn_host(exe: &std::path::Path, pipe_name: &str) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(exe).arg("--pipe").arg(pipe_name).creation_flags(BELOW_NORMAL_PRIORITY_CLASS.0).spawn()
}
