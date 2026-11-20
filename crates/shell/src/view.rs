//! Explorer's desktop view as a source of truth (read-only, public COM API, no injection):
//! icon spacing and, on first run, where the user had placed each icon. Plus drop targets and
//! paste, which go to Windows' own folder drop targets.

use crate::{hide, pcw, wide};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::{MODIFIERKEYS_FLAGS, SFGAO_DROPTARGET};
use windows::Win32::UI::Shell::Common::*;
use windows::Win32::UI::Shell::*;

#[derive(Clone, Debug, Default)]
pub struct NativeView {
    /// Icon spacing (cell size) in physical px.
    pub spacing: (i32, i32),
    /// (parsing name, x, y): item positions in the desktop list view's client coordinates.
    pub positions: Vec<(String, i32, i32)>,
}

pub fn native_view(with_positions: bool) -> Option<NativeView> {
    unsafe {
        let fv = hide::desktop_folder_view().ok()?;
        let mut sp = POINT::default();
        fv.GetSpacing(&mut sp).ok()?;
        let mut nv = NativeView {
            spacing: (sp.x, sp.y),
            positions: vec![],
        };
        if with_positions {
            let desk = SHGetDesktopFolder().ok()?;
            let n = fv.ItemCount(SVGIO_ALLVIEW).unwrap_or(0);
            for i in 0..n {
                let Ok(pidl) = fv.Item(i) else { continue };
                let pos = fv.GetItemPosition(pidl);
                let mut sr = STRRET::default();
                let mut name = String::new();
                if desk.GetDisplayNameOf(pidl, SHGDN_FORPARSING, &mut sr).is_ok() {
                    let mut p = windows::core::PWSTR::null();
                    if StrRetToStrW(&mut sr, Some(pidl), &mut p).is_ok() {
                        name = p.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(p.0 as _));
                    }
                }
                CoTaskMemFree(Some(pidl as _));
                if let (Ok(pt), false) = (pos, name.is_empty()) {
                    nv.positions.push((name, pt.x, pt.y));
                }
            }
        }
        Some(nv)
    }
}

/// Explorer's desktop icon size (View > Large / Medium / Small icons), in DIPs (48 / 32 / 16).
pub fn set_desktop_icon_size(dip: u32) -> windows::core::Result<()> {
    unsafe {
        let fv = hide::desktop_folder_view()?;
        fv.SetViewModeAndIconSize(FVM_ICON, dip as i32)
    }
}

/// The drop target of a folder's background (`None` = the desktop): what Explorer drops into.
pub fn background_drop_target(folder: Option<&str>, hwnd: isize) -> windows::core::Result<IDropTarget> {
    unsafe {
        let sf: IShellFolder = match folder {
            None => SHGetDesktopFolder()?,
            Some(f) => {
                let h = wide(f);
                let si: IShellItem = SHCreateItemFromParsingName(pcw(&h), None)?;
                si.BindToHandler(None, &BHID_SFObject)?
            }
        };
        sf.CreateViewObject(HWND(hwnd as _))
    }
}

/// The drop target of an item (folder, program, zip...): `SFGAO_DROPTARGET` items only.
pub fn item_drop_target(parsing: &str) -> Option<IDropTarget> {
    unsafe {
        let h = wide(parsing);
        let si: IShellItem = SHCreateItemFromParsingName(pcw(&h), None).ok()?;
        let a = si.GetAttributes(SFGAO_DROPTARGET).ok()?;
        if a.0 & SFGAO_DROPTARGET.0 == 0 {
            return None;
        }
        si.BindToHandler(None, &BHID_SFUIObject).ok()
    }
}

/// Paste the clipboard into a folder the way Explorer does: drop the clipboard's data object on
/// the folder's drop target with the effect the source asked for (`Preferred DropEffect`), or a
/// link for "Paste shortcut". Needs an OLE-initialized STA thread.
pub fn paste(folder: Option<&str>, hwnd: isize, link: bool, pt: (i32, i32)) -> windows::core::Result<()> {
    unsafe {
        let obj = OleGetClipboard()?;
        let target = background_drop_target(folder, hwnd)?;
        let mut eff = if link { DROPEFFECT_LINK } else { preferred_effect(&obj).unwrap_or(DROPEFFECT_COPY) };
        let p = POINTL { x: pt.0, y: pt.1 };
        let keys = MODIFIERKEYS_FLAGS(0);
        let want = eff;
        target.DragEnter(&obj, keys, p, &mut eff)?;
        eff = want;
        target.DragOver(keys, p, &mut eff)?;
        eff = want;
        target.Drop(&obj, keys, p, &mut eff)?;
        Ok(())
    }
}

fn preferred_effect(obj: &IDataObject) -> Option<DROPEFFECT> {
    unsafe {
        let cf = windows::Win32::System::DataExchange::RegisterClipboardFormatW(windows::core::w!("Preferred DropEffect"));
        let fmt = FORMATETC {
            cfFormat: cf as u16,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let mut m = obj.GetData(&fmt).ok()?;
        let hg = m.u.hGlobal;
        let p = windows::Win32::System::Memory::GlobalLock(hg) as *const u32;
        let v = if p.is_null() { None } else { Some(*p) };
        let _ = windows::Win32::System::Memory::GlobalUnlock(hg);
        ReleaseStgMedium(&mut m);
        v.map(|v| if v & DROPEFFECT_MOVE.0 != 0 { DROPEFFECT_MOVE } else { DROPEFFECT_COPY })
    }
}

/// Put items on the clipboard as Explorer does (shell data object + `Preferred DropEffect`).
pub fn set_clipboard(paths: &[String], cut: bool) -> windows::core::Result<()> {
    unsafe {
        let obj = crate::fileops::data_object(paths)?;
        set_preferred_effect(&obj, if cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY | DROPEFFECT_LINK })?;
        OleSetClipboard(&obj)?;
        OleFlushClipboard()
    }
}

fn set_preferred_effect(obj: &IDataObject, eff: DROPEFFECT) -> windows::core::Result<()> {
    unsafe {
        use windows::Win32::System::Memory::*;
        let cf = windows::Win32::System::DataExchange::RegisterClipboardFormatW(windows::core::w!("Preferred DropEffect"));
        let hg = GlobalAlloc(GMEM_MOVEABLE, 4)?;
        let p = GlobalLock(hg) as *mut u32;
        *p = eff.0;
        let _ = GlobalUnlock(hg);
        let fmt = FORMATETC {
            cfFormat: cf as u16,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let m = STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: hg },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        };
        obj.SetData(&fmt, &m, true)
    }
}

/// Drop items onto another item (a folder, a program): what dragging onto its icon does.
pub fn drop_on_item(target: &str, paths: &[String], copy: bool) -> windows::core::Result<()> {
    unsafe {
        let t = item_drop_target(target).ok_or_else(|| windows::core::Error::from(E_INVALIDARG))?;
        let obj = crate::fileops::data_object(paths)?;
        let want = if copy { DROPEFFECT_COPY } else { DROPEFFECT_MOVE };
        let keys = MODIFIERKEYS_FLAGS(if copy { 0x0008 } else { 0x0004 }); // MK_CONTROL / MK_SHIFT
        let p = POINTL::default();
        let mut eff = want;
        t.DragEnter(&obj, keys, p, &mut eff)?;
        if eff == DROPEFFECT_NONE {
            t.DragLeave()?;
            return Err(windows::core::Error::from(E_ABORT));
        }
        eff = want;
        t.DragOver(keys, p, &mut eff)?;
        eff = want;
        t.Drop(&obj, keys, p, &mut eff)
    }
}

/// Plain text on the clipboard (CF_UNICODETEXT), for the box title field.
pub fn clipboard_text(owner: isize) -> Option<String> {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    unsafe {
        OpenClipboard(Some(HWND(owner as *mut _))).ok()?;
        let r = (|| {
            let h = GetClipboardData(13).ok()?; // CF_UNICODETEXT
            let hg = HGLOBAL(h.0);
            let p = GlobalLock(hg) as *const u16;
            if p.is_null() {
                return None;
            }
            let n = (GlobalSize(hg) / 2).max(1);
            let s = std::slice::from_raw_parts(p, n);
            let end = s.iter().position(|c| *c == 0).unwrap_or(n);
            let t = String::from_utf16_lossy(&s[..end]);
            let _ = GlobalUnlock(hg);
            Some(t)
        })();
        let _ = CloseClipboard();
        r
    }
}

/// Put plain text on the clipboard.
pub fn set_clipboard_text(owner: isize, text: &str) -> windows::core::Result<()> {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    unsafe {
        let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        OpenClipboard(Some(HWND(owner as *mut _)))?;
        let r = (|| {
            EmptyClipboard()?;
            let hg = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2)?;
            let p = GlobalLock(hg) as *mut u16;
            std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
            let _ = GlobalUnlock(hg);
            SetClipboardData(13, Some(HANDLE(hg.0)))?;
            Ok(())
        })();
        let _ = CloseClipboard();
        r
    }
}
