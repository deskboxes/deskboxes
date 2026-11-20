//! Hide all with a double-click (Settings › Hide all; off by default): double-clicking the bare
//! desktop fades the boxes and desktop icons away, leaving only the wallpaper; double-clicking
//! again (or starting a search) brings them back. A box can stay visible (Box settings › Stay
//! visible when hiding all). Nothing is moved: it is only how the desktop is drawn.

use crate::app::{ease_out, App};
use deskboxes_core::*;
use std::time::Instant;

/// Fading away or back, s.
const TUCK_S: f32 = 0.25;

impl App {
    /// Whether this box (None: the desktop icons) is hidden now, so the pointer goes through it.
    pub fn is_tucked(&self, b: Option<&BoxModel>) -> bool {
        self.tucked.0 && !b.map(|b| b.stay_visible).unwrap_or(false)
    }

    /// How visible a box (None: the desktop icons) is, 0..1, fading.
    pub fn tuck_alpha(&self, b: Option<&BoxModel>) -> f32 {
        if b.map(|b| b.stay_visible).unwrap_or(false) {
            return 1.0;
        }
        let (on, t0) = self.tucked;
        let t = ease_out(t0.elapsed().as_secs_f32() / TUCK_S);
        if on {
            1.0 - t
        } else {
            t
        }
    }

    /// Hide everything, or bring it back.
    pub fn set_tucked(&mut self, on: bool) {
        if self.tucked.0 == on {
            return;
        }
        self.tucked = (on, Instant::now());
        if on {
            self.sel.clear();
            self.end_rename(true);
        }
        crate::anim::set(true);
        self.invalidate();
    }

    pub fn tuck_animating(&self) -> bool {
        self.tucked.1.elapsed().as_secs_f32() < TUCK_S
    }
}
