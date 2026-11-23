//! Scene drawing (docs/ARCHITECTURE.md §4). Everything is in DIPs; the surface transform scales.
//! Phase 1 redraws the whole surface per frame (frames happen only on input or model changes).

use crate::app::{ease_out, header_h, radius, App, Drag, PAD, TAB_ANIM_S, TAB_GAP, TIP_FADE_S};
use deskboxes_core::pages;
use deskboxes_core::*;
use deskboxes_render::{self as r, rgba, Color, Dc, RectF};
use std::time::Instant;

fn rf(x: Rect) -> RectF {
    RectF {
        left: x.x,
        top: x.y,
        right: x.right(),
        bottom: x.bottom(),
    }
}

use crate::app::accent;

pub struct Theme {
    pub text: Color,
    pub sel_fill: Color,
    pub sel_border: Color,
    pub hover: Color,
}

impl App {
    fn box_theme(light: bool) -> Theme {
        if light {
            Theme {
                text: rgba(0, 0, 0, 0.9),
                sel_fill: rgba(0, 0, 0, 0.09),
                sel_border: rgba(0, 0, 0, 0.16),
                hover: rgba(0, 0, 0, 0.05),
            }
        } else {
            Theme {
                text: rgba(255, 255, 255, 1.0),
                sel_fill: rgba(255, 255, 255, 0.16),
                sel_border: rgba(255, 255, 255, 0.26),
                hover: rgba(255, 255, 255, 0.08),
            }
        }
    }

    /// Whether a box reads as light (dark text): a light tint color, or the light Windows theme
    /// for the automatic tint.
    fn box_is_light(&self, b: &BoxModel) -> bool {
        match App::box_color(b) {
            Some(c) => {
                let lum = (0.2126 * c.r as f32 + 0.7152 * c.g as f32 + 0.0722 * c.b as f32) / 255.0;
                lum > 0.6 && b.style.opacity >= 0.35
            }
            None => self.light && b.style.opacity >= 0.35,
        }
    }

    /// Explorer's desktop colors (white text with a shadow, translucent white selection).
    fn desktop_theme() -> Theme {
        Theme {
            text: rgba(255, 255, 255, 1.0),
            sel_fill: rgba(255, 255, 255, 0x40 as f32 / 255.0),
            sel_border: rgba(255, 255, 255, 0x60 as f32 / 255.0),
            hover: rgba(255, 255, 255, 0x20 as f32 / 255.0),
        }
    }

    /// Prepare label layouts (needs `&mut`) for everything that will be drawn.
    fn prepare(&mut self) {
        self.update_glides();
        if self.tab_anim.map(|(_, _, t0)| t0.elapsed().as_secs_f32() >= TAB_ANIM_S).unwrap_or(false) {
            self.tab_anim = None;
        }
        let cells = self.cells_in_view();
        for (k, cell, in_box) in &cells {
            self.prep_label(k, *cell, *in_box);
        }
        self.request_sized_icons(&cells);
        if self.prep_gen != self.items_gen {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(self.msg_hwnd), crate::app::T_PREP, 60, None);
            }
        }
    }

    /// A label's layout and its shadow bitmap, made once.
    fn prep_label(&mut self, k: &ItemKey, cell: Rect, in_box: Option<u32>) {
        let m = self.metrics_of(in_box);
        let (w, full, row) = self.label_spec(k, cell, &m);
        let _ = self.label_for(k, w, full, row);
        if !row {
            let _ = self.label_layout(k, w, false);
        }
        let light_box = in_box.and_then(|id| self.box_by_id(id)).map(|b| self.box_is_light(b)).unwrap_or(false);
        if light_box {
            return;
        }
        for f in [full, false] {
            let key = App::lkey(k, w, f, row);
            if !self.label_bmps.contains_key(&key) {
                if let Some(l) = self.labels.get(&key) {
                    let h = r::text_height(l);
                    match self.gpu.shadowed_label(l, w, h, self.scale, Self::desktop_theme().text) {
                        Ok(b) => {
                            self.label_bmps.insert(key, b);
                        }
                        Err(e) => log::warn!("label bitmap: {e}"),
                    }
                }
            }
        }
    }

    /// While idle: labels of icons out of view (scrolled away in a box), a few ms at a time, so
    /// scrolling later finds them ready.
    pub fn prep_more(&mut self) {
        let t = Instant::now();
        let gen = self.items_gen;
        for (k, cell, in_box) in self.visible_cells() {
            let m = self.metrics_of(in_box);
            let (w, _, row) = self.label_spec(&k, cell, &m);
            if self.labels.contains_key(&App::lkey(&k, w, false, row)) {
                continue;
            }
            self.prep_label(&k, cell, in_box);
            if t.elapsed().as_secs_f32() > 0.004 {
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(self.msg_hwnd), crate::app::T_PREP, 30, None);
                }
                return;
            }
        }
        self.prep_gen = gen;
    }

    pub fn draw(&mut self) {
        let t = Instant::now();
        self.step_roll_anim();
        self.vanish_animating();
        self.prepare();
        self.pages_before_frame();
        self.prepare_beside();
        let Some(surf) = self.surf.take() else { return };
        let res = surf.draw(self.scale, |dc| self.paint_frame(dc));
        self.surf = Some(surf);
        if let Err(e) = res.and_then(|_| self.gpu.commit()) {
            log::warn!("draw: {e}");
        }
        self.stats.draw_ms_last = t.elapsed().as_secs_f64() * 1000.0;
        if self.draw_times.len() < 100_000 {
            self.draw_times.push(self.stats.draw_ms_last as f32);
        }
        let now = Instant::now();
        if matches!(self.drag, Drag::Rubber { .. } | Drag::RightRubber { .. } | Drag::Items { .. }) {
            if self.drag_draw_times.len() < 100_000 {
                self.drag_draw_times.push(self.stats.draw_ms_last as f32);
                if let Some(prev) = self.last_drag_frame {
                    self.drag_intervals.push(now.duration_since(prev).as_secs_f32() * 1000.0);
                }
            }
            self.last_drag_frame = Some(now);
        } else {
            self.last_drag_frame = None;
        }
        self.frame_marks.push_back(now);
        while self.frame_marks.front().is_some_and(|f| now.duration_since(*f).as_secs_f32() > 1.0) {
            self.frame_marks.pop_front();
        }
        // Test harness only; not during a drag, so frame timings stay representative (the frame
        // after the drag ends writes it).
        if self.args.scene_out.is_some() && matches!(self.drag, Drag::None | Drag::Pending { .. }) {
            self.write_scene();
        }
        self.selection_changed_check();
        self.pages_after_frame();
        crate::anim::set(self.animating());
    }

    /// During a slide between pages: the labels of the page beside made too.
    fn prepare_beside(&mut self) {
        let Some(o) = self.page_frame().and_then(|f| f.other).filter(|o| *o != self.layout.page) else { return };
        let back = self.layout.page;
        if !pages::switch_to(&mut self.layout, o) {
            return;
        }
        let cells = self.cells_in_view();
        for (k, cell, in_box) in &cells {
            self.prep_label(k, *cell, *in_box);
        }
        self.request_sized_icons(&cells);
        pages::switch_to(&mut self.layout, back);
    }

    /// Where the monitor's top-left is in what is being drawn (a page sliding by is moved: the
    /// wallpaper behind its boxes' glass is not).
    pub fn mon_offset(&self) -> (f32, f32) {
        ((self.mon.left - self.origin_px.x) as f32 / self.scale - self.pages.paint_dx, (self.mon.top - self.origin_px.y) as f32 / self.scale)
    }

    /// Explorer's drag image: the dragged icons and labels, half transparent, moved by (dx, dy)
    /// so they keep their offsets from the pointer.
    fn paint_ghost(&self, dc: &Dc, keys: &[ItemKey], dx: f32, dy: f32, alpha: f32) -> r::R<()> {
        let ghost_text = Color { a: (1.25 * alpha).min(1.0), ..Self::desktop_theme().text };
        let set: std::collections::HashSet<&ItemKey> = keys.iter().collect();
        for (k, cell, bx) in self.drag_cells().iter().filter(|(k, _, _)| set.contains(k)).take(64) {
            let m = self.metrics_of(*bx);
            let (icon, label) = self.item_parts(k, *cell, false, &m);
            if self.privacy_on() {
                let name = self.items.get(k).map(|i| i.display_name.as_str()).unwrap_or("");
                self.paint_private(dc, name, icon.offset(dx, dy), label.offset(dx, dy), cell.offset(dx, dy), &Self::desktop_theme(), true, alpha)?;
                continue;
            }
            if let Some((_, bmp)) = self.items.get(k).and_then(|i| self.icon_bitmap(i, &m)) {
                r::draw_bitmap(dc, bmp, rf(fit_icon(bmp, icon).offset(dx, dy)), alpha);
            }
            let (lw, _, row) = self.label_spec(k, *cell, &m);
            let lk = App::lkey(k, lw, false, row);
            if let (Some(b), Some(l)) = (self.label_bmps.get(&lk), self.labels.get(&lk)) {
                let sz = unsafe { b.GetPixelSize() };
                let m = r::LABEL_MARGIN;
                r::draw_bitmap(dc, b, r::rect(label.x + dx - m, label.y + dy - m, sz.width as f32 / self.scale, sz.height as f32 / self.scale), alpha);
                r::draw_text(dc, l, label.x + dx, label.y + dy, ghost_text)?;
            } else if let Some(l) = self.labels.get(&lk) {
                r::draw_text(dc, l, label.x + dx, label.y + dy, ghost_text)?;
            }
        }
        Ok(())
    }

    /// The same ghost as pixels, for a drag over other apps (Windows draws it there): width,
    /// height, premultiplied BGRA rows and where the pointer (`at`) is on it. Kept within 480
    /// DIPs of the pointer.
    pub fn ghost_pixels(&self, keys: &[ItemKey], at: Pt) -> Option<(u32, u32, Vec<u8>, (i32, i32))> {
        let m = r::LABEL_MARGIN + 2.0;
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let set: std::collections::HashSet<&ItemKey> = keys.iter().collect();
        for (_, cell, _) in self.drag_cells().iter().filter(|(k, _, _)| set.contains(k)).take(64) {
            x0 = x0.min(cell.x - m);
            y0 = y0.min(cell.y - m);
            x1 = x1.max(cell.right() + m);
            y1 = y1.max(cell.bottom() + m);
        }
        let reach = 480.0;
        let (mut x0, mut y0, mut x1, y1) = (x0.max(at.x - reach), y0.max(at.y - reach), x1.min(at.x + reach), y1.min(at.y + reach));
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        // How many are being dragged: a badge above the pointer, clear of Windows' caption below it.
        let count = (self.settings.extras.drag_count && keys.len() > 1).then(|| Pt { x: at.x + 10.0, y: at.y - 26.0 });
        if let Some(c) = count {
            x0 = x0.min(c.x - 2.0);
            y0 = y0.min(c.y - 2.0);
            x1 = x1.max(c.x + 44.0);
        }
        let s = self.scale;
        let (w, h) = (((x1 - x0) * s).ceil() as u32, ((y1 - y0) * s).ceil() as u32);
        let px = self
            .gpu
            .render_pixels(w, h, s, (x0, y0), |dc| {
                self.paint_ghost(dc, keys, 0.0, 0.0, 0.85)?;
                if let Some(c) = count {
                    self.paint_drag_count(dc, keys.len(), c)?;
                }
                Ok(())
            })
            .ok()?;
        Some((w, h, px, (((at.x - x0) * s).round() as i32, ((at.y - y0) * s).round() as i32)))
    }

    /// What the desktop shows while the boxes are above the windows: the wallpaper and the
    /// icons on it (the boxes are in the window above).
    pub fn paint_still(&self, dc: &Dc) -> r::R<()> {
        if let Some(w) = &self.wall {
            let (ox, oy) = self.mon_offset();
            let sz = unsafe { w.GetPixelSize() };
            r::draw_bitmap(dc, w, r::rect(ox, oy, sz.width as f32 / self.scale, sz.height as f32 / self.scale), 1.0);
        }
        if !self.settings.show_icons || self.tuck_alpha(None) < 0.5 {
            return Ok(());
        }
        let dt = Self::desktop_theme();
        let desk = self.metrics(None);
        for (k, c) in self.loose_view().iter() {
            if self.items.contains_key(k) {
                let fx = self.icon_fx(None, k);
                self.paint_item(dc, k, self.grid.cell_rect(*c), &dt, true, 1.0, fx, &desk)?;
            }
        }
        Ok(())
    }

    /// The accent badge with how many icons are dragged, its top-left at `at`.
    fn paint_drag_count(&self, dc: &Dc, n: usize, at: Pt) -> r::R<()> {
        let (ar, ag, ab) = accent();
        if let Ok(l) = self.gpu.label(&self.az_tf, &n.to_string(), 60.0, 16.0, 1, false) {
            let w = (r::text_width(&l) + 10.0).max(18.0);
            let bx = Rect::new(at.x.round(), at.y.round(), w, 18.0);
            r::fill_round(dc, rf(bx.offset(0.0, 1.0)), 9.0, rgba(0, 0, 0, 0.3))?;
            r::fill_round(dc, rf(bx), 9.0, rgba(ar, ag, ab, 1.0))?;
            r::draw_text(dc, &l, bx.x + (w - r::text_width(&l)) / 2.0, bx.y + (18.0 - r::text_height(&l)) / 2.0, rgba(255, 255, 255, 1.0))?;
        }
        Ok(())
    }

    /// Where a drag would land: the icon taking it highlighted (a folder, the Recycle Bin, a
    /// program), as Explorer does; else the box outlined, with a marker where our icons go.
    fn paint_drop_feedback(&self, dc: &Dc, cells: &[(ItemKey, Rect, Option<u32>)], cur: Pt, onto: Option<ItemKey>, own: bool) -> r::R<()> {
        let (ar, ag, ab) = accent();
        if let Some(t) = onto {
            if let Some((_, cell, bx)) = cells.iter().find(|(k, _, _)| *k == t) {
                let hr = self.hit_rect(&t, *cell, &self.metrics_of(*bx));
                r::fill_rect(dc, rf(hr), rgba(ar, ag, ab, 0.35))?;
                r::stroke_round(dc, rf(hr.inflate(-0.5)), 0.0, rgba(ar, ag, ab, 0.9), 1.0)?;
            }
        } else if let Some(b) = self.drop_box(cur).and_then(|id| self.box_by_id(id)) {
            r::stroke_round(dc, rf(b.rect), radius(), rgba(ar, ag, ab, 0.9), 2.0)?;
            // Where our icons go: a box's own order (a folder box's, for its own icons).
            let ours = match &self.drag {
                Drag::Items { keys, .. } => !b.is_folder() || self.folder_box_owns(b, keys),
                _ => !b.is_folder(),
            };
            // (Where the icons make room instead, none.)
            if own && ours && self.fully_open(b) && cur.y >= App::content_rect(b).y && !b.az && self.box_want.is_none() && self.box_room.is_none() {
                let (_, at) = self.box_drop_slot(b, cur);
                let c = App::content_rect(b);
                let m = self.metrics(Some(b));
                r::push_clip(dc, rf(c));
                let line = if m.row() && self.box_cols(b) == 1 {
                    Rect::new(at.x + 4.0, at.y - 1.5, m.cell_w - 8.0, 3.0)
                } else if m.row() {
                    Rect::new(at.x - 1.5, at.y + 3.0, 3.0, m.cell_h - 6.0)
                } else {
                    Rect::new(at.x - 1.5, at.y + 6.0, 3.0, m.cell_h - 14.0)
                };
                r::fill_round(dc, rf(line), 1.5, rgba(ar, ag, ab, 1.0))?;
                r::pop_clip(dc);
            }
        } else if let (true, Drag::Items { keys, .. }) = (own, &self.drag) {
            // In between icons on the desktop: a line where they go.
            if let (Some((at, dir)), None) = (self.grid_insert_at(cur, keys), &self.preview_loose) {
                let r0 = self.grid.cell_rect(at);
                let line = match dir {
                    (1, _) => Rect::new(r0.x - 1.5, r0.y + 8.0, 3.0, r0.h - 16.0),
                    (-1, _) => Rect::new(r0.right() - 1.5, r0.y + 8.0, 3.0, r0.h - 16.0),
                    (_, -1) => Rect::new(r0.x + 8.0, r0.bottom() - 1.5, r0.w - 16.0, 3.0),
                    _ => Rect::new(r0.x + 8.0, r0.y - 1.5, r0.w - 16.0, 3.0),
                };
                r::fill_round(dc, rf(line), 1.5, rgba(ar, ag, ab, 1.0))?;
            }
        }
        Ok(())
    }

    /// A frame: the page shown, or while it slides to or from another page both of them, each
    /// moved (`pages.rs`); the wallpaper and what is over the pages stay put.
    fn paint_frame(&mut self, dc: &Dc) -> r::R<()> {
        let Some(f) = self.page_frame() else { return self.paint(dc) };
        self.paint_back(dc)?;
        if !self.settings.show_icons {
            return Ok(());
        }
        self.pages.sliding = true;
        let r: r::R<()> = (|| {
            if let Some(o) = f.other.filter(|o| *o != self.layout.page) {
                let back = self.layout.page;
                if pages::switch_to(&mut self.layout, o) {
                    let pl = self.preview_loose.take();
                    let r = self.paint_moved(dc, f.other_dx, f.other_alpha, false);
                    self.preview_loose = pl;
                    pages::switch_to(&mut self.layout, back);
                    r?;
                }
            }
            self.paint_moved(dc, f.dx, f.alpha, true)?;
            // A box carried to this page: under the pointer, not sliding.
            if let Drag::MoveBox { id, .. } = &self.drag {
                if let Some(b) = self.box_by_id(*id) {
                    self.paint_box(dc, b)?;
                }
            }
            Ok(())
        })();
        self.pages.sliding = false;
        r?;
        self.paint_overlays(dc)
    }

    /// A page moved by `dx` at `alpha` (`current`: the page shown, with what is being dragged).
    fn paint_moved(&mut self, dc: &Dc, dx: f32, alpha: f32, current: bool) -> r::R<()> {
        if alpha <= 0.002 || dx.abs() >= self.size_dip.0 {
            return Ok(());
        }
        self.pages.paint_dx = dx;
        let all = rf(Rect::new(-dx, 0.0, self.size_dip.0, self.size_dip.1));
        let layer = alpha < 0.998;
        let r = r::translated(dc, dx, 0.0, || {
            if layer {
                r::push_layer(dc, all, alpha)?;
            }
            let r = self.paint_content(dc, current);
            if layer {
                r::pop_layer(dc);
            }
            r
        });
        self.pages.paint_dx = 0.0;
        r
    }

    fn paint(&self, dc: &Dc) -> r::R<()> {
        self.paint_back(dc)?;
        if !self.settings.show_icons {
            return Ok(());
        }
        self.paint_content(dc, true)?;
        self.paint_overlays(dc)
    }

    /// Behind the pages: the wallpaper, or (boxes above windows) the windows, dimmed a little.
    fn paint_back(&self, dc: &Dc) -> r::R<()> {
        if self.is_above() {
            // Boxes above windows: the windows behind, dimmed a little; no wallpaper or loose icons.
            let d = self.above_dim();
            if d > 0.002 {
                r::fill_rect(dc, r::rect(0.0, 0.0, self.size_dip.0, self.size_dip.1), rgba(0, 0, 0, d))?;
            }
        } else if let Some(w) = &self.wall {
            let (ox, oy) = self.mon_offset();
            let sz = unsafe { w.GetPixelSize() };
            r::draw_bitmap(dc, w, r::rect(ox, oy, sz.width as f32 / self.scale, sz.height as f32 / self.scale), 1.0);
        }
        Ok(())
    }

    /// Over the pages: an edge a drag is held at, the search pill, the page's name, tooltips.
    fn paint_overlays(&self, dc: &Dc) -> r::R<()> {
        self.paint_page_edge(dc)?;
        self.paint_search(dc)?;
        self.paint_page_pill(dc)?;
        self.paint_tip(dc)?;
        if self.diag {
            self.paint_diagnostics(dc)?;
        }
        Ok(())
    }

    /// A page: its loose icons and boxes, and (`current`: the page shown) the selection band and
    /// what is being dragged.
    fn paint_content(&self, dc: &Dc, current: bool) -> r::R<()> {
        let above = self.is_above();
        let dragging: Vec<ItemKey> = match &self.drag {
            Drag::Items { keys, .. } => keys.clone(),
            _ => vec![],
        };
        let dt = Self::desktop_theme();
        // Hidden with a double-click (Settings › Hide all): fading away or back.
        let all = rf(Rect::new(-self.pages.paint_dx, 0.0, self.size_dip.0, self.size_dip.1));
        let ta = if above { 0.0 } else { self.tuck_alpha(None) };
        if ta > 0.002 {
            if ta < 0.998 {
                r::push_layer(dc, all, ta)?;
            }
            // Loose items (where they settle while a box is dragged, gliding there). The selected
            // icon's whole name goes over the icons below it, always (Explorer's): drawn last.
            let mut last = None;
            let desk = self.metrics(None);
            for (k, c) in self.loose_view().iter() {
                if self.items.contains_key(k) {
                    let cell = self.glided(k, self.grid.cell_rect(*c));
                    if self.is_full_label(k) {
                        last = Some((k.clone(), cell));
                        continue;
                    }
                    let fx = self.icon_fx(None, k);
                    self.paint_item(dc, k, cell, &dt, true, 1.0, fx, &desk)?;
                }
            }
            if let Some((k, cell)) = last {
                let fx = self.icon_fx(None, &k);
                self.paint_item(dc, &k, cell, &dt, true, 1.0, fx, &desk)?;
            }
            self.paint_vanishing(dc, None, &dt, true)?;
            if ta < 0.998 {
                r::pop_layer(dc);
            }
        }
        // A selection band started on the desktop goes behind the boxes: it selects only loose
        // icons, and this shows it.
        let band_behind = current
            && match &self.drag {
                Drag::Rubber { scope: None, .. } | Drag::RightRubber { scope: None, .. } => true,
                _ => false,
            };
        if band_behind {
            self.paint_band(dc)?;
        }
        // Boxes, in z-order.
        for b in &self.layout.boxes {
            let ba = self.tuck_alpha(Some(b));
            if ba <= 0.002 {
                continue;
            }
            // (A box carried to another page is drawn over the slide.)
            if self.pages.sliding && matches!(&self.drag, Drag::MoveBox { id, .. } if *id == b.id) {
                continue;
            }
            if ba < 0.998 {
                r::push_layer(dc, all, ba)?;
            }
            self.paint_box(dc, b)?;
            if ba < 0.998 {
                r::pop_layer(dc);
            }
        }
        if !current {
            return Ok(());
        }
        // Drag feedback.
        match &self.drag {
            Drag::Items { keys, start, cur } => {
                let cells = self.drag_cells();
                self.paint_ghost(dc, &dragging, cur.x - start.x, cur.y - start.y, 0.6)?;
                // How many are being dragged.
                if self.settings.extras.drag_count && keys.len() > 1 {
                    self.paint_drag_count(dc, keys.len(), Pt { x: cur.x + 12.0, y: cur.y + 14.0 })?;
                }
                self.paint_drop_feedback(dc, &cells, *cur, self.drop_onto_item(*cur, keys), true)?;
            }
            // Windows' drag and drop over us (our icons, or files from an app).
            Drag::None if self.drop_at.is_some() => {
                let cells = self.visible_cells();
                let own = self.own_drag.is_some();
                self.paint_drop_feedback(dc, &cells, self.drop_at.unwrap(), self.drop_hl.clone(), own)?;
            }
            Drag::Tab { id, tab, cur, .. } => {
                if let Some(b) = self.box_by_id(*id) {
                    let own = Rect::new(b.rect.x, b.rect.y - 16.0, b.rect.w, header_h() + 32.0);
                    if own.contains(*cur) {
                        // Reordering: a marker where the tab goes.
                        let (ar, ag, ab) = accent();
                        let others: Vec<Rect> = self.header_layout(b).tabs.iter().enumerate().filter(|(i, _)| i != tab).map(|(_, r)| *r).collect();
                        let to = self.tab_insert_index(b, *tab, cur.x);
                        let x = match others.get(to) {
                            Some(r) => r.x - TAB_GAP / 2.0,
                            None => others.last().map(|r| r.right() + TAB_GAP / 2.0).unwrap_or(b.rect.x + 6.0),
                        };
                        r::fill_round(dc, rf(Rect::new(x - 1.5, b.rect.y + 5.0, 3.0, header_h() - 10.0)), 1.5, rgba(ar, ag, ab, 1.0))?;
                    } else {
                        if let Some(t) = self.header_target(*id, *cur).and_then(|d| self.box_by_id(d)) {
                            self.paint_header_target(dc, t)?;
                        }
                        let title = b.tabs.get(*tab).map(|t| t.title.clone()).unwrap_or_default();
                        self.paint_tab_ghost(dc, b, &title, *cur)?;
                    }
                }
            }
            Drag::MoveBox { id, cur, .. } if self.settings.tabs.combine => {
                if let Some(t) = self.header_target(*id, *cur).and_then(|d| self.box_by_id(d)) {
                    self.paint_header_target(dc, t)?;
                }
            }
            Drag::Rubber { .. } | Drag::RightRubber { .. } if !band_behind => self.paint_band(dc)?,
            _ => {}
        }
        Ok(())
    }

    /// The selection band (Explorer's accent rectangle).
    fn paint_band(&self, dc: &Dc) -> r::R<()> {
        let (Drag::Rubber { start, cur, .. } | Drag::RightRubber { start, cur, .. }) = &self.drag else { return Ok(()) };
        let band = Rect::from_points(*start, *cur);
        let (ar, ag, ab) = accent();
        // A band started in a box stays inside it (it selects only that box's icons).
        let clip = match &self.drag {
            Drag::Rubber { scope: Some(id), .. } | Drag::RightRubber { scope: Some(id), .. } => self.box_by_id(*id).map(App::content_rect),
            _ => None,
        };
        if let Some(c) = clip {
            r::push_clip(dc, rf(c));
        }
        r::fill_rect(dc, rf(band), rgba(ar, ag, ab, 0.3))?;
        r::stroke_round(dc, rf(band), 0.0, rgba(ar, ag, ab, 0.85), 1.0)?;
        if clip.is_some() {
            r::pop_clip(dc);
        }
        Ok(())
    }

    /// An icon's tooltip as Windows 11 draws one: a small rounded card under the pointer with the
    /// full name, then Explorer's own text (type, size, date ...), fading in.
    fn paint_tip(&self, dc: &Dc) -> r::R<()> {
        if let Some((text, at, t0)) = &self.btn_tip {
            self.paint_tip_card(dc, text, "", *at, *t0)?;
        }
        let Some((_, name, text, at, t0)) = &self.tip else { return Ok(()) };
        self.paint_tip_card(dc, name, text, *at, *t0)
    }

    fn paint_tip_card(&self, dc: &Dc, name: &str, text: &str, at: Pt, t0: Instant) -> r::R<()> {
        let a = ease_out(t0.elapsed().as_secs_f32() / TIP_FADE_S);
        let (bg, fg, edge) = if self.light {
            (rgba(0xF9, 0xF9, 0xF9, 0.98 * a), rgba(0, 0, 0, 0.9 * a), rgba(0, 0, 0, 0.07 * a))
        } else {
            (rgba(0x2C, 0x2C, 0x2C, 0.98 * a), rgba(255, 255, 255, a), rgba(255, 255, 255, 0.09 * a))
        };
        let maxw = 380.0;
        let n = self.gpu.label(&self.tip_tf_bold, name, maxw, 16.0, 0, false)?;
        let body = (!text.is_empty()).then(|| self.gpu.label(&self.tip_tf, text, maxw, 16.0, 0, false)).transpose()?;
        let w = r::text_width(&n).max(body.as_ref().map(r::text_width).unwrap_or(0.0)).ceil() + 18.0;
        let h = r::text_height(&n) + body.as_ref().map(|b| r::text_height(b) + 3.0).unwrap_or(0.0) + 14.0;
        let bounds = Rect::new(0.0, 0.0, self.size_dip.0, self.size_dip.1);
        let mut x = at.x + 2.0;
        let mut y = at.y + 22.0;
        if x + w > bounds.right() - 4.0 {
            x = (bounds.right() - 4.0 - w).max(0.0);
        }
        if y + h > self.work.bottom() - 4.0 {
            y = at.y - h - 6.0;
        }
        let card = Rect::new(x.round(), y.round(), w, h);
        // A soft shadow, then the card.
        for (i, s) in [(1.0, 0.10), (2.5, 0.05), (4.5, 0.03)].iter().enumerate() {
            let _ = i;
            r::fill_round(dc, rf(card.offset(0.0, s.0 * 0.6).inflate(s.0)), 4.0 + s.0, rgba(0, 0, 0, s.1 * a))?;
        }
        r::fill_round(dc, rf(card), 4.0, bg)?;
        r::stroke_round(dc, rf(card.inflate(-0.5)), 4.0, edge, 1.0)?;
        r::draw_text(dc, &n, card.x + 9.0, card.y + 6.0, fg)?;
        if let Some(b) = &body {
            r::draw_text(dc, b, card.x + 9.0, card.y + 6.0 + r::text_height(&n) + 3.0, Color { a: fg.a * 0.85, ..fg })?;
        }
        Ok(())
    }

    /// Diagnostics overlay (`--diagnostics`, Ctrl+Shift+F12): frame rate, draw times, item and
    /// icon-cache counts, startup phase timings.
    fn paint_diagnostics(&self, dc: &Dc) -> r::R<()> {
        let s = &self.stats;
        let n = self.draw_times.len();
        let recent: Vec<f32> = self.draw_times.iter().rev().take(120).copied().collect();
        let avg = if recent.is_empty() { 0.0 } else { recent.iter().sum::<f32>() / recent.len() as f32 };
        let maxr = recent.iter().copied().fold(0.0f32, f32::max);
        let hit = if s.items > 0 { 100.0 * (1.0 - s.scan_resolved as f64 / s.items as f64) } else { 0.0 };
        let text = format!(
            "Deskboxes diagnostics\nframes/s (last 1 s): {}\ndraw ms: last {:.1}, avg {:.1}, max {:.1} (last 120 of {n})\nitems {}, icon bitmaps {}, boxes {}\nlast scan: {} resolved through the shell ({hit:.0}% from cache), {:.0} ms\nstartup: first paint {:.0} ms, hide {:.0} ms, reconcile {:.0} ms, host {:.0} ms",
            self.frame_marks.len(),
            s.draw_ms_last,
            avg,
            maxr,
            s.items,
            self.bitmaps.len(),
            self.layout.boxes.len(),
            s.scan_resolved,
            s.scan_ms,
            s.first_paint_ms,
            s.hide_ms,
            s.reconcile_ms,
            s.host_ready_ms
        );
        let (ox, oy) = self.mon_offset();
        let l = self.gpu.label(&self.title_tf, &text, 520.0, self.title_line_h, 0, false)?;
        let h = r::text_height(&l);
        let rect = r::rect(ox + 12.0, oy + 12.0, 540.0, h + 16.0);
        r::fill_round(dc, rect, 6.0, rgba(0, 0, 0, 0.78))?;
        r::draw_text(dc, &l, ox + 22.0, oy + 20.0, rgba(255, 255, 255, 1.0))?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_item(&self, dc: &Dc, k: &ItemKey, cell: Rect, th: &Theme, shadow: bool, opacity: f32, fx: crate::iconfx::IconFx, m: &crate::views::Metrics) -> r::R<()> {
        let Some(it) = self.items.get(k) else { return Ok(()) };
        // Searching: what does not match fades.
        let opacity = opacity * self.search_alpha(k);
        let (lw, full, row) = self.label_spec(k, cell, m);
        let (icon, label) = self.item_parts(k, cell, full, m);
        let sel = self.sel.is_selected(k);
        let hit = self.hit_rect(k, cell, m);
        if sel {
            r::fill_rect(dc, rf(hit), th.sel_fill)?;
            r::stroke_round(dc, rf(hit.inflate(-0.5)), 0.0, th.sel_border, 1.0)?;
        } else if self.hover.as_ref() == Some(k) {
            r::fill_rect(dc, rf(hit), th.hover)?;
        }
        if self.sel.focus.as_ref() == Some(k) && !sel {
            r::stroke_round(dc, rf(hit.inflate(-0.5)), 0.0, th.sel_border, 1.0)?;
        }
        let cut_ghost = if it.attrs & 0x2 != 0 { 0.55 } else { 1.0 }; // hidden items are drawn faded, as Explorer does
        if self.privacy_on() {
            return self.paint_private(dc, &it.display_name, icon, label, cell, th, shadow, opacity * cut_ghost);
        }
        let a = opacity * cut_ghost;
        // Blending in: the wallpaper's color behind the icon.
        let wall_c = (fx.blend > 0.0).then(|| self.wall_color_at(Pt { x: icon.x + icon.w / 2.0, y: icon.y + icon.h / 2.0 }));
        if let Some((bid, b)) = self.icon_bitmap(it, m) {
            let dst = rf(fit_icon(b, icon));
            let own = 1.0 - fx.blend;
            if own > 0.002 {
                match fx.tint.and_then(|(c, s)| self.fx_bitmap(bid, b, c, 0).map(|t| (t, s))) {
                    Some((t, s)) => {
                        // The icon's own colors under its tinted copy (strength = how much shows).
                        if s < 0.999 {
                            r::draw_bitmap(dc, b, dst, a * own);
                        }
                        r::draw_bitmap(dc, &t, dst, a * own * if s < 0.999 { s } else { 1.0 });
                    }
                    None => r::draw_bitmap(dc, b, dst, a * own),
                }
            }
            if let Some(c) = wall_c {
                if let Some(g) = self.fx_bitmap(bid, b, c, 1) {
                    r::draw_bitmap(dc, &g, dst, a * fx.blend * crate::iconfx::GHOST_A);
                }
            }
        }
        // A blended name: the wallpaper's color, fainter, its shadow mostly gone.
        let (text_c, shadow_a) = match wall_c {
            Some(c) => (crate::iconfx::blend_text(th.text, c, fx.blend), 1.0 - 0.85 * fx.blend),
            None => (th.text, 1.0),
        };
        let opacity = opacity * (1.0 - 0.3 * fx.blend);
        // Its name is being edited: the field (drawn on top) shows it.
        if self.rename.as_ref().map(|r| &r.key) == Some(k) {
            return Ok(());
        }
        // Details and Tiles: the item's other texts, dimmer.
        if matches!(m.view, deskboxes_core::model::View::Details | deskboxes_core::model::View::Tiles) {
            let dim = Color { a: text_c.a * opacity * 0.72, ..text_c };
            self.paint_item_texts(dc, k, cell, label, m, dim, shadow)?;
        }
        let lk = App::lkey(k, lw, full, row);
        match (shadow, self.label_bmps.get(&lk), self.labels.get(&lk)) {
            (true, Some(b), Some(l)) => {
                // The shadow (cached), then the text with ClearType, as Explorer draws it.
                let sz = unsafe { b.GetPixelSize() };
                let m = r::LABEL_MARGIN;
                if shadow_a > 0.01 {
                    r::draw_bitmap(dc, b, r::rect(label.x - m, label.y - m, sz.width as f32 / self.scale, sz.height as f32 / self.scale), opacity * shadow_a);
                }
                r::draw_text_cleartype(dc, l, label.x, label.y, Color { a: text_c.a * opacity, ..text_c })?;
            }
            (true, None, Some(l)) => r::draw_text_shadowed(dc, l, label.x, label.y, Color { a: text_c.a * opacity, ..text_c })?,
            (false, _, Some(l)) => r::draw_text(dc, l, label.x, label.y, Color { a: text_c.a * opacity, ..text_c })?,
            _ => {}
        }
        Ok(())
    }

    /// Privacy mode: the icon as a blank outline (dashed, like a blueprint) and its name as
    /// dashes, one per word, as long as the words.
    #[allow(clippy::too_many_arguments)]
    /// A title in privacy mode: one rounded dash per word, `w` wide in all, centered on `cy`.
    fn paint_dashes(&self, dc: &Dc, text: &str, x: f32, cy: f32, w: f32, c: Color, shadow: bool) -> r::R<()> {
        let words: Vec<usize> = text.split_whitespace().map(|w| w.chars().count()).filter(|n| *n > 0).collect();
        let chars: usize = words.iter().sum::<usize>().max(1);
        let gap = 5.0;
        let room = (w - gap * words.len().saturating_sub(1) as f32).max(6.0);
        let mut px = x.round();
        let y = (cy - 2.0).round();
        for n in words {
            let ww = (room * n as f32 / chars as f32).max(6.0);
            let rr = Rect::new(px, y, ww, 4.0);
            if shadow {
                r::fill_round(dc, rf(rr.offset(0.0, 1.0)), 2.0, rgba(0, 0, 0, 0.4 * c.a))?;
            }
            r::fill_round(dc, rf(rr), 2.0, Color { a: c.a * 0.85, ..c })?;
            px += ww + gap;
        }
        Ok(())
    }

    fn paint_private(&self, dc: &Dc, name: &str, icon: Rect, label: Rect, cell: Rect, th: &Theme, shadow: bool, a: f32) -> r::R<()> {
        let s = (icon.w.min(icon.h) * 0.78).round();
        let ic = Rect::new((icon.x + (icon.w - s) / 2.0).round() + 0.5, (icon.y + (icon.h - s) / 2.0).round() + 0.5, s - 1.0, s - 1.0);
        let rad = (s * 0.14).max(2.0);
        let line = Color { a: th.text.a * 0.8 * a, ..th.text };
        if shadow {
            r::stroke_round_styled(dc, rf(ic.offset(0.0, 1.0)), rad, rgba(0, 0, 0, 0.45 * a), 1.4, &self.gpu.dash)?;
        }
        r::fill_round(dc, rf(ic), rad, Color { a: th.text.a * 0.07 * a, ..th.text })?;
        r::stroke_round_styled(dc, rf(ic), rad, line, 1.4, &self.gpu.dash)?;
        // The name: a dash per word (at most two lines' worth).
        let unit = 5.2;
        let max_w = (cell.w - 10.0).max(10.0);
        let words: Vec<usize> = name.split(|c: char| c.is_whitespace() || c == '.' || c == '_' || c == '-').map(|w| w.chars().count()).filter(|n| *n > 0).collect();
        let mut lines: Vec<Vec<f32>> = vec![vec![]];
        let mut used = 0.0;
        for n in words {
            let w = (n as f32 * unit).clamp(6.0, max_w);
            if used > 0.0 && used + 4.0 + w > max_w {
                if lines.len() == 2 {
                    break;
                }
                lines.push(vec![]);
                used = 0.0;
            }
            used += if used > 0.0 { 4.0 + w } else { w };
            lines.last_mut().unwrap().push(w);
        }
        let bar = Color { a: th.text.a * 0.75 * a, ..th.text };
        for (i, ln) in lines.iter().enumerate() {
            let total: f32 = ln.iter().sum::<f32>() + 4.0 * ln.len().saturating_sub(1) as f32;
            let mut x = (cell.x + (cell.w - total) / 2.0).round();
            let y = (label.y + 6.0 + i as f32 * 9.0).round();
            for w in ln {
                let rr = Rect::new(x, y, *w, 3.0);
                if shadow {
                    r::fill_round(dc, rf(rr.offset(0.0, 1.0)), 1.5, rgba(0, 0, 0, 0.4 * a))?;
                }
                r::fill_round(dc, rf(rr), 1.5, bar)?;
                x += w + 4.0;
            }
        }
        Ok(())
    }

    /// Deleted (or moved away) icons shrinking and fading out where they were.
    fn paint_vanishing(&self, dc: &Dc, in_box: Option<u32>, th: &Theme, shadow: bool) -> r::R<()> {
        if self.privacy_on() {
            return Ok(());
        }
        for v in self.vanishing.iter().filter(|v| v.in_box == in_box) {
            let t = (v.t0.elapsed().as_secs_f32() / crate::app::VANISH_S).min(1.0);
            let a = 1.0 - crate::app::ease_out(t);
            if a <= 0.0 {
                continue;
            }
            let s = 1.0 - 0.35 * crate::app::ease_out(t);
            let ic = v.icon_rect;
            let (cx, cy) = (ic.x + ic.w / 2.0, ic.y + ic.h / 2.0);
            if let Some(b) = &v.icon {
                let r0 = fit_icon(b, ic);
                let dst = Rect::new(cx + (r0.x - cx) * s, cy + (r0.y - cy) * s, r0.w * s, r0.h * s);
                r::draw_bitmap(dc, b, rf(dst), a);
            }
            if let Some((l, sh)) = &v.label {
                if let (true, Some(b)) = (shadow, sh) {
                    let sz = unsafe { b.GetPixelSize() };
                    let m = r::LABEL_MARGIN;
                    r::draw_bitmap(dc, b, r::rect(v.label_at.x - m, v.label_at.y - m, sz.width as f32 / self.scale, sz.height as f32 / self.scale), a * a);
                }
                r::draw_text(dc, l, v.label_at.x, v.label_at.y, Color { a: th.text.a * a * a, ..th.text })?;
            }
        }
        Ok(())
    }

    fn paint_box(&self, dc: &Dc, b: &BoxModel) -> r::R<()> {
        let rect = self.shown_rect(b);
        let open = self.fully_open(b);
        // Sides touching the screen edge get square corners: draw past the edge there.
        let d = self.dock(b);
        let ext = radius() + 2.0;
        let mut body = rect;
        if d.top {
            body.y -= ext;
            body.h += ext;
        }
        if d.bottom && (open || !self.rolls_up(b) || !d.top) {
            body.h += ext;
        }
        if d.left {
            body.x -= ext;
            body.w += ext;
        }
        if d.right {
            body.w += ext;
        }
        let st = &b.style;
        let light = self.box_is_light(b);
        let auto_tint = if self.light { rgba(0xF3, 0xF3, 0xF3, 1.0) } else { rgba(0x20, 0x20, 0x20, 1.0) };
        let shown_c = App::box_color(b);
        let mut tint = shown_c.map(|c| rgba(c.r, c.g, c.b, 1.0)).unwrap_or(auto_tint);
        // Another tab just opened with its own color: the background crossfades.
        if let Some((_, from, t0)) = self.tab_color_from.filter(|(id, _, _)| *id == b.id) {
            let t = ease_out(t0.elapsed().as_secs_f32() / crate::app::TAB_COLOR_S);
            if t < 1.0 {
                let f = from.map(|c| rgba(c.r, c.g, c.b, 1.0)).unwrap_or(auto_tint);
                tint = Color { r: f.r + (tint.r - f.r) * t, g: f.g + (tint.g - f.g) * t, b: f.b + (tint.b - f.b) * t, a: 1.0 };
            }
        }
        // Blending into the wallpaper (and the pointer not on it): the box fades too.
        let bk = 1.0 - 0.85 * self.box_blend(b);
        let op = st.opacity.clamp(0.0, 1.0) * bk;
        // Being moved: a soft shadow, as if lifted off the desktop (under the wallpaper copy a
        // box is filled with, so it shows only around it).
        if self.settings.extras.lift_box && st.material == Material::Mica && matches!(self.drag, Drag::MoveBox { id, .. } if id == b.id) {
            let k = ease_out(self.move_t0.elapsed().as_secs_f32() / crate::app::LIFT_S);
            for i in 1..=6 {
                let g = i as f32 * 2.5;
                r::fill_round(dc, rf(body.inflate(g).offset(0.0, 3.0 + i as f32)), radius() + g, rgba(0, 0, 0, 0.045 * k))?;
            }
        }
        match st.material {
            Material::Mica => {
                // The wallpaper under the box, blurred (or sharp with no blur) ...
                if st.blur < 1.0 {
                    // (Above the windows: what is behind the box now.)
                    if let Some(w) = self.above_wall.as_ref().filter(|_| self.is_above()).or(self.wall.as_ref()) {
                        r::fill_with_backdrop(dc, w, 1.0 / self.scale, self.mon_offset(), rf(body), radius())?;
                    }
                } else if let Some(bd) = self.backdrops.get(&(st.blur as i32)) {
                    let s = self.backdrop_down as f32 / self.scale;
                    r::fill_with_backdrop(dc, bd, s, self.mon_offset(), rf(body), radius())?;
                }
                // ... a luminosity layer as Mica has, which evens out bright and dark spots of the
                // wallpaper so text stays readable, then the tint.
                let lum = if light { rgba(255, 255, 255, (0.10 + 0.2 * op) * bk) } else { rgba(0, 0, 0, (0.10 + 0.2 * op) * bk) };
                r::fill_round(dc, rf(body), radius(), lum)?;
                r::fill_round(dc, rf(body), radius(), Color { a: op, ..tint })?;
            }
            Material::Plain => r::fill_round(dc, rf(body), radius(), Color { a: op, ..tint })?,
        }
        // The title bar: a darker band (or an opaque one) across the top, clipped to the box.
        let look = &self.settings.look;
        // Titles only when pointed at: the whole title bar fades in and out (always shown on a
        // box that is rolled up, being renamed or searched).
        let shown_title = if look.titles_on_hover && open && self.box_edit.is_none() && self.search.scope != Some(b.id) {
            self.pointer_on(b.id)
        } else {
            1.0
        };
        let hdr_a = shown_title * (1.0 - 0.7 * self.box_blend(b));
        let header_layer = hdr_a < 0.999;
        if header_layer {
            r::push_layer(dc, rf(Rect::new(body.x, body.y, body.w, rect.y + header_h() - body.y + 1.0)), hdr_a)?;
        }
        let band = match look.title_bar.as_str() {
            "band" => Some(rgba(0, 0, 0, if light { look.band * 0.45 } else { look.band })),
            "solid" => Some(if light { rgba(0, 0, 0, 0.16) } else { rgba(0, 0, 0, 0.42) }),
            _ => None,
        };
        if let Some(c) = band {
            let top = Rect::new(body.x, body.y, body.w, (rect.y + header_h() - body.y).min(body.h));
            r::push_clip(dc, rf(top));
            r::fill_round(dc, rf(body), radius(), c)?;
            if look.title_bar == "solid" {
                r::fill_round(dc, rf(body), radius(), Color { a: 0.55, ..tint })?;
            }
            r::pop_clip(dc);
        }
        // The outline (Settings › Styles › Outline): faint by default.
        let ow = look.outline_w.clamp(0.5, 4.0);
        let faint = if light { rgba(0, 0, 0, 0.10 * bk) } else { rgba(255, 255, 255, 0.10 * bk) };
        let border = match look.outline.as_str() {
            "none" => None,
            "accent" => {
                let (ar, ag, ab) = accent();
                Some(rgba(ar, ag, ab, 0.9 * bk))
            }
            "box" => Some(shown_c.map(|c| rgba(c.r, c.g, c.b, 0.95 * bk)).unwrap_or(faint)),
            "white" => Some(rgba(255, 255, 255, 0.6 * bk)),
            "black" => Some(rgba(0, 0, 0, 0.5 * bk)),
            _ => Some(faint),
        };
        if let Some(c) = border {
            r::stroke_round(dc, rf(body.inflate(-ow / 2.0)), (radius() - ow / 2.0 + 0.5).max(0.0), c, ow)?;
        }
        let th = Self::box_theme(light);
        let title_c = st.title_color.map(|c| rgba(c.r, c.g, c.b, 1.0)).unwrap_or(th.text);
        // Header: one centered title, or tabs sized to their titles in the chosen style (with
        // their colors), and + (add a tab) while the pointer is on the title.
        let hl = self.header_layout(b);
        let n = b.tabs.len();
        let editing = self.box_edit.as_ref().filter(|e| e.id == b.id).map(|e| e.tab);
        let hov = self.hover_hdr.filter(|h| h.0 == b.id && matches!(self.drag, Drag::None));
        let style = self.settings.tabs.style.as_str();
        // A search field over the title hides it.
        let searched_here = self.box_search_field().map(|(id, _)| id == b.id).unwrap_or(false);
        let (ar, ag, ab) = accent();
        let hover_fill = if light { rgba(0, 0, 0, 0.06) } else { rgba(255, 255, 255, 0.07) };
        // A tab just opened: its highlight slides over from the previous one, its icons fade in.
        let ta = self.tab_anim.filter(|(id, _, _)| *id == b.id).map(|(_, from, t0)| (from, ease_out(t0.elapsed().as_secs_f32() / TAB_ANIM_S)));
        let slide = |to: Rect| match ta {
            Some((f, t)) => Rect::new(f.x + (to.x - f.x) * t, to.y, f.w + (to.w - f.w) * t, to.h),
            None => to,
        };
        if n > 1 && style == "segments" && !searched_here {
            if let (Some(f), Some(l)) = (hl.tabs.first(), hl.tabs.last()) {
                let track = if light { rgba(0, 0, 0, 0.06) } else { rgba(0, 0, 0, 0.24) };
                r::fill_round(dc, rf(Rect::new(f.x - 2.0, f.y - 2.0, l.right() - f.x + 4.0, f.h + 4.0)), 7.0, track)?;
            }
        }
        for (i, tab) in b.tabs.iter().enumerate() {
            let Some(tr) = hl.tabs.get(i).copied() else { continue };
            if editing == Some(i) || searched_here {
                continue;
            }
            let active = i == b.active_tab || n == 1;
            let tc = tab.color.filter(|_| n > 1).map(|c| rgba(c.r, c.g, c.b, 1.0));
            let on_search = matches!(self.hdr_btn, Some((h, 1, _)) if h == b.id);
            let hovered = n > 1 && !on_search && hov.map(|h| h.1 == Some(i)).unwrap_or(false);
            let hf = if hovered { ease_out(self.hdr_hover_t0.elapsed().as_secs_f32() / crate::app::HOVER_FADE_S) } else { 0.0 };
            let hover_c = Color { a: hover_fill.a * hf, ..hover_fill };
            let dragged = matches!(self.drag, Drag::Tab { id, tab: dt, .. } if id == b.id && dt == i);
            // The fill under an active colored tab (the text goes black or white on it).
            let mut filled: Option<Color> = None;
            if n > 1 {
                match style {
                    "underline" => {
                        if hovered {
                            r::fill_round(dc, rf(tr), 5.0, hover_c)?;
                        }
                        let line = if active { Some(tc.unwrap_or(rgba(ar, ag, ab, 1.0))) } else { tc.map(|c| Color { a: 0.55, ..c }) };
                        if let Some(c) = line {
                            let lr = if active { slide(tr) } else { tr };
                            let (lw, lh) = if active { (lr.w - 16.0, 3.0) } else { (lr.w - 24.0, 2.0) };
                            r::fill_round(dc, rf(Rect::new(lr.x + (lr.w - lw) / 2.0, rect.y + header_h() - 4.0, lw.max(6.0), lh)), lh / 2.0, c)?;
                        }
                    }
                    _ => {
                        let segs = style == "segments";
                        let neutral = match (segs, light) {
                            (true, true) => rgba(255, 255, 255, 0.85),
                            (true, false) => rgba(255, 255, 255, 0.13),
                            (false, true) => rgba(0, 0, 0, 0.08),
                            (false, false) => rgba(0, 0, 0, 0.30),
                        };
                        let f = if active {
                            Some(tc.map(|c| Color { a: 0.85, ..c }).unwrap_or(neutral))
                        } else if let Some(c) = tc {
                            // A colored tab shows the pointer by deepening its color.
                            Some(Color { a: 0.30 + 0.22 * hf, ..c })
                        } else if hovered {
                            Some(hover_c)
                        } else {
                            None
                        };
                        let rad = if segs { 5.0 } else { 6.0 };
                        if let Some(c) = f {
                            r::fill_round(dc, rf(if active { slide(tr) } else { tr }), rad, c)?;
                            if active && tc.is_some() {
                                filled = Some(c);
                            }
                        }
                        // The open tab under the pointer: a light veil over its fill.
                        if active && hovered {
                            let veil = if light { rgba(255, 255, 255, 0.18 * hf) } else { rgba(255, 255, 255, 0.10 * hf) };
                            r::fill_round(dc, rf(tr), rad, veil)?;
                        }
                    }
                }
            }
            if !active && n > 1 && self.tab_has_matches(tab) {
                let (ar, ag, ab) = accent();
                r::fill_round(dc, rf(Rect::new(tr.right() - 7.0, tr.y + 3.0, 5.0, 5.0)), 2.5, rgba(ar, ag, ab, 1.0))?;
            }
            let tf = if active { &self.title_tf_bold } else { &self.title_tf };
            let center = n > 1 || self.settings.look.align != "left";
            // A folder box's tab in a subfolder says which.
            let title = self.tab_label(b, i);
            if let Ok(l) = self.gpu.label(tf, &title, (tr.w - 16.0).max(4.0), self.title_line_h, 1, center) {
                let h = r::text_height(&l);
                let base = match filled {
                    Some(c) if c.r * 0.299 + c.g * 0.587 + c.b * 0.114 > 0.6 => rgba(0, 0, 0, 0.9),
                    Some(_) => rgba(255, 255, 255, 1.0),
                    None => title_c,
                };
                let mut c = if active { base } else { Color { a: base.a * 0.62, ..base } };
                if dragged {
                    c.a *= 0.45;
                }
                let (x, y) = (tr.x + 8.0, rect.y + (header_h() - h) / 2.0);
                if self.privacy_on() {
                    // Privacy mode: the title as a dash per word, where its words would be.
                    let tw = r::text_width(&l).min((tr.w - 16.0).max(4.0));
                    let x0 = if center { x + ((tr.w - 16.0).max(4.0) - tw) / 2.0 } else { x };
                    self.paint_dashes(dc, &title, x0, rect.y + header_h() / 2.0, tw, c, !light && filled.is_none())?;
                    continue;
                }
                if !light && filled.is_none() {
                    r::draw_text(dc, &l, x, y + 1.0, rgba(0, 0, 0, 0.45 * c.a))?;
                }
                r::draw_text(dc, &l, x, y, c)?;
            }
        }
        if editing.is_some() {
            self.paint_title_field(dc, light)?;
        }
        // A folder box: a folder at the left of its title, or the back arrow in a subfolder.
        if b.is_folder() && editing.is_none() && !searched_here {
            let g = Rect::new(rect.x + 6.0, rect.y + 4.0, 26.0, header_h() - 8.0);
            let back = b.tab_in_sub(b.active_tab);
            let hv = back && self.folder_back_hover == Some(b.id) && matches!(self.drag, Drag::None);
            if hv {
                r::fill_round(dc, rf(g), 5.0, hover_fill)?;
            }
            let (glyph, a) = if back { ("\u{E72B}", if hv { 1.0 } else { 0.85 }) } else { ("\u{E8B7}", 0.6) };
            if let Ok(l) = self.gpu.label(&self.glyph_tf, glyph, g.w, g.h, 1, true) {
                r::draw_text(dc, &l, g.x, g.y + (g.h - r::text_height(&l)) / 2.0, Color { a: title_c.a * a, ..title_c })?;
            }
        }
        let lk = self.locks(b);
        // A locked box shows a lock in its title while the pointer is on it (where + would be
        // when its tabs are locked, else just left of +).
        if hov.is_some() && editing.is_none() && (self.settings.lock_all || b.locks.any()) {
            let a = if lk.tabs { hl.add } else { hl.add.offset(-2.0 * (hl.add.w + 2.0), 0.0) };
            if let Ok(l) = self.gpu.label(&self.glyph_tf, "\u{E72E}", a.w, a.h, 1, true) {
                r::draw_text(dc, &l, a.x, a.y + (a.h - r::text_height(&l)) / 2.0, Color { a: title_c.a * 0.6, ..title_c })?;
            }
        }
        // Roll-up and menu buttons (Settings › Styles › Title-bar buttons), while the pointer is on
        // the title.
        if let (Some(_), None, false, Some(btns)) = (hov, editing, searched_here, self.title_btn_rects(b)) {
            let rolled = !self.fully_open(b);
            for (i, g) in btns.iter().enumerate() {
                let hv = self.title_btn_hover == Some((b.id, i as u8));
                if hv {
                    r::fill_round(dc, rf(*g), 5.0, hover_fill)?;
                }
                let glyph = match (i, rolled) {
                    (0, false) => "\u{E70E}",
                    (0, true) => "\u{E70D}",
                    _ => "\u{E712}",
                };
                if let Ok(l) = self.gpu.label(&self.glyph_tf, glyph, g.w, g.h, 1, true) {
                    r::draw_text(dc, &l, g.x, g.y + (g.h - r::text_height(&l)) / 2.0, Color { a: title_c.a * if hv { 0.95 } else { 0.6 }, ..title_c })?;
                }
            }
        }
        // The magnifier: search this box. On hover it opens into a small field saying "Search".
        if hov.is_some() && editing.is_none() && !searched_here {
            let a = self.header_search_rect(b);
            let e = self.search_expand(b.id);
            let hv = self.btn_hover(b.id, 1);
            if e > 0.01 {
                let (bg, edge) = if light { (rgba(0xFB, 0xFB, 0xFB, 0.96 * e), rgba(0, 0, 0, 0.10 * e)) } else { (rgba(0x2A, 0x2A, 0x2A, 0.96 * e), rgba(255, 255, 255, 0.12 * e)) };
                r::fill_round(dc, rf(a), 5.0, bg)?;
                r::stroke_round(dc, rf(a.inflate(-0.5)), 5.0, edge, 1.0)?;
            } else if hv > 0.0 {
                r::fill_round(dc, rf(a), 5.0, Color { a: hover_fill.a * hv, ..hover_fill })?;
            }
            let g = Rect::new(a.x, a.y, 24.0f32.min(a.w), a.h);
            if let Ok(l) = self.gpu.label(&self.glyph_tf, "\u{E721}", g.w, g.h, 1, true) {
                r::draw_text(dc, &l, g.x, g.y + (g.h - r::text_height(&l)) / 2.0, Color { a: title_c.a * (0.6 + 0.35 * hv.max(e)), ..title_c })?;
            }
            if e > 0.05 {
                if let Ok(l) = self.gpu.label(&self.title_tf, deskboxes_i18n::t(self.lang, deskboxes_i18n::S::Search), (a.w - 30.0).max(1.0), self.title_line_h, 1, false) {
                    r::push_clip(dc, rf(a));
                    r::draw_text(dc, &l, a.x + 26.0, a.y + (a.h - r::text_height(&l)) / 2.0, Color { a: title_c.a * 0.55 * e, ..title_c })?;
                    r::pop_clip(dc);
                }
            }
        }
        if searched_here {
            self.paint_box_search(dc, b, light)?;
        }
        if let Some(h) = hov.filter(|_| editing.is_none() && !lk.tabs && !searched_here) {
            let a = hl.add;
            let hv = self.btn_hover(b.id, 2);
            if h.2 && hv > 0.0 {
                r::fill_round(dc, rf(a), 5.0, Color { a: hover_fill.a * hv, ..hover_fill })?;
            }
            let c = Color {
                a: title_c.a * (0.6 + 0.35 * hv),
                ..title_c
            };
            let (cx, cy) = ((a.x + a.w / 2.0).round(), (a.y + a.h / 2.0).round());
            r::fill_rect(dc, rf(Rect::new(cx - 5.0, cy - 0.5, 10.0, 1.0)), c)?;
            r::fill_rect(dc, rf(Rect::new(cx - 0.5, cy - 5.0, 1.0, 10.0)), c)?;
        }
        if self.openness(b) > 0.05 && self.settings.look.title_bar == "line" {
            let sep = if light { rgba(0, 0, 0, 0.10) } else { rgba(255, 255, 255, 0.10) };
            r::fill_rect(dc, rf(Rect::new(rect.x + 10.0, rect.y + header_h() - 1.0, rect.w - 20.0, 1.0)), sep)?;
        }
        if header_layer {
            r::pop_layer(dc);
        }
        // The icons, also while the box rolls open or closed: in the part that is open (moving
        // with the title of a box that opens upward), fading with it.
        let o = self.openness(b);
        if o <= 0.01 {
            return Ok(());
        }
        let content = App::content_rect(b);
        let dy = rect.y - b.rect.y;
        let m = self.metrics(Some(b));
        // Details: the rows scroll under the column names, not through them.
        let head = if m.view == deskboxes_core::model::View::Details { crate::views::DETAILS_HEAD } else { 0.0 };
        let top = header_h() + crate::az::bar_h(b) + head;
        let vis = Rect::new(content.x, rect.y + top, content.w, (rect.bottom() - rect.y - top - 2.0).max(0.0));
        let fade = if open { 1.0 } else { o };
        let tagged = self.az_tagged(b);
        r::push_clip(dc, rf(vis));
        let a = ta.map(|(_, t)| t).unwrap_or(1.0) * fade;
        // The selected icon's whole name over the icons below it: drawn last.
        let mut last = None;
        for (i, k, cell) in self.box_cells_in(b, vis.y - dy, vis.bottom() - dy) {
            if self.is_box_room_gap(b.id, &k) {
                continue;
            }
            let cell = self.box_glided(b, &k, cell).offset(0.0, dy);
            if cell.bottom() < vis.y || cell.y > vis.bottom() {
                continue;
            }
            if self.is_full_label(&k) && !m.row() {
                last = Some((k.clone(), cell));
            } else {
                let fx = self.icon_fx(Some(b), &k);
                self.paint_item(dc, &k, cell, &th, !light, a, fx, &m)?;
            }
            if let Some((_, c)) = tagged.iter().find(|t| t.0 == i) {
                if !self.privacy_on() {
                    self.paint_az_tag(dc, *c, cell, th.text, !light, a * self.search_alpha(&k))?;
                }
            }
        }
        if let Some((k, cell)) = last {
            let fx = self.icon_fx(Some(b), &k);
            self.paint_item(dc, &k, cell, &th, !light, a, fx, &m)?;
        }
        self.paint_vanishing(dc, Some(b.id), &th, !light)?;
        r::pop_clip(dc);
        if m.view == deskboxes_core::model::View::Details {
            self.paint_details_head(dc, b, &m, th.text, dy, a)?;
        }
        if !open {
            return Ok(());
        }
        // An A–Z box's letter bar.
        if b.az {
            self.paint_az_bar(dc, b, th.text, dy, 1.0)?;
        }
        // Scroll indicator.
        let max = self.max_scroll(b);
        if max > 0.0 {
            let s = self.shown_scroll(b.id).clamp(0.0, max);
            let track = content.h - 2.0 * PAD;
            let thumb = (track * content.h / (content.h + max)).max(20.0);
            let y = content.y + PAD + (track - thumb) * (s / max);
            let a = if self.settings.look.scroll_on_hover { self.pointer_on(b.id) } else { 1.0 };
            if a > 0.01 {
                r::fill_round(dc, rf(Rect::new(content.right() - 5.0, y, 3.0, thumb)), 1.5, Color { a: 0.35 * a, ..th.text })?;
            }
        }
        Ok(())
    }
}

impl App {
    /// A box's title while something dropped there would become a tab: accent, "+ Add as a tab".
    fn paint_header_target(&self, dc: &Dc, b: &BoxModel) -> r::R<()> {
        let r0 = self.shown_rect(b);
        let hr = Rect::new(r0.x, r0.y, r0.w, header_h());
        let (ar, ag, ab) = accent();
        r::fill_round(dc, rf(hr), radius(), rgba(ar, ag, ab, 0.92))?;
        let text = format!("+  {}", deskboxes_i18n::t(self.lang, deskboxes_i18n::S::AddAsTab));
        if let Ok(l) = self.gpu.label(&self.title_tf_bold, &text, hr.w - 16.0, self.title_line_h, 1, true) {
            let h = r::text_height(&l);
            r::draw_text(dc, &l, hr.x + 8.0, hr.y + (header_h() - h) / 2.0, rgba(255, 255, 255, 1.0))?;
        }
        Ok(())
    }

    /// A tab carried away from its box: a small box-like card under the pointer.
    fn paint_tab_ghost(&self, dc: &Dc, b: &BoxModel, title: &str, at: Pt) -> r::R<()> {
        let w = b.rect.w.min(220.0);
        let g = Rect::new(at.x - w / 2.0, at.y - header_h() / 2.0, w, header_h() + 44.0);
        let light = self.box_is_light(b);
        let (bg, fg) = if light {
            (rgba(0xF3, 0xF3, 0xF3, 0.88), rgba(0, 0, 0, 0.9))
        } else {
            (rgba(0x20, 0x20, 0x20, 0.88), rgba(255, 255, 255, 1.0))
        };
        r::fill_round(dc, rf(g), radius(), bg)?;
        let (ar, ag, ab) = accent();
        r::stroke_round(dc, rf(g.inflate(-0.5)), radius(), rgba(ar, ag, ab, 0.9), 1.0)?;
        if let Ok(l) = self.gpu.label(&self.title_tf_bold, title, w - 16.0, self.title_line_h, 1, true) {
            let h = r::text_height(&l);
            r::draw_text(dc, &l, g.x + 8.0, g.y + (header_h() - h) / 2.0, fg)?;
        }
        let sep = if light { rgba(0, 0, 0, 0.08) } else { rgba(255, 255, 255, 0.08) };
        r::fill_rect(dc, rf(Rect::new(g.x + 10.0, g.y + header_h() - 1.0, g.w - 20.0, 1.0)), sep)?;
        Ok(())
    }

    /// A name being edited in place (box title or icon name): a Windows 11 text box, focused —
    /// rounded, the theme's field color, a hairline border and the accent underline — with
    /// selection and a blinking caret. It fades in.
    fn paint_title_field(&self, dc: &Dc, light: bool) -> r::R<()> {
        let (Some((field, l, x)), Some(be)) = (self.box_edit_layout(), self.box_edit.as_ref()) else {
            return Ok(());
        };
        let fade = crate::app::ease_out(be.since.elapsed().as_secs_f32() / crate::app::EDIT_FADE_S);
        let (bg, fg, edge) = if light {
            (rgba(255, 255, 255, 0.97), rgba(0, 0, 0, 0.9), rgba(0, 0, 0, 0.10))
        } else {
            (rgba(30, 30, 30, 0.96), rgba(255, 255, 255, 1.0), rgba(255, 255, 255, 0.10))
        };
        let a = |c: Color| Color { a: c.a * fade, ..c };
        r::fill_round(dc, rf(field), 4.0, a(bg))?;
        r::stroke_round(dc, rf(field.inflate(-0.5)), 4.0, a(edge), 1.0)?;
        let (ar, ag, ab) = accent();
        r::push_clip(dc, rf(Rect::new(field.x, field.bottom() - 2.0, field.w, 2.0)));
        r::fill_round(dc, rf(field), 4.0, rgba(ar, ag, ab, fade))?;
        r::pop_clip(dc);
        r::push_clip(dc, rf(Rect::new(field.x + 4.0, field.y, field.w - 8.0, field.h)));
        let h = r::text_height(&l);
        let y = (field.y + (field.h - 2.0 - h) / 2.0).round();
        let (s0, s1) = be.ed.sel();
        if s0 != s1 {
            let (x0, x1) = (r::caret_x(&l, be.ed.utf16_pos(s0)), r::caret_x(&l, be.ed.utf16_pos(s1)));
            r::fill_rect(dc, rf(Rect::new(x + x0, y, x1 - x0, h)), rgba(ar, ag, ab, if light { 0.45 } else { 0.7 } * fade))?;
        }
        r::draw_text(dc, &l, x, y, a(fg))?;
        if be.caret_on {
            let cx = x + r::caret_x(&l, be.ed.utf16_pos(be.ed.caret));
            r::fill_rect(dc, rf(Rect::new((cx * self.scale).round() / self.scale, y, 1.0, h)), a(fg))?;
        }
        r::pop_clip(dc);
        Ok(())
    }
}

/// Where an icon bitmap goes in the icon square: square icons fill it; thumbnails keep their
/// aspect ratio, centred horizontally and standing on the bottom edge, as Explorer draws them.
fn fit_icon(b: &r::Bitmap, icon: Rect) -> Rect {
    let sz = unsafe { b.GetPixelSize() };
    if sz.width == sz.height || sz.width == 0 || sz.height == 0 {
        return icon;
    }
    let k = (icon.w / sz.width as f32).min(icon.h / sz.height as f32);
    let (w, h) = (sz.width as f32 * k, sz.height as f32 * k);
    Rect::new(icon.x + (icon.w - w) / 2.0, icon.bottom() - h, w, h)
}
