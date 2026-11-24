//! Icons from the shell (docs/ARCHITECTURE.md §6): `IShellItemImageFactory::GetImage(ICONONLY)` for
//! the image, the system image list index to share one image between items of the same type, and
//! the system image list overlay (shortcut arrow) drawn on top.
//! Files with a thumbnail handler show their thumbnail, like Explorer (`thumbnail`).

use crate::{pcw, wide};
use deskboxes_core::snapshot::IconImage;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// System image list index and overlay index (1-based, 0 = none: the shortcut arrow is 2) for
/// an item. The overlay is only filled in together with SHGFI_ICON (verified on 26300: without
/// it every shortcut reported none), so a small icon is made and thrown away.
pub fn sys_index(parsing: &str) -> (i32, u32) {
    unsafe {
        let p = wide(parsing);
        let mut sfi = SHFILEINFOW::default();
        let r = SHGetFileInfoW(
            pcw(&p),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut sfi),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_OVERLAYINDEX | SHGFI_ICON | SHGFI_SMALLICON,
        );
        if !sfi.hIcon.is_invalid() {
            let _ = DestroyIcon(sfi.hIcon);
        }
        if r == 0 {
            return (-1, 0);
        }
        (sfi.iIcon & 0x00ff_ffff, (sfi.iIcon as u32) >> 24)
    }
}

/// Whether Explorer shows the UAC shield on the item (a program, or a shortcut to one, that asks
/// for administrator rights): the item's own icon extractor says so for GIL_CHECKSHIELD.
pub fn needs_shield(parsing: &str) -> bool {
    unsafe {
        let p = wide(parsing);
        let Ok(si) = SHCreateItemFromParsingName::<_, _, IShellItem>(pcw(&p), None) else { return false };
        let Ok(ei) = si.BindToHandler::<_, IExtractIconW>(None, &BHID_SFUIObject) else { return false };
        let mut buf = [0u16; 260];
        let (mut ix, mut flags) = (0i32, 0u32);
        ei.GetIconLocation(GIL_FORSHELL | GIL_CHECKSHIELD, &mut buf, &mut ix, &mut flags).is_ok() && flags & GIL_SHIELD != 0
    }
}

/// Windows' shield at `px` × `px` (drawn on the icons of programs that run as administrator).
pub fn shield_image(px: u32) -> Option<IconImage> {
    unsafe {
        let mut si = SHSTOCKICONINFO {
            cbSize: std::mem::size_of::<SHSTOCKICONINFO>() as u32,
            ..Default::default()
        };
        SHGetStockIconInfo(SIID_SHIELD, SHGSI_ICONLOCATION, &mut si).ok()?;
        let mut hi = HICON::default();
        let px16 = px.clamp(8, 256);
        SHDefExtractIconW(windows::core::PCWSTR(si.szPath.as_ptr()), si.iIcon, 0, Some(&mut hi), None, px16 | (px16 << 16)).ok().ok()?;
        let img = icon_pixels(hi);
        let _ = DestroyIcon(hi);
        img
    }
}

/// The item's picture as Explorer shows it on the desktop with no "icon only" restriction: a
/// folder's icon with a preview of what is inside.
pub fn folder_image(parsing: &str, px: u32) -> Option<IconImage> {
    unsafe {
        let p = wide(parsing);
        let f: IShellItemImageFactory = SHCreateItemFromParsingName(pcw(&p), None).ok()?;
        let hb = f.GetImage(SIZE { cx: px as i32, cy: px as i32 }, SIIGBF_BIGGERSIZEOK).ok()?;
        let raw = bitmap_raw(hb);
        let _ = DeleteObject(hb.into());
        let (img, had_alpha) = raw?;
        // A folder picture always has see-through corners. Windows sometimes hands one back
        // flattened (no alpha, or on black), e.g. while its thumbnail cache is still being built at
        // sign-in; drawn as is, the folder sits in a black box. The caller falls back to the plain
        // folder icon instead.
        if !had_alpha || !see_through_corners(&img) || forced_flat() {
            log::info!("folder picture without transparency for {parsing}; using the icon for now");
            FLAT.lock().unwrap_or_else(|p| p.into_inner()).push(parsing.to_string());
            return None;
        }
        Some(img)
    }
}

/// Folders whose picture came back flattened since `take_flat` was last called: worth asking for
/// again once Windows has its thumbnail ready.
static FLAT: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The folders `folder_image` turned down since the last call.
pub fn take_flat() -> Vec<String> {
    std::mem::take(&mut *FLAT.lock().unwrap_or_else(|p| p.into_inner()))
}

/// Tests (debug builds only): `DBX_TEST_FLAT_FOLDERS=n` takes the first n folder pictures for
/// flattened ones.
fn forced_flat() -> bool {
    if !cfg!(debug_assertions) {
        return false;
    }
    use std::sync::atomic::{AtomicI64, Ordering};
    static LEFT: AtomicI64 = AtomicI64::new(i64::MIN);
    if LEFT.load(Ordering::Relaxed) == i64::MIN {
        let n = std::env::var("DBX_TEST_FLAT_FOLDERS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        let _ = LEFT.compare_exchange(i64::MIN, n, Ordering::Relaxed, Ordering::Relaxed);
    }
    LEFT.fetch_sub(1, Ordering::Relaxed) > 0
}

/// Whether at least three of the four corner pixels are mostly transparent.
fn see_through_corners(img: &IconImage) -> bool {
    if img.w < 4 || img.h < 4 {
        return true;
    }
    let a = |x: u32, y: u32| img.bgra[((y * img.w + x) * 4 + 3) as usize];
    let (r, b) = (img.w - 1, img.h - 1);
    [a(0, 0), a(r, 0), a(0, b), a(r, b)].iter().filter(|&&v| v < 128).count() >= 3
}

/// Explorer's tooltip text for an item (type, size, date modified, a shortcut's description...).
pub fn info_tip(parsing: &str) -> Option<String> {
    unsafe {
        let p = wide(parsing);
        let si: IShellItem = SHCreateItemFromParsingName(pcw(&p), None).ok()?;
        let qi: IQueryInfo = si.BindToHandler(None, &BHID_SFUIObject).ok()?;
        let tip = qi.GetInfoTip(QITIPF_DEFAULT).ok()?;
        let s = tip.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(tip.0 as _));
        s.map(|s| s.replace("\r\n", "\n")).filter(|s| !s.trim().is_empty())
    }
}

/// 32-bpp pixels of an HBITMAP, top-down, premultiplied (opaque when it has no alpha).
unsafe fn bitmap_pixels(hb: HBITMAP) -> Option<IconImage> {
    bitmap_raw(hb).map(|(img, _)| img)
}

/// Same, and whether the bitmap had an alpha channel of its own.
unsafe fn bitmap_raw(hb: HBITMAP) -> Option<(IconImage, bool)> {
    let mut bm = BITMAP::default();
    if GetObjectW(hb.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as _)) == 0 {
        return None;
    }
    let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
        ..Default::default()
    };
    let mut px = vec![0u8; (w * h * 4) as usize];
    let dc = GetDC(None);
    let n = GetDIBits(dc, hb, 0, h as u32, Some(px.as_mut_ptr() as _), &mut bi, DIB_RGB_COLORS);
    ReleaseDC(None, dc);
    if n == 0 {
        return None;
    }
    let has_alpha = px.chunks_exact(4).any(|p| p[3] != 0);
    if !has_alpha {
        px.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    } else if px.chunks_exact(4).any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]) {
        // straight alpha → premultiply
        for p in px.chunks_exact_mut(4) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
    Some((IconImage { w: w as u32, h: h as u32, bgra: px }, has_alpha))
}

/// The item's icon at `px` × `px` (no thumbnail, no overlay).
pub fn item_icon(parsing: &str, px: u32) -> Option<IconImage> {
    unsafe {
        let p = wide(parsing);
        let f: IShellItemImageFactory = SHCreateItemFromParsingName(pcw(&p), None).ok()?;
        let hb = f.GetImage(SIZE { cx: px as i32, cy: px as i32 }, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK).ok()?;
        let img = bitmap_pixels(hb);
        let _ = DeleteObject(hb.into());
        img
    }
}

/// The file's thumbnail fitted into `px` × `px` (aspect kept), if Windows has one for it.
pub fn thumbnail(parsing: &str, px: u32) -> Option<IconImage> {
    unsafe {
        let p = wide(parsing);
        let f: IShellItemImageFactory = SHCreateItemFromParsingName(pcw(&p), None).ok()?;
        let hb = f.GetImage(SIZE { cx: px as i32, cy: px as i32 }, SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK).ok()?; // may be larger: the caller fits it
        let img = bitmap_pixels(hb);
        let _ = DeleteObject(hb.into());
        img
    }
}

/// Whether files with this extension (".png") have a thumbnail handler registered.
pub fn has_thumbnail_handler(ext: &str) -> bool {
    unsafe {
        let e = wide(ext);
        let iid = wide("{e357fccd-a995-4576-b01f-234630154e96}"); // IThumbnailProvider
        let mut n = 0u32;
        AssocQueryStringW(ASSOCF_INIT_DEFAULTTOSTAR, ASSOCSTR_SHELLEXTENSION, pcw(&e), pcw(&iid), None, &mut n).is_ok() && n > 1
    }
}

/// Pixels of an HICON (color bitmap with alpha; mask applied when it has no alpha, as for old
/// icons, which would otherwise sit in a black square).
unsafe fn icon_pixels(hi: HICON) -> Option<IconImage> {
    let mut ii = ICONINFO::default();
    GetIconInfo(hi, &mut ii).ok()?;
    let raw = bitmap_raw(ii.hbmColor);
    let mask = if matches!(raw, Some((_, false))) { bitmap_raw(ii.hbmMask) } else { None };
    let _ = DeleteObject(ii.hbmColor.into());
    let _ = DeleteObject(ii.hbmMask.into());
    let (mut img, _) = raw?;
    if let Some((m, _)) = mask {
        if m.w == img.w && m.h >= img.h {
            apply_mask(&mut img, &m);
        }
    }
    Some(img)
}

/// Clears the pixels an icon's AND mask marks as transparent (mask set = see-through).
fn apply_mask(img: &mut IconImage, mask: &IconImage) {
    for (p, m) in img.bgra.chunks_exact_mut(4).zip(mask.bgra.chunks_exact(4)) {
        if m[0] > 127 {
            p.copy_from_slice(&[0, 0, 0, 0]);
        }
    }
}

/// The system overlay image (1-based overlay index, e.g. the shortcut arrow) from the image list
/// of the icon size, as Explorer takes it (the arrow is drawn smaller relative to a 48 px icon
/// than to a 256 px one); the caller scales it to the icon size if they differ.
pub fn overlay_icon(overlay: u32, px: u32) -> Option<IconImage> {
    if overlay == 0 {
        return None;
    }
    // The smallest list at least `px` big (the caller scales it down to `px`).
    let list = match px {
        0..=16 => SHIL_SMALL,
        17..=32 => SHIL_LARGE,
        33..=48 => SHIL_EXTRALARGE,
        _ => SHIL_JUMBO,
    };
    unsafe {
        let il: IImageList = SHGetImageList(list as i32).ok()?;
        let ix = il.GetOverlayImage(overlay as i32).ok()?;
        let hi = il.GetIcon(ix, ILD_TRANSPARENT.0).ok()?;
        let img = icon_pixels(hi);
        let _ = DestroyIcon(hi);
        img
    }
}
