//! How a box shows its icons, as File Explorer's View menu has it: icons in a grid at a size of
//! the box's own (small, medium, large, extra large, or any step of Ctrl + mouse wheel), a list,
//! details (name, date modified, type, size) or tiles. Loose desktop icons keep the desktop's
//! own size and grid.
//!
//! Icons at a size other than the desktop's are extracted again by the worker at that size
//! (`Req::IconsAt`), once per icon and size; until they arrive the desktop's image is scaled.

use crate::app::{App, PAD};
use crate::worker::Req;
use deskboxes_core::geom::Rect;
use deskboxes_core::model::{View, BOX_WHEEL_SIZES};
use deskboxes_core::*;
use deskboxes_render::TextLayout;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

/// List and Details rows, Tiles: their heights in DIPs.
pub const ROW_H: f32 = 22.0;
pub const TILE_H: f32 = 64.0;
/// The column header of a Details box.
pub const DETAILS_HEAD: f32 = 24.0;
/// The column widths a List and a Tiles box aim for (they fill the box).
const LIST_COL: f32 = 230.0;
const TILE_COL: f32 = 250.0;
/// Details columns: date, type, size (from the right).
const COL_DATE: f32 = 132.0;
const COL_TYPE: f32 = 128.0;
const COL_SIZE: f32 = 76.0;

/// Cell size, view and icon size of a box (or of the desktop, for loose icons).
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub view: View,
    pub cell_w: f32,
    pub cell_h: f32,
    /// Icon size in DIPs, and in pixels (the bitmap wanted).
    pub icon: f32,
    pub px: u32,
}

impl Metrics {
    /// A row view: the name is one line to the right of the icon.
    pub fn row(&self) -> bool {
        matches!(self.view, View::List | View::Details | View::Tiles)
    }
}

/// Details columns in a row `w` wide: the name's width and which of date, type, size show (their
/// left edges relative to the row), the narrower the box the fewer.
pub struct Cols {
    pub name_w: f32,
    pub date: Option<f32>,
    pub kind: Option<f32>,
    pub size: Option<f32>,
}

pub fn details_cols(w: f32) -> Cols {
    let mut right = w - 4.0;
    let size = (w >= 250.0).then(|| {
        right -= COL_SIZE;
        right
    });
    let kind = (w >= 520.0).then(|| {
        right -= COL_TYPE;
        right
    });
    let date = (w >= 390.0).then(|| {
        right -= COL_DATE;
        right
    });
    Cols { name_w: (right - 26.0 - 8.0).max(20.0), date, kind, size }
}

/// Per-size icons fetched from the worker, Explorer's type names, the Details and Tiles texts.
#[derive(Default)]
pub struct ViewState {
    /// (desktop icon id, px) -> the icon id at that size.
    pub sized: HashMap<(u32, u32), u32>,
    pub pending: HashSet<(u32, u32)>,
    types: RefCell<HashMap<String, String>>,
    /// (item, column, width) -> its text laid out (0 date, 1 type, 2 size, 3 the tile's lines).
    texts: RefCell<HashMap<(ItemKey, u8), String>>,
    /// Ctrl+wheel over a box: the turn not used yet (a notch is 120; touchpads send less).
    pub wheel: i32,
    /// Game covers (the Covers view).
    pub covers: crate::covers::Covers,
}

impl ViewState {
    /// Everything at other sizes goes (the desktop's icons were made again).
    pub fn forget_icons(&mut self) {
        self.sized.clear();
        self.pending.clear();
    }
    pub fn forget_texts(&self) {
        self.texts.borrow_mut().clear();
    }
}

/// A view and icon size from a menu entry ("xl", "large", ... "desktop").
pub fn preset(id: &str) -> Option<(&'static str, u32)> {
    Some(match id {
        "xl" => ("", 256),
        "large" => ("", 96),
        "medium" => ("", 48),
        "small" => ("", 32),
        "list" => ("list", 0),
        "details" => ("details", 0),
        "tiles" => ("tiles", 0),
        "covers" => ("covers", 0),
        "desktop" => ("", 0),
        _ => return None,
    })
}

/// The menu entry a box's view is (for the radio mark).
pub fn preset_of(b: &BoxModel) -> &'static str {
    match b.view() {
        View::List => "list",
        View::Details => "details",
        View::Tiles => "tiles",
        View::Covers => "covers",
        View::Icons => match b.icon_size {
            0 => "desktop",
            256 => "xl",
            96 => "large",
            48 => "medium",
            32 => "small",
            _ => "",
        },
    }
}

impl App {
    pub fn metrics_of(&self, id: Option<u32>) -> Metrics {
        self.metrics(id.and_then(|i| self.box_by_id(i)))
    }

    /// The cells, view and icon size of box `b` (None: the desktop's grid).
    pub fn metrics(&self, b: Option<&BoxModel>) -> Metrics {
        let desk = Metrics { view: View::Icons, cell_w: self.grid.cell_w, cell_h: self.grid.cell_h, icon: self.icon_dip, px: self.icon_px };
        let Some(b) = b else { return desk };
        let avail = (b.rect.w - 2.0 * PAD).max(40.0);
        let px = |dip: f32| (dip * self.scale).round().max(1.0) as u32;
        match b.view() {
            View::Icons if b.icon_size == 0 => desk,
            View::Icons => {
                let icon = b.icon_size as f32;
                // Explorer's spacing (75 × 100 DIPs around a 48 DIP icon): the name keeps its two
                // lines and room to the sides grows with the icon.
                Metrics { view: View::Icons, cell_w: (icon + 28.0).max(self.grid.cell_w), cell_h: icon + 20.0 + 2.0 * self.line_h, icon, px: px(icon) }
            }
            View::List => {
                let cols = (avail / LIST_COL).floor().max(1.0);
                Metrics { view: View::List, cell_w: (avail / cols).floor(), cell_h: ROW_H, icon: 16.0, px: px(16.0) }
            }
            View::Details => Metrics { view: View::Details, cell_w: avail.floor(), cell_h: ROW_H, icon: 16.0, px: px(16.0) },
            View::Tiles => {
                let cols = (avail / TILE_COL).floor().max(1.0);
                Metrics { view: View::Tiles, cell_w: (avail / cols).floor(), cell_h: TILE_H, icon: 48.0, px: px(48.0) }
            }
            View::Covers => {
                // Columns of covers spread over the box's width (as Tiles fill it).
                let (cw, ch) = crate::covers::cell_size(self.line_h);
                let cols = (avail / cw).floor().max(1.0);
                Metrics { view: View::Covers, cell_w: (avail / cols).floor().max(cw), cell_h: ch, icon: 48.0, px: px(48.0) }
            }
        }
    }

    /// Icon rect and the name's rect in a cell, for a row view.
    pub fn row_parts(&self, cell: Rect, m: &Metrics) -> (Rect, Rect) {
        match m.view {
            View::Tiles => {
                let icon = Rect::new(cell.x + 8.0, cell.y + (cell.h - m.icon) / 2.0, m.icon, m.icon);
                let x = icon.right() + 10.0;
                let lines = 3.0 * self.line_h;
                (icon, Rect::new(x, cell.y + (cell.h - lines) / 2.0, (cell.right() - x - 6.0).max(10.0), self.line_h))
            }
            _ => {
                let icon = Rect::new(cell.x + 6.0, cell.y + (cell.h - m.icon) / 2.0, m.icon, m.icon);
                let x = icon.right() + 6.0;
                let w = if m.view == View::Details { details_cols(cell.w).name_w } else { cell.right() - x - 6.0 };
                (icon, Rect::new(x, cell.y + (cell.h - self.line_h) / 2.0 - 1.0, w.max(10.0), self.line_h))
            }
        }
    }

    /// The name's layout key: width, whole name (two lines else), one line to the right.
    pub fn label_spec(&self, k: &ItemKey, cell: Rect, m: &Metrics) -> (f32, bool, bool) {
        if m.row() {
            let (_, l) = self.row_parts(cell, m);
            (l.w, false, true)
        } else {
            (cell.w - 4.0, self.is_full_label(k), false)
        }
    }

    /// Key of a label in `labels` / `label_bmps`: a one-line name (row views) has a negative width.
    pub fn lkey(k: &ItemKey, w: f32, full: bool, row: bool) -> (ItemKey, bool, i32) {
        (k.clone(), full, if row { -(w as i32) - 1 } else { w as i32 })
    }

    /// A one-line name, left-aligned and cut with an ellipsis (List, Details, Tiles).
    pub fn row_label(&mut self, key: &ItemKey, w: f32) -> Option<TextLayout> {
        let k = Self::lkey(key, w, false, true);
        if let Some(l) = self.labels.get(&k) {
            return Some(l.clone());
        }
        let text = self.items.get(key)?.display_name.clone();
        let l = self.gpu.label(&self.label_tf, &text, w, self.line_h, 1, false).ok()?;
        self.labels.insert(k, l.clone());
        Some(l)
    }

    /// The icon image for `it` at `m`'s size: the one made at that size, else the desktop's
    /// (scaled), with the id its recolored copies are kept under.
    pub fn icon_bitmap(&self, it: &Item, m: &Metrics) -> Option<(u32, &deskboxes_render::Bitmap)> {
        if m.px != self.icon_px {
            if let Some(id) = self.views.sized.get(&(it.icon, m.px)) {
                if let Some(b) = self.bitmaps.get(id) {
                    return Some((*id, b));
                }
            }
        }
        self.bitmaps.get(&it.icon).map(|b| (it.icon, b))
    }

    /// Ask the worker for the icons in view that boxes show at another size (once each).
    pub fn request_sized_icons(&mut self, cells: &[(ItemKey, Rect, Option<u32>)]) {
        let mut want: HashMap<u32, Vec<(u32, String, u32)>> = HashMap::new();
        let mut ms: HashMap<u32, Metrics> = HashMap::new();
        let mut covers: Vec<&ItemKey> = vec![];
        for (k, _, bx) in cells {
            let Some(id) = bx else { continue };
            let m = *ms.entry(*id).or_insert_with(|| self.metrics_of(Some(*id)));
            if m.view == View::Covers {
                covers.push(k);
            }
            if m.px == self.icon_px {
                continue;
            }
            let Some(it) = self.items.get(k) else { continue };
            let key = (it.icon, m.px);
            if self.views.sized.contains_key(&key) || self.views.pending.contains(&key) {
                continue;
            }
            self.views.pending.insert(key);
            want.entry(m.px).or_default().push((it.icon, it.parsing_name.clone(), it.attrs));
        }
        for (px, items) in want {
            log::info!("icons at {px} px: {} asked", items.len());
            let _ = self.worker.send(Req::IconsAt { px, items });
        }
        if !covers.is_empty() {
            self.request_covers(covers.into_iter());
        }
    }

    /// Icons the worker made at another size.
    pub fn apply_sized_icons(&mut self, px: u32, map: Vec<(u32, u32)>, icons: Vec<(u32, deskboxes_core::snapshot::IconImage)>) {
        for (id, img) in icons {
            match self.gpu.bitmap(&img) {
                Ok(b) => {
                    self.bitmaps.insert(id, b);
                }
                Err(e) => log::warn!("icon upload: {e}"),
            }
        }
        for (base, id) in map {
            self.views.pending.remove(&(base, px));
            self.views.sized.insert((base, px), id);
        }
        self.invalidate();
    }

    /// Box `id` shows its icons as `view` at `size` ("" and 0: as the desktop). Not an undo step
    /// (as in File Explorer); a box too narrow for one column grows to fit it.
    pub fn set_box_view(&mut self, ids: &[u32], view: &str, size: u32) {
        self.apply_box_view(ids, view, size);
        log::info!("view of {} box(es): '{view}' {size}", ids.len());
        self.layout_changed();
    }

    /// "Icons in all boxes": every box on every page.
    pub fn set_all_box_views(&mut self, view: &str, size: u32) {
        let shown = self.layout.page;
        let mut n = 0;
        for p in deskboxes_core::pages::ids(&self.layout) {
            if p != shown && !deskboxes_core::pages::switch_to(&mut self.layout, p) {
                continue;
            }
            let ids: Vec<u32> = self.layout.boxes.iter().map(|b| b.id).collect();
            n += ids.len();
            self.apply_box_view(&ids, view, size);
            deskboxes_core::pages::switch_to(&mut self.layout, shown);
        }
        log::info!("view of all {n} box(es): '{view}' {size}");
        self.layout_changed();
    }

    /// Boxes `ids` on the page shown get `view` at `size` (wide enough for one column of it).
    fn apply_box_view(&mut self, ids: &[u32], view: &str, size: u32) {
        let bounds = self.work;
        for id in ids {
            let Some(b) = self.layout.boxes.iter_mut().find(|b| b.id == *id) else { continue };
            b.view = view.to_string();
            b.icon_size = size;
        }
        for id in ids {
            let m = self.metrics_of(Some(*id));
            let Some(b) = self.layout.boxes.iter_mut().find(|b| b.id == *id) else { continue };
            let need = m.cell_w.min(bounds.w - 2.0 * PAD) + 2.0 * PAD;
            if b.rect.w < need {
                b.rect.w = need;
                if b.rect.right() > bounds.right() {
                    b.rect.x = (bounds.right() - b.rect.w).max(bounds.x);
                }
            }
        }
        for id in ids {
            let max = self.box_by_id(*id).map(|b| self.max_scroll(b)).unwrap_or(0.0);
            if let Some(s) = self.scroll.get_mut(id) {
                *s = s.min(max);
            }
            self.scroll_from.remove(id);
        }
    }

    /// Ctrl + mouse wheel over a box: the next view in Explorer's order (details, list, then
    /// icons smallest to largest).
    pub fn wheel_box_view(&mut self, id: u32, delta: i16) {
        self.views.wheel += delta as i32;
        let steps = self.views.wheel / 120;
        self.views.wheel -= steps * 120;
        if steps == 0 {
            return;
        }
        let Some(b) = self.box_by_id(id) else { return };
        // Steps: 0 details, 1 list, 2.. the icon sizes.
        let cur = match b.view() {
            View::Details => 0,
            View::List => 1,
            View::Tiles | View::Covers => 2 + BOX_WHEEL_SIZES.iter().position(|s| *s == 48).unwrap_or(2),
            View::Icons => {
                let dip = if b.icon_size == 0 { self.icon_dip.round() as u32 } else { b.icon_size };
                2 + BOX_WHEEL_SIZES.iter().enumerate().min_by_key(|(_, s)| s.abs_diff(dip)).map(|(i, _)| i).unwrap_or(2)
            }
        };
        let last = 1 + BOX_WHEEL_SIZES.len();
        let next = (cur as i32 + steps).clamp(0, last as i32) as usize;
        if next == cur {
            return;
        }
        match next {
            0 => self.set_box_view(&[id], "details", 0),
            1 => self.set_box_view(&[id], "list", 0),
            n => self.set_box_view(&[id], "", BOX_WHEEL_SIZES[n - 2]),
        }
    }

    /// Explorer's type name for an item ("PDF Document", "File folder", "Shortcut").
    pub fn type_name(&self, it: &Item) -> String {
        if it.kind == ItemKind::Virtual {
            return String::new();
        }
        let folder = it.is_folder();
        let key = if folder { "<dir>".to_string() } else { it.ext.to_ascii_lowercase() };
        if let Some(s) = self.views.types.borrow().get(&key) {
            return s.clone();
        }
        let s = shell_type_name(if folder { "folder" } else if key.is_empty() { "file" } else { &key }, folder)
            .unwrap_or_else(|| if folder { deskboxes_i18n::t(self.lang, deskboxes_i18n::S::FileFolder).to_string() } else { String::new() });
        self.views.types.borrow_mut().insert(key, s.clone());
        s
    }

    /// One of an item's texts for Details (0 date, 1 type, 2 size) or its tile's lines (3), made
    /// once.
    pub fn item_text(&self, k: &ItemKey, col: u8) -> String {
        let key = (k.clone(), col);
        if let Some(s) = self.views.texts.borrow().get(&key) {
            return s.clone();
        }
        let Some(it) = self.items.get(k) else { return String::new() };
        let text = match col {
            0 => format_date(it.modified),
            1 => self.type_name(it),
            2 if it.is_folder() || it.kind == ItemKind::Virtual => String::new(),
            2 => format_kb(it.size),
            _ => {
                let mut v = vec![self.type_name(it)];
                if !it.is_folder() && it.kind != ItemKind::Virtual {
                    v.push(format_bytes(it.size));
                }
                v.retain(|s| !s.is_empty());
                v.join("\n")
            }
        };
        self.views.texts.borrow_mut().insert(key, text.clone());
        text
    }
}
fn rf(x: Rect) -> deskboxes_render::RectF {
    deskboxes_render::RectF { left: x.x, top: x.y, right: x.right(), bottom: x.bottom() }
}

impl App {
    fn text_at(&self, dc: &deskboxes_render::Dc, text: &str, x: f32, y: f32, w: f32, lines: u32, right: bool, c: deskboxes_render::Color, shadow: bool) -> deskboxes_render::R<()> {
        if text.is_empty() || w < 8.0 {
            return Ok(());
        }
        let l = self.gpu.label(&self.label_tf, text, w, self.line_h, lines, false)?;
        let x = if right { x + (w - deskboxes_render::text_width(&l)).max(0.0) } else { x };
        if shadow {
            deskboxes_render::draw_text_shadowed(dc, &l, x, y, c)
        } else {
            deskboxes_render::draw_text(dc, &l, x, y, c)
        }
    }

    /// Details: date, type and size in their columns; Tiles: type and size under the name.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_item_texts(&self, dc: &deskboxes_render::Dc, k: &ItemKey, cell: Rect, label: Rect, m: &Metrics, c: deskboxes_render::Color, shadow: bool) -> deskboxes_render::R<()> {
        if self.privacy_on() {
            return Ok(());
        }
        if m.view == View::Tiles {
            return self.text_at(dc, &self.item_text(k, 3), label.x, label.y + self.line_h, label.w, 2, false, c, shadow);
        }
        let cols = details_cols(cell.w);
        if let Some(x) = cols.date {
            self.text_at(dc, &self.item_text(k, 0), cell.x + x, label.y, COL_DATE - 10.0, 1, false, c, shadow)?;
        }
        if let Some(x) = cols.kind {
            self.text_at(dc, &self.item_text(k, 1), cell.x + x, label.y, COL_TYPE - 10.0, 1, false, c, shadow)?;
        }
        if let Some(x) = cols.size {
            self.text_at(dc, &self.item_text(k, 2), cell.x + x, label.y, COL_SIZE - 10.0, 1, true, c, shadow)?;
        }
        Ok(())
    }

    /// The column names over a Details box's rows, `dy` below where the box is laid out.
    pub fn paint_details_head(&self, dc: &deskboxes_render::Dc, b: &BoxModel, m: &Metrics, text: deskboxes_render::Color, dy: f32, a: f32) -> deskboxes_render::R<()> {
        use deskboxes_i18n::{t, S};
        let y = b.rect.y + crate::app::header_h() + crate::az::bar_h(b) + dy;
        let x0 = b.rect.x + (b.rect.w - m.cell_w) / 2.0;
        let c = deskboxes_render::Color { a: text.a * 0.62 * a, ..text };
        let ty = y + (DETAILS_HEAD - self.line_h) / 2.0 - 1.0;
        let cols = details_cols(m.cell_w);
        let l = self.lang;
        self.text_at(dc, t(l, S::ColName), x0 + 28.0, ty, cols.name_w, 1, false, c, false)?;
        if let Some(x) = cols.date {
            self.text_at(dc, t(l, S::ColModified), x0 + x, ty, COL_DATE - 10.0, 1, false, c, false)?;
        }
        if let Some(x) = cols.kind {
            self.text_at(dc, t(l, S::ColType), x0 + x, ty, COL_TYPE - 10.0, 1, false, c, false)?;
        }
        if let Some(x) = cols.size {
            self.text_at(dc, t(l, S::ColSize), x0 + x, ty, COL_SIZE - 10.0, 1, true, c, false)?;
        }
        let sep = deskboxes_render::Color { a: text.a * 0.14 * a, ..text };
        deskboxes_render::fill_rect(dc, rf(Rect::new(b.rect.x + 8.0, y + DETAILS_HEAD - 1.0, b.rect.w - 16.0, 1.0)), sep)
    }
}

/// SHGetFileInfo's type name for an extension (no file is touched).
fn shell_type_name(ext: &str, folder: bool) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
    use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_TYPENAME, SHGFI_USEFILEATTRIBUTES};
    let name: Vec<u16> = ext.encode_utf16().chain(std::iter::once(0)).collect();
    let mut fi = SHFILEINFOW::default();
    let attrs = if folder { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
    let r = unsafe { SHGetFileInfoW(PCWSTR(name.as_ptr()), attrs, Some(&mut fi), std::mem::size_of::<SHFILEINFOW>() as u32, SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES) };
    if r == 0 {
        return None;
    }
    let n = fi.szTypeName.iter().position(|c| *c == 0).unwrap_or(fi.szTypeName.len());
    let s = String::from_utf16_lossy(&fi.szTypeName[..n]);
    (!s.is_empty()).then_some(s)
}

fn wstr(buf: &[u16]) -> String {
    let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// "12/11/2026 14:05" in the user's own format, as Explorer's Date modified.
fn format_date(ft: u64) -> String {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::UI::Shell::SHFormatDateTimeW;
    if ft == 0 {
        return String::new();
    }
    let f = FILETIME { dwLowDateTime: ft as u32, dwHighDateTime: (ft >> 32) as u32 };
    // FDTF_SHORTDATE | FDTF_SHORTTIME
    let mut flags: u32 = 0x2 | 0x1;
    let mut buf = [0u16; 64];
    let n = unsafe { SHFormatDateTimeW(&f, Some(&mut flags), &mut buf) };
    if n <= 0 {
        return String::new();
    }
    // Explorer's text has direction marks in it; they would show as boxes in some fonts.
    wstr(&buf).chars().filter(|c| !matches!(c, '\u{200e}' | '\u{200f}')).collect()
}

/// "1,234 KB", as Explorer's Size column.
fn format_kb(bytes: u64) -> String {
    let mut buf = [0u16; 64];
    unsafe {
        let _ = windows::Win32::UI::Shell::StrFormatKBSizeW(bytes as i64, &mut buf);
    }
    wstr(&buf)
}

/// "1.20 MB", as Explorer's tiles.
fn format_bytes(bytes: u64) -> String {
    let mut buf = [0u16; 64];
    unsafe {
        let _ = windows::Win32::UI::Shell::StrFormatByteSizeW(bytes as i64, &mut buf);
    }
    wstr(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_drop_as_the_box_narrows() {
        let c = details_cols(700.0);
        assert!(c.date.is_some() && c.kind.is_some() && c.size.is_some());
        let c = details_cols(420.0);
        assert!(c.date.is_some() && c.kind.is_none() && c.size.is_some());
        let c = details_cols(200.0);
        assert!(c.date.is_none() && c.kind.is_none() && c.size.is_none());
        assert!(c.name_w > 100.0);
    }

    #[test]
    fn presets() {
        assert_eq!(preset("xl"), Some(("", 256)));
        assert_eq!(preset("details"), Some(("details", 0)));
        assert_eq!(preset("nope"), None);
    }
}
