//! File operations, only through `IFileOperation` (Windows' own engine: undo, Recycle Bin, conflict
//! dialogs). `no_ui` is for tests on fixtures (FOF_NO_UI); the product passes false.

use crate::{pcw, wide};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;

/// `undo`: the operation can be undone (Ctrl+Z); for a delete it also means "to the Recycle
/// Bin", so a permanent delete (Shift+Delete) passes false and gets Windows' own confirmation.
fn op(owner: isize, no_ui: bool, undo: bool) -> windows::core::Result<IFileOperation> {
    unsafe {
        let fo: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
        let mut flags = 0;
        if undo {
            flags |= FOF_ALLOWUNDO.0;
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
        let fo = op(owner, no_ui, true)?;
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
        if recycle {
            fo.SetOperationFlags(FILEOPERATION_FLAGS(FOF_ALLOWUNDO.0 | FOFX_RECYCLEONDELETE.0 | if no_ui { FOF_NO_UI.0 } else { 0 }))?;
        }
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
        let fo = op(owner, no_ui, true)?;
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

// ---------------------------------------------------------------- what was done (for undo)

/// One item an operation handled: where it was and where it is now (the new name, the moved or
/// copied file, the file in the Recycle Bin; "" when deleted for good or not known).
#[derive(Clone, Debug, PartialEq)]
pub struct Done {
    pub from: String,
    pub to: String,
}

fn path_of(i: &IShellItem) -> String {
    unsafe {
        i.GetDisplayName(SIGDN_FILESYSPATH)
            .map(|p| {
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as _));
                s
            })
            .unwrap_or_default()
    }
}

/// Listens to Windows' copy engine: every item that went through, and where it ended up.
#[windows::core::implement(IFileOperationProgressSink)]
struct Sink {
    done: std::rc::Rc<std::cell::RefCell<Vec<Done>>>,
}

impl Sink {
    fn note(&self, item: windows::core::Ref<IShellItem>, hr: windows::core::HRESULT, new: windows::core::Ref<IShellItem>) {
        if hr.is_err() {
            return;
        }
        let from = item.as_ref().map(path_of).unwrap_or_default();
        let to = new.as_ref().map(path_of).unwrap_or_default();
        if !from.is_empty() {
            self.done.borrow_mut().push(Done { from, to });
        }
    }
}

#[allow(non_snake_case)]
impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _: windows::core::HRESULT) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(&self, _: u32, _: windows::core::Ref<IShellItem>, _: &windows::core::PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostRenameItem(&self, _: u32, i: windows::core::Ref<IShellItem>, _: &windows::core::PCWSTR, hr: windows::core::HRESULT, n: windows::core::Ref<IShellItem>) -> windows::core::Result<()> {
        self.note(i, hr, n);
        Ok(())
    }
    fn PreMoveItem(&self, _: u32, _: windows::core::Ref<IShellItem>, _: windows::core::Ref<IShellItem>, _: &windows::core::PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostMoveItem(&self, _: u32, i: windows::core::Ref<IShellItem>, _: windows::core::Ref<IShellItem>, _: &windows::core::PCWSTR, hr: windows::core::HRESULT, n: windows::core::Ref<IShellItem>) -> windows::core::Result<()> {
        self.note(i, hr, n);
        Ok(())
    }
    fn PreCopyItem(&self, _: u32, _: windows::core::Ref<IShellItem>, _: windows::core::Ref<IShellItem>, _: &windows::core::PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostCopyItem(&self, _: u32, i: windows::core::Ref<IShellItem>, _: windows::core::Ref<IShellItem>, _: &windows::core::PCWSTR, hr: windows::core::HRESULT, n: windows::core::Ref<IShellItem>) -> windows::core::Result<()> {
        self.note(i, hr, n);
        Ok(())
    }
    fn PreDeleteItem(&self, _: u32, _: windows::core::Ref<IShellItem>) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostDeleteItem(&self, _: u32, i: windows::core::Ref<IShellItem>, hr: windows::core::HRESULT, n: windows::core::Ref<IShellItem>) -> windows::core::Result<()> {
        self.note(i, hr, n);
        Ok(())
    }
    fn PreNewItem(&self, _: u32, _: windows::core::Ref<IShellItem>, _: &windows::core::PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostNewItem(&self, _: u32, _: windows::core::Ref<IShellItem>, _: &windows::core::PCWSTR, _: &windows::core::PCWSTR, _: u32, _: windows::core::HRESULT, _: windows::core::Ref<IShellItem>) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _: u32, _: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Run `fill` (adding operations to `fo`) and perform them, listening: what went through.
fn run_logged(fo: &IFileOperation, fill: impl FnOnce(&IFileOperation) -> windows::core::Result<()>) -> windows::core::Result<Vec<Done>> {
    unsafe {
        let done = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink: IFileOperationProgressSink = Sink { done: done.clone() }.into();
        let cookie = fo.Advise(&sink)?;
        let r = fill(fo).and_then(|_| fo.PerformOperations());
        let _ = fo.Unadvise(cookie);
        r?;
        let v = done.borrow().clone();
        Ok(v)
    }
}

/// `rename`, telling what was renamed to what.
pub fn rename_logged(parsing: &str, new_name: &str, owner: isize, no_ui: bool) -> windows::core::Result<Vec<Done>> {
    let fo = op(owner, no_ui, true)?;
    let n = wide(new_name);
    run_logged(&fo, |fo| unsafe { fo.RenameItem(&item(parsing)?, pcw(&n), None) })
}

/// `delete`, telling where each item went (its file in the Recycle Bin; "" when deleted for good).
pub fn delete_logged(parsing: &[String], recycle: bool, owner: isize, no_ui: bool) -> windows::core::Result<Vec<Done>> {
    let fo = op(owner, no_ui, recycle)?;
    unsafe {
        if recycle {
            fo.SetOperationFlags(FILEOPERATION_FLAGS(FOF_ALLOWUNDO.0 | FOFX_RECYCLEONDELETE.0 | if no_ui { FOF_NO_UI.0 } else { 0 }))?;
        }
    }
    run_logged(&fo, |fo| {
        for p in parsing {
            unsafe { fo.DeleteItem(&item(p)?, None)? };
        }
        Ok(())
    })
}

/// `transfer`, telling where each item ended up (Windows may have named it "name (2)").
pub fn transfer_logged(parsing: &[String], dest_folder: &str, copy: bool, owner: isize, no_ui: bool) -> windows::core::Result<Vec<Done>> {
    let fo = op(owner, no_ui, true)?;
    let dest = item(dest_folder)?;
    run_logged(&fo, |fo| {
        for p in parsing {
            unsafe {
                if copy {
                    fo.CopyItem(&item(p)?, &dest, None, None)?;
                } else {
                    fo.MoveItem(&item(p)?, &dest, None, None)?;
                }
            }
        }
        Ok(())
    })
}

/// Move each `from` to the full path `to` (its folder and name): an undone move goes back.
pub fn move_back(pairs: &[(String, String)], owner: isize, no_ui: bool) -> windows::core::Result<Vec<Done>> {
    let fo = op(owner, no_ui, true)?;
    run_logged(&fo, |fo| {
        for (from, to) in pairs {
            let p = std::path::Path::new(to);
            let (Some(dir), Some(name)) = (p.parent(), p.file_name()) else { continue };
            let n = wide(&name.to_string_lossy());
            unsafe { fo.MoveItem(&item(from)?, &item(&dir.to_string_lossy())?, pcw(&n), None)? };
        }
        Ok(())
    })
}

/// Take items back out of the Recycle Bin to where they were (`(original path, file in the
/// Recycle Bin)`), as its Restore does: the file goes back and the bin's record of it goes.
/// When the bin's file is not known, the newest one deleted from that path is taken. Returns
/// what came back (original path, "").
pub fn restore(items: &[(String, String)]) -> Vec<Done> {
    let mut out = vec![];
    for (orig, bin) in items {
        let bin = if !bin.is_empty() && std::path::Path::new(bin).exists() { Some(std::path::PathBuf::from(bin)) } else { find_in_bin(orig) };
        let Some(r) = bin else {
            log::warn!("restore: {orig} is not in the Recycle Bin");
            continue;
        };
        if std::path::Path::new(orig).exists() {
            log::warn!("restore: {orig} exists again; left in the Recycle Bin");
            continue;
        }
        match std::fs::rename(&r, orig) {
            Ok(()) => {
                // Its record ($I…) next to it ($R…).
                if let (Some(dir), Some(name)) = (r.parent(), r.file_name().map(|n| n.to_string_lossy().into_owned())) {
                    if let Some(rest) = name.strip_prefix("$R") {
                        let _ = std::fs::remove_file(dir.join(format!("$I{rest}")));
                    }
                }
                notify(orig, &r);
                out.push(Done { from: orig.clone(), to: String::new() });
            }
            Err(e) => log::warn!("restore {orig}: {e}"),
        }
    }
    out
}

/// Tell Explorer (its windows, the Recycle Bin) that `gone` became `back`.
fn notify(back: &str, gone: &std::path::Path) {
    let a = wide(&gone.to_string_lossy());
    let b = wide(back);
    let dir = std::path::Path::new(back).is_dir();
    unsafe {
        SHChangeNotify(if dir { SHCNE_RMDIR } else { SHCNE_DELETE }, SHCNF_PATHW, Some(a.as_ptr() as _), None);
        SHChangeNotify(if dir { SHCNE_MKDIR } else { SHCNE_CREATE }, SHCNF_PATHW, Some(b.as_ptr() as _), None);
    }
}

/// The newest file in this user's Recycle Bin on `orig`'s drive deleted from `orig`: read from
/// the bin's records ($I files: version, size, deletion time, then the original path).
fn find_in_bin(orig: &str) -> Option<std::path::PathBuf> {
    let drive: String = orig.chars().take(3).collect();
    let root = std::path::Path::new(&drive).join("$Recycle.Bin");
    let mut best: Option<(u64, std::path::PathBuf)> = None;
    for sid in std::fs::read_dir(&root).ok()?.flatten() {
        let Ok(files) = std::fs::read_dir(sid.path()) else { continue };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            let Some(rest) = name.strip_prefix("$I") else { continue };
            let Ok(b) = std::fs::read(f.path()) else { continue };
            if b.len() < 28 {
                continue;
            }
            let ver = u64::from_le_bytes(b[0..8].try_into().ok()?);
            let when = u64::from_le_bytes(b[16..24].try_into().ok()?);
            let path: Vec<u16> = if ver >= 2 {
                b[28..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
            } else {
                b[24..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
            };
            let end = path.iter().position(|c| *c == 0).unwrap_or(path.len());
            let p = String::from_utf16_lossy(&path[..end]);
            let r = sid.path().join(format!("$R{rest}"));
            if p.eq_ignore_ascii_case(orig) && r.exists() && best.as_ref().map(|(t, _)| when > *t).unwrap_or(true) {
                best = Some((when, r));
            }
        }
    }
    best.map(|(_, p)| p)
}
