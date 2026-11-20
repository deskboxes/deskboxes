//! Scene drawing (docs/ARCHITECTURE.md §4). Everything is in DIPs; the surface transform scales.
//! Phase 1 redraws the whole surface per frame (frames happen only on input or model changes).

use crate::app::{ease_out, header_h, radius, App, Drag, PAD, TAB_ANIM_S, TAB_GAP, TIP_FADE_S};
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
        match b.style.color {
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
        let cells = self.visible_cells();
        for (k, cell, in_box) in &cells {
            let full = self.is_full_label(k);
            let _ = self.label_layout(k, cell.w - 4.0, full);
            let _ = self.label_layout(k, cell.w - 4.0, false);
            let light_box = in_box.and_then(|id| self.box_by_id(id)).map(|b| self.box_is_light(b)).unwrap_or(false);
            if !light_box {
                for f in [full, false] {
                    let key = (k.clone(), f, (cell.w - 4.0) as i32);
                    if !self.label_bmps.contains_key(&key) {
                        if let Some(l) = self.labels.get(&key) {
                            let h = r::text_height(l);
                            match self.gpu.shadowed_label(l, cell.w - 4.0, h, self.scale, Self::desktop_theme().text) {
                                Ok(b) => {
                                    self.label_bmps.insert(key, b);
                                }
                                Err(e) => log::warn!("label bitmap: {e}"),
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn draw(&mut self) {
        let t = Instant::now();
        self.step_roll_anim();
        self.vanish_animating();
        self.prepare();
        let Some(surf) = self.surf.take() else { return };
        let res = surf.draw(self.scale, |dc| self.paint(dc));
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
        crate::anim::set(self.animating());
    }

    pub fn mon_offset(&self) -> (f32, f32) {
        ((self.mon.left - self.origin_px.x) as f32 / self.scale, (self.mon.top - self.origin_px.y) as f32 / self.scale)
    }

    /// Explorer's drag image: the dragged icons and labels, half transparent, moved by (dx, dy)
    /// so they keep their offsets from the pointer.
    fn paint_ghost(&self, dc: &Dc, keys: &[ItemKey], dx: f32, dy: f32, alpha: f32) -> r::R<()> {
        let ghost_text = Color { a: (1.25 * alpha).min(1.0), ..Self::desktop_theme().text };
        for (k, cell, _) in self.visible_cells().iter().filter(|(k, _, _)| keys.contains(k)).take(64) {
            let (icon, label) = self.item_parts(k, *cell, false);
            if self.privacy_on() {
                let name = self.items.get(k).map(|i| i.display_name.as_str()).unwrap_or("");
                self.paint_private(dc, name, icon.offset(dx, dy), label.offset(dx, dy), cell.offset(dx, dy), &Self::desktop_theme(), true, alpha)?;
                continue;
            }
            if let Some(bmp) = self.items.get(k).and_then(|i| self.bitmaps.get(&i.icon)) {
                r::draw_bitmap(dc, bmp, rf(fit_icon(bmp, icon).offset(dx, dy)), alpha);
            }
            let lk = (k.clone(), false, (cell.w - 4.0) as i32);
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
        for (_, cell, _) in self.visible_cells().iter().filter(|(k, _, _)| keys.contains(k)).take(64) {
            x0 = x0.min(cell.x - m);
            y0 = y0.min(cell.y - m);
            x1 = x1.max(cell.right() + m);
            y1 = y1.max(cell.bottom() + m);
        }
        let reach = 480.0;
        let (x0, y0, x1, y1) = (x0.max(at.x - reach), y0.max(at.y - reach), x1.min(at.x + reach), y1.min(at.y + reach));
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let s = self.scale;
        let (w, h) = (((x1 - x0) * s).ceil() as u32, ((y1 - y0) * s).ceil() as u32);
        let px = self.gpu.render_pixels(w, h, s, (x0, y0), |dc| self.paint_ghost(dc, keys, 0.0, 0.0, 0.85)).ok()?;
        Some((w, h, px, (((at.x - x0) * s).round() as i32, ((at.y - y0) * s).round() as i32)))
    }

    fn paint(&self, dc: &Dc) -> r::R<()> {
        let above = self.is_above();
        if above {
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
        let dragging: Vec<ItemKey> = match &self.drag {
            Drag::Items { keys, .. } => keys.clone(),
            _ => vec![],
        };
        let dt = Self::desktop_theme();
        if !self.settings.show_icons {
            return Ok(());
        }
        // Hidden with a double-click (Settings › Hide all): fading away or back.
        let all = rf(Rect::new(0.0, 0.0, self.size_dip.0, self.size_dip.1));
        let ta = if above { 0.0 } else { self.tuck_alpha(None) };
        if ta > 0.002 {
            if ta < 0.998 {
                r::push_layer(dc, all, ta)?;
            }
            // Loose items (where they settle while a box is dragged, gliding there).
            for (k, c) in self.loose_view().iter() {
                if self.items.contains_key(k) {
                    let cell = self.glided(k, self.grid.cell_rect(*c));
                    let fx = self.icon_fx(None, k);
                    self.paint_item(dc, k, cell, &dt, true, 1.0, fx)?;
                }
            }
            self.paint_vanishing(dc, None, &dt, true)?;
            if ta < 0.998 {
                r::pop_layer(dc);
            }
        }
        // Boxes, in z-order.
        for b in &self.layout.boxes {
            let ba = self.tuck_alpha(Some(b));
            if ba <= 0.002 {
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
        // Drag feedback.
        match &self.drag {
            Drag::Items { keys, start, cur } => {
                let cells = self.visible_cells();
                self.paint_ghost(dc, &dragging, cur.x - start.x, cur.y - start.y, 0.6)?;
                // How many are being dragged.
                if self.settings.extras.drag_count && keys.len() > 1 {
                    let (ar, ag, ab) = accent();
                    let txt = keys.len().to_string();
                    if let Ok(l) = self.gpu.label(&self.az_tf, &txt, 60.0, 16.0, 1, false) {
                        let w = (r::text_width(&l) + 10.0).max(18.0);
                        let bx = Rect::new((cur.x + 12.0).round(), (cur.y + 14.0).round(), w, 18.0);
                        r::fill_round(dc, rf(bx.offset(0.0, 1.0)), 9.0, rgba(0, 0, 0, 0.3))?;
                        r::fill_round(dc, rf(bx), 9.0, rgba(ar, ag, ab, 1.0))?;
                        r::draw_text(dc, &l, bx.x + (w - r::text_width(&l)) / 2.0, bx.y + (18.0 - r::text_height(&l)) / 2.0, rgba(255, 255, 255, 1.0))?;
                    }
                }
                let (ar, ag, ab) = accent();
                if let Some(t) = self.drop_onto_item(*cur, keys) {
                    // Dropping into a folder / onto a program: highlight it, as Explorer does.
                    if let Some((_, cell, _)) = cells.iter().find(|(k, _, _)| *k == t) {
                        let hr = self.hit_rect(&t, *cell);
                        r::fill_rect(dc, rf(hr), rgba(ar, ag, ab, 0.35))?;
                        r::stroke_round(dc, rf(hr.inflate(-0.5)), 0.0, rgba(ar, ag, ab, 0.9), 1.0)?;
                    }
                } else if let Some(b) = self.drop_box(*cur).and_then(|id| self.box_by_id(id)) {
                    r::stroke_round(dc, rf(b.rect), radius(), rgba(ar, ag, ab, 0.9), 2.0)?;
                    // Insertion marker where the items will go.
                    if self.fully_open(b) && cur.y >= b.rect.y + header_h() && !b.az {
                        let (_, at) = self.box_drop_slot(b, *cur);
                        let c = App::content_rect(b);
                        r::push_clip(dc, rf(c));
                        r::fill_round(dc, rf(Rect::new(at.x - 1.5, at.y + 6.0, 3.0, self.grid.cell_h - 14.0)), 1.5, rgba(ar, ag, ab, 1.0))?;
                        r::pop_clip(dc);
                    }
                }
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
            Drag::Rubber { start, cur, .. } | Drag::RightRubber { start, cur } => {
                let band = Rect::from_points(*start, *cur);
                let (ar, ag, ab) = accent();
                // A band started in a box stays inside it (it selects only that box's icons).
                let clip = match &self.drag {
                    Drag::Rubber { scope: Some(id), .. } => self.box_by_id(*id).map(App::content_rect),
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
            }
            _ => {}
        }
        self.paint_search(dc)?;
        self.paint_tip(dc)?;
        if self.diag {
            self.paint_diagnostics(dc)?;
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

    fn paint_item(&self, dc: &Dc, k: &ItemKey, cell: Rect, th: &Theme, shadow: bool, opacity: f32, fx: crate::iconfx::IconFx) -> r::R<()> {
        let Some(it) = self.items.get(k) else { return Ok(()) };
        // Searching: what does not match fades.
        let opacity = opacity * self.search_alpha(k);
        let full = self.is_full_label(k);
        let (icon, label) = self.item_parts(k, cell, full);
        let sel = self.sel.is_selected(k);
        let hit = self.hit_rect(k, cell);
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
        if let Some(b) = self.bitmaps.get(&it.icon) {
            if fx.frost > 0.0 {
                if let Some(bd) = self.backdrops.get(&fx.frost_blur) {
                    let s = self.backdrop_down as f32 / self.scale;
                    let light = if self.light { rgba(255, 255, 255, 0.32) } else { rgba(255, 255, 255, 0.20) };
                    r::draw_frosted(dc, b, rf(fit_icon(b, icon)), bd, s, self.mon_offset(), light, fx.frost * opacity * cut_ghost)?;
                }
            }
            match fx.tint {
                Some((c, s)) => r::draw_bitmap_tinted(dc, b, rf(fit_icon(b, icon)), opacity * cut_ghost * fx.icon_a, c, s)?,
                None => r::draw_bitmap(dc, b, rf(fit_icon(b, icon)), opacity * cut_ghost * fx.icon_a),
            }
        }
        let opacity = opacity * fx.label_a;
        let lk = (k.clone(), full, (cell.w - 4.0) as i32);
        match (shadow, self.label_bmps.get(&lk), self.labels.get(&lk)) {
            (true, Some(b), Some(l)) => {
                // The shadow (cached), then the text with ClearType, as Explorer draws it.
                let sz = unsafe { b.GetPixelSize() };
                let m = r::LABEL_MARGIN;
                r::draw_bitmap(dc, b, r::rect(label.x - m, label.y - m, sz.width as f32 / self.scale, sz.height as f32 / self.scale), opacity);
                r::draw_text_cleartype(dc, l, label.x, label.y, Color { a: th.text.a * opacity, ..th.text })?;
            }
            (true, None, Some(l)) => r::draw_text_shadowed(dc, l, label.x, label.y, Color { a: th.text.a * opacity, ..th.text })?,
            (false, _, Some(l)) => r::draw_text(dc, l, label.x, label.y, Color { a: th.text.a * opacity, ..th.text })?,
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
        let tint = st.color.map(|c| rgba(c.r, c.g, c.b, 1.0)).unwrap_or(auto_tint);
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
                    if let Some(w) = &self.wall {
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
        let shown_title = if look.titles_on_hover && open && self.box_edit.is_none() && self.rename.is_none() && self.search.scope != Some(b.id) {
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
            "box" => Some(st.color.map(|c| rgba(c.r, c.g, c.b, 0.95 * bk)).unwrap_or(faint)),
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
            // A folder box in a subfolder says which.
            let title = self.folder_title(b).filter(|_| n == 1).unwrap_or_else(|| tab.title.clone());
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
            let back = !b.folder_at.is_empty();
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
        let top = header_h() + crate::az::bar_h(b);
        let vis = Rect::new(content.x, rect.y + top, content.w, (rect.bottom() - rect.y - top - 2.0).max(0.0));
        let fade = if open { 1.0 } else { o };
        let tagged = self.az_tagged(b);
        r::push_clip(dc, rf(vis));
        for (i, (k, cell)) in self.box_item_cells(b).into_iter().enumerate() {
            let cell = cell.offset(0.0, dy);
            if cell.bottom() < vis.y || cell.y > vis.bottom() {
                continue;
            }
            let a = ta.map(|(_, t)| t).unwrap_or(1.0) * fade;
            let fx = self.icon_fx(Some(b), &k);
            self.paint_item(dc, &k, cell, &th, !light, a, fx)?;
            if let Some((_, c)) = tagged.iter().find(|t| t.0 == i) {
                if !self.privacy_on() {
                    self.paint_az_tag(dc, *c, cell, th.text, !light, a * self.search_alpha(&k))?;
                }
            }
        }
        self.paint_vanishing(dc, Some(b.id), &th, !light)?;
        r::pop_clip(dc);
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

    /// The box title being edited in place: a darker field with the accent underline of a focused
    /// Windows text box, selection, and a blinking caret.
    fn paint_title_field(&self, dc: &Dc, light: bool) -> r::R<()> {
        let (Some((field, l, x)), Some(be)) = (self.box_edit_layout(), self.box_edit.as_ref()) else {
            return Ok(());
        };
        let (bg, fg, edge) = if light {
            (rgba(255, 255, 255, 0.75), rgba(0, 0, 0, 0.9), rgba(0, 0, 0, 0.12))
        } else {
            (rgba(0, 0, 0, 0.38), rgba(255, 255, 255, 1.0), rgba(255, 255, 255, 0.14))
        };
        r::fill_round(dc, rf(field), 5.0, bg)?;
        r::stroke_round(dc, rf(field.inflate(-0.5)), 5.0, edge, 1.0)?;
        let (ar, ag, ab) = accent();
        r::fill_round(dc, rf(Rect::new(field.x + 2.0, field.bottom() - 2.0, field.w - 4.0, 2.0)), 1.0, rgba(ar, ag, ab, 1.0))?;
        r::push_clip(dc, rf(Rect::new(field.x + 4.0, field.y, field.w - 8.0, field.h)));
        let h = r::text_height(&l);
        let y = field.y + (field.h - h) / 2.0;
        let (a, b) = be.ed.sel();
        if a != b {
            let (x0, x1) = (r::caret_x(&l, be.ed.utf16_pos(a)), r::caret_x(&l, be.ed.utf16_pos(b)));
            r::fill_rect(dc, rf(Rect::new(x + x0, y, x1 - x0, h)), rgba(ar, ag, ab, if light { 0.45 } else { 0.7 }))?;
        }
        r::draw_text(dc, &l, x, y, fg)?;
        if be.caret_on {
            let cx = x + r::caret_x(&l, be.ed.utf16_pos(be.ed.caret));
            r::fill_rect(dc, rf(Rect::new((cx * self.scale).round() / self.scale, y, 1.0, h)), fg)?;
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
