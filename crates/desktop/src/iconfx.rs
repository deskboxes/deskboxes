//! How icons are colored (Settings › Look › Icons; each box can differ, Settings › Boxes):
//! - tint: like tinted icons on a phone, every icon becomes shades of one color (the box's own,
//!   or a picked one): its dark parts a deep shade, its light parts a pale one, white nearly
//!   white with a touch of the color. The strength mixes it with the icon's own colors.
//! - blend into the wallpaper: icons and their names take the wallpaper's color right behind
//!   them, keep only their shading and turn see-through, so the desktop looks calm; pointing at
//!   a box brings its icons back (fading), and pointing at a desktop icon brings that icon back.
//! Recolored icons are made once per icon and color (`App::fx_bitmap`), not per frame.

use crate::app::{accent, ease_out, App};
use deskboxes_core::*;
use deskboxes_render::{rgba, Color};
use std::time::Instant;

/// Icons coming back (or blending in again) when the pointer enters (or leaves) a box, s.
const REVEAL_S: f32 = 0.22;

/// How one icon is drawn: tinted (color, strength), and how far it blends into the wallpaper.
#[derive(Clone, Copy)]
pub struct IconFx {
    pub tint: Option<(Color, f32)>,
    /// Blending into the wallpaper, 0..1.
    pub blend: f32,
}

impl IconFx {
    pub const NONE: IconFx = IconFx { tint: None, blend: 0.0 };
}

/// How see-through a fully blended icon is.
pub const GHOST_A: f32 = 0.78;

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: 1.0 }
}

fn shade(a: Color, k: f32) -> Color {
    Color { r: a.r * k, g: a.g * k, b: a.b * k, a: 1.0 }
}

const WHITE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };

/// Lightness 0, .25, .5, .75, 1 → the tint's shades.
pub fn tint_ramp(t: Color) -> [Color; 5] {
    [shade(t, 0.16), shade(t, 0.55), mix(t, WHITE, 0.0), mix(t, WHITE, 0.42), mix(t, WHITE, 0.8)]
}

/// Lightness → the wallpaper color behind, darker and lighter (blending in).
pub fn blend_ramp(c: Color) -> [Color; 5] {
    [shade(c, 0.62), shade(c, 0.84), mix(c, WHITE, 0.10), mix(c, WHITE, 0.28), mix(c, WHITE, 0.52)]
}

/// The name of a blended icon: the wallpaper color behind, lightened (or darkened on a light
/// box), mixed in by `t`.
pub fn blend_text(text: Color, c: Color, t: f32) -> Color {
    let lum = 0.299 * text.r + 0.587 * text.g + 0.114 * text.b;
    let target = if lum > 0.5 { mix(c, WHITE, 0.55) } else { shade(c, 0.45) };
    let m = mix(text, target, t);
    Color { a: text.a, ..m }
}

/// Pack a color to 5 bits per channel (cache key) and back.
pub fn quantize(c: Color) -> (u32, Color) {
    let q = |v: f32| ((v.clamp(0.0, 1.0) * 255.0) as u32) >> 3;
    let (r, g, b) = (q(c.r), q(c.g), q(c.b));
    let f = |v: u32| (v << 3 | v >> 2) as f32 / 255.0;
    ((r << 10) | (g << 5) | b, Color { r: f(r), g: f(g), b: f(b), a: 1.0 })
}

/// "#RRGGBB" → color.
fn hex(s: &str) -> Option<Color> {
    let h = s.trim_start_matches('#');
    let x = u32::from_str_radix(h, 16).ok().filter(|_| h.len() == 6)?;
    Some(rgba((x >> 16) as u8, (x >> 8) as u8, x as u8, 1.0))
}

impl App {
    /// The tint a box's icons take (its own choice, else the one for all boxes).
    fn tint_of(&self, b: Option<&BoxModel>) -> Option<Color> {
        let own = b.map(|b| b.tint.as_str()).unwrap_or("");
        let pick = if own.is_empty() { self.settings.icons.tint.as_str() } else { own };
        match pick {
            "" | "off" => None,
            "box" => match b.and_then(App::box_color) {
                Some(c) => Some(rgba(c.r, c.g, c.b, 1.0)),
                None => {
                    let (r, g, bb) = accent();
                    Some(rgba(r, g, bb, 1.0))
                }
            },
            h => hex(h),
        }
    }

    fn blends(&self, b: Option<&BoxModel>) -> bool {
        // Above the windows there is no wallpaper to blend into: the boxes' own look.
        if self.is_above() {
            return false;
        }
        match b.map(|b| b.blend.as_str()).unwrap_or("") {
            "on" => true,
            "off" => false,
            _ => self.settings.icons.blend,
        }
    }

    /// How far the pointer is on box `id` (0..1, fading in and out).
    pub fn pointer_on(&self, id: u32) -> f32 {
        let (cur, prev, t0) = self.blend_hover;
        let t = ease_out(t0.elapsed().as_secs_f32() / REVEAL_S);
        if cur == Some(id) {
            t
        } else if prev == Some(id) {
            1.0 - t
        } else {
            0.0
        }
    }

    /// How far the desktop is "in use" (0..1, fading).
    fn desk_in_use(&self) -> f32 {
        let (on, t0) = self.desk_active;
        let t = ease_out(t0.elapsed().as_secs_f32() / REVEAL_S);
        if on {
            t
        } else {
            1.0 - t
        }
    }

    /// The desktop got or lost the keyboard focus.
    pub fn set_desk_active(&mut self, on: bool) {
        if self.desk_active.0 != on {
            self.desk_active = (on, Instant::now());
            if self.settings.icons.blend && self.settings.icons.full_when_active {
                crate::anim::set(true);
                self.invalidate();
            }
        }
    }

    /// How far a box's icons are back from blending (0 = blended, 1 = back), fading.
    fn revealed(&self, id: u32) -> f32 {
        let s = &self.settings.icons;
        let hover = if s.full_on_hover { self.pointer_on(id) } else { 0.0 };
        let desk = if s.full_when_active { self.desk_in_use() } else { 0.0 };
        hover.max(desk)
    }

    /// How far a box's own background and title blend in (0 = not at all).
    pub fn box_blend(&self, b: &BoxModel) -> f32 {
        let s = &self.settings.icons;
        if !s.blend_boxes || !self.blends(Some(b)) {
            return 0.0;
        }
        s.blend_strength.clamp(0.0, 1.0) * (1.0 - self.revealed(b.id))
    }

    /// The pointer moved: the box it is on brings its icons back.
    pub fn blend_pointer(&mut self, p: Pt) {
        let on = self.layout.boxes.iter().rev().find(|b| self.shown_rect(b).contains(p)).map(|b| b.id);
        if on != self.blend_hover.0 {
            self.blend_hover = (on, self.blend_hover.0, Instant::now());
            crate::anim::set(true);
            self.invalidate();
        }
    }

    pub fn blend_animating(&self) -> bool {
        self.blend_hover.2.elapsed().as_secs_f32() < REVEAL_S || self.desk_active.1.elapsed().as_secs_f32() < REVEAL_S
    }

    /// How the icon in `cell` is drawn: in box `b` (None: on the desktop).
    pub fn icon_fx(&self, b: Option<&BoxModel>, k: &ItemKey) -> IconFx {
        let s = &self.settings.icons;
        if b.is_none() && !s.desktop {
            return IconFx::NONE;
        }
        let tint = self.tint_of(b);
        let blend = if self.blends(b) {
            let back = match b {
                Some(b) => self.revealed(b.id),
                None => {
                    let on = if (s.full_on_hover && self.hover.as_ref() == Some(k)) || self.sel.is_selected(k) { 1.0 } else { 0.0 };
                    let desk = if s.full_when_active { self.desk_in_use() } else { 0.0 };
                    f32::max(on, desk)
                }
            };
            s.blend_strength.clamp(0.0, 1.0) * (1.0 - back)
        } else {
            0.0
        };
        if tint.is_none() && blend <= 0.0 {
            return IconFx::NONE;
        }
        IconFx {
            tint: tint.map(|t| (t, s.tint_strength.clamp(0.0, 1.0))),
            blend,
        }
    }

    /// The icon `id` (bitmap `src`) recolored: kind 0 = tinted with `c`, 1 = blended into
    /// wallpaper color `c`. Made once and cached.
    pub fn fx_bitmap(&self, id: u32, src: &deskboxes_render::Bitmap, c: Color, kind: u8) -> Option<deskboxes_render::Bitmap> {
        let (q, cq) = quantize(c);
        let key = (id, q, kind);
        if let Some(b) = self.fx_cache.borrow().get(&key) {
            return Some(b.clone());
        }
        let ramp = if kind == 0 { tint_ramp(cq) } else { blend_ramp(cq) };
        let b = self.gpu.recolor(src, ramp).map_err(|e| log::warn!("recolor icon: {e}")).ok()?;
        let mut cache = self.fx_cache.borrow_mut();
        if cache.len() > 4000 {
            cache.clear();
        }
        cache.insert(key, b.clone());
        Some(b)
    }

    /// The wallpaper's color behind a point (surface DIPs), from a small blurred copy.
    pub fn wall_color_at(&self, p: Pt) -> Color {
        let Some((w, h, px, step)) = &self.wall_grid else { return rgba(128, 128, 128, 1.0) };
        let (ox, oy) = self.mon_offset();
        let x = (((p.x - ox) * self.scale / step).floor().max(0.0) as u32).min(w.saturating_sub(1));
        let y = (((p.y - oy) * self.scale / step).floor().max(0.0) as u32).min(h.saturating_sub(1));
        let i = ((y * w + x) * 4) as usize;
        let c = match px.get(i..i + 4) {
            Some(b) => rgba(b[2], b[1], b[0], 1.0),
            None => return rgba(128, 128, 128, 1.0),
        };
        // A heavily blurred wallpaper averages toward grey: bring its hue back a little.
        let l = 0.299 * c.r + 0.587 * c.g + 0.114 * c.b;
        let sat = |v: f32| (l + (v - l) * 1.4).clamp(0.0, 1.0);
        Color { r: sat(c.r), g: sat(c.g), b: sat(c.b), a: 1.0 }
    }
}
