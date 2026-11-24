//! Several screens (monitors): a surface window of its own on each, each with its own pages
//! (the model side is `deskboxes_core::screens`).
//!
//! The `App` keeps the screen in use in its own fields (`hwnd`, `surf`, `grid`, the wallpaper,
//! ...), as it did with one screen, and the others' in `screens`; using another screen swaps
//! them (`use_screen`) and switches the layout to the page that screen shows. So everything that
//! works on "the surface" works on whichever screen the pointer is on, unchanged. Each frame
//! draws every screen in turn.
//!
//! The screens are found at the start and again when Windows reports a display change (a
//! monitor plugged in or out, another resolution or scale): `rebuild_screens`.

use crate::app::{create_surface_window, App, Drag};
use deskboxes_core::screens as scr;
use deskboxes_core::*;
use deskboxes_i18n::{t, S};
use deskboxes_render::{Bitmap, Surface};
use std::collections::HashMap;
use std::time::Instant;
use windows::core::BOOL;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// A monitor as Windows has it now.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    /// Stable id: the monitor's device path (the same monitor on the same port).
    pub id: String,
    /// Its own name ("DELL U2720Q"), "" if none.
    pub name: String,
    /// Its number in Windows' display settings ("\\.\DISPLAY2" = 2).
    pub n: u32,
    pub rect: RECT,
    pub work: RECT,
    pub dpi: u32,
}

/// The monitors, the main one first, then in Windows' order.
pub fn monitors() -> Vec<Monitor> {
    let names = display_names();
    let mut hs: Vec<HMONITOR> = vec![];
    unsafe extern "system" fn each(h: HMONITOR, _: HDC, _: *mut RECT, l: LPARAM) -> BOOL {
        unsafe { (*(l.0 as *mut Vec<HMONITOR>)).push(h) };
        TRUE
    }
    let mut v: Vec<(bool, Monitor)> = vec![];
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut hs as *mut _ as isize));
        for h in hs {
            let mut mi = MONITORINFOEXW::default();
            mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if !GetMonitorInfoW(h, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
                continue;
            }
            let dev = wide(&mi.szDevice);
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            let n = dev.trim_start_matches(r"\\.\DISPLAY").parse().unwrap_or(0);
            let (id, name) = names.get(&dev.to_ascii_lowercase()).cloned().unwrap_or_else(|| (dev.to_ascii_lowercase(), String::new()));
            let primary = mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0;
            v.push((primary, Monitor { id, name, n, rect: mi.monitorInfo.rcMonitor, work: mi.monitorInfo.rcWork, dpi: dx.max(96) }));
        }
    }
    v.sort_by_key(|(p, m)| (!*p, m.n));
    // Two monitors showing the same thing (duplicated) are one screen.
    let mut out: Vec<Monitor> = vec![];
    for (_, m) in v {
        if !out.iter().any(|o| o.id == m.id || o.rect == m.rect) {
            out.push(m);
        }
    }
    out
}

/// Fixture screens (`--screens 1200x800,900x700`): test windows side by side, standing in for
/// monitors, at the main monitor's scale.
pub fn fixture_monitors(spec: &str, main: &Monitor) -> Vec<Monitor> {
    let mut x = main.rect.left + 40;
    let y = main.rect.top + 40;
    let mut v = vec![];
    for (i, s) in spec.split(',').map(str::trim).filter(|s| !s.is_empty()).enumerate() {
        let (w, h) = s.split_once('x').and_then(|(w, h)| Some((w.trim().parse::<i32>().ok()?, h.trim().parse::<i32>().ok()?))).unwrap_or((1200, 800));
        let rect = RECT { left: x, top: y, right: x + w, bottom: y + h };
        v.push(Monitor { id: format!("fixture-{}", i + 1), name: String::new(), n: i as u32 + 1, rect, work: rect, dpi: main.dpi });
        x += w + 16;
    }
    v
}

fn wide(b: &[u16]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..n])
}

/// Each active display's GDI name (lowercase "\\.\display2") -> (device path, own name), from
/// Windows' display configuration.
fn display_names() -> HashMap<String, (String, String)> {
    use windows::Win32::Devices::Display::*;
    let mut out = HashMap::new();
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
            return out;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None) != ERROR_SUCCESS {
            return out;
        }
        for p in &paths[..np as usize] {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            src.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            src.header.adapterId = p.sourceInfo.adapterId;
            src.header.id = p.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                continue;
            }
            let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            tgt.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            tgt.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            tgt.header.adapterId = p.targetInfo.adapterId;
            tgt.header.id = p.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut tgt.header) != 0 {
                continue;
            }
            let gdi = wide(&src.viewGdiDeviceName).to_ascii_lowercase();
            let path = wide(&tgt.monitorDevicePath).to_ascii_lowercase();
            let name = wide(&tgt.monitorFriendlyDeviceName);
            if !path.is_empty() {
                out.entry(gdi).or_insert((path, name));
            }
        }
    }
    out
}

/// What the screens are now, for noticing a change (ids, places, scales, work areas).
pub fn signature(ms: &[Monitor]) -> String {
    ms.iter().map(|m| format!("{}@{},{},{},{}/{},{},{},{}:{}", m.id, m.rect.left, m.rect.top, m.rect.right, m.rect.bottom, m.work.left, m.work.top, m.work.right, m.work.bottom, m.dpi)).collect::<Vec<_>>().join(";")
}

/// What one screen has of its own, while another screen is in use (see the module doc). The
/// screen in use has its values in the `App`'s own fields; its slot here holds leftovers.
pub struct ScreenRt {
    pub mon: Monitor,
    pub hwnd: HWND,
    pub surf: Option<Surface>,
    pub scale: f32,
    pub mon_rect: RECT,
    pub origin_px: POINT,
    pub size_dip: (f32, f32),
    pub work: Rect,
    pub grid: Grid,
    pub cell_px: (u32, u32),
    pub wall: Option<Bitmap>,
    pub backdrops: HashMap<i32, Bitmap>,
    pub backdrop_down: u32,
    pub wall_src: Option<(Option<(Bitmap, (u32, u32))>, deskboxes_render::Color, i32, u32, u32)>,
    pub wall_sig: String,
    pub wall_used: (String, i32, u32),
    pub wall_grid: Option<(u32, u32, Vec<u8>, f32)>,
    pub label_bmps: HashMap<(ItemKey, bool, i32), Bitmap>,
    pub frame_ok: bool,
    pub page_anim: Option<crate::pages::Slide>,
    pub page_other: Option<u32>,
    pub page_pill: Option<crate::pages::Pill>,
    pub page_shown: Option<u32>,
    pub page_gesture: Option<crate::pages::Gesture>,
    pub page_edge: Option<(f32, Instant)>,
}

impl ScreenRt {
    pub fn empty(mon: Monitor) -> Self {
        ScreenRt {
            mon,
            hwnd: HWND::default(),
            surf: None,
            scale: 1.0,
            mon_rect: RECT::default(),
            origin_px: POINT::default(),
            size_dip: (1.0, 1.0),
            work: Rect::default(),
            grid: Grid::new(Rect::new(0.0, 0.0, 1.0, 1.0), 1.0, 1.0),
            cell_px: (1, 1),
            wall: None,
            backdrops: HashMap::new(),
            backdrop_down: 1,
            wall_src: None,
            wall_sig: String::new(),
            wall_used: Default::default(),
            wall_grid: None,
            label_bmps: HashMap::new(),
            frame_ok: false,
            page_anim: None,
            page_other: None,
            page_pill: None,
            page_shown: None,
            page_gesture: None,
            page_edge: None,
        }
    }
}

impl App {
    /// More than one screen.
    pub fn multi_screen(&self) -> bool {
        self.screens.len() > 1
    }

    /// Swap the `App`'s screen fields with slot `i`.
    fn swap_rt(&mut self, i: usize) {
        let s = &mut self.screens[i];
        std::mem::swap(&mut self.hwnd, &mut s.hwnd);
        std::mem::swap(&mut self.surf, &mut s.surf);
        std::mem::swap(&mut self.scale, &mut s.scale);
        std::mem::swap(&mut self.mon, &mut s.mon_rect);
        std::mem::swap(&mut self.origin_px, &mut s.origin_px);
        std::mem::swap(&mut self.size_dip, &mut s.size_dip);
        std::mem::swap(&mut self.work, &mut s.work);
        std::mem::swap(&mut self.grid, &mut s.grid);
        std::mem::swap(&mut self.cell_px, &mut s.cell_px);
        std::mem::swap(&mut self.wall, &mut s.wall);
        std::mem::swap(&mut self.backdrops, &mut s.backdrops);
        std::mem::swap(&mut self.backdrop_down, &mut s.backdrop_down);
        std::mem::swap(&mut self.wall_src, &mut s.wall_src);
        std::mem::swap(&mut self.wall_sig, &mut s.wall_sig);
        std::mem::swap(&mut self.wall_used, &mut s.wall_used);
        std::mem::swap(&mut self.wall_grid, &mut s.wall_grid);
        std::mem::swap(&mut self.label_bmps, &mut s.label_bmps);
        std::mem::swap(&mut self.frame_ok, &mut s.frame_ok);
        std::mem::swap(&mut self.pages.anim, &mut s.page_anim);
        std::mem::swap(&mut self.pages.other, &mut s.page_other);
        std::mem::swap(&mut self.pages.pill, &mut s.page_pill);
        std::mem::swap(&mut self.pages.shown, &mut s.page_shown);
        std::mem::swap(&mut self.pages.gesture, &mut s.page_gesture);
        std::mem::swap(&mut self.pages.edge, &mut s.page_edge);
        *self.pages.title_layout.borrow_mut() = None;
    }

    /// Make screen `k` the one in use (its fields the `App`'s, its page the layout's own).
    /// Nothing else changes: see `enter_screen` for the pointer moving there.
    pub fn use_screen(&mut self, k: usize) {
        if k == self.cur || k >= self.screens.len() {
            return;
        }
        let cur = self.cur;
        self.swap_rt(cur);
        self.swap_rt(k);
        self.cur = k;
        if !scr::activate(&mut self.layout, &self.screens[k].mon.id) {
            log::warn!("screen {}: no page to show", self.screens[k].mon.id);
        }
    }

    /// The pointer (or a drag) moved onto screen `k`: use it. What was going on on the other one
    /// (hover, tooltips, a rename) ends; icons being dragged keep where they were (for the
    /// group's shape where they are dropped).
    pub fn enter_screen(&mut self, k: usize) {
        if k == self.cur || k >= self.screens.len() {
            return;
        }
        let keys = match &self.drag {
            Drag::Items { keys, .. } => Some(keys.clone()),
            _ => self.own_drag.as_ref().map(|(k, _)| k.clone()),
        };
        if let Some(keys) = keys {
            for (k, r, _) in self.visible_cells() {
                if keys.contains(&k) {
                    self.pages.carried.entry(k).or_insert(r);
                }
            }
        }
        self.end_rename(true);
        self.hide_tip();
        // (Boxes above windows are those of one screen.)
        if self.is_above() {
            self.set_above(false);
        }
        self.hover = None;
        self.hover_hdr = None;
        self.pending_tab = None;
        self.revealed.clear();
        self.room = None;
        self.box_want = None;
        self.box_room = None;
        self.preview_loose = None;
        // A held edge ("hold here for the next page") stays with the screen it is on, and only the
        // screen in use runs its timer: let go of it, or that screen animated for good (2026-11-24).
        self.pages.edge = None;
        self.use_screen(k);
        self.invalidate();
    }

    /// A box being moved went onto another screen: it goes there with the pointer, onto the page
    /// that screen shows (as when carried to another page: one undo puts it back, Esc too).
    pub fn carry_box_across(&mut self) {
        let Drag::MoveBox { id, .. } = self.drag else { return };
        let Some(k) = self.screen_under_cursor().filter(|k| *k != self.cur) else { return };
        let mut c = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut c);
        }
        let here = self.to_dip(c.x - self.origin_px.x, c.y - self.origin_px.y);
        let Some(r) = self.box_by_id(id).map(|b| b.rect) else { return };
        // Where the pointer holds it, as a share of its size (it may have to get smaller there).
        let (fx, fy) = (((here.x - r.x) / r.w.max(1.0)).clamp(0.0, 1.0), ((here.y - r.y) / r.h.max(1.0)).clamp(0.0, 1.0));
        self.page_keep_before();
        let Some(mut b) = pages::take_box(&mut self.layout, id) else { return };
        self.enter_screen(k);
        let there = self.to_dip(c.x - self.origin_px.x, c.y - self.origin_px.y);
        let (w, h) = (b.rect.w.min(self.work.w), b.rect.h.min(self.work.h));
        b.rect = Rect::new(there.x - fx * w, there.y - fy * h, w, h);
        let rect = b.rect;
        self.layout.boxes.push(b);
        self.drag = Drag::MoveBox { id, start: there, orig: rect, cur: there };
        // The pointer's messages come to this screen's window from now on.
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(self.hwnd);
        }
        log::info!("box {id} carried to screen {}", self.screens[k].mon.id);
        self.update_preview();
        self.layout_changed();
    }

    /// The screen whose surface window is `h`.
    pub fn screen_of_hwnd(&self, h: HWND) -> Option<usize> {
        if h == self.hwnd {
            return Some(self.cur);
        }
        self.screens.iter().position(|s| s.hwnd == h).filter(|i| *i != self.cur)
    }

    /// The screen a point (screen px) is on.
    pub fn screen_at(&self, x: i32, y: i32) -> Option<usize> {
        let inside = |r: &RECT| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        (0..self.screens.len()).find(|&i| {
            let (o, sz, sc) = if i == self.cur { (self.origin_px, self.size_dip, self.scale) } else { (self.screens[i].origin_px, self.screens[i].size_dip, self.screens[i].scale) };
            inside(&RECT { left: o.x, top: o.y, right: o.x + (sz.0 * sc).round() as i32, bottom: o.y + (sz.1 * sc).round() as i32 })
        })
    }

    /// The screen the pointer is on.
    pub fn screen_under_cursor(&self) -> Option<usize> {
        let mut c = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut c);
        }
        self.screen_at(c.x, c.y)
    }

    /// Run `f` on every screen in turn (each in use meanwhile); the screen in use stays.
    pub fn for_each_screen(&mut self, mut f: impl FnMut(&mut App)) {
        let cur = self.cur;
        for k in 0..self.screens.len() {
            self.use_screen(k);
            f(self);
        }
        self.use_screen(cur);
    }

    /// Every other screen's frame (each frame draws all of them; the one in use last, with what
    /// the pointer is doing). True while something on one of them is animating.
    pub fn draw_other_screens(&mut self) -> bool {
        if !self.multi_screen() {
            return false;
        }
        let mut anim = false;
        let cur = self.cur;
        // (Only the screen in use shows what is being moved or previewed there.)
        let preview = self.preview_loose.take();
        for k in (0..self.screens.len()).filter(|k| *k != cur) {
            self.use_screen(k);
            // (Only the screen in use holds an edge: its timer would never run here.)
            self.pages.edge = None;
            self.paint_inactive = true;
            anim |= self.draw_screen_only();
            self.paint_inactive = false;
        }
        self.use_screen(cur);
        self.preview_loose = preview;
        anim
    }

    /// The layout's page shown moved to another screen's page (a search result, an undo): use
    /// that screen, each screen keeping its page.
    pub fn screen_sync(&mut self) {
        if !self.multi_screen() {
            return;
        }
        let now = scr::current(&self.layout).to_string();
        let cur_id = self.screens[self.cur].mon.id.clone();
        if now == cur_id {
            return;
        }
        let Some(k) = self.screens.iter().position(|s| s.mon.id == now) else {
            // (A page of no screen here: back to this screen's own.)
            scr::activate(&mut self.layout, &cur_id);
            return;
        };
        // What the screen in use showed until now stays shown there.
        let was_page = self.pages.shown.unwrap_or(self.layout.page);
        scr::note_switch(&mut self.layout, &cur_id, was_page);
        // Swap the screens' own fields without switching pages again.
        let cur = self.cur;
        self.swap_rt(cur);
        self.swap_rt(k);
        self.cur = k;
    }

    /// The name a screen goes by: its own name, or "Screen N".
    pub fn screen_title(&self, id: &str) -> String {
        let info = self.screens.iter().map(|s| &s.mon).find(|m| m.id == id).map(|m| (m.name.clone(), m.n)).or_else(|| self.layout.screens.get(id).map(|s| (s.name.clone(), s.n)));
        match info {
            Some((name, _)) if !name.is_empty() => name,
            Some((_, n)) if n > 0 => t(self.lang, S::ScreenN).replace("{n}", &n.to_string()),
            _ => t(self.lang, S::ScreenN).replace("{n}", "2"),
        }
    }

    /// The model's view of the screens there now.
    fn present(&self) -> Vec<scr::Present> {
        (0..self.screens.len())
            .map(|i| {
                let s = &self.screens[i];
                let (size, work, grid, scale) = if i == self.cur { (self.size_dip, self.work, self.grid, self.scale) } else { (s.size_dip, s.work, s.grid, s.scale) };
                let _ = scale;
                scr::Present {
                    id: s.mon.id.clone(),
                    info: ScreenInfo { w: size.0, h: size.1, work, cols: grid.cols, rows: grid.rows, name: s.mon.name.clone(), n: s.mon.n },
                    grid,
                }
            })
            .collect()
    }

    /// Fit the layout to the screens there now; pages of an unplugged screen moved onto the main
    /// screen are explained in a dialog.
    pub fn arrange_screens(&mut self) {
        let present = self.present();
        let main = self.screens.first().map(|s| s.mon.id.clone()).unwrap_or_default();
        // The model works from the main screen's page.
        if self.multi_screen() || !scr::current(&self.layout).is_empty() {
            self.use_screen(0);
            scr::activate(&mut self.layout, &main);
        }
        let before = self.layout.clone();
        let out = scr::arrange(&mut self.layout, &present);
        if self.layout != before {
            log::info!("screens: {} there; pages gone to the main screen {:?}, back {:?}", present.len(), out.gone, out.back);
            self.pages.shown = Some(self.layout.page);
            self.save_soon();
        }
        for (screen, pages) in &out.gone {
            self.screen_gone_dialog(screen, pages);
        }
    }

    /// A screen was unplugged and its pages are on the main screen now: say so, once per time.
    fn screen_gone_dialog(&mut self, screen: &str, pages: &[u32]) {
        if !self.settings.pages.screen_notice {
            return;
        }
        let name = self.screen_title(screen);
        let first = pages.first().map(|p| self.page_title(*p)).unwrap_or_default();
        self.screen_notice(&name, &first, pages.len(), pages.first().copied());
    }

    /// Make the runtime part of a screen for monitor `m` (its window, surface, grid, wallpaper).
    pub fn make_screen(&mut self, m: Monitor) -> Option<ScreenRt> {
        let mut s = ScreenRt::empty(m.clone());
        let test_rect = (!self.desktop_mode).then_some(m.rect);
        let (hwnd, origin) = match unsafe { create_surface_window(self.desktop_mode, m.rect, test_rect) } {
            Ok(x) => x,
            Err(e) => {
                log::warn!("screen {}: no window: {e}", m.id);
                return None;
            }
        };
        let scale = m.dpi as f32 / 96.0;
        let mut cr = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut cr);
        }
        let (wpx, hpx) = ((cr.right - cr.left).max(1) as u32, (cr.bottom - cr.top).max(1) as u32);
        s.surf = Surface::new(&self.gpu, hwnd, wpx, hpx).map_err(|e| log::warn!("screen {}: surface: {e}", m.id)).ok();
        s.hwnd = hwnd;
        s.origin_px = origin;
        s.scale = scale;
        // A fixture window crops the wallpaper like the main test window: from the real primary monitor.
        s.mon_rect = if self.desktop_mode { m.rect } else { unsafe { crate::app::primary_monitor().0 } };
        s.size_dip = (wpx as f32 / scale, hpx as f32 / scale);
        s.work = if self.desktop_mode {
            Rect::new((m.work.left - m.rect.left) as f32 / scale, (m.work.top - m.rect.top) as f32 / scale, (m.work.right - m.work.left) as f32 / scale, (m.work.bottom - m.work.top) as f32 / scale)
        } else {
            Rect::new(0.0, 0.0, s.size_dip.0, s.size_dip.1)
        };
        // The same icon spacing (in DIPs) as on the main screen.
        let (cw, ch) = (self.grid.cell_w, self.grid.cell_h);
        s.grid = Grid::new(s.work, cw, ch);
        s.cell_px = ((cw * scale).round() as u32, (ch * scale).round() as u32);
        s.backdrop_down = self.backdrop_down;
        unsafe {
            let _ = windows::Win32::System::Ole::RegisterDragDrop(hwnd, &crate::droptarget::DropTarget::create(hwnd));
        }
        log::info!("screen {} '{}' #{}: {}x{} px at ({}, {}), scale {scale}", m.id, m.name, m.n, wpx, hpx, origin.x, origin.y);
        Some(s)
    }

    /// The other screens, made after the main one is on screen (start).
    pub fn add_other_screens(&mut self, ms: Vec<Monitor>) {
        for m in ms.into_iter().skip(1) {
            if let Some(s) = self.make_screen(m) {
                self.screens.push(s);
            }
        }
        if !self.multi_screen() && scr::current(&self.layout).is_empty() && self.layout.screens.len() <= 1 {
            // One screen, as before: only noted.
            self.arrange_screens();
            return;
        }
        self.arrange_screens();
        let n = self.screens.len();
        for k in 1..n {
            self.use_screen(k);
            self.load_wallpaper();
        }
        self.use_screen(0);
        self.draw();
        for s in &self.screens[1..] {
            unsafe {
                let _ = ShowWindow(s.hwnd, if self.desktop_mode { SW_SHOWNA } else { SW_SHOW });
            }
        }
    }

    /// The monitors as found now (fixture screens in window mode).
    pub fn find_monitors(&self) -> Vec<Monitor> {
        let real = monitors();
        if !self.desktop_mode {
            let main = real.first().cloned().unwrap_or(Monitor { id: "fixture".into(), name: String::new(), n: 1, rect: self.mon, work: self.mon, dpi: 96 });
            let spec = self.fixture_screens.clone();
            return fixture_monitors(&spec, &main);
        }
        if real.is_empty() {
            log::warn!("no monitors found; using the main one as before");
        }
        real
    }

    /// Windows reported a display change: once it has settled, the screens are found again and,
    /// if they are not the same (plugged in or out, another size or scale), made anew.
    pub fn on_display_change(&mut self) {
        unsafe {
            SetTimer(Some(self.msg_hwnd), T_SCREENS, 1200, None);
        }
    }

    pub fn screens_due(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.msg_hwnd), T_SCREENS);
        }
        let ms = self.find_monitors();
        if ms.is_empty() {
            return;
        }
        let sig = signature(&ms);
        if sig == self.screens_sig {
            return;
        }
        log::info!("display change: screens now {sig}");
        self.rebuild_screens(ms);
    }

    /// A fixture screen's window was closed: as if that monitor were unplugged.
    pub fn fixture_screen_closed(&mut self, h: HWND) {
        let Some(i) = self.screen_of_hwnd(h).filter(|i| *i > 0) else { return };
        let mut parts: Vec<&str> = self.fixture_screens.split(',').collect();
        if i < parts.len() {
            parts.remove(i);
        }
        self.fixture_screens = parts.join(",");
        let ms = self.find_monitors();
        self.rebuild_screens(ms);
    }

    /// Make every screen anew for monitors `ms` (the main one first): the windows and surfaces,
    /// their grids and wallpapers; the layout fitted to them.
    pub fn rebuild_screens(&mut self, ms: Vec<Monitor>) {
        if ms.is_empty() {
            return;
        }
        self.cancel_drag();
        self.end_rename(true);
        self.hide_tip();
        self.set_above(false);
        // The main screen's page in use while the screens change.
        self.use_screen(0);
        let old: Vec<ScreenRt> = std::mem::take(&mut self.screens);
        let mut fresh: Vec<ScreenRt> = vec![];
        for m in ms.iter() {
            if let Some(s) = self.make_screen(m.clone()) {
                fresh.push(s);
            }
        }
        if fresh.is_empty() {
            log::error!("display change: no screen could be made; keeping the old ones");
            self.screens = old;
            return;
        }
        // The old windows go (the one in use is in the App's fields, slot 0 holds leftovers).
        let old_main = std::mem::replace(&mut self.hwnd, HWND::default());
        self.surf = None;
        unsafe {
            for h in std::iter::once(old_main).chain(old.iter().skip(1).map(|s| s.hwnd)) {
                if IsWindow(Some(h)).as_bool() {
                    let _ = windows::Win32::System::Ole::RevokeDragDrop(h);
                    crate::uia::forget(h);
                    let _ = DestroyWindow(h);
                }
            }
        }
        drop(old);
        self.screens = fresh;
        self.cur = 0;
        // Slot 0's values into the App's own fields.
        self.swap_rt(0);
        self.screens_sig = signature(&ms);
        self.arrange_screens();
        let n = self.screens.len();
        for k in (0..n).rev() {
            self.use_screen(k);
            self.load_wallpaper();
            self.labels_rescaled();
        }
        self.layout_changed();
        self.draw();
        let ok = self.frame_ok;
        for k in 0..n {
            let h = if k == self.cur { self.hwnd } else { self.screens[k].hwnd };
            unsafe {
                let _ = ShowWindow(h, if self.desktop_mode { SW_SHOWNA } else { SW_SHOW });
            }
        }
        if self.desktop_mode && !self.args.no_hide && ok {
            deskboxes_shell::hide::set_hidden(true);
        }
        log::info!("screens made anew: {n}");
    }

    /// A screen's scale may differ from the last one's: labels drawn as pictures are made again.
    fn labels_rescaled(&mut self) {
        self.label_bmps.clear();
    }

    /// Every screen's label pictures made again (a new font, icon size ...).
    pub fn clear_label_bmps_all(&mut self) {
        self.label_bmps.clear();
        for s in &mut self.screens {
            s.label_bmps.clear();
        }
    }
}

/// The screens are looked at again (`screens_due`).
pub const T_SCREENS: usize = 61;
