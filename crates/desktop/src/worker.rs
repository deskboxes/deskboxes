//! The shell worker (docs/ARCHITECTURE.md §2): one OLE STA thread that does every shell call the
//! UI thread must not block on: enumeration, display names, icons, file operations, paste,
//! clipboard, drops onto items. Results go back to the UI thread as events.

use crate::events::{post, Ev};
use deskboxes_core::snapshot::IconImage;
use deskboxes_core::layout::{FileId, FileMove, FileOp};
use deskboxes_core::Item;
use deskboxes_shell::items::{self, DirSpec, Entry};
use deskboxes_shell::{fileops, icons, view};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, SendError, Sender};
use std::time::Instant;
use windows::Win32::Foundation::{HANDLE, WAIT_FAILED};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, INFINITE};
use windows::Win32::UI::WindowsAndMessaging::{MsgWaitForMultipleObjectsEx, MWMO_INPUTAVAILABLE, QS_ALLINPUT};

pub enum Req {
    Scan,
    Rename { path: String, new_name: String },
    Delete { paths: Vec<String>, recycle: bool },
    Paste { folder: Option<String>, link: bool, pt: (i32, i32) },
    Clipboard { paths: Vec<String>, cut: bool },
    DropOnItem { target: String, paths: Vec<String>, copy: bool },
    /// View > icon size: Explorer's desktop view gets it, then every icon is extracted again.
    IconSize { dip: u32, px: u32, explorer: bool },
    /// Explorer's tooltip text for an item.
    InfoTip { key: deskboxes_core::ItemKey, path: String },
    /// The folders folder boxes show now: listed with the desktop from the next scan on.
    Folders(Vec<String>),
    /// Undo (`undo`) or redo a file operation.
    UndoFiles { op: FileOp, undo: bool },
    /// Icons for boxes that show them at `px`: (desktop icon id, parsing name, attributes).
    IconsAt { px: u32, items: Vec<(u32, String, u32)> },
}

fn file_name(p: &str) -> String {
    std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn id_of(p: &str) -> Option<FileId> {
    items::file_id(p)
}

/// Is the file at `at` still `m`'s file? (Its identity not known: taken to be.) A file saved
/// under that name since is somebody else's: undo and redo leave it alone.
fn still(m: &FileMove, at: &str) -> bool {
    let ok = m.id.is_none() || id_of(at) == m.id;
    if !ok {
        log::warn!("{at} is another file now: left alone");
    }
    ok
}

/// Each file that went (`Done`), as moved from `from` to `to`, with its identity where it is now.
fn moves(d: &[fileops::Done]) -> Vec<FileMove> {
    d.iter().filter(|d| !d.to.is_empty()).map(|d| FileMove::new(d.from.clone(), d.to.clone(), id_of(&d.to))).collect()
}

/// Undo or redo `op`; what really happened then (None: nothing went through).
fn undo_files(op: &FileOp, undo: bool, owner: isize, no_ui: bool) -> Option<FileOp> {
    let r = match (op, undo) {
        (FileOp::Moved(v), true) => {
            // Back from where it went to where it was.
            let go: Vec<(String, String)> = v.iter().filter(|m| still(m, &m.to)).map(|m| (m.to.clone(), m.from.clone())).collect();
            (!go.is_empty())
                .then(|| fileops::move_back(&go, owner, no_ui).ok())
                .flatten()
                .map(|d| FileOp::Moved(d.iter().filter(|d| !d.to.is_empty()).map(|d| FileMove::new(d.to.clone(), d.from.clone(), id_of(&d.to))).collect()))
        }
        (FileOp::Moved(v), false) => {
            let go: Vec<(String, String)> = v.iter().filter(|m| still(m, &m.from)).map(|m| (m.from.clone(), m.to.clone())).collect();
            (!go.is_empty()).then(|| fileops::move_back(&go, owner, no_ui).ok()).flatten().map(|d| FileOp::Moved(moves(&d)))
        }
        (FileOp::Copied(v), true) => {
            // Only the copies that are still the ones made.
            let copies: Vec<&FileMove> = v.iter().filter(|m| !m.to.is_empty() && still(m, &m.to)).collect();
            let paths: Vec<String> = copies.iter().map(|m| m.to.clone()).collect();
            (!paths.is_empty()).then(|| fileops::delete_logged(&paths, true, owner, no_ui).ok()).flatten().map(|d| {
                FileOp::Copied(copies.iter().filter(|m| d.iter().any(|d| d.from.eq_ignore_ascii_case(&m.to))).map(|m| FileMove::new(m.from.clone(), m.to.clone(), None)).collect())
            })
        }
        (FileOp::Copied(v), false) => {
            // Copied again: each copy where it was (the folder it was copied into).
            let mut out = vec![];
            for m in v {
                let Some(dir) = std::path::Path::new(&m.to).parent() else { continue };
                if let Ok(d) = fileops::transfer_logged(std::slice::from_ref(&m.from), &dir.to_string_lossy(), true, owner, no_ui) {
                    out.extend(moves(&d));
                }
            }
            Some(FileOp::Copied(out))
        }
        (FileOp::Recycled(v), true) => Some(FileOp::Recycled(fileops::restore(v))),
        (FileOp::Recycled(v), false) => {
            let go: Vec<(String, Option<FileId>)> = v.iter().filter(|m| still(m, &m.from)).map(|m| (m.from.clone(), m.id)).collect();
            recycle(&go, owner, no_ui)
        }
        (FileOp::Renamed(m), true) => still(m, &m.to)
            .then(|| fileops::rename_logged(&m.to, &file_name(&m.from), owner, no_ui).ok())
            .flatten()
            .and_then(|d| d.into_iter().find(|d| !d.to.is_empty()))
            .map(|d| FileOp::Renamed(FileMove::new(d.to.clone(), m.to.clone(), id_of(&d.to)))),
        (FileOp::Renamed(m), false) => still(m, &m.from)
            .then(|| fileops::rename_logged(&m.from, &file_name(&m.to), owner, no_ui).ok())
            .flatten()
            .and_then(|d| d.into_iter().find(|d| !d.to.is_empty()))
            .map(|d| FileOp::Renamed(FileMove::new(m.from.clone(), d.to.clone(), id_of(&d.to)))),
    };
    let r = r.filter(|o| !o.is_empty());
    log::info!("{} {op:?}: {}", if undo { "undo" } else { "redo" }, if r.is_some() { "done" } else { "failed" });
    r
}

/// Into the Recycle Bin (`(path, its identity)`): what went, with its bin file and identity.
fn recycle(files: &[(String, Option<FileId>)], owner: isize, no_ui: bool) -> Option<FileOp> {
    if files.is_empty() {
        return None;
    }
    let paths: Vec<String> = files.iter().map(|(p, _)| p.clone()).collect();
    let d = fileops::delete_logged(&paths, true, owner, no_ui).ok()?;
    let v: Vec<FileMove> = d
        .iter()
        .map(|d| {
            let before = files.iter().find(|(p, _)| p.eq_ignore_ascii_case(&d.from)).and_then(|(_, i)| *i);
            let id = if d.to.is_empty() { before } else { id_of(&d.to).or(before) };
            FileMove::new(d.from.clone(), d.to.clone(), id)
        })
        .collect();
    Some(FileOp::Recycled(v))
}

pub struct Config {
    pub dirs: Vec<DirSpec>,
    pub virtual_items: bool,
    pub icon_px: u32,
    pub owner: isize,
    pub import_positions: bool,
    /// FOF_NO_UI for file operations (fixture tests only).
    pub no_ui: bool,
    /// Items from the snapshot: unchanged entries reuse them (and their icon ids) without any
    /// shell call, so a warm start reconciles at file-system speed.
    pub seed: Vec<Item>,
    /// First icon id the worker may assign (above the snapshot's ids).
    pub first_icon_id: u32,
}

/// Sending side: the channel plus an auto-reset event that wakes the worker (no polling: the
/// worker sleeps in MsgWaitForMultipleObjectsEx until a request or a COM/window message arrives).
pub struct WorkerTx {
    tx: Sender<Req>,
    ev: isize,
}

impl WorkerTx {
    pub fn send(&self, r: Req) -> Result<(), SendError<Req>> {
        self.tx.send(r)?;
        unsafe {
            let _ = SetEvent(HANDLE(self.ev as _));
        }
        Ok(())
    }
}

pub fn start(cfg: Config) -> WorkerTx {
    let (tx, rx) = channel();
    let ev = unsafe { CreateEventW(None, false, false, None).expect("worker event") };
    let evi = ev.0 as isize;
    std::thread::Builder::new().name("shell-worker".into()).spawn(move || run(cfg, rx, evi)).expect("spawn shell worker");
    WorkerTx { tx, ev: evi }
}

/// Box-filter downscale of premultiplied BGRA to `px`×`px` (overlays come from the 256 px list).
fn scale(img: &IconImage, px: u32) -> IconImage {
    resample(img, px, px)
}

/// Fit an image into `px` × `px`, keeping its aspect ratio (thumbnails); never enlarges.
fn fit(img: IconImage, px: u32) -> IconImage {
    if img.w <= px && img.h <= px {
        return img;
    }
    let k = (px as f32 / img.w as f32).min(px as f32 / img.h as f32);
    resample(&img, ((img.w as f32 * k).round() as u32).max(1), ((img.h as f32 * k).round() as u32).max(1))
}

/// Box-filter resample to `ow` × `oh` (premultiplied BGRA).
fn resample(img: &IconImage, ow: u32, oh: u32) -> IconImage {
    if img.w == ow && img.h == oh {
        return img.clone();
    }
    let mut out = vec![0u8; (ow * oh * 4) as usize];
    for y in 0..oh {
        let (y0, y1) = (y * img.h / oh, ((y + 1) * img.h / oh).max(y * img.h / oh + 1));
        for x in 0..ow {
            let (x0, x1) = (x * img.w / ow, ((x + 1) * img.w / ow).max(x * img.w / ow + 1));
            let mut acc = [0u32; 4];
            let mut n = 0;
            for sy in y0..y1.min(img.h) {
                for sx in x0..x1.min(img.w) {
                    let i = ((sy * img.w + sx) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += img.bgra[i + c] as u32;
                    }
                    n += 1;
                }
            }
            let o = ((y * ow + x) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    IconImage { w: ow, h: oh, bgra: out }
}

/// Draw an overlay (the shortcut arrow) the way Explorer does: the overlay of the image list
/// two-thirds the icon's size, in the icon's bottom-left corner (a 32 px arrow on a 48 px icon;
/// measured against Explorer's desktop on 26300, where scaling it to the full icon came out
/// half again too big).
fn composite(base: &mut IconImage, over: &IconImage) {
    let t = overlay_px(base.w).min(base.h);
    let ov = scale(over, t);
    let y0 = base.h - t;
    for y in 0..t {
        for x in 0..t {
            let o = ((y * t + x) * 4) as usize;
            let i = (((y0 + y) * base.w + x) * 4) as usize;
            let ia = 255 - ov.bgra[o + 3] as u32;
            for c in 0..4 {
                base.bgra[i + c] = (ov.bgra[o + c] as u32 + (base.bgra[i + c] as u32 * ia + 127) / 255).min(255) as u8;
            }
        }
    }
}

/// The overlay's size for an icon of `px`.
fn overlay_px(px: u32) -> u32 {
    ((px * 2 + 1) / 3).max(8)
}

fn pump() {
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let mut m = MSG::default();
        while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}

struct IconCache {
    px: u32,
    thumb_exts: HashMap<String, bool>,
    by_sys: HashMap<(i32, u32), u32>,
    overlays: HashMap<u32, Option<IconImage>>,
    shield: Option<Option<IconImage>>,
    next: u32,
    /// Thumbnails and folder previews made ahead, in parallel (`prefetch_images`); `None` is a
    /// failed one (the item then gets its plain icon without trying again).
    prefetch: HashMap<String, Option<IconImage>>,
}

/// Windows' UAC shield on a program's icon: in the bottom-right corner, about 0.44 of the icon
/// (21 px on a 48 px icon; measured on Explorer's desktop, 26300).
fn add_shield(img: &mut IconImage, shield: &IconImage) {
    // The stock icon has empty margins: measure the shield itself (21 × 22 px on a 48 px icon,
    // 2 px in from the right edge, on the bottom edge).
    let shield = trim(shield);
    let sw = ((img.w as f32) * 0.44).round().max(1.0) as u32;
    let s_h = ((sw as f32) * shield.h as f32 / shield.w.max(1) as f32).round().max(1.0) as u32;
    let sh = resample(&shield, sw, s_h);
    let x0 = img.w.saturating_sub(sw + (img.w as f32 * 0.04).round() as u32);
    let y0 = img.h.saturating_sub(s_h);
    for y in 0..s_h.min(img.h - y0) {
        for x in 0..sw.min(img.w - x0) {
            let o = (((y * sw) + x) * 4) as usize;
            let b = ((((y0 + y) * img.w) + x0 + x) * 4) as usize;
            let ia = 255 - sh.bgra[o + 3] as u32;
            for c in 0..4 {
                img.bgra[b + c] = (sh.bgra[o + c] as u32 + (img.bgra[b + c] as u32 * ia + 127) / 255).min(255) as u8;
            }
        }
    }
}

impl IconCache {
    /// Whether an item shows a picture of its own instead of its type's icon: `Some(false)` a
    /// folder's preview of what is inside (as on Explorer's desktop), `Some(true)` a thumbnail
    /// (files whose type has a thumbnail handler: pictures, videos, documents that carry one).
    /// Never for cloud placeholders: reading one would download it.
    fn image_kind(&mut self, parsing: &str, attrs: u32) -> Option<bool> {
        const DIR: u32 = 0x10;
        const NOT_LOCAL: u32 = 0x1000 | 0x40000 | 0x400000; // OFFLINE, RECALL_ON_OPEN, RECALL_ON_DATA_ACCESS
        if attrs & NOT_LOCAL != 0 {
            return None;
        }
        if attrs & DIR != 0 {
            return std::path::Path::new(parsing).is_dir().then_some(false);
        }
        let ext = std::path::Path::new(parsing).extension().and_then(|e| e.to_str()).map(|e| format!(".{}", e.to_ascii_lowercase()))?;
        if per_instance_icon(parsing) {
            return None;
        }
        let has = *self.thumb_exts.entry(ext.clone()).or_insert_with(|| icons::has_thumbnail_handler(&ext));
        has.then_some(true)
    }

    /// Makes the thumbnails and folder previews of many new items at once on a few STA threads
    /// (one at a time on the worker took most of a cold start: ~3.8 s of 4.7 s for 1010 items,
    /// 315 of them pictures). `get_with` takes them from `prefetch`.
    fn prefetch_images(&mut self, items: &[(&str, u32)]) {
        let jobs = self.image_jobs(items);
        let made = make_images(&jobs, self.px);
        self.prefetch.extend(made);
    }

    /// The items of these that show a picture of their own (see `image_kind`).
    fn image_jobs<'a>(&mut self, items: &[(&'a str, u32)]) -> Vec<(&'a str, bool)> {
        items.iter().filter_map(|&(p, a)| self.image_kind(p, a).map(|t| (p, t))).collect()
    }

    /// Icon id for an item; new images are appended to `fresh`.
    fn get(&mut self, parsing: &str, fresh: &mut Vec<(u32, IconImage)>) -> u32 {
        let sys = icons::sys_index(parsing);
        self.get_with(parsing, 0x10, sys, fresh)
    }

    /// Same, with the system icon index already known (resolved off this thread).
    fn get_with(&mut self, parsing: &str, attrs: u32, (ix, ov): (i32, u32), fresh: &mut Vec<(u32, IconImage)>) -> u32 {
        let px = self.px;
        if let Some(thumb) = self.image_kind(parsing, attrs) {
            let img = match self.prefetch.remove(parsing) {
                Some(v) => v,
                None if thumb => icons::thumbnail(parsing, px),
                None => icons::folder_image(parsing, px),
            };
            if let Some(img) = img {
                let mut img = fit(img, px);
                if ov != 0 && img.w == img.h {
                    if let Some(o) = self.overlays.entry(ov).or_insert_with(|| icons::overlay_icon(ov, overlay_px(px))).clone() {
                        composite(&mut img, &o);
                    }
                }
                let id = self.next;
                self.next += 1;
                fresh.push((id, img));
                return id;
            }
        }
        // Index -1 (no system icon), or a type whose icon is per file (shortcuts, programs, .ico):
        // extract per item, never shared. Off the STA, SHGetFileInfo gives every .url the same
        // index, so sharing by index showed one game's icon on all Steam shortcuts (2026-11-19).
        let shared = ix >= 0 && !per_instance_icon(parsing);
        if shared {
            if let Some(id) = self.by_sys.get(&(ix, ov)) {
                return *id;
            }
        }
        let mut img = icons::item_icon(parsing, self.px).unwrap_or_else(|| {
            log::warn!("no icon for {parsing}; drawing an empty icon");
            IconImage {
                w: self.px,
                h: self.px,
                bgra: vec![0; (self.px * self.px * 4) as usize],
            }
        });
        if ov != 0 {
            let o = self.overlays.entry(ov).or_insert_with(|| icons::overlay_icon(ov, overlay_px(px))).clone();
            match o {
                Some(o) => composite(&mut img, &o),
                None => log::warn!("overlay {ov} unavailable for {parsing}"),
            }
        }
        // Programs (and shortcuts to them) that run as administrator get Windows' shield.
        if !shared && shield_candidate(parsing) && icons::needs_shield(parsing) {
            let px = self.px;
            if let Some(s) = self.shield.get_or_insert_with(|| icons::shield_image(px)).clone() {
                add_shield(&mut img, &s);
            }
        }
        let id = self.next;
        self.next += 1;
        if shared {
            self.by_sys.insert((ix, ov), id);
        }
        fresh.push((id, img));
        id
    }
}

/// Thumbnails (`true`) and folder previews (`false`) for these paths, made on a few STA threads.
/// Fewer than 8 are left to `get_with`, one at a time.
fn make_images(jobs: &[(&str, bool)], px: u32) -> Vec<(String, Option<IconImage>)> {
    if jobs.len() < 8 {
        return Vec::new();
    }
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 6).min(jobs.len() / 4);
    let at = std::sync::atomic::AtomicUsize::new(0);
    let t = Instant::now();
    let made: Vec<(String, Option<IconImage>)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let at = &at;
                s.spawn(move || {
                    use windows::Win32::System::Com::*;
                    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
                    let mut v = Vec::new();
                    loop {
                        let i = at.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&(p, thumb)) = jobs.get(i) else { break };
                        let img = if thumb { icons::thumbnail(p, px) } else { icons::folder_image(p, px) };
                        v.push((p.to_string(), img));
                    }
                    if init.is_ok() {
                        unsafe { CoUninitialize() };
                    }
                    v
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    log::info!("made {} picture(s) on {threads} threads in {:.1} ms", made.len(), t.elapsed().as_secs_f64() * 1000.0);
    made
}

/// The image cropped to its non-transparent pixels.
fn trim(img: &IconImage) -> IconImage {
    let (mut x0, mut y0, mut x1, mut y1) = (img.w, img.h, 0, 0);
    for y in 0..img.h {
        for x in 0..img.w {
            if img.bgra[((y * img.w + x) * 4 + 3) as usize] > 8 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return img.clone();
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in y0..y1 {
        let i = ((y * img.w + x0) * 4) as usize;
        out.extend_from_slice(&img.bgra[i..i + (w * 4) as usize]);
    }
    IconImage { w, h, bgra: out }
}

/// Types that can carry the shield: programs and shortcuts.
fn shield_candidate(parsing: &str) -> bool {
    let ext = std::path::Path::new(parsing).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "lnk" | "exe" | "msc" | "bat" | "cmd")
}

/// File types whose icon comes from the file itself (Explorer's per-instance icons).
fn per_instance_icon(parsing: &str) -> bool {
    let ext = std::path::Path::new(parsing).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "lnk" | "url" | "website" | "exe" | "ico" | "cur" | "ani" | "appref-ms" | "msc" | "scr" | "cpl" | "pif")
}

/// Shell lookups for new entries on up to 8 MTA threads (SHCreateItemFromParsingName,
/// GetDisplayName, the file id and SHGetFileInfo's system icon index are free-threaded; icon
/// IMAGES are extracted afterwards on the worker's STA). Order is preserved.
fn resolve_parallel(entries: &[&Entry]) -> Vec<(Item, (i32, u32))> {
    let resolve = |e: &Entry| (items::item_from_entry(e), icons::sys_index(&e.path));
    if entries.len() < 32 {
        return entries.iter().map(|e| resolve(e)).collect();
    }
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
    let chunk = entries.len().div_ceil(threads);
    std::thread::scope(|s| {
        let handles: Vec<_> = entries
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    use windows::Win32::System::Com::*;
                    let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                    let v: Vec<_> = part.iter().map(|e| resolve(e)).collect();
                    if init.is_ok() {
                        unsafe { CoUninitialize() };
                    }
                    v
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().expect("resolve thread")).collect()
    })
}

fn run(cfg: Config, rx: Receiver<Req>, ev: isize) {
    unsafe {
        let _ = windows::Win32::System::Ole::OleInitialize(None);
    }
    let mut cache: HashMap<String, (Entry, Item)> = HashMap::new();
    let mut folders: Vec<String> = vec![];
    for it in &cfg.seed {
        if it.kind == deskboxes_core::ItemKind::Virtual {
            continue;
        }
        let name = std::path::Path::new(&it.parsing_name).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let e = Entry {
            path: it.parsing_name.clone(),
            name,
            attrs: it.attrs,
            size: it.size,
            modified: it.modified,
            source: it.source,
        };
        cache.insert(it.parsing_name.clone(), (e, it.clone()));
    }
    let mut ic = IconCache {
        px: cfg.icon_px,
        thumb_exts: HashMap::new(),
        by_sys: HashMap::new(),
        overlays: HashMap::new(),
        shield: None,
        next: cfg.first_icon_id,
        prefetch: HashMap::new(),
    };
    let mut sized: HashMap<u32, IconCache> = HashMap::new();
    let mut first = true;
    let mut batch: Vec<Req> = Vec::new();
    loop {
        // An STA must pump: shell drop targets and the copy engine call back into this
        // apartment (the data object lives here) from their own threads.
        pump();
        loop {
            match rx.try_recv() {
                Ok(r) => batch.push(r),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
        }
        if batch.is_empty() {
            let h = [HANDLE(ev as _)];
            if unsafe { MsgWaitForMultipleObjectsEx(Some(&h), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE) } == WAIT_FAILED {
                log::error!("worker wait failed");
                return;
            }
            continue;
        }
        // Coalesce: a scan followed later in the batch by another scan is redundant.
        let reqs = std::mem::take(&mut batch);
        let last_scan = reqs.iter().rposition(|r| matches!(r, Req::Scan));
        for (i, req) in reqs.into_iter().enumerate() {
            if matches!(req, Req::Scan) && Some(i) != last_scan {
                continue;
            }
            match req {
                Req::Scan => {
                    let t0 = Instant::now();
                    let f = items::filters();
                    let mut dirs = cfg.dirs.clone();
                    dirs.extend(folders.iter().map(|p| items::DirSpec {
                        path: p.into(),
                        source: deskboxes_core::Source::Folder,
                    }));
                    let (mut entries, failed) = items::scan(&dirs, f);
                    // A folder that could not be listed keeps what was known of it (its icons
                    // are not taken for deleted and lose their boxes).
                    for dir in &failed {
                        let prefix = format!("{}\\", dir.display()).to_lowercase();
                        let kept: Vec<Entry> = cache.values().map(|(e, _)| e).filter(|e| e.path.to_lowercase().strip_prefix(&prefix).is_some_and(|rest| !rest.contains('\\'))).cloned().collect();
                        log::warn!("{}: keeping the {} item(s) known from before", dir.display(), kept.len());
                        entries.extend(kept);
                    }
                    let t_scan = t0.elapsed().as_secs_f64() * 1000.0;
                    let mut fresh = Vec::new();
                    let mut out = Vec::with_capacity(entries.len() + 5);
                    let mut next_cache = HashMap::with_capacity(entries.len());
                    // Unchanged entries reuse their item; the rest are resolved through the shell
                    // (display name, file id, system icon index) on parallel MTA threads, then get
                    // their icon images here, serially, deduplicated by system icon index.
                    let mut slots: Vec<Option<Item>> = Vec::with_capacity(entries.len());
                    let mut todo: Vec<usize> = Vec::new();
                    for e in &entries {
                        match cache.remove(&e.path) {
                            Some((old, it)) if old == *e => slots.push(Some(it)),
                            _ => {
                                todo.push(slots.len());
                                slots.push(None);
                            }
                        }
                    }
                    let resolved = todo.len();
                    let todo_entries: Vec<&Entry> = todo.iter().map(|&i| &entries[i]).collect();
                    let t_res = Instant::now();
                    // The shell lookups and the pictures at the same time (both on threads of their own).
                    let jobs = ic.image_jobs(&todo_entries.iter().map(|e| (e.path.as_str(), e.attrs)).collect::<Vec<_>>());
                    let px = ic.px;
                    let (res, res_ms, made) = std::thread::scope(|s| {
                        let pics = s.spawn(|| make_images(&jobs, px));
                        let res = resolve_parallel(&todo_entries);
                        let res_ms = t_res.elapsed().as_secs_f64() * 1000.0;
                        (res, res_ms, pics.join().unwrap_or_default())
                    });
                    ic.prefetch.extend(made);
                    for (&i, (mut it, sys)) in todo.iter().zip(res) {
                        it.icon = ic.get_with(&entries[i].path, entries[i].attrs, sys, &mut fresh);
                        slots[i] = Some(it);
                    }
                    ic.prefetch.clear();
                    for (e, item) in entries.into_iter().zip(slots) {
                        let item = item.expect("every entry resolved");
                        out.push(item.clone());
                        next_cache.insert(e.path.clone(), (e, item));
                    }
                    if resolved > 0 {
                        log::info!("resolved {resolved} entr(ies) through the shell in {res_ms:.1} ms (parallel)");
                    }
                    cache = next_cache;
                    // Hard links: two names of one file share its identity. Each name is an icon
                    // of its own: the second and later get their name added to the key.
                    let mut seen: std::collections::HashSet<deskboxes_core::ItemKey> = std::collections::HashSet::with_capacity(out.len());
                    for it in out.iter_mut() {
                        if !seen.insert(it.key.clone()) {
                            it.key = deskboxes_core::ItemKey(format!("{}|{}", it.key.0, it.parsing_name.to_lowercase()));
                            seen.insert(it.key.clone());
                        }
                    }
                    if cfg.virtual_items {
                        for mut v in items::virtual_items() {
                            v.icon = ic.get(&v.parsing_name, &mut fresh);
                            out.push(v);
                        }
                    }
                    let native = view::native_view(first && cfg.import_positions);
                    let ms = t0.elapsed().as_secs_f64() * 1000.0;
                    log::info!("PHASE scan items={} resolved={resolved} new_icons={} fast_path_ms={t_scan:.1} total_ms={ms:.1}", out.len(), fresh.len());
                    let keep: Vec<u32> = ic.by_sys.values().chain(sized.values().flat_map(|c| c.by_sys.values())).copied().collect();
                    post(Ev::Scanned {
                        items: out,
                        icons: fresh,
                        native,
                        ms,
                        first,
                        resolved,
                        keep,
                    });
                    first = false;
                }
                Req::Folders(v) => folders = v,
                Req::Rename { path, new_name } => {
                    let r = fileops::rename_logged(&path, &new_name, cfg.owner, cfg.no_ui);
                    log::info!("rename {path} -> {new_name}: {r:?}");
                    if let Some(d) = r.ok().and_then(|d| d.into_iter().find(|d| !d.to.is_empty())) {
                        let id = id_of(&d.to);
                        post(Ev::FileDone { op: FileOp::Renamed(FileMove::new(d.from, d.to, id)) });
                    }
                    post(Ev::Rescan);
                }
                Req::Delete { paths, recycle: true } => {
                    // Into the Recycle Bin: undoable. Each file's identity first, so an undo
                    // brings back this very file.
                    let files: Vec<(String, Option<FileId>)> = paths.iter().map(|p| (p.clone(), id_of(p))).collect();
                    let r = recycle(&files, cfg.owner, cfg.no_ui);
                    log::info!("delete {} item(s) to the Recycle Bin: {r:?}", paths.len());
                    if let Some(op) = r.filter(|o| !o.is_empty()) {
                        post(Ev::FileDone { op });
                    }
                    post(Ev::Rescan);
                }
                Req::Delete { paths, recycle: false } => {
                    // Deleted for good: not undoable.
                    let r = fileops::delete_logged(&paths, false, cfg.owner, cfg.no_ui);
                    log::info!("delete {} item(s) for good: {r:?}", paths.len());
                    post(Ev::Rescan);
                }
                Req::UndoFiles { op, undo } => {
                    // Always answered (None: nothing went through), so the history moves on or
                    // offers the same step again.
                    let done = undo_files(&op, undo, cfg.owner, cfg.no_ui);
                    post(Ev::FileStepDone { op: done, undo });
                    post(Ev::Rescan);
                }
                Req::Paste { folder, link, pt } => {
                    let r = view::paste(folder.as_deref(), cfg.owner, link, pt);
                    log::info!("paste link={link}: {r:?}");
                    post(Ev::Rescan);
                }
                Req::Clipboard { paths, cut } => {
                    let r = view::set_clipboard(&paths, cut);
                    log::info!("clipboard {} item(s) cut={cut}: {r:?}", paths.len());
                }
                Req::IconSize { dip, px, explorer } => {
                    if explorer {
                        let r = view::set_desktop_icon_size(dip);
                        log::info!("desktop icon size {dip}: {r:?}");
                    } else {
                        log::info!("icon size {dip} (not the real desktop: Explorer's left as it is)");
                    }
                    ic = IconCache {
                        px,
                        thumb_exts: HashMap::new(),
                        by_sys: HashMap::new(),
                        overlays: HashMap::new(),
                        shield: None,
                        next: ic.next,
                        prefetch: HashMap::new(),
                    };
                    cache.clear();
                    sized.clear();
                    // The next scan replaces every icon (as a first scan does).
                    first = true;
                    batch.push(Req::Scan);
                }
                Req::IconsAt { px, items } => {
                    // One cache per size; icon ids come from the one counter.
                    let c = sized.entry(px).or_insert_with(|| IconCache {
                        px,
                        thumb_exts: HashMap::new(),
                        by_sys: HashMap::new(),
                        overlays: HashMap::new(),
                        shield: None,
                        next: ic.next,
                        prefetch: HashMap::new(),
                    });
                    c.next = c.next.max(ic.next);
                    let mut fresh = Vec::new();
                    let mut map = Vec::with_capacity(items.len());
                    c.prefetch_images(&items.iter().map(|(_, p, a)| (p.as_str(), *a)).collect::<Vec<_>>());
                    for (base, parsing, attrs) in items {
                        let sys = icons::sys_index(&parsing);
                        map.push((base, c.get_with(&parsing, attrs, sys, &mut fresh)));
                    }
                    c.prefetch.clear();
                    ic.next = c.next;
                    post(Ev::IconsAt { px, map, icons: fresh });
                }
                Req::InfoTip { key, path } => {
                    let text = icons::info_tip(&path).unwrap_or_default();
                    post(Ev::InfoTip { key, text });
                }
                Req::DropOnItem { target, paths, copy } => {
                    // A file-system folder: Windows' copy engine directly (verified to move; the folder's
                    // drop target returned S_OK without moving when driven from this worker).
                    // Programs, archives and other drop targets: the item's own drop target.
                    let r = if std::path::Path::new(&target).is_dir() {
                        let r = fileops::transfer_logged(&paths, &target, copy, cfg.owner, cfg.no_ui);
                        if let Ok(d) = &r {
                            let v = moves(d);
                            if !v.is_empty() {
                                post(Ev::FileDone { op: if copy { FileOp::Copied(v) } else { FileOp::Moved(v) } });
                            }
                        }
                        r.map(|_| ())
                    } else {
                        view::drop_on_item(&target, &paths, copy)
                    };
                    log::info!("drop {} item(s) on {target}: {r:?}", paths.len());
                    post(Ev::Rescan);
                }
            }
        }
    }
}
