//! Boxes above windows (Settings › Boxes above windows): a shortcut, or a screen corner, brings
//! the boxes above the open windows for a moment, so a file can be opened or dropped into a box
//! without minimizing anything. The windows behind are dimmed a little. Clicking anywhere
//! outside a box, Esc, the shortcut again, or switching to another app puts them back.
//!
//! How: a top-level window over the monitor (not topmost, so our menus still open above it)
//! takes over the surface's composition visual and input while it is up (`App::hwnd` points at
//! it); the desktop surface gets it back afterwards. Nothing is drawn twice.

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
        match &self.above {
            Some((_, _, t0)) if self.settings.above.dim => 0.24 * ease_out(t0.elapsed().as_secs_f32() / DIM_S),
            _ => 0.0,
        }
    }

    pub fn above_animating(&self) -> bool {
        self.above.as_ref().map(|(_, _, t0)| t0.elapsed().as_secs_f32() < DIM_S).unwrap_or(false)
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
            let Some(surf) = self.surf.as_mut() else { return };
            let (w, h) = (surf.w as i32, surf.h as i32);
            let hw = unsafe {
                CreateWindowExW(
                    WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW,
                    w!("DeskboxesAbove"),
                    w!("Deskboxes"),
                    WS_POPUP,
                    self.origin_px.x,
                    self.origin_px.y,
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
                return;
            };
            if let Err(e) = surf.retarget(&self.gpu, Some(hw)) {
                log::warn!("boxes above windows: {e}");
                unsafe {
                    let _ = DestroyWindow(hw);
                }
                return;
            }
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
            self.hwnd = hw;
            self.above_poll();
            log::info!("boxes above windows: on");
        } else {
            let Some((hw, desk, _)) = self.above.take() else { return };
            self.cancel_drag();
            self.hwnd = desk;
            if let Some(surf) = self.surf.as_mut() {
                if let Err(e) = surf.retarget(&self.gpu, None) {
                    log::warn!("boxes above windows: back: {e}");
                }
            }
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
        if !self.is_above() {
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
        let inside = match c.as_str() {
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
                self.set_above(true);
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
