//! Pure layout operations on a `MonitorLayout` (docs/ARCHITECTURE.md §7). Layout is metadata
//! only: nothing here touches a file. Every item is placed exactly once: on the loose grid or in
//! one box tab.

use crate::geom::{Cell, Grid, Rect};
use crate::model::*;
use std::collections::{BTreeSet, HashMap, HashSet};

pub fn alloc_id(l: &mut MonitorLayout) -> u32 {
    l.next_id = l.next_id.max(1) + 1;
    l.next_id
}

pub fn place_of(l: &MonitorLayout, k: &ItemKey) -> Option<Place> {
    if let Some(c) = l.loose.get(k) {
        return Some(Place::Grid(*c));
    }
    for b in &l.boxes {
        if let Some((ti, _)) = b.contains(k) {
            return Some(Place::Box { box_id: b.id, tab: b.tabs[ti].id });
        }
    }
    None
}

fn occupied(l: &MonitorLayout) -> HashSet<Cell> {
    l.loose.values().copied().collect()
}

/// Grid cells covered by boxes are not used for loose icons.
fn blocked_by_boxes(l: &MonitorLayout, g: &Grid, c: Cell) -> bool {
    let r = g.cell_rect(c).inflate(-4.0);
    l.boxes.iter().any(|b| b.rect.intersects(&r))
}

/// First free cell in column-major order, starting at `from` (inclusive).
pub fn free_cell(l: &MonitorLayout, g: &Grid, from: i32, taken: &HashSet<Cell>) -> Option<Cell> {
    let total = g.cols * g.rows;
    (from.max(0)..total).map(|n| g.nth(n)).find(|c| !taken.contains(c) && !blocked_by_boxes(l, g, *c))
}

/// Bring the layout in line with the items that exist now: forget removed ones, place new ones in
/// the first free grid cells (Explorer's behaviour for new desktop items). Returns true if changed.
pub fn reconcile(l: &mut MonitorLayout, present: &[ItemKey], g: &Grid) -> bool {
    let set: HashSet<&ItemKey> = present.iter().collect();
    let mut changed = false;
    // The other desktop pages: removed items go from there too; their items are placed.
    for c in l.parked.values_mut() {
        let n = c.loose.len();
        c.loose.retain(|k, _| set.contains(k));
        c.homes.retain(|k, _| set.contains(k));
        changed |= n != c.loose.len();
        for b in c.boxes.iter_mut().filter(|b| !b.is_folder()) {
            for t in &mut b.tabs {
                let n = t.items.len();
                t.items.retain(|k| set.contains(k));
                changed |= n != t.items.len();
            }
        }
    }
    let before = l.loose.len();
    l.loose.retain(|k, _| set.contains(k));
    changed |= before != l.loose.len();
    l.homes.retain(|k, _| set.contains(k));
    // (A folder box's items are not desktop items: it keeps them.)
    for b in l.boxes.iter_mut().filter(|b| !b.is_folder()) {
        for t in &mut b.tabs {
            let n = t.items.len();
            t.items.retain(|k| set.contains(k));
            changed |= n != t.items.len();
        }
    }
    let mut placed: HashSet<ItemKey> = l.loose.keys().cloned().collect();
    for b in &l.boxes {
        for t in &b.tabs {
            placed.extend(t.items.iter().cloned());
        }
    }
    placed.extend(crate::pages::parked_keys(l));
    let mut taken = occupied(l);
    // Cells pushed off-grid (smaller screen) are re-placed.
    let off: Vec<ItemKey> = l.loose.iter().filter(|(_, c)| c.col >= g.cols || c.row >= g.rows).map(|(k, _)| k.clone()).collect();
    for k in off {
        l.loose.remove(&k);
        placed.remove(&k);
        changed = true;
    }
    let mut cursor = 0;
    for k in present {
        if placed.contains(k) {
            continue;
        }
        let c = match free_cell(l, g, cursor, &taken) {
            Some(c) => c,
            None => Cell { col: g.cols - 1, row: g.rows - 1 }, // full: stack on the last cell
        };
        cursor = g.index_of(c) + 1;
        taken.insert(c);
        l.loose.insert(k.clone(), c);
        placed.insert(k.clone());
        changed = true;
    }
    changed
}

/// Take items out of wherever they are (they are being placed somewhere on purpose: a cell they
/// were pushed out of no longer counts as theirs).
fn remove_everywhere(l: &mut MonitorLayout, keys: &[ItemKey]) {
    let set: HashSet<&ItemKey> = keys.iter().collect();
    l.loose.retain(|k, _| !set.contains(k));
    l.homes.retain(|k, _| !set.contains(k));
    for b in &mut l.boxes {
        for t in &mut b.tabs {
            t.items.retain(|k| !set.contains(k));
        }
    }
    // (And from the other desktop pages.)
    for c in l.parked.values_mut() {
        c.loose.retain(|k, _| !set.contains(k));
        c.homes.retain(|k, _| !set.contains(k));
        for b in &mut c.boxes {
            for t in &mut b.tabs {
                t.items.retain(|k| !set.contains(k));
            }
        }
    }
}

/// Move items to the loose grid. The first goes to `target` (or the next free cell after it);
/// the others keep their order in the following free cells.
pub fn move_to_grid(l: &mut MonitorLayout, keys: &[ItemKey], target: Cell, g: &Grid) {
    remove_everywhere(l, keys);
    let mut taken = occupied(l);
    let mut cursor = g.index_of(target);
    for k in keys {
        let c = free_cell(l, g, cursor, &taken).or_else(|| free_cell(l, g, 0, &taken)).unwrap_or(target);
        cursor = g.index_of(c) + 1;
        taken.insert(c);
        l.loose.insert(k.clone(), c);
    }
}

/// Put items into the loose grid at `at`, as Explorer inserts between icons: the icons from
/// `at` on (column by column) move along to make room, up to the first free cells; cells under
/// boxes are skipped.
pub fn insert_in_grid(l: &mut MonitorLayout, keys: &[ItemKey], at: Cell, g: &Grid) {
    remove_everywhere(l, keys);
    let by_cell: HashMap<Cell, ItemKey> = l.loose.iter().map(|(k, c)| (*c, k.clone())).collect();
    let mut queue: std::collections::VecDeque<ItemKey> = keys.iter().cloned().collect();
    let total = g.cols * g.rows;
    let mut n = g.index_of(Cell { col: at.col.clamp(0, g.cols - 1), row: at.row.clamp(0, g.rows - 1) });
    while let Some(k) = queue.pop_front() {
        // Past the last cell: whatever is left goes to free cells from the start.
        if n >= total {
            queue.push_front(k);
            let rest: Vec<ItemKey> = queue.into_iter().collect();
            let mut taken = occupied(l);
            for k in rest {
                let c = free_cell(l, g, 0, &taken).unwrap_or(Cell { col: g.cols - 1, row: g.rows - 1 });
                taken.insert(c);
                l.loose.insert(k, c);
            }
            return;
        }
        let c = g.nth(n);
        n += 1;
        if blocked_by_boxes(l, g, c) {
            queue.push_front(k);
            continue;
        }
        if let Some(o) = by_cell.get(&c) {
            if !keys.contains(o) {
                queue.push_back(o.clone());
            }
        }
        l.loose.insert(k, c);
    }
}

/// Nearest cell to `want` that is free (not in `taken`, not under a box), by distance on screen.
pub fn nearest_free(l: &MonitorLayout, g: &Grid, want: Cell, taken: &HashSet<Cell>) -> Option<Cell> {
    let want = Cell {
        col: want.col.clamp(0, g.cols - 1),
        row: want.row.clamp(0, g.rows - 1),
    };
    let max_ring = g.cols.max(g.rows);
    for ring in 0..=max_ring {
        let mut best: Option<(f32, Cell)> = None;
        for col in (want.col - ring)..=(want.col + ring) {
            for row in (want.row - ring)..=(want.row + ring) {
                if (col - want.col).abs() != ring && (row - want.row).abs() != ring {
                    continue; // inside the ring: already searched
                }
                if col < 0 || row < 0 || col >= g.cols || row >= g.rows {
                    continue;
                }
                let c = Cell { col, row };
                if taken.contains(&c) || blocked_by_boxes(l, g, c) {
                    continue;
                }
                let (dx, dy) = ((col - want.col) as f32 * g.cell_w, (row - want.row) as f32 * g.cell_h);
                let d = dx * dx + dy * dy;
                if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                    best = Some((d, c));
                }
            }
        }
        if let Some((_, c)) = best {
            return Some(c);
        }
    }
    None
}

/// Move items to the loose grid, each to its own wanted cell (Explorer's multi-item drop: every
/// icon moves by the same offset, so the group keeps its shape). A wanted cell that is taken by
/// another item, under a box or off the grid falls back to the nearest free cell.
pub fn move_to_grid_at(l: &mut MonitorLayout, moves: &[(ItemKey, Cell)], g: &Grid) {
    let keys: Vec<ItemKey> = moves.iter().map(|(k, _)| k.clone()).collect();
    remove_everywhere(l, &keys);
    let mut taken = occupied(l);
    // Items whose wanted cell is free go first, so a collision never pushes away a group member.
    let (direct, rest): (Vec<&(ItemKey, Cell)>, Vec<&(ItemKey, Cell)>) = moves
        .iter()
        .partition(|(_, c)| c.col >= 0 && c.row >= 0 && c.col < g.cols && c.row < g.rows && !blocked_by_boxes(l, g, *c));
    let mut later = vec![];
    for (k, c) in direct {
        if taken.insert(*c) {
            l.loose.insert(k.clone(), *c);
        } else {
            later.push((k, *c));
        }
    }
    for (k, c) in rest.into_iter().map(|(k, c)| (k, *c)).chain(later) {
        let c = nearest_free(l, g, c, &taken).unwrap_or(Cell { col: g.cols - 1, row: g.rows - 1 });
        taken.insert(c);
        l.loose.insert(k.clone(), c);
    }
}

/// Keep loose icons clear of boxes: icons pushed aside earlier go back to their own cell once no
/// box covers it (and nothing else took it), and icons a box now covers (it was created, moved
/// or resized over them) go to the nearest free cells, remembering where they were. Returns
/// true if any icon moved.
pub fn settle(l: &mut MonitorLayout, g: &Grid) -> bool {
    let mut changed = false;
    let loose = &l.loose;
    l.homes.retain(|k, h| loose.get(k).is_some_and(|c| c != h));
    // Back home (a returning icon frees a cell that may be another one's home: repeat).
    loop {
        let mut taken = occupied(l);
        let mut back = vec![];
        for (k, h) in &l.homes {
            if h.col < g.cols && h.row < g.rows && !taken.contains(h) && !blocked_by_boxes(l, g, *h) {
                taken.insert(*h);
                back.push((k.clone(), *h));
            }
        }
        if back.is_empty() {
            break;
        }
        for (k, h) in back {
            l.loose.insert(k.clone(), h);
            l.homes.remove(&k);
        }
        changed = true;
    }
    let covered: Vec<(ItemKey, Cell)> = l.loose.iter().filter(|(_, c)| blocked_by_boxes(l, g, **c)).map(|(k, c)| (k.clone(), *c)).collect();
    if covered.is_empty() {
        return changed;
    }
    for (k, c) in &covered {
        l.loose.remove(k);
        l.homes.entry(k.clone()).or_insert(*c);
    }
    // Icons pushed along remember their cell too: they go back when the box moves away.
    for (k, c) in step_aside(l, g, covered) {
        if let Some(was) = l.loose.insert(k.clone(), c) {
            l.homes.entry(k).or_insert(was);
        }
    }
    true
}

/// Where icons a box covers go: all to the same side of it, each pushed along its own row or
/// column past the box, keeping their order; icons already in that line past the box move along
/// to make room (up to the first free cell), as on a phone's home screen. Icons that sit in a row
/// with their neighbours stay in that row, a column in that column (a covered icon jumping up out
/// of the row, while the box should have pushed the row along, round 13); then the side that
/// moves them least. An icon whose line runs off the grid takes the nearest free cell instead.
/// Returns every icon that moves: the covered ones and the ones pushed along.
fn step_aside(l: &MonitorLayout, g: &Grid, covered: Vec<(ItemKey, Cell)>) -> Vec<(ItemKey, Cell)> {
    // right, down, left, up (ties go in this order)
    const DIRS: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];
    let others: HashMap<Cell, ItemKey> = l.loose.iter().map(|(k, c)| (*c, k.clone())).collect();
    let near = |col: i32, row: i32| others.contains_key(&Cell { col, row });
    let in_row = covered.iter().filter(|(_, c)| near(c.col - 1, c.row) || near(c.col + 1, c.row)).count();
    let in_col = covered.iter().filter(|(_, c)| near(c.col, c.row - 1) || near(c.col, c.row + 1)).count();
    // (icons off their line, across the line they sit in, distance on screen)
    let mut best: Option<((usize, bool, f32), Vec<(ItemKey, Cell)>)> = None;
    for (dx, dy) in DIRS {
        let across = if dx != 0 { in_col > in_row } else { in_row > in_col };
        let mut at = others.clone();
        let mut moved: Vec<(ItemKey, Cell, Cell)> = vec![];
        let mut stray: Vec<(ItemKey, Cell)> = vec![];
        // Line by line (rows when going sideways, columns when going up or down).
        let mut lines: HashMap<i32, Vec<(ItemKey, Cell)>> = HashMap::new();
        for (k, c) in &covered {
            lines.entry(if dx != 0 { c.row } else { c.col }).or_default().push((k.clone(), *c));
        }
        let mut lines: Vec<(i32, Vec<(ItemKey, Cell)>)> = lines.into_iter().collect();
        lines.sort_by_key(|(n, _)| *n);
        for (_, mut line) in lines {
            // The one farthest from the way out takes the first cell past the box: the line keeps its order.
            line.sort_by_key(|(_, c)| c.col * dx + c.row * dy);
            let lead = line.iter().map(|(_, c)| *c).max_by_key(|c| c.col * dx + c.row * dy).unwrap();
            let start = Cell { col: lead.col + dx, row: lead.row + dy };
            let (m, s) = push_line(l, g, &mut at, start, (dx, dy), line.into_iter().collect());
            moved.extend(m);
            stray.extend(s);
        }
        let mut dist: f32 = moved.iter().map(|(_, a, b)| ((b.col - a.col) as f32 * g.cell_w).abs() + ((b.row - a.row) as f32 * g.cell_h).abs()).sum();
        let mut out: Vec<(ItemKey, Cell)> = moved.into_iter().map(|(k, _, c)| (k, c)).collect();
        let n_stray = stray.len();
        let mut taken: HashSet<Cell> = at.keys().copied().collect();
        for (k, c) in stray {
            let to = nearest_free(l, g, c, &taken).unwrap_or(c);
            dist += ((to.col - c.col) as f32 * g.cell_w).abs() + ((to.row - c.row) as f32 * g.cell_h).abs();
            taken.insert(to);
            out.push((k, to));
        }
        let cost = (n_stray, across, dist);
        if best.as_ref().map(|(b, _)| cost.partial_cmp(b) == Some(std::cmp::Ordering::Less)).unwrap_or(true) {
            best = Some((cost, out));
        }
    }
    best.map(|(_, v)| v).unwrap_or_default()
}

/// Push items into a line of cells from `start`, one cell further each (cells under boxes are
/// skipped): an icon in a cell they need moves along behind them, up to the first free cell.
/// `at` is who is where, kept up to date. Returns the moves (item, from, to) and the items left
/// when the line ran off the grid (with where they were).
fn push_line(l: &MonitorLayout, g: &Grid, at: &mut HashMap<Cell, ItemKey>, start: Cell, dir: (i32, i32), mut q: std::collections::VecDeque<(ItemKey, Cell)>) -> (Vec<(ItemKey, Cell, Cell)>, Vec<(ItemKey, Cell)>) {
    let mut c = start;
    let mut out = vec![];
    while let Some((k, from)) = q.pop_front() {
        while c.col >= 0 && c.row >= 0 && c.col < g.cols && c.row < g.rows && blocked_by_boxes(l, g, c) {
            c = Cell { col: c.col + dir.0, row: c.row + dir.1 };
        }
        if c.col < 0 || c.row < 0 || c.col >= g.cols || c.row >= g.rows {
            q.push_front((k, from));
            return (out, q.into_iter().collect());
        }
        if let Some(o) = at.remove(&c) {
            q.push_back((o, c));
        }
        at.insert(c, k.clone());
        out.push((k, from, c));
        c = Cell { col: c.col + dir.0, row: c.row + dir.1 };
    }
    (out, vec![])
}

/// Put items into the loose grid at `at`, in a row (`dir` (1, 0)) or a column ((0, 1)): the
/// icons from `at` on move along that line to make room, up to the first free cell, and never
/// jump to another line (Explorer's order carried the last icon of a column to the top of the
/// next one, round 13). What runs off the grid goes to the nearest free cells.
pub fn insert_along(l: &mut MonitorLayout, keys: &[ItemKey], at: Cell, dir: (i32, i32), g: &Grid) {
    remove_everywhere(l, keys);
    let mut by_cell: HashMap<Cell, ItemKey> = l.loose.iter().map(|(k, c)| (*c, k.clone())).collect();
    let at = Cell { col: at.col.clamp(0, g.cols - 1), row: at.row.clamp(0, g.rows - 1) };
    let (moved, stray) = push_line(l, g, &mut by_cell, at, dir, keys.iter().map(|k| (k.clone(), at)).collect());
    for (k, _, c) in moved {
        l.loose.insert(k, c);
    }
    let mut taken = occupied(l);
    for (k, c) in stray {
        let to = nearest_free(l, g, c, &taken).unwrap_or(Cell { col: g.cols - 1, row: g.rows - 1 });
        taken.insert(to);
        l.loose.insert(k, to);
    }
}

/// Loose icons a box covers move out of its way (see `settle`).
pub fn evict_from_boxes(l: &mut MonitorLayout, g: &Grid) -> bool {
    settle(l, g)
}

/// Desktop "Sort by": the loose icons in `order` fill the grid from the first cell, column by
/// column, around the boxes (Explorer's arrangement).
pub fn arrange_loose(l: &mut MonitorLayout, g: &Grid, order: &[ItemKey]) -> bool {
    let keys: Vec<ItemKey> = order.iter().filter(|k| l.loose.contains_key(*k)).cloned().collect();
    if keys.is_empty() {
        return false;
    }
    let before = l.loose.clone();
    for k in &keys {
        l.loose.remove(k);
        l.homes.remove(k);
    }
    let mut taken = occupied(l);
    let mut cursor = 0;
    for k in keys {
        let c = free_cell(l, g, cursor, &taken).or_else(|| free_cell(l, g, 0, &taken)).unwrap_or(Cell { col: g.cols - 1, row: g.rows - 1 });
        cursor = g.index_of(c) + 1;
        taken.insert(c);
        l.loose.insert(k, c);
    }
    l.loose != before
}

/// Move items into a box's active tab at `index` (None = append), keeping their order.
pub fn move_to_box(l: &mut MonitorLayout, keys: &[ItemKey], box_id: u32, index: Option<usize>) -> bool {
    let Some(tab) = l.boxes.iter().find(|b| b.id == box_id).map(|b| b.active_tab) else { return false };
    move_to_tab(l, keys, box_id, tab, index)
}

/// Move items into tab `tab` of a box at `index` (None = append), keeping their order.
pub fn move_to_tab(l: &mut MonitorLayout, keys: &[ItemKey], box_id: u32, tab: usize, index: Option<usize>) -> bool {
    let Some(bi) = l.boxes.iter().position(|b| b.id == box_id) else { return false };
    // A folder box shows a folder: desktop items never go into it as layout.
    if l.boxes[bi].tabs.is_empty() || l.boxes[bi].is_folder() {
        return false;
    }
    // Adjust the insertion index for items removed before it in the same tab.
    let tab_ix = tab.min(l.boxes[bi].tabs.len() - 1);
    let mut idx = index;
    if let Some(i) = idx {
        let before = l.boxes[bi].tabs[tab_ix].items.iter().take(i).filter(|k| keys.contains(k)).count();
        idx = Some(i - before);
    }
    remove_everywhere(l, keys);
    let tab = &mut l.boxes[bi].tabs[tab_ix];
    let at = idx.unwrap_or(tab.items.len()).min(tab.items.len());
    for (n, k) in keys.iter().enumerate() {
        tab.items.insert(at + n, k.clone());
    }
    true
}

pub fn create_box(l: &mut MonitorLayout, rect: Rect, title: &str, style: BoxStyle) -> u32 {
    let id = alloc_id(l);
    let tab = alloc_id(l);
    l.boxes.push(BoxModel {
        id,
        rect,
        style,
        tabs: vec![Tab::new(tab, title)],

        active_tab: 0,
        rolled_up: false,
        locks: Default::default(),
        az: false,
        tint: String::new(),
        blend: String::new(),
        stay_visible: false,
        folder: String::new(),
        folder_at: String::new(),
        view: String::new(),
        icon_size: 0,
    });
    id
}

/// A folder box showing `folder` (its files stay where they are).
pub fn create_folder_box(l: &mut MonitorLayout, rect: Rect, title: &str, style: BoxStyle, folder: &str) -> u32 {
    let id = create_box(l, rect, title, style);
    if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) {
        b.folder = folder.to_string();
        b.set_tab_folder(0, folder, "");
    }
    id
}

/// Delete a box; its items return to the grid (files are never touched).
pub fn delete_box(l: &mut MonitorLayout, box_id: u32, g: &Grid) -> bool {
    let Some(bi) = l.boxes.iter().position(|b| b.id == box_id) else { return false };
    let b = l.boxes.remove(bi);
    // A folder box's items are in its folder, not on the desktop.
    if b.is_folder() {
        return true;
    }
    let keys: Vec<ItemKey> = b.tabs.into_iter().flat_map(|t| t.items).collect();
    let mut taken = occupied(l);
    let mut cursor = 0;
    for k in keys {
        if let Some(c) = free_cell(l, g, cursor, &taken) {
            cursor = g.index_of(c) + 1;
            taken.insert(c);
            l.loose.insert(k, c);
        }
    }
    true
}

/// Combine `src` into `dst` as tabs (a tab pair from two boxes). `src` disappears.
pub fn combine(l: &mut MonitorLayout, src: u32, dst: u32) -> bool {
    if src == dst || !same_kind(l, src, dst) {
        return false;
    }
    for b in l.boxes.iter_mut().filter(|b| b.id == src || b.id == dst) {
        b.own_folders();
    }
    let Some(si) = l.boxes.iter().position(|b| b.id == src) else { return false };
    if !l.boxes.iter().any(|b| b.id == dst) {
        return false;
    }
    let s = l.boxes.remove(si);
    let d = l.boxes.iter_mut().find(|b| b.id == dst).unwrap();
    let first_new = d.tabs.len();
    d.tabs.extend(s.tabs);
    d.active_tab = first_new;
    true
}

/// Both boxes are folder boxes, or neither is (their tabs can be put together).
pub fn same_kind(l: &MonitorLayout, a: u32, b: u32) -> bool {
    let kind = |id: u32| l.boxes.iter().find(|x| x.id == id).map(|x| x.is_folder());
    matches!((kind(a), kind(b)), (Some(x), Some(y)) if x == y)
}

/// A new empty tab at the end of a box, made active. Returns its index. In a folder box it shows
/// the open tab's folder, as a new tab does in File Explorer.
pub fn add_tab(l: &mut MonitorLayout, box_id: u32, title: &str) -> Option<usize> {
    l.boxes.iter().position(|b| b.id == box_id)?;
    let id = alloc_id(l);
    let b = l.boxes.iter_mut().find(|b| b.id == box_id)?;
    let folder = b.is_folder().then(|| b.folder_root().to_string());
    b.own_folders();
    b.tabs.push(Tab::new(id, title));
    b.active_tab = b.tabs.len() - 1;
    if let Some(f) = folder {
        let ti = b.active_tab;
        b.set_tab_folder(ti, &f, "");
    }
    Some(b.active_tab)
}

/// A folder box: a new tab showing `folder` (a folder in it), made active, after the open tab.
pub fn open_in_new_tab(l: &mut MonitorLayout, box_id: u32, folder: &str) -> Option<usize> {
    l.boxes.iter().position(|b| b.id == box_id && b.is_folder())?;
    let id = alloc_id(l);
    let b = l.boxes.iter_mut().find(|b| b.id == box_id)?;
    b.own_folders();
    let (root, title) = (b.folder_root().to_string(), b.title().to_string());
    let at = (b.active_tab + 1).min(b.tabs.len());
    b.tabs.insert(at, Tab::new(id, &title));
    b.active_tab = at;
    b.set_tab_folder(at, &root, folder);
    Some(at)
}

/// Remove a tab; its items return to the grid (files are never touched). The last tab removes
/// the box.
pub fn remove_tab(l: &mut MonitorLayout, box_id: u32, tab_ix: usize, g: &Grid) -> bool {
    let Some(bi) = l.boxes.iter().position(|b| b.id == box_id) else { return false };
    if l.boxes[bi].tabs.len() < 2 {
        return delete_box(l, box_id, g);
    }
    if tab_ix >= l.boxes[bi].tabs.len() {
        return false;
    }
    l.boxes[bi].own_folders();
    let t = l.boxes[bi].tabs.remove(tab_ix);
    let b = &mut l.boxes[bi];
    if b.active_tab > tab_ix || b.active_tab >= b.tabs.len() {
        b.active_tab = b.active_tab.saturating_sub(1);
    }
    // A folder box's items are in its folders, not on the desktop.
    if b.is_folder() {
        return true;
    }
    let mut taken = occupied(l);
    let mut cursor = 0;
    for k in t.items {
        if let Some(c) = free_cell(l, g, cursor, &taken) {
            cursor = g.index_of(c) + 1;
            taken.insert(c);
            l.loose.insert(k, c);
        }
    }
    true
}

/// Reorder: tab `from` goes to index `to` (an index in the list without it). It stays active.
pub fn move_tab(l: &mut MonitorLayout, box_id: u32, from: usize, to: usize) -> bool {
    let Some(b) = l.boxes.iter_mut().find(|b| b.id == box_id) else { return false };
    if from >= b.tabs.len() {
        return false;
    }
    let t = b.tabs.remove(from);
    let to = to.min(b.tabs.len());
    b.tabs.insert(to, t);
    b.active_tab = to;
    from != to
}

/// Move one tab of `src` into `dst` (at the end, made active). A box left without tabs goes.
pub fn move_tab_to_box(l: &mut MonitorLayout, src: u32, tab_ix: usize, dst: u32) -> bool {
    if src == dst || !l.boxes.iter().any(|b| b.id == dst) || !same_kind(l, src, dst) {
        return false;
    }
    let Some(si) = l.boxes.iter().position(|b| b.id == src) else { return false };
    if tab_ix >= l.boxes[si].tabs.len() {
        return false;
    }
    l.boxes[si].own_folders();
    if let Some(d) = l.boxes.iter_mut().find(|b| b.id == dst) {
        d.own_folders();
    }
    let t = l.boxes[si].tabs.remove(tab_ix);
    if l.boxes[si].tabs.is_empty() {
        l.boxes.remove(si);
    } else {
        let b = &mut l.boxes[si];
        if b.active_tab > tab_ix || b.active_tab >= b.tabs.len() {
            b.active_tab = b.active_tab.saturating_sub(1);
        }
    }
    let d = l.boxes.iter_mut().find(|b| b.id == dst).unwrap();
    d.tabs.push(t);
    d.active_tab = d.tabs.len() - 1;
    true
}

/// Split a tab out into a new box at `rect`.
pub fn split_tab(l: &mut MonitorLayout, box_id: u32, tab_ix: usize, rect: Rect) -> Option<u32> {
    let bi = l.boxes.iter().position(|b| b.id == box_id)?;
    if l.boxes[bi].tabs.len() < 2 || tab_ix >= l.boxes[bi].tabs.len() {
        return None;
    }
    l.boxes[bi].own_folders();
    let tab = l.boxes[bi].tabs.remove(tab_ix);
    let b = &mut l.boxes[bi];
    b.active_tab = b.active_tab.min(b.tabs.len() - 1);
    let style = b.style.clone();
    // The new box is the same kind (a folder box's tab stays a folder box) and shows its icons
    // the same way.
    let (folder, view, icon_size) = (if b.is_folder() { tab.folder.clone() } else { String::new() }, b.view.clone(), b.icon_size);
    let id = alloc_id(l);
    l.boxes.push(BoxModel {
        id,
        rect,
        style,
        tabs: vec![tab],
        active_tab: 0,
        rolled_up: false,
        locks: Default::default(),
        az: false,
        tint: String::new(),
        blend: String::new(),
        stay_visible: false,
        folder,
        folder_at: String::new(),
        view,
        icon_size,
    });
    Some(id)
}

pub fn rename_tab(l: &mut MonitorLayout, box_id: u32, tab_ix: usize, title: &str) -> bool {
    match l.boxes.iter_mut().find(|b| b.id == box_id).and_then(|b| b.tabs.get_mut(tab_ix)) {
        Some(t) => {
            t.title = title.to_string();
            true
        }
        None => false,
    }
}

pub fn set_box_rect(l: &mut MonitorLayout, box_id: u32, rect: Rect) -> bool {
    match l.boxes.iter_mut().find(|b| b.id == box_id) {
        Some(b) => {
            b.rect = rect;
            true
        }
        None => false,
    }
}

/// Snap a moving/resizing box rect to the work area edges and other boxes' edges (within
/// `threshold` DIPs). Holding Shift disables snapping (the caller skips this).
pub fn snap(rect: Rect, others: &[Rect], bounds: &Rect, threshold: f32) -> Rect {
    let mut xs = vec![bounds.x, bounds.right()];
    let mut ys = vec![bounds.y, bounds.bottom()];
    for o in others {
        xs.extend([o.x, o.right()]);
        ys.extend([o.y, o.bottom()]);
    }
    let best = |v: f32, cands: &[f32]| {
        cands
            .iter()
            .map(|c| (c - v, *c))
            .filter(|(d, _)| d.abs() <= threshold)
            .min_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
            .map(|x| x.0)
    };
    let mut r = rect;
    // Edges: prefer whichever of left/right (top/bottom) is closer to a candidate.
    let dl = best(r.x, &xs);
    let dr = best(r.right(), &xs);
    match (dl, dr) {
        (Some(a), Some(b)) => r.x += if a.abs() <= b.abs() { a } else { b },
        (Some(a), None) | (None, Some(a)) => r.x += a,
        _ => {}
    }
    let dt = best(r.y, &ys);
    let db = best(r.bottom(), &ys);
    match (dt, db) {
        (Some(a), Some(b)) => r.y += if a.abs() <= b.abs() { a } else { b },
        (Some(a), None) | (None, Some(a)) => r.y += a,
        _ => {}
    }
    r.clamp_into(bounds)
}

/// All keys that are placed somewhere (for consistency checks).
pub fn placed_keys(l: &MonitorLayout) -> BTreeSet<ItemKey> {
    let mut s: BTreeSet<ItemKey> = l.loose.keys().cloned().collect();
    for b in &l.boxes {
        for t in &b.tabs {
            s.extend(t.items.iter().cloned());
        }
    }
    s
}

/// A file operation Deskboxes did that can be undone, as Explorer's Undo Move / Undo Delete /
/// Undo Rename (paths: full file-system paths).
#[derive(Clone, Debug, PartialEq)]
pub enum FileOp {
    /// Moved into a folder: (from, to).
    Moved(Vec<(String, String)>),
    /// Copied into a folder: (source, the copy).
    Copied(Vec<(String, String)>),
    /// Into the Recycle Bin: (where it was, the file in the Recycle Bin; "" if not known).
    Recycled(Vec<(String, String)>),
    /// Renamed: (from, to).
    Renamed(String, String),
}

/// A file operation in the undo history, with where its icons were (so they come back there).
#[derive(Clone, Debug, PartialEq)]
pub struct FileStep {
    pub op: FileOp,
    /// Each icon's place and its position in a box tab.
    pub places: Vec<(ItemKey, Place, usize)>,
}

enum Entry {
    Layout(MonitorLayout),
    Files(FileStep),
}

/// What an undo or redo did: the layout was put back, or a file operation is to be reversed
/// (undo) or done again (redo) by the caller.
#[derive(Debug)]
pub enum Step {
    Layout,
    Files(FileStep),
}

/// Undo history: whole-layout snapshots (layouts are small: a few KB for 1,000 items) and file
/// operations, in the order they happened.
#[derive(Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
}

impl History {
    fn push(&mut self, e: Entry) {
        self.undo.push(e);
        if self.undo.len() > 50 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    pub fn record(&mut self, before: &MonitorLayout) {
        self.push(Entry::Layout(before.clone()));
    }
    pub fn record_files(&mut self, step: FileStep) {
        self.push(Entry::Files(step));
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    /// The kind of the next undo / redo: None, Some(None) a layout change, Some(Some(op)) a file
    /// operation (for the menu's "Undo Move" and the like).
    pub fn next_undo(&self) -> Option<Option<&FileOp>> {
        self.undo.last().map(|e| match e {
            Entry::Layout(_) => None,
            Entry::Files(s) => Some(&s.op),
        })
    }
    pub fn next_redo(&self) -> Option<Option<&FileOp>> {
        self.redo.last().map(|e| match e {
            Entry::Layout(_) => None,
            Entry::Files(s) => Some(&s.op),
        })
    }
    pub fn undo(&mut self, current: &mut MonitorLayout) -> Option<Step> {
        match self.undo.pop()? {
            Entry::Layout(prev) => {
                self.redo.push(Entry::Layout(std::mem::replace(current, prev)));
                Some(Step::Layout)
            }
            Entry::Files(s) => {
                self.redo.push(Entry::Files(s.clone()));
                Some(Step::Files(s))
            }
        }
    }
    pub fn redo(&mut self, current: &mut MonitorLayout) -> Option<Step> {
        match self.redo.pop()? {
            Entry::Layout(next) => {
                self.undo.push(Entry::Layout(std::mem::replace(current, next)));
                Some(Step::Layout)
            }
            Entry::Files(s) => {
                self.undo.push(Entry::Files(s.clone()));
                Some(Step::Files(s))
            }
        }
    }
    /// A file step just undone or redone came out differently (new names, new Recycle Bin
    /// files): keep what really happened for the next redo / undo.
    pub fn update_last(&mut self, undone: bool, op: FileOp) {
        let stack = if undone { &mut self.redo } else { &mut self.undo };
        if let Some(Entry::Files(s)) = stack.last_mut() {
            s.op = op;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn keys(n: usize) -> Vec<ItemKey> {
        (0..n).map(|i| ItemKey(format!("p:{i}"))).collect()
    }
    fn grid() -> Grid {
        Grid::new(Rect::new(0.0, 0.0, 750.0, 500.0), 75.0, 100.0) // 10 x 5
    }

    #[test]
    fn tabs_add_reorder_move_and_remove() {
        let g = grid();
        let mut l = MonitorLayout::default();
        let k = keys(3);
        reconcile(&mut l, &k, &g);
        let a = create_box(&mut l, Rect::new(450.0, 300.0, 150.0, 200.0), "A", BoxStyle::default());
        let b = create_box(&mut l, Rect::new(600.0, 300.0, 150.0, 200.0), "B", BoxStyle::default());
        assert_eq!(add_tab(&mut l, a, "A2"), Some(1));
        move_to_tab(&mut l, &k[..2], a, 1, None);
        move_to_tab(&mut l, &k[2..], a, 0, None);
        let ba = |l: &MonitorLayout| l.boxes.iter().find(|x| x.id == a).cloned().unwrap();
        assert_eq!(ba(&l).tabs[1].items, k[..2].to_vec());
        // reorder: A2 first, and it stays the open tab
        assert!(move_tab(&mut l, a, 1, 0));
        assert_eq!(ba(&l).tabs.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["A2", "A"]);
        assert_eq!(ba(&l).active_tab, 0);
        // a tab moves into another box with its items
        assert!(move_tab_to_box(&mut l, a, 0, b));
        let bb = l.boxes.iter().find(|x| x.id == b).unwrap();
        assert_eq!(bb.tabs.len(), 2);
        assert_eq!(bb.active_tab, 1);
        assert_eq!(bb.tabs[1].items, k[..2].to_vec());
        // removing a tab puts its items back on the grid; the last tab removes the box
        assert!(remove_tab(&mut l, b, 1, &g));
        assert!(k[..2].iter().all(|x| l.loose.contains_key(x)));
        assert!(remove_tab(&mut l, a, 0, &g));
        assert!(l.boxes.iter().all(|x| x.id != a));
        assert!(l.loose.contains_key(&k[2]));
    }

    #[test]
    fn covered_icons_step_aside_together_in_order() {
        let g = grid(); // 10 x 5 cells of 75 x 100
        let k = keys(4);
        let cell = |col, row| Cell { col, row };
        // A tall box over columns 3-4: the icons in them go left (the shorter way), in order.
        let mut l = MonitorLayout::default();
        for (i, c) in [cell(3, 1), cell(4, 1), cell(3, 2), cell(8, 0)].into_iter().enumerate() {
            l.loose.insert(k[i].clone(), c);
        }
        create_box(&mut l, Rect::new(225.0, 0.0, 150.0, 500.0), "B", BoxStyle::default());
        assert!(settle(&mut l, &g));
        assert_eq!((l.loose[&k[0]], l.loose[&k[1]], l.loose[&k[2]], l.loose[&k[3]]), (cell(1, 1), cell(2, 1), cell(2, 2), cell(8, 0)));
        // A flat box over one row: its icons step down a row, side by side as they were.
        let mut l = MonitorLayout::default();
        for (i, c) in [cell(3, 2), cell(4, 2), cell(5, 2)].into_iter().enumerate() {
            l.loose.insert(k[i].clone(), c);
        }
        create_box(&mut l, Rect::new(225.0, 200.0, 225.0, 100.0), "B", BoxStyle::default());
        assert!(settle(&mut l, &g));
        assert_eq!((l.loose[&k[0]], l.loose[&k[1]], l.loose[&k[2]]), (cell(3, 3), cell(4, 3), cell(5, 3)));
    }

    #[test]
    fn a_box_pushes_a_row_of_icons_along_and_they_come_back() {
        let g = grid(); // 10 x 5
        let k = keys(7);
        let cell = |col, row| Cell { col, row };
        // A row along the bottom, k0..k6 in columns 0-6; a box comes down over columns 3-4.
        let mut l = MonitorLayout::default();
        for (i, key) in k.iter().enumerate() {
            l.loose.insert(key.clone(), cell(i as i32, 4));
        }
        let b = create_box(&mut l, Rect::new(225.0, 200.0, 150.0, 300.0), "B", BoxStyle::default());
        assert!(settle(&mut l, &g));
        // Not up out of the row: the row moves along to the right (left would run off the grid).
        let row: Vec<Cell> = k.iter().map(|x| l.loose[x]).collect();
        assert_eq!(row, vec![cell(0, 4), cell(1, 4), cell(2, 4), cell(5, 4), cell(6, 4), cell(7, 4), cell(8, 4)]);
        // The box goes: all back where they were.
        assert!(delete_box(&mut l, b, &g));
        assert!(settle(&mut l, &g));
        let row: Vec<Cell> = k.iter().map(|x| l.loose[x]).collect();
        assert_eq!(row, (0..7).map(|c| cell(c, 4)).collect::<Vec<_>>());
    }

    #[test]
    fn insert_in_a_row_moves_the_rest_of_the_row_along() {
        let g = grid();
        let k = keys(5);
        let cell = |col, row| Cell { col, row };
        let mut l = MonitorLayout::default();
        for i in 0..4 {
            l.loose.insert(k[i].clone(), cell(i as i32, 4));
        }
        l.loose.insert(k[4].clone(), cell(7, 0));
        insert_along(&mut l, &k[4..5], cell(2, 4), (1, 0), &g);
        let got: Vec<Cell> = k.iter().map(|x| l.loose[x]).collect();
        assert_eq!(got, vec![cell(0, 4), cell(1, 4), cell(3, 4), cell(4, 4), cell(2, 4)]);
        // In a column, at the bottom of the grid: the icon there does not wrap to the top of the
        // next column; it takes the nearest free cell.
        let mut l = MonitorLayout::default();
        l.loose.insert(k[0].clone(), cell(3, 3));
        l.loose.insert(k[1].clone(), cell(3, 4));
        insert_along(&mut l, &k[2..3], cell(3, 4), (0, 1), &g);
        assert_eq!(l.loose[&k[2]], cell(3, 4));
        assert_eq!(l.loose[&k[0]], cell(3, 3));
        let c = l.loose[&k[1]];
        assert!((c.col - 3).abs() <= 1 && c.row >= 3, "{c:?}");
    }

    #[test]
    fn insert_between_icons_moves_the_rest_along() {
        let g = grid();
        let mut l = MonitorLayout::default();
        let k = keys(6);
        // a column: k0..k2 at rows 0-2, row 3 free, k3 at row 4; k4 elsewhere; k5 dragged in
        for (i, r) in [0, 1, 2, 4].iter().enumerate() {
            l.loose.insert(k[i].clone(), Cell { col: 0, row: *r });
        }
        l.loose.insert(k[4].clone(), Cell { col: 3, row: 0 });
        l.loose.insert(k[5].clone(), Cell { col: 5, row: 0 });
        insert_in_grid(&mut l, &k[5..6], Cell { col: 0, row: 1 }, &g);
        assert_eq!(l.loose[&k[0]], Cell { col: 0, row: 0 });
        assert_eq!(l.loose[&k[5]], Cell { col: 0, row: 1 });
        assert_eq!(l.loose[&k[1]], Cell { col: 0, row: 2 });
        assert_eq!(l.loose[&k[2]], Cell { col: 0, row: 3 });
        // the gap took the last one: the icon after it stays
        assert_eq!(l.loose[&k[3]], Cell { col: 0, row: 4 });
        assert_eq!(l.loose[&k[4]], Cell { col: 3, row: 0 });
        assert_eq!(l.loose.len(), 6);
    }

    #[test]
    fn reconcile_places_each_item_once() {
        let mut l = MonitorLayout::default();
        let k = keys(12);
        assert!(reconcile(&mut l, &k, &grid()));
        assert_eq!(l.loose.len(), 12);
        let cells: HashSet<Cell> = l.loose.values().copied().collect();
        assert_eq!(cells.len(), 12);
        assert_eq!(l.loose[&k[0]], Cell { col: 0, row: 0 });
        assert_eq!(l.loose[&k[5]], Cell { col: 1, row: 0 });
        assert!(!reconcile(&mut l, &k, &grid()));
        // removing items forgets them
        assert!(reconcile(&mut l, &k[..3], &grid()));
        assert_eq!(l.loose.len(), 3);
    }

    #[test]
    fn boxes_block_grid_cells_and_hold_items() {
        let g = grid();
        let mut l = MonitorLayout::default();
        let b = create_box(&mut l, Rect::new(0.0, 0.0, 150.0, 200.0), "Dokumenty", BoxStyle::default());
        let k = keys(4);
        reconcile(&mut l, &k, &g);
        for c in l.loose.values() {
            assert!(!g.cell_rect(*c).inflate(-4.0).intersects(&l.boxes[0].rect), "cell {c:?} under the box");
        }
        move_to_box(&mut l, &k[..2], b, None);
        assert_eq!(l.boxes[0].tabs[0].items, k[..2].to_vec());
        assert_eq!(placed_keys(&l).len(), 4);
        assert_eq!(
            place_of(&l, &k[0]),
            Some(Place::Box {
                box_id: b,
                tab: l.boxes[0].tabs[0].id
            })
        );
        // reorder inside the box
        move_to_box(&mut l, &k[1..2], b, Some(0));
        assert_eq!(l.boxes[0].tabs[0].items, vec![k[1].clone(), k[0].clone()]);
        // back to the grid
        move_to_grid(&mut l, &k[..1], Cell { col: 5, row: 2 }, &g);
        assert_eq!(l.loose[&k[0]], Cell { col: 5, row: 2 });
        assert_eq!(placed_keys(&l).len(), 4);
    }

    #[test]
    fn combine_split_delete() {
        let g = grid();
        let mut l = MonitorLayout::default();
        let a = create_box(&mut l, Rect::new(0.0, 0.0, 150.0, 200.0), "A", BoxStyle::default());
        let b = create_box(&mut l, Rect::new(300.0, 0.0, 150.0, 200.0), "B", BoxStyle::default());
        let k = keys(3);
        reconcile(&mut l, &k, &g);
        move_to_box(&mut l, &k[..1], a, None);
        move_to_box(&mut l, &k[1..2], b, None);
        assert!(combine(&mut l, b, a));
        assert_eq!(l.boxes.len(), 1);
        assert_eq!(l.boxes[0].tabs.len(), 2);
        assert_eq!(l.boxes[0].title(), "B");
        let c = split_tab(&mut l, a, 1, Rect::new(400.0, 0.0, 150.0, 200.0)).unwrap();
        assert_eq!(l.boxes.len(), 2);
        assert!(delete_box(&mut l, c, &g));
        assert_eq!(placed_keys(&l).len(), 3);
        assert!(l.loose.contains_key(&k[1]));
    }

    #[test]
    fn undo_redo() {
        let mut l = MonitorLayout::default();
        let mut h = History::default();
        h.record(&l);
        create_box(&mut l, Rect::new(0.0, 0.0, 10.0, 10.0), "x", BoxStyle::default());
        assert!(h.undo(&mut l).is_some());
        assert!(l.boxes.is_empty());
        assert!(h.redo(&mut l).is_some());
        assert_eq!(l.boxes.len(), 1);
    }

    #[test]
    fn group_drop_keeps_shape() {
        let g = Grid::new(Rect::new(0.0, 0.0, 750.0, 980.0), 75.0, 98.0);
        let mut l = MonitorLayout::default();
        let ks = keys(4);
        reconcile(&mut l, &ks, &g); // p:0..3 at (0,0) (0,1) (0,2) (0,3)
                                    // Move p:0 and p:1 three columns right; p:2 wants (3,0) too: taken, so the nearest free.
        let moves = vec![
            (ks[0].clone(), Cell { col: 3, row: 0 }),
            (ks[1].clone(), Cell { col: 3, row: 1 }),
            (ks[2].clone(), Cell { col: 3, row: 0 }),
        ];
        move_to_grid_at(&mut l, &moves, &g);
        assert_eq!(l.loose[&ks[0]], Cell { col: 3, row: 0 });
        assert_eq!(l.loose[&ks[1]], Cell { col: 3, row: 1 });
        let c2 = l.loose[&ks[2]];
        assert!(c2 != Cell { col: 3, row: 0 } && c2 != Cell { col: 3, row: 1 } && c2 != Cell { col: 0, row: 3 });
        assert!((c2.col - 3).abs() <= 1 && c2.row <= 1, "{c2:?}");
        assert_eq!(l.loose[&ks[3]], Cell { col: 0, row: 3 });
        // Off-grid wants are clamped to the nearest free cell.
        move_to_grid_at(&mut l, &[(ks[3].clone(), Cell { col: 40, row: -2 })], &g);
        assert_eq!(l.loose[&ks[3]], Cell { col: 9, row: 0 });
    }

    #[test]
    fn box_moved_over_icons_pushes_them_out() {
        let g = Grid::new(Rect::new(0.0, 0.0, 750.0, 980.0), 75.0, 98.0);
        let mut l = MonitorLayout::default();
        let ks = keys(3);
        reconcile(&mut l, &ks, &g); // (0,0) (0,1) (0,2)
        create_box(&mut l, Rect::new(0.0, 100.0, 140.0, 190.0), "B", BoxStyle::default());
        assert!(evict_from_boxes(&mut l, &g));
        assert_eq!(l.loose[&ks[0]], Cell { col: 0, row: 0 });
        let cells: HashSet<Cell> = l.loose.values().copied().collect();
        assert_eq!(cells.len(), 3);
        for c in l.loose.values() {
            assert!(!blocked_by_boxes(&l, &g, *c), "{c:?} still under the box");
        }
        assert!(!evict_from_boxes(&mut l, &g));
    }

    #[test]
    fn pushed_icons_go_back_when_the_box_leaves() {
        let g = Grid::new(Rect::new(0.0, 0.0, 750.0, 980.0), 75.0, 98.0);
        let mut l = MonitorLayout::default();
        let ks = keys(3);
        reconcile(&mut l, &ks, &g); // (0,0) (0,1) (0,2)
        let b = create_box(&mut l, Rect::new(300.0, 0.0, 140.0, 190.0), "B", BoxStyle::default());
        assert!(!settle(&mut l, &g));
        // the box moves over the first column: its icons push the column down and they all
        // remember their cells (the one pushed along too)
        set_box_rect(&mut l, b, Rect::new(0.0, 0.0, 70.0, 190.0));
        assert!(settle(&mut l, &g));
        assert_eq!(l.homes.len(), 3);
        assert_eq!(l.loose[&ks[2]], Cell { col: 0, row: 4 });
        assert_ne!(l.loose[&ks[0]], Cell { col: 0, row: 0 });
        // it moves away again: they go back
        set_box_rect(&mut l, b, Rect::new(300.0, 0.0, 140.0, 190.0));
        assert!(settle(&mut l, &g));
        assert_eq!(l.loose[&ks[0]], Cell { col: 0, row: 0 });
        assert_eq!(l.loose[&ks[1]], Cell { col: 0, row: 1 });
        assert_eq!(l.loose[&ks[2]], Cell { col: 0, row: 2 });
        assert!(l.homes.is_empty());
        // an icon the user moves in between forgets its old cell
        set_box_rect(&mut l, b, Rect::new(0.0, 0.0, 70.0, 190.0));
        settle(&mut l, &g);
        move_to_grid_at(&mut l, &[(ks[0].clone(), Cell { col: 5, row: 5 })], &g);
        set_box_rect(&mut l, b, Rect::new(300.0, 0.0, 140.0, 190.0));
        settle(&mut l, &g);
        assert_eq!(l.loose[&ks[0]], Cell { col: 5, row: 5 });
        assert_eq!(l.loose[&ks[1]], Cell { col: 0, row: 1 });
    }

    #[test]
    fn arrange_fills_columns_around_boxes() {
        let g = grid(); // 10 x 5
        let mut l = MonitorLayout::default();
        let ks = keys(4);
        reconcile(&mut l, &ks, &g);
        create_box(&mut l, Rect::new(0.0, 0.0, 70.0, 190.0), "B", BoxStyle::default()); // covers (0,0) (0,1)
        let rev: Vec<ItemKey> = ks.iter().rev().cloned().collect();
        assert!(arrange_loose(&mut l, &g, &rev));
        assert_eq!(l.loose[&ks[3]], Cell { col: 0, row: 2 });
        assert_eq!(l.loose[&ks[2]], Cell { col: 0, row: 3 });
        assert_eq!(l.loose[&ks[0]], Cell { col: 1, row: 0 });
    }

    #[test]
    fn snapping() {
        let b = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let r = snap(Rect::new(6.0, 300.0, 200.0, 100.0), &[Rect::new(500.0, 0.0, 100.0, 100.0)], &b, 10.0);
        assert_eq!(r.x, 0.0);
        let r = snap(Rect::new(292.0, 300.0, 200.0, 100.0), &[Rect::new(500.0, 0.0, 100.0, 100.0)], &b, 10.0);
        assert_eq!(r.right(), 500.0);
    }
}
