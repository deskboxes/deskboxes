//! Rendering (docs/ARCHITECTURE.md §4): one D3D11 device, a Direct2D device, a DirectComposition
//! desktop device; one composition surface per window. Mica is emulated from the blurred wallpaper
//! (DWM backdrops do not apply to a child of Progman).

use deskboxes_core::snapshot::IconImage;
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

pub use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F as Color, D2D_RECT_F as RectF};
pub use windows::Win32::Graphics::Direct2D::{ID2D1Bitmap1 as Bitmap, ID2D1DeviceContext as Dc};
pub use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout as TextLayout;

pub type R<T> = windows::core::Result<T>;

/// Margin (DIPs) around a label in its `shadowed_label` bitmap, room for the shadow.
pub const LABEL_MARGIN: f32 = 4.0;

pub fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

pub fn rect(x: f32, y: f32, w: f32, h: f32) -> RectF {
    RectF {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}

pub struct Gpu {
    pub factory: ID2D1Factory1,
    pub d2d: ID2D1Device,
    /// Resource context: bitmaps and effects created here are usable by every surface's context.
    pub rdc: ID2D1DeviceContext,
    pub dcomp: IDCompositionDesktopDevice,
    pub dwrite: IDWriteFactory,
    pub wic: IWICImagingFactory,
    /// Short round dashes (outlines drawn in privacy mode).
    pub dash: ID2D1StrokeStyle,
    /// Text layouts made by `label`, reused: titles, tabs and letters are drawn every frame and
    /// making a layout costs far more than drawing it.
    labels: std::cell::RefCell<std::collections::HashMap<(usize, String, u32, u32, u32, bool), TextLayout>>,
}

impl Gpu {
    pub fn new() -> R<Gpu> {
        unsafe {
            let mut dev: Option<ID3D11Device> = None;
            let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
            if D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(&mut dev), None, None).is_err() {
                // No usable GPU (VM, driver reset): WARP keeps the desktop working.
                D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(&mut dev), None, None)?;
            }
            let dxgi: IDXGIDevice = dev.unwrap().cast()?;
            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d = factory.CreateDevice(&dxgi)?;
            let rdc = d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let dcomp: IDCompositionDesktopDevice = DCompositionCreateDevice2(&d2d)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let base: ID2D1Factory = factory.cast()?;
            let dash = base.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_CUSTOM,
                    dashOffset: 0.0,
                },
                Some(&[2.2, 2.6]),
            )?;
            Ok(Gpu {
                factory,
                d2d,
                rdc,
                dcomp,
                dwrite,
                wic,
                dash,
                labels: Default::default(),
            })
        }
    }

    pub fn commit(&self) -> R<()> {
        unsafe { self.dcomp.Commit() }
    }

    fn props(target: bool) -> D2D1_BITMAP_PROPERTIES1 {
        D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: if target { D2D1_BITMAP_OPTIONS_TARGET } else { D2D1_BITMAP_OPTIONS_NONE },
            colorContext: std::mem::ManuallyDrop::new(None),
        }
    }

    /// Upload premultiplied BGRA pixels.
    pub fn bitmap(&self, img: &IconImage) -> R<Bitmap> {
        unsafe {
            self.rdc
                .CreateBitmap(D2D_SIZE_U { width: img.w, height: img.h }, Some(img.bgra.as_ptr() as _), img.w * 4, &Self::props(false))
        }
    }

    /// Decode an image file with WIC into a premultiplied bitmap.
    pub fn load_image(&self, path: &str) -> R<Bitmap> {
        self.load_image_scaled(path, 1.0).map(|(b, _)| b)
    }

    /// Pixel size of an image file without decoding its pixels.
    pub fn image_size(&self, path: &str) -> R<(u32, u32)> {
        unsafe {
            let p = HSTRING::from(path);
            let dec = self.wic.CreateDecoderFromFilename(PCWSTR(p.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
            let (mut w, mut h) = (0u32, 0u32);
            dec.GetFrame(0)?.GetSize(&mut w, &mut h)?;
            Ok((w, h))
        }
    }

    /// Decode an image scaled by `scale` (<= 1; WIC Fant scaler, so the full-size pixels are never
    /// held in memory). Returns the bitmap and the image's ORIGINAL pixel size (for layout).
    pub fn load_image_scaled(&self, path: &str, scale: f32) -> R<(Bitmap, (u32, u32))> {
        unsafe {
            let p = HSTRING::from(path);
            let dec = self.wic.CreateDecoderFromFilename(PCWSTR(p.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
            let frame = dec.GetFrame(0)?;
            let (mut w, mut h) = (0u32, 0u32);
            frame.GetSize(&mut w, &mut h)?;
            let conv = self.wic.CreateFormatConverter()?;
            if scale < 0.999 {
                let (sw, sh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
                let sc = self.wic.CreateBitmapScaler()?;
                sc.Initialize(&frame, sw, sh, WICBitmapInterpolationModeFant)?;
                conv.Initialize(&sc, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
            } else {
                conv.Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
            }
            Ok((self.rdc.CreateBitmapFromWicBitmap(&conv, None)?, (w, h)))
        }
    }

    /// Render `f` into a new offscreen bitmap of `w`×`h` pixels.
    pub fn offscreen(&self, w: u32, h: u32, f: impl FnOnce(&ID2D1DeviceContext) -> R<()>) -> R<Bitmap> {
        unsafe {
            let bmp = self.rdc.CreateBitmap(D2D_SIZE_U { width: w.max(1), height: h.max(1) }, None, 0, &Self::props(true))?;
            self.rdc.SetTarget(&bmp);
            self.rdc.BeginDraw();
            self.rdc.Clear(Some(&Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            let r = f(&self.rdc);
            let e = self.rdc.EndDraw(None, None);
            self.rdc.SetTarget(None);
            r?;
            e?;
            Ok(bmp)
        }
    }

    /// Draw `f` (in DIPs, `origin` at the top-left, `scale` DIP → pixel) into `w`×`h` pixels and
    /// read them back: premultiplied BGRA, top-down rows of `w * 4` bytes.
    pub fn render_pixels(&self, w: u32, h: u32, scale: f32, origin: (f32, f32), f: impl FnOnce(&ID2D1DeviceContext) -> R<()>) -> R<Vec<u8>> {
        let (w, h) = (w.max(1), h.max(1));
        let target = self.offscreen(w, h, |dc| unsafe {
            dc.SetTransform(&Matrix3x2 {
                M11: scale,
                M12: 0.0,
                M21: 0.0,
                M22: scale,
                M31: -origin.0 * scale,
                M32: -origin.1 * scale,
            });
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let r = f(dc);
            dc.SetTransform(&Matrix3x2::identity());
            r
        })?;
        unsafe {
            let mut p = Self::props(false);
            p.bitmapOptions = D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW;
            let read = self.rdc.CreateBitmap(D2D_SIZE_U { width: w, height: h }, None, 0, &p)?;
            read.CopyFromBitmap(None, &target, None)?;
            let m = read.Map(D2D1_MAP_OPTIONS_READ)?;
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for y in 0..h {
                out.extend_from_slice(std::slice::from_raw_parts(m.bits.add((y * m.pitch) as usize), (w * 4) as usize));
            }
            read.Unmap()?;
            Ok(out)
        }
    }

    /// An icon recolored like iOS tinted icons: its lightness mapped onto a ramp of one color
    /// (`ramp` = colors for lightness 0, 0.25, 0.5, 0.75, 1), shape and shading kept, alpha
    /// kept. Rendered once at the icon's own pixel size (callers cache it).
    pub fn recolor(&self, icon: &Bitmap, ramp: [Color; 5]) -> R<Bitmap> {
        unsafe {
            let sz = icon.GetPixelSize();
            let gray = self.rdc.CreateEffect(&CLSID_D2D1ColorMatrix)?;
            gray.SetInput(0, icon, true);
            // Perceived lightness into all three channels; alpha as it is.
            let l = [0.2126f32, 0.7152, 0.0722];
            let mut m = [0f32; 20];
            for i in 0..3 {
                for j in 0..3 {
                    m[i * 4 + j] = l[i];
                }
            }
            m[15] = 1.0;
            let mb: Vec<u8> = m.iter().flat_map(|f| f.to_le_bytes()).collect();
            gray.SetValue(D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32, D2D1_PROPERTY_TYPE_MATRIX_5X4, &mb)?;
            gray.SetValue(D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT.0 as u32, D2D1_PROPERTY_TYPE_BOOL, &1u32.to_le_bytes())?;
            let tt = self.rdc.CreateEffect(&CLSID_D2D1TableTransfer)?;
            tt.SetInput(0, &gray.GetOutput()?, true);
            let table = |f: fn(&Color) -> f32| -> Vec<u8> { ramp.iter().map(f).flat_map(|v| v.clamp(0.0, 1.0).to_le_bytes()).collect() };
            tt.SetValue(D2D1_TABLETRANSFER_PROP_RED_TABLE.0 as u32, D2D1_PROPERTY_TYPE_BLOB, &table(|c| c.r))?;
            tt.SetValue(D2D1_TABLETRANSFER_PROP_GREEN_TABLE.0 as u32, D2D1_PROPERTY_TYPE_BLOB, &table(|c| c.g))?;
            tt.SetValue(D2D1_TABLETRANSFER_PROP_BLUE_TABLE.0 as u32, D2D1_PROPERTY_TYPE_BLOB, &table(|c| c.b))?;
            tt.SetValue(D2D1_TABLETRANSFER_PROP_ALPHA_DISABLE.0 as u32, D2D1_PROPERTY_TYPE_BOOL, &1u32.to_le_bytes())?;
            tt.SetValue(D2D1_TABLETRANSFER_PROP_CLAMP_OUTPUT.0 as u32, D2D1_PROPERTY_TYPE_BOOL, &1u32.to_le_bytes())?;
            let out = tt.GetOutput()?;
            self.offscreen(sz.width, sz.height, |dc| {
                dc.DrawImage(&out, None, None, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
                Ok(())
            })
        }
    }

    /// Read a bitmap's pixels back (premultiplied BGRA rows), drawn at its own size.
    pub fn pixels_of(&self, b: &Bitmap) -> R<(u32, u32, Vec<u8>)> {
        let sz = unsafe { b.GetPixelSize() };
        let px = self.render_pixels(sz.width, sz.height, 1.0, (0.0, 0.0), |dc| unsafe {
            dc.DrawBitmap(b, None, 1.0, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None, None);
            Ok(())
        })?;
        Ok((sz.width, sz.height, px))
    }

    /// The wallpaper as Windows lays it out on a monitor of `mw`×`mh` px, downscaled by `down`, then
    /// Gaussian-blurred with `sigma` (in full-size px). `position` = DWPOS_*; `wall` None = solid.
    /// `wall` may be a scaled-down decode: `orig` is the image's original pixel size (layout).
    pub fn blurred_wallpaper(&self, wall: Option<(&Bitmap, (u32, u32))>, bg: Color, position: i32, mw: u32, mh: u32, down: u32, sigma: f32) -> R<Bitmap> {
        let (w, h) = (mw.div_ceil(down), mh.div_ceil(down));
        let base = self.offscreen(w, h, |dc| unsafe {
            dc.Clear(Some(&bg));
            if let Some((b, (ow, oh))) = wall {
                let dst = wallpaper_rect(position, ow as f32, oh as f32, mw as f32, mh as f32);
                let s = 1.0 / down as f32;
                let dst = RectF {
                    left: dst.left * s,
                    top: dst.top * s,
                    right: dst.right * s,
                    bottom: dst.bottom * s,
                };
                dc.DrawBitmap(b, Some(&dst), 1.0, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None);
            }
            Ok(())
        })?;
        let effect = unsafe { self.rdc.CreateEffect(&CLSID_D2D1GaussianBlur)? };
        unsafe {
            effect.SetInput(0, &base, true);
            effect.SetValue(D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32, D2D1_PROPERTY_TYPE_FLOAT, &(sigma / down as f32).to_le_bytes())?;
            effect.SetValue(D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32, D2D1_PROPERTY_TYPE_ENUM, &(D2D1_BORDER_MODE_HARD.0 as u32).to_le_bytes())?;
            effect.SetValue(
                D2D1_GAUSSIANBLUR_PROP_OPTIMIZATION.0 as u32,
                D2D1_PROPERTY_TYPE_ENUM,
                &(D2D1_GAUSSIANBLUR_OPTIMIZATION_QUALITY.0 as u32).to_le_bytes(),
            )?;
        }
        let out = unsafe { effect.GetOutput()? };
        self.offscreen(w, h, |dc| unsafe {
            dc.DrawImage(&out, None, None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
            Ok(())
        })
    }

    /// The shadow of a desktop label as Explorer draws it (measured against Explorer's own labels):
    /// a soft glow around the glyphs and a dark drop shadow down and to the right. Only the shadow:
    /// the text itself is drawn live over it with ClearType (`draw_text_cleartype`), as Explorer
    /// does. Rendered once into a bitmap with `LABEL_MARGIN` DIPs around the layout box (`w` × `h`
    /// DIPs); `scale` = DIP → pixel.
    pub fn shadowed_label(&self, l: &TextLayout, w: f32, h: f32, scale: f32, _text: Color) -> R<Bitmap> {
        let m = LABEL_MARGIN;
        let (pw, ph) = (((w + 2.0 * m) * scale).ceil() as u32, ((h + 2.0 * m) * scale).ceil() as u32);
        let sc = Matrix3x2 {
            M11: scale,
            M12: 0.0,
            M21: 0.0,
            M22: scale,
            M31: 0.0,
            M32: 0.0,
        };
        let id = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: 0.0,
            M32: 0.0,
        };
        let cl = unsafe {
            let cl = self.rdc.CreateCommandList()?;
            self.rdc.SetTarget(&cl);
            self.rdc.BeginDraw();
            self.rdc.SetTransform(&sc);
            let r = draw_text(&self.rdc, l, m, m, Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 });
            let e = self.rdc.EndDraw(None, None);
            self.rdc.SetTarget(None);
            self.rdc.SetTransform(&id);
            r?;
            e?;
            cl.Close()?;
            cl
        };
        // Blurred copies of the glyphs in black; `gain` > 1 strengthens the blur's core (the
        // alpha is scaled and clamped), which gives Explorer's dark edge right at the strokes.
        let shadow = |sigma: f32, gain: f32| -> R<ID2D1Image> {
            unsafe {
                let fx = self.rdc.CreateEffect(&CLSID_D2D1Shadow)?;
                fx.SetInput(0, &cl, true);
                fx.SetValue(D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0 as u32, D2D1_PROPERTY_TYPE_FLOAT, &(sigma * scale).to_le_bytes())?;
                let c: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0].iter().flat_map(|f| f.to_le_bytes()).collect();
                fx.SetValue(D2D1_SHADOW_PROP_COLOR.0 as u32, D2D1_PROPERTY_TYPE_VECTOR4, &c)?;
                let cm = self.rdc.CreateEffect(&CLSID_D2D1ColorMatrix)?;
                cm.SetInput(0, &fx.GetOutput()?, true);
                #[rustfmt::skip]
                let m: [f32; 20] = [
                    1.0, 0.0, 0.0, 0.0,
                    0.0, 1.0, 0.0, 0.0,
                    0.0, 0.0, 1.0, 0.0,
                    0.0, 0.0, 0.0, gain,
                    0.0, 0.0, 0.0, 0.0,
                ];
                let mb: Vec<u8> = m.iter().flat_map(|f| f.to_le_bytes()).collect();
                cm.SetValue(D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32, D2D1_PROPERTY_TYPE_MATRIX_5X4, &mb)?;
                cm.SetValue(D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT.0 as u32, D2D1_PROPERTY_TYPE_BOOL, &1u32.to_le_bytes())?;
                cm.GetOutput()
            }
        };
        let (glow, drop) = (shadow(2.5, 0.7)?, shadow(1.2, 2.0)?);
        self.offscreen(pw, ph, |dc| unsafe {
            dc.SetTransform(&id);
            dc.DrawImage(&glow, None, None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
            let off = Vector2 {
                X: scale.max(1.0).round(),
                Y: scale.max(1.0).round(),
            };
            dc.DrawImage(&drop, Some(&off), None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
            Ok(())
        })
    }

    pub fn text_format(&self, family: &str, size: f32, bold: bool) -> R<IDWriteTextFormat> {
        unsafe {
            let f = HSTRING::from(family);
            let tf = self.dwrite.CreateTextFormat(
                PCWSTR(f.as_ptr()),
                None,
                if bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL },
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!(""),
            )?;
            Ok(tf)
        }
    }

    /// A text format of a given weight (DWRITE_FONT_WEIGHT: 300 light … 700 bold).
    pub fn text_format_weight(&self, family: &str, size: f32, weight: i32) -> R<IDWriteTextFormat> {
        unsafe {
            let f = HSTRING::from(family);
            self.dwrite.CreateTextFormat(PCWSTR(f.as_ptr()), None, DWRITE_FONT_WEIGHT(weight), DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!(""))
        }
    }

    /// A centered, wrapped label: at most `max_lines` lines with an ellipsis (0 = unlimited).
    pub fn label(&self, tf: &IDWriteTextFormat, text: &str, width: f32, line_h: f32, max_lines: u32, center: bool) -> R<TextLayout> {
        let key = (tf.as_raw() as usize, text.to_string(), width.to_bits(), line_h.to_bits(), max_lines, center);
        if let Some(l) = self.labels.borrow().get(&key) {
            return Ok(l.clone());
        }
        let l = self.make_label(tf, text, width, line_h, max_lines, center)?;
        let mut c = self.labels.borrow_mut();
        if c.len() > 6000 {
            c.clear();
        }
        c.insert(key, l.clone());
        Ok(l)
    }

    /// Text formats are replaced (title font or size changed): their layouts go too.
    pub fn forget_labels(&self) {
        self.labels.borrow_mut().clear();
    }

    fn make_label(&self, tf: &IDWriteTextFormat, text: &str, width: f32, line_h: f32, max_lines: u32, center: bool) -> R<TextLayout> {
        unsafe {
            let s: Vec<u16> = text.encode_utf16().collect();
            let max_h = if max_lines == 0 { 10_000.0 } else { line_h * max_lines as f32 + 0.5 };
            let l = self.dwrite.CreateTextLayout(&s, tf, width, max_h)?;
            l.SetTextAlignment(if center { DWRITE_TEXT_ALIGNMENT_CENTER } else { DWRITE_TEXT_ALIGNMENT_LEADING })?;
            l.SetWordWrapping(if max_lines == 1 { DWRITE_WORD_WRAPPING_NO_WRAP } else { DWRITE_WORD_WRAPPING_EMERGENCY_BREAK })?;
            if max_lines != 0 {
                let sign = self.dwrite.CreateEllipsisTrimmingSign(&l)?;
                let t = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                l.SetTrimming(&t, &sign)?;
            }
            Ok(l)
        }
    }

    pub fn rounded_geometry(&self, r: RectF, radius: f32) -> R<ID2D1RoundedRectangleGeometry> {
        unsafe {
            self.factory.CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
                rect: r,
                radiusX: radius,
                radiusY: radius,
            })
        }
    }
}

/// Where Windows puts a wallpaper of `iw`×`ih` on a `mw`×`mh` monitor (DWPOS_*). Tile is drawn as
/// "center" in Phase 1 (UNVERIFIED for tiled wallpapers).
pub fn wallpaper_rect(position: i32, iw: f32, ih: f32, mw: f32, mh: f32) -> RectF {
    let (sw, sh) = (mw / iw, mh / ih);
    let fitted = |s: f32| {
        let (w, h) = (iw * s, ih * s);
        rect((mw - w) / 2.0, (mh - h) / 2.0, w, h)
    };
    match position {
        2 => rect(0.0, 0.0, mw, mh), // stretch
        3 => fitted(sw.min(sh)),     // fit
        0 | 1 => fitted(1.0),        // center (tile)
        _ => fitted(sw.max(sh)),     // fill, span
    }
}

pub struct Surface {
    _target: IDCompositionTarget,
    /// Another window showing the surface for now (boxes above windows).
    elsewhere: Option<IDCompositionTarget>,
    visual: IDCompositionVisual2,
    surface: Option<IDCompositionSurface>,
    pub w: u32,
    pub h: u32,
}

impl Surface {
    pub fn new(gpu: &Gpu, hwnd: HWND, w: u32, h: u32) -> R<Surface> {
        unsafe {
            let target = gpu.dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = gpu.dcomp.CreateVisual()?;
            target.SetRoot(&visual)?;
            let mut s = Surface {
                _target: target,
                elsewhere: None,
                visual,
                surface: None,
                w: 0,
                h: 0,
            };
            s.resize(gpu, w, h)?;
            Ok(s)
        }
    }

    pub fn resize(&mut self, gpu: &Gpu, w: u32, h: u32) -> R<()> {
        if w == self.w && h == self.h && self.surface.is_some() {
            return Ok(());
        }
        unsafe {
            let s = gpu.dcomp.CreateSurface(w.max(1), h.max(1), DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            self.visual.SetContent(&s)?;
            self.surface = Some(s);
        }
        self.w = w;
        self.h = h;
        Ok(())
    }

    /// Show the surface in another window (`Some`) instead of its own, or back in its own (`None`).
    pub fn retarget(&mut self, gpu: &Gpu, hwnd: Option<HWND>) -> R<()> {
        unsafe {
            match hwnd {
                Some(h) => {
                    let t = gpu.dcomp.CreateTargetForHwnd(h, true)?;
                    self._target.SetRoot(None::<&IDCompositionVisual>)?;
                    t.SetRoot(&self.visual)?;
                    self.elsewhere = Some(t);
                }
                None => {
                    if let Some(t) = self.elsewhere.take() {
                        t.SetRoot(None::<&IDCompositionVisual>)?;
                    }
                    self._target.SetRoot(&self.visual)?;
                }
            }
            gpu.commit()
        }
    }

    /// Redraw the whole surface (Phase 1: full redraws; dirty rects are a Phase 2 optimization).
    /// `scale` = DIP → pixel factor; callers draw in DIPs.
    pub fn draw(&self, scale: f32, f: impl FnOnce(&ID2D1DeviceContext) -> R<()>) -> R<()> {
        let Some(s) = &self.surface else { return Ok(()) };
        unsafe {
            let mut off = POINT::default();
            let dc: ID2D1DeviceContext = s.BeginDraw(None, &mut off)?;
            dc.SetTransform(&Matrix3x2 {
                M11: scale,
                M12: 0.0,
                M21: 0.0,
                M22: scale,
                M31: off.x as f32,
                M32: off.y as f32,
            });
            dc.Clear(Some(&Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let r = f(&dc);
            let e = s.EndDraw();
            r?;
            e
        }
    }
}

/// Fill a rounded rect with a region of `bmp` that maps the bitmap (scaled by `scale`) to (0,0) of
/// the surface: the Mica backdrop of a box.
pub fn fill_with_backdrop(dc: &ID2D1DeviceContext, bmp: &Bitmap, scale: f32, offset: (f32, f32), r: RectF, radius: f32) -> R<()> {
    unsafe {
        let props = D2D1_BITMAP_BRUSH_PROPERTIES1 {
            extendModeX: D2D1_EXTEND_MODE_CLAMP,
            extendModeY: D2D1_EXTEND_MODE_CLAMP,
            interpolationMode: D2D1_INTERPOLATION_MODE_LINEAR,
        };
        let br = dc.CreateBitmapBrush(bmp, Some(&props), None)?;
        br.SetTransform(&Matrix3x2 {
            M11: scale,
            M12: 0.0,
            M21: 0.0,
            M22: scale,
            M31: offset.0,
            M32: offset.1,
        });
        dc.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: r,
                radiusX: radius,
                radiusY: radius,
            },
            &br,
        );
        Ok(())
    }
}

/// An icon as frosted glass: the blurred wallpaper (`backdrop`, placed as in
/// `fill_with_backdrop`) seen through the icon's shape, lit by `light`, at `alpha`.
pub fn draw_frosted(dc: &ID2D1DeviceContext, icon: &Bitmap, dst: RectF, backdrop: &Bitmap, scale: f32, offset: (f32, f32), light: Color, alpha: f32) -> R<()> {
    unsafe {
        let props = D2D1_BITMAP_BRUSH_PROPERTIES1 {
            extendModeX: D2D1_EXTEND_MODE_CLAMP,
            extendModeY: D2D1_EXTEND_MODE_CLAMP,
            interpolationMode: D2D1_INTERPOLATION_MODE_LINEAR,
        };
        let br = dc.CreateBitmapBrush(backdrop, Some(&props), None)?;
        br.SetTransform(&Matrix3x2 {
            M11: scale,
            M12: 0.0,
            M21: 0.0,
            M22: scale,
            M31: offset.0,
            M32: offset.1,
        });
        br.SetOpacity(alpha.clamp(0.0, 1.0));
        let lb = brush(dc, Color { a: light.a * alpha.clamp(0.0, 1.0), ..light })?;
        let aa = dc.GetAntialiasMode();
        dc.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
        dc.FillOpacityMask(icon, &br, Some(&dst), None);
        dc.FillOpacityMask(icon, &lb, Some(&dst), None);
        dc.SetAntialiasMode(aa);
        Ok(())
    }
}

pub fn brush(dc: &ID2D1DeviceContext, c: Color) -> R<ID2D1SolidColorBrush> {
    unsafe { dc.CreateSolidColorBrush(&c, None) }
}

pub fn fill_round(dc: &ID2D1DeviceContext, r: RectF, radius: f32, c: Color) -> R<()> {
    unsafe {
        let b = brush(dc, c)?;
        dc.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: r,
                radiusX: radius,
                radiusY: radius,
            },
            &b,
        );
        Ok(())
    }
}

pub fn stroke_round(dc: &ID2D1DeviceContext, r: RectF, radius: f32, c: Color, width: f32) -> R<()> {
    unsafe {
        let b = brush(dc, c)?;
        dc.DrawRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: r,
                radiusX: radius,
                radiusY: radius,
            },
            &b,
            width,
            None,
        );
        Ok(())
    }
}

pub fn fill_rect(dc: &ID2D1DeviceContext, r: RectF, c: Color) -> R<()> {
    unsafe {
        let b = brush(dc, c)?;
        dc.FillRectangle(&r, &b);
        Ok(())
    }
}

pub fn draw_bitmap(dc: &ID2D1DeviceContext, b: &Bitmap, dst: RectF, opacity: f32) {
    unsafe { dc.DrawBitmap(b, Some(&dst), opacity, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None) }
}

thread_local! {
    /// The color-matrix effect for tinted icons, made once per device context.
    static TINT_FX: std::cell::RefCell<Option<(usize, ID2D1Effect)>> = const { std::cell::RefCell::new(None) };
}

/// Draw an icon tinted toward `tint`: by `strength` (0 = as it is, 1 = only the tint, shaded by
/// the icon's own lightness), at `opacity`.
pub fn draw_bitmap_tinted(dc: &ID2D1DeviceContext, b: &Bitmap, dst: RectF, opacity: f32, tint: Color, strength: f32) -> R<()> {
    unsafe {
        let key = dc.as_raw() as usize;
        let fx = TINT_FX.with(|c| -> R<ID2D1Effect> {
            let mut c = c.borrow_mut();
            if let Some((k, fx)) = c.as_ref() {
                if *k == key {
                    return Ok(fx.clone());
                }
            }
            let fx = dc.CreateEffect(&CLSID_D2D1ColorMatrix)?;
            fx.SetValue(D2D1_COLORMATRIX_PROP_CLAMP_OUTPUT.0 as u32, D2D1_PROPERTY_TYPE_BOOL, &1u32.to_le_bytes())?;
            *c = Some((key, fx.clone()));
            Ok(fx)
        })?;
        fx.SetInput(0, b, true);
        let s = strength.clamp(0.0, 1.0);
        // Lightness of the icon, brightened a little so a tinted white page stays light.
        let lum = [0.299f32 * 1.35, 0.587 * 1.35, 0.114 * 1.35];
        let tc = [tint.r, tint.g, tint.b];
        let mut m = [0f32; 20];
        for i in 0..3 {
            for j in 0..3 {
                m[i * 4 + j] = if i == j { 1.0 - s } else { 0.0 } + s * lum[i] * tc[j];
            }
        }
        m[15] = opacity.clamp(0.0, 1.0);
        let mb: Vec<u8> = m.iter().flat_map(|f| f.to_le_bytes()).collect();
        fx.SetValue(D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32, D2D1_PROPERTY_TYPE_MATRIX_5X4, &mb)?;
        let size = b.GetSize();
        if size.width <= 0.0 || size.height <= 0.0 {
            return Ok(());
        }
        let mut old = Matrix3x2::identity();
        dc.GetTransform(&mut old);
        let m = Matrix3x2 {
            M11: (dst.right - dst.left) / size.width,
            M12: 0.0,
            M21: 0.0,
            M22: (dst.bottom - dst.top) / size.height,
            M31: dst.left,
            M32: dst.top,
        };
        dc.SetTransform(&(m * old));
        dc.DrawImage(&fx.GetOutput()?, None, None, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_COMPOSITE_MODE_SOURCE_OVER);
        dc.SetTransform(&old);
        Ok(())
    }
}

pub fn draw_text(dc: &ID2D1DeviceContext, l: &TextLayout, x: f32, y: f32, c: Color) -> R<()> {
    unsafe {
        let b = brush(dc, c)?;
        dc.DrawTextLayout(Vector2 { X: x, Y: y }, l, &b, D2D1_DRAW_TEXT_OPTIONS_NONE);
        Ok(())
    }
}

/// Text with ClearType (colored subpixel edges), as Explorer draws desktop labels. Only over
/// opaque pixels (the wallpaper or a box we drew): ClearType needs to know the background.
pub fn draw_text_cleartype(dc: &ID2D1DeviceContext, l: &TextLayout, x: f32, y: f32, c: Color) -> R<()> {
    unsafe {
        let old = dc.GetTextAntialiasMode();
        dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
        let r = draw_text(dc, l, x, y, c);
        dc.SetTextAntialiasMode(old);
        r
    }
}

/// Label with Explorer-like legibility on the bare wallpaper: a soft dark shadow, then the text.
pub fn draw_text_shadowed(dc: &ID2D1DeviceContext, l: &TextLayout, x: f32, y: f32, c: Color) -> R<()> {
    draw_text(dc, l, x + 1.0, y + 1.0, Color { r: 0.0, g: 0.0, b: 0.0, a: 0.75 })?;
    draw_text(dc, l, x, y + 0.5, Color { r: 0.0, g: 0.0, b: 0.0, a: 0.35 })?;
    draw_text(dc, l, x, y, c)
}

pub fn text_height(l: &TextLayout) -> f32 {
    unsafe {
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = l.GetMetrics(&mut m);
        m.height
    }
}

/// Width of the text including trailing spaces (for a one-line text field).
pub fn text_width(l: &TextLayout) -> f32 {
    unsafe {
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = l.GetMetrics(&mut m);
        m.widthIncludingTrailingWhitespace
    }
}

/// X of the caret before UTF-16 position `pos`, relative to the layout's origin.
pub fn caret_x(l: &TextLayout, pos: u32) -> f32 {
    unsafe {
        let (mut x, mut y) = (0.0f32, 0.0f32);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        let _ = l.HitTestTextPosition(pos, false, &mut x, &mut y, &mut m);
        x
    }
}

/// UTF-16 caret position nearest to `x` (relative to the layout's origin).
pub fn caret_at(l: &TextLayout, x: f32) -> u32 {
    unsafe {
        let (mut trailing, mut inside) = (windows::core::BOOL(0), windows::core::BOOL(0));
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        if l.HitTestPoint(x, 1.0, &mut trailing, &mut inside, &mut m).is_err() {
            return 0;
        }
        m.textPosition + if trailing.as_bool() { m.length } else { 0 }
    }
}

/// A rounded rectangle's outline in a stroke style (dashes).
pub fn stroke_round_styled(dc: &ID2D1DeviceContext, r: RectF, radius: f32, c: Color, width: f32, style: &ID2D1StrokeStyle) -> R<()> {
    unsafe {
        let b = brush(dc, c)?;
        dc.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }, &b, width, style);
        Ok(())
    }
}

/// Draw `f` scaled by `s` around (`cx`, `cy`).
pub fn scaled<T>(dc: &ID2D1DeviceContext, cx: f32, cy: f32, s: f32, f: impl FnOnce() -> R<T>) -> R<T> {
    unsafe {
        let mut old = Matrix3x2::identity();
        dc.GetTransform(&mut old);
        let m = Matrix3x2 {
            M11: s,
            M12: 0.0,
            M21: 0.0,
            M22: s,
            M31: cx * (1.0 - s),
            M32: cy * (1.0 - s),
        };
        dc.SetTransform(&(m * old));
        let r = f();
        dc.SetTransform(&old);
        r
    }
}

/// Draw what follows at `opacity` (within `bounds`) until `pop_layer`.
pub fn push_layer(dc: &ID2D1DeviceContext, bounds: RectF, opacity: f32) -> R<()> {
    unsafe {
        let p = D2D1_LAYER_PARAMETERS1 {
            contentBounds: bounds,
            geometricMask: std::mem::ManuallyDrop::new(None),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: opacity.clamp(0.0, 1.0),
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS1_NONE,
        };
        dc.PushLayer(&p, None);
        Ok(())
    }
}

pub fn pop_layer(dc: &ID2D1DeviceContext) {
    unsafe { dc.PopLayer() }
}

pub fn push_clip(dc: &ID2D1DeviceContext, r: RectF) {
    unsafe { dc.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) }
}

pub fn pop_clip(dc: &ID2D1DeviceContext) {
    unsafe { dc.PopAxisAlignedClip() }
}
