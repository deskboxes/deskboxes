//! Boxes above windows (Settings › Boxes above windows): a shortcut, or a screen corner, brings
//! the boxes above the open windows for a moment, so a file can be opened or dropped into a box
//! without minimizing anything. The windows behind are dimmed a little. Clicking anywhere
//! outside a box, Esc, the shortcut again, or switching to another app puts them back.
//!
//! How: a top-level window over the monitor (not topmost, so our menus still open above it)
//! takes over the surface's composition visual and input while it is up (`App::hwnd` points at
//! it); the desktop surface gets it back afterwards. Nothing is drawn twice.
//!
//! Dragging while they are up: the dim fades and the window shrinks (its region) to the boxes,
//! so outside them the drag reaches the app below (a file dropped into it), or the desktop where
//! no window is; over a box, the box takes it. Back to the whole screen when the drag ends.

use crate::app::{ease_out, App};
use deskboxes_core::*;
use std::time::Instant;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const HK_ABOVE: i32 = 0x4442;
/// Dimming in, s.
const DIM_S: f32 = 0.15;
/// How long the pointer rests in the corner before the boxes come up, ms.
const CORNER_MS: u128 = 250;

/// The shortcuts Settings offers: id, modifiers, key.
pub const HOTKEYS: &[(&str, HOT_KEY_MODIFIERS, u32)] = &[
    ("win+shift+d", HOT_KEY_MODIFIERS(MOD_WIN.0 | MOD_SHIFT.0), b'D' as u32),
    ("ctrl+alt+d", HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0), b'D' as u32),
    ("ctrl+shift+space", HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0), 0x20),
    ("ctrl+alt+b", HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_ALT.0), b'B' as u32),
];

/// Where other apps' windows are on the monitor (px relative to its top-left, clipped to it):
/// visible, not minimized or cloaked, not ours, not the desktop or the taskbar.
fn app_windows(mon: RECT) -> Vec<RECT> {
    use windows::Win32::Graphics::Dwm::*;
    let mut rects: Vec<RECT> = vec![];
    unsafe extern "system" fn each(hw: HWND, l: LPARAM) -> windows::core::BOOL {
        let v = &mut *(l.0 as *mut Vec<RECT>);
        if !IsWindowVisible(hw).as_bool() || IsIconic(hw).as_bool() {
            return windows::core::BOOL(1);
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(hw, DWMWA_CLOAKED, &mut cloaked as *mut u32 as _, 4);
        if cloaked != 0 {
            return windows::core::BOOL(1);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hw, Some(&mut pid));
        if pid == std::process::id() {
            return windows::core::BOOL(1);
        }
        // Click-through overlays (game and recording overlays) cover the screen but show nothing.
        if GetWindowLongW(hw, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0 != 0 {
            return windows::core::BOOL(1);
        }
        let mut cls = [0u16; 64];
        let n = GetClassNameW(hw, &mut cls) as usize;
        let c = String::from_utf16_lossy(&cls[..n]);
        if matches!(c.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return windows::core::BOOL(1);
        }
        let mut r = RECT::default();
        if DwmGetWindowAttribute(hw, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as _, std::mem::size_of::<RECT>() as u32).is_err() {
            let _ = GetWindowRect(hw, &mut r);
        }
        if r.right > r.left && r.bottom > r.top {
            v.push(r);
        }
        windows::core::BOOL(1)
    }
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut rects as *mut Vec<RECT> as isize));
    }
    rects
        .into_iter()
        .filter_map(|r| {
            let (l, t, rr, b) = (r.left.max(mon.left) - mon.left, r.top.max(mon.top) - mon.top, r.right.min(mon.right) - mon.left, r.bottom.min(mon.bottom) - mon.top);
            (rr > l && b > t).then_some(RECT { left: l, top: t, right: rr, bottom: b })
        })
        .collect()
}

/// The monitor's pixels where other apps' windows are (premultiplied BGRA; transparent where
/// the desktop shows, which is ours: its boxes would be blurred into their own backdrop).
fn capture_windows(mon: RECT) -> Option<deskboxes_core::snapshot::IconImage> {
    use windows::Win32::Graphics::Gdi::*;
    let (w, h) = (mon.right - mon.left, mon.bottom - mon.top);
    if w <= 0 || h <= 0 {
        return None;
    }
    let inside = app_windows(mon);
    if inside.is_empty() {
        // Nothing open: the wallpaper alone (no capture needed).
        return Some(deskboxes_core::snapshot::IconImage { w: w as u32, h: h as u32, bgra: vec![0; (w * h * 4) as usize] });
    }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(dib) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            return None;
        };
        let old = SelectObject(mem, dib.into());
        let ok = BitBlt(mem, 0, 0, w, h, Some(screen), mon.left, mon.top, SRCCOPY).is_ok();
        SelectObject(mem, old);
        let src = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
        // Only the windows' pixels, opaque; the rest transparent.
        let mut out = vec![0u8; src.len()];
        if ok {
            let row = (w * 4) as usize;
            for r in &inside {
                for y in r.top..r.bottom {
                    let a = y as usize * row + r.left as usize * 4;
                    let b = y as usize * row + r.right as usize * 4;
                    out[a..b].copy_from_slice(&src[a..b]);
                    for px in out[a..b].chunks_exact_mut(4) {
                        px[3] = 255;
                    }
                }
            }
        }
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        ok.then_some(deskboxes_core::snapshot::IconImage { w: w as u32, h: h as u32, bgra: out })
    }
}

impl App {
    /// (Re)register the shortcut from Settings; remembers whether Windows gave it to us.
    pub fn register_above_hotkey(&mut self) {
        unsafe {
            let _ = UnregisterHotKey(Some(self.msg_hwnd), HK_ABOVE);
            self.above_hotkey_ok = true;
            let want = self.settings.above.hotkey.clone();
            if let Some((_, m, vk)) = HOTKEYS.iter().find(|(id, _, _)| *id == want) {
                self.above_hotkey_ok = RegisterHotKey(Some(self.msg_hwnd), HK_ABOVE, *m | MOD_NOREPEAT, *vk).is_ok();
                if !self.above_hotkey_ok {
                    log::warn!("boxes above windows: shortcut {want} is taken");
                }
            }
        }
    }

    pub fn is_above(&self) -> bool {
        self.above.is_some()
    }

    /// How far the windows behind are dimmed (0..dim).
    pub fn above_dim(&self) -> f32 {
        let undim = match self.above_undim {
            Some((true, t0)) => 1.0 - ease_out(t0.elapsed().as_secs_f32() / DIM_S),
            Some((false, t0)) => ease_out(t0.elapsed().as_secs_f32() / DIM_S),
            None => 1.0,
        };
        match &self.above {
            Some((_, _, t0)) if self.settings.above.dim => 0.24 * ease_out(t0.elapsed().as_secs_f32() / DIM_S) * undim,
            _ => 0.0,
        }
    }

    pub fn above_animating(&self) -> bool {
        self.above.as_ref().map(|(_, _, t0)| t0.elapsed().as_secs_f32() < DIM_S).unwrap_or(false)
            || self.above_undim.map(|(_, t0)| t0.elapsed().as_secs_f32() < DIM_S).unwrap_or(false)
    }

    /// The desktop is in front (it, or our surface, has the focus), or no app window is open on
    /// the screen: the boxes are in front already, so the shortcut and the corner do nothing.
    pub fn desktop_in_front(&self) -> bool {
        unsafe {
            let fg = GetForegroundWindow();
            if fg.0.is_null() {
                return true;
            }
            let root = GetAncestor(fg, GA_ROOT);
            let mut cls = [0u16; 64];
            let n = GetClassNameW(root, &mut cls) as usize;
            let c = String::from_utf16_lossy(&cls[..n]);
            let mut pid = 0u32;
            GetWindowThreadProcessId(root, Some(&mut pid));
            if matches!(c.as_str(), "Progman" | "WorkerW") || pid == std::process::id() {
                return true;
            }
        }
        app_windows(self.mon).is_empty()
    }

    /// A drag starts over the boxes (ours, or one from an app entering them): the dim fades, then
    /// the window shrinks to the boxes so the apps behind can take the drop.
    pub fn above_drag_begin(&mut self) {
        if !self.is_above() || self.above_drag.is_some() {
            return;
        }
        log::info!("boxes above windows: drag; the windows behind take drops outside the boxes");
        self.above_drag = Some(Instant::now());
        self.above_undim = Some((true, Instant::now()));
        unsafe {
            SetTimer(Some(self.msg_hwnd), crate::app::T_ABOVE_DRAG, 40, None);
        }
        crate::anim::set(true);
        self.invalidate();
    }

    /// The drag is over (dropped or cancelled): the whole screen again, dimmed again, and the
    /// desktop behind shown as it is now (an icon may have been dropped there).
    pub fn above_drag_end(&mut self) {
        if self.above_drag.take().is_none() {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.msg_hwnd), crate::app::T_ABOVE_DRAG);
        }
        self.set_above_region(vec![]);
        if self.is_above() {
            self.above_undim = Some((false, Instant::now()));
            self.redraw_still();
        } else {
            self.above_undim = None;
        }
        crate::anim::set(true);
        self.invalidate();
    }

    /// While dragging: once the dim is gone, the window covers only the boxes (as they are now:
    /// one opened on hover grows). And the drag's end, noticed by the buttons being up (a drag
    /// from an app that ends elsewhere doesn't tell us).
    pub fn above_drag_tick(&mut self) {
        let Some(t0) = self.above_drag else {
            unsafe {
                let _ = KillTimer(Some(self.msg_hwnd), crate::app::T_ABOVE_DRAG);
            }
            return;
        };
        let held = unsafe { [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|v| GetAsyncKeyState(v.0 as i32) as u16 & 0x8000 != 0) };
        if !held && self.own_drag.is_none() {
            self.above_drag_end();
            return;
        }
        if t0.elapsed().as_secs_f32() >= DIM_S {
            let s = self.scale;
            let (ox, oy) = self.above_off;
            let rects: Vec<(i32, i32, i32, i32)> = self
                .layout
                .boxes
                .iter()
                .map(|b| self.shown_rect(b).inflate(2.0))
                .map(|r| ((r.x * s).floor() as i32 - ox, (r.y * s).floor() as i32 - oy, (r.right() * s).ceil() as i32 - ox, (r.bottom() * s).ceil() as i32 - oy))
                .collect();
            self.set_above_region(rects);
        }
        // The WM_TIMER dispatcher kills every timer it dispatches: armed again while the drag
        // lasts (to shrink the window after the dim, and to notice a drag ending elsewhere).
        if self.above_drag.is_some() {
            unsafe {
                SetTimer(Some(self.msg_hwnd), crate::app::T_ABOVE_DRAG, 40, None);
            }
        }
    }

    /// The boxes-above window answers (and shows) only inside `rects` (empty: everywhere).
    fn set_above_region(&mut self, rects: Vec<(i32, i32, i32, i32)>) {
        if rects == self.above_rgn {
            return;
        }
        let Some((hw, _, _)) = self.above else {
            self.above_rgn.clear();
            return;
        };
        unsafe {
            use windows::Win32::Graphics::Gdi::*;
            if rects.is_empty() {
                SetWindowRgn(hw, None, true);
            } else {
                let rgn = CreateRectRgn(0, 0, 0, 0);
                for (l, t, r, b) in &rects {
                    let one = CreateRectRgn(*l, *t, *r, *b);
                    CombineRgn(Some(rgn), Some(rgn), Some(one), RGN_OR);
                    let _ = DeleteObject(one.into());
                }
                // (Windows owns the region from here.)
                SetWindowRgn(hw, Some(rgn), true);
            }
        }
        self.above_rgn = rects;
    }

    /// The desktop window behind the boxes-above window, drawn again (its icons as they are now).
    fn redraw_still(&mut self) {
        if let Some(surf) = self.surf.take() {
            let scale = self.scale;
            let still = |dc: &windows::Win32::Graphics::Direct2D::ID2D1DeviceContext| self.paint_still(dc);
            if let Err(e) = surf.redraw_still(&self.gpu, scale, &still) {
                log::warn!("boxes above windows: desktop behind: {e}");
            }
            self.surf = Some(surf);
        }
    }

    pub fn toggle_above(&mut self) {
        let on = !self.is_above();
        self.set_above(on);
    }

    pub fn set_above(&mut self, on: bool) {
        if on == self.is_above() {
            return;
        }
        if on {
            let t0 = Instant::now();
            // The windows behind, as they are now: the boxes' backdrop while above them (in place
            // of the wallpaper, which is not behind them now).
            self.above_backdrops();
            let Some((sw, sh)) = self.surf.as_ref().map(|s| (s.w as i32, s.h as i32)) else { return };
            // Over the work area only: the taskbar stays in front (a window as big as the screen
            // is taken for a full-screen app and the taskbar goes behind it, uncovering the parts
            // of boxes it hides).
            let work = unsafe {
                let mut mi = windows::Win32::Graphics::Gdi::MONITORINFO { cbSize: std::mem::size_of::<windows::Win32::Graphics::Gdi::MONITORINFO>() as u32, ..Default::default() };
                let hm = windows::Win32::Graphics::Gdi::MonitorFromPoint(POINT { x: self.mon.left, y: self.mon.top }, windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST);
                let _ = windows::Win32::Graphics::Gdi::GetMonitorInfoW(hm, &mut mi);
                mi.rcWork
            };
            let (x, y) = (work.left.max(self.origin_px.x), work.top.max(self.origin_px.y));
            let (r, b) = (work.right.min(self.origin_px.x + sw), work.bottom.min(self.origin_px.y + sh));
            let (w, h) = ((r - x).max(1), (b - y).max(1));
            let off = (x - self.origin_px.x, y - self.origin_px.y);
            let hw = unsafe {
                CreateWindowExW(
                    WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW,
                    w!("DeskboxesAbove"),
                    w!("Deskboxes"),
                    WS_POPUP,
                    x,
                    y,
                    w,
                    h,
                    None,
                    None,
                    GetModuleHandleW(None).ok().map(|m| m.into()),
                    None,
                )
            };
            let Ok(hw) = hw else {
                log::warn!("boxes above windows: no window");
                self.restore_backdrops();
                return;
            };
            // The desktop keeps showing the wallpaper and its icons meanwhile.
            let surf = self.surf.take();
            let r = match surf.as_ref() {
                Some(_) => {
                    let mut s = surf.unwrap();
                    let scale = self.scale;
                    let still = |dc: &windows::Win32::Graphics::Direct2D::ID2D1DeviceContext| self.paint_still(dc);
                    let r = s.retarget(&self.gpu, Some(hw), off, Some((scale, &still)));
                    self.surf = Some(s);
                    r
                }
                None => return,
            };
            if let Err(e) = r {
                log::warn!("boxes above windows: {e}");
                unsafe {
                    let _ = DestroyWindow(hw);
                }
                self.restore_backdrops();
                return;
            }
            self.above_off = off;
            log::info!("boxes above windows: up in {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
            unsafe {
                let _ = windows::Win32::System::Ole::RegisterDragDrop(hw, &crate::droptarget::DropTarget::create(hw));
                let _ = ShowWindow(hw, SW_SHOW);
                let _ = SetForegroundWindow(hw);
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(hw));
            }
            self.end_rename(true);
            self.set_tucked(false);
            self.above = Some((hw, self.hwnd, Instant::now()));
            self.above_had_front = false;
            self.above_drag = None;
            self.above_undim = None;
            self.above_rgn.clear();
            self.hwnd = hw;
            self.above_poll();
            log::info!("boxes above windows: on");
        } else {
            if self.above_drag.take().is_some() {
                self.set_above_region(vec![]);
                unsafe {
                    let _ = KillTimer(Some(self.msg_hwnd), crate::app::T_ABOVE_DRAG);
                }
            }
            self.above_undim = None;
            self.above_rgn.clear();
            let Some((hw, desk, _)) = self.above.take() else { return };
            self.cancel_drag();
            self.hwnd = desk;
            if let Some(surf) = self.surf.as_mut() {
                if let Err(e) = surf.retarget(&self.gpu, None, (0, 0), None) {
                    log::warn!("boxes above windows: back: {e}");
                }
            }
            self.above_off = (0, 0);
            self.restore_backdrops();
            unsafe {
                let _ = windows::Win32::System::Ole::RevokeDragDrop(hw);
                let _ = DestroyWindow(hw);
            }
            self.above_poll();
            log::info!("boxes above windows: off");
        }
        self.hover = None;
        crate::anim::set(true);
        self.invalidate();
    }

    /// The boxes' backdrops while they are above the windows: what is behind them now (the
    /// windows where there are windows, the wallpaper elsewhere), blurred as their wallpaper is.
    fn above_backdrops(&mut self) {
        let t0 = Instant::now();
        let (mw, mh) = ((self.mon.right - self.mon.left) as u32, (self.mon.bottom - self.mon.top) as u32);
        let Some(cap) = capture_windows(self.mon) else {
            log::warn!("boxes above windows: no capture; boxes keep the wallpaper");
            return;
        };
        let Ok(cap) = self.gpu.bitmap(&cap) else { return };
        let Some(wall) = self.wall.as_ref() else { return };
        // The wallpaper, then the windows over it.
        let Ok(behind) = self.gpu.offscreen(mw, mh, |dc| {
            deskboxes_render::draw_bitmap(dc, wall, deskboxes_render::rect(0.0, 0.0, mw as f32, mh as f32), 1.0);
            deskboxes_render::draw_bitmap(dc, &cap, deskboxes_render::rect(0.0, 0.0, mw as f32, mh as f32), 1.0);
            Ok(())
        }) else {
            return;
        };
        let mut blurs: Vec<i32> = self.layout.boxes.iter().map(|b| b.style.blur as i32).collect();
        blurs.push(self.settings.default_style.blur as i32);
        blurs.sort();
        blurs.dedup();
        let mut fresh = std::collections::HashMap::new();
        for b in blurs.into_iter().filter(|b| *b >= 1) {
            let black = deskboxes_render::rgba(0, 0, 0, 1.0);
            if let Ok(bm) = self.gpu.blurred_wallpaper(Some((&behind, (mw, mh))), black, 2, mw, mh, self.backdrop_down, b as f32 * self.scale) {
                fresh.insert(b, bm);
            }
        }
        self.saved_backdrops = Some(std::mem::replace(&mut self.backdrops, fresh));
        self.above_wall = Some(behind);
        log::info!("boxes above windows: backdrops from the screen in {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
    }

    /// Back on the desktop: the wallpaper's backdrops again.
    fn restore_backdrops(&mut self) {
        if let Some(b) = self.saved_backdrops.take() {
            self.backdrops = b;
        }
        self.above_wall = None;
    }

    /// Keep polling (the corner, and who is in front) while it can matter.
    pub fn above_poll(&mut self) {
        unsafe {
            if self.is_above() || !self.settings.above.corner.is_empty() {
                SetTimer(Some(self.msg_hwnd), crate::app::T_ABOVE, 120, None);
            } else {
                let _ = KillTimer(Some(self.msg_hwnd), crate::app::T_ABOVE);
            }
        }
    }

    /// Another app came to the front (a file opened from a box, Alt+Tab, a click on the taskbar):
    /// put the boxes back. Our own menus and Settings keep them up. Only once our window has had
    /// the front: the corner, reached while dragging from another app, cannot take it.
    pub fn above_check_foreground(&mut self) {
        // (Not in the middle of a drag: it goes where it was going first.)
        if !self.is_above() || self.above_drag.is_some() {
            return;
        }
        unsafe {
            let fg = GetForegroundWindow();
            let mut pid = 0u32;
            GetWindowThreadProcessId(fg, Some(&mut pid));
            let ours = !fg.0.is_null() && (pid == std::process::id() || self.host_child.as_ref().map(|c| c.id() == pid).unwrap_or(false));
            if ours {
                self.above_had_front = true;
            } else if self.above_had_front {
                self.set_above(false);
            }
        }
    }

    /// Hot corner: the pointer resting in the chosen corner of the monitor brings the boxes up.
    pub fn corner_tick(&mut self) {
        let c = self.settings.above.corner.clone();
        if c.is_empty() {
            self.corner_since = None;
            return;
        }
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
        }
        let m = self.mon;
        let (l, t, r, b) = (p.x <= m.left + 1, p.y <= m.top + 1, p.x >= m.right - 2, p.y >= m.bottom - 2);
        // On this monitor (a monitor beside it lines up with its edges too).
        let on_mon = p.x >= m.left && p.x < m.right && p.y >= m.top && p.y < m.bottom;
        let inside = on_mon
            && match c.as_str() {
                "tl" => l && t,
                "tr" => r && t,
                "bl" => l && b,
                _ => r && b,
            };
        // (With a button held too: dragging a file from a window to the corner brings the boxes
        // up to drop it into one.)
        if !inside {
            self.corner_since = None;
            return;
        }
        match self.corner_since {
            None => self.corner_since = Some(Instant::now()),
            Some(t0) if t0.elapsed().as_millis() >= CORNER_MS && !self.is_above() => {
                self.corner_since = Some(Instant::now() + std::time::Duration::from_secs(3600));
                if self.desktop_in_front() {
                    log::info!("boxes above windows: corner ignored (the desktop is in front)");
                } else {
                    self.set_above(true);
                }
            }
            _ => {}
        }
    }

    /// The pointer pressed outside every box while the boxes are up: put them back.
    pub fn above_click_outside(&mut self, p: Pt) -> bool {
        if self.is_above() && matches!(self.hit_test(p), crate::app::Hit::Empty) {
            self.set_above(false);
            return true;
        }
        false
    }
}
