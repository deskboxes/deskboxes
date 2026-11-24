//! Finding icons. A search field floats at the bottom (or top) of the desktop: always there and
//! quiet (a little see-through) until the pointer comes near or someone types; a click or any
//! typing on the desktop opens it (Ctrl+F too). A box can also be searched on its own from the
//! magnifier in its title. While searching, matches stay bright and everything else fades (or,
//! with "Hide the others", goes, and the matches in each box close up); rolled-up boxes with
//! matches open; the best match is selected, ↑/↓ (or Tab) step through the matches, Enter opens
//! the current one, Esc ends the search.
//!
//! The field opens into a panel (Settings › Search): how many were found and "Hide the others",
//! type filters (chips), the matches best first (a click shows one, a double-click opens it), and
//! "Make a box of these" / "Move to a box…".

use crate::app::{ease_out, header_h, App, PAD};
use crate::textedit::TextEdit;
use deskboxes_core::find::Query;
use deskboxes_core::*;
use deskboxes_i18n::{t, S};
use deskboxes_render::{self as r, rgba, Color, Dc, RectF};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub use deskboxes_core::find::fold;

/// Width change of the pill when it opens or closes, s.
const PILL_ANIM_S: f32 = 0.16;
/// The pill's fade between quiet and full, s.
const PILL_FADE_S: f32 = 0.12;
/// Non-matching icons fading out, s.
const DIM_S: f32 = 0.15;
const PILL_H: f32 = 34.0;
const PILL_IDLE_W: f32 = 210.0;
/// How strongly non-matches show while searching.
const DIM_ALPHA: f32 = 0.22;
/// The quiet pill's opacity.
const IDLE_ALPHA: f32 = 0.62;
/// The panel sliding out of the field (and back), s; "Hide the others" fading, s.
const PANEL_S: f32 = 0.16;
const HIDE_S: f32 = 0.16;
const PANEL_GAP: f32 = 8.0;
const PANEL_PAD: f32 = 8.0;
const PANEL_R: f32 = 12.0;
const HEAD_H: f32 = 32.0;
const CHIPS_H: f32 = 34.0;
const ROW_H: f32 = 40.0;
const FOOT_H: f32 = 42.0;
const MAX_ROWS: usize = 7;

/// The type filters: the sorting rules' category and its name.
const CHIPS: [(&str, S); 9] = [
    ("folders", S::ChipFolders),
    ("documents", S::ChipDocuments),
    ("images", S::ChipPictures),
    ("games", S::ChipGames),
    ("programs", S::ChipApps),
    ("videos", S::ChipVideos),
    ("music", S::ChipMusic),
    ("archives", S::ChipArchives),
    ("weblinks", S::ChipWeb),
];

/// Something in the panel the pointer can be on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PanelCtl {
    Toggle,
    /// A chip: its category (None: All).
    Chip(Option<&'static str>),
    /// A row of the list: a match (or, choosing where to move them, a box), by index.
    Row(usize),
    Make,
    Move,
    Cancel,
}

/// Where the panel's parts are (at rest; drawing slides and fades it).
pub struct Panel {
    pub frame: Rect,
    pub toggle: Option<Rect>,
    pub chips: Vec<(Option<&'static str>, String, Rect)>,
    /// The rows' area, how many rows fit and how many there are.
    pub list: Option<Rect>,
    pub rows: usize,
    pub count: usize,
    pub make: Option<Rect>,
    pub mv: Option<Rect>,
    pub cancel: Option<Rect>,
}

pub struct Search {
    pub ed: TextEdit,
    /// Keys go to the search field.
    pub focused: bool,
    /// None: the whole desktop; Some(box): that box only (its field is in its title).
    pub scope: Option<u32>,
    /// The pointer is on the pill / on its clear button.
    pub hover: bool,
    pub hover_x: bool,
    pub hover_t: Instant,
    /// The pill is open (wide); since when.
    pub open: bool,
    pub open_t: Instant,
    /// Since when non-matches are fading (a query was typed).
    pub dim_t: Option<Instant>,
    pub caret_on: bool,
    pub hi: Option<u16>,
    /// Matches in the order they are stepped through (best first), and the same as a set.
    pub matches: Vec<ItemKey>,
    pub set: HashSet<ItemKey>,
    pub cur: usize,
    /// The searched box's icons (all tabs), cached per box and its item count.
    pub scope_items: std::cell::RefCell<Option<(u32, usize, HashSet<ItemKey>)>>,
    /// The type filter chosen (a chip), and the categories the query's matches fall in, with
    /// how many (before the filter).
    pub filter: Option<&'static str>,
    pub chips: Vec<(&'static str, usize)>,
    /// Counts changes of the matches (boxes closing up their matches redo their order).
    pub gen: u64,
    /// The panel is out; since when (sliding).
    pub panel: bool,
    pub panel_t: Instant,
    /// The list scrolled down by, DIPs.
    pub scroll: f32,
    pub hover_ctl: Option<PanelCtl>,
    /// Choosing the box to move the matches to (the list shows boxes).
    pub picking: bool,
    /// "Hide the others" turned on or off; since when (fading).
    pub hide_t: Instant,
}

impl Default for Search {
    fn default() -> Self {
        let past = Instant::now() - Duration::from_secs(5);
        Search {
            ed: TextEdit::new(""),
            focused: false,
            scope: None,
            scope_items: Default::default(),
            hover: false,
            hover_x: false,
            hover_t: past,
            open: false,
            open_t: past,
            dim_t: None,
            caret_on: true,
            hi: None,
            matches: vec![],
            set: HashSet::new(),
            cur: 0,
            filter: None,
            chips: vec![],
            gen: 0,
            panel: false,
            panel_t: past,
            scroll: 0.0,
            hover_ctl: None,
            picking: false,
            hide_t: past,
        }
    }
}

fn rf(x: Rect) -> RectF {
    RectF {
        left: x.x,
        top: x.y,
        right: x.right(),
        bottom: x.bottom(),
    }
}

/// From `from` to 1 over `s` seconds since `t0` (`on`), or back to 0.
fn eased(on: bool, t0: Instant, s: f32) -> f32 {
    let e = ease_out(t0.elapsed().as_secs_f32() / s);
    if on {
        e
    } else {
        1.0 - e
    }
}

/// Restart a reversible animation from where it is now.
fn flip(t0: &mut Instant, s: f32) {
    let done = (t0.elapsed().as_secs_f32() / s).min(1.0);
    *t0 = Instant::now() - Duration::from_secs_f32((1.0 - done) * s);
    crate::anim::set(true);
}

impl App {
    pub fn search_query(&self) -> String {
        self.search.ed.string()
    }

    /// Something is being looked for: words, a `pdf:`, or a type filter.
    fn searching(&self) -> bool {
        !self.search.ed.text.iter().all(|c| c.is_whitespace()) || self.search.filter.is_some()
    }

    /// The field is at the top of the screen (Settings › Search).
    pub fn search_on_top(&self) -> bool {
        self.settings.search.position == "top"
    }

    fn pill_wide_w(&self) -> f32 {
        (self.work.w * 0.5).clamp(PILL_IDLE_W, 440.0)
    }

    /// The pill, where it is drawn now (its width eases between quiet and open).
    pub fn pill_rect(&self) -> Rect {
        let wide = self.pill_wide_w();
        let t = ease_out(self.search.open_t.elapsed().as_secs_f32() / PILL_ANIM_S);
        let (a, b) = if self.search.open { (PILL_IDLE_W, wide) } else { (wide, PILL_IDLE_W) };
        let w = a + (b - a) * t;
        let y = if self.search_on_top() { self.work.y + 14.0 } else { self.work.bottom() - PILL_H - 16.0 };
        Rect::new((self.work.x + (self.work.w - w) / 2.0).round(), y.round(), w.round(), PILL_H)
    }

    /// How far the pages pill moves down to stay clear of a search field at the top.
    pub fn search_top_room(&self) -> f32 {
        if self.search_on_top() && self.pill_shown() {
            PILL_H + 8.0
        } else {
            0.0
        }
    }

    fn pill_clear_rect(&self) -> Rect {
        let p = self.pill_rect();
        Rect::new(p.right() - 32.0, p.y + 5.0, 26.0, p.h - 10.0)
    }

    /// The magnifier in a box's title (left of +); while the pointer is on it, it opens into a
    /// small field that says "Search" (growing to the left).
    pub fn header_search_rect(&self, b: &BoxModel) -> Rect {
        let a = self.header_layout(b).add;
        let base = a.offset(-a.w - 2.0, 0.0);
        let e = self.search_expand(b.id);
        if e <= 0.001 {
            return base;
        }
        let full = (self.search_btn_text_w() + 34.0).min((b.rect.w * 0.6).max(base.w));
        let w = base.w + (full - base.w) * e;
        Rect::new(base.right() - w, base.y, w, base.h)
    }

    pub fn search_btn_text_w(&self) -> f32 {
        self.gpu
            .label(&self.title_tf, deskboxes_i18n::t(self.lang, deskboxes_i18n::S::Search), 400.0, self.title_line_h, 1, false)
            .map(|l| deskboxes_render::text_width(&l))
            .unwrap_or(44.0)
    }

    /// The pill or its panel takes the pointer here (nothing under it does).
    pub fn on_pill(&self, p: Pt) -> bool {
        self.on_field(p) || self.panel_at(p)
    }

    /// On the pill itself (the field).
    pub fn on_field(&self, p: Pt) -> bool {
        self.pill_shown() && self.pill_rect().inflate(2.0).contains(p)
    }

    /// The pill is on the desktop: always, or (Settings › Search › field off) only while the
    /// desktop is being searched.
    fn pill_shown(&self) -> bool {
        self.settings.show_icons && !self.tucked.0 && (self.settings.search.field || (self.search.scope.is_none() && (self.search.open || self.search.focused)))
    }

    /// The field of a box being searched: its title area.
    pub fn box_search_field(&self) -> Option<(u32, Rect)> {
        let id = self.search.scope?;
        if !(self.search.focused || self.searching()) {
            return None;
        }
        let b = self.box_by_id(id)?;
        let r = self.shown_rect(b);
        Some((id, Rect::new(r.x + 6.0, r.y + 4.0, r.w - 12.0, header_h() - 8.0)))
    }

    fn set_open(&mut self, open: bool) {
        if self.search.open != open {
            // Reverse from where the width is now.
            let done = (self.search.open_t.elapsed().as_secs_f32() / PILL_ANIM_S).min(1.0);
            self.search.open = open;
            self.search.open_t = Instant::now() - Duration::from_secs_f32((1.0 - done) * PILL_ANIM_S);
            crate::anim::set(true);
        }
    }

    /// Start searching (the whole desktop, or one box), with the keyboard in the field.
    pub fn open_search(&mut self, scope: Option<u32>) {
        self.set_tucked(false);
        self.end_rename(true);
        if self.search.scope != scope {
            self.search.ed = TextEdit::new("");
            self.search.scope = scope;
            self.update_search();
        }
        self.search.focused = true;
        self.set_open(scope.is_none());
        unsafe {
            let root = windows::Win32::UI::WindowsAndMessaging::GetAncestor(self.hwnd, GA_ROOT);
            let _ = SetForegroundWindow(root);
            let _ = SetFocus(Some(self.hwnd));
        }
        self.restart_search_caret();
        self.invalidate();
    }

    /// End the search: the query goes, everything shows again.
    pub fn close_search(&mut self) {
        let had = self.searching() || self.search.focused;
        self.search.focused = false;
        self.search.ed = TextEdit::new("");
        self.search.scope = None;
        self.search.filter = None;
        self.search.picking = false;
        self.update_search();
        self.set_open(false);
        if had {
            self.invalidate();
        }
    }

    /// The field lost the keyboard (another window, a click elsewhere): a typed query stays.
    pub fn blur_search(&mut self) {
        if self.search.focused {
            self.search.focused = false;
            if !self.searching() {
                self.close_search();
            }
            self.invalidate();
        }
    }

    fn restart_search_caret(&mut self) {
        self.search.caret_on = true;
        unsafe {
            SetTimer(Some(self.msg_hwnd), crate::app::T_CARET, GetCaretBlinkTime().clamp(250, 1200), None);
        }
    }

    /// The query or the icons changed: find the matches again (best first) and select the first.
    pub fn update_search(&mut self) {
        let q = Query::parse(&self.search_query());
        let filter = self.search.filter;
        let was = !self.search.set.is_empty() || self.search.dim_t.is_some();
        self.search.matches.clear();
        self.search.set.clear();
        self.search.chips.clear();
        self.search.gen = self.search.gen.wrapping_add(1);
        self.search.scroll = 0.0;
        if q.is_empty() && filter.is_none() {
            self.search.dim_t = None;
            self.search.picking = false;
            self.sync_panel();
            if was {
                self.invalidate();
            }
            return;
        }
        if self.search.dim_t.is_none() {
            self.search.dim_t = Some(Instant::now());
            crate::anim::set(true);
        }
        let score = |k: &ItemKey| -> Option<u32> {
            let it = self.items.get(k)?;
            if !q.ext_ok(&it.ext) {
                return None;
            }
            let file = std::path::Path::new(&it.parsing_name).file_name().and_then(|f| f.to_str()).unwrap_or("");
            q.score(&it.display_name).max(q.score(file))
        };
        // In reading order: this page's loose icons, its boxes (open tab first), then the
        // other pages'.
        let mut seen: Vec<ItemKey> = vec![];
        if self.search.scope.is_none() {
            let mut loose: Vec<(&ItemKey, &Cell)> = self.layout.loose.iter().filter(|(k, _)| self.items.contains_key(*k)).collect();
            loose.sort_by_key(|(_, c)| self.grid.index_of(**c));
            seen.extend(loose.into_iter().map(|(k, _)| k.clone()));
        }
        for b in &self.layout.boxes {
            if self.search.scope.is_some_and(|s| s != b.id) {
                continue;
            }
            let mut tabs: Vec<usize> = (0..b.tabs.len()).collect();
            tabs.sort_by_key(|i| *i != b.active_tab);
            for ti in tabs {
                seen.extend(b.tabs[ti].items.iter().filter(|k| self.items.contains_key(*k)).cloned());
            }
        }
        if self.search.scope.is_none() && self.pages_on() {
            for p in deskboxes_core::pages::ids(&self.layout).into_iter().filter(|p| *p != self.layout.page) {
                let Some(c) = self.layout.parked.get(&p) else { continue };
                let mut loose: Vec<(&ItemKey, &Cell)> = c.loose.iter().filter(|(k, _)| self.items.contains_key(*k)).collect();
                loose.sort_by_key(|(_, c)| self.grid.index_of(**c));
                seen.extend(loose.into_iter().map(|(k, _)| k.clone()));
                for b in c.boxes.iter() {
                    for tab in &b.tabs {
                        seen.extend(tab.items.iter().filter(|k| self.items.contains_key(*k)).cloned());
                    }
                }
            }
        }
        let mut found: Vec<(ItemKey, u32)> = seen.into_iter().filter_map(|k| score(&k).map(|s| (k, s))).collect();
        // The type filters that would find something, with how many. Games are not Apps too.
        let is = |k: &ItemKey, cat: &str| self.items.get(k).is_some_and(|it| rules::in_category(it, cat) && (cat != "programs" || !rules::in_category(it, "games")));
        for (cat, _) in CHIPS {
            let n = found.iter().filter(|(k, _)| is(k, cat)).count();
            if n > 0 || filter == Some(cat) {
                self.search.chips.push((cat, n));
            }
        }
        if let Some(cat) = filter {
            found.retain(|(k, _)| is(k, cat));
        }
        // Best first; equals keep their reading order.
        found.sort_by(|a, b| b.1.cmp(&a.1));
        self.search.matches = found.into_iter().map(|(k, _)| k).collect();
        self.search.set = self.search.matches.iter().cloned().collect();
        self.search.cur = 0;
        self.show_current_match();
        self.sync_panel();
        self.invalidate();
    }

    /// Select the current match and bring it into view (its tab opened, its box scrolled).
    fn show_current_match(&mut self) {
        let Some(k) = self.search.matches.get(self.search.cur).cloned() else {
            self.sel.clear();
            return;
        };
        // On another desktop page: that page first.
        if let Some(p) = deskboxes_core::pages::page_of_item(&self.layout, &k).filter(|p| *p != self.layout.page) {
            self.go_page(p);
        }
        self.sel.select_only(&k);
        let found = self.layout.boxes.iter().find_map(|b| b.tabs.iter().position(|t| t.items.contains(&k)).map(|ti| (b.id, ti)));
        if let Some((id, ti)) = found {
            if self.box_by_id(id).map(|b| b.active_tab != ti).unwrap_or(false) {
                self.set_active_tab(id, ti);
            }
            if let Some(b) = self.box_by_id(id) {
                let c = Self::content_rect(b);
                if let Some((_, cell)) = self.box_item_cells(b).into_iter().find(|(x, _)| *x == k) {
                    let s = self.scroll.get(&id).copied().unwrap_or(0.0);
                    let max = self.max_scroll(b);
                    let ns = if cell.y < c.y {
                        s - (c.y - cell.y) - 4.0
                    } else if cell.bottom() > c.bottom() {
                        s + (cell.bottom() - c.bottom()) + 4.0
                    } else {
                        s
                    };
                    self.scroll_to(id, ns.clamp(0.0, max));
                }
            }
        }
    }

    fn step_match(&mut self, back: bool) {
        let n = self.search.matches.len();
        if n == 0 || self.search.picking {
            return;
        }
        self.search.cur = if back { (self.search.cur + n - 1) % n } else { (self.search.cur + 1) % n };
        self.show_current_match();
        self.ensure_row_visible();
        self.invalidate();
    }

    /// While a query is typed, a box with matches stays open (rolled-up ones too).
    pub fn search_holds_open(&self, b: &BoxModel) -> bool {
        if !self.searching() {
            return false;
        }
        match self.search.scope {
            Some(id) => id == b.id,
            None => b.tabs.iter().any(|t| t.items.iter().any(|k| self.search.set.contains(k))),
        }
    }

    /// How strongly an icon shows: non-matches fade while searching.
    pub fn search_alpha(&self, k: &ItemKey) -> f32 {
        let Some(t0) = self.search.dim_t else { return 1.0 };
        if self.search.set.contains(k) {
            return 1.0;
        }
        if let Some(id) = self.search.scope {
            // Searching one box: the rest of the desktop is left alone.
            let inside = match self.box_by_id(id) {
                Some(b) => {
                    let n: usize = b.tabs.iter().map(|t| t.items.len()).sum();
                    let mut c = self.search.scope_items.borrow_mut();
                    if !matches!(&*c, Some((cid, cn, _)) if *cid == id && *cn == n) {
                        *c = Some((id, n, b.tabs.iter().flat_map(|t| t.items.iter().cloned()).collect()));
                    }
                    c.as_ref().map(|(_, _, set)| set.contains(k)).unwrap_or(false)
                }
                None => false,
            };
            if !inside {
                return 1.0;
            }
        }
        let t = ease_out(t0.elapsed().as_secs_f32() / DIM_S);
        // "Hide the others": all the way out.
        let h = self.hide_amount();
        let target = DIM_ALPHA * (1.0 - h);
        1.0 + (target - 1.0) * t
    }

    /// "Hide the others" applies now: offered, on, and something searched for.
    pub fn search_hiding(&self) -> bool {
        self.settings.search.hide_toggle && self.settings.search.hide && self.search.scope.is_none() && self.searching()
    }

    /// How far "Hide the others" has faded the others out (0..1).
    fn hide_amount(&self) -> f32 {
        if !self.settings.search.hide_toggle || self.search.scope.is_some() {
            return 0.0;
        }
        eased(self.settings.search.hide, self.search.hide_t, HIDE_S)
    }

    /// The matches a box shows while "Hide the others" closes them up (None: all its icons).
    pub fn search_shown_filter(&self, b: &BoxModel) -> Option<(u64, &HashSet<ItemKey>)> {
        let _ = b;
        self.search_hiding().then_some((self.search.gen, &self.search.set))
    }

    /// A loose icon hidden by "Hide the others" (not there for the pointer either).
    pub fn search_hides(&self, k: &ItemKey) -> bool {
        self.search_hiding() && !self.search.set.contains(k)
    }

    /// A tab (not open) holding matches gets a dot.
    pub fn tab_has_matches(&self, tab: &Tab) -> bool {
        self.searching() && tab.items.iter().any(|k| self.search.set.contains(k))
    }

    pub fn search_animating(&self) -> bool {
        self.search.panel_t.elapsed().as_secs_f32() < PANEL_S
            || self.search.hide_t.elapsed().as_secs_f32() < HIDE_S
            || self.search.open_t.elapsed().as_secs_f32() < PILL_ANIM_S
            || self.search.hover_t.elapsed().as_secs_f32() < PILL_FADE_S
            || self.search.dim_t.map(|t| t.elapsed().as_secs_f32() < DIM_S).unwrap_or(false)
    }

    /// Pointer moved: the pill wakes up under it (and its clear button).
    pub fn search_hover(&mut self, p: Pt) {
        let ctl = self.panel_ctl_at(p);
        if ctl != self.search.hover_ctl {
            self.search.hover_ctl = ctl;
            self.invalidate();
        }
        let on = self.on_field(p);
        let on_x = on && self.searching() && self.search.scope.is_none() && self.pill_clear_rect().contains(p);
        if on != self.search.hover {
            let done = (self.search.hover_t.elapsed().as_secs_f32() / PILL_FADE_S).min(1.0);
            self.search.hover = on;
            self.search.hover_t = Instant::now() - Duration::from_secs_f32((1.0 - done) * PILL_FADE_S);
            crate::anim::set(true);
            self.invalidate();
        }
        if on_x != self.search.hover_x {
            self.search.hover_x = on_x;
            self.invalidate();
        }
    }

    /// A click: on the pill (open it, or clear it), on a box's magnifier, or elsewhere (the field
    /// lets go of the keyboard; a click on empty desktop ends the search). True if it was ours.
    pub fn search_click(&mut self, p: Pt, hit: &crate::app::Hit) -> bool {
        if self.on_field(p) {
            if self.searching() && self.search.scope.is_none() && self.pill_clear_rect().contains(p) {
                self.close_search();
                self.open_search(None);
            } else {
                self.open_search(None);
                self.search_caret_to(p);
            }
            return true;
        }
        if let Some((id, f)) = self.box_search_field() {
            if f.contains(p) {
                // The × at the field's right end closes it; elsewhere the field takes the keyboard.
                if p.x > f.right() - 26.0 {
                    self.close_search();
                } else {
                    self.open_search(Some(id));
                }
                return true;
            }
        }
        if let crate::app::Hit::Header { id, .. } = hit {
            if let Some(b) = self.box_by_id(*id) {
                if self.header_search_rect(b).inflate(1.0).contains(p) && self.hover_hdr.map(|h| h.0 == *id).unwrap_or(false) {
                    let id = *id;
                    self.open_search(Some(id));
                    return true;
                }
            }
        }
        if self.search.focused || self.searching() {
            match hit {
                crate::app::Hit::Empty => self.close_search(),
                crate::app::Hit::BoxBody { id } if self.search.scope.is_none() || self.search.scope == Some(*id) => self.close_search(),
                _ => self.blur_search(),
            }
        }
        false
    }

    fn search_caret_to(&mut self, p: Pt) {
        if let Some((l, x)) = self.pill_text_layout() {
            let u = r::caret_at(&l, p.x - x);
            let i = self.search.ed.char_pos(u);
            self.search.ed.move_to(i, false);
        }
    }

    /// A key while the field has the keyboard.
    pub fn search_key(&mut self, vk: VIRTUAL_KEY) {
        let (ctrl, shift) = (crate::app::key_down(VK_CONTROL), crate::app::key_down(VK_SHIFT));
        let owner = self.hwnd.0 as isize;
        let ed = &mut self.search.ed;
        let before = ed.string();
        match vk {
            VK_ESCAPE if self.search.picking => {
                self.search.picking = false;
                self.search.scroll = 0.0;
                self.ensure_row_visible();
                return self.invalidate();
            }
            VK_ESCAPE => return self.close_search(),
            VK_RETURN => {
                if let Some(k) = self.search.matches.get(self.search.cur) {
                    if let Some(p) = self.items.get(k).map(|i| i.parsing_name.clone()) {
                        deskboxes_shell::open_default(vec![p], self.hwnd.0 as isize);
                        self.close_search();
                    }
                }
                return;
            }
            VK_DOWN => return self.step_match(false),
            VK_UP => return self.step_match(true),
            VK_TAB => return self.step_match(shift),
            VK_BACK => ed.backspace(ctrl),
            VK_DELETE => ed.delete(ctrl),
            VK_LEFT => ed.left(ctrl, shift),
            VK_RIGHT => ed.right(ctrl, shift),
            VK_HOME => ed.move_to(0, shift),
            VK_END => {
                let n = ed.text.len();
                ed.move_to(n, shift)
            }
            _ if ctrl => match vk.0 as u8 {
                b'A' => ed.select_all(),
                b'C' | b'X' => {
                    let s = ed.selected();
                    if !s.is_empty() {
                        let _ = deskboxes_shell::view::set_clipboard_text(owner, &s);
                        if vk.0 as u8 == b'X' {
                            ed.backspace(false);
                        }
                    }
                }
                b'V' => {
                    if let Some(s) = deskboxes_shell::view::clipboard_text(owner) {
                        ed.insert(s.lines().next().unwrap_or(""));
                    }
                }
                b'F' => {}
                _ => return,
            },
            _ => return, // characters arrive as WM_CHAR
        }
        if self.search.ed.string() != before {
            self.update_search();
        }
        self.restart_search_caret();
        self.invalidate();
    }

    /// A typed character for the field.
    pub fn search_char(&mut self, c: u32) {
        if (0xD800..0xDC00).contains(&c) {
            self.search.hi = Some(c as u16);
            return;
        }
        let s = if (0xDC00..0xE000).contains(&c) {
            self.search.hi.take().map(|h| String::from_utf16_lossy(&[h, c as u16]))
        } else {
            char::from_u32(c).filter(|ch| !ch.is_control()).map(|ch| ch.to_string())
        };
        if let Some(s) = s {
            self.search.ed.insert(&s);
            self.update_search();
            self.restart_search_caret();
            self.invalidate();
        }
    }

    /// Typing on the desktop (nothing being renamed): the search opens with what was typed.
    pub fn type_to_search(&mut self, c: u32) -> bool {
        let printable = char::from_u32(c).map(|ch| !ch.is_control() && !ch.is_whitespace()).unwrap_or(false);
        if !printable || crate::app::key_down(VK_CONTROL) || crate::app::key_down(VK_MENU) || !self.settings.show_icons || !self.settings.search.type_to_search {
            return false;
        }
        self.open_search(None);
        self.search_char(c);
        true
    }

    pub fn search_caret_tick(&mut self) -> bool {
        if self.search.focused {
            self.search.caret_on = !self.search.caret_on;
            self.invalidate();
            return true;
        }
        false
    }

    /// The query (or placeholder) laid out in the pill, and the x it starts at.
    fn pill_text_layout(&self) -> Option<(r::TextLayout, f32)> {
        let p = self.pill_rect();
        let x = p.x + 38.0;
        let text = if self.search.scope.is_none() { self.search_query() } else { String::new() };
        let l = self.gpu.label(&self.search_tf, &text, 10_000.0, 20.0, 1, false).ok()?;
        let room = p.right() - x - 80.0;
        let w = r::text_width(&l);
        let cx = r::caret_x(&l, self.search.ed.utf16_pos(self.search.ed.caret));
        let x = if w > room { x - (cx - room + 4.0).max(0.0) } else { x };
        Some((l, x))
    }

    fn field_colors(&self, light: bool, a: f32) -> (Color, Color, Color) {
        if light {
            (rgba(0xF9, 0xF9, 0xF9, 0.92 * a), rgba(0, 0, 0, 0.9 * a), rgba(0, 0, 0, 0.09 * a))
        } else {
            (rgba(0x2B, 0x2B, 0x2B, 0.88 * a), rgba(255, 255, 255, a), rgba(255, 255, 255, 0.11 * a))
        }
    }

    fn draw_query(&self, dc: &Dc, l: &r::TextLayout, x: f32, y: f32, h: f32, fg: Color, caret: bool) -> r::R<()> {
        let (ar, ag, ab) = crate::app::accent();
        let (a, b) = self.search.ed.sel();
        if a != b {
            let (x0, x1) = (r::caret_x(l, self.search.ed.utf16_pos(a)), r::caret_x(l, self.search.ed.utf16_pos(b)));
            r::fill_rect(dc, rf(Rect::new(x + x0, y, x1 - x0, h)), rgba(ar, ag, ab, 0.6))?;
        }
        r::draw_text(dc, l, x, y, fg)?;
        if caret && self.search.caret_on {
            let cx = x + r::caret_x(l, self.search.ed.utf16_pos(self.search.ed.caret));
            r::fill_rect(dc, rf(Rect::new((cx * self.scale).round() / self.scale, y, 1.0, h)), fg)?;
        }
        Ok(())
    }

    /// The pill at the bottom of the desktop.
    pub fn paint_search(&self, dc: &Dc) -> r::R<()> {
        if !self.pill_shown() {
            return Ok(());
        }
        self.paint_panel(dc)?;
        let p = self.pill_rect();
        let s = &self.search;
        let global = s.scope.is_none();
        let awake = s.hover || (global && (s.focused || self.searching()));
        let ft = ease_out(s.hover_t.elapsed().as_secs_f32() / PILL_FADE_S);
        let a = if awake { IDLE_ALPHA + (1.0 - IDLE_ALPHA) * ft } else { 1.0 + (IDLE_ALPHA - 1.0) * ft };
        let a = if global && (s.focused || self.searching()) { 1.0 } else { a };
        let light = self.light;
        let (bg, fg, edge) = self.field_colors(light, a);
        let rad = p.h / 2.0;
        for (spread, alpha) in [(1.0, 0.10), (3.0, 0.05), (6.0, 0.03)] {
            r::fill_round(dc, rf(p.offset(0.0, spread * 0.5).inflate(spread)), rad + spread, rgba(0, 0, 0, alpha * a))?;
        }
        // Frosted like the boxes when their blurred wallpaper is at hand.
        if let Some(bd) = self.backdrops.get(&(self.settings.default_style.blur as i32)) {
            let k = self.backdrop_down as f32 / self.scale;
            r::push_clip(dc, rf(p));
            r::fill_with_backdrop(dc, bd, k, self.mon_offset(), rf(p), rad)?;
            r::pop_clip(dc);
            r::fill_round(dc, rf(p), rad, Color { a: bg.a * 0.82, ..bg })?;
        } else {
            r::fill_round(dc, rf(p), rad, bg)?;
        }
        r::stroke_round(dc, rf(p.inflate(-0.5)), rad, edge, 1.0)?;
        if global && s.focused {
            let (ar, ag, ab) = crate::app::accent();
            r::push_clip(dc, rf(Rect::new(p.x, p.bottom() - 2.0, p.w, 2.0)));
            r::fill_round(dc, rf(p.inflate(-1.0)), rad - 1.0, rgba(ar, ag, ab, 1.0))?;
            r::pop_clip(dc);
        }
        // Magnifier.
        if let Ok(g) = self.gpu.label(&self.glyph_tf, "\u{E721}", 20.0, 20.0, 1, true) {
            r::draw_text(dc, &g, p.x + 12.0, p.y + (p.h - r::text_height(&g)) / 2.0, Color { a: fg.a * 0.8, ..fg })?;
        }
        r::push_clip(dc, rf(Rect::new(p.x + 34.0, p.y, (p.w - 34.0 - if global && self.searching() { 34.0 } else { 12.0 }).max(0.0), p.h)));
        if global && self.searching() {
            // The query, then how many were found.
            let n = s.matches.len();
            let count = if n == 0 { t(self.lang, S::NoResults).to_string() } else { format!("{} / {}", s.cur + 1, n) };
            let cl = self.gpu.label(&self.tip_tf, &count, 200.0, 20.0, 1, false)?;
            let cw = r::text_width(&cl);
            if let Some((l, x)) = self.pill_text_layout() {
                let h = r::text_height(&l);
                r::push_clip(dc, rf(Rect::new(p.x + 34.0, p.y, (p.right() - 34.0 - cw - 12.0 - (p.x + 34.0)).max(0.0), p.h)));
                self.draw_query(dc, &l, x, p.y + (p.h - h) / 2.0, h, fg, s.focused)?;
                r::pop_clip(dc);
            }
            r::draw_text(dc, &cl, p.right() - 34.0 - cw - 4.0, p.y + (p.h - r::text_height(&cl)) / 2.0, Color { a: fg.a * 0.6, ..fg })?;
        } else {
            let ph = self.gpu.label(&self.search_tf, t(self.lang, S::SearchDesktop), p.w, 20.0, 1, false)?;
            let y = p.y + (p.h - r::text_height(&ph)) / 2.0;
            r::draw_text(dc, &ph, p.x + 38.0, y, Color { a: fg.a * 0.62, ..fg })?;
            if global && s.focused && s.caret_on {
                r::fill_rect(dc, rf(Rect::new(((p.x + 38.0) * self.scale).round() / self.scale, y, 1.0, r::text_height(&ph))), fg)?;
            }
        }
        r::pop_clip(dc);
        if global && self.searching() {
            let c = self.pill_clear_rect();
            if s.hover_x {
                r::fill_round(dc, rf(c), 4.0, if light { rgba(0, 0, 0, 0.06) } else { rgba(255, 255, 255, 0.08) })?;
            }
            if let Ok(g) = self.gpu.label(&self.glyph_tf, "\u{E711}", c.w, c.h, 1, true) {
                r::draw_text(dc, &g, c.x, c.y + (c.h - r::text_height(&g)) / 2.0, Color { a: fg.a * 0.75, ..fg })?;
            }
        }
        Ok(())
    }

    /// A box being searched: the field over its title.
    pub fn paint_box_search(&self, dc: &Dc, b: &BoxModel, light: bool) -> r::R<()> {
        let Some((id, f)) = self.box_search_field() else { return Ok(()) };
        if id != b.id {
            return Ok(());
        }
        let (bg, fg, edge) = self.field_colors(light, 1.0);
        r::fill_round(dc, rf(f), 5.0, bg)?;
        r::stroke_round(dc, rf(f.inflate(-0.5)), 5.0, edge, 1.0)?;
        if self.search.focused {
            let (ar, ag, ab) = crate::app::accent();
            r::fill_round(dc, rf(Rect::new(f.x + 2.0, f.bottom() - 2.0, f.w - 4.0, 2.0)), 1.0, rgba(ar, ag, ab, 1.0))?;
        }
        if let Ok(g) = self.gpu.label(&self.glyph_tf, "\u{E721}", 20.0, f.h, 1, true) {
            r::draw_text(dc, &g, f.x + 4.0, f.y + (f.h - r::text_height(&g)) / 2.0, Color { a: 0.7, ..fg })?;
        }
        let x0 = f.x + 26.0;
        let room = f.right() - 30.0 - x0;
        r::push_clip(dc, rf(Rect::new(x0, f.y, room.max(0.0), f.h)));
        if self.searching() {
            let n = self.search.matches.len();
            let count = if n == 0 { t(self.lang, S::NoResults).to_string() } else { format!("{} / {}", self.search.cur + 1, n) };
            let cl = self.gpu.label(&self.tip_tf, &count, 200.0, 20.0, 1, false)?;
            let cw = r::text_width(&cl);
            let l = self.gpu.label(&self.search_tf, &self.search_query(), 10_000.0, 20.0, 1, false)?;
            let h = r::text_height(&l);
            let tr = room - cw - 8.0;
            let cx = r::caret_x(&l, self.search.ed.utf16_pos(self.search.ed.caret));
            let x = x0 - (cx - tr + 4.0).max(0.0);
            r::push_clip(dc, rf(Rect::new(x0, f.y, tr.max(0.0), f.h)));
            self.draw_query(dc, &l, x, f.y + (f.h - h) / 2.0, h, fg, self.search.focused)?;
            r::pop_clip(dc);
            r::draw_text(dc, &cl, x0 + room - cw, f.y + (f.h - r::text_height(&cl)) / 2.0, Color { a: fg.a * 0.6, ..fg })?;
        } else {
            let ph = self.gpu.label(&self.search_tf, t(self.lang, S::SearchBox), room, 20.0, 1, false)?;
            let y = f.y + (f.h - r::text_height(&ph)) / 2.0;
            r::draw_text(dc, &ph, x0, y, Color { a: fg.a * 0.6, ..fg })?;
            if self.search.focused && self.search.caret_on {
                r::fill_rect(dc, rf(Rect::new((x0 * self.scale).round() / self.scale, y, 1.0, r::text_height(&ph))), fg)?;
            }
        }
        r::pop_clip(dc);
        let c = Rect::new(f.right() - 26.0, f.y, 24.0, f.h);
        if let Ok(g) = self.gpu.label(&self.glyph_tf, "\u{E711}", c.w, c.h, 1, true) {
            r::draw_text(dc, &g, c.x, c.y + (c.h - r::text_height(&g)) / 2.0, Color { a: fg.a * 0.7, ..fg })?;
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------- the panel

impl App {
    /// The panel belongs out: the desktop is being searched for something, and Settings asks
    /// for the list or the "Hide the others" switch.
    fn panel_wanted(&self) -> bool {
        let s = &self.settings.search;
        (s.results || s.hide_toggle) && self.search.scope.is_none() && self.searching() && self.pill_shown()
    }

    /// Slide the panel out or back when that changes.
    pub fn sync_panel(&mut self) {
        let want = self.panel_wanted();
        if want != self.search.panel {
            self.search.panel = want;
            flip(&mut self.search.panel_t, PANEL_S);
            if !want {
                self.search.picking = false;
                self.search.hover_ctl = None;
            }
            self.invalidate();
        }
    }

    /// The matches on this page that can be moved into a box (not ones in folder boxes: those
    /// are files of another folder).
    fn movable_matches(&self) -> Vec<ItemKey> {
        let boxed: HashSet<&ItemKey> = self.layout.boxes.iter().filter(|b| !b.is_folder()).flat_map(|b| b.tabs.iter().flat_map(|t| t.items.iter())).collect();
        self.search.matches.iter().filter(|k| self.layout.loose.contains_key(*k) || boxed.contains(k)).cloned().collect()
    }

    /// The boxes the matches can be moved to.
    fn move_targets(&self) -> Vec<u32> {
        self.layout.boxes.iter().filter(|b| !b.is_folder() && !self.locks(b).icons).map(|b| b.id).collect()
    }

    fn text_w(&self, s: &str) -> f32 {
        self.gpu.label(&self.tip_tf, s, 2000.0, 20.0, 1, false).map(|l| r::text_width(&l)).unwrap_or(s.chars().count() as f32 * 7.0)
    }

    fn chip_text(&self, cat: Option<&'static str>, n: usize) -> String {
        match cat {
            None => t(self.lang, S::ChipAll).to_string(),
            Some(c) => {
                let name = CHIPS.iter().find(|(id, _)| *id == c).map(|(_, s)| t(self.lang, *s)).unwrap_or(c);
                format!("{name}  {n}")
            }
        }
    }

    /// Where the panel's parts are (None: not out).
    pub fn panel_layout(&self) -> Option<Panel> {
        if !self.search.panel && eased(false, self.search.panel_t, PANEL_S) <= 0.01 {
            return None;
        }
        let s = &self.settings.search;
        let top = self.search_on_top();
        let pill = self.pill_rect();
        let w = self.pill_wide_w();
        let x = (self.work.x + (self.work.w - w) / 2.0).round();
        let (ix, iw) = (x + PANEL_PAD, w - 2.0 * PANEL_PAD);
        let picking = self.search.picking;
        let count = if picking { self.move_targets().len() } else { self.search.matches.len() };
        let chips_on = s.results && !picking && !self.search.chips.is_empty();
        let here = s.results && !picking && !self.movable_matches().is_empty();
        let foot = s.results && (picking || here);
        let fixed = PANEL_PAD + HEAD_H + if chips_on { CHIPS_H } else { 0.0 } + if foot { FOOT_H } else { 0.0 } + PANEL_PAD;
        let room = if top { self.work.bottom() - 8.0 - (pill.bottom() + PANEL_GAP) } else { pill.y - PANEL_GAP - (self.work.y + 8.0) };
        let rows = if s.results { count.max(1).min(MAX_ROWS).min(((room - fixed) / ROW_H).floor().max(1.0) as usize) } else { 0 };
        let h = fixed + rows as f32 * ROW_H;
        let frame = Rect::new(x, if top { pill.bottom() + PANEL_GAP } else { pill.y - PANEL_GAP - h }, w, h);
        let mut y = frame.y + PANEL_PAD;
        let toggle = (s.hide_toggle && !picking).then(|| {
            let tw = self.text_w(t(self.lang, S::SearchHideOthers)) + 12.0 + 36.0 + 12.0;
            Rect::new(ix + iw - tw, y + 3.0, tw, HEAD_H - 6.0)
        });
        y += HEAD_H;
        let mut chips = vec![];
        if chips_on {
            let mut cx = ix;
            let all = std::iter::once((None, 0)).chain(self.search.chips.iter().map(|(c, n)| (Some(*c), *n)));
            for (cat, n) in all {
                let text = self.chip_text(cat, n);
                let cw = (self.text_w(&text) + 22.0).ceil();
                if cx + cw > ix + iw {
                    break;
                }
                chips.push((cat, text, Rect::new(cx, y + 4.0, cw, 26.0)));
                cx += cw + 6.0;
            }
            y += CHIPS_H;
        }
        let list = (rows > 0).then(|| Rect::new(ix, y, iw, rows as f32 * ROW_H));
        y += rows as f32 * ROW_H;
        let (mut make, mut mv, mut cancel) = (None, None, None);
        if foot {
            let by = y + 6.0;
            if picking {
                let cw = self.text_w(t(self.lang, S::Cancel)) + 28.0;
                cancel = Some(Rect::new(ix + iw - cw, by, cw, 30.0));
            } else {
                let mw = self.text_w(t(self.lang, S::SearchMakeBox)) + 28.0;
                make = Some(Rect::new(ix, by, mw, 30.0));
                if !self.move_targets().is_empty() {
                    let vw = self.text_w(t(self.lang, S::SearchMoveToBox)) + 28.0;
                    mv = Some(Rect::new(ix + mw + 8.0, by, vw, 30.0));
                }
            }
        }
        Some(Panel { frame, toggle, chips, list, rows, count, make, mv, cancel })
    }

    /// The pointer is on the panel.
    pub fn panel_at(&self, p: Pt) -> bool {
        self.search.panel && self.panel_layout().is_some_and(|pl| pl.frame.contains(p))
    }

    fn panel_ctl_at(&self, p: Pt) -> Option<PanelCtl> {
        if !self.search.panel {
            return None;
        }
        let pl = self.panel_layout()?;
        if !pl.frame.contains(p) {
            return None;
        }
        if pl.toggle.is_some_and(|r| r.contains(p)) {
            return Some(PanelCtl::Toggle);
        }
        if let Some((cat, _, _)) = pl.chips.iter().find(|(_, _, r)| r.contains(p)) {
            return Some(PanelCtl::Chip(*cat));
        }
        if let Some(l) = pl.list.filter(|l| l.contains(p)) {
            let i = ((p.y - l.y + self.search.scroll) / ROW_H).floor() as usize;
            return (i < pl.count).then_some(PanelCtl::Row(i));
        }
        for (r, c) in [(pl.make, PanelCtl::Make), (pl.mv, PanelCtl::Move), (pl.cancel, PanelCtl::Cancel)] {
            if r.is_some_and(|r| r.contains(p)) {
                return Some(c);
            }
        }
        None
    }

    /// A click on the panel (a double-click on a row opens it). True if it was on the panel.
    pub fn search_panel_down(&mut self, p: Pt, dbl: bool) -> bool {
        if !self.panel_at(p) {
            return false;
        }
        self.search.focused = true;
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
        }
        match self.panel_ctl_at(p) {
            Some(PanelCtl::Toggle) => self.set_search_hide(!self.settings.search.hide),
            Some(PanelCtl::Chip(c)) => {
                if self.search.filter != c {
                    self.search.filter = c;
                    self.update_search();
                }
            }
            Some(PanelCtl::Row(i)) if self.search.picking => {
                if let Some(id) = self.move_targets().get(i).copied() {
                    self.move_matches_to(id);
                }
            }
            Some(PanelCtl::Row(i)) => {
                self.search.cur = i;
                self.show_current_match();
                self.ensure_row_visible();
                if dbl {
                    if let Some(path) = self.search.matches.get(i).and_then(|k| self.items.get(k)).map(|it| it.parsing_name.clone()) {
                        deskboxes_shell::open_default(vec![path], self.hwnd.0 as isize);
                        self.close_search();
                    }
                }
            }
            Some(PanelCtl::Make) => self.make_box_of_matches(),
            Some(PanelCtl::Move) => {
                self.search.picking = true;
                self.search.scroll = 0.0;
            }
            Some(PanelCtl::Cancel) => {
                self.search.picking = false;
                self.search.scroll = 0.0;
                self.ensure_row_visible();
            }
            None => {}
        }
        self.restart_search_caret();
        self.invalidate();
        true
    }

    /// The wheel over the panel scrolls its list. True if it was on the panel.
    pub fn search_panel_wheel(&mut self, p: Pt, delta: i16) -> bool {
        if !self.panel_at(p) {
            return false;
        }
        if let Some(pl) = self.panel_layout() {
            let max = (pl.count.saturating_sub(pl.rows)) as f32 * ROW_H;
            self.search.scroll = (self.search.scroll - delta as f32 / 120.0 * ROW_H * 1.5).clamp(0.0, max);
            self.search_hover(p);
            self.invalidate();
        }
        true
    }

    /// The list scrolled so the current match is in it.
    fn ensure_row_visible(&mut self) {
        let Some(pl) = self.panel_layout() else { return };
        if pl.rows == 0 || self.search.picking {
            return;
        }
        let y = self.search.cur as f32 * ROW_H;
        let h = pl.rows as f32 * ROW_H;
        if y < self.search.scroll {
            self.search.scroll = y;
        } else if y + ROW_H > self.search.scroll + h {
            self.search.scroll = y + ROW_H - h;
        }
    }

    /// "Hide the others" on or off (kept for the next search).
    pub fn set_search_hide(&mut self, on: bool) {
        if self.settings.search.hide == on {
            return;
        }
        self.settings.search.hide = on;
        flip(&mut self.search.hide_t, HIDE_S);
        self.search.gen = self.search.gen.wrapping_add(1);
        self.save_settings_soon();
        self.settings_sent = self.settings_sig();
        self.invalidate();
    }

    /// The new box's name: what was searched for (or the type filter).
    fn results_title(&self) -> String {
        let q = self.search_query();
        let q = q.trim();
        if !q.is_empty() {
            return q.to_string();
        }
        self.search
            .filter
            .and_then(|c| CHIPS.iter().find(|(id, _)| *id == c))
            .map(|(_, s)| t(self.lang, *s).to_string())
            .unwrap_or_else(|| t(self.lang, S::NewBoxTitle).to_string())
    }

    /// "Make a box of these": a new box with this page's matches, in a free spot, its name
    /// ready to change.
    fn make_box_of_matches(&mut self) {
        let keys = self.movable_matches();
        if keys.is_empty() || !self.icons_movable(&keys, None) {
            return;
        }
        let n = keys.len();
        let cols = n.clamp(3, 5);
        let rows = n.div_ceil(cols).min(4);
        let (w, h) = (cols as f32 * self.grid.cell_w + 2.0 * PAD, header_h() + rows as f32 * self.grid.cell_h + 8.0);
        let want = Rect::new(self.work.x + (self.work.w - w) / 2.0, self.work.y + (self.work.h - h) / 2.0, w, h);
        let r = deskboxes_core::pages::clear_spot(&self.layout, want, &self.work);
        let title = self.results_title();
        let style = self.settings.default_style.clone();
        let mut id = 0;
        self.change_layout(|l, _| {
            id = layout::create_box(l, r, &title, style);
            layout::move_to_box(l, &keys, id, None);
        });
        log::info!("search: box {id} made of {n} match(es)");
        self.close_search();
        self.sel.set = keys.into_iter().collect();
        self.begin_rename(crate::app::RenameTarget::Box { id, tab: 0 });
    }

    /// "Move to a box…": this page's matches into box `id`.
    fn move_matches_to(&mut self, id: u32) {
        let keys: Vec<ItemKey> = self.movable_matches().into_iter().filter(|k| !self.box_by_id(id).is_some_and(|b| b.tabs.iter().any(|t| t.items.contains(k)))).collect();
        if keys.is_empty() || !self.icons_movable(&keys, Some(id)) {
            self.search.picking = false;
            return;
        }
        log::info!("search: {} match(es) moved to box {id}", keys.len());
        self.change_layout(|l, _| {
            layout::move_to_box(l, &keys, id, None);
        });
        self.close_search();
        self.sel.set = keys.into_iter().collect();
    }

    /// Where a match is: its box (and tab), or the desktop, and its page when it is another.
    fn place_of(&self, k: &ItemKey) -> String {
        let desk = t(self.lang, S::SearchOnDesktop);
        let in_boxes = |boxes: &[BoxModel]| {
            boxes.iter().find_map(|b| {
                b.tabs.iter().find(|t| t.items.contains(k)).map(|tab| if b.tabs.len() > 1 && !tab.title.is_empty() { format!("{} › {}", b.title(), tab.title) } else { b.title().to_string() })
            })
        };
        if let Some(s) = in_boxes(&self.layout.boxes) {
            return s;
        }
        if self.layout.loose.contains_key(k) {
            return desk.to_string();
        }
        for p in deskboxes_core::pages::ids(&self.layout).into_iter().filter(|p| *p != self.layout.page) {
            let Some(c) = self.layout.parked.get(&p) else { continue };
            if let Some(w) = in_boxes(&c.boxes).or_else(|| c.loose.contains_key(k).then(|| desk.to_string())) {
                return format!("{} · {w}", self.page_title(p));
            }
        }
        String::new()
    }

    /// The panel above (or below) the field.
    fn paint_panel(&self, dc: &Dc) -> r::R<()> {
        let a = eased(self.search.panel, self.search.panel_t, PANEL_S);
        if a <= 0.01 {
            return Ok(());
        }
        let Some(pl) = self.panel_layout() else { return Ok(()) };
        // Sliding out of the field.
        let dy = (1.0 - a) * 12.0 * if self.search_on_top() { -1.0 } else { 1.0 };
        let o = |x: Rect| x.offset(0.0, dy);
        let light = self.light;
        let (bg, fg, edge) = self.field_colors(light, a);
        let (ar, ag, ab) = crate::app::accent();
        let accent = |al: f32| rgba(ar, ag, ab, al * a);
        let soft = |al: f32| if light { rgba(0, 0, 0, al * a) } else { rgba(255, 255, 255, al * a) };
        let on_accent = if light { rgba(255, 255, 255, a) } else { rgba(0, 0, 0, a) };
        let f = o(pl.frame);
        for (spread, alpha) in [(1.0, 0.12), (4.0, 0.06), (10.0, 0.04)] {
            r::fill_round(dc, rf(f.offset(0.0, spread * 0.5).inflate(spread)), PANEL_R + spread, rgba(0, 0, 0, alpha * a))?;
        }
        if let Some(bd) = self.backdrops.get(&(self.settings.default_style.blur as i32)) {
            let k = self.backdrop_down as f32 / self.scale;
            r::push_clip(dc, rf(f));
            r::fill_with_backdrop(dc, bd, k, self.mon_offset(), rf(f), PANEL_R)?;
            r::pop_clip(dc);
            r::fill_round(dc, rf(f), PANEL_R, Color { a: bg.a * 0.86, ..bg })?;
        } else {
            r::fill_round(dc, rf(f), PANEL_R, bg)?;
        }
        r::stroke_round(dc, rf(f.inflate(-0.5)), PANEL_R, edge, 1.0)?;
        let hov = self.search.hover_ctl;
        let (ix, iw) = (f.x + PANEL_PAD, f.w - 2.0 * PANEL_PAD);
        let head_y = f.y + PANEL_PAD;
        // How many were found (or what the list is for now).
        let head = if self.search.picking {
            t(self.lang, S::SearchPickBox).replace("{n}", &self.movable_matches().len().to_string())
        } else if pl.count == 0 {
            t(self.lang, S::NoResults).to_string()
        } else {
            t(self.lang, S::SearchFound).replace("{n}", &pl.count.to_string())
        };
        let hl = self.gpu.label(&self.tip_tf_bold, &head, iw * 0.6, 20.0, 1, false)?;
        r::draw_text(dc, &hl, ix + 6.0, head_y + (HEAD_H - r::text_height(&hl)) / 2.0, Color { a: fg.a * 0.85, ..fg })?;
        // "Hide the others": a label and a switch.
        if let Some(tr) = pl.toggle.map(o) {
            if hov == Some(PanelCtl::Toggle) {
                r::fill_round(dc, rf(tr), 6.0, soft(0.06))?;
            }
            let k = eased(self.settings.search.hide, self.search.hide_t, HIDE_S);
            let ll = self.gpu.label(&self.tip_tf, t(self.lang, S::SearchHideOthers), tr.w, 20.0, 1, false)?;
            r::draw_text(dc, &ll, tr.x + 8.0, tr.y + (tr.h - r::text_height(&ll)) / 2.0, fg)?;
            let track = Rect::new(tr.right() - 8.0 - 36.0, tr.y + (tr.h - 18.0) / 2.0, 36.0, 18.0);
            if k > 0.01 {
                r::fill_round(dc, rf(track), 9.0, accent(k))?;
            }
            r::stroke_round(dc, rf(track.inflate(-0.5)), 9.0, Color { a: fg.a * 0.6 * (1.0 - k), ..fg }, 1.0)?;
            let kx = track.x + 9.0 + k * 18.0;
            let knob = Rect::new(kx - 5.0, track.y + 4.0, 10.0, 10.0);
            let kc = if k > 0.5 { on_accent } else { Color { a: fg.a * 0.8, ..fg } };
            r::fill_round(dc, rf(knob), 5.0, kc)?;
        }
        // Type filters.
        for (cat, text, cr) in &pl.chips {
            let cr = o(*cr);
            let sel = self.search.filter == *cat;
            if sel {
                r::fill_round(dc, rf(cr), 13.0, accent(0.95))?;
            } else {
                r::fill_round(dc, rf(cr), 13.0, soft(if hov == Some(PanelCtl::Chip(*cat)) { 0.12 } else { 0.06 }))?;
                r::stroke_round(dc, rf(cr.inflate(-0.5)), 13.0, soft(0.10), 1.0)?;
            }
            let l = self.gpu.label(&self.tip_tf, text, cr.w, 20.0, 1, false)?;
            r::draw_text(dc, &l, cr.x + (cr.w - r::text_width(&l)) / 2.0, cr.y + (cr.h - r::text_height(&l)) / 2.0, if sel { on_accent } else { fg })?;
        }
        // The list.
        if let Some(lr) = pl.list.map(o) {
            r::push_clip(dc, rf(lr));
            let first = (self.search.scroll / ROW_H).floor() as usize;
            let targets = if self.search.picking { self.move_targets() } else { vec![] };
            if pl.count == 0 && !self.search.picking {
                let l = self.gpu.label(&self.tip_tf, t(self.lang, S::NoResults), iw, 20.0, 1, false)?;
                r::draw_text(dc, &l, lr.x + 12.0, lr.y + (ROW_H - r::text_height(&l)) / 2.0, Color { a: fg.a * 0.6, ..fg })?;
            }
            for i in first..pl.count.min(first + pl.rows + 1) {
                let row = Rect::new(lr.x, lr.y + i as f32 * ROW_H - self.search.scroll, lr.w, ROW_H);
                if !self.search.picking && i == self.search.cur {
                    r::fill_round(dc, rf(row.inflate(-1.0)), 6.0, accent(0.22))?;
                    r::fill_round(dc, rf(Rect::new(row.x + 1.0, row.y + 12.0, 3.0, ROW_H - 24.0)), 1.5, accent(1.0))?;
                } else if hov == Some(PanelCtl::Row(i)) {
                    r::fill_round(dc, rf(row.inflate(-1.0)), 6.0, soft(0.07))?;
                }
                let tx = row.x + 44.0;
                if self.search.picking {
                    let Some(b) = targets.get(i).and_then(|id| self.box_by_id(*id)) else { continue };
                    if let Ok(g) = self.gpu.label(&self.glyph_tf, "\u{E8A9}", 24.0, 24.0, 1, true) {
                        r::draw_text(dc, &g, row.x + 12.0, row.y + (ROW_H - r::text_height(&g)) / 2.0, Color { a: fg.a * 0.8, ..fg })?;
                    }
                    let n: usize = b.tabs.iter().map(|t| t.items.len()).sum();
                    let cl = self.gpu.label(&self.tip_tf, &n.to_string(), 80.0, 20.0, 1, false)?;
                    let cw = r::text_width(&cl);
                    r::draw_text(dc, &cl, row.right() - 12.0 - cw, row.y + (ROW_H - r::text_height(&cl)) / 2.0, Color { a: fg.a * 0.55, ..fg })?;
                    let nl = self.gpu.label(&self.search_tf, b.title(), (row.right() - 24.0 - cw - tx).max(20.0), 20.0, 1, false)?;
                    r::draw_text(dc, &nl, tx, row.y + (ROW_H - r::text_height(&nl)) / 2.0, fg)?;
                    continue;
                }
                let Some(k) = self.search.matches.get(i) else { continue };
                let Some(it) = self.items.get(k) else { continue };
                if let Some((_, bmp)) = self.icon_bitmap(it, &self.metrics(None)) {
                    let sz = unsafe { bmp.GetSize() };
                    let sc = 24.0 / sz.width.max(sz.height).max(1.0);
                    let (bw, bh) = (sz.width * sc, sz.height * sc);
                    let d = Rect::new(row.x + 10.0 + (24.0 - bw) / 2.0, row.y + (ROW_H - bh) / 2.0, bw, bh);
                    r::draw_bitmap(dc, bmp, rf(d), a);
                }
                let place = self.place_of(k);
                let pw = if place.is_empty() { 0.0 } else { self.text_w(&place).min(lr.w * 0.42) };
                if pw > 0.0 {
                    let pll = self.gpu.label(&self.tip_tf, &place, pw + 1.0, 20.0, 1, false)?;
                    r::draw_text(dc, &pll, row.right() - 12.0 - r::text_width(&pll), row.y + (ROW_H - r::text_height(&pll)) / 2.0, Color { a: fg.a * 0.55, ..fg })?;
                }
                let nl = self.gpu.label(&self.search_tf, &it.display_name, (row.right() - 24.0 - pw - tx).max(20.0), 20.0, 1, false)?;
                r::draw_text(dc, &nl, tx, row.y + (ROW_H - r::text_height(&nl)) / 2.0, fg)?;
            }
            r::pop_clip(dc);
            // More than fits: a thin bar showing where the list is.
            if pl.count > pl.rows {
                let total = pl.count as f32 * ROW_H;
                let bh = (lr.h * lr.h / total).max(18.0);
                let by = lr.y + (lr.h - bh) * (self.search.scroll / (total - lr.h).max(1.0));
                r::fill_round(dc, rf(Rect::new(lr.right() - 3.0, by, 2.0, bh)), 1.0, soft(0.35))?;
            }
        }
        // Buttons.
        for (br, ctl, s, strong) in [(pl.make, PanelCtl::Make, S::SearchMakeBox, true), (pl.mv, PanelCtl::Move, S::SearchMoveToBox, false), (pl.cancel, PanelCtl::Cancel, S::Cancel, false)] {
            let Some(br) = br.map(o) else { continue };
            let h = hov == Some(ctl);
            if strong {
                r::fill_round(dc, rf(br), 6.0, accent(if h { 0.85 } else { 1.0 }))?;
            } else {
                r::fill_round(dc, rf(br), 6.0, soft(if h { 0.12 } else { 0.07 }))?;
                r::stroke_round(dc, rf(br.inflate(-0.5)), 6.0, soft(0.10), 1.0)?;
            }
            let l = self.gpu.label(&self.tip_tf, t(self.lang, s), br.w, 20.0, 1, false)?;
            r::draw_text(dc, &l, br.x + (br.w - r::text_width(&l)) / 2.0, br.y + (br.h - r::text_height(&l)) / 2.0, if strong { on_accent } else { fg })?;
        }
        Ok(())
    }
}
