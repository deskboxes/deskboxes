//! File operations, only through `IFileOperation` (Windows' own engine: undo, Recycle Bin, conflict
//! dialogs). `no_ui` is for tests on fixtures (FOF_NO_UI); the product passes false.

use crate::{pcw, wide};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;

fn op(owner: isize, no_ui: bool, recycle: bool) -> windows::core::Result<IFileOperation> {
    unsafe {
        let fo: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
        let mut flags = FOF_ALLOWUNDO.0;
        if recycle {
            flags |= FOFX_RECYCLEONDELETE.0;
        }
        if no_ui {
            flags |= FOF_NO_UI.0;
        }
        fo.SetOperationFlags(FILEOPERATION_FLAGS(flags))?;
        if owner != 0 {
            fo.SetOwnerWindow(HWND(owner as _))?;
        }
        Ok(fo)
    }
}

fn item(parsing: &str) -> windows::core::Result<IShellItem> {
    let p = wide(parsing);
    unsafe { SHCreateItemFromParsingName(pcw(&p), None) }
}

/// Rename through the shell. Returns false if the user or a conflict aborted it.
pub fn rename(parsing: &str, new_name: &str, owner: isize, no_ui: bool) -> windows::core::Result<bool> {
    unsafe {
        let fo = op(owner, no_ui, false)?;
        let n = wide(new_name);
        fo.RenameItem(&item(parsing)?, pcw(&n), None)?;
        fo.PerformOperations()?;
        Ok(!fo.GetAnyOperationsAborted()?.as_bool())
    }
}

/// Delete (to the Recycle Bin when `recycle`, as the Delete key does; Shift+Delete passes false).
pub fn delete(parsing: &[String], recycle: bool, owner: isize, no_ui: bool) -> windows::core::Result<bool> {
    unsafe {
        let fo = op(owner, no_ui, recycle)?;
        for p in parsing {
            fo.DeleteItem(&item(p)?, None)?;
        }
        fo.PerformOperations()?;
        Ok(!fo.GetAnyOperationsAborted()?.as_bool())
    }
}

/// The shell item array for a selection (drag source, data objects).
pub fn item_array(parsing: &[String]) -> windows::core::Result<IShellItemArray> {
    unsafe {
        let mut pidls = Vec::new();
        for p in parsing {
            let h = wide(p);
            let mut pidl = std::ptr::null_mut();
            SHParseDisplayName(pcw(&h), None, &mut pidl, 0, None)?;
            pidls.push(pidl as *const Common::ITEMIDLIST);
        }
        let r = SHCreateShellItemArrayFromIDLists(&pidls);
        for p in pidls {
            CoTaskMemFree(Some(p as _));
        }
        r
    }
}

pub fn data_object(parsing: &[String]) -> windows::core::Result<IDataObject> {
    unsafe { item_array(parsing)?.BindToHandler(None, &BHID_DataObject) }
}


/// Move (or copy) items into a folder: what dropping them on a folder's icon does.
pub fn transfer(parsing: &[String], dest_folder: &str, copy: bool, owner: isize, no_ui: bool) -> windows::core::Result<bool> {
    unsafe {
        let fo = op(owner, no_ui, false)?;
        let dest = item(dest_folder)?;
        for p in parsing {
            if copy {
                fo.CopyItem(&item(p)?, &dest, None, None)?;
            } else {
                fo.MoveItem(&item(p)?, &dest, None, None)?;
            }
        }
        fo.PerformOperations()?;
        Ok(!fo.GetAnyOperationsAborted()?.as_bool())
    }
}
