//! A–Z boxes: the icons in name order (Explorer's natural order: "File 2" before "File 10").
//! Two quiet helpers, each can be turned off (Settings › A–Z index):
//! - the letter bar: a row of the box's first letters under its title, set like the title (in
//!   the middle, or on the left). The letter you are at is bright with a short accent line under
//!   it that glides when you scroll; the others are dim. Click a letter, or drag along the row,
//!   to jump there.
//! - letter tags: the first icon of each letter has the letter beside it, small and dim, so you
//!   can see where each letter starts while scrolling.
//! The icons keep the box's full width and even grid. Only the display is sorted: the box's own
//! order is kept for when A–Z is turned off.

use crate::app::{accent, ease_out, header_h, App, PAD};
use deskboxes_core::*;
use deskboxes_render::{self as r, rgba, Color, Dc, RectF};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Height of the letter bar.
pub const AZ_BAR_H: f32 = 22.0;
/// Room for one letter (less when there are many; below `DENSE_STEP` every other one is a dot).
const STEP: f32 = 19.0;
const DENSE_STEP: f32 = 11.0;
/// The accent line gliding to another letter, s.
const IND_SLIDE_S: f32 = 0.18;

static BAR: AtomicBool = AtomicBool::new(true);

/// Settings › A–Z index › Letter bar.
pub fn set_bar(on: bool) {
    BAR.store(on, Ordering::Relaxed);
}

/// Room the letter bar takes under a box's title.
pub fn bar_h(b: &BoxModel) -> f32 {
    if b.az && BAR.load(Ordering::Relaxed) {
        AZ_BAR_H
    } else {
        0.0
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

/// The index letter of a name: its first character when that is a letter (without accents:
/// "Ł" → "L"), else "#" (digits and symbols: "2025 plan", "#tags").
pub fn initial(name: &str) -> char {
    let Some(c) = name.chars().next().filter(|c| c.is_alphabetic()) else { return '#' };
    let f = crate::search::fold(&c.to_string());
    f.chars().next().map(|c| c.to_uppercase().next().unwrap_or(c)).filter(|c| c.is_alphabetic()).unwrap_or('#')
}

/// A tab's shown order, cached until its items or any item's name change: an A–Z box sorts
/// hundreds of names, and that order is needed several times per frame and per pointer move.
pub struct Shown {
    /// What it was made for: the icons' generation, A–Z, the search filter's generation, and
    /// the tab's items then (compared key by key: no hashing of every name on each look).
    pub meta: (u64, bool, Option<u64>),
    pub src: std::rc::Rc<Vec<ItemKey>>,
    pub keys: std::rc::Rc<Vec<ItemKey>>,
    /// Each letter and the index of its first icon (A–Z boxes).
    pub firsts: std::rc::Rc<Vec<(char, usize)>>,
}

impl App {
    fn shown(&self, b: &BoxModel, tab: &Tab) -> (std::rc::Rc<Vec<ItemKey>>, std::rc::Rc<Vec<(char, usize)>>) {
        if let Some(m) = self.paint_memo.borrow().as_ref() {
            if let Some(r) = m.get(&tab.id) {
                return r.clone();
            }
        }
        let r = self.shown_checked(b, tab);
        if let Some(m) = self.paint_memo.borrow_mut().as_mut() {
            m.insert(tab.id, r.clone());
        }
        r
    }

    fn shown_checked(&self, b: &BoxModel, tab: &Tab) -> (std::rc::Rc<Vec<ItemKey>>, std::rc::Rc<Vec<(char, usize)>>) {
        // "Hide the others" while searching: only the matches, closed up.
        let only = self.search_shown_filter(b);
        let meta = (self.items_gen, b.az, only.map(|(g, _)| g));
        if let Some(s) = self.shown_cache.borrow().get(&tab.id) {
            // (Same length first; equal keys compare as bytes, far cheaper than hashing them.)
            if s.meta == meta && s.src.len() == tab.items.len() && *s.src == tab.items {
                return (s.keys.clone(), s.firsts.clone());
            }
        }
        let mut v: Vec<ItemKey> = tab.items.iter().filter(|k| self.items.contains_key(*k) && only.is_none_or(|(_, set)| set.contains(*k))).cloned().collect();
        let mut firsts = vec![];
        if b.az {
            // StrCmpLogicalW (Explorer's order) on names converted once, not per comparison.
            let mut w: Vec<(Vec<u16>, ItemKey)> = v
                .into_iter()
                .map(|k| (self.items[&k].display_name.encode_utf16().chain(std::iter::once(0)).collect(), k))
                .collect();
            w.sort_by(|x, y| unsafe { windows::Win32::UI::Shell::StrCmpLogicalW(windows::core::PCWSTR(x.0.as_ptr()), windows::core::PCWSTR(y.0.as_ptr())) }.cmp(&0));
            v = w.into_iter().map(|(_, k)| k).collect();
            for (i, k) in v.iter().enumerate() {
                let c = initial(&self.items[k].display_name);
                if !firsts.iter().any(|l: &(char, usize)| l.0 == c) {
                    firsts.push((c, i));
                }
            }
        }
        let keys = std::rc::Rc::new(v);
        // (An ordinary tab whose icons are all there shows its items as they are: one copy.)
        let src = if *keys == tab.items { keys.clone() } else { std::rc::Rc::new(tab.items.clone()) };
        let s = Shown { meta, src, keys, firsts: std::rc::Rc::new(firsts) };
        let out = (s.keys.clone(), s.firsts.clone());
        self.shown_cache.borrow_mut().insert(tab.id, s);
        out
    }

    /// A tab's icons as shown: present on disk, by name in an A–Z box.
    pub fn shown_items(&self, b: &BoxModel, tab: &Tab) -> std::rc::Rc<Vec<ItemKey>> {
        self.shown(b, tab).0
    }

    /// Each letter of the active tab and the index of its first icon.
    fn az_firsts(&self, b: &BoxModel) -> Vec<(char, usize)> {
        let Some(tab) = b.active() else { return vec![] };
        if !b.az {
            return vec![];
        }
        (*self.shown(b, tab).1).clone()
    }

    /// Indexes of the icons that carry a letter tag (the first of each letter).
    pub fn az_tagged(&self, b: &BoxModel) -> Vec<(usize, char)> {
        if !b.az || !self.settings.az.letter_tags {
            return vec![];
        }
        self.az_firsts(b).into_iter().map(|(c, i)| (i, c)).collect()
    }

    /// The letter bar of an open A–Z box (in the box as it is laid out).
    fn az_bar(&self, b: &BoxModel) -> Option<Rect> {
        (bar_h(b) > 0.0 && self.fully_open(b)).then(|| Rect::new(b.rect.x + PAD, b.rect.y + header_h(), b.rect.w - 2.0 * PAD, AZ_BAR_H))
    }

    /// The bar's letters, the first icon having each, and the room each takes: evenly spaced,
    /// set like the title (in the middle, or level with the icons on the left).
    fn az_letters(&self, b: &BoxModel) -> Vec<(char, usize, Rect)> {
        let Some(bar) = self.az_bar(b) else { return vec![] };
        let letters = self.az_firsts(b);
        if letters.is_empty() {
            return vec![];
        }
        let n = letters.len() as f32;
        let step = (bar.w / n).min(STEP);
        let left = if self.settings.look.align == "left" { bar.x + 2.0 } else { bar.x + (bar.w - step * n) / 2.0 };
        letters.into_iter().enumerate().map(|(i, (c, ix))| (c, ix, Rect::new(left + i as f32 * step, bar.y, step, bar.h))).collect()
    }

    /// The box whose letter bar is under `p`.
    pub fn az_bar_at(&self, p: Pt) -> Option<u32> {
        let b = self.layout.boxes.iter().rev().find(|b| self.shown_rect(b).contains(p))?;
        let bar = self.az_bar(b)?;
        let letters = self.az_letters(b);
        let (first, last) = (letters.first()?.2, letters.last()?.2);
        (bar.contains(p) && p.x >= first.x - 4.0 && p.x <= last.right() + 4.0).then_some(b.id)
    }

    /// Jump to the letter nearest to `x` on the bar (a click, or dragging along it).
    pub fn az_jump(&mut self, id: u32, x: f32) {
        let Some(b) = self.box_by_id(id) else { return };
        let letters = self.az_letters(b);
        let Some((c, ix, _)) = letters.iter().min_by(|a, b| (a.2.x + a.2.w / 2.0 - x).abs().total_cmp(&(b.2.x + b.2.w / 2.0 - x).abs())).copied() else { return };
        let cols = self.box_cols(b);
        let max = self.max_scroll(b);
        let s = ((ix / cols) as f32 * self.metrics(Some(b)).cell_h).clamp(0.0, max);
        self.scroll_to(id, s);
        self.set_az_letter(id, c);
        self.invalidate();
    }

    /// The letter you are at changed: the accent line glides there from where it is drawn now.
    fn set_az_letter(&mut self, id: u32, c: char) {
        if self.az_current(id) == Some(c) {
            return;
        }
        let from = self.box_by_id(id).and_then(|b| self.az_ind_x(b));
        self.az_at = Some((id, c));
        self.az_ind = from.map(|x| (id, x, Instant::now()));
        crate::anim::set(true);
    }

    /// The letter the box is at: the last one jumped or scrolled to, else the one at the top.
    fn az_current(&self, id: u32) -> Option<char> {
        if let Some((b, c)) = self.az_at {
            if b == id {
                return Some(c);
            }
        }
        self.az_top_letter(id)
    }

    fn az_top_letter(&self, id: u32) -> Option<char> {
        let b = self.box_by_id(id)?;
        let tab = b.active()?;
        let s = self.scroll.get(&id).copied().unwrap_or(0.0);
        let ch = self.metrics(Some(b)).cell_h.max(1.0);
        let row = ((s + ch * 0.5) / ch).floor() as usize;
        let items = self.shown_items(b, tab);
        items.get((row * self.box_cols(b)).min(items.len().saturating_sub(1))).map(|k| initial(&self.items[k].display_name))
    }

    /// Where the accent line is drawn (its center), gliding.
    fn az_ind_x(&self, b: &BoxModel) -> Option<f32> {
        let cur = self.az_current(b.id)?;
        let to = self.az_letters(b).iter().find(|l| l.0 == cur).map(|l| l.2.x + l.2.w / 2.0)?;
        Some(match self.az_ind {
            Some((id, from, t0)) if id == b.id => from + (to - from) * ease_out(t0.elapsed().as_secs_f32() / IND_SLIDE_S),
            _ => to,
        })
    }

    /// After a scroll: the letter at the top of the box is the one you are at.
    pub fn az_after_scroll(&mut self, id: u32) {
        if !self.box_by_id(id).map(|b| b.az).unwrap_or(false) {
            return;
        }
        if let Some(c) = self.az_top_letter(id) {
            self.set_az_letter(id, c);
        }
    }

    pub fn az_animating(&self) -> bool {
        self.az_ind.map(|(_, _, t0)| t0.elapsed().as_secs_f32() < IND_SLIDE_S).unwrap_or(false)
    }

    /// The letter bar, `dy` below where the box is laid out (a box rolling open), at `alpha`.
    pub fn paint_az_bar(&self, dc: &Dc, b: &BoxModel, text: Color, dy: f32, alpha: f32) -> r::R<()> {
        let letters = self.az_letters(b);
        if letters.is_empty() {
            return Ok(());
        }
        let hover = self.cursor_dip();
        let hover = (self.shown_rect(b).contains(hover) && self.az_bar_at(hover) == Some(b.id)).then_some(hover.x);
        let cur = self.az_current(b.id);
        let dense = letters[0].2.w < DENSE_STEP;
        let (ar, ag, ab) = accent();
        for (i, (c, _, rct)) in letters.iter().enumerate() {
            let rct = rct.offset(0.0, dy);
            let cx = rct.x + rct.w / 2.0;
            let on = cur == Some(*c);
            let near = hover.map(|x| (x - cx).abs() < rct.w.max(DENSE_STEP) * 0.5).unwrap_or(false);
            // Privacy mode: dots, so the first letters of the names don't show.
            if self.privacy_on() || (dense && i % 2 == 1 && !on && !near) {
                let d = 2.5;
                r::fill_round(dc, rf(Rect::new(cx - d / 2.0, rct.y + rct.h / 2.0 - d / 2.0 - 1.0, d, d)), d / 2.0, Color { a: text.a * 0.4 * alpha, ..text })?;
                continue;
            }
            let a = if on { 1.0 } else if near { 0.85 } else { 0.5 };
            let tf = if on { &self.az_bar_on_tf } else { &self.az_bar_tf };
            if let Ok(l) = self.gpu.label(tf, &c.to_string(), rct.w.max(14.0), rct.h, 1, true) {
                let h = r::text_height(&l);
                let w = rct.w.max(14.0);
                r::draw_text(dc, &l, cx - w / 2.0, rct.y + (rct.h - h) / 2.0 - 1.0, Color { a: text.a * a * alpha, ..text })?;
            }
        }
        // You are here: a short accent line under the letter.
        if let Some(x) = self.az_ind_x(b) {
            let w = 10.0;
            let y = b.rect.y + header_h() + AZ_BAR_H - 3.0 + dy;
            r::fill_round(dc, rf(Rect::new(x - w / 2.0, y, w, 2.0)), 1.0, rgba(ar, ag, ab, alpha))?;
        }
        Ok(())
    }

    /// A letter tag: the letter at the top left of its icon's cell, small and dim.
    pub fn paint_az_tag(&self, dc: &Dc, c: char, cell: Rect, text: Color, shadow: bool, alpha: f32) -> r::R<()> {
        // (Beside the icon; in a row view, at the row's left end.)
        let icon = Rect::new(cell.x + (cell.w - self.icon_dip) / 2.0, cell.y + 7.0, self.icon_dip, self.icon_dip);
        let w = (icon.x - cell.x - 2.0).max(12.0);
        let Ok(l) = self.gpu.label(&self.az_tf, &c.to_string(), w, 14.0, 1, true) else { return Ok(()) };
        let col = Color { a: text.a * 0.62 * alpha, ..text };
        let (x, y) = (cell.x + 1.0, icon.y - 2.0);
        if shadow {
            r::draw_text_shadowed(dc, &l, x, y, col)
        } else {
            r::draw_text(dc, &l, x, y, col)
        }
    }
}
