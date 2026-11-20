//! Drops from other windows onto our surface (docs/ARCHITECTURE.md §10). Every drop is forwarded
//! to Windows' own drop target: the item under the cursor if it accepts drops (folder, program,
//! zip), otherwise the desktop folder (or the fixture folder). We only remember where it happened
//! so the new items appear there. Runs on the UI thread (OLE's rule, the documented exception).

use crate::app::{with, Hit};
use std::cell::RefCell;
use windows::core::{implement, Ref};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::*;

/// Where a drag over our surface would land.
enum Target {
    /// Windows' own drop target: an item (folder, program, zip) or the folder background.
    Shell(String, IDropTarget),
    /// Our own icons dragged out and back again: a layout move, never a file operation (the
    /// desktop folder's drop target would try to move the files onto themselves).
    Own,
}

impl Target {
    fn key(&self) -> &str {
        match self {
            Target::Shell(k, _) => k,
            Target::Own => "own",
        }
    }
}

#[implement(IDropTarget)]
pub struct DropTarget {
    hwnd: HWND,
    cur: RefCell<Option<Target>>,
    data: RefCell<Option<IDataObject>>,
    helper: Option<IDropTargetHelper>,
    /// Our own icons are over us: the drag shows no caption (set once per visit).
    own_caption_cleared: std::cell::Cell<bool>,
}

impl DropTarget {
    pub fn create(hwnd: HWND) -> IDropTarget {
        let helper = unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok() };
        DropTarget {
            hwnd,
            cur: RefCell::new(None),
            data: RefCell::new(None),
            helper,
            own_caption_cleared: std::cell::Cell::new(false),
        }
        .into()
    }
}

/// The target at a point: an item's own drop target (not one of the dragged items), else, for our
/// own drag, a layout move, else the folder background's drop target.
fn target_at(hwnd: HWND, pt: &POINTL) -> Option<Target> {
    with(|app| {
        let p = app.screen_to_dip(pt.x, pt.y);
        let own = app.own_drag.as_ref().map(|(keys, _)| keys.clone());
        if let Hit::Item { key, .. } = app.hit_test(p) {
            let dragged = own.as_ref().map(|k| k.contains(&key)).unwrap_or(false);
            if !dragged {
                if let Some(it) = app.items.get(&key) {
                    if let Some(t) = deskboxes_shell::view::item_drop_target(&it.parsing_name) {
                        return Some(Target::Shell(key.0.clone(), t));
                    }
                }
            }
        }
        if let Some(keys) = &own {
            // A folder box's files go only into a folder (another folder box, a folder icon).
            if keys.iter().any(|k| app.is_folder_item(k)) && app.drop_box(p).filter(|id| app.is_folder_box(*id)).is_none() {
                return None;
            }
            return Some(Target::Own);
        }
        // Over a folder box: its folder takes the drop.
        if let Some(f) = app.folder_box_at(p) {
            return deskboxes_shell::view::background_drop_target(Some(&f), hwnd.0 as isize)
                .map_err(|e| log::warn!("folder box drop target: {e}"))
                .ok()
                .map(|t| Target::Shell(format!("fb:{f}"), t));
        }
        let folder = app.args.desktop_dir.as_ref().map(|d| d.display().to_string());
        deskboxes_shell::view::background_drop_target(folder.as_deref(), hwnd.0 as isize)
            .map_err(|e| log::warn!("background drop target: {e}"))
            .ok()
            .map(|t| Target::Shell("bg".to_string(), t))
    })
    .flatten()
}

/// The drag image's caption ("Move to …"): another window's drop target sets it while the drag
/// is over that window, and it stayed when our own icons came back over the desktop. Over us, a
/// layout move shows none, as Explorer's desktop does.
unsafe fn clear_caption(data: &IDataObject) {
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    use windows::Win32::System::Memory::*;
    let cf = RegisterClipboardFormatW(windows::core::w!("DropDescription"));
    if cf == 0 {
        return;
    }
    let fmt = FORMATETC { cfFormat: cf as u16, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
    let Ok(hg) = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, std::mem::size_of::<DROPDESCRIPTION>()) else { return };
    let p = GlobalLock(hg) as *mut DROPDESCRIPTION;
    if p.is_null() {
        let _ = GlobalFree(Some(hg));
        return;
    }
    (*p).r#type = DROPIMAGE_NOIMAGE;
    let _ = GlobalUnlock(hg);
    let med = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: hg }, pUnkForRelease: std::mem::ManuallyDrop::new(None) };
    // fRelease = true: the data object owns the memory now.
    if data.SetData(&fmt, &med, true).is_err() {
        let _ = GlobalFree(Some(hg));
    }
}

impl DropTarget_Impl {
    /// Over us with our own icons: no caption (once per visit).
    fn own_over(&self) {
        if !self.own_caption_cleared.get() {
            if let Some(d) = self.data.borrow().as_ref() {
                unsafe { clear_caption(d) };
            }
            self.own_caption_cleared.set(true);
        }
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, pdataobj: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, eff: *mut DROPEFFECT) -> windows::core::Result<()> {
        let data = pdataobj.ok()?.clone();
        *self.data.borrow_mut() = Some(data.clone());
        let allowed = unsafe { *eff };
        let t = target_at(self.hwnd, pt);
        self.own_caption_cleared.set(false);
        unsafe {
            match &t {
                Some(Target::Shell(_, t)) => t.DragEnter(&data, keys, *pt, eff)?,
                Some(Target::Own) => {
                    *eff = allowed & DROPEFFECT_MOVE;
                    self.own_over();
                }
                None => *eff = DROPEFFECT_NONE,
            }
            if let Some(h) = &self.helper {
                let _ = h.DragEnter(self.hwnd, &data, &POINT { x: pt.x, y: pt.y }, *eff);
            }
        }
        let _ = allowed;
        *self.cur.borrow_mut() = t;
        Ok(())
    }

    fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, eff: *mut DROPEFFECT) -> windows::core::Result<()> {
        let allowed = unsafe { *eff };
        let key_now = target_at(self.hwnd, pt);
        let changed = self.cur.borrow().as_ref().map(|c| c.key().to_string()) != key_now.as_ref().map(|k| k.key().to_string());
        unsafe {
            if changed {
                if let Some(Target::Shell(_, old)) = self.cur.borrow_mut().take() {
                    let _ = old.DragLeave();
                }
                if let (Some(Target::Shell(_, t)), Some(d)) = (&key_now, self.data.borrow().as_ref()) {
                    let mut e = allowed;
                    let _ = t.DragEnter(d, keys, *pt, &mut e);
                }
                *self.cur.borrow_mut() = key_now;
                self.own_caption_cleared.set(false);
            }
            match self.cur.borrow().as_ref() {
                Some(Target::Shell(_, t)) => {
                    *eff = allowed;
                    t.DragOver(keys, *pt, eff)?;
                }
                Some(Target::Own) => {
                    *eff = allowed & DROPEFFECT_MOVE;
                    self.own_over();
                }
                None => *eff = DROPEFFECT_NONE,
            }
            if let Some(h) = &self.helper {
                let _ = h.DragOver(&POINT { x: pt.x, y: pt.y }, *eff);
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        unsafe {
            if let Some(Target::Shell(_, t)) = self.cur.borrow_mut().take() {
                let _ = t.DragLeave();
            }
            if let Some(h) = &self.helper {
                let _ = h.DragLeave();
            }
        }
        *self.data.borrow_mut() = None;
        Ok(())
    }

    fn Drop(&self, pdataobj: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, eff: *mut DROPEFFECT) -> windows::core::Result<()> {
        let data = pdataobj.ok()?.clone();
        let cur = self.cur.borrow_mut().take();
        // Remember where the drop happened (new items will be placed there).
        with(|app| {
            let p = app.screen_to_dip(pt.x, pt.y);
            let bx = match app.hit_test(p) {
                Hit::BoxBody { id } | Hit::Header { id, .. } | Hit::Item { in_box: Some(id), .. } => Some(id),
                _ => None,
            };
            if let (Some(Target::Own), Some((keys, start))) = (&cur, app.own_drag.clone()) {
                app.drop_items(keys, start, p);
                app.invalidate();
            } else if cur.as_ref().map(|c| c.key() == "bg").unwrap_or(false) {
                app.pending_drop = Some((std::time::Instant::now(), p, bx));
                app.known_before_drop = app.items.keys().cloned().collect();
            }
        });
        unsafe {
            if let Some(h) = &self.helper {
                let _ = h.Drop(&data, &POINT { x: pt.x, y: pt.y }, *eff);
            }
            let r = match &cur {
                Some(Target::Shell(_, t)) => t.Drop(&data, keys, *pt, eff),
                // A layout move: tell the drag source nothing happened to the files.
                Some(Target::Own) | None => {
                    *eff = DROPEFFECT_NONE;
                    Ok(())
                }
            };
            log::info!("drop on {:?} -> {r:?} effect={:?}", cur.as_ref().map(|c| c.key().to_string()), *eff);
        }
        *self.data.borrow_mut() = None;
        crate::events::post(crate::events::Ev::Rescan);
        Ok(())
    }
}
