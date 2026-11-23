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
    /// Windows' own drop target: an item (folder, program, zip) or the folder background, and
    /// what the drag's caption says about it.
    Shell(String, IDropTarget, Dest),
    /// Our own icons dragged out and back again: a layout move, never a file operation (the
    /// desktop folder's drop target would try to move the files onto themselves).
    Own,
}

/// A drop place as the drag's caption names it ("Move to Documents", "Open with Notepad").
struct Dest {
    name: String,
    /// A program (or a shortcut to one): files dropped on it are opened with it.
    open_with: bool,
}

impl Target {
    fn key(&self) -> &str {
        match self {
            Target::Shell(k, _, _) => k,
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
    /// The drag caption we set last (type, text, place).
    caption_set: RefCell<Option<(DROPIMAGETYPE, String, String)>>,
}

impl DropTarget {
    pub fn create(hwnd: HWND) -> IDropTarget {
        let helper = unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok() };
        DropTarget {
            hwnd,
            cur: RefCell::new(None),
            data: RefCell::new(None),
            helper,
            caption_set: RefCell::new(None),
        }
        .into()
    }
}

/// The target at a point: an item's own drop target (not one of the dragged items), else, for our
/// own drag, a layout move, else the folder background's drop target.
fn target_at(hwnd: HWND, pt: &POINTL) -> Option<Target> {
    with(|app| {
        // (The desktop window under the boxes-above window: its loose icons count.)
        app.hit_desk = app.is_above() && hwnd != app.hwnd;
        let t = target_at_in(app, hwnd, pt);
        app.hit_desk = false;
        t
    })
    .flatten()
}

fn target_at_in(app: &mut crate::app::App, hwnd: HWND, pt: &POINTL) -> Option<Target> {
    {
        let p = app.screen_to_dip(pt.x, pt.y);
        let own = app.own_drag.as_ref().map(|(keys, _)| keys.clone());
        let hit = app.hit_test(p);
        // Boxes above windows, outside the boxes: the app below takes it once the window has
        // shrunk to the boxes (`above_drag_begin`); until then, nothing here does.
        if app.is_above() && hwnd == app.hwnd && matches!(hit, Hit::Empty) {
            return None;
        }
        if let Hit::Item { key, .. } = hit {
            let dragged = own.as_ref().map(|k| k.contains(&key)).unwrap_or(false);
            // Our own icons next to an icon (its name, the room around it) go in beside it: only
            // its picture takes them in (as the icons making room show).
            let beside = own.as_ref().is_some_and(|k| app.onto_icon(p, k).is_none());
            if !dragged && !beside {
                if let Some(it) = app.items.get(&key) {
                    if let Some(t) = deskboxes_shell::view::item_drop_target(&it.parsing_name) {
                        let container = it.is_folder() || crate::app::is_recycle_bin(&it.parsing_name) || it.ext.eq_ignore_ascii_case("zip") || it.parsing_name.starts_with("::");
                        let dest = Dest { name: it.display_name.clone(), open_with: !container };
                        return Some(Target::Shell(key.0.clone(), t, dest));
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
                .map(|t| {
                    let name = std::path::Path::new(&f).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| f.clone());
                    Target::Shell(format!("fb:{f}"), t, Dest { name, open_with: false })
                });
        }
        let folder = app.args.desktop_dir.as_ref().map(|d| d.display().to_string());
        deskboxes_shell::view::background_drop_target(folder.as_deref(), hwnd.0 as isize)
            .map_err(|e| log::warn!("background drop target: {e}"))
            .ok()
            .map(|t| Target::Shell("bg".to_string(), t, Dest { name: deskboxes_i18n::t(app.lang, deskboxes_i18n::S::DesktopName).to_string(), open_with: false }))
    }
}

/// The drag image's caption ("Move to Documents"): set by the drop target. Windows' folder drop
/// targets we hand drops to leave it to Explorer's window around them, so we say it: what the
/// drop would do (the effect) and where. Our own icons over the desktop: no caption, as on
/// Explorer's desktop (and none left over from an app the drag passed over).
unsafe fn set_caption(data: &IDataObject, ty: DROPIMAGETYPE, msg: &str, insert: &str) {
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
    (*p).r#type = ty;
    for (i, c) in msg.encode_utf16().take(259).enumerate() {
        (*p).szMessage[i] = c;
    }
    for (i, c) in insert.encode_utf16().take(259).enumerate() {
        (*p).szInsert[i] = c;
    }
    let _ = GlobalUnlock(hg);
    let med = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: hg }, pUnkForRelease: std::mem::ManuallyDrop::new(None) };
    // fRelease = true: the data object owns the memory now.
    if data.SetData(&fmt, &med, true).is_err() {
        let _ = GlobalFree(Some(hg));
    }
}

/// Tell the surface where the drag is and which icon would take it (highlighted).
fn feedback(pt: Option<&POINTL>, cur: Option<&Target>, eff: DROPEFFECT) {
    with(|app| {
        let p = pt.map(|pt| app.screen_to_dip(pt.x, pt.y));
        let hl = match cur {
            Some(Target::Shell(k, _, _)) if k != "bg" && !k.starts_with("fb:") && eff != DROPEFFECT_NONE => Some(deskboxes_core::ItemKey(k.clone())),
            _ => None,
        };
        app.ole_drag_over(p, hl);
    });
}

impl DropTarget_Impl {
    /// The caption for the current target and effect (set only when it changes).
    fn caption(&self, eff: DROPEFFECT) {
        use deskboxes_i18n::{t, S};
        let lang = with(|app| app.lang).unwrap_or(deskboxes_i18n::Lang::En);
        let want: (DROPIMAGETYPE, String, String) = match self.cur.borrow().as_ref() {
            Some(Target::Shell(_, _, d)) => {
                let pick = if eff.0 & DROPEFFECT_MOVE.0 != 0 {
                    Some((DROPIMAGE_MOVE, S::DropMoveTo))
                } else if eff.0 & DROPEFFECT_COPY.0 != 0 {
                    Some((DROPIMAGE_COPY, S::DropCopyTo))
                } else if eff.0 & DROPEFFECT_LINK.0 != 0 {
                    Some((DROPIMAGE_LINK, S::DropLinkIn))
                } else {
                    None
                };
                match pick {
                    Some((ty, s)) => (ty, t(lang, if d.open_with { S::DropOpenWith } else { s }).to_string(), d.name.clone()),
                    None => (DROPIMAGE_INVALID, String::new(), String::new()),
                }
            }
            // Our icons moved on the desktop: the ghost alone, no "Move" under it (Explorer's).
            Some(Target::Own) => (DROPIMAGE_NOIMAGE, String::new(), String::new()),
            _ => (DROPIMAGE_INVALID, String::new(), String::new()),
        };
        if self.caption_set.borrow().as_ref() == Some(&want) {
            return;
        }
        if let Some(d) = self.data.borrow().as_ref() {
            unsafe { set_caption(d, want.0, &want.1, &want.2) };
        }
        *self.caption_set.borrow_mut() = Some(want);
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, pdataobj: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, eff: *mut DROPEFFECT) -> windows::core::Result<()> {
        let data = pdataobj.ok()?.clone();
        *self.data.borrow_mut() = Some(data.clone());
        let allowed = unsafe { *eff };
        // A drag from an app over the boxes above windows: outside the boxes it goes on to the apps.
        with(|app| {
            if app.is_above() && self.hwnd == app.hwnd {
                app.above_drag_begin();
            }
        });
        let t = target_at(self.hwnd, pt);
        // Back from another window: its caption may still be on the drag.
        *self.caption_set.borrow_mut() = None;
        unsafe {
            match &t {
                Some(Target::Shell(_, t, _)) => t.DragEnter(&data, keys, *pt, eff)?,
                Some(Target::Own) => *eff = allowed & DROPEFFECT_MOVE,
                None => *eff = DROPEFFECT_NONE,
            }
        }
        *self.cur.borrow_mut() = t;
        self.caption(unsafe { *eff });
        feedback(Some(pt), self.cur.borrow().as_ref(), unsafe { *eff });
        unsafe {
            if let Some(h) = &self.helper {
                let _ = h.DragEnter(self.hwnd, &data, &POINT { x: pt.x, y: pt.y }, *eff);
            }
        }
        Ok(())
    }

    fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, eff: *mut DROPEFFECT) -> windows::core::Result<()> {
        let allowed = unsafe { *eff };
        let key_now = target_at(self.hwnd, pt);
        let changed = self.cur.borrow().as_ref().map(|c| c.key().to_string()) != key_now.as_ref().map(|k| k.key().to_string());
        unsafe {
            if changed {
                if let Some(Target::Shell(_, old, _)) = self.cur.borrow_mut().take() {
                    let _ = old.DragLeave();
                }
                if let (Some(Target::Shell(_, t, _)), Some(d)) = (&key_now, self.data.borrow().as_ref()) {
                    let mut e = allowed;
                    let _ = t.DragEnter(d, keys, *pt, &mut e);
                }
                *self.cur.borrow_mut() = key_now;
            }
            match self.cur.borrow().as_ref() {
                Some(Target::Shell(_, t, _)) => {
                    *eff = allowed;
                    t.DragOver(keys, *pt, eff)?;
                }
                Some(Target::Own) => *eff = allowed & DROPEFFECT_MOVE,
                None => *eff = DROPEFFECT_NONE,
            }
            self.caption(*eff);
            if let Some(h) = &self.helper {
                let _ = h.DragOver(&POINT { x: pt.x, y: pt.y }, *eff);
            }
            feedback(Some(pt), self.cur.borrow().as_ref(), *eff);
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        unsafe {
            if let Some(Target::Shell(_, t, _)) = self.cur.borrow_mut().take() {
                let _ = t.DragLeave();
            }
            if let Some(h) = &self.helper {
                let _ = h.DragLeave();
            }
        }
        *self.data.borrow_mut() = None;
        feedback(None, None, DROPEFFECT_NONE);
        Ok(())
    }

    fn Drop(&self, pdataobj: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, eff: *mut DROPEFFECT) -> windows::core::Result<()> {
        let data = pdataobj.ok()?.clone();
        let cur = self.cur.borrow_mut().take();
        // Remember where the drop happened (new items will be placed there). Our own icons
        // dropped on the surface, or onto a folder, the Recycle Bin or a program here, are ours to
        // do (through the worker, so Undo can take them back).
        let ours = with(|app| {
            app.hit_desk = app.is_above() && self.hwnd != app.hwnd;
            let p = app.screen_to_dip(pt.x, pt.y);
            let bx = match app.hit_test(p) {
                Hit::BoxBody { id } | Hit::Header { id, .. } | Hit::Item { in_box: Some(id), .. } => Some(id),
                _ => None,
            };
            if let Some((keys, start)) = app.own_drag.clone() {
                let take = match &cur {
                    Some(Target::Own) => true,
                    Some(Target::Shell(k, _, _)) => app.drop_onto_item(p, &keys).map(|t| t.0 == *k).unwrap_or(false),
                    None => false,
                };
                if take {
                    app.drop_items(keys, start, p);
                    app.hit_desk = false;
                    app.invalidate();
                    return true;
                }
            } else if cur.as_ref().map(|c| c.key() == "bg").unwrap_or(false) {
                app.pending_drop = Some((std::time::Instant::now(), p, bx));
                app.known_before_drop = app.items.keys().cloned().collect();
            }
            app.hit_desk = false;
            false
        })
        .unwrap_or(false);
        feedback(None, None, DROPEFFECT_NONE);
        unsafe {
            if let Some(h) = &self.helper {
                let _ = h.Drop(&data, &POINT { x: pt.x, y: pt.y }, *eff);
            }
            let r = match &cur {
                Some(Target::Shell(_, t, _)) if !ours => t.Drop(&data, keys, *pt, eff),
                Some(Target::Shell(_, t, _)) => {
                    let _ = t.DragLeave();
                    *eff = DROPEFFECT_NONE;
                    Ok(())
                }
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
