//! Frame clock for animations: while one runs, a thread waits for each vertical blank of the
//! primary monitor (its refresh rate: 60, 144 Hz ...) and asks the UI thread for a frame. Idle
//! otherwise; input-driven frames do not need it.

use crate::events::WM_APP_DRAW;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::OnceLock;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

static ON: AtomicBool = AtomicBool::new(false);
/// The app is closing: no more frames. (Posted frames would keep the queue from ever being empty,
/// and Windows hands out WM_QUIT only then: a two-screen debug build never quit, 2026-11-24.)
static STOPPED: AtomicBool = AtomicBool::new(false);
/// A frame is posted and not drawn yet (shared with `App::invalidate`).
pub static PENDING: AtomicBool = AtomicBool::new(false);
static TARGET: AtomicIsize = AtomicIsize::new(0);
static THREAD: OnceLock<std::thread::Thread> = OnceLock::new();

/// Post one frame request to the UI thread unless one is already queued.
pub fn request_frame() {
    if STOPPED.load(Ordering::Acquire) {
        return;
    }
    if !PENDING.swap(true, Ordering::AcqRel) {
        let h = HWND(TARGET.load(Ordering::Relaxed) as _);
        unsafe {
            let _ = PostMessageW(Some(h), WM_APP_DRAW, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn start(target: HWND) {
    TARGET.store(target.0 as isize, Ordering::Relaxed);
    let t = std::thread::Builder::new()
        .name("frame-clock".into())
        .spawn(|| {
            let out = primary_output();
            if out.is_none() {
                log::warn!("frame clock: no DXGI output for the primary monitor; pacing with DwmFlush");
            }
            loop {
                while !ON.load(Ordering::Acquire) {
                    std::thread::park();
                }
                let ok = out.as_ref().map(|o| unsafe { o.WaitForVBlank().is_ok() }).unwrap_or(false);
                if !ok && unsafe { windows::Win32::Graphics::Dwm::DwmFlush().is_err() } {
                    std::thread::sleep(std::time::Duration::from_millis(7));
                }
                if ON.load(Ordering::Acquire) {
                    request_frame();
                }
            }
        })
        .expect("frame clock thread");
    let _ = THREAD.set(t.thread().clone());
}

/// Animations running (keep frames coming at the refresh rate) or not.
pub fn set(on: bool) {
    let on = on && !STOPPED.load(Ordering::Acquire);
    let was = ON.swap(on, Ordering::AcqRel);
    if on && !was {
        if let Some(t) = THREAD.get() {
            t.unpark();
        }
    }
}

/// No more frames, for good (the app is closing).
pub fn stop() {
    STOPPED.store(true, Ordering::Release);
    ON.store(false, Ordering::Release);
}

fn primary_output() -> Option<IDXGIOutput> {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let f: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut ai = 0;
        while let Ok(a) = f.EnumAdapters1(ai) {
            let mut oi = 0;
            while let Ok(o) = a.EnumOutputs(oi) {
                if o.GetDesc().map(|d| d.Monitor == mon).unwrap_or(false) {
                    return Some(o);
                }
                oi += 1;
            }
            ai += 1;
        }
        None
    }
}
