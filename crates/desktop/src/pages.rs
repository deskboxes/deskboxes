//! Desktop pages in the app: going from page to page (Alt+Left/Right, Alt+wheel, a touchpad's
//! two-finger swipe, dragging the desktop with the middle button or both buttons, pulling it from
//! a screen edge, icons or a box held at an edge), the slide between pages, and the page's name
//! at the top of the screen. The pages themselves are the core's (`deskboxes_core::pages`); both
//! pages of a slide are drawn in `draw.rs` (`paint_frame`).

use crate::app::{ease_out, App, Drag, Hit};
use deskboxes_core::pages as pg;
use deskboxes_core::*;
use deskboxes_i18n::{t, S};
use deskboxes_render::{self as r, rgba, Color, Dc, RectF};
use std::collections::HashMap;
use std::time::Instant;
use windows::core::BOOL;
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// A drag resting at a screen edge (checked until it moves away or ends).
pub const T_PAGE_EDGE: usize = 21;
/// A touchpad swipe ended (no horizontal wheel for a moment).
pub const T_PAGE_SWIPE: usize = 22;
/// The page's name has been shown long enough.
pub const T_PAGE_PILL: usize = 23;

/// A whole slide from one page to the next.
const SLIDE_S: f32 = 0.36;
/// The bounce settles a little later.
const BOUNCE_EXTRA_S: f32 = 0.1;
/// Pages fade instead (animations off).
const FADE_S: f32 = 0.18;
const PILL_IN_S: f32 = 0.2;
const PILL_OUT_S: f32 = 0.32;
const PILL_HOLD_MS: u32 = 1500;
/// The names in the pill slide past each other.
const NAME_S: f32 = 0.3;
/// The active dot glides to its page.
const DOT_S: f32 = 0.32;
/// Pulling the desktop from its edge starts this close to it (DIPs).
const EDGE_PULL: f32 = 8.0;
/// Icons or a box held this close to an edge go to the next page ...
const EDGE_DRAG: f32 = 28.0;
/// ... after this long.
const EDGE_HOLD_S: f32 = 0.6;
/// DIPs a touchpad's horizontal wheel unit moves the page.
const SWIPE_K: f32 = 1.2;
/// A flick (DIPs/s) goes to the next page however short it was.
const FLING: f32 = 650.0;
/// The pill's height.
const PILL_H: f32 = 52.0;

/// What moves the pages under the pointer (or fingers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum By {
    Middle,
    Both,
    Edge,
    Swipe,
}

/// The pages following the pointer: `raw` how far it has gone (DIPs, + = right), `off` where the
/// page shown is drawn (with resistance past the first and the last page).
pub struct Gesture {
    pub by: By,
    pub x0: f32,
    pub raw: f32,
    pub last: (f32, Instant),
    pub vel: f32,
    pub off: f32,
}

/// The page shown sliding to rest from `from` (DIPs; the page beside on the other `side`).
pub struct Slide {
    pub from: f32,
    pub side: f32,
    pub t0: Instant,
    pub dur: f32,
    pub bounce: bool,
    pub fade: bool,
}

impl Slide {
    fn t(&self) -> f32 {
        (self.t0.elapsed().as_secs_f32() / self.dur).min(1.0)
    }
    pub fn done(&self) -> bool {
        self.t() >= 1.0
    }
    fn off(&self) -> f32 {
        let t = self.t();
        self.from * (1.0 - if self.bounce { ease_back(t) } else { ease_out(t) })
    }
}

/// The page's name at the top: since when (fading in from `a0`), fading out since, the name it
/// replaced sliding away (name, direction, the pill's width then, since), the active dot gliding
/// from (position, since).
pub struct Pill {
    pub t0: Instant,
    pub a0: f32,
    pub out: Option<Instant>,
    pub hover: bool,
    pub name_from: Option<(String, f32, f32, Instant)>,
    pub dot_from: Option<(f32, Instant)>,
}

impl Pill {
    fn alpha(&self) -> f32 {
        match self.out {
            Some(o) => 1.0 - ease_out(o.elapsed().as_secs_f32() / PILL_OUT_S),
            None => self.a0 + (1.0 - self.a0) * ease_out(self.t0.elapsed().as_secs_f32() / PILL_IN_S),
        }
    }
    fn animating(&self) -> bool {
        self.out.is_some()
            || self.t0.elapsed().as_secs_f32() < PILL_IN_S
            || self.name_from.as_ref().is_some_and(|n| n.3.elapsed().as_secs_f32() < NAME_S)
            || self.dot_from.is_some_and(|d| d.1.elapsed().as_secs_f32() < DOT_S)
    }
}

/// How the pages are drawn this frame: the page shown moved by `dx` at `alpha`, the page beside
/// (if any) by `other_dx` at `other_alpha`.
pub struct Frame {
    pub other: Option<u32>,
    pub other_dx: f32,
    pub dx: f32,
    pub alpha: f32,
    pub other_alpha: f32,
}

#[derive(Default)]
pub struct PageState {
    pub anim: Option<Slide>,
    /// The page sliding away (None: the neighbour the pages are moved toward).
    pub other: Option<u32>,
    pub gesture: Option<Gesture>,
    pub pill: Option<Pill>,
    /// A drag resting at the left (-1) or right (1) edge, since.
    pub edge: Option<(f32, Instant)>,
    /// A page made by holding a drag at the right edge past the last page: removed again if the
    /// drag leaves nothing there.
    pub made: Option<u32>,
    /// The layout before a drag took things to another page: what its change is recorded
    /// against (one undo puts them back on their page), or restored if it is cancelled.
    pub before: Option<MonitorLayout>,
    /// The page drawn last (another one now, after an undo say, slides to it).
    pub shown: Option<u32>,
    /// Icons dragged from another page: where they were (the group keeps its shape there).
    pub carried: HashMap<ItemKey, Rect>,
    /// The page being drawn is moved by this (the box glass stays on the wallpaper).
    pub paint_dx: f32,
    /// Both pages of a slide are being drawn.
    pub sliding: bool,
    /// The last page step by a key, a wheel notch or a tilt.
    pub step_at: Option<Instant>,
    /// Button ups that belong to a gesture just ended (1: left, 2: right).
    pub swallow_up: u8,
    pub tf: Option<IDWriteTextFormat>,
    /// The pill's title laid out (shaped once, not per frame and per pointer move): (title, its
    /// layout). The text format only changes with the DPI, which makes `tf` anew and clears it.
    pub title_layout: std::cell::RefCell<Option<(String, r::TextLayout)>>,
    pub dot_tf: Option<IDWriteTextFormat>,
    /// The next menu shown is this (our own entries only: the page name's menu).
    pub menu_own: Option<Vec<serde_json::Value>>,
    /// A dialog open: its id and what it asks.
    pub dialog: Option<(u32, Ask)>,
    pub dialog_seq: u32,
    /// Disconnected screens still to tell about (screen, its first page's name, how many pages,
    /// that page): one dialog at a time, once the shell host is there.
    pub screen_notices: Vec<(String, String, usize, Option<u32>)>,
}

/// Ease out with a light overshoot (the page settling with a small bounce).
fn ease_back(t: f32) -> f32 {
    let c1 = 0.7;
    let c3 = c1 + 1.0;
    let u = t.clamp(0.0, 1.0) - 1.0;
    1.0 + c3 * u * u * u + c1 * u * u
}

fn rf(x: Rect) -> RectF {
    RectF {
        left: x.x,
        top: x.y,
        right: x.right(),
        bottom: x.bottom(),
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Windows' "Animation effects" (off: pages fade instead of sliding).
pub(crate) fn windows_animates() -> bool {
    let mut on = BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut BOOL as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    on.as_bool()
}

fn button_held() -> bool {
    unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 || GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0 }
}

/// Dots shown for up to this many pages (more: "3 / 20").
const MAX_DOTS: usize = 12;
const DOT: f32 = 6.0;
const DOT_STEP: f32 = 14.0;
const DOT_ON_W: f32 = 18.0;

fn dots_width(n: usize) -> f32 {
    if n > MAX_DOTS {
        44.0
    } else {
        (n.max(1) - 1) as f32 * DOT_STEP + DOT_ON_W
    }
}

impl App {
    pub fn pages_on(&self) -> bool {
        self.settings.pages.enabled
    }

    fn page_w(&self) -> f32 {
        self.size_dip.0.max(1.0)
    }

    /// A page's name as shown: its own, or "Page N".
    pub fn page_title(&self, id: u32) -> String {
        let n = pg::name(&self.layout, id);
        if !n.is_empty() {
            return n.to_string();
        }
        let i = pg::index_of(&self.layout, id).unwrap_or(0) + 1;
        t(self.lang, S::PageN).replace("{n}", &i.to_string())
    }

    /// Pages slide (else they fade).
    fn page_slides(&self) -> bool {
        self.settings.pages.animate && windows_animates()
    }

    // ------------------------------------------------------------------ drawing

    /// How the pages are drawn this frame (None: the page shown alone, at rest).
    pub fn page_frame(&self) -> Option<Frame> {
        let w = self.page_w();
        let (off, side, other) = if let Some(g) = &self.pages.gesture {
            (g.off, if g.off >= 0.0 { 1.0 } else { -1.0 }, None)
        } else if let Some(a) = self.pages.anim.as_ref().filter(|a| !a.done()) {
            if a.fade {
                let e = ease_out(a.t());
                return Some(Frame { other: self.pages.other, other_dx: 0.0, dx: 0.0, alpha: e, other_alpha: 1.0 - e });
            }
            (a.off(), a.side, self.pages.other)
        } else {
            return None;
        };
        // Beside it: the page it came from, or the neighbour on the side it is moved away from.
        let other = other.or_else(|| {
            let i = pg::current_index(&self.layout) as i64 + if side > 0.0 { -1 } else { 1 };
            if i < 0 {
                None
            } else {
                pg::id_at(&self.layout, i as usize)
            }
        });
        Some(Frame { other, other_dx: off - side * w, dx: off, alpha: 1.0, other_alpha: 1.0 })
    }

    pub fn pages_animating(&self) -> bool {
        self.pages.anim.as_ref().is_some_and(|a| !a.done()) || self.pages.pill.as_ref().is_some_and(|p| p.animating()) || self.pages.edge.is_some()
    }

    /// Before a frame: the pill's text formats; the page shown noticed.
    pub fn pages_before_frame(&mut self) {
        if self.pages.shown.is_none() {
            self.pages.shown = Some(self.layout.page);
        }
        if self.pages.pill.is_some() && self.pages.tf.is_none() {
            self.pages.tf = self.gpu.text_format_weight("Segoe UI Variable Display", 15.0, 600).or_else(|_| self.gpu.text_format("Segoe UI", 15.0, true)).ok();
            self.pages.dot_tf = self.gpu.text_format("Segoe UI", 11.0, false).ok();
            *self.pages.title_layout.borrow_mut() = None;
        }
    }

    /// After a frame: finished animations let go.
    pub fn pages_after_frame(&mut self) {
        if self.pages.anim.as_ref().is_some_and(|a| a.done()) {
            self.pages.anim = None;
            self.pages.other = None;
        }
        if self.pages.pill.as_ref().is_some_and(|p| p.out.is_some_and(|o| o.elapsed().as_secs_f32() >= PILL_OUT_S)) {
            self.pages.pill = None;
        }
        // Held at the edge long enough (or let go): its timer is due. While the edge's hint is
        // drawn the frames keep the queue busy, and Windows' drag loop never gets round to a
        // timer (WM_TIMER comes only when nothing else is waiting), so the frame posts it.
        if self.pages.edge.is_some_and(|(_, since)| since.elapsed().as_secs_f32() >= EDGE_HOLD_S) || (self.pages.edge.is_some() && !button_held()) {
            unsafe {
                let _ = PostMessageW(Some(self.msg_hwnd), WM_TIMER, windows::Win32::Foundation::WPARAM(T_PAGE_EDGE), windows::Win32::Foundation::LPARAM(0));
            }
        }
    }

    /// The page's title laid out in the pill's format (kept while the title stays).
    fn pill_title(&self, title: &str) -> Option<r::TextLayout> {
        let tf = self.pages.tf.as_ref()?;
        let mut c = self.pages.title_layout.borrow_mut();
        if let Some((t, l)) = c.as_ref() {
            if t == title {
                return Some(l.clone());
            }
        }
        let l = self.gpu.label(tf, title, 600.0, 22.0, 1, false).ok()?;
        *c = Some((title.to_string(), l.clone()));
        Some(l)
    }

    /// Where the page's name is shown now (its full size), if it is.
    pub fn page_pill_rect(&self) -> Option<Rect> {
        let p = self.pages.pill.as_ref()?;
        let title = self.page_title(self.layout.page);
        let l = self.pill_title(&title)?;
        let now_w = (r::text_width(&l).ceil().max(dots_width(pg::count(&self.layout))) + 44.0).max(128.0);
        let w = match &p.name_from {
            Some((_, _, w0, t0)) => lerp(*w0, now_w, ease_out(t0.elapsed().as_secs_f32() / NAME_S)),
            None => now_w,
        };
        let cx = self.work.x + self.work.w / 2.0;
        let y = self.work.y + 14.0 + self.search_top_room();
        Some(Rect::new(((cx - w / 2.0) * self.scale).round() / self.scale, (y * self.scale).round() / self.scale, w, PILL_H))
    }

    /// The page's name at the top centre: Fluent glass like the search pill, the name (sliding
    /// past the one before it) and a dot per page, the page's own one gliding to it.
    pub fn paint_page_pill(&self, dc: &Dc) -> r::R<()> {
        let (Some(p), Some(tf), Some(rect)) = (self.pages.pill.as_ref(), self.pages.tf.as_ref(), self.page_pill_rect()) else { return Ok(()) };
        let a = p.alpha().clamp(0.0, 1.0);
        if a <= 0.002 {
            return Ok(());
        }
        let light = self.light;
        let (bg, fg, edge) = if light {
            (rgba(0xF9, 0xF9, 0xF9, 0.9 * a), rgba(0, 0, 0, 0.9 * a), rgba(0, 0, 0, 0.09 * a))
        } else {
            (rgba(0x2B, 0x2B, 0x2B, 0.86 * a), rgba(255, 255, 255, a), rgba(255, 255, 255, 0.12 * a))
        };
        let s = 0.96 + 0.04 * a;
        let rad = 14.0;
        r::scaled(dc, rect.x + rect.w / 2.0, rect.y + rect.h / 2.0, s, || {
            for (spread, alpha) in [(1.0, 0.12), (3.0, 0.06), (7.0, 0.035)] {
                r::fill_round(dc, rf(rect.offset(0.0, spread * 0.6).inflate(spread)), rad + spread, rgba(0, 0, 0, alpha * a))?;
            }
            if let Some(bd) = self.backdrops.get(&(self.settings.default_style.blur as i32)).filter(|_| !self.is_above()) {
                let k = self.backdrop_down as f32 / self.scale;
                r::push_layer(dc, rf(rect), a)?;
                r::fill_with_backdrop(dc, bd, k, self.mon_offset(), rf(rect), rad)?;
                r::pop_layer(dc);
                r::fill_round(dc, rf(rect), rad, Color { a: bg.a * 0.8, ..bg })?;
            } else {
                r::fill_round(dc, rf(rect), rad, bg)?;
            }
            r::stroke_round(dc, rf(rect.inflate(-0.5)), rad - 0.5, edge, 1.0)?;
            // The name (the one before it sliding out the other way).
            r::push_clip(dc, rf(rect.inflate(-6.0)));
            let name_y = rect.y + 8.0;
            let title = self.page_title(self.layout.page);
            let Some(l) = self.pill_title(&title) else { return Ok(()) };
            let nx = |l: &r::TextLayout| rect.x + (rect.w - r::text_width(l)) / 2.0;
            match &p.name_from {
                Some((old, dir, _, t0)) if t0.elapsed().as_secs_f32() < NAME_S => {
                    let e = ease_out(t0.elapsed().as_secs_f32() / NAME_S);
                    let d = 28.0;
                    // (Staggered: the old name is gone before the new one is clear, so the two
                    // never read as one smudge.)
                    let (a_old, a_new) = ((1.0 - e * 2.2).max(0.0), ((e - 0.3) / 0.7).clamp(0.0, 1.0));
                    if let Ok(ol) = self.gpu.label(tf, old, 600.0, 22.0, 1, false) {
                        r::draw_text(dc, &ol, nx(&ol) - dir * d * e, name_y, Color { a: fg.a * a_old, ..fg })?;
                    }
                    r::draw_text(dc, &l, nx(&l) + dir * d * (1.0 - e), name_y, Color { a: fg.a * a_new, ..fg })?;
                }
                _ => r::draw_text(dc, &l, nx(&l), name_y, fg)?,
            }
            r::pop_clip(dc);
            // A dot per page.
            let n = pg::count(&self.layout);
            let i = pg::current_index(&self.layout) as f32;
            let dy = rect.y + 36.0;
            let (ar, ag, ab) = crate::app::accent();
            if n > MAX_DOTS {
                if let Some(dtf) = &self.pages.dot_tf {
                    let c = self.gpu.label(dtf, &format!("{} / {n}", i as usize + 1), 100.0, 14.0, 1, true)?;
                    r::draw_text(dc, &c, rect.x + (rect.w - r::text_width(&c)) / 2.0, dy - 4.0, Color { a: fg.a * 0.6, ..fg })?;
                }
                return Ok(());
            }
            let pos = match p.dot_from {
                Some((from, t0)) => lerp(from, i, ease_out(t0.elapsed().as_secs_f32() / DOT_S)),
                None => i,
            };
            let x0 = rect.x + (rect.w - dots_width(n)) / 2.0 + (DOT_ON_W - DOT) / 2.0;
            for k in 0..n {
                let near = (k as f32 - pos).abs().min(1.0);
                let da = fg.a * 0.36 * near;
                if da > 0.004 {
                    r::fill_round(dc, rf(Rect::new(x0 + k as f32 * DOT_STEP, dy, DOT, DOT)), DOT / 2.0, Color { a: da, ..fg })?;
                }
            }
            let cx = x0 + pos * DOT_STEP + DOT / 2.0;
            r::fill_round(dc, rf(Rect::new(cx - DOT_ON_W / 2.0, dy, DOT_ON_W, DOT)), DOT / 2.0, rgba(ar, ag, ab, a))?;
            Ok(())
        })
    }

    /// The page under the pointer in the pill's dots.
    fn page_dot_at(&self, p: Pt) -> Option<u32> {
        let rect = self.page_pill_rect()?;
        let n = pg::count(&self.layout);
        if n > MAX_DOTS || !rect.contains(p) || p.y < rect.y + 28.0 {
            return None;
        }
        let x0 = rect.x + (rect.w - dots_width(n)) / 2.0 + (DOT_ON_W - DOT) / 2.0 + DOT / 2.0;
        let k = ((p.x - x0) / DOT_STEP).round();
        (k >= 0.0 && (k as usize) < n && (p.x - (x0 + k * DOT_STEP)).abs() <= DOT_STEP / 2.0).then(|| pg::id_at(&self.layout, k as usize)).flatten()
    }

    /// A drag held at a screen edge: the edge lights up (more as the hold goes on) with an arrow,
    /// or a + past the last page.
    pub fn paint_page_edge(&self, dc: &Dc) -> r::R<()> {
        let Some((dir, since)) = self.pages.edge else { return Ok(()) };
        let k = (since.elapsed().as_secs_f32() / EDGE_HOLD_S).min(1.0);
        let e = ease_out((since.elapsed().as_secs_f32() / 0.15).min(1.0));
        let (ar, ag, ab) = crate::app::accent();
        let w = 56.0;
        let x_edge = if dir < 0.0 { self.work.x } else { self.work.right() };
        let steps = 14;
        for s in 0..steps {
            let f = s as f32 / steps as f32;
            let a = (1.0 - f) * (1.0 - f) * (0.16 + 0.22 * k) * e;
            let sw = w / steps as f32;
            let x = if dir < 0.0 { x_edge + f * w } else { x_edge - (f * w) - sw };
            r::fill_rect(dc, rf(Rect::new(x, self.work.y, sw + 0.5, self.work.h)), rgba(ar, ag, ab, a))?;
        }
        // The arrow (or +) in a circle that fills as the hold goes on.
        let c = Pt { x: if dir < 0.0 { x_edge + 30.0 } else { x_edge - 30.0 }, y: self.work.y + self.work.h / 2.0 };
        let d = 40.0;
        let circle = Rect::new(c.x - d / 2.0, c.y - d / 2.0, d, d);
        r::fill_round(dc, rf(circle.offset(0.0, 1.5).inflate(1.5)), d / 2.0 + 1.5, rgba(0, 0, 0, 0.18 * e))?;
        r::fill_round(dc, rf(circle), d / 2.0, rgba(ar, ag, ab, (0.55 + 0.45 * k) * e))?;
        let last = pg::current_index(&self.layout) + 1 >= pg::count(&self.layout);
        let glyph = if dir < 0.0 {
            "\u{E76B}"
        } else if last {
            "\u{E710}"
        } else {
            "\u{E76C}"
        };
        if let Ok(g) = self.gpu.label(&self.glyph_tf, glyph, d, d, 1, true) {
            r::scaled(dc, c.x, c.y, 1.25, || r::draw_text(dc, &g, c.x - d / 2.0, c.y - r::text_height(&g) / 2.0, rgba(255, 255, 255, e)))?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------ switching

    /// Show page `id`, sliding to it from the side it is on.
    pub fn go_page(&mut self, id: u32) {
        let (Some(i0), Some(i1)) = (pg::index_of(&self.layout, self.layout.page), pg::index_of(&self.layout, id)) else { return };
        if i0 == i1 {
            return;
        }
        let dir = if i1 > i0 { 1.0 } else { -1.0 };
        // (Mid-slide: on from where the page shown is drawn now.)
        let cur = match (&self.pages.gesture, self.page_frame()) {
            (None, Some(f)) if f.other_alpha >= 1.0 => f.dx,
            _ => 0.0,
        };
        self.switch_page(id, dir, cur + dir * self.page_w());
    }

    /// The page `d` pages on (-1: the one before). At the first or last page the desktop gives a
    /// little instead.
    pub fn page_step(&mut self, d: i32) {
        let i = pg::current_index(&self.layout) as i32 + d;
        match (i >= 0).then(|| pg::id_at(&self.layout, i as usize)).flatten() {
            Some(id) => self.go_page(id),
            None => self.page_bump(d as f32),
        }
    }

    /// No page that way: the page shown gives a little and comes back.
    fn page_bump(&mut self, dir: f32) {
        if !self.page_slides() || self.pages.anim.as_ref().is_some_and(|a| !a.done()) {
            return;
        }
        self.pages.other = None;
        self.pages.anim = Some(Slide { from: -dir * 36.0, side: -dir, t0: Instant::now(), dur: 0.32, bounce: false, fade: false });
        crate::anim::set(true);
        self.invalidate();
    }

    /// The layout before a drag takes things to another page (kept for its undo, or its cancel).
    pub(crate) fn page_keep_before(&mut self) {
        let carrying = matches!(self.drag, Drag::MoveBox { .. } | Drag::Items { .. }) || self.own_drag.is_some();
        if carrying && self.pages.before.is_none() {
            let mut b = self.layout.clone();
            if let Drag::MoveBox { id, orig, .. } = &self.drag {
                if let Some(x) = b.boxes.iter_mut().find(|x| x.id == *id) {
                    x.rect = *orig;
                }
            }
            self.pages.before = Some(b);
        }
    }

    /// Show page `id`; the page shown so far slides away to the side opposite `dir` (1: the new
    /// page is after it), the new page from `from` (DIPs) to rest.
    pub fn switch_page(&mut self, id: u32, dir: f32, from: f32) {
        let old = self.layout.page;
        if id == old || pg::index_of(&self.layout, id).is_none() {
            return;
        }
        let old_title = self.page_title(old);
        let old_w = self.page_pill_rect().map(|r| r.w);
        let old_dot = self.page_dot_pos();
        self.page_keep_before();
        // Icons dragged: where they were on their page.
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
        // Whatever else was going on on the page shown ends.
        match self.drag {
            Drag::MoveBox { .. } | Drag::Items { .. } | Drag::None => {}
            _ => {
                unsafe {
                    let _ = ReleaseCapture();
                }
                self.cancel_drag();
            }
        }
        self.end_rename(true);
        self.hide_tip();
        self.sel.clear();
        self.hover = None;
        self.hover_hdr = None;
        self.pending_tab = None;
        self.revealed.clear();
        self.room = None;
        self.box_want = None;
        self.box_room = None;
        // A box being moved goes along.
        let carry = match &self.drag {
            Drag::MoveBox { id, .. } => pg::take_box(&mut self.layout, *id),
            _ => None,
        };
        let ok = pg::switch_to(&mut self.layout, id);
        if let Some(b) = carry {
            self.layout.boxes.push(b);
        }
        if !ok {
            return;
        }
        if matches!(self.drag, Drag::MoveBox { .. }) {
            self.update_preview();
        }
        self.pages.shown = Some(id);
        let fade = !self.page_slides();
        let w = self.page_w();
        let bounce = self.settings.pages.bounce && !fade;
        let dur = if fade { FADE_S } else { (0.16 + (SLIDE_S - 0.16) * (from.abs() / w).min(1.0)).max(0.16) + if bounce { BOUNCE_EXTRA_S } else { 0.0 } };
        self.pages.other = Some(old);
        self.pages.anim = Some(Slide { from, side: dir, t0: Instant::now(), dur, bounce, fade });
        self.show_page_pill(Some((old_title, dir, old_w)), old_dot);
        log::info!("page {} of {} ({id})", pg::current_index(&self.layout) + 1, pg::count(&self.layout));
        self.layout_changed();
        crate::anim::set(true);
    }

    /// The layout's page changed by itself (undo, a saved layout ...): slide to it.
    pub fn page_check_shown(&mut self) {
        let now = self.layout.page;
        let Some(was) = self.pages.shown else {
            self.pages.shown = Some(now);
            return;
        };
        if was == now {
            return;
        }
        self.pages.shown = Some(now);
        let (i0, i1) = (pg::index_of(&self.layout, was), pg::current_index(&self.layout));
        let dir = match i0 {
            Some(i0) if i0 > i1 => -1.0,
            _ => 1.0,
        };
        let fade = !self.page_slides() || i0.is_none();
        self.sel.clear();
        self.pages.other = i0.map(|_| was);
        let bounce = self.settings.pages.bounce && !fade;
        self.pages.anim = Some(Slide { from: dir * self.page_w(), side: dir, t0: Instant::now(), dur: if fade { FADE_S } else { SLIDE_S + if bounce { BOUNCE_EXTRA_S } else { 0.0 } }, bounce, fade });
        self.show_page_pill(None, None);
        crate::anim::set(true);
    }

    // ------------------------------------------------------------------ the name pill

    fn page_dot_pos(&self) -> Option<f32> {
        let p = self.pages.pill.as_ref()?;
        let i = pg::current_index(&self.layout) as f32;
        Some(match p.dot_from {
            Some((from, t0)) => lerp(from, i, ease_out(t0.elapsed().as_secs_f32() / DOT_S)),
            None => i,
        })
    }

    /// Show the page's name for a moment (`old`: the name it replaces, the direction, the pill's
    /// width then; `dot`: where the active dot was).
    pub fn show_page_pill(&mut self, old: Option<(String, f32, Option<f32>)>, dot: Option<f32>) {
        if !self.settings.pages.show_name || !self.pages_on() {
            return;
        }
        let now = Instant::now();
        let i = pg::current_index(&self.layout) as f32;
        match self.pages.pill.as_mut() {
            Some(p) => {
                if p.out.is_some() {
                    p.a0 = p.alpha();
                    p.t0 = now;
                    p.out = None;
                }
                if let Some((name, dir, w)) = old {
                    p.name_from = w.map(|w| (name, dir, w, now));
                }
                p.dot_from = dot.map(|d| (d, now));
            }
            None => {
                let from = old.as_ref().map(|(_, dir, _)| i - dir).filter(|f| *f >= 0.0);
                self.pages.pill = Some(Pill { t0: now, a0: 0.0, out: None, hover: false, name_from: None, dot_from: from.map(|f| (f, now)) });
            }
        }
        unsafe {
            SetTimer(Some(self.msg_hwnd), T_PAGE_PILL, PILL_HOLD_MS, None);
        }
        crate::anim::set(true);
        self.invalidate();
    }

    fn page_pill_due(&mut self) {
        let Some(p) = self.pages.pill.as_mut() else { return };
        if p.hover {
            unsafe {
                SetTimer(Some(self.msg_hwnd), T_PAGE_PILL, 500, None);
            }
            return;
        }
        if p.out.is_none() {
            p.out = Some(Instant::now());
            crate::anim::set(true);
            self.invalidate();
        }
    }

    /// The pointer on the pill keeps it shown.
    pub fn page_pill_hover(&mut self, p: Pt) {
        let on = self.page_pill_rect().is_some_and(|r| r.contains(p));
        if let Some(pill) = self.pages.pill.as_mut() {
            if on != pill.hover {
                pill.hover = on;
                if on && pill.out.is_some() {
                    pill.a0 = pill.alpha();
                    pill.t0 = Instant::now();
                    pill.out = None;
                    crate::anim::set(true);
                    self.invalidate();
                }
                if !on {
                    unsafe {
                        SetTimer(Some(self.msg_hwnd), T_PAGE_PILL, 700, None);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ input

    pub fn page_timer(&mut self, id: usize) {
        match id {
            T_PAGE_EDGE => self.page_edge_due(),
            T_PAGE_SWIPE => self.page_gesture_end(),
            T_PAGE_PILL => self.page_pill_due(),
            _ => {}
        }
    }

    /// Alt+Left / Alt+Right.
    pub fn page_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        if !self.pages_on() || !self.settings.pages.alt_keys || !crate::app::key_down(VK_MENU) {
            return false;
        }
        let d = match vk {
            VK_LEFT => -1,
            VK_RIGHT => 1,
            _ => return false,
        };
        self.page_step(d);
        true
    }

    /// Alt+wheel: a page per notch (down: the next one). True: taken.
    pub fn page_wheel(&mut self, delta: i16) -> bool {
        if !self.pages_on() || !self.settings.pages.alt_keys || !crate::app::key_down(VK_MENU) || delta == 0 {
            return false;
        }
        if self.pages.step_at.is_some_and(|t| t.elapsed().as_millis() < 220) {
            return true;
        }
        self.pages.step_at = Some(Instant::now());
        self.page_step(if delta < 0 { 1 } else { -1 });
        true
    }

    /// The horizontal wheel: a touchpad's two-finger swipe moves the pages with the fingers; a
    /// tilt wheel's notch goes a page.
    pub fn page_hwheel(&mut self, delta: i16) {
        if !self.pages_on() || !self.settings.pages.swipe || delta == 0 {
            return;
        }
        if delta % 120 == 0 && self.pages.gesture.is_none() {
            if self.pages.step_at.is_some_and(|t| t.elapsed().as_millis() < 300) {
                return;
            }
            self.pages.step_at = Some(Instant::now());
            self.page_step(if delta > 0 { 1 } else { -1 });
            return;
        }
        if self.pages.gesture.is_none() {
            self.page_gesture_begin(By::Swipe, 0.0);
        }
        let raw = self.pages.gesture.as_ref().map(|g| g.raw).unwrap_or(0.0) - delta as f32 * SWIPE_K;
        self.page_gesture_to(raw);
        unsafe {
            SetTimer(Some(self.msg_hwnd), T_PAGE_SWIPE, 140, None);
        }
    }

    /// The middle button pressed or let go on the desktop.
    pub fn page_mbutton(&mut self, down: bool, p: Pt) {
        if down {
            if self.pages_on() && self.settings.pages.middle_drag && self.pages.gesture.is_none() && matches!(self.drag, Drag::None) {
                unsafe {
                    SetCapture(self.hwnd);
                }
                self.page_gesture_begin(By::Middle, p.x);
            }
        } else if self.pages.gesture.as_ref().is_some_and(|g| g.by == By::Middle) {
            unsafe {
                let _ = ReleaseCapture();
            }
            self.page_gesture_end();
        }
    }

    /// The left button pressed: on the page's name (a dot goes to its page; a double-click
    /// renames), the right one already held on the desktop (both buttons drag the pages), or at
    /// a screen edge (pulls the pages). True: taken.
    pub fn page_lbutton_down(&mut self, p: Pt, dbl: bool) -> bool {
        if self.page_pill_rect().is_some_and(|r| r.contains(p)) {
            if dbl {
                self.rename_page_dialog(self.layout.page);
            } else if let Some(id) = self.page_dot_at(p) {
                self.go_page(id);
            }
            return true;
        }
        if !self.pages_on() {
            return false;
        }
        if self.settings.pages.both_drag {
            if let Drag::RightRubber { scope: None, .. } = self.drag {
                self.drag = Drag::None;
                self.sel.clear();
                self.page_gesture_begin(By::Both, p.x);
                return true;
            }
        }
        if self.settings.pages.edge_pull && !dbl && !self.is_above() && (p.x <= self.work.x + EDGE_PULL || p.x >= self.work.right() - EDGE_PULL) && self.hit_test(p) == Hit::Empty {
            unsafe {
                SetCapture(self.hwnd);
            }
            self.page_gesture_begin(By::Edge, p.x);
            return true;
        }
        false
    }

    /// The right button pressed: on the page's name (the Pages menu), or the left one already
    /// held on the desktop (both buttons drag the pages). True: taken.
    pub fn page_rbutton_down(&mut self, p: Pt) -> bool {
        if self.page_pill_rect().is_some_and(|r| r.contains(p)) {
            self.pages.swallow_up |= 2;
            self.show_pages_menu(p);
            return true;
        }
        if !self.pages_on() || !self.settings.pages.both_drag {
            return false;
        }
        let desk = match &self.drag {
            Drag::Pending { hit: Hit::Empty, .. } => true,
            Drag::Rubber { scope: None, base, .. } => {
                let base = base.clone();
                self.sel.set = base;
                true
            }
            _ => false,
        };
        if desk {
            self.drag = Drag::None;
            unsafe {
                SetCapture(self.hwnd);
            }
            self.page_gesture_begin(By::Both, p.x);
            return true;
        }
        false
    }

    /// A button let go: the end of a gesture (or the other button of one just ended). True:
    /// taken.
    pub fn page_button_up(&mut self, right: bool) -> bool {
        let bit = if right { 2 } else { 1 };
        if self.pages.swallow_up & bit != 0 {
            self.pages.swallow_up &= !bit;
            return true;
        }
        match self.pages.gesture.as_ref().map(|g| g.by) {
            Some(By::Both) => {
                self.pages.swallow_up |= if right { 1 } else { 2 };
                unsafe {
                    let _ = ReleaseCapture();
                }
                self.page_gesture_end();
                true
            }
            Some(By::Edge) if !right => {
                unsafe {
                    let _ = ReleaseCapture();
                }
                self.page_gesture_end();
                true
            }
            _ => false,
        }
    }

    /// The pointer moved: the pages follow it in a gesture. True: taken.
    pub fn page_gesture_move(&mut self, p: Pt) -> bool {
        match self.pages.gesture.as_ref() {
            Some(g) if g.by != By::Swipe => {
                let raw = p.x - g.x0;
                self.page_gesture_to(raw);
                true
            }
            _ => false,
        }
    }

    /// The mouse was taken away mid-gesture: the pages settle.
    pub fn page_gesture_lost(&mut self) {
        if self.pages.gesture.as_ref().is_some_and(|g| g.by != By::Swipe) {
            self.page_gesture_end();
        }
    }

    fn page_gesture_begin(&mut self, by: By, x: f32) {
        // Caught mid-slide: from where the page is drawn now.
        let off = match self.page_frame() {
            Some(f) if f.other_alpha >= 1.0 && self.pages.other.is_none() => f.dx,
            _ => 0.0,
        };
        self.pages.anim = None;
        self.pages.other = None;
        self.hide_tip();
        self.pages.gesture = Some(Gesture { by, x0: x - off, raw: off, last: (off, Instant::now()), vel: 0.0, off });
        crate::anim::set(true);
    }

    /// The gesture has gone `raw` DIPs: the page shown follows, held back past the first and
    /// the last page.
    fn page_gesture_to(&mut self, raw: f32) {
        let w = self.page_w();
        let i = pg::current_index(&self.layout);
        let n = pg::count(&self.layout);
        let free = if raw > 0.0 { i > 0 } else { i + 1 < n };
        let off = if free {
            raw.clamp(-w, w)
        } else {
            let d = w * 0.2;
            raw.signum() * d * (1.0 - 1.0 / (raw.abs() * 0.55 / d + 1.0))
        };
        let Some(g) = self.pages.gesture.as_mut() else { return };
        let now = Instant::now();
        let dt = now.duration_since(g.last.1).as_secs_f32();
        if dt > 0.004 {
            let v = (raw - g.last.0) / dt;
            g.vel = if dt > 0.1 { v } else { g.vel * 0.6 + v * 0.4 };
            g.last = (raw, now);
        }
        g.raw = raw;
        g.off = off;
        self.invalidate();
    }

    /// The gesture ended: on to the page beside if it went a quarter of the way or was flicked,
    /// else back.
    fn page_gesture_end(&mut self) {
        let Some(g) = self.pages.gesture.take() else { return };
        let w = self.page_w();
        let i = pg::current_index(&self.layout);
        let n = pg::count(&self.layout);
        // (A flick that has stopped is no flick.)
        // (A swipe is over only once its events have stopped for a moment: its last speed counts.)
        let stale = if g.by == By::Swipe { 0.3 } else { 0.12 };
        let vel = if g.last.1.elapsed().as_secs_f32() > stale { 0.0 } else { g.vel };
        let off = g.off;
        let next = (off < -0.25 * w || (vel < -FLING && off < 0.0)) && i + 1 < n;
        let prev = (off > 0.25 * w || (vel > FLING && off > 0.0)) && i > 0;
        log::info!("page gesture {:?}: {off:.0} of {w:.0} DIPs at {vel:.0}/s -> {}", g.by, if next { "next" } else if prev { "previous" } else { "back" });
        if next || prev {
            let d = if next { 1 } else { -1 };
            if let Some(id) = pg::id_at(&self.layout, (i as i32 + d) as usize) {
                let dir = d as f32;
                self.switch_page(id, dir, off + dir * w);
                return;
            }
        }
        if off.abs() > 0.5 {
            self.pages.other = None;
            self.pages.anim = Some(Slide { from: off, side: if off >= 0.0 { 1.0 } else { -1.0 }, t0: Instant::now(), dur: (0.12 + 0.2 * (off.abs() / w)).min(0.3), bounce: false, fade: false });
        }
        crate::anim::set(true);
        self.invalidate();
    }

    // ------------------------------------------------------------------ drags across pages

    /// Something dragged (icons, a box, files from an app) is at `p` (None: gone): held at the
    /// left or right edge, it goes to the page beside.
    pub fn page_edge_over(&mut self, p: Option<Pt>) {
        if !self.pages_on() || !self.settings.pages.edge_drag {
            if self.pages.edge.take().is_some() {
                self.invalidate();
            }
            return;
        }
        let dir = match p {
            Some(p) if p.x <= self.work.x + EDGE_DRAG && p.y >= self.work.y && p.y <= self.work.bottom() => -1.0,
            Some(p) if p.x >= self.work.right() - EDGE_DRAG && p.y >= self.work.y && p.y <= self.work.bottom() => 1.0,
            _ => 0.0,
        };
        // Nowhere to go on the left of the first page.
        let dir = if dir < 0.0 && pg::current_index(&self.layout) == 0 { 0.0 } else { dir };
        match (dir, self.pages.edge) {
            (d, Some((e, _))) if d == e => {}
            (0.0, None) => {}
            (0.0, Some(_)) => {
                self.pages.edge = None;
                self.invalidate();
            }
            (d, _) => {
                self.pages.edge = Some((d, Instant::now()));
                crate::anim::set(true);
                unsafe {
                    SetTimer(Some(self.msg_hwnd), T_PAGE_EDGE, 60, None);
                }
            }
        }
    }

    fn page_edge_due(&mut self) {
        // The drag ended without us hearing of it (files from an app dropped elsewhere).
        let dragging = matches!(self.drag, Drag::MoveBox { .. } | Drag::Items { .. }) || self.own_drag.is_some() || self.drop_at.is_some();
        if !button_held() && !dragging {
            self.page_drag_ended();
            return;
        }
        if let Some((dir, since)) = self.pages.edge {
            let sliding = self.pages.anim.as_ref().is_some_and(|a| !a.done());
            if since.elapsed().as_secs_f32() >= EDGE_HOLD_S && !sliding {
                self.page_flip(dir);
                // Held on: another page after a longer wait (the slide, then the hold again), so
                // a hand that lingers a moment doesn't go two pages.
                if self.pages.edge.is_some() {
                    self.pages.edge = Some((dir, Instant::now() + std::time::Duration::from_millis(400)));
                }
            }
        }
        if self.pages.edge.is_some() || self.pages.made.is_some() || self.pages.before.is_some() {
            unsafe {
                SetTimer(Some(self.msg_hwnd), T_PAGE_EDGE, 60, None);
            }
        }
    }

    fn page_empty(&self, id: u32) -> bool {
        if id == self.layout.page {
            return self.layout.boxes.is_empty() && !self.layout.loose.keys().any(|k| self.items.contains_key(k));
        }
        self.layout.parked.get(&id).is_some_and(|c| c.boxes.is_empty() && !c.loose.keys().any(|k| self.items.contains_key(k)))
    }

    /// The drag goes to the page beside (a new one past the last).
    fn page_flip(&mut self, dir: f32) {
        let i = pg::current_index(&self.layout) as i32 + dir as i32;
        if i < 0 {
            return;
        }
        let id = match pg::id_at(&self.layout, i as usize) {
            Some(id) => id,
            None => {
                // One new page per drag: held on at the edge of the page it made, the drag stays
                // (the box or icons carried along would make that page count as used).
                if self.pages.made.is_some() && self.pages.made == Some(self.layout.page) {
                    return;
                }
                self.page_keep_before();
                let id = pg::new_page(&mut self.layout, "", None);
                log::info!("page {id} made by a drag at the edge");
                self.pages.made = Some(id);
                id
            }
        };
        self.go_page(id);
    }

    /// Something was dropped on the page shown: a page made for the drag stays.
    pub fn page_dropped(&mut self) {
        if self.pages.made == Some(self.layout.page) {
            self.pages.made = None;
        }
    }

    /// A drag ended: a page it made and left empty goes again; what it kept for undo is let go.
    pub fn page_drag_ended(&mut self) {
        if self.pages.edge.take().is_some() {
            self.invalidate();
        }
        self.pages.carried.clear();
        self.pages.before = None;
        if let Some(m) = self.pages.made.take() {
            if self.page_empty(m) && pg::count(&self.layout) > 1 {
                if m == self.layout.page {
                    if let Some(to) = pg::neighbour(&self.layout, m) {
                        self.go_page(to);
                    }
                }
                let g = self.grid.clone();
                pg::delete(&mut self.layout, m, &g);
                log::info!("page {m} made by a drag removed again (left empty)");
                self.layout_changed();
            }
        }
    }

    /// A drag that took things to another page was cancelled: everything back where it was.
    pub fn page_drag_cancelled(&mut self) -> bool {
        match self.pages.before.take() {
            Some(b) => {
                self.layout = b;
                self.pages.made = None;
                self.layout_changed();
                true
            }
            None => false,
        }
    }

    /// New desktop icons onto the page shown, or the first page (Settings › Pages); icons gone
    /// from the disk leave every page.
    pub fn reconcile_pages(&mut self, present: &[ItemKey]) -> bool {
        // Several screens, another one in use: new icons go to the main screen (as Windows puts
        // new desktop files there), onto the page it shows or its first.
        if self.multi_screen() && self.cur != 0 {
            let main = &self.screens[0];
            let (g, id) = (main.grid, main.mon.id.clone());
            let page = if self.pages_on() && self.settings.pages.new_files == "first" { deskboxes_core::screens::pages_of(&self.layout, &id).first().copied() } else { deskboxes_core::screens::shown_on(&self.layout, &id) };
            if let Some(p) = page.filter(|p| *p != self.layout.page) {
                return pg::with_page(&mut self.layout, p, |l| deskboxes_core::layout::reconcile(l, present, &g)).unwrap_or(false);
            }
        }
        let g = self.grid;
        let first = pg::id_at(&self.layout, 0).unwrap_or(self.layout.page);
        if self.pages_on() && self.settings.pages.new_files == "first" && first != self.layout.page {
            pg::with_page(&mut self.layout, first, |l| deskboxes_core::layout::reconcile(l, present, &g)).unwrap_or(false)
        } else {
            deskboxes_core::layout::reconcile(&mut self.layout, present, &g)
        }
    }

    /// Cells of the icons dragged, on the page shown or the one they came from.
    pub fn drag_cells(&self) -> Vec<(ItemKey, Rect, Option<u32>)> {
        let mut v = self.visible_cells();
        for (k, r) in &self.pages.carried {
            if !v.iter().any(|(x, _, _)| x == k) {
                v.push((k.clone(), *r, None));
            }
        }
        v
    }
}

/// Pages' glyph (Task View) in menus.
pub const PAGES_GLYPH: &str = "\u{E7C4}";

/// What a dialog shown for pages asks.
#[derive(Clone, Debug)]
pub enum Ask {
    /// A name for a new page; then this box or these icons go there, or (`go`) it is shown.
    NewPage { boxed: Option<u32>, items: Vec<ItemKey>, go: bool },
    Rename(u32),
    Delete(u32),
    /// Turning pages off with several: everything onto one first.
    Merge,
    /// A screen was disconnected (`screens.rs`): "Show page" shows the first of its pages.
    ScreenGone(Option<u32>),
    /// Import this export file (`transfer.rs`).
    Import(std::path::PathBuf),
    /// A message with only OK.
    Info,
}

impl App {
    /// Deskboxes › Pages (and the page name's own menu): the pages, then new / rename / delete.
    pub fn pages_menu_entries(&self) -> Vec<serde_json::Value> {
        use serde_json::json;
        let l = self.lang;
        let cur = self.layout.page;
        let mut v: Vec<serde_json::Value> = pg::ids(&self.layout)
            .into_iter()
            .map(|id| json!({ "id": format!("dbx.page:{id}"), "text": self.page_title(id), "radio": true, "checked": id == cur }))
            .collect();
        v.push(json!({ "separator": true }));
        v.push(json!({ "id": "dbx.pagenew", "text": t(l, S::NewPage), "glyph": "\u{E710}" }));
        let name = self.page_title(cur);
        v.push(json!({ "id": "dbx.pagerename", "text": t(l, S::RenamePageMenu).replace("{name}", &name), "glyph": "\u{E8AC}" }));
        v.push(json!({ "id": "dbx.pagedelete", "text": t(l, S::DeletePageMenu).replace("{name}", &name), "glyph": "\u{E74D}", "enabled": pg::count(&self.layout) > 1 }));
        v.push(json!({ "separator": true }));
        v.push(json!({ "id": "dbx.pagesettings", "text": t(l, S::PagesSettings), "glyph": "\u{E713}" }));
        v
    }

    /// "Move to page ›": the other pages and a new one (ids `prefix` + page id, `prefix` + "new").
    pub fn move_to_page_entry(&self, prefix: &str, placement: Option<&str>) -> serde_json::Value {
        use serde_json::json;
        let mut ch: Vec<serde_json::Value> = pg::ids(&self.layout)
            .into_iter()
            .filter(|id| *id != self.layout.page)
            .map(|id| json!({ "id": format!("{prefix}{id}"), "text": self.page_title(id) }))
            .collect();
        if !ch.is_empty() {
            ch.push(json!({ "separator": true }));
        }
        ch.push(json!({ "id": format!("{prefix}new"), "text": t(self.lang, S::NewPage), "glyph": "\u{E710}" }));
        let mut e = json!({ "id": format!("{prefix}menu"), "text": t(self.lang, S::MoveToPage), "glyph": PAGES_GLYPH, "children": ch });
        if let Some(p) = placement {
            e["placement"] = json!(p);
        }
        e
    }

    /// The page name's menu (right-click on it): the Pages menu alone.
    pub fn show_pages_menu(&mut self, at: Pt) {
        self.pages.menu_own = Some(self.pages_menu_entries());
        self.show_menu(Hit::Empty, at, false, None);
    }

    /// A Pages command from a menu (`box_id`: the box the menu was for).
    pub fn page_command(&mut self, cmd: &str, box_id: Option<u32>) {
        if let Some(id) = cmd.strip_prefix("dbx.page:").and_then(|s| s.parse::<u32>().ok()) {
            self.go_page(id);
            return;
        }
        match cmd {
            "dbx.pagenew" => self.new_page_dialog(None, vec![], true),
            "dbx.pagerename" => self.rename_page_dialog(self.layout.page),
            "dbx.pagedelete" => self.delete_page_ask(self.layout.page),
            "dbx.pagesettings" => {
                self.settings_page = Some("pages");
                self.send_settings(true, None);
            }
            _ => {}
        }
        if let Some(rest) = cmd.strip_prefix("dbx.pagebox:") {
            let Some(b) = box_id else { return };
            if self.locks_of(b).position {
                log::info!("box {b} is locked in place: not moved to another page");
                return;
            }
            match rest {
                "new" => self.new_page_dialog(Some(b), vec![], false),
                s => {
                    if let Ok(to) = s.parse::<u32>() {
                        log::info!("box {b} to page {to}");
                        self.change_layout(|l, g| {
                            pg::move_box_to_page(l, b, to, g);
                        });
                    }
                }
            }
        }
        if let Some(rest) = cmd.strip_prefix("dbx.pageitems:") {
            let keys = self.sel.ordered(&self.order());
            if keys.is_empty() {
                return;
            }
            // As a drag: icons out of a box whose icons are locked (or with everything locked) stay.
            if !self.icons_movable(&keys, None) {
                log::info!("icons locked: not moved to another page");
                return;
            }
            match rest {
                "new" => self.new_page_dialog(None, keys, false),
                s => {
                    if let Ok(to) = s.parse::<u32>() {
                        log::info!("{} icon(s) to page {to}", keys.len());
                        self.change_layout(|l, g| {
                            pg::move_items_to_page(l, &keys, to, g);
                        });
                        self.sel.clear();
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ dialogs

    /// Ask in a Fluent dialog of the shell host's (`ShowDialog`); the answer comes back to
    /// `on_dialog`.
    pub(crate) fn open_dialog(&mut self, ask: Ask, mut body: serde_json::Value) {
        let Some(h) = self.host.clone() else {
            log::warn!("dialog asked for before the shell host is ready");
            return;
        };
        self.pages.dialog_seq += 1;
        let id = self.pages.dialog_seq;
        body["dialog"] = serde_json::json!(id);
        // We received the click: the dialog may take the foreground.
        if let Some(c) = &self.host_child {
            unsafe {
                let _ = AllowSetForegroundWindow(c.id());
            }
        }
        match h.send("dialog", body) {
            Ok(_) => self.pages.dialog = Some((id, ask)),
            Err(e) => log::warn!("dialog: {e}"),
        }
    }

    /// A screen was disconnected and its pages are on the main screen now: say so (one dialog at
    /// a time; at the start, once the shell host is there).
    pub fn screen_notice(&mut self, screen: &str, page: &str, n: usize, first: Option<u32>) {
        self.pages.screen_notices.push((screen.to_string(), page.to_string(), n, first));
        self.next_screen_notice();
    }

    pub fn next_screen_notice(&mut self) {
        if self.host.is_none() || self.pages.dialog.is_some() || self.pages.screen_notices.is_empty() {
            return;
        }
        let (screen, page, n, first) = self.pages.screen_notices.remove(0);
        let l = self.lang;
        let text = if n > 1 { t(l, S::ScreenGoneBodyMore).replace("{n}", &n.to_string()) } else { t(l, S::ScreenGoneBody).to_string() };
        let body = serde_json::json!({
            "kind": "confirm",
            "title": t(l, S::ScreenGoneTitle).replace("{screen}", &screen),
            "body": text.replace("{page}", &page),
            "ok": t(l, S::ShowPage),
            "cancel": t(l, S::Close),
        });
        self.open_dialog(Ask::ScreenGone(first), body);
    }

    /// A name for a new page (suggested: "Page N").
    pub fn new_page_dialog(&mut self, boxed: Option<u32>, items: Vec<ItemKey>, go: bool) {
        let l = self.lang;
        let n = pg::count(&self.layout) + 1;
        let suggested = t(l, S::PageN).replace("{n}", &n.to_string());
        let body = serde_json::json!({ "kind": "text", "title": t(l, S::NewPageTitle), "value": suggested, "placeholder": t(l, S::PageNamePlaceholder), "ok": t(l, S::Create), "cancel": t(l, S::Cancel) });
        self.open_dialog(Ask::NewPage { boxed, items, go }, body);
    }

    pub fn rename_page_dialog(&mut self, id: u32) {
        let l = self.lang;
        let body = serde_json::json!({ "kind": "text", "title": t(l, S::RenamePageTitle), "value": self.page_title(id), "placeholder": t(l, S::PageNamePlaceholder), "ok": t(l, S::Save), "cancel": t(l, S::Cancel) });
        self.open_dialog(Ask::Rename(id), body);
    }

    /// Delete a page: asked first unless it is empty (its things go to its neighbour).
    pub fn delete_page_ask(&mut self, id: u32) {
        if pg::count(&self.layout) < 2 || pg::index_of(&self.layout, id).is_none() {
            return;
        }
        if self.page_empty(id) {
            self.delete_page(id);
            return;
        }
        let l = self.lang;
        let to = pg::neighbour(&self.layout, id).map(|n| self.page_title(n)).unwrap_or_default();
        let body = serde_json::json!({
            "kind": "confirm",
            "title": t(l, S::DeletePageTitle).replace("{name}", &self.page_title(id)),
            "body": t(l, S::DeletePageBody).replace("{to}", &to),
            "ok": t(l, S::Delete),
            "cancel": t(l, S::Cancel),
            "danger": true,
        });
        self.open_dialog(Ask::Delete(id), body);
    }

    /// Delete page `id` (shown: its neighbour slides in, with its things on it). One undo
    /// brings it back.
    fn delete_page(&mut self, id: u32) {
        // (Undo returns to the page as it was shown: the deleted page slides back in.)
        if self.pages.before.is_none() {
            self.pages.before = Some(self.layout.clone());
        }
        if id == self.layout.page {
            if let Some(n) = pg::neighbour(&self.layout, id) {
                self.go_page(n);
            }
        }
        log::info!("page {id} deleted");
        self.change_layout(|l, g| {
            pg::delete(l, id, g);
        });
        self.pages.shown = Some(self.layout.page);
    }

    /// Turning pages off with several: asked first (everything goes onto one).
    pub fn merge_pages_ask(&mut self) {
        let l = self.lang;
        let body = serde_json::json!({
            "kind": "confirm",
            "title": t(l, S::MergePagesTitle),
            "body": t(l, S::MergePagesBody).replace("{n}", &pg::count(&self.layout).to_string()),
            "ok": t(l, S::TurnOffPages),
            "cancel": t(l, S::Cancel),
        });
        self.open_dialog(Ask::Merge, body);
    }

    /// A name typed for the page at position `n` (1-based): "Page n" is no name of its own (the
    /// number follows the page when pages move).
    fn own_name(&self, v: &str, n: usize) -> String {
        if v == t(self.lang, S::PageN).replace("{n}", &n.to_string()) {
            String::new()
        } else {
            v.to_string()
        }
    }

    /// A dialog's answer: { dialog, ok, value }.
    pub fn on_dialog(&mut self, v: &serde_json::Value) {
        let id = v["dialog"].as_f64().unwrap_or(-1.0) as u32;
        let Some((want, ask)) = self.pages.dialog.take() else { return };
        if want != id {
            self.pages.dialog = Some((want, ask));
            return;
        }
        let ok = v["ok"].as_bool().unwrap_or(false);
        let value = v["value"].as_str().unwrap_or("").trim().to_string();
        log::info!("dialog {id} {ask:?}: ok={ok}");
        match ask {
            Ask::NewPage { boxed, items, go } if ok && !value.is_empty() => {
                // (Locks may have been turned on while the dialog was open.)
                let boxed = boxed.filter(|b| !self.locks_of(*b).position);
                let items = if self.icons_movable(&items, None) { items } else { vec![] };
                let name = self.own_name(&value, pg::count(&self.layout) + 1);
                let mut made = 0;
                self.change_layout(|l, g| {
                    made = pg::new_page(l, &name, None);
                    if let Some(b) = boxed {
                        pg::move_box_to_page(l, b, made, g);
                    }
                    if !items.is_empty() {
                        pg::move_items_to_page(l, &items, made, g);
                    }
                });
                if !items.is_empty() {
                    self.sel.clear();
                }
                // A new page with pages off (Settings' "New page"): pages come on, or the page
                // shown would be the only one reachable.
                if !self.settings.pages.enabled {
                    log::info!("a page made with pages off: pages on");
                    self.settings.pages.enabled = true;
                    self.save_settings_soon();
                    self.send_settings(false, None);
                }
                if go {
                    self.go_page(made);
                }
            }
            Ask::Rename(p) if ok && !value.is_empty() => {
                let name = self.own_name(&value, pg::index_of(&self.layout, p).map(|i| i + 1).unwrap_or(1));
                self.change_layout(|l, _| {
                    pg::rename(l, p, &name);
                });
                if p == self.layout.page {
                    self.show_page_pill(None, None);
                }
            }
            Ask::Delete(p) if ok => self.delete_page(p),
            Ask::Merge => {
                if ok {
                    self.change_layout(|l, g| {
                        pg::merge_all(l, g);
                    });
                    self.pages.shown = Some(self.layout.page);
                    self.settings.pages.enabled = false;
                    self.pages.pill = None;
                    self.save_settings_soon();
                }
                // (Cancelled: the switch in Settings goes back on.)
                self.send_settings(false, None);
            }
            Ask::ScreenGone(first) => {
                // Its pages are the main screen's now.
                if let (true, Some(p)) = (ok, first) {
                    if self.multi_screen() {
                        self.enter_screen(0);
                    }
                    if pg::index_of(&self.layout, p).is_some() {
                        self.go_page(p);
                    }
                }
                self.next_screen_notice();
            }
            Ask::Import(path) if ok => self.import_all(&path),
            _ => {}
        }
        self.invalidate();
    }

    // ------------------------------------------------------------------ settings

    /// Settings › Desktop pages, for the settings window.
    pub fn pages_json(&self) -> serde_json::Value {
        use serde_json::json;
        let p = &self.settings.pages;
        let counts = |boxes: &[BoxModel], loose: &std::collections::BTreeMap<ItemKey, Cell>| {
            let icons = loose.keys().filter(|k| self.items.contains_key(*k)).count()
                + boxes.iter().filter(|b| !b.is_folder()).flat_map(|b| b.tabs.iter()).map(|t| t.items.iter().filter(|k| self.items.contains_key(*k)).count()).sum::<usize>();
            (boxes.len(), icons)
        };
        let list: Vec<serde_json::Value> = pg::ids(&self.layout)
            .into_iter()
            .map(|id| {
                let (b, i) = if id == self.layout.page {
                    counts(&self.layout.boxes, &self.layout.loose)
                } else {
                    self.layout.parked.get(&id).map(|c| counts(&c.boxes, &c.loose)).unwrap_or((0, 0))
                };
                json!({ "id": id, "name": self.page_title(id), "boxes": b, "icons": i })
            })
            .collect();
        json!({
            "enabled": p.enabled, "altKeys": p.alt_keys, "swipe": p.swipe, "middleDrag": p.middle_drag, "bothDrag": p.both_drag,
            "edgePull": p.edge_pull, "edgeDrag": p.edge_drag, "animate": p.animate, "bounce": p.bounce, "showName": p.show_name,
            "newFiles": p.new_files, "current": self.layout.page, "list": list,
            "screenNotice": p.screen_notice, "screens": self.layout.screens.len().max(self.screens.len()),
        })
    }

    /// A change in Settings › Desktop pages.
    pub fn apply_pages_setting(&mut self, key: &str, val: &serde_json::Value) {
        let b = val.as_bool().unwrap_or(true);
        let id = val.as_f64().map(|x| x as u32);
        let p = &mut self.settings.pages;
        match key {
            "enabled" => {
                if !b && pg::count(&self.layout) > 1 {
                    self.merge_pages_ask();
                    return;
                }
                p.enabled = b;
                if !b {
                    self.pages.pill = None;
                    self.pages.gesture = None;
                    self.pages.edge = None;
                }
            }
            "altKeys" => p.alt_keys = b,
            "swipe" => p.swipe = b,
            "middleDrag" => p.middle_drag = b,
            "bothDrag" => p.both_drag = b,
            "edgePull" => p.edge_pull = b,
            "edgeDrag" => p.edge_drag = b,
            "animate" => p.animate = b,
            "bounce" => p.bounce = b,
            "showName" => p.show_name = b,
            "screenNotice" => p.screen_notice = b,
            "newFiles" => p.new_files = if val.as_str() == Some("first") { "first" } else { "current" }.to_string(),
            "go" => {
                if let Some(id) = id {
                    self.go_page(id);
                }
                return;
            }
            "new" => {
                self.new_page_dialog(None, vec![], true);
                return;
            }
            "rename" => {
                if let Some(id) = id {
                    self.rename_page_dialog(id);
                }
                return;
            }
            "delete" => {
                if let Some(id) = id {
                    self.delete_page_ask(id);
                }
                return;
            }
            "up" | "down" => {
                if let Some(id) = id {
                    let i = pg::index_of(&self.layout, id).unwrap_or(0);
                    let to = if key == "up" { i.saturating_sub(1) } else { i + 1 };
                    self.change_layout(|l, _| {
                        pg::move_page(l, id, to);
                    });
                }
                return;
            }
            _ => return,
        }
        self.save_settings_soon();
        self.send_settings(false, None);
        self.invalidate();
    }

    /// What of the pages the settings window shows (a change resends it).
    pub fn pages_sig(&self) -> String {
        let names: Vec<(u32, String)> = pg::ids(&self.layout).into_iter().map(|id| (id, self.page_title(id))).collect();
        serde_json::to_string(&(names, self.layout.page, &self.settings.pages)).unwrap_or_default()
    }
}
