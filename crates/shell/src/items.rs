//! Desktop contents (docs/ARCHITECTURE.md §6): the user and Public desktop folders (or one fixture
//! folder), plus the virtual items enabled in Desktop Icon Settings.

use crate::{pcw, reg, wide};
use deskboxes_core::{Item, ItemKey, ItemKind, Source};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::*;

#[derive(Clone, Debug)]
pub struct DirSpec {
    pub path: PathBuf,
    pub source: Source,
}

fn known(id: &windows::core::GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s.map(PathBuf::from)
    }
}

/// The user's own folders a folder box can be made for: (id, path), those that exist.
pub fn known_folders() -> Vec<(&'static str, PathBuf)> {
    [("documents", &FOLDERID_Documents), ("downloads", &FOLDERID_Downloads), ("pictures", &FOLDERID_Pictures), ("music", &FOLDERID_Music), ("videos", &FOLDERID_Videos)]
        .iter()
        .filter_map(|(n, id)| known(id).filter(|p| p.is_dir()).map(|p| (*n, p)))
        .collect()
}

/// The folders to show: the fixture folder in test mode, else the user's and the Public desktop.
pub fn desktop_dirs(fixture: Option<&Path>) -> Vec<DirSpec> {
    if let Some(f) = fixture {
        return vec![DirSpec { path: f.to_path_buf(), source: Source::Fixture }];
    }
    let mut v = Vec::new();
    if let Some(p) = known(&FOLDERID_Desktop) {
        v.push(DirSpec { path: p, source: Source::User });
    }
    if let Some(p) = known(&FOLDERID_PublicDesktop) {
        v.push(DirSpec { path: p, source: Source::Public });
    }
    v
}

/// Explorer's view filters (HKCU ...\Explorer\Advanced).
#[derive(Clone, Copy, Debug)]
pub struct Filters {
    pub show_hidden: bool,
    pub show_super_hidden: bool,
}

pub fn filters() -> Filters {
    Filters {
        show_hidden: reg::get_dword(reg::ADVANCED, "Hidden") == Some(1),
        show_super_hidden: reg::get_dword(reg::ADVANCED, "ShowSuperHidden") == Some(1),
    }
}

/// One directory entry from the fast path (no shell calls).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: String,
    pub name: String,
    pub attrs: u32,
    pub size: u64,
    pub modified: u64,
    pub source: Source,
}

/// `FindFirstFileExW(FindExInfoBasic, LARGE_FETCH)` over the folders, filtered like Explorer.
pub fn scan(dirs: &[DirSpec], f: Filters) -> Vec<Entry> {
    let mut out = Vec::new();
    for d in dirs {
        let pat = wide(&format!("{}\\*", d.path.display()));
        let mut fd = WIN32_FIND_DATAW::default();
        let h = unsafe { FindFirstFileExW(pcw(&pat), FindExInfoBasic, &mut fd as *mut _ as _, FindExSearchNameMatch, None, FIND_FIRST_EX_LARGE_FETCH) };
        let Ok(h) = h else { continue };
        loop {
            let n = fd.cFileName.iter().position(|&c| c == 0).unwrap_or(fd.cFileName.len());
            let name = String::from_utf16_lossy(&fd.cFileName[..n]);
            let a = fd.dwFileAttributes;
            let hidden = a & FILE_ATTRIBUTE_HIDDEN.0 != 0;
            let system = a & FILE_ATTRIBUTE_SYSTEM.0 != 0;
            let skip = name == "." || name == ".." || (hidden && !f.show_hidden) || (hidden && system && !f.show_super_hidden);
            if !skip {
                out.push(Entry {
                    path: format!("{}\\{}", d.path.display(), name),
                    name,
                    attrs: a,
                    size: ((fd.nFileSizeHigh as u64) << 32) | fd.nFileSizeLow as u64,
                    modified: ((fd.ftLastWriteTime.dwHighDateTime as u64) << 32) | fd.ftLastWriteTime.dwLowDateTime as u64,
                    source: d.source,
                });
            }
            if unsafe { FindNextFileW(h, &mut fd) }.is_err() {
                break;
            }
        }
        unsafe {
            let _ = FindClose(h);
        }
    }
    out
}

/// Stable identity: (volume serial, FileId). Opened with FILE_READ_ATTRIBUTES only and
/// OPEN_REPARSE_POINT, so cloud placeholders are not hydrated.
pub fn file_key(path: &str) -> ItemKey {
    unsafe {
        let p = wide(path);
        let h = CreateFileW(
            pcw(&p),
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        );
        if let Ok(h) = h {
            let mut info = FILE_ID_INFO::default();
            let r = GetFileInformationByHandleEx(h, FileIdInfo, &mut info as *mut _ as _, std::mem::size_of::<FILE_ID_INFO>() as u32);
            let _ = CloseHandle(h);
            if r.is_ok() {
                return ItemKey::file(info.VolumeSerialNumber, u128::from_le_bytes(info.FileId.Identifier));
            }
        }
        ItemKey::path(path)
    }
}

/// Explorer's display name (SIGDN_NORMALDISPLAY: desktop.ini names, hidden .lnk/.url, hide-extension).
pub fn display_name(parsing: &str) -> Option<String> {
    unsafe {
        let p = wide(parsing);
        let si: IShellItem = SHCreateItemFromParsingName(pcw(&p), None).ok()?;
        let n = si.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        let s = n.to_string().ok();
        CoTaskMemFree(Some(n.0 as _));
        s
    }
}

fn kind_of(e: &Entry) -> ItemKind {
    let lower = e.name.to_lowercase();
    if e.attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0 {
        ItemKind::Folder
    } else if lower.ends_with(".lnk") {
        ItemKind::Shortcut
    } else if lower.ends_with(".url") {
        ItemKind::UrlFile
    } else {
        ItemKind::File
    }
}

/// Full item for an entry (one CreateFile + one shell item; run on a worker).
pub fn item_from_entry(e: &Entry) -> Item {
    let kind = kind_of(e);
    let ext = if kind == ItemKind::Folder { String::new() } else { Path::new(&e.name).extension().map(|x| format!(".{}", x.to_string_lossy().to_lowercase())).unwrap_or_default() };
    Item {
        key: file_key(&e.path),
        display_name: display_name(&e.path).unwrap_or_else(|| e.name.clone()),
        parsing_name: e.path.clone(),
        kind,
        ext,
        size: e.size,
        modified: e.modified,
        attrs: e.attrs,
        source: e.source,
        icon: 0,
    }
}

/// Virtual desktop items and whether Windows shows them by default (Desktop Icon Settings).
const VIRTUAL: &[(&str, bool)] = &[
    ("{20D04FE0-3AEA-1069-A2D8-08002B30309D}", false), // This PC
    ("{59031a47-3f72-44a7-89c5-5595fe6b30ee}", false), // user's files
    ("{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}", false), // Network
    ("{645FF040-5081-101B-9F08-00AA002F954E}", true),  // Recycle Bin
    ("{5399E694-6CE5-4D6C-8FCE-1D8870FDCBA0}", false), // Control Panel
];

/// Virtual items enabled in Desktop Icon Settings (`HideDesktopIcons\NewStartPanel`: 0 = shown).
pub fn virtual_items() -> Vec<Item> {
    let key = r"Software\Microsoft\Windows\CurrentVersion\Explorer\HideDesktopIcons\NewStartPanel";
    VIRTUAL
        .iter()
        .filter(|(clsid, def)| reg::get_dword(key, clsid).map(|v| v == 0).unwrap_or(*def))
        .filter_map(|(clsid, _)| {
            let parsing = format!("::{clsid}");
            let name = display_name(&parsing)?;
            Some(Item {
                key: ItemKey::virtual_item(&parsing),
                parsing_name: parsing,
                display_name: name,
                kind: ItemKind::Virtual,
                ext: String::new(),
                size: 0,
                modified: 0,
                attrs: 0,
                source: Source::Virtual,
                icon: 0,
            })
        })
        .collect()
}
