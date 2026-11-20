//! Drawing for the setup window: Direct2D into a DirectComposition surface on the window
//! (premultiplied alpha, so the Mica backdrop shows through where nothing is drawn), DirectWrite
//! text in Segoe UI Variable, glyphs from Segoe Fluent Icons. All coordinates are DIPs (scaled to
//! the window's DPI by the transform).
//!
//! Why DirectComposition and not an HWND render target: the window has no redirection bitmap
//! (`WS_EX_NOREDIRECTIONBITMAP`), as WinUI windows do, so DWM draws Mica behind the visual and
//! composes our premultiplied pixels over it. (An `ID2D1HwndRenderTarget` on a frame-extended
//! window showed up washed out and half transparent on this PC.)

use crate::theme::C;
use windows::core::{w, Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows_numerics::{Matrix3x2, Vector2};

pub type R<T> = windows::core::Result<T>;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn r(&self) -> f32 {
        self.x + self.w
    }
    pub fn b(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.r() && py >= self.y && py < self.b()
    }
    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }
    pub fn offset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
    fn d2(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.r(), bottom: self.b() }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Font {
    Title,
    Body,
    Caption,
    /// Segoe Fluent Icons at the given size.
    Icon(u32),
}

#[derive(Clone, Copy, Default)]
pub struct TextOpts {
    pub center: bool,
    pub right: bool,
    /// One line, trimmed with an ellipsis; with `path`, the last path segment stays visible.
    pub one_line: bool,
    pub path: bool,
    pub vcenter: bool,
}

/// GPU objects: D3D11 → D2D device → a DirectComposition visual on the window whose content is a
/// premultiplied surface.
struct Device {
    dcomp: IDCompositionDesktopDevice,
    _target: IDCompositionTarget,
    visual: IDCompositionVisual2,
    surface: Option<IDCompositionSurface>,
    brush: Option<ID2D1SolidColorBrush>,
}

pub struct Gfx {
    dwrite: IDWriteFactory,
    wic: IWICImagingFactory,
    dev: Option<Device>,
    /// The device context of the frame being drawn (between `begin` and `end`).
    dc: Option<ID2D1DeviceContext>,
    hwnd: HWND,
    pub dpi: f32,
    size: (u32, u32),
    families: (String, String, String, String),
    formats: Vec<(Font, IDWriteTextFormat)>,
    png: &'static [u8],
    icons: Vec<(u32, ID2D1Bitmap)>,
    /// Multiplies every color's alpha (page entrance fades).
    pub alpha: f32,
    /// Vertical offset applied to everything drawn (page entrance rise).
    pub dy: f32,
}

fn cf(c: C, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a * alpha }
}

impl Gfx {
    pub fn new(hwnd: HWND, dpi: f32, png: &'static [u8]) -> R<Gfx> {
        unsafe {
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let has = |name: &str| -> bool {
                let mut coll = None;
                if dwrite.GetSystemFontCollection(&mut coll, false).is_err() {
                    return false;
                }
                let Some(coll) = coll else { return false };
                let (mut ix, mut exists) = (0u32, windows::core::BOOL(0));
                coll.FindFamilyName(&HSTRING::from(name), &mut ix, &mut exists).is_ok() && exists.as_bool()
            };
            let pick = |a: &str| if has(a) { a.to_string() } else { "Segoe UI".to_string() };
            let icons = if has("Segoe Fluent Icons") { "Segoe Fluent Icons".to_string() } else { "Segoe MDL2 Assets".to_string() };
            let families = (pick("Segoe UI Variable Display"), pick("Segoe UI Variable Text"), pick("Segoe UI Variable Small"), icons);
            Ok(Gfx { dwrite, wic, dev: None, dc: None, hwnd, dpi, size: (1, 1), families, formats: vec![], png, icons: vec![], alpha: 1.0, dy: 0.0 })
        }
    }

    pub fn set_dpi(&mut self, dpi: f32) {
        if (dpi - self.dpi).abs() > 0.01 {
            self.dpi = dpi;
            self.icons.clear();
        }
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        let s = (w.max(1), h.max(1));
        if s != self.size {
            self.size = s;
            if let Some(d) = &mut self.dev {
                d.surface = None;
            }
        }
    }

    fn device(&mut self) -> R<&mut Device> {
        if self.dev.is_none() {
            unsafe {
                let mut d3d: Option<ID3D11Device> = None;
                let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
                if D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(&mut d3d), None, None).is_err() {
                    // No usable GPU (VM, remote session, driver reset): WARP.
                    D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(&mut d3d), None, None)?;
                }
                let dxgi: IDXGIDevice = d3d.unwrap().cast()?;
                let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
                let d2d = factory.CreateDevice(&dxgi)?;
                let dcomp: IDCompositionDesktopDevice = DCompositionCreateDevice2(&d2d)?;
                let target = dcomp.CreateTargetForHwnd(self.hwnd, true)?;
                let visual = dcomp.CreateVisual()?;
                target.SetRoot(&visual)?;
                self.icons.clear();
                self.dev = Some(Device { dcomp, _target: target, visual, surface: None, brush: None });
            }
        }
        Ok(self.dev.as_mut().unwrap())
    }

    /// Start a frame, cleared to `bg` (transparent where Mica should show).
    pub fn begin(&mut self, bg: C) -> R<()> {
        let (w, h) = self.size;
        let scale = self.dpi / 96.0;
        let d = self.device()?;
        unsafe {
            if d.surface.is_none() {
                let s = d.dcomp.CreateSurface(w, h, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_ALPHA_MODE_PREMULTIPLIED)?;
                d.visual.SetContent(&s)?;
                d.surface = Some(s);
            }
            let mut off = POINT::default();
            let dc: ID2D1DeviceContext = d.surface.as_ref().unwrap().BeginDraw(None, &mut off)?;
            dc.SetTransform(&Matrix3x2 { M11: scale, M12: 0.0, M21: 0.0, M22: scale, M31: off.x as f32, M32: off.y as f32 });
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            dc.Clear(Some(&cf(bg, 1.0)));
            if d.brush.is_none() {
                d.brush = Some(dc.CreateSolidColorBrush(&cf(C::black(1.0), 1.0), None)?);
            }
            self.dc = Some(dc);
        }
        self.alpha = 1.0;
        self.dy = 0.0;
        Ok(())
    }

    /// End the frame and show it. A lost device is dropped (recreated next frame).
    pub fn end(&mut self) {
        self.dc = None;
        let Some(d) = &self.dev else { return };
        let r = unsafe {
            match &d.surface {
                Some(s) => s.EndDraw().and_then(|_| d.dcomp.Commit()),
                None => Ok(()),
            }
        };
        if let Err(e) = r {
            crate::ops::log(&format!("present: {e}; recreating the device"));
            self.dev = None;
            self.icons.clear();
        }
    }

    fn rt(&self) -> &ID2D1DeviceContext {
        self.dc.as_ref().unwrap()
    }

    fn br(&self, c: C) -> &ID2D1SolidColorBrush {
        let b = self.dev.as_ref().unwrap().brush.as_ref().unwrap();
        unsafe { b.SetColor(&cf(c, self.alpha)) };
        b
    }

    fn sh(&self, r: Rect) -> Rect {
        r.offset(0.0, self.dy)
    }

    pub fn fill(&self, r: Rect, radius: f32, c: C) {
        let r = self.sh(r);
        unsafe {
            if radius <= 0.0 {
                self.rt().FillRectangle(&r.d2(), self.br(c));
            } else {
                self.rt().FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r.d2(), radiusX: radius, radiusY: radius }, self.br(c));
            }
        }
    }

    /// A 1-DIP (or `width`) stroke inside `r`.
    pub fn stroke(&self, r: Rect, radius: f32, c: C, width: f32) {
        let r = self.sh(r).inset(width / 2.0);
        let rad = (radius - width / 2.0).max(0.0);
        unsafe {
            self.rt().DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r.d2(), radiusX: rad, radiusY: rad }, self.br(c), width, None);
        }
    }

    fn gradient(&self, stops: &[(f32, C)], from: (f32, f32), to: (f32, f32)) -> R<ID2D1LinearGradientBrush> {
        unsafe {
            let st: Vec<D2D1_GRADIENT_STOP> = stops.iter().map(|(p, c)| D2D1_GRADIENT_STOP { position: *p, color: cf(*c, self.alpha) }).collect();
            let coll = ID2D1RenderTarget::CreateGradientStopCollection(self.rt(), &st, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
            self.rt().CreateLinearGradientBrush(
                &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: from.0, Y: from.1 + self.dy }, endPoint: Vector2 { X: to.0, Y: to.1 + self.dy } },
                None,
                &coll,
            )
        }
    }

    /// A control's elevation border: `edge` is the stronger color, at the bottom 3 DIPs
    /// (`bottom`) or the top 3 DIPs.
    pub fn stroke_elevation(&self, r: Rect, radius: f32, base: C, edge: C, bottom: bool) {
        let (from, to, stops) = if bottom { ((r.x, r.b() - 3.0), (r.x, r.b()), [(0.0, base), (1.0, edge)]) } else { ((r.x, r.y), (r.x, r.y + 3.0), [(0.0, edge), (1.0, base)]) };
        let Ok(b) = self.gradient(&stops, from, to) else { return };
        let rr = self.sh(r).inset(0.5);
        unsafe {
            self.rt().DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rr.d2(), radiusX: radius - 0.5, radiusY: radius - 0.5 }, &b, 1.0, None);
        }
    }

    /// A rounded rect filled with a diagonal gradient (the settings window's hero tile).
    pub fn fill_gradient(&self, r: Rect, radius: f32, c0: C, c1: C) {
        let Ok(b) = self.gradient(&[(0.0, c0), (1.0, c1)], (r.x, r.y), (r.r(), r.b())) else { return };
        let rr = self.sh(r);
        unsafe {
            self.rt().FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rr.d2(), radiusX: radius, radiusY: radius }, &b);
        }
    }

    fn format(&mut self, f: Font) -> R<IDWriteTextFormat> {
        if let Some((_, tf)) = self.formats.iter().find(|(k, _)| *k == f) {
            return Ok(tf.clone());
        }
        let (fam, size, weight) = match f {
            Font::Title => (self.families.0.clone(), 28.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            Font::Body => (self.families.1.clone(), 14.0, DWRITE_FONT_WEIGHT_NORMAL),
            Font::Caption => (self.families.2.clone(), 12.0, DWRITE_FONT_WEIGHT_NORMAL),
            Font::Icon(s) => (self.families.3.clone(), s as f32, DWRITE_FONT_WEIGHT_NORMAL),
        };
        let tf = unsafe {
            let fam = HSTRING::from(fam);
            self.dwrite.CreateTextFormat(PCWSTR(fam.as_ptr()), None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!(""))?
        };
        self.formats.push((f, tf.clone()));
        Ok(tf)
    }

    fn layout(&mut self, s: &str, f: Font, w: f32, h: f32, o: TextOpts) -> R<IDWriteTextLayout> {
        let tf = self.format(f)?;
        unsafe {
            let wide: Vec<u16> = s.encode_utf16().collect();
            let l = self.dwrite.CreateTextLayout(&wide, &tf, w.max(1.0), h.max(1.0))?;
            l.SetTextAlignment(if o.center {
                DWRITE_TEXT_ALIGNMENT_CENTER
            } else if o.right {
                DWRITE_TEXT_ALIGNMENT_TRAILING
            } else {
                DWRITE_TEXT_ALIGNMENT_LEADING
            })?;
            if o.vcenter {
                l.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            }
            if o.one_line {
                l.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                let sign = self.dwrite.CreateEllipsisTrimmingSign(&tf)?;
                let t = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: if o.path { '\\' as u32 } else { 0 },
                    delimiterCount: if o.path { 1 } else { 0 },
                };
                l.SetTrimming(&t, &sign)?;
            } else {
                l.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            }
            Ok(l)
        }
    }

    /// Size of `s` laid out in width `w` (wrapping unless `one_line`).
    pub fn measure(&mut self, s: &str, f: Font, w: f32, o: TextOpts) -> (f32, f32) {
        let Ok(l) = self.layout(s, f, w, 10_000.0, o) else { return (0.0, 0.0) };
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = l.GetMetrics(&mut m);
        }
        (m.widthIncludingTrailingWhitespace, m.height)
    }

    /// Draw `s` in `r`; returns the height it took.
    pub fn text(&mut self, s: &str, f: Font, r: Rect, c: C, o: TextOpts) -> f32 {
        let Ok(l) = self.layout(s, f, r.w, r.h, o) else { return 0.0 };
        let r = self.sh(r);
        unsafe {
            self.rt().DrawTextLayout(Vector2 { X: r.x, Y: r.y }, &l, self.br(c), D2D1_DRAW_TEXT_OPTIONS_NONE);
            let mut m = DWRITE_TEXT_METRICS::default();
            let _ = l.GetMetrics(&mut m);
            m.height
        }
    }

    /// One Fluent icon glyph centered on (cx, cy).
    pub fn glyph(&mut self, ch: char, size: u32, cx: f32, cy: f32, c: C) {
        let s = ch.to_string();
        let box_ = size as f32 * 2.0;
        self.text(&s, Font::Icon(size), Rect::new(cx - box_ / 2.0, cy - box_ / 2.0, box_, box_), c, TextOpts { center: true, vcenter: true, ..Default::default() });
    }

    /// The app icon in `r`, pre-scaled with WIC to the exact pixel size (sharpest at small sizes).
    pub fn app_icon(&mut self, r: Rect) {
        let px = (r.w * self.dpi / 96.0).round().max(1.0) as u32;
        if !self.icons.iter().any(|(s, _)| *s == px) {
            match self.load_icon(px) {
                Ok(b) => self.icons.push((px, b)),
                Err(_) => return,
            }
        }
        let b = &self.icons.iter().find(|(s, _)| *s == px).unwrap().1;
        let rr = self.sh(r);
        unsafe {
            self.rt().DrawBitmap(b, Some(&rr.d2()), self.alpha, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None);
        }
    }

    fn load_icon(&self, px: u32) -> R<ID2D1Bitmap> {
        unsafe {
            let stream = windows::Win32::UI::Shell::SHCreateMemStream(Some(self.png)).ok_or_else(|| windows::core::Error::from(E_OUTOFMEMORY))?;
            let dec = self.wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)?;
            let frame = dec.GetFrame(0)?;
            let sc = self.wic.CreateBitmapScaler()?;
            sc.Initialize(&frame, px, px, WICBitmapInterpolationModeHighQualityCubic)?;
            let conv = self.wic.CreateFormatConverter()?;
            conv.Initialize(&sc, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
            let src: IWICBitmapSource = conv.cast()?;
            let b1: ID2D1Bitmap1 = self.rt().CreateBitmapFromWicBitmap(&src, None)?;
            b1.cast()
        }
    }
}
