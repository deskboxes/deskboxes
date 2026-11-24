//! Several screens (monitors). Each screen has pages of its own (`PageMeta::screen`); a screen
//! shows one of them at a time. The page shown on the screen in use is the layout's own
//! `boxes` / `loose` / `homes`, as for a single screen; what the other screens show waits in
//! `parked` with the other pages, and `shown` says which page each of them shows. So every
//! layout operation (and everything that already looks at every page: rules, search, undo, new
//! icons) works unchanged; using another screen is a page switch (`activate`).
//!
//! A screen that is gone (unplugged) has its pages shown on the main screen meanwhile, refitted
//! to its size, and marked with where they belong (`PageMeta::home`); they go back, refitted
//! again, when it returns (`arrange`).
//!
//! A layout that was only ever shown on one screen has no screen ids at all ("" everywhere),
//! and stays that way until a second screen is seen.

use crate::geom::{Cell, Grid, Rect};
use crate::layout;
use crate::model::*;
use crate::pages;
use std::collections::{BTreeMap, HashSet};

/// The screen page `page` is on ("" = the only screen, or an unknown page).
pub fn screen_of(l: &MonitorLayout, page: u32) -> &str {
    l.pages.iter().find(|p| p.id == page).map(|p| p.screen.as_str()).unwrap_or("")
}

/// The screen in use: the one the page shown is on.
pub fn current(l: &MonitorLayout) -> &str {
    screen_of(l, l.page)
}

/// The pages of screen `screen`, in order.
pub fn pages_of(l: &MonitorLayout, screen: &str) -> Vec<u32> {
    if l.pages.is_empty() {
        return vec![l.page];
    }
    l.pages.iter().filter(|p| p.screen == screen).map(|p| p.id).collect()
}

/// The page screen `screen` shows now.
pub fn shown_on(l: &MonitorLayout, screen: &str) -> Option<u32> {
    if current(l) == screen {
        return Some(l.page);
    }
    l.shown.get(screen).copied().or_else(|| pages_of(l, screen).first().copied())
}

/// Use screen `screen`: the page it shows becomes the layout's own, the page shown so far is
/// noted as what its screen shows. False if the screen has no page.
pub fn activate(l: &mut MonitorLayout, screen: &str) -> bool {
    let cur = current(l).to_string();
    if cur == screen {
        return true;
    }
    let Some(to) = shown_on(l, screen).filter(|p| screen_of(l, *p) == screen) else { return false };
    let was = l.page;
    if !pages::switch_to(l, to) {
        return false;
    }
    l.shown.remove(screen);
    l.shown.insert(cur, was);
    true
}

/// The page shown was switched to a page of another screen (`was_screen` showed `was_page`
/// until then): note that, so each screen still knows its page.
pub fn note_switch(l: &mut MonitorLayout, was_screen: &str, was_page: u32) {
    let now = current(l).to_string();
    if now == was_screen {
        return;
    }
    l.shown.remove(&now);
    if screen_of(l, was_page) == was_screen && was_page != l.page {
        l.shown.insert(was_screen.to_string(), was_page);
    }
}

/// Take icons off every page not shown (they are being moved onto the page shown, from another
/// screen's).
pub fn pull_from_parked(l: &mut MonitorLayout, keys: &[ItemKey]) {
    let set: HashSet<&ItemKey> = keys.iter().collect();
    for c in l.parked.values_mut() {
        c.loose.retain(|k, _| !set.contains(k));
        c.homes.retain(|k, _| !set.contains(k));
        for b in c.boxes.iter_mut().filter(|b| !b.is_folder()) {
            for t in &mut b.tabs {
                t.items.retain(|k| !set.contains(k));
            }
        }
    }
}

/// A screen there now: its id, how it is, and its icon grid.
pub struct Present {
    pub id: String,
    pub info: ScreenInfo,
    pub grid: Grid,
}

/// What `arrange` moved: pages of screens that are gone, now on the main screen, and pages that
/// went back to their returned screen (screen id, page ids).
#[derive(Debug, Default, PartialEq)]
pub struct Arranged {
    pub gone: Vec<(String, Vec<u32>)>,
    pub back: Vec<(String, Vec<u32>)>,
}

/// Fit the layout to the screens there now (`present`, the main screen first): pages of a gone
/// screen go to the main screen (refitted), pages whose screen returned go back to it, a screen
/// seen for the first time gets an empty page, and every screen shows one of its pages.
pub fn arrange(l: &mut MonitorLayout, present: &[Present]) -> Arranged {
    let mut out = Arranged::default();
    let Some(main) = present.first() else { return out };
    let here: HashSet<&str> = present.iter().map(|p| p.id.as_str()).collect();
    let info = |l: &MonitorLayout, s: &str| -> ScreenInfo {
        present.iter().find(|p| p.id == s).map(|p| p.info.clone()).or_else(|| l.screens.get(s).cloned()).unwrap_or_else(|| main.info.clone())
    };
    let untagged = l.pages.iter().all(|p| p.screen.is_empty() && p.home.is_empty());
    if untagged && present.len() == 1 {
        // One screen, never more: no ids (as before), and this screen owns the pages. A new
        // size (another resolution or scale, another monitor): every page refitted.
        if let Some(old) = l.screens.values().next().cloned().filter(|o| !same_size(o, &main.info)) {
            for id in pages::ids(l) {
                refit_page(l, id, &old, &main.info, &main.grid);
            }
        }
        l.screens.clear();
        l.screens.insert(main.id.clone(), main.info.clone());
        l.shown.clear();
        return out;
    }
    // Pages without a screen belong to the one screen they were shown on (the main one if that
    // is not known).
    let owner = if l.screens.len() == 1 { l.screens.keys().next().cloned().unwrap_or_default() } else { main.id.clone() };
    if l.pages.is_empty() {
        l.pages.push(PageMeta { id: l.page, ..Default::default() });
    }
    for p in l.pages.iter_mut().filter(|p| p.screen.is_empty()) {
        p.screen = owner.clone();
    }
    // A screen there now at another size than last time: its pages refitted.
    for p in present {
        if let Some(old) = l.screens.get(&p.id).cloned().filter(|o| !same_size(o, &p.info)) {
            let ids: Vec<u32> = l.pages.iter().filter(|x| x.screen == p.id).map(|x| x.id).collect();
            for id in ids {
                refit_page(l, id, &old, &p.info, &p.grid);
            }
        }
    }
    // Back to a returned screen.
    let mut back: BTreeMap<String, Vec<(u32, ScreenInfo)>> = BTreeMap::new();
    for p in l.pages.iter().filter(|p| !p.home.is_empty() && here.contains(p.home.as_str())) {
        back.entry(p.home.clone()).or_default().push((p.id, info(l, &p.screen)));
    }
    for (home, pages) in back {
        let Some(to) = present.iter().find(|s| s.id == home) else { continue };
        for (id, from) in &pages {
            refit_page(l, *id, from, &to.info, &to.grid);
            if let Some(p) = l.pages.iter_mut().find(|p| p.id == *id) {
                p.screen = home.clone();
                p.home.clear();
            }
        }
        out.back.push((home, pages.into_iter().map(|(id, _)| id).collect()));
    }
    // Pages on a screen that is gone: onto the main screen.
    let mut gone: BTreeMap<String, Vec<(u32, ScreenInfo)>> = BTreeMap::new();
    for p in l.pages.iter().filter(|p| !here.contains(p.screen.as_str())) {
        gone.entry(p.screen.clone()).or_default().push((p.id, info(l, &p.screen)));
    }
    for (screen, pages) in gone {
        let mut newly = vec![];
        for (id, from) in &pages {
            refit_page(l, *id, from, &main.info, &main.grid);
            if let Some(p) = l.pages.iter_mut().find(|p| p.id == *id) {
                if p.home.is_empty() {
                    p.home = screen.clone();
                    newly.push(*id);
                }
                p.screen = main.id.clone();
            }
        }
        // (Pages already away from a screen that was gone before: not news.)
        if !newly.is_empty() {
            out.gone.push((screen, newly));
        }
    }
    // A main screen's own pages come before those visiting it.
    let mut ps = std::mem::take(&mut l.pages);
    ps.sort_by_key(|p| !p.home.is_empty());
    l.pages = ps;
    // Every screen there has a page.
    for s in present {
        if !l.pages.iter().any(|p| p.screen == s.id) {
            let id = layout::alloc_id(l);
            l.pages.push(PageMeta { id, screen: s.id.clone(), ..Default::default() });
            l.parked.insert(id, PageContent::default());
        }
        l.screens.insert(s.id.clone(), s.info.clone());
    }
    // Each screen shows one of its own pages; the screen in use is the one of the page shown,
    // which (for the main screen) is its own page shown before rather than a visiting one.
    let active = current(l).to_string();
    if let Some(p) = l.shown.remove(&active) {
        if p != l.page && screen_of(l, p) == active {
            pages::switch_to(l, p);
        }
    }
    let active = current(l).to_string();
    let valid: Vec<(String, u32)> = l.shown.iter().filter(|(s, p)| **s != active && here.contains(s.as_str()) && screen_of(l, **p) == s.as_str()).map(|(s, p)| (s.clone(), *p)).collect();
    l.shown = valid.into_iter().collect();
    for s in present {
        if s.id != active && !l.shown.contains_key(&s.id) {
            if let Some(first) = pages_of(l, &s.id).first().copied() {
                l.shown.insert(s.id.clone(), first);
            }
        }
    }
    out
}

/// Refit page `id`'s boxes and loose icons from a screen like `from` to one like `to` (grid
/// `g`): each box keeps its size (as far as it fits) and its place across the free room, so one
/// at an edge stays there; icons keep their place in proportion, the nearest free cell if taken.
fn refit_page(l: &mut MonitorLayout, id: u32, from: &ScreenInfo, to: &ScreenInfo, g: &Grid) {
    if from == to {
        return;
    }
    let (fw, tw) = (work_of(from), work_of(to));
    pages::with_page(l, id, |l| {
        for b in &mut l.boxes {
            let frac = |pos: f32, start: f32, room: f32, size: f32| if room > size + 0.5 { ((pos - start) / (room - size)).clamp(0.0, 1.0) } else { 0.0 };
            let fx = frac(b.rect.x, fw.x, fw.w, b.rect.w);
            let fy = frac(b.rect.y, fw.y, fw.h, b.rect.h);
            let (w, h) = (b.rect.w.min(tw.w), b.rect.h.min(tw.h));
            b.rect = Rect::new(tw.x + fx * (tw.w - w), tw.y + fy * (tw.h - h), w, h);
        }
        let scale = |v: i32, n: i32, m: i32| if n > 1 && m > 1 { ((v as f32) * (m - 1) as f32 / (n - 1) as f32).round() as i32 } else { v };
        let moves: Vec<(ItemKey, Cell)> = l.loose.iter().map(|(k, c)| (k.clone(), Cell { col: scale(c.col, from.cols, to.cols), row: scale(c.row, from.rows, to.rows) })).collect();
        l.homes.clear();
        if !moves.is_empty() {
            layout::move_to_grid_at(l, &moves, g);
        }
        layout::settle(l, g);
    });
}

/// The same size and grid (the name and number don't matter here).
fn same_size(a: &ScreenInfo, b: &ScreenInfo) -> bool {
    (a.w - b.w).abs() < 0.5 && (a.h - b.h).abs() < 0.5 && a.work == b.work && a.cols == b.cols && a.rows == b.rows
}

fn work_of(s: &ScreenInfo) -> Rect {
    if s.work.w > 0.0 && s.work.h > 0.0 {
        s.work
    } else {
        Rect::new(0.0, 0.0, s.w.max(1.0), s.h.max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{create_box, move_to_box, placed_keys, reconcile};

    fn keys(n: usize) -> Vec<ItemKey> {
        (0..n).map(|i| ItemKey(format!("s:{i}"))).collect()
    }
    fn screen(id: &str, w: f32, h: f32, n: u32) -> Present {
        let work = Rect::new(0.0, 0.0, w, h - 48.0);
        let grid = Grid::new(work, 75.0, 100.0);
        Present { id: id.into(), info: ScreenInfo { w, h, work, cols: grid.cols, rows: grid.rows, name: String::new(), n }, grid }
    }
    fn all_placed(l: &MonitorLayout) -> usize {
        let mut s: HashSet<ItemKey> = placed_keys(l).into_iter().collect();
        s.extend(pages::parked_keys(l));
        s.len()
    }

    #[test]
    fn one_screen_stays_untagged() {
        let a = screen("A", 1920.0, 1080.0, 1);
        let mut l = MonitorLayout::default();
        reconcile(&mut l, &keys(3), &a.grid);
        let before = (l.pages.clone(), l.page, l.boxes.clone(), l.loose.clone());
        assert_eq!(arrange(&mut l, std::slice::from_ref(&a)), Arranged::default());
        assert_eq!((l.pages.clone(), l.page, l.boxes.clone(), l.loose.clone()), before);
        assert_eq!(l.screens.keys().collect::<Vec<_>>(), vec!["A"]);
        assert_eq!(pages::count(&l), 1);
    }

    #[test]
    fn second_screen_gets_its_own_pages_and_switching_keeps_each() {
        let (a, b) = (screen("A", 1920.0, 1080.0, 1), screen("B", 1280.0, 1024.0, 2));
        let k = keys(6);
        let mut l = MonitorLayout::default();
        reconcile(&mut l, &k, &a.grid);
        arrange(&mut l, std::slice::from_ref(&a));
        let first = l.page;
        // The second screen appears: the old page is A's, B gets a new empty one.
        let out = arrange(&mut l, &[a, b]);
        assert!(out.gone.is_empty() && out.back.is_empty());
        assert_eq!(current(&l), "A");
        assert_eq!(l.page, first);
        assert_eq!(pages::ids(&l), vec![first]);
        let b_page = shown_on(&l, "B").unwrap();
        assert_ne!(b_page, first);
        assert_eq!(pages_of(&l, "B"), vec![b_page]);
        // Use B: its page is the layout's own; A's waits and is still what A shows.
        assert!(activate(&mut l, "B"));
        assert_eq!(l.page, b_page);
        assert!(l.loose.is_empty());
        assert_eq!(shown_on(&l, "A"), Some(first));
        // A new page on B is B's; A's pages are not B's.
        let b2 = pages::new_page(&mut l, "", None);
        assert_eq!(pages::ids(&l), vec![b_page, b2]);
        assert_eq!(screen_of(&l, b2), "B");
        // Icons moved from A to B: off A's page, onto B's.
        pull_from_parked(&mut l, &k[..2]);
        layout::move_to_grid(&mut l, &k[..2], Cell { col: 0, row: 0 }, &screen("B", 1280.0, 1024.0, 2).grid);
        assert_eq!(l.loose.len(), 2);
        assert_eq!(all_placed(&l), 6);
        assert!(activate(&mut l, "A"));
        assert_eq!(l.page, first);
        assert_eq!(l.loose.len(), 4);
        assert_eq!(shown_on(&l, "B"), Some(b_page));
        assert!(!reconcile(&mut l, &k, &screen("A", 1920.0, 1080.0, 1).grid));
    }

    #[test]
    fn a_gone_screen_visits_the_main_one_and_goes_back() {
        let (a, b) = (screen("A", 1920.0, 1080.0, 1), screen("B", 2560.0, 1440.0, 2));
        let k = keys(5);
        let mut l = MonitorLayout::default();
        reconcile(&mut l, &k, &a.grid);
        arrange(&mut l, &[screen("A", 1920.0, 1080.0, 1), screen("B", 2560.0, 1440.0, 2)]);
        let a_page = l.page;
        activate(&mut l, "B");
        let b_page = l.page;
        // A box at B's top right corner, and an icon at the bottom right.
        let bx = create_box(&mut l, Rect::new(2560.0 - 300.0, 0.0, 300.0, 400.0), "Games", BoxStyle::default());
        pull_from_parked(&mut l, &k[..1]);
        move_to_box(&mut l, &k[..1], bx, None);
        pull_from_parked(&mut l, &k[1..2]);
        layout::move_to_grid_at(&mut l, &[(k[1].clone(), Cell { col: b.grid.cols - 1, row: b.grid.rows - 1 })], &b.grid);
        activate(&mut l, "A");
        assert_eq!(all_placed(&l), 5);
        // B is unplugged: its page visits A, after A's own, and A still shows its own page.
        let out = arrange(&mut l, std::slice::from_ref(&a));
        assert_eq!(out.gone, vec![("B".to_string(), vec![b_page])]);
        assert_eq!(l.page, a_page);
        assert_eq!(pages::ids(&l), vec![a_page, b_page]);
        assert_eq!(l.pages.iter().find(|p| p.id == b_page).unwrap().home, "B");
        assert!(l.shown.is_empty());
        pages::switch_to(&mut l, b_page);
        let r = l.boxes[0].rect;
        // Still at the right edge, all of it on A.
        assert!((r.right() - 1920.0).abs() < 0.5 && r.y == 0.0 && r.w == 300.0, "{r:?}");
        assert_eq!(*l.loose.values().next().unwrap(), Cell { col: a.grid.cols - 1, row: a.grid.rows - 1 });
        pages::switch_to(&mut l, a_page);
        // Started again with B still gone: nothing new to tell.
        assert_eq!(arrange(&mut l, std::slice::from_ref(&a)), Arranged::default());
        // B is back: its page goes back, refitted to B again.
        let out = arrange(&mut l, &[screen("A", 1920.0, 1080.0, 1), screen("B", 2560.0, 1440.0, 2)]);
        assert_eq!(out.back, vec![("B".to_string(), vec![b_page])]);
        assert_eq!(pages::ids(&l), vec![a_page]);
        assert_eq!(shown_on(&l, "B"), Some(b_page));
        activate(&mut l, "B");
        assert!((l.boxes[0].rect.right() - 2560.0).abs() < 0.5);
        assert_eq!(*l.loose.values().next().unwrap(), Cell { col: b.grid.cols - 1, row: b.grid.rows - 1 });
        assert_eq!(all_placed(&l), 5);
    }

    #[test]
    fn the_page_in_use_going_away_shows_the_main_screens_own() {
        let (a, b) = (screen("A", 1920.0, 1080.0, 1), screen("B", 1920.0, 1080.0, 2));
        let mut l = MonitorLayout::default();
        reconcile(&mut l, &keys(2), &a.grid);
        arrange(&mut l, &[screen("A", 1920.0, 1080.0, 1), b]);
        let a_page = l.page;
        activate(&mut l, "B");
        let b_page = l.page;
        // Saved while B was in use; B is gone at the next start.
        let out = arrange(&mut l, &[a]);
        assert_eq!(out.gone.len(), 1);
        assert_eq!(l.page, a_page);
        assert_eq!(current(&l), "A");
        assert!(pages::ids(&l).contains(&b_page));
    }

    #[test]
    fn another_resolution_refits_the_boxes() {
        let a = screen("A", 2560.0, 1440.0, 1);
        let mut l = MonitorLayout::default();
        arrange(&mut l, std::slice::from_ref(&a));
        create_box(&mut l, Rect::new(2560.0 - 400.0, 0.0, 400.0, 300.0), "Right", BoxStyle::default());
        // The same monitor at a lower resolution: the box stays at the right edge.
        let small = screen("A", 1920.0, 1080.0, 1);
        arrange(&mut l, std::slice::from_ref(&small));
        assert!((l.boxes[0].rect.right() - 1920.0).abs() < 0.5, "{:?}", l.boxes[0].rect);
        assert_eq!(l.screens["A"].w, 1920.0);
    }

    #[test]
    fn note_switch_keeps_what_each_screen_shows() {
        let (a, b) = (screen("A", 1920.0, 1080.0, 1), screen("B", 1920.0, 1080.0, 2));
        let mut l = MonitorLayout::default();
        arrange(&mut l, &[a, b]);
        let a_page = l.page;
        activate(&mut l, "B");
        let b2 = pages::new_page(&mut l, "", None);
        activate(&mut l, "A");
        // A search jumps straight to B's other page.
        pages::switch_to(&mut l, b2);
        note_switch(&mut l, "A", a_page);
        assert_eq!(current(&l), "B");
        assert_eq!(shown_on(&l, "A"), Some(a_page));
        assert!(!l.shown.contains_key("B"));
    }
}
