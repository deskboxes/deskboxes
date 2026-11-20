//! Finding icons. A search pill floats at the bottom of the desktop: always there and quiet (a
//! little see-through) until the pointer comes near or someone types; a click or any typing on
//! the desktop opens it (Ctrl+F too). A box can also be searched on its own from the magnifier
//! in its title. While searching, matches stay bright and everything else fades; rolled-up boxes
//! with matches open; the first match is selected, ↑/↓ (or Tab) step through the matches, Enter
//! opens the current one, Esc ends the search. Nothing about the layout changes.

use crate::app::{ease_out, header_h, App};
use crate::textedit::TextEdit;
use deskboxes_core::*;
use deskboxes_i18n::{t, S};
use deskboxes_render::{self as r, rgba, Color, Dc, RectF};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

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
    /// Matches in the order they are stepped through, and the same as a set.
    pub matches: Vec<ItemKey>,
    pub set: HashSet<ItemKey>,
    pub cur: usize,
    /// The searched box's icons (all tabs), cached per box and its item count.
    pub scope_items: std::cell::RefCell<Option<(u32, usize, HashSet<ItemKey>)>>,
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
        }
    }
}

/// Lowercase without accents, so "zolc" finds "Żółć" and "resume" finds "Résumé".
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'ą' | 'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ā' => 'a',
            'ć' | 'ç' | 'č' => 'c',
            'ę' | 'é' | 'è' | 'ê' | 'ë' | 'ě' | 'ē' => 'e',
            'í' | 'ì' | 'î' | 'ï' | 'ī' => 'i',
            'ł' => 'l',
            'ń' | 'ñ' | 'ň' => 'n',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' | 'ō' => 'o',
            'ś' | 'š' | 'ß' => 's',
            'ú' | 'ù' | 'û' | 'ü' | 'ů' | 'ū' => 'u',
            'ý' | 'ÿ' => 'y',
            'ź' | 'ż' | 'ž' => 'z',
            'ř' => 'r',
            'ď' => 'd',
            'ť' => 't',
            c => c,
        })
        .collect()
}

/// Every word of the query is somewhere in the name.
fn matches(query: &[String], name: &str) -> bool {
    let n = fold(name);
    query.iter().all(|w| n.contains(w.as_str()))
}

fn rf(x: Rect) -> RectF {
    RectF {
        left: x.x,
        top: x.y,
        right: x.right(),
        bottom: x.bottom(),
    }
}

impl App {
    pub fn search_query(&self) -> String {
        self.search.ed.string()
    }

    fn searching(&self) -> bool {
        !self.search.ed.text.iter().all(|c| c.is_whitespace())
    }

    /// The pill, where it is drawn now (its width eases between quiet and open).
    pub fn pill_rect(&self) -> Rect {
        let wide = (self.work.w * 0.5).clamp(PILL_IDLE_W, 440.0);
        let t = ease_out(self.search.open_t.elapsed().as_secs_f32() / PILL_ANIM_S);
        let (a, b) = if self.search.open { (PILL_IDLE_W, wide) } else { (wide, PILL_IDLE_W) };
        let w = a + (b - a) * t;
        Rect::new((self.work.x + (self.work.w - w) / 2.0).round(), (self.work.bottom() - PILL_H - 16.0).round(), w.round(), PILL_H)
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

    /// The pill takes the pointer here (nothing under it does).
    pub fn on_pill(&self, p: Pt) -> bool {
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

    /// The query or the icons changed: find the matches again and select the first.
    pub fn update_search(&mut self) {
        let words: Vec<String> = fold(&self.search_query()).split_whitespace().map(|w| w.to_string()).collect();
        let was = !self.search.set.is_empty() || self.search.dim_t.is_some();
        self.search.matches.clear();
        self.search.set.clear();
        if words.is_empty() {
            self.search.dim_t = None;
            if was {
                self.invalidate();
            }
            return;
        }
        if self.search.dim_t.is_none() {
            self.search.dim_t = Some(Instant::now());
            crate::anim::set(true);
        }
        let name_ok = |k: &ItemKey| {
            self.items
                .get(k)
                .map(|it| {
                    let file = std::path::Path::new(&it.parsing_name).file_name().and_then(|f| f.to_str()).unwrap_or("");
                    matches(&words, &it.display_name) || matches(&words, file)
                })
                .unwrap_or(false)
        };
        let mut out: Vec<ItemKey> = vec![];
        if self.search.scope.is_none() {
            let mut loose: Vec<(&ItemKey, &Cell)> = self.layout.loose.iter().filter(|(k, _)| self.items.contains_key(*k)).collect();
            loose.sort_by_key(|(_, c)| self.grid.index_of(**c));
            out.extend(loose.into_iter().map(|(k, _)| k.clone()).filter(|k| name_ok(k)));
        }
        for b in &self.layout.boxes {
            if self.search.scope.is_some_and(|s| s != b.id) {
                continue;
            }
            // The open tab first, then the others.
            let mut tabs: Vec<usize> = (0..b.tabs.len()).collect();
            tabs.sort_by_key(|i| *i != b.active_tab);
            for ti in tabs {
                out.extend(b.tabs[ti].items.iter().filter(|k| self.items.contains_key(*k) && name_ok(k)).cloned());
            }
        }
        self.search.set = out.iter().cloned().collect();
        self.search.matches = out;
        self.search.cur = 0;
        self.show_current_match();
        self.invalidate();
    }

    /// Select the current match and bring it into view (its tab opened, its box scrolled).
    fn show_current_match(&mut self) {
        let Some(k) = self.search.matches.get(self.search.cur).cloned() else {
            self.sel.clear();
            return;
        };
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
        if n == 0 {
            return;
        }
        self.search.cur = if back { (self.search.cur + n - 1) % n } else { (self.search.cur + 1) % n };
        self.show_current_match();
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
        1.0 + (DIM_ALPHA - 1.0) * t
    }

    /// A tab (not open) holding matches gets a dot.
    pub fn tab_has_matches(&self, tab: &Tab) -> bool {
        self.searching() && tab.items.iter().any(|k| self.search.set.contains(k))
    }

    pub fn search_animating(&self) -> bool {
        self.search.open_t.elapsed().as_secs_f32() < PILL_ANIM_S
            || self.search.hover_t.elapsed().as_secs_f32() < PILL_FADE_S
            || self.search.dim_t.map(|t| t.elapsed().as_secs_f32() < DIM_S).unwrap_or(false)
    }

    /// Pointer moved: the pill wakes up under it (and its clear button).
    pub fn search_hover(&mut self, p: Pt) {
        let on = self.on_pill(p);
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
        if self.on_pill(p) {
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
