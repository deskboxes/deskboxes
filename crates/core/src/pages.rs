//! Desktop pages: more desktops side by side, as a phone's home screens, each with its own boxes
//! and loose icons. The files all stay in the Desktop folders: a page only says where their icons
//! are shown.
//!
//! The page shown is the layout's own `boxes` / `loose` / `homes`; the other pages wait in
//! `parked` and are swapped in on a switch, so every layout operation works on the page shown,
//! unchanged. What has to see every page does so here (items placed on another page count as
//! placed, a rule's box may be on another page, ...).

use crate::geom::{Cell, Grid, Rect};
use crate::layout;
use crate::model::*;
use std::collections::HashSet;

/// The pages' ids, in order (one unnamed page until a second one is made).
pub fn ids(l: &MonitorLayout) -> Vec<u32> {
    if l.pages.is_empty() {
        vec![l.page]
    } else {
        l.pages.iter().map(|p| p.id).collect()
    }
}

pub fn count(l: &MonitorLayout) -> usize {
    l.pages.len().max(1)
}

pub fn index_of(l: &MonitorLayout, id: u32) -> Option<usize> {
    if l.pages.is_empty() {
        return (id == l.page).then_some(0);
    }
    l.pages.iter().position(|p| p.id == id)
}

/// The page shown: its position (0-based).
pub fn current_index(l: &MonitorLayout) -> usize {
    index_of(l, l.page).unwrap_or(0)
}

/// The id of the page at `index`.
pub fn id_at(l: &MonitorLayout, index: usize) -> Option<u32> {
    ids(l).get(index).copied()
}

/// A page's own name ("" = none given: the caller shows "Page N").
pub fn name(l: &MonitorLayout, id: u32) -> &str {
    l.pages.iter().find(|p| p.id == id).map(|p| p.name.as_str()).unwrap_or("")
}

/// The single page gets an entry of its own (before a second one is made, or it is named).
fn materialize(l: &mut MonitorLayout) {
    if l.pages.is_empty() {
        l.pages.push(PageMeta { id: l.page, name: String::new() });
    }
}

/// Show page `id`: its boxes and icons become the layout's own, the page shown so far waits.
pub fn switch_to(l: &mut MonitorLayout, id: u32) -> bool {
    if id == l.page {
        return true;
    }
    let Some(mut next) = l.parked.remove(&id) else { return false };
    std::mem::swap(&mut next.boxes, &mut l.boxes);
    std::mem::swap(&mut next.loose, &mut l.loose);
    std::mem::swap(&mut next.homes, &mut l.homes);
    l.parked.insert(l.page, next);
    l.page = id;
    true
}

/// Run `f` on page `id` as if it were shown, then show the page shown before again.
pub fn with_page<R>(l: &mut MonitorLayout, id: u32, f: impl FnOnce(&mut MonitorLayout) -> R) -> Option<R> {
    let back = l.page;
    if !switch_to(l, id) {
        return None;
    }
    let r = f(l);
    switch_to(l, back);
    Some(r)
}

/// A new, empty page at position `at` (None: last). Returns its id.
pub fn new_page(l: &mut MonitorLayout, name: &str, at: Option<usize>) -> u32 {
    materialize(l);
    let id = layout::alloc_id(l);
    let at = at.unwrap_or(l.pages.len()).min(l.pages.len());
    l.pages.insert(at, PageMeta { id, name: name.trim().to_string() });
    l.parked.insert(id, PageContent::default());
    id
}

pub fn rename(l: &mut MonitorLayout, id: u32, name: &str) -> bool {
    materialize(l);
    match l.pages.iter_mut().find(|p| p.id == id) {
        Some(p) => {
            p.name = name.trim().to_string();
            true
        }
        None => false,
    }
}

/// Move page `id` to position `to`.
pub fn move_page(l: &mut MonitorLayout, id: u32, to: usize) -> bool {
    let Some(from) = l.pages.iter().position(|p| p.id == id) else { return false };
    let to = to.min(l.pages.len() - 1);
    if from == to {
        return false;
    }
    let p = l.pages.remove(from);
    l.pages.insert(to, p);
    true
}

/// The page a deleted page's boxes and icons go to: the one on its left, or on its right if it
/// was the first.
pub fn neighbour(l: &MonitorLayout, id: u32) -> Option<u32> {
    let ids = ids(l);
    let i = ids.iter().position(|p| *p == id)?;
    if i > 0 {
        Some(ids[i - 1])
    } else {
        ids.get(1).copied()
    }
}

/// Delete page `id`: its boxes and icons go to its neighbour (in free spots there). The last page
/// can't be deleted. Returns the neighbour.
pub fn delete(l: &mut MonitorLayout, id: u32, g: &Grid) -> Option<u32> {
    if count(l) < 2 {
        return None;
    }
    let to = neighbour(l, id)?;
    if l.page == id {
        switch_to(l, to);
    }
    let content = l.parked.remove(&id)?;
    l.pages.retain(|p| p.id != id);
    with_page(l, to, |l| absorb(l, content, g));
    Some(to)
}

/// Everything onto one page (the first): the others' boxes and icons go there, in free spots.
pub fn merge_all(l: &mut MonitorLayout, g: &Grid) -> bool {
    if count(l) < 2 {
        return false;
    }
    let first = ids(l)[0];
    switch_to(l, first);
    let others: Vec<u32> = ids(l).into_iter().skip(1).collect();
    for id in others {
        if let Some(c) = l.parked.remove(&id) {
            absorb(l, c, g);
        }
        l.pages.retain(|p| p.id != id);
    }
    true
}

/// A page's boxes and icons put onto the page shown: boxes where they were, or the nearest spot
/// clear of the boxes there; icons in their cells, or the nearest free ones.
fn absorb(l: &mut MonitorLayout, c: PageContent, g: &Grid) {
    let bounds = Rect::new(g.origin.x, g.origin.y, g.cols as f32 * g.cell_w, g.rows as f32 * g.cell_h);
    for mut b in c.boxes {
        b.rect = clear_spot(l, b.rect, &bounds);
        l.boxes.push(b);
    }
    let moves: Vec<(ItemKey, Cell)> = c.loose.into_iter().collect();
    if !moves.is_empty() {
        layout::move_to_grid_at(l, &moves, g);
    }
    layout::settle(l, g);
}

/// The spot nearest `want` (same size) inside `bounds` that no box on the page shown covers;
/// `want` itself if none is free.
pub fn clear_spot(l: &MonitorLayout, want: Rect, bounds: &Rect) -> Rect {
    let free = |r: &Rect| !l.boxes.iter().any(|b| b.rect.inflate(8.0).intersects(r));
    let want = want.clamp_into(bounds);
    if free(&want) {
        return want;
    }
    let step = 16.0;
    let mut best: Option<(f32, Rect)> = None;
    let mut y = bounds.y;
    while y + want.h <= bounds.bottom() + 0.5 {
        let mut x = bounds.x;
        while x + want.w <= bounds.right() + 0.5 {
            let r = Rect::new(x, y, want.w, want.h);
            if free(&r) {
                let d = (x - want.x).powi(2) + (y - want.y).powi(2);
                if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                    best = Some((d, r));
                }
            }
            x += step;
        }
        y += step;
    }
    best.map(|(_, r)| r).unwrap_or(want)
}

/// Take box `id` off the page shown (for putting it onto another page).
pub fn take_box(l: &mut MonitorLayout, id: u32) -> Option<BoxModel> {
    let i = l.boxes.iter().position(|b| b.id == id)?;
    Some(l.boxes.remove(i))
}

/// Move box `id` (on the page shown) to page `to`, where it keeps its place if that is clear.
pub fn move_box_to_page(l: &mut MonitorLayout, id: u32, to: u32, g: &Grid) -> bool {
    if to == l.page || !l.parked.contains_key(&to) {
        return false;
    }
    let Some(b) = take_box(l, id) else { return false };
    with_page(l, to, |l| absorb(l, PageContent { boxes: vec![b], ..Default::default() }, g));
    true
}

/// Move icons to page `to`'s desktop: the first free cells there, in order.
pub fn move_items_to_page(l: &mut MonitorLayout, keys: &[ItemKey], to: u32, g: &Grid) -> bool {
    if keys.is_empty() || index_of(l, to).is_none() {
        return false;
    }
    with_page(l, to, |l| layout::move_to_grid(l, keys, Cell { col: 0, row: 0 }, g)).is_some()
}

/// Every page's boxes, with the page each is on.
pub fn all_boxes(l: &MonitorLayout) -> impl Iterator<Item = (u32, &BoxModel)> {
    l.boxes.iter().map(move |b| (l.page, b)).chain(l.parked.iter().flat_map(|(id, c)| c.boxes.iter().map(move |b| (*id, b))))
}

/// The page box `id` is on.
pub fn page_of_box(l: &MonitorLayout, id: u32) -> Option<u32> {
    all_boxes(l).find(|(_, b)| b.id == id).map(|(p, _)| p)
}

/// The page an icon is on.
pub fn page_of_item(l: &MonitorLayout, k: &ItemKey) -> Option<u32> {
    if layout::place_of(l, k).is_some() {
        return Some(l.page);
    }
    l.parked.iter().find(|(_, c)| c.loose.contains_key(k) || c.boxes.iter().any(|b| b.contains(k).is_some())).map(|(id, _)| *id)
}

/// Where an icon is, on any page: (page, its place there, its position in its box tab).
pub fn find_item(l: &MonitorLayout, k: &ItemKey) -> Option<(u32, Place, usize)> {
    let look = |boxes: &[BoxModel], loose: &std::collections::BTreeMap<ItemKey, Cell>| -> Option<(Place, usize)> {
        if let Some(c) = loose.get(k) {
            return Some((Place::Grid(*c), 0));
        }
        boxes.iter().find_map(|b| b.contains(k).map(|(ti, ix)| (Place::Box { box_id: b.id, tab: b.tabs[ti].id }, ix)))
    };
    if let Some((p, ix)) = look(&l.boxes, &l.loose) {
        return Some((l.page, p, ix));
    }
    l.parked.iter().find_map(|(id, c)| look(&c.boxes, &c.loose).map(|(p, ix)| (*id, p, ix)))
}

/// A tab rules and "new icons" can send icons to (on any page): a plain box's, not a folder
/// box's (that shows a folder; desktop icons never go into it).
pub fn sortable_tab(l: &MonitorLayout, tab: u32) -> Option<(u32, u32, usize)> {
    all_boxes(l).filter(|(_, b)| !b.is_folder()).find_map(|(p, b)| b.tabs.iter().position(|t| t.id == tab).map(|i| (p, b.id, i)))
}

/// The page, box and tab index holding tab id `tab`, on any page.
pub fn find_tab(l: &MonitorLayout, tab: u32) -> Option<(u32, u32, usize)> {
    all_boxes(l).find_map(|(p, b)| b.tabs.iter().position(|t| t.id == tab).map(|i| (p, b.id, i)))
}

/// Icons placed on the other pages.
pub fn parked_keys(l: &MonitorLayout) -> HashSet<ItemKey> {
    let mut s = HashSet::new();
    for c in l.parked.values() {
        s.extend(c.loose.keys().cloned());
        for b in &c.boxes {
            for t in &b.tabs {
                s.extend(t.items.iter().cloned());
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{create_box, move_to_box, place_of, placed_keys, reconcile};

    fn keys(n: usize) -> Vec<ItemKey> {
        (0..n).map(|i| ItemKey(format!("p:{i}"))).collect()
    }
    fn grid() -> Grid {
        Grid::new(Rect::new(0.0, 0.0, 750.0, 500.0), 75.0, 100.0) // 10 x 5
    }
    fn all_placed(l: &MonitorLayout) -> HashSet<ItemKey> {
        let mut s: HashSet<ItemKey> = placed_keys(l).into_iter().collect();
        s.extend(parked_keys(l));
        s
    }

    #[test]
    fn switch_swaps_pages_and_keeps_each_item_once() {
        let g = grid();
        let k = keys(4);
        let mut l = MonitorLayout::default();
        reconcile(&mut l, &k, &g);
        let first = l.page;
        let b = create_box(&mut l, Rect::new(300.0, 0.0, 150.0, 200.0), "A", BoxStyle::default());
        move_to_box(&mut l, &k[..1], b, None);
        let p2 = new_page(&mut l, "Work", None);
        assert_eq!(count(&l), 2);
        assert_eq!(ids(&l), vec![first, p2]);
        assert!(move_items_to_page(&mut l, &k[1..2], p2, &g));
        // k1 left this page and is on page 2.
        assert!(place_of(&l, &k[1]).is_none());
        assert_eq!(page_of_item(&l, &k[1]), Some(p2));
        assert!(switch_to(&mut l, p2));
        assert_eq!(l.page, p2);
        assert!(l.boxes.is_empty());
        assert!(matches!(place_of(&l, &k[1]), Some(Place::Grid(_))));
        assert_eq!(page_of_box(&l, b), Some(first));
        // Nothing new appears and nothing is lost: every item placed once, across pages.
        assert!(!reconcile(&mut l, &k, &g));
        assert_eq!(all_placed(&l).len(), 4);
        assert!(switch_to(&mut l, first));
        assert_eq!(l.boxes.len(), 1);
        assert_eq!(name(&l, p2), "Work");
    }

    #[test]
    fn reconcile_forgets_removed_items_on_other_pages_and_new_ones_land_on_the_page_shown() {
        let g = grid();
        let k = keys(3);
        let mut l = MonitorLayout::default();
        reconcile(&mut l, &k[..2], &g);
        let p2 = new_page(&mut l, "", None);
        move_items_to_page(&mut l, &k[1..2], p2, &g);
        // k1 deleted from the disk, k2 new.
        assert!(reconcile(&mut l, &[k[0].clone(), k[2].clone()], &g));
        assert_eq!(page_of_item(&l, &k[1]), None);
        assert_eq!(page_of_item(&l, &k[2]), Some(l.page));
        // New items onto the first page while page 2 is shown.
        switch_to(&mut l, p2);
        let k3 = ItemKey("p:new".into());
        let first = ids(&l)[0];
        with_page(&mut l, first, |l| reconcile(l, &[k[0].clone(), k[2].clone(), k3.clone()], &g));
        assert_eq!(l.page, p2);
        assert_eq!(page_of_item(&l, &k3), Some(first));
    }

    #[test]
    fn delete_moves_everything_to_the_neighbour_clear_of_its_boxes() {
        let g = grid();
        let k = keys(4);
        let mut l = MonitorLayout::default();
        reconcile(&mut l, &k[..2], &g);
        let first = l.page;
        create_box(&mut l, Rect::new(300.0, 0.0, 150.0, 200.0), "A", BoxStyle::default());
        let p2 = new_page(&mut l, "", None);
        let p3 = new_page(&mut l, "", None);
        reconcile(&mut l, &k, &g); // k2, k3 on the page shown (the first)
        move_items_to_page(&mut l, &k[2..], p2, &g);
        switch_to(&mut l, p2);
        let b2 = create_box(&mut l, Rect::new(300.0, 0.0, 150.0, 200.0), "B", BoxStyle::default());
        move_to_box(&mut l, &k[2..3], b2, None);
        // Deleting the page shown: we go to its neighbour on the left, which gets its things.
        assert_eq!(delete(&mut l, p2, &g), Some(first));
        assert_eq!(l.page, first);
        assert_eq!(ids(&l), vec![first, p3]);
        assert_eq!(l.boxes.len(), 2);
        let (a, b) = (l.boxes[0].rect, l.boxes[1].rect);
        assert!(!a.inflate(8.0).intersects(&b), "{a:?} {b:?}");
        assert_eq!(all_placed(&l).len(), 4);
        // The first page's neighbour is on its right.
        assert_eq!(neighbour(&l, first), Some(p3));
        assert_eq!(delete(&mut l, first, &g), Some(p3));
        assert_eq!(l.page, p3);
        assert_eq!(count(&l), 1);
        assert_eq!(delete(&mut l, p3, &g), None);
        assert_eq!(all_placed(&l).len(), 4);
    }

    #[test]
    fn merge_and_move_box() {
        let g = grid();
        let k = keys(3);
        let mut l = MonitorLayout::default();
        reconcile(&mut l, &k, &g);
        let b = create_box(&mut l, Rect::new(300.0, 0.0, 150.0, 200.0), "A", BoxStyle::default());
        move_to_box(&mut l, &k[..1], b, None);
        let p2 = new_page(&mut l, "", None);
        assert!(move_box_to_page(&mut l, b, p2, &g));
        assert!(l.boxes.is_empty());
        assert_eq!(page_of_box(&l, b), Some(p2));
        assert_eq!(page_of_item(&l, &k[0]), Some(p2));
        assert!(merge_all(&mut l, &g));
        assert_eq!(count(&l), 1);
        assert_eq!(l.boxes.len(), 1);
        assert_eq!(all_placed(&l).len(), 3);
    }

    #[test]
    fn rules_file_into_a_box_on_another_page() {
        let g = grid();
        let mut l = MonitorLayout::default();
        let it = Item {
            key: ItemKey::path(r"C:\Users\u\Desktop\a.pdf"),
            parsing_name: r"C:\Users\u\Desktop\a.pdf".into(),
            display_name: "a.pdf".into(),
            kind: ItemKind::File,
            ext: ".pdf".into(),
            size: 1,
            modified: crate::rules::now_filetime(),
            attrs: 0,
            source: Source::User,
            icon: 0,
            link: String::new(),
        };
        reconcile(&mut l, std::slice::from_ref(&it.key), &g);
        let p2 = new_page(&mut l, "", None);
        let first = l.page;
        switch_to(&mut l, p2);
        let b = create_box(&mut l, Rect::new(300.0, 0.0, 150.0, 200.0), "PDF", BoxStyle::default());
        let tab = l.boxes[0].tabs[0].id;
        crate::rules::new_rule(&mut l, "type", "pdf", tab);
        switch_to(&mut l, first);
        assert!(crate::rules::apply(&mut l, &[&it], false, crate::rules::now_filetime()));
        assert_eq!(page_of_item(&l, &it.key), Some(p2));
        assert_eq!(l.page, first);
        assert!(!crate::rules::apply(&mut l, &[&it], false, crate::rules::now_filetime()));
        let _ = b;
    }
}
