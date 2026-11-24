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
/// Also returns the folders that could not be listed in full (their entries are left out: the
/// caller keeps what it knew of them, rather than taking them for empty).
pub fn scan(dirs: &[DirSpec], f: Filters) -> (Vec<Entry>, Vec<PathBuf>) {
    let mut out = Vec::new();
    let mut failed = Vec::new();
    for d in dirs {
        let pat = wide(&format!("{}\\*", d.path.display()));
        let mut fd = WIN32_FIND_DATAW::default();
        let h = unsafe { FindFirstFileExW(pcw(&pat), FindExInfoBasic, &mut fd as *mut _ as _, FindExSearchNameMatch, None, FIND_FIRST_EX_LARGE_FETCH) };
        let h = match h {
            Ok(h) => h,
            // Not there (any more): truly nothing in it.
            Err(e) if e.code() == ERROR_PATH_NOT_FOUND.to_hresult() || e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => continue,
            Err(e) => {
                log::warn!("cannot list {}: {e}", d.path.display());
                failed.push(d.path.clone());
                continue;
            }
        };
        let start = out.len();
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
            if let Err(e) = unsafe { FindNextFileW(h, &mut fd) } {
                if e.code() != ERROR_NO_MORE_FILES.to_hresult() {
                    // Stopped part way: what was listed is not the whole folder.
                    log::warn!("listing {} stopped: {e}", d.path.display());
                    out.truncate(start);
                    failed.push(d.path.clone());
                }
                break;
            }
        }
        unsafe {
            let _ = FindClose(h);
        }
    }
    (out, failed)
}

/// Stable identity: (volume serial, FileId). Opened with FILE_READ_ATTRIBUTES only and
/// OPEN_REPARSE_POINT, so cloud placeholders are not hydrated.
pub fn file_key(path: &str) -> ItemKey {
    match file_id(path) {
        Some(deskboxes_core::layout::FileId(v, id)) => ItemKey::file(v, id),
        None => ItemKey::path(path),
    }
}

/// The file's identity on its volume, if it can be read (as `file_key`).
pub fn file_id(path: &str) -> Option<deskboxes_core::layout::FileId> {
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
        )
        .ok()?;
        let mut info = FILE_ID_INFO::default();
        let r = GetFileInformationByHandleEx(h, FileIdInfo, &mut info as *mut _ as _, std::mem::size_of::<FILE_ID_INFO>() as u32);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(deskboxes_core::layout::FileId(info.VolumeSerialNumber, u128::from_le_bytes(info.FileId.Identifier)))
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

fn from_buf(b: &[u16]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..n])
}

/// A .lnk's target (a path, or a shell item's parsing name such as a Store app's) and its
/// arguments after a space. Only read, never resolved (no searching for a moved target).
fn lnk_target(path: &str) -> Option<String> {
    use windows::core::Interface;
    use windows::Win32::System::Com::*;
    unsafe {
        let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let pf: IPersistFile = sl.cast().ok()?;
        pf.Load(pcw(&wide(path)), STGM_READ).ok()?;
        let mut buf = vec![0u16; 2048];
        let mut target = if sl.GetPath(&mut buf, std::ptr::null_mut(), 0).is_ok() { from_buf(&buf) } else { String::new() };
        if target.is_empty() {
            if let Ok(pidl) = sl.GetIDList() {
                if !pidl.is_null() {
                    if let Ok(n) = SHGetNameFromIDList(pidl, SIGDN_DESKTOPABSOLUTEPARSING) {
                        target = n.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(n.0 as _));
                    }
                    CoTaskMemFree(Some(pidl as _));
                }
            }
        }
        buf.fill(0);
        let args = if sl.GetArguments(&mut buf).is_ok() { from_buf(&buf) } else { String::new() };
        let args = args.trim();
        Some(if args.is_empty() { target } else { format!("{target} {args}") })
    }
}

/// The URL in an internet shortcut's text: `URL=` in its `[InternetShortcut]` section.
pub fn url_in(text: &str) -> Option<String> {
    let mut section = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            section = l.eq_ignore_ascii_case("[InternetShortcut]");
        } else if let (true, Some((k, v))) = (section, l.split_once('=')) {
            if k.trim().eq_ignore_ascii_case("url") {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// A .url or .website file's URL (the file is ANSI, UTF-8 or UTF-16 text).
fn url_target(path: &str) -> Option<String> {
    use std::io::Read;
    let mut b = Vec::new();
    std::fs::File::open(path).ok()?.take(64 * 1024).read_to_end(&mut b).ok()?;
    let text = if let Some(u) = b.strip_prefix(&[0xFF, 0xFE]) {
        String::from_utf16_lossy(&u.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
    } else {
        String::from_utf8_lossy(b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&b)).into_owned()
    };
    url_in(&text)
}

/// Where a desktop shortcut leads (`Item::link`), for sorting rules. Not read for icons in a
/// folder box (rules never sort those), nor for files only in the cloud (reading would
/// download them).
fn link_of(e: &Entry, kind: ItemKind, ext: &str) -> String {
    const NOT_LOCAL: u32 = 0x1000 | 0x40000 | 0x400000; // OFFLINE, RECALL_ON_OPEN, RECALL_ON_DATA_ACCESS
    if e.attrs & NOT_LOCAL != 0 || e.source == Source::Folder {
        return String::new();
    }
    let link = match kind {
        ItemKind::Shortcut => lnk_target(&e.path),
        ItemKind::UrlFile => url_target(&e.path),
        ItemKind::File if ext == ".website" => url_target(&e.path),
        _ => None,
    };
    link.unwrap_or_default()
}

/// Full item for an entry (one CreateFile + one shell item, and a shortcut's target; run on a
/// worker).
pub fn item_from_entry(e: &Entry) -> Item {
    let kind = kind_of(e);
    let ext = if kind == ItemKind::Folder { String::new() } else { Path::new(&e.name).extension().map(|x| format!(".{}", x.to_string_lossy().to_lowercase())).unwrap_or_default() };
    Item {
        link: link_of(e, kind, &ext),
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
                link: String::new(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::Interface;
    use windows::Win32::System::Com::*;

    fn entry(path: &Path) -> Entry {
        Entry { path: path.display().to_string(), name: path.file_name().unwrap().to_string_lossy().into_owned(), attrs: 0x20, size: 1, modified: 1, source: Source::Fixture }
    }

    #[test]
    fn shortcut_targets_are_read() {
        crate::com_sta();
        let dir = std::env::temp_dir().join(format!("dbx-links-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A .lnk with arguments (made the way Explorer makes one).
        let lnk = dir.join("Dota 2.lnk");
        unsafe {
            let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            sl.SetPath(pcw(&wide(r"C:\Program Files (x86)\Steam\steam.exe"))).unwrap();
            sl.SetArguments(pcw(&wide("-applaunch 570"))).unwrap();
            sl.cast::<IPersistFile>().unwrap().Save(pcw(&wide(&lnk.display().to_string())), true).unwrap();
        }
        let it = item_from_entry(&entry(&lnk));
        assert_eq!(it.kind, ItemKind::Shortcut);
        assert_eq!(it.link.to_lowercase(), r"c:\program files (x86)\steam\steam.exe -applaunch 570");
        // .url files as Steam (ANSI) and as UTF-16 with a BOM.
        let url = dir.join("Cyberpunk 2077.url");
        std::fs::write(&url, "[{000214A0-0000-0000-C000-000000000046}]\r\nProp3=19,0\r\n[InternetShortcut]\r\nIDList=\r\nIconIndex=0\r\nURL=steam://rungameid/1091500\r\n").unwrap();
        assert_eq!(item_from_entry(&entry(&url)).link, "steam://rungameid/1091500");
        let wurl = dir.join("Wiadomości.url");
        let mut b = vec![0xFF, 0xFE];
        b.extend("[InternetShortcut]\r\nURL=https://example.com/zażółć\r\n".encode_utf16().flat_map(|c| c.to_le_bytes()));
        std::fs::write(&wurl, b).unwrap();
        assert_eq!(item_from_entry(&entry(&wurl)).link, "https://example.com/zażółć");
        // Cloud-only files are not read; plain files have no link.
        assert_eq!(item_from_entry(&Entry { attrs: 0x20 | 0x400000, ..entry(&url) }).link, "");
        let txt = dir.join("a.txt");
        std::fs::write(&txt, "URL=x").unwrap();
        assert_eq!(item_from_entry(&entry(&txt)).link, "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn url_lines() {
        assert_eq!(url_in("[Other]\nURL=nope\n[InternetShortcut]\n url = https://a.b/?x=1 \n").as_deref(), Some("https://a.b/?x=1"));
        assert_eq!(url_in("URL=https://a.b/"), None);
    }
}
