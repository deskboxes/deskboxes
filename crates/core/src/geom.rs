//! Geometry in DIPs (device-independent pixels, 96 per inch).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.right() && p.y < self.bottom()
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
    /// Normalised rectangle spanning two points (rubber band).
    pub fn from_points(a: Pt, b: Pt) -> Self {
        let (x0, x1) = if a.x <= b.x { (a.x, b.x) } else { (b.x, a.x) };
        let (y0, y1) = if a.y <= b.y { (a.y, b.y) } else { (b.y, a.y) };
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
    pub fn offset(&self, dx: f32, dy: f32) -> Self {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
    pub fn inflate(&self, d: f32) -> Self {
        Rect::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }
    /// Keep the rectangle inside `bounds` (moving it, shrinking only if it is larger).
    pub fn clamp_into(&self, bounds: &Rect) -> Self {
        let w = self.w.min(bounds.w);
        let h = self.h.min(bounds.h);
        let x = self.x.max(bounds.x).min(bounds.right() - w);
        let y = self.y.max(bounds.y).min(bounds.bottom() - h);
        Rect::new(x, y, w, h)
    }
}

/// Grid cell on the loose desktop grid (column-major like Explorer's auto-arrange).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Cell {
    pub col: i32,
    pub row: i32,
}

/// The loose-icon grid of one monitor's work area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub origin: Pt,
    pub cell_w: f32,
    pub cell_h: f32,
    pub cols: i32,
    pub rows: i32,
}

impl Grid {
    /// `work` = work area in DIPs relative to the monitor, `cell` = icon spacing in DIPs.
    pub fn new(work: Rect, cell_w: f32, cell_h: f32) -> Self {
        let cols = ((work.w / cell_w).floor() as i32).max(1);
        let rows = ((work.h / cell_h).floor() as i32).max(1);
        Grid { origin: Pt { x: work.x, y: work.y }, cell_w, cell_h, cols, rows }
    }
    pub fn cell_rect(&self, c: Cell) -> Rect {
        Rect::new(self.origin.x + c.col as f32 * self.cell_w, self.origin.y + c.row as f32 * self.cell_h, self.cell_w, self.cell_h)
    }
    pub fn cell_at(&self, p: Pt) -> Cell {
        let col = ((p.x - self.origin.x) / self.cell_w).floor() as i32;
        let row = ((p.y - self.origin.y) / self.cell_h).floor() as i32;
        Cell { col: col.clamp(0, self.cols - 1), row: row.clamp(0, self.rows - 1) }
    }
    /// Column-major order (top to bottom, then the next column), Explorer's auto-arrange order.
    pub fn nth(&self, n: i32) -> Cell {
        Cell { col: n / self.rows, row: n % self.rows }
    }
    pub fn index_of(&self, c: Cell) -> i32 {
        c.col * self.rows + c.row
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_order_is_column_major() {
        let g = Grid::new(Rect::new(0.0, 0.0, 300.0, 200.0), 75.0, 100.0);
        assert_eq!((g.cols, g.rows), (4, 2));
        assert_eq!(g.nth(0), Cell { col: 0, row: 0 });
        assert_eq!(g.nth(1), Cell { col: 0, row: 1 });
        assert_eq!(g.nth(2), Cell { col: 1, row: 0 });
        assert_eq!(g.index_of(Cell { col: 1, row: 1 }), 3);
        assert_eq!(g.cell_at(Pt { x: 80.0, y: 150.0 }), Cell { col: 1, row: 1 });
    }
    #[test]
    fn rect_clamp() {
        let b = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(Rect::new(90.0, -5.0, 20.0, 20.0).clamp_into(&b), Rect::new(80.0, 0.0, 20.0, 20.0));
    }
}
