//! How icons are colored (Settings › Look › Icons; each box can differ, Settings › Boxes):
//! - tint: the icons take a color (the box's own, or a picked one), shaded by their lightness;
//! - blend into the wallpaper: icons turn into frosted glass (the blurred wallpaper seen through
//!   their shape, a little lighter, with a faint trace of the icon) and their names fade, so the
//!   desktop looks calm; pointing at a box brings its icons back (fading), and pointing at a
//!   desktop icon brings that icon back.

use crate::app::{accent, ease_out, App};
use deskboxes_core::*;
use deskboxes_render::{rgba, Color};
use std::time::Instant;

/// Icons coming back (or blending in again) when the pointer enters (or leaves) a box, s.
const REVEAL_S: f32 = 0.22;

/// How one icon is drawn: tinted (color, strength), and how opaque the icon and its label are.
#[derive(Clone, Copy)]
pub struct IconFx {
    pub tint: Option<(Color, f32)>,
    pub icon_a: f32,
    pub label_a: f32,
    /// Frosted glass, 0..1 (blending in).
    pub frost: f32,
    /// Blur of the backdrop to see through (a key of `App::backdrops`).
    pub frost_blur: i32,
}

impl IconFx {
    pub const NONE: IconFx = IconFx { tint: None, icon_a: 1.0, label_a: 1.0, frost: 0.0, frost_blur: 0 };
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
            "box" => match b.and_then(|b| b.style.color) {
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
        // See through the most blurred wallpaper there is (the frost).
        let frost_blur = self.backdrops.keys().copied().max().unwrap_or(0);
        IconFx {
            tint: tint.map(|t| (t, s.tint_strength.clamp(0.0, 1.0))),
            icon_a: 1.0 - blend,
            label_a: 1.0 - 0.7 * blend,
            frost: blend,
            frost_blur,
        }
    }
}
