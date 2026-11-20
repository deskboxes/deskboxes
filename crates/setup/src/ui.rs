//! The setup window, drawn to look like the Deskboxes settings window (and Windows Settings):
//! Mica backdrop, content in the title bar (48 DIPs, icon + caption, own minimize/close buttons),
//! a page title, a hero row (picture + what it does), cards with a glyph, title, description and
//! a control at the right edge (switches with "On"/"Off" on their left), and a footer band with
//! the buttons. Pages come in like Settings pages: elements rise 36 DIPs and fade in, one after
//! another. Keyboard: Tab / Shift+Tab move the focus, Space or Enter press, Esc cancels.

use crate::gfx::{Font, Gfx, Rect, TextOpts};
use crate::ops::{self, Failure, InstallOptions, Installed, Report, Step, UninstallOptions};
use crate::payload::Payload;
use crate::text::{t, tf, Lang, T};
use crate::theme::{Theme, C};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Instant;
use windows::core::{w, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const W: f32 = 680.0;
const H: f32 = 540.0;
const TITLE_H: f32 = 48.0;
const FOOTER_H: f32 = 64.0;
const MARGIN: f32 = 36.0;
const CAPTION_W: f32 = 46.0;
const WM_REPORT: u32 = WM_APP + 1;
const WM_MOUSELEAVE: u32 = 0x02A3;
const ICON_PNG: &[u8] = include_bytes!("../../../res/deskboxes-256.png");

// Segoe Fluent Icons.
const G_FOLDER: char = '\u{E8B7}';
const G_AUTOSTART: char = '\u{E823}';
const G_START: char = '\u{E71D}';
const G_PLAY: char = '\u{E768}';
const G_DELETE: char = '\u{E74D}';
const G_DOWNLOAD: char = '\u{E896}';
const G_CHECK: char = '\u{E73E}';
const G_WARNING: char = '\u{E7BA}';
const G_INFO: char = '\u{E946}';
const G_MINIMIZE: char = '\u{E921}';
const G_CLOSE: char = '\u{E8BB}';

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Id {
    Min,
    Close,
    Change,
    Autostart,
    StartMenu,
    LaunchNow,
    RemoveData,
    Primary,
    Secondary,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Welcome,
    Work,
    Done,
    Error,
    Confirm,
    Removed,
    NotInstalled,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Install,
    Uninstall,
}

pub struct Init {
    pub lang: Lang,
    pub mode: Mode,
    pub payload: Option<Payload>,
    pub dir: Option<PathBuf>,
    pub data_root: Option<PathBuf>,
}

enum Hero {
    App,
    Tile(char, C, C),
}

enum Control {
    Toggle(Id, bool),
    Button(Id, String),
    Progress(f32),
}

/// Something to do after the window's state is released (a modal dialog, closing).
enum After {
    Nothing,
    PickFolder,
    Close,
}

struct App {
    hwnd: HWND,
    gfx: Gfx,
    theme: Theme,
    lang: Lang,
    mode: Mode,
    page: Page,
    page_t0: Instant,
    payload: Option<Arc<Payload>>,
    installed: Option<Installed>,
    running_at_start: bool,
    dir: PathBuf,
    autostart: bool,
    start_menu: bool,
    launch_now: bool,
    remove_data: bool,
    data_root: Option<PathBuf>,
    // Work in progress.
    step: Option<Step>,
    copy: Option<(usize, usize)>,
    frac: f32,
    frac_shown: f32,
    work_t0: Instant,
    pending: Option<Page>,
    failure: Option<Failure>,
    launched: bool,
    uninstalled: bool,
    rx: Option<Receiver<Report>>,
    // Input and animation.
    targets: Vec<(Id, Rect)>,
    order: Vec<Id>,
    disabled: Vec<Id>,
    hot: Option<Id>,
    pressed: Option<Id>,
    focus: Option<Id>,
    focus_visible: bool,
    hover: HashMap<Id, f32>,
    knob: HashMap<Id, f32>,
    last_frame: Instant,
    active: bool,
    mica: bool,
    animations: bool,
    tracking: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(4)
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn approach(v: &mut f32, target: f32, dt: f32, dur: f32) -> bool {
    if (*v - target).abs() < 1e-3 {
        *v = target;
        return false;
    }
    let step = dt / dur;
    if *v < target {
        *v = (*v + step).min(target);
    } else {
        *v = (*v - step).max(target);
    }
    true
}

fn client_animations_on() -> bool {
    let mut on = BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    on.as_bool()
}

impl App {
    fn s(&self, k: T) -> &'static str {
        t(self.lang, k)
    }

    fn set_page(&mut self, p: Page) {
        self.page = p;
        self.page_t0 = Instant::now();
        self.hot = None;
        self.pressed = None;
        self.focus = None;
        self.invalidate();
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    // ------------------------------------------------------------------------------------------
    // DWM: Mica, dark mode.

    fn apply_dwm(&mut self) {
        unsafe {
            let dark = BOOL(self.theme.dark as i32);
            let _ = DwmSetWindowAttribute(self.hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
            let m = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
            let _ = DwmExtendFrameIntoClientArea(self.hwnd, &m);
            let backdrop = DWMSBT_MAINWINDOW;
            self.mica = DwmSetWindowAttribute(self.hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const _ as _, 4).is_ok();
        }
    }

    // ------------------------------------------------------------------------------------------
    // Painting.

    fn paint(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        let mut busy = false;

        // Hover and switch animations.
        let ids = [Id::Min, Id::Close, Id::Change, Id::Autostart, Id::StartMenu, Id::LaunchNow, Id::RemoveData, Id::Primary, Id::Secondary];
        for id in ids {
            let target = if self.hot == Some(id) { 1.0 } else { 0.0 };
            let v = self.hover.entry(id).or_insert(0.0);
            if self.animations {
                busy |= approach(v, target, dt, 0.083);
            } else {
                *v = target;
            }
            let on = match id {
                Id::Autostart => self.autostart,
                Id::StartMenu => self.start_menu,
                Id::LaunchNow => self.launch_now,
                Id::RemoveData => self.remove_data,
                _ => continue,
            };
            let k = self.knob.entry(id).or_insert(if on { 1.0 } else { 0.0 });
            if self.animations {
                busy |= approach(k, if on { 1.0 } else { 0.0 }, dt, 0.167);
            } else {
                *k = if on { 1.0 } else { 0.0 };
            }
        }
        // Progress: the bar glides to the reported value.
        if self.page == Page::Work {
            let gap = self.frac - self.frac_shown;
            if gap > 0.0005 {
                self.frac_shown += (gap * (dt * 6.0).min(1.0)).max(dt * 0.25).min(gap);
                busy = true;
            } else {
                self.frac_shown = self.frac;
            }
            if let Some(p) = self.pending {
                if self.frac_shown >= self.frac - 0.001 && self.work_t0.elapsed().as_secs_f32() > 0.7 {
                    self.pending = None;
                    self.set_page(p);
                } else {
                    busy = true;
                }
            }
        }
        if self.animations && self.page_t0.elapsed().as_secs_f32() < 1.0 {
            busy = true;
        }

        let bg = if self.mica { C::black(0.0) } else { self.theme.base };
        if let Err(e) = self.gfx.begin(bg) {
            ops::log(&format!("render target: {e}"));
            return;
        }
        self.targets.clear();
        self.order.clear();
        self.disabled.clear();
        self.draw_page();
        self.gfx.alpha = 1.0;
        self.gfx.dy = 0.0;
        self.draw_footer();
        self.draw_title_bar();
        self.gfx.end();
        if busy {
            // Next frame after the next composition (DirectComposition does not wait for vsync).
            unsafe {
                let _ = DwmFlush();
            }
            self.invalidate();
        }
    }

    /// Entrance of the page's `i`-th element: rise and fade, staggered.
    fn enter(&mut self, i: usize) {
        if !self.animations {
            self.gfx.alpha = 1.0;
            self.gfx.dy = 0.0;
            return;
        }
        let t = (self.page_t0.elapsed().as_secs_f32() - i as f32 * 0.045) / 0.5;
        let e = ease_out(t);
        self.gfx.dy = 36.0 * (1.0 - e);
        self.gfx.alpha = (t * 1.6).clamp(0.0, 1.0);
    }

    fn target(&mut self, id: Id, r: Rect, focusable: bool) {
        let r = r.offset(0.0, self.gfx.dy);
        self.targets.push((id, r));
        if focusable {
            self.order.push(id);
        }
    }

    fn draw_title_bar(&mut self) {
        let th = self.theme.clone();
        let fg = if self.active { th.text1 } else { th.text3 };
        self.gfx.app_icon(Rect::new(16.0, 16.0, 16.0, 16.0));
        let title = self.s(if self.mode == Mode::Install { T::WindowInstall } else { T::WindowUninstall });
        self.gfx.text(title, Font::Caption, Rect::new(44.0, 0.0, W - 44.0 - 2.0 * CAPTION_W, TITLE_H), fg, TextOpts { vcenter: true, one_line: true, ..Default::default() });
        for (i, id) in [Id::Min, Id::Close].into_iter().enumerate() {
            let r = Rect::new(W - CAPTION_W * (2 - i) as f32, 0.0, CAPTION_W, TITLE_H);
            let h = self.hover[&id];
            let down = self.pressed == Some(id) && self.hot == Some(id);
            let mut glyph_c = fg;
            if id == Id::Close {
                let red = C::rgba(0xC4, 0x2B, 0x1C, if down { 0.9 } else { 1.0 });
                if h > 0.0 {
                    self.gfx.fill(r, 0.0, red.mul_a(h));
                    glyph_c = fg.lerp(C::white(if down { 0.7 } else { 1.0 }), h);
                }
            } else if h > 0.0 || down {
                self.gfx.fill(r, 0.0, if down { th.subtle_pressed } else { th.subtle_hover.mul_a(h) });
            }
            let disabled = id == Id::Close && self.page == Page::Work;
            if disabled {
                glyph_c = th.text_disabled;
            }
            self.gfx.glyph(if id == Id::Min { G_MINIMIZE } else { G_CLOSE }, 10, r.x + r.w / 2.0, TITLE_H / 2.0, glyph_c);
            self.targets.push((id, r));
            if disabled {
                self.disabled.push(id);
            }
        }
    }

    fn page_title(&mut self, i: usize, y: f32, s: &str) -> f32 {
        self.enter(i);
        let c = self.theme.text1;
        let h = self.gfx.text(s, Font::Title, Rect::new(MARGIN, y, W - 2.0 * MARGIN, 60.0), c, TextOpts { one_line: true, ..Default::default() });
        y + h.max(36.0) + 12.0
    }

    fn hero(&mut self, i: usize, y: f32, pic: Hero, text: &str, caption: Option<&str>) -> f32 {
        self.enter(i);
        let tile = Rect::new(MARGIN, y, 72.0, 72.0);
        match pic {
            Hero::App => self.gfx.app_icon(tile),
            Hero::Tile(g, c0, c1) => {
                self.gfx.fill_gradient(tile, 16.0, c0, c1);
                self.gfx.glyph(g, 32, tile.x + 36.0, tile.y + 36.0, C::white(1.0));
            }
        }
        let x = MARGIN + 72.0 + 20.0;
        let w = W - MARGIN - x;
        let (_, th) = self.gfx.measure(text, Font::Body, w, TextOpts::default());
        let ch = caption.map(|c| self.gfx.measure(c, Font::Caption, w, TextOpts::default()).1 + 6.0).unwrap_or(0.0);
        let total = th + ch;
        let mut ty = if total < 72.0 { y + (72.0 - total) / 2.0 } else { y };
        let (t2, t3) = (self.theme.text2, self.theme.text3);
        ty += self.gfx.text(text, Font::Body, Rect::new(x, ty, w, 400.0), t2, TextOpts::default());
        if let Some(c) = caption {
            self.gfx.text(c, Font::Caption, Rect::new(x, ty + 6.0, w, 200.0), t3, TextOpts::default());
        }
        y + total.max(72.0) + 20.0
    }

    /// A settings card. Returns the y below it.
    fn card(&mut self, i: usize, y: f32, glyph: char, title: &str, desc: Option<&str>, ctl: Control) -> f32 {
        self.enter(i);
        let th = self.theme.clone();
        let x = MARGIN;
        let w = W - 2.0 * MARGIN;
        // Right part.
        let (on_w, off_w) = (
            self.gfx.measure(self.s(T::On), Font::Body, 400.0, TextOpts { one_line: true, ..Default::default() }).0,
            self.gfx.measure(self.s(T::Off), Font::Body, 400.0, TextOpts { one_line: true, ..Default::default() }).0,
        );
        let ctl_w = match &ctl {
            Control::Toggle(..) => 40.0 + 12.0 + on_w.max(off_w),
            Control::Button(_, label) => (self.gfx.measure(label, Font::Body, 400.0, TextOpts { one_line: true, ..Default::default() }).0 + 22.0).max(96.0),
            Control::Progress(_) => 0.0,
        };
        let tx = x + 16.0 + 20.0 + 16.0;
        let tw = (x + w - 16.0 - if ctl_w > 0.0 { ctl_w + 16.0 } else { 0.0 }) - tx;
        let one = TextOpts { one_line: true, ..Default::default() };
        let (_, title_h) = self.gfx.measure(title, Font::Body, tw, TextOpts::default());
        let desc_opts = TextOpts { one_line: matches!(ctl, Control::Button(Id::Change, _)), path: matches!(ctl, Control::Button(Id::Change, _)), ..Default::default() };
        let desc_h = desc.map(|d| self.gfx.measure(d, Font::Caption, tw, desc_opts).1).unwrap_or(0.0);
        let extra = if matches!(ctl, Control::Progress(_)) { 18.0 } else { 0.0 };
        let content_h = title_h + desc_h + extra;
        let h = (content_h + 24.0).max(if extra > 0.0 { 84.0 } else { 64.0 });
        let r = Rect::new(x, y, w, h);
        self.gfx.fill(r, 4.0, th.card);
        self.gfx.stroke(r, 4.0, th.card_stroke, 1.0);
        let cy = y + h / 2.0;
        self.gfx.glyph(glyph, 20, x + 16.0 + 10.0, cy, th.text1);
        let mut ty = cy - content_h / 2.0;
        ty += self.gfx.text(title, Font::Body, Rect::new(tx, ty, tw, 200.0), th.text1, TextOpts::default());
        if let Some(d) = desc {
            ty += self.gfx.text(d, Font::Caption, Rect::new(tx, ty, tw, 200.0), th.text2, desc_opts);
        }
        let right = x + w - 16.0;
        match ctl {
            Control::Toggle(id, on) => {
                // The whole card is the target (bigger is easier); the switch shows hover/press.
                self.target(id, r, true);
                let sw = Rect::new(right - 40.0, cy - 10.0, 40.0, 20.0);
                let label = self.s(if on { T::On } else { T::Off });
                self.gfx.text(label, Font::Body, Rect::new(sw.x - 12.0 - 200.0, cy - 12.0, 200.0, 24.0), th.text1, TextOpts { right: true, vcenter: true, one_line: true, ..Default::default() });
                self.toggle(id, sw);
                self.focus_ring(id, sw, 10.0);
            }
            Control::Button(id, label) => {
                let b = Rect::new(right - ctl_w, cy - 16.0, ctl_w, 32.0);
                self.button(id, b, &label, false, true);
            }
            Control::Progress(f) => {
                let bar = Rect::new(tx, ty + 12.0, tw - 64.0, 3.0);
                // Track: a 1-DIP line; indicator: 3 DIPs, rounded, accent.
                self.gfx.fill(Rect::new(bar.x, bar.y + 1.0, bar.w, 1.0), 0.0, th.strong);
                if f > 0.0 {
                    self.gfx.fill(Rect::new(bar.x, bar.y, (bar.w * f).max(3.0), 3.0), 1.5, th.accent);
                }
                let pct = tf(self.lang, T::Percent, &[&format!("{}", (f * 100.0).round() as i32)]);
                self.gfx.text(&pct, Font::Body, Rect::new(right - 56.0, bar.y - 10.0, 56.0, 22.0), th.text2, TextOpts { right: true, vcenter: true, ..one });
            }
        }
        y + h + 4.0
    }

    fn toggle(&mut self, id: Id, r: Rect) {
        let th = self.theme.clone();
        let p = smooth(self.knob[&id]);
        let h = self.hover[&id];
        let down = self.pressed == Some(id) && self.hot == Some(id);
        // Off look.
        if p < 1.0 {
            let fill = if down { th.alt4 } else { th.alt2.lerp(th.alt3, h) };
            self.gfx.fill(r, 10.0, fill.mul_a(1.0 - p));
            self.gfx.stroke(r, 10.0, th.strong.mul_a(1.0 - p), 1.0);
        }
        // On look.
        if p > 0.0 {
            let fill = if down { th.accent_pressed } else { th.accent.lerp(th.accent_hover, h) };
            self.gfx.fill(r, 10.0, fill.mul_a(p));
        }
        let (kw, kh) = if down { (17.0, 14.0) } else { (12.0 + 2.0 * h, 12.0 + 2.0 * h) };
        let left = r.x + 10.0;
        let rightc = r.r() - 10.0;
        let mut cx = left + (rightc - left) * p;
        if down {
            cx += if p > 0.5 { -1.5 } else { 1.5 };
        }
        let kc = th.strong.lerp(th.on_accent, p);
        self.gfx.fill(Rect::new(cx - kw / 2.0, r.y + 10.0 - kh / 2.0, kw, kh), kh / 2.0, kc);
    }

    fn button(&mut self, id: Id, r: Rect, label: &str, accent: bool, enabled: bool) {
        let th = self.theme.clone();
        self.target(id, r, enabled);
        if !enabled {
            self.disabled.push(id);
        }
        let h = self.hover[&id];
        let down = enabled && self.pressed == Some(id) && self.hot == Some(id);
        let text_c;
        if accent {
            if !enabled {
                self.gfx.fill(r, 4.0, th.accent_disabled);
                text_c = th.on_accent_disabled;
            } else {
                let fill = if down { th.accent_pressed } else { th.accent.lerp(th.accent_hover, h) };
                self.gfx.fill(r, 4.0, fill);
                self.gfx.stroke_elevation(r, 4.0, th.on_accent_stroke_top, th.on_accent_stroke_bottom, true);
                text_c = if down { th.on_accent_pressed } else { th.on_accent };
            }
        } else if !enabled {
            self.gfx.fill(r, 4.0, th.control_disabled);
            self.gfx.stroke(r, 4.0, th.control_stroke, 1.0);
            text_c = th.text_disabled;
        } else {
            let fill = if down { th.control_pressed } else { th.control.lerp(th.control_hover, h) };
            self.gfx.fill(r, 4.0, fill);
            if down {
                self.gfx.stroke(r, 4.0, th.control_stroke, 1.0);
            } else {
                self.gfx.stroke_elevation(r, 4.0, th.control_stroke, th.control_stroke_edge, !th.dark);
            }
            text_c = if down { th.text2 } else { th.text1 };
        }
        self.gfx.text(label, Font::Body, r, text_c, TextOpts { center: true, vcenter: true, one_line: true, ..Default::default() });
        self.focus_ring(id, r, 4.0);
    }

    /// WinUI's focus visual: 2 DIPs outer, 1 DIP inner, 3 DIPs outside the control.
    fn focus_ring(&mut self, id: Id, r: Rect, radius: f32) {
        if !(self.focus_visible && self.focus == Some(id)) {
            return;
        }
        let th = &self.theme;
        let (outer, inner) = (th.focus_outer, th.focus_inner);
        self.gfx.stroke(r.inset(-3.0), radius + 3.0, outer, 2.0);
        self.gfx.stroke(r.inset(-1.0), radius + 1.0, inner, 1.0);
    }

    fn note(&mut self, i: usize, y: f32, s: &str) -> f32 {
        self.enter(i);
        let c = self.theme.text2;
        self.gfx.glyph(G_INFO, 16, MARGIN + 10.0, y + 10.0, c);
        let h = self.gfx.text(s, Font::Caption, Rect::new(MARGIN + 28.0, y + 2.0, W - 2.0 * MARGIN - 28.0, 100.0), c, TextOpts::default());
        y + h + 8.0
    }

    fn draw_page(&mut self) {
        let lang = self.lang;
        let th = self.theme.clone();
        let version = self.payload.as_ref().map(|p| p.header.app_version.clone()).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
        let mut y = TITLE_H + 8.0;
        match self.page {
            Page::Welcome => {
                let (title, cap) = match &self.installed {
                    None => (T::InstallTitle, tf(lang, T::Version, &[&version])),
                    Some(i) if i.version == version => (T::ReinstallTitle, tf(lang, T::VersionSame, &[&version])),
                    Some(i) if i.version.is_empty() => (T::UpdateTitle, tf(lang, T::Version, &[&version])),
                    Some(i) => (T::UpdateTitle, tf(lang, T::VersionUpdate, &[&version, &i.version])),
                };
                y = self.page_title(0, y, self.s(title));
                y = self.hero(1, y, Hero::App, self.s(T::Tagline), Some(&cap));
                let dir = self.dir.display().to_string();
                y = self.card(2, y, G_FOLDER, self.s(T::Location), Some(&dir), Control::Button(Id::Change, self.s(T::Change).into()));
                y = self.card(3, y, G_AUTOSTART, self.s(T::Autostart), Some(self.s(T::AutostartDesc)), Control::Toggle(Id::Autostart, self.autostart));
                y = self.card(4, y, G_START, self.s(T::StartMenu), Some(self.s(T::StartMenuDesc)), Control::Toggle(Id::StartMenu, self.start_menu));
                if self.running_at_start {
                    self.note(5, y + 8.0, self.s(T::RunningNote));
                }
            }
            Page::Work => {
                let title = match self.mode {
                    Mode::Uninstall => T::Uninstalling,
                    Mode::Install if self.installed.is_some() => T::Updating,
                    Mode::Install => T::Installing,
                };
                y = self.page_title(0, y, self.s(title));
                y = self.hero(1, y, Hero::App, self.s(T::WorkText), None);
                let step = match self.step {
                    Some(Step::Copying) => {
                        let (n, total) = self.copy.unwrap_or((0, 0));
                        tf(lang, T::StepCopying, &[&n.to_string(), &total.to_string()])
                    }
                    Some(s) => self
                        .s(match s {
                            Step::Closing => T::StepClosing,
                            Step::Registering => T::StepRegistering,
                            Step::Starting => T::StepStarting,
                            Step::Restoring => T::StepRestoring,
                            Step::Cleanup => T::StepCleanup,
                            Step::Removing => T::StepRemoving,
                            Step::Copying => T::StepCopying,
                        })
                        .to_string(),
                    None => String::new(),
                };
                let glyph = if self.mode == Mode::Install { G_DOWNLOAD } else { G_DELETE };
                let f = self.frac_shown;
                self.card(2, y, glyph, &step, None, Control::Progress(f));
            }
            Page::Done => {
                y = self.page_title(0, y, self.s(if self.installed.is_some() { T::DoneUpdated } else { T::DoneTitle }));
                y = self.hero(1, y, Hero::Tile(G_CHECK, th.accent_light1, th.accent_dark1), self.s(T::DoneText), None);
                if !self.launched {
                    self.card(2, y, G_PLAY, self.s(T::LaunchNow), Some(self.s(T::LaunchNowDesc)), Control::Toggle(Id::LaunchNow, self.launch_now));
                }
            }
            Page::Error => {
                y = self.page_title(0, y, self.s(if self.mode == Mode::Install { T::ErrorTitle } else { T::ErrorUninstallTitle }));
                let (msg, detail) = match &self.failure {
                    Some(Failure::NoPayload) => (self.s(T::ErrNoPayload).to_string(), None),
                    Some(Failure::StillRunning) => (self.s(T::ErrRunning).to_string(), None),
                    Some(Failure::Location(e)) => (self.s(T::ErrLocation).to_string(), Some(tf(lang, T::Details, &[e]))),
                    Some(Failure::Other(e)) => (tf(lang, T::Details, &[e]), None),
                    None => (String::new(), None),
                };
                self.hero(1, y, Hero::Tile(G_WARNING, th.critical, th.critical_dark), &msg, detail.as_deref());
            }
            Page::Confirm => {
                y = self.page_title(0, y, self.s(T::UninstallTitle));
                let cap = self.installed.as_ref().map(|i| tf(lang, T::Version, &[&i.version]));
                y = self.hero(1, y, Hero::App, self.s(T::UninstallText), cap.as_deref());
                self.card(2, y, G_DELETE, self.s(T::RemoveData), Some(self.s(T::RemoveDataDesc)), Control::Toggle(Id::RemoveData, self.remove_data));
            }
            Page::Removed => {
                y = self.page_title(0, y, self.s(T::UninstalledTitle));
                let cap = self.s(if self.remove_data { T::DataRemoved } else { T::DataKept });
                self.hero(1, y, Hero::Tile(G_CHECK, th.accent_light1, th.accent_dark1), self.s(T::UninstalledText), Some(cap));
            }
            Page::NotInstalled => {
                y = self.page_title(0, y, self.s(T::UninstallTitle));
                self.hero(1, y, Hero::App, self.s(T::NotInstalled), None);
            }
        }
    }

    /// (secondary, primary) buttons: label and enabled.
    fn footer_buttons(&self) -> (Option<(T, bool)>, Option<(T, bool)>) {
        match self.page {
            Page::Welcome => {
                let primary = match &self.installed {
                    None => T::Install,
                    Some(i) if self.payload.as_ref().map(|p| p.header.app_version == i.version).unwrap_or(false) => T::Reinstall,
                    Some(_) => T::Update,
                };
                (Some((T::Cancel, true)), Some((primary, true)))
            }
            Page::Work => (Some((T::Cancel, false)), None),
            Page::Done => (None, Some((T::Finish, true))),
            Page::Error => match self.failure {
                Some(Failure::NoPayload) => (None, Some((T::Close, true))),
                _ => (Some((T::Close, true)), Some((T::TryAgain, true))),
            },
            Page::Confirm => (Some((T::Cancel, true)), Some((T::UninstallButton, true))),
            Page::Removed | Page::NotInstalled => (None, Some((T::Close, true))),
        }
    }

    fn draw_footer(&mut self) {
        let th = self.theme.clone();
        let r = Rect::new(0.0, H - FOOTER_H, W, FOOTER_H);
        self.gfx.fill(r, 0.0, th.layer_alt);
        self.gfx.fill(Rect::new(0.0, r.y, W, 1.0), 0.0, th.card_stroke);
        let (sec, pri) = self.footer_buttons();
        let mut x = W - 24.0;
        let by = r.y + (FOOTER_H - 32.0) / 2.0 + 0.5;
        let one = TextOpts { one_line: true, ..Default::default() };
        if let Some((k, en)) = pri {
            let label = self.s(k);
            let bw = (self.gfx.measure(label, Font::Body, 400.0, one).0 + 22.0).max(120.0);
            x -= bw;
            self.button(Id::Primary, Rect::new(x, by, bw, 32.0), label, true, en);
            x -= 8.0;
        }
        if let Some((k, en)) = sec {
            let label = self.s(k);
            let bw = (self.gfx.measure(label, Font::Body, 400.0, one).0 + 22.0).max(120.0);
            x -= bw;
            self.button(Id::Secondary, Rect::new(x, by, bw, 32.0), label, false, en);
        }
        if self.page == Page::Welcome {
            let mb = self.payload.as_ref().map(|p| (p.header.total() as f64 / 1_048_576.0).ceil() as u64).unwrap_or(0);
            let s = tf(self.lang, T::Footnote, &[&mb.to_string()]);
            self.gfx.text(&s, Font::Caption, Rect::new(24.0, r.y, x - 24.0 - 16.0, FOOTER_H), th.text3, TextOpts { vcenter: true, ..Default::default() });
        }
    }

    // ------------------------------------------------------------------------------------------
    // Input.

    fn hit(&self, x: f32, y: f32) -> Option<Id> {
        self.targets.iter().rev().find(|(_, r)| r.contains(x, y)).map(|(id, _)| *id).filter(|id| !self.disabled.contains(id))
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        let hot = self.hit(x, y);
        if hot != self.hot {
            self.hot = hot;
            self.invalidate();
        }
        if !self.tracking {
            let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            self.tracking = true;
        }
    }

    fn activate(&mut self, id: Id) -> After {
        match id {
            Id::Min => unsafe {
                let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
            },
            Id::Close => return After::Close,
            Id::Change => return After::PickFolder,
            Id::Autostart => self.autostart = !self.autostart,
            Id::StartMenu => self.start_menu = !self.start_menu,
            Id::LaunchNow => self.launch_now = !self.launch_now,
            Id::RemoveData => self.remove_data = !self.remove_data,
            Id::Secondary => match self.page {
                Page::Error if !matches!(self.failure, Some(Failure::NoPayload)) => return After::Close,
                Page::Welcome | Page::Confirm => return After::Close,
                _ => {}
            },
            Id::Primary => match self.page {
                Page::Welcome => self.start_work(),
                Page::Confirm => self.start_work(),
                Page::Done => {
                    if !self.launched && self.launch_now {
                        let wd = self.dir.join(ops::WATCHDOG);
                        if let Err(e) = ops::launch(&wd, "--launch --enable", &self.dir) {
                            ops::log(&format!("launch failed: {e}"));
                            self.failure = Some(Failure::Other(e));
                            self.set_page(Page::Error);
                            return After::Nothing;
                        }
                    }
                    return After::Close;
                }
                Page::Error => {
                    if matches!(self.failure, Some(Failure::NoPayload)) {
                        return After::Close;
                    }
                    self.installed = ops::installed();
                    self.running_at_start = ops::app_running();
                    self.failure = None;
                    self.set_page(if self.mode == Mode::Install { Page::Welcome } else { Page::Confirm });
                }
                Page::Removed | Page::NotInstalled => return After::Close,
                Page::Work => {}
            },
        }
        self.invalidate();
        After::Nothing
    }

    fn key(&mut self, vk: u16) -> After {
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        match VIRTUAL_KEY(vk) {
            VK_TAB => {
                let order: Vec<Id> = self.order.iter().copied().filter(|id| !self.disabled.contains(id)).collect();
                if order.is_empty() {
                    return After::Nothing;
                }
                let ix = self.focus.and_then(|f| order.iter().position(|&o| o == f));
                let n = order.len();
                let next = match (ix, shift) {
                    (None, false) => 0,
                    (None, true) => n - 1,
                    (Some(i), false) => (i + 1) % n,
                    (Some(i), true) => (i + n - 1) % n,
                };
                self.focus = Some(order[next]);
                self.focus_visible = true;
                self.invalidate();
            }
            VK_SPACE => {
                if let Some(f) = self.focus.filter(|_| self.focus_visible) {
                    return self.activate(f);
                }
            }
            VK_RETURN => {
                let f = self.focus.filter(|_| self.focus_visible).unwrap_or(Id::Primary);
                if !self.disabled.contains(&f) && self.targets.iter().any(|(id, _)| *id == f) {
                    return self.activate(f);
                }
            }
            VK_ESCAPE => {
                if self.page != Page::Work {
                    return After::Close;
                }
            }
            _ => {}
        }
        After::Nothing
    }

    // ------------------------------------------------------------------------------------------
    // Work.

    fn start_work(&mut self) {
        let (tx, rx) = channel::<Report>();
        self.rx = Some(rx);
        self.step = None;
        self.copy = None;
        self.frac = 0.0;
        self.frac_shown = 0.0;
        self.work_t0 = Instant::now();
        self.pending = None;
        self.focus_visible = false;
        let hwnd = self.hwnd.0 as isize;
        let send = move |tx: &std::sync::mpsc::Sender<Report>, r: Report| {
            let _ = tx.send(r);
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd as _)), WM_REPORT, WPARAM(0), LPARAM(0));
            }
        };
        match self.mode {
            Mode::Install => {
                let Some(p) = self.payload.clone() else {
                    self.failure = Some(Failure::NoPayload);
                    self.set_page(Page::Error);
                    return;
                };
                let o = InstallOptions { dir: self.dir.clone(), autostart: self.autostart, start_menu: self.start_menu, launch: false, update: false, shortcut_desc: self.s(T::ShortcutDesc).into() };
                std::thread::spawn(move || {
                    deskboxes_shell::com_sta();
                    let rep = |r: Report| send(&tx, r);
                    match ops::install(&p, &o, &rep) {
                        Ok(launched) => rep(Report::Done(launched)),
                        Err(f) => {
                            ops::log(&format!("install failed: {f:?}"));
                            rep(Report::Failed(f))
                        }
                    }
                });
            }
            Mode::Uninstall => {
                let o = UninstallOptions { dir: self.dir.clone(), remove_data: self.remove_data, data_root: self.data_root.clone() };
                std::thread::spawn(move || {
                    deskboxes_shell::com_sta();
                    let rep = |r: Report| send(&tx, r);
                    match ops::uninstall(&o, &rep) {
                        Ok(()) => rep(Report::Done(false)),
                        Err(f) => {
                            ops::log(&format!("uninstall failed: {f:?}"));
                            rep(Report::Failed(f))
                        }
                    }
                });
            }
        }
        self.set_page(Page::Work);
    }

    fn reports(&mut self) {
        let Some(rx) = &self.rx else { return };
        let msgs: Vec<Report> = rx.try_iter().collect();
        for r in msgs {
            match r {
                Report::Step(s) => self.step = Some(s),
                Report::Copy { files, files_total } => self.copy = Some((files, files_total)),
                Report::Fraction(f) => self.frac = f.max(self.frac),
                Report::Done(launched) => {
                    self.frac = 1.0;
                    self.launched = launched;
                    if self.mode == Mode::Uninstall {
                        self.uninstalled = true;
                    }
                    self.pending = Some(if self.mode == Mode::Install { Page::Done } else { Page::Removed });
                }
                Report::Failed(f) => {
                    if self.mode == Mode::Uninstall {
                        // Whatever could be removed is gone; the uninstaller still deletes itself.
                        self.uninstalled = true;
                    }
                    self.failure = Some(f);
                    self.pending = None;
                    self.set_page(Page::Error);
                }
            }
        }
        self.invalidate();
    }
}

// ----------------------------------------------------------------------------------------------
// Window.

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn dips(lp: LPARAM, dpi: f32) -> (f32, f32) {
    let x = (lp.0 & 0xFFFF) as i16 as f32;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f32;
    (x * 96.0 / dpi, y * 96.0 / dpi)
}

fn pick_folder(hwnd: HWND, lang: Lang, current: &std::path::Path) -> Option<PathBuf> {
    unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let _ = d.SetOptions(FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST);
        let _ = d.SetTitle(&HSTRING::from(t(lang, T::PickFolder)));
        let start = current.ancestors().find(|p| p.is_dir()).map(|p| p.to_path_buf());
        if let Some(s) = start {
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(s.as_os_str()), None) {
                let _ = d.SetFolder(&item);
            }
        }
        d.Show(Some(hwnd)).ok()?;
        let item = d.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s.map(PathBuf::from)
    }
}

fn do_after(hwnd: HWND, a: After) {
    match a {
        After::Nothing => {}
        After::Close => unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        After::PickFolder => {
            let Some((lang, cur)) = with_app(|a| (a.lang, a.dir.clone())) else { return };
            if let Some(p) = pick_folder(hwnd, lang, &cur) {
                with_app(|a| {
                    a.dir = ops::dir_for_picked(&p);
                    a.invalidate();
                });
            }
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_NCCALCSIZE if wp.0 != 0 => return LRESULT(0),
            WM_NCACTIVATE => {
                with_app(|a| {
                    a.active = wp.0 != 0;
                    a.invalidate();
                });
                return DefWindowProcW(hwnd, msg, wp, LPARAM(-1));
            }
            WM_NCHITTEST => {
                let mut pt = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
                let _ = ScreenToClient(hwnd, &mut pt);
                let dpi = with_app(|a| a.gfx.dpi).unwrap_or(96.0);
                let (x, y) = (pt.x as f32 * 96.0 / dpi, pt.y as f32 * 96.0 / dpi);
                if y >= 0.0 && y < TITLE_H && x < W - 2.0 * CAPTION_W {
                    return LRESULT(HTCAPTION as isize);
                }
                return LRESULT(HTCLIENT as isize);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut ps);
                with_app(|a| a.paint());
                let _ = EndPaint(hwnd, &ps);
                return LRESULT(0);
            }
            WM_SIZE => {
                let (w, h) = ((lp.0 & 0xFFFF) as u32, ((lp.0 >> 16) & 0xFFFF) as u32);
                with_app(|a| a.gfx.resize(w, h));
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                let dpi = (wp.0 & 0xFFFF) as f32;
                let r = &*(lp.0 as *const RECT);
                with_app(|a| a.gfx.set_dpi(dpi));
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                with_app(|a| {
                    a.gfx.resize(rc.right as u32, rc.bottom as u32);
                    a.invalidate();
                });
                return LRESULT(0);
            }
            WM_MOUSEMOVE => {
                with_app(|a| {
                    let (x, y) = dips(lp, a.gfx.dpi);
                    a.mouse_move(x, y);
                });
                return LRESULT(0);
            }
            WM_MOUSELEAVE => {
                with_app(|a| {
                    a.tracking = false;
                    if a.pressed.is_none() {
                        a.hot = None;
                    }
                    a.invalidate();
                });
                return LRESULT(0);
            }
            WM_LBUTTONDOWN => {
                with_app(|a| {
                    let (x, y) = dips(lp, a.gfx.dpi);
                    a.hot = a.hit(x, y);
                    a.pressed = a.hot;
                    a.focus_visible = false;
                    if let Some(p) = a.pressed {
                        if p != Id::Min && p != Id::Close {
                            a.focus = Some(p);
                        }
                    }
                    a.invalidate();
                });
                SetCapture(hwnd);
                return LRESULT(0);
            }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                let after = with_app(|a| {
                    let (x, y) = dips(lp, a.gfx.dpi);
                    let hot = a.hit(x, y);
                    let p = a.pressed.take();
                    a.hot = hot;
                    a.invalidate();
                    match p {
                        Some(id) if hot == Some(id) => a.activate(id),
                        _ => After::Nothing,
                    }
                });
                if let Some(a) = after {
                    do_after(hwnd, a);
                }
                return LRESULT(0);
            }
            WM_KEYDOWN => {
                let after = with_app(|a| a.key(wp.0 as u16));
                if let Some(a) = after {
                    do_after(hwnd, a);
                }
                return LRESULT(0);
            }
            WM_REPORT => {
                with_app(|a| a.reports());
                return LRESULT(0);
            }
            WM_SETTINGCHANGE | WM_DWMCOLORIZATIONCOLORCHANGED | WM_THEMECHANGED => {
                with_app(|a| {
                    a.theme = Theme::current();
                    a.animations = client_animations_on();
                    a.apply_dwm();
                    a.invalidate();
                });
            }
            WM_CLOSE => {
                let working = with_app(|a| a.page == Page::Work).unwrap_or(false);
                if working {
                    return LRESULT(0);
                }
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

/// Show the window and run until it closes. Returns the process exit code.
pub fn run(init: Init) -> i32 {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        // One setup window at a time.
        let _mutex = windows::Win32::System::Threading::CreateMutexW(None, true, w!(r"Local\DeskboxesSetup"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(h) = FindWindowW(w!("DeskboxesSetup"), None) {
                let _ = ShowWindow(h, SW_RESTORE);
                let _ = SetForegroundWindow(h);
            }
            return 0;
        }
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let icon = LoadImageW(Some(hinst.into()), PCWSTR(1 as _), IMAGE_ICON, 0, 0, LR_DEFAULTSIZE | LR_SHARED).map(|h| HICON(h.0)).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hIcon: icon,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("DeskboxesSetup"),
            ..Default::default()
        };
        RegisterClassExW(&wc);

        // Centered on the work area of the monitor with the pointer.
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let dpi = dx as f32;
        let (pw, ph) = ((W * dpi / 96.0).round() as i32, (H * dpi / 96.0).round() as i32);
        let wa = mi.rcWork;
        let (x, y) = (wa.left + (wa.right - wa.left - pw) / 2, wa.top + (wa.bottom - wa.top - ph) / 2);
        let title = t(init.lang, if init.mode == Mode::Install { T::WindowInstall } else { T::WindowUninstall });
        let Ok(hwnd) = CreateWindowExW(
            // Drawn by DirectComposition only (gfx.rs), so no redirection bitmap.
            WS_EX_NOREDIRECTIONBITMAP,
            w!("DeskboxesSetup"),
            &HSTRING::from(title),
            // No WS_SYSMENU: with the frame extended over the window, DWM would draw its own caption
            // buttons on top of ours.
            WS_OVERLAPPED | WS_CAPTION | WS_MINIMIZEBOX,
            x,
            y,
            pw,
            ph,
            None,
            None,
            Some(hinst.into()),
            None,
        ) else {
            return 1;
        };
        let dpi = GetDpiForWindow(hwnd) as f32;
        let Ok(gfx) = Gfx::new(hwnd, if dpi > 0.0 { dpi } else { 96.0 }, ICON_PNG) else { return 1 };

        let installed = ops::installed();
        let dir = init.dir.clone().or_else(|| installed.as_ref().map(|i| i.dir.clone())).unwrap_or_else(ops::default_dir);
        let page = match init.mode {
            Mode::Install if init.payload.is_none() => Page::Error,
            Mode::Install => Page::Welcome,
            Mode::Uninstall if installed.is_none() && init.dir.is_none() => Page::NotInstalled,
            Mode::Uninstall => Page::Confirm,
        };
        let mut app = App {
            hwnd,
            gfx,
            theme: Theme::current(),
            lang: init.lang,
            mode: init.mode,
            page,
            page_t0: Instant::now(),
            payload: init.payload.map(Arc::new),
            autostart: installed.is_none() || ops::autostart_on(),
            start_menu: installed.is_none() || ops::start_menu_link().map(|l| l.exists()).unwrap_or(true),
            installed,
            running_at_start: ops::app_running(),
            dir,
            launch_now: true,
            remove_data: false,
            data_root: init.data_root,
            step: None,
            copy: None,
            frac: 0.0,
            frac_shown: 0.0,
            work_t0: Instant::now(),
            pending: None,
            failure: if page == Page::Error { Some(Failure::NoPayload) } else { None },
            launched: false,
            uninstalled: false,
            rx: None,
            targets: vec![],
            order: vec![],
            disabled: vec![],
            hot: None,
            pressed: None,
            focus: None,
            focus_visible: false,
            hover: HashMap::new(),
            knob: HashMap::new(),
            last_frame: Instant::now(),
            active: true,
            mica: false,
            animations: client_animations_on(),
            tracking: false,
        };
        app.apply_dwm();
        APP.with(|a| *a.borrow_mut() = Some(app));
        // Drop the standard frame (WM_NCCALCSIZE above): the content fills the window.
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        with_app(|a| {
            a.gfx.resize(rc.right as u32, rc.bottom as u32);
            a.page_t0 = Instant::now();
        });
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        // Tests: keep the window above everything so screenshots see it.
        if std::env::var_os("DBX_SETUP_TOPMOST").is_some() {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }

        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        let (uninstalled, dir, failed) = APP.with(|a| a.borrow().as_ref().map(|a| (a.uninstalled, a.dir.clone(), a.failure.is_some())).unwrap_or((false, PathBuf::new(), true)));
        if uninstalled {
            ops::schedule_self_delete(&dir);
        }
        if failed {
            1
        } else {
            0
        }
    }
}
