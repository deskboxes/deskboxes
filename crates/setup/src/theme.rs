//! Colors: the WinUI 3 (Fluent) theme resources the settings window uses, in light and dark, and
//! the Windows accent color (Settings › Personalization › Colors), so setup looks like the rest of
//! Deskboxes and like Windows Settings.

use windows::UI::ViewManagement::{UIColorType, UISettings};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct C {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl C {
    pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> C {
        C { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a }
    }
    pub const fn white(a: f32) -> C {
        C::rgba(255, 255, 255, a)
    }
    pub const fn black(a: f32) -> C {
        C::rgba(0, 0, 0, a)
    }
    pub fn with_a(self, a: f32) -> C {
        C { a, ..self }
    }
    pub fn mul_a(self, k: f32) -> C {
        C { a: self.a * k, ..self }
    }
    /// Straight-alpha mix of two colors (for hover transitions).
    pub fn lerp(self, o: C, t: f32) -> C {
        let t = t.clamp(0.0, 1.0);
        C { r: self.r + (o.r - self.r) * t, g: self.g + (o.g - self.g) * t, b: self.b + (o.b - self.b) * t, a: self.a + (o.a - self.a) * t }
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub dark: bool,
    /// Solid background when Mica is not available (Mica's own fallback color).
    pub base: C,
    pub text1: C,
    pub text2: C,
    pub text3: C,
    pub text_disabled: C,
    pub control: C,
    pub control_hover: C,
    pub control_pressed: C,
    pub control_disabled: C,
    pub control_stroke: C,
    /// The stronger edge of a button's elevation border (bottom in light, top in dark).
    pub control_stroke_edge: C,
    pub card: C,
    pub card_stroke: C,
    pub layer_alt: C,
    pub subtle_hover: C,
    pub subtle_pressed: C,
    pub strong: C,
    pub alt2: C,
    pub alt3: C,
    pub alt4: C,
    pub accent: C,
    pub accent_hover: C,
    pub accent_pressed: C,
    pub accent_disabled: C,
    pub accent_light1: C,
    pub accent_dark1: C,
    pub on_accent: C,
    pub on_accent_pressed: C,
    pub on_accent_disabled: C,
    pub on_accent_stroke_top: C,
    pub on_accent_stroke_bottom: C,
    pub focus_outer: C,
    pub focus_inner: C,
    pub critical: C,
    pub critical_dark: C,
}

fn to_c(c: windows::UI::Color) -> C {
    C::rgba(c.R, c.G, c.B, 1.0)
}

/// Accent shades from UISettings (they follow the accent and the Accent Colorizer). Defaults:
/// Windows' default blue.
fn accents() -> (C, C, C, C) {
    let get = || -> windows::core::Result<(C, C, C, C)> {
        let s = UISettings::new()?;
        Ok((
            to_c(s.GetColorValue(UIColorType::AccentLight2)?),
            to_c(s.GetColorValue(UIColorType::AccentLight1)?),
            to_c(s.GetColorValue(UIColorType::AccentDark1)?),
            to_c(s.GetColorValue(UIColorType::Accent)?),
        ))
    };
    get().unwrap_or((C::rgba(0x99, 0xEB, 0xFF, 1.0), C::rgba(0x4C, 0xC2, 0xFF, 1.0), C::rgba(0x00, 0x5F, 0xB8, 1.0), C::rgba(0x00, 0x78, 0xD4, 1.0)))
}

impl Theme {
    pub fn current() -> Theme {
        // DBX_SETUP_THEME=light|dark: tests (screenshots of both themes without changing Windows).
        let dark = match std::env::var("DBX_SETUP_THEME").as_deref() {
            Ok("light") => false,
            Ok("dark") => true,
            _ => !deskboxes_shell::reg::apps_use_light_theme(),
        };
        let (light2, light1, dark1, _accent) = accents();
        if dark {
            Theme {
                dark,
                base: C::rgba(0x20, 0x20, 0x20, 1.0),
                text1: C::white(1.0),
                text2: C::white(0.786),
                text3: C::white(0.545),
                text_disabled: C::white(0.363),
                control: C::white(0.0605),
                control_hover: C::white(0.0837),
                control_pressed: C::white(0.0326),
                control_disabled: C::white(0.0419),
                control_stroke: C::white(0.0698),
                control_stroke_edge: C::white(0.093),
                card: C::white(0.0512),
                card_stroke: C::black(0.1),
                layer_alt: C::rgba(0x3A, 0x3A, 0x3A, 0.45),
                subtle_hover: C::white(0.0605),
                subtle_pressed: C::white(0.0419),
                strong: C::white(0.5442),
                alt2: C::black(0.1),
                alt3: C::white(0.0419),
                alt4: C::white(0.0698),
                accent: light2,
                accent_hover: light2.with_a(0.9),
                accent_pressed: light2.with_a(0.8),
                accent_disabled: C::white(0.1581),
                accent_light1: light1,
                accent_dark1: dark1,
                on_accent: C::black(1.0),
                on_accent_pressed: C::black(0.5),
                on_accent_disabled: C::white(0.5302),
                on_accent_stroke_top: C::white(0.08),
                on_accent_stroke_bottom: C::black(0.14),
                focus_outer: C::white(1.0),
                focus_inner: C::black(0.7),
                critical: C::rgba(0xFF, 0x99, 0xA4, 1.0),
                critical_dark: C::rgba(0xC4, 0x2B, 0x1C, 1.0),
            }
        } else {
            Theme {
                dark,
                base: C::rgba(0xF3, 0xF3, 0xF3, 1.0),
                text1: C::black(0.8956),
                text2: C::black(0.6063),
                text3: C::black(0.4458),
                text_disabled: C::black(0.3614),
                control: C::white(0.7),
                control_hover: C::rgba(0xF9, 0xF9, 0xF9, 0.5),
                control_pressed: C::rgba(0xF9, 0xF9, 0xF9, 0.3),
                control_disabled: C::rgba(0xF9, 0xF9, 0xF9, 0.3),
                control_stroke: C::black(0.0578),
                control_stroke_edge: C::black(0.1622),
                card: C::white(0.7),
                card_stroke: C::black(0.0578),
                layer_alt: C::white(0.7),
                subtle_hover: C::black(0.0373),
                subtle_pressed: C::black(0.0241),
                strong: C::black(0.4458),
                alt2: C::black(0.0241),
                alt3: C::black(0.0578),
                alt4: C::black(0.0924),
                accent: dark1,
                accent_hover: dark1.with_a(0.9),
                accent_pressed: dark1.with_a(0.8),
                accent_disabled: C::black(0.2169),
                accent_light1: light1,
                accent_dark1: dark1,
                on_accent: C::white(1.0),
                on_accent_pressed: C::white(0.7),
                on_accent_disabled: C::white(1.0),
                on_accent_stroke_top: C::white(0.08),
                on_accent_stroke_bottom: C::black(0.4),
                focus_outer: C::black(0.8956),
                focus_inner: C::white(0.7),
                critical: C::rgba(0xC4, 0x2B, 0x1C, 1.0),
                critical_dark: C::rgba(0x9E, 0x1C, 0x10, 1.0),
            }
        }
    }
}
