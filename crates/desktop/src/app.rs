//! The core's UI thread: windows, state, input, menus, boxes, persistence (ARCHITECTURE §2–§10).

use crate::events::{self, Ev, WM_APP_DRAW, WM_APP_EVENTS, WM_APP_RENAME_END};
use crate::textedit::TextEdit;
use crate::worker::{self, Req};
use crate::{Args, Paths};
use deskboxes_core::layout::{self, History};
use deskboxes_core::persist;
use deskboxes_core::selection::Selection;
use deskboxes_core::snapshot::{IconImage, Snapshot};
use deskboxes_core::*;
use deskboxes_i18n::{category_name, color_name, settings_strings, t, t as t_, Lang, BOX_COLORS, S};
use deskboxes_render::{Bitmap, Gpu, Surface, TextLayout};
use deskboxes_shell::{hide, items, reg, wallpaper};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::Instant;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{EM_GETSEL, EM_REPLACESEL, EM_SETSEL};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const T_HOST: usize = 1;
const T_SAVE: usize = 2;
const T_FS: usize = 3;
const T_EXIT: usize = 4;
const T_HOST_RETRY: usize = 5;
/// Prefetch the selection's menu shortly after the selection settles (Menu key, Shift+F10).
const T_PREFETCH: usize = 6;
/// Prefetch the menu of an unselected item the pointer rests on (a right-click selects it).
const T_HOVER: usize = 7;
/// Caret blink while a box title is edited in place.
pub const T_CARET: usize = 8;
/// Open a rolled-up box after the pointer rested on it (delay mode, or while dragging icons).
const T_REVEAL: usize = 10;
/// While boxes are open on hover: roll them up again once the pointer has left.
const T_ROLLCHECK: usize = 11;
/// Pointing at a tab (or dragging icons over it) for a moment opens it.
const T_TABHOVER: usize = 12;
/// The pointer left a box title (the + button goes).
const T_HDRCHECK: usize = 13;
/// The pointer rested on an icon: show its tooltip.
const T_TIP: usize = 14;
/// The pointer rested on a title button (search, +): its tooltip.
const T_BTNTIP: usize = 15;
/// Boxes above windows: the screen corner and whether another app came to the front.
pub const T_ABOVE: usize = 16;
/// Hover highlights fading in, s.
pub const HOVER_FADE_S: f32 = 0.12;
/// The search button opening into a field (and closing), s.
pub const SEARCH_EXPAND_S: f32 = 0.14;
/// A box gliding to where it was scrolled, s.
const SCROLL_S: f32 = 0.16;
/// A moved box's shadow fading in, s.
pub const LIFT_S: f32 = 0.14;
/// A new tab growing in, s.
const TAB_GROW_S: f32 = 0.18;
/// Tooltip fade-in, s.
pub const TIP_FADE_S: f32 = 0.12;
/// TrackMouseEvent's notification (winuser.h).
const WM_MOUSELEAVE: u32 = 0x02A3;
/// Space between tabs in a box title.
pub const TAB_GAP: f32 = 4.0;
/// Roll-up / roll-down animation length, s.
const ROLL_ANIM_S: f32 = 0.16;
/// Icons stepping aside for a box (and back), s.
pub const GLIDE_S: f32 = 0.18;
/// A deleted icon shrinking and fading out, s.
pub const VANISH_S: f32 = 0.22;
/// A box gliding the last few DIPs when it snaps to an edge (or lets go of one), s.
const SNAP_GLIDE_S: f32 = 0.09;

/// An icon that has just disappeared from the desktop (deleted, moved away), drawn fading out.
pub struct Vanish {
    pub icon: Option<deskboxes_render::Bitmap>,
    pub label: Option<(deskboxes_render::TextLayout, Option<deskboxes_render::Bitmap>)>,
    pub icon_rect: Rect,
    pub label_at: Pt,
    pub in_box: Option<u32>,
    pub t0: Instant,
}
/// A tab opening: the highlight slides over and the icons fade in, s.
pub const TAB_ANIM_S: f32 = 0.16;
/// Pointer rest before a hover prefetch, ms.
const HOVER_PREFETCH_MS: u32 = 150;

static HEADER_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x41F0_0000); // 30.0
static RADIUS_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x4100_0000); // 8.0
/// A box's title bar height (follows the title text size).
pub fn header_h() -> f32 {
    f32::from_bits(HEADER_BITS.load(std::sync::atomic::Ordering::Relaxed))
}
/// A box's corner radius.
pub fn radius() -> f32 {
    f32::from_bits(RADIUS_BITS.load(std::sync::atomic::Ordering::Relaxed))
}
static ACCENT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The Windows accent color (Settings › Personalization › Colors), read once and again after a
/// settings change; the classic highlight color if there is none.
pub fn accent() -> (u8, u8, u8) {
    let v = ACCENT.load(std::sync::atomic::Ordering::Relaxed);
    if v != 0 {
        return ((v >> 16) as u8, (v >> 8) as u8, v as u8);
    }
    let c = match reg::get_dword(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent", "AccentColorMenu") {
        // 0xAABBGGRR
        Some(x) => (x as u8, (x >> 8) as u8, (x >> 16) as u8),
        None => {
            let x = unsafe { GetSysColor(COLOR_HIGHLIGHT) };
            ((x & 0xff) as u8, ((x >> 8) & 0xff) as u8, ((x >> 16) & 0xff) as u8)
        }
    };
    ACCENT.store(0x0100_0000 | (c.0 as u32) << 16 | (c.1 as u32) << 8 | c.2 as u32, std::sync::atomic::Ordering::Relaxed);
    c
}

fn reset_accent() {
    ACCENT.store(0, std::sync::atomic::Ordering::Relaxed);
}

fn set_look_metrics(l: &deskboxes_core::model::LookSettings) {
    let h = (l.title_size.clamp(9.0, 24.0) * 2.3).round().clamp(24.0, 52.0);
    HEADER_BITS.store(h.to_bits(), std::sync::atomic::Ordering::Relaxed);
    RADIUS_BITS.store(l.radius.clamp(0.0, 20.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
}
pub const PAD: f32 = 6.0;
pub const EDGE: f32 = 10.0;


#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Item {
        key: ItemKey,
        in_box: Option<u32>,
    },
    /// A box title: `tab` = the tab under the pointer (boxes with tabs), `add` = the + button.
    Header {
        id: u32,
        tab: Option<usize>,
        add: bool,
    },
    Edge {
        id: u32,
        l: bool,
        t: bool,
        r: bool,
        b: bool,
    },
    BoxBody {
        id: u32,
    },
    Empty,
}

#[derive(Clone, Debug)]
pub enum Drag {
    None,
    Pending {
        start: Pt,
        hit: Hit,
        ctrl: bool,
        shift: bool,
        was_selected: bool,
    },
    Rubber {
        start: Pt,
        cur: Pt,
        base: BTreeSet<ItemKey>,
        ctrl: bool,
        /// Where it started: a box (selects only that box's icons) or the desktop (only loose
        /// icons).
        scope: Option<u32>,
    },
    Items {
        keys: Vec<ItemKey>,
        start: Pt,
        cur: Pt,
    },
    MoveBox {
        id: u32,
        start: Pt,
        orig: Rect,
        cur: Pt,
    },
    Resize {
        id: u32,
        start: Pt,
        orig: Rect,
        l: bool,
        t: bool,
        r: bool,
        b: bool,
    },
    RightRubber {
        start: Pt,
        cur: Pt,
    },
    /// Dragging along a box's A–Z index.
    Scrub {
        id: u32,
    },
    /// A tab dragged by its title: reordered in its box, moved to another box, or torn off.
    Tab {
        id: u32,
        tab: usize,
        start: Pt,
        cur: Pt,
    },
}

/// Where things are in a box title: one rect per tab (a single tab: its centered title) and
/// the + (add a tab) button.
pub struct HeaderLayout {
    pub tabs: Vec<Rect>,
    pub add: Rect,
}

#[derive(Clone, Debug)]
pub struct MenuCtx {
    pub at: Pt,
    pub hit: Hit,
    pub new_rect: Option<Rect>,
}

pub struct Rename {
    pub edit: HWND,
    pub target: RenameTarget,
}

/// Screen edges a box touches.
#[derive(Clone, Copy, Debug, Default)]
pub struct Dock {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

/// A box (tab) title being edited in place, drawn in the header by us.
pub struct BoxEdit {
    pub id: u32,
    pub tab: usize,
    pub ed: TextEdit,
    pub caret_on: bool,
    /// Mouse button held in the field: drag selects.
    pub selecting: bool,
    /// First half of a surrogate pair typed (WM_CHAR delivers them one at a time).
    pub hi: Option<u16>,
}

/// Box background blur presets for the box menu (DIPs).
const BLUR_PRESETS: [(i32, S); 4] = [(0, S::BlurNone), (15, S::BlurLight), (30, S::BlurMedium), (60, S::BlurStrong)];
/// Box transparency presets for the box menu (percent).
const TRANSPARENCY_PRESETS: [u32; 6] = [0, 20, 40, 60, 80, 100];

#[derive(Clone, Debug)]
pub enum RenameTarget {
    Item(ItemKey),
    Box { id: u32, tab: usize },
}

#[derive(Default, serde::Serialize)]
pub struct Stats {
    pub first_paint_ms: f64,
    pub snapshot_items: usize,
    pub hide_ms: f64,
    pub reconcile_ms: f64,
    pub scan_ms: f64,
    pub items: usize,
    pub host_ready_ms: f64,
    pub working_set_mb: f64,
    pub private_mb: f64,
    pub draw_ms_last: f64,
    pub draw_count: usize,
    pub draw_ms_p50: f64,
    pub draw_ms_p95: f64,
    pub draw_ms_max: f64,
    /// Entries the last scan had to resolve through the shell (the rest came from the cache).
    pub scan_resolved: usize,
    /// Frames drawn while a rubber band or an item drag was in progress.
    pub drag_frames: usize,
    pub drag_draw_ms_p50: f64,
    pub drag_draw_ms_p95: f64,
    pub drag_draw_ms_max: f64,
    /// Time between consecutive frames of one drag (input-driven: bounded below by the input rate).
    pub drag_interval_ms_p50: f64,
    pub drag_interval_ms_p95: f64,
    pub drag_interval_ms_max: f64,
}

pub struct App {
    pub args: Args,
    pub paths: Paths,
    pub settings: Settings,
    pub lang: Lang,
    pub t0: Instant,
    pub msg_hwnd: HWND,
    pub hwnd: HWND,
    pub desktop_mode: bool,
    pub gpu: Gpu,
    pub surf: Option<Surface>,
    pub scale: f32,
    /// Monitor rect in screen px and the surface's screen origin in px.
    pub mon: RECT,
    pub origin_px: POINT,
    /// Size of the surface in DIPs, and the work area in surface DIPs.
    pub size_dip: (f32, f32),
    pub work: Rect,
    pub grid: Grid,
    pub cell_px: (u32, u32),
    pub icon_dip: f32,
    pub icon_px: u32,
    pub items: HashMap<ItemKey, Item>,
    pub bitmaps: HashMap<u32, Bitmap>,
    pub icon_imgs: HashMap<u32, IconImage>,
    pub layout: MonitorLayout,
    pub hist: History,
    pub topology: String,
    pub sel: Selection,
    pub hover: Option<ItemKey>,
    pub drag: Drag,
    pub scroll: HashMap<u32, f32>,
    pub labels: HashMap<(ItemKey, bool, i32), TextLayout>,
    /// Our icons being dragged through OLE (out of our window and maybe back): keys and start.
    pub own_drag: Option<(Vec<ItemKey>, Pt)>,
    /// Desktop labels pre-rendered with their shadow (same keys as `labels`).
    pub label_bmps: HashMap<(ItemKey, bool, i32), deskboxes_render::Bitmap>,
    pub label_tf: windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
    pub title_tf: windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
    pub title_tf_bold: windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
    pub font: (String, f32),
    pub line_h: f32,
    /// Line height of box titles (their text size follows the settings).
    pub title_line_h: f32,
    pub wall: Option<Bitmap>,
    pub backdrops: HashMap<i32, Bitmap>,
    pub backdrop_down: u32,
    pub light: bool,
    pub worker: worker::WorkerTx,
    pub host: Option<deskboxes_ipc::HostClient>,
    pub host_child: Option<std::process::Child>,
    pub host_starts: Vec<Instant>,
    pub menu: Option<(u32, MenuCtx)>,
    pub rename: Option<Rename>,
    pub box_edit: Option<BoxEdit>,
    /// What the settings window last got (boxes, styles, default style, language): resent when
    /// the layout changes it, so an open window stays current.
    pub settings_sent: String,
    /// Rolled-up boxes opened because the pointer is on them.
    pub revealed: std::collections::HashSet<u32>,
    /// How far open each box is drawn, 0 (title only) .. 1 (animated toward its target).
    pub open_t: HashMap<u32, f32>,
    pub anim_last: Option<Instant>,
    pub pending_reveal: Option<u32>,
    /// A box rolled up by a double-click on its title: it stays rolled up while the pointer is
    /// still on it (so the roll-up shows), and opens on hover again once the pointer has left.
    pub no_reveal: Option<u32>,
    /// Boxes gliding to a new scroll position: where they were drawn, since.
    pub scroll_from: HashMap<u32, (f32, Instant)>,
    /// When the box being moved was picked up (its shadow fades in).
    pub move_t0: Instant,
    /// The title button under the pointer: box, 1 = search, 2 = +, since.
    pub hdr_btn: Option<(u32, u8, Instant)>,
    /// The search button closing again: box, how far open it was, since.
    pub search_btn_out: Option<(u32, f32, Instant)>,
    /// When the pointer last moved onto another part of a title (hover highlights fade in).
    pub hdr_hover_t0: Instant,
    /// A tab just added, growing in: box, tab, since.
    pub tab_grow: Option<(u32, usize, Instant)>,
    /// A title button's tooltip: text, where, since.
    pub btn_tip: Option<(String, Pt, Instant)>,
    pub vanishing: Vec<Vanish>,
    /// A moved box gliding into a snap: box, offset it is drawn at (from its rect) when the glide
    /// started, start; and the snap correction of the last move.
    pub snap_glide: Option<(u32, Pt, Instant)>,
    pub snap_delta: Pt,
    /// The box title under the pointer: (box, tab, on the + button).
    pub hover_hdr: Option<(u32, Option<usize>, bool)>,
    /// A tab that opens if the pointer stays on it.
    pub pending_tab: Option<(u32, usize)>,
    /// Page the settings window opens on next ("rules", "boxes", ...).
    pub settings_page: Option<&'static str>,
    pub pending_drop: Option<(Instant, Pt, Option<u32>)>,
    pub known_before_drop: BTreeSet<ItemKey>,
    pub stats: Stats,
    pub scanned_once: bool,
    pub quitting: bool,
    /// No snapshot to show yet (first run): keep Explorer's icons until the first scan is drawn,
    /// so the desktop is never empty while the cold scan runs.
    pub hide_after_scan: bool,
    /// Every frame's draw time (ms), for the stats and the diagnostics overlay.
    pub draw_times: Vec<f32>,
    pub frame_marks: std::collections::VecDeque<Instant>,
    pub diag: bool,
    pub drag_draw_times: Vec<f32>,
    pub drag_intervals: Vec<f32>,
    pub last_drag_frame: Option<Instant>,
    pub taskbar_created: u32,
    /// Loose icons as they would settle with the box being moved or resized where it is now
    /// (they step aside live, and back).
    pub preview_loose: Option<std::collections::BTreeMap<ItemKey, Cell>>,
    /// Loose icons gliding to a new cell: (where it started, when).
    pub glide: HashMap<ItemKey, (Pt, Instant)>,
    /// Each loose icon's cell position as last drawn (a change starts a glide).
    pub glide_at: HashMap<ItemKey, Pt>,
    /// Icons placed by the user's own drop: no glide (they go exactly where dropped).
    pub no_glide: std::collections::HashSet<ItemKey>,
    /// A tab just opened: (box, the active tab's rect before, when).
    pub tab_anim: Option<(u32, Rect, Instant)>,
    /// A roll animation is running.
    pub anim_roll: bool,
    /// The tooltip of the icon under the pointer: (icon, full name, Explorer's text, where, since).
    pub tip: Option<(ItemKey, String, String, Pt, Instant)>,
    pub tip_tf: windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
    pub tip_tf_bold: windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
    /// Segoe Fluent Icons, for small glyphs drawn in box titles (the lock).
    pub glyph_tf: windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
    /// The search pill / box search field.
    pub search: crate::search::Search,
    /// The custom color window is open for: box, tab (None: the box), the color it started from.
    pub color_pick: Option<(u32, Option<usize>, Option<Rgba>)>,
    /// The letter an A–Z box is at (the last one jumped or scrolled to): box, letter.
    pub az_at: Option<(u32, char)>,
    /// Blending icons: the box the pointer is on, the one it left, since.
    pub blend_hover: (Option<u32>, Option<u32>, Instant),
    /// The desktop has the keyboard focus (in use), since.
    pub desk_active: (bool, Instant),
    /// Everything hidden with a double-click on the desktop (Settings › Hide all), since.
    pub tucked: (bool, Instant),
    /// Boxes above windows: the window showing them, the desktop surface window, since.
    pub above: Option<(HWND, HWND, Instant)>,
    /// Our window had the front while the boxes were up (so another app taking it puts them back).
    pub above_had_front: bool,
    /// Windows gave us the shortcut.
    pub above_hotkey_ok: bool,
    /// The pointer has been resting in the hot corner since.
    pub corner_since: Option<Instant>,
    /// The folders folder boxes show, as last sent to the worker.
    pub folders_sent: Vec<String>,
    /// The desktop's own folders (a folder box never shows one).
    pub desk_dirs: Vec<String>,
    /// The folder box whose back arrow the pointer is on.
    pub folder_back_hover: Option<u32>,
    /// The title button the pointer is on: box, 0 roll / 1 menu.
    pub title_btn_hover: Option<(u32, u8)>,
    /// "Copy box style": the style to paste onto another box.
    /// A box's look, copied to paste onto other boxes: style, icon tint, blending.
    pub style_clip: Option<(BoxStyle, String, String)>,
    /// The letter bar's accent line gliding to another letter: box, where it was, since.
    pub az_ind: Option<(u32, f32, Instant)>,
    pub az_tf: windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
    pub az_bar_tf: windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
    pub az_bar_on_tf: windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
    pub search_tf: windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
    /// The pointer leaving our window is being tracked (WM_MOUSELEAVE).
    pub tracking_leave: bool,
    /// Selection (and focus) the last selection prefetch was scheduled for.
    pub prefetched_sel: (BTreeSet<ItemKey>, Option<ItemKey>),
    /// (items, shift) of the prefetch the host holds, as far as we know: a show uses it up.
    pub host_prefetch: Option<(Vec<String>, bool)>,
    /// The worker reuses the snapshot's items and icon ids.
    pub seeded: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Work to run after the App borrow is released (modal loops re-enter the window procedure).
pub enum After {
    None,
    /// Paths, and the drag image (see `ghost_pixels`).
    OleDrag(Vec<String>, Option<(u32, u32, Vec<u8>, (i32, i32))>),
}

pub fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn lparam_pt(l: LPARAM) -> (i32, i32) {
    ((l.0 & 0xffff) as i16 as i32, ((l.0 >> 16) & 0xffff) as i16 as i32)
}

/// Ease-out cubic.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Where a gliding icon is at `now` on its way to `to` (no glide: at `to`).
fn glide_pos(g: Option<&(Pt, Instant)>, to: Pt, now: Instant) -> Pt {
    match g {
        Some((from, t0)) => {
            let t = ease_out(now.duration_since(*t0).as_secs_f32() / GLIDE_S);
            Pt {
                x: from.x + (to.x - from.x) * t,
                y: from.y + (to.y - from.y) * t,
            }
        }
        None => to,
    }
}

/// DirectWrite weight for a title weight name.
pub fn title_weight(w: &str) -> i32 {
    match w {
        "light" => 300,
        "normal" => 400,
        "bold" => 700,
        _ => 600,
    }
}

pub fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

pub fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default()
}

fn find_host(a: &Args) -> Option<PathBuf> {
    if let Some(h) = &a.host {
        return Some(h.clone());
    }
    let d = exe_dir();
    let mut cands = vec![d.join("deskboxes-shellhost.exe")];
    // Development layout: target\<profile>\deskboxes.exe and out\<Config>\shellhost\.
    if let Some(root) = d.parent().and_then(|p| p.parent()) {
        let cfg = if d.ends_with("release") { "Release" } else { "Debug" };
        cands.push(root.join("out").join(cfg).join("shellhost").join("deskboxes-shellhost.exe"));
    }
    cands.into_iter().find(|p| p.exists())
}

fn icon_title_font() -> (String, f32) {
    unsafe {
        let mut lf = LOGFONTW::default();
        if SystemParametersInfoForDpi(SPI_GETICONTITLELOGFONT.0, std::mem::size_of::<LOGFONTW>() as u32, Some(&mut lf as *mut _ as _), 0, 96).is_ok() {
            let n = lf.lfFaceName.iter().position(|&c| c == 0).unwrap_or(32);
            let face = String::from_utf16_lossy(&lf.lfFaceName[..n]);
            return (face, (lf.lfHeight.abs() as f32).max(8.0));
        }
        log::warn!("SPI_GETICONTITLELOGFONT failed; using Segoe UI 12 DIP");
        ("Segoe UI".into(), 12.0)
    }
}

unsafe fn primary_monitor() -> (RECT, RECT) {
    let hm = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let _ = GetMonitorInfoW(hm, &mut mi);
    (mi.rcMonitor, mi.rcWork)
}

fn class_name(desktop: bool) -> PCWSTR {
    if desktop {
        w!("DeskboxesSurface")
    } else {
        w!("DeskboxesTestWindow")
    }
}

unsafe fn register_classes() {
    let hinst = GetModuleHandleW(None).unwrap_or_default();
    for (name, style) in [(w!("DeskboxesSurface"), CS_DBLCLKS), (w!("DeskboxesTestWindow"), CS_DBLCLKS), (w!("DeskboxesAbove"), CS_DBLCLKS), (w!("DeskboxesCore"), WNDCLASS_STYLES(0))] {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: name,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

/// Create the surface window: a child of Progman covering the primary monitor (desktop mode),
/// or a normal top-level window (fixture `--window` mode).
unsafe fn create_surface_window(desktop: bool, mon: RECT) -> windows::core::Result<(HWND, POINT)> {
    let hinst = GetModuleHandleW(None)?;
    if desktop {
        let progman = hide::progman();
        let mut pt = POINT { x: mon.left, y: mon.top };
        let _ = ScreenToClient(progman, &mut pt);
        let h = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            class_name(true),
            w!("Deskboxes"),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            pt.x,
            pt.y,
            mon.right - mon.left,
            mon.bottom - mon.top,
            Some(progman),
            None,
            Some(hinst.into()),
            None,
        )?;
        let _ = SetWindowPos(h, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        Ok((h, POINT { x: mon.left, y: mon.top }))
    } else {
        let (w, h) = (1600, 1000);
        // The harness brings it to the foreground before screenshots and input. Not topmost:
        // the shell host's menus must open above it, as they do above the desktop.
        let hw = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            class_name(false),
            w!("Deskboxes (fixture)"),
            WS_POPUP | WS_VISIBLE | WS_BORDER,
            mon.left + 80,
            mon.top + 60,
            w,
            h,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;
        let mut pt = POINT::default();
        let _ = ClientToScreen(hw, &mut pt);
        Ok((hw, pt))
    }
}

/// (private bytes, working set) of this process in MB.
pub fn mem_mb() -> (f64, f64) {
    unsafe {
        use windows::Win32::System::ProcessStatus::*;
        let mut pmc = PROCESS_MEMORY_COUNTERS_EX::default();
        if GetProcessMemoryInfo(
            windows::Win32::System::Threading::GetCurrentProcess(),
            &mut pmc as *mut _ as _,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
        .is_ok()
        {
            return (pmc.PrivateUsage as f64 / 1048576.0, pmc.WorkingSetSize as f64 / 1048576.0);
        }
    }
    (0.0, 0.0)
}

pub fn run(args: Args, paths: Paths, t0: Instant) -> i32 {
    unsafe { run_inner(args, paths, t0) }
}

unsafe fn run_inner(args: Args, paths: Paths, t0: Instant) -> i32 {
    let mut settings = persist::load_settings(&paths.settings);
    crate::az::set_bar(settings.az.letter_bar);
    if args.enable && !settings.enabled {
        // Started by the user on purpose (Start menu, setup): on again.
        log::info!("--enable: Deskboxes was disabled; turning it back on");
        settings.enabled = true;
        let _ = persist::save_settings(&paths.settings, &settings);
    }
    // A downloaded update installs now, before anything is shown; the installer starts
    // Deskboxes again (unless it is turned off).
    if crate::update::install_pending(&paths.update, settings.enabled && args.desktop_dir.is_none() && !args.window) {
        return 0;
    }
    if !settings.enabled {
        log::info!("Deskboxes is disabled in settings: restoring native icons and exiting");
        hide::restore();
        return 0;
    }
    let desktop_mode = !args.window;
    let fixture = args.desktop_dir.clone();
    let lang = Lang::pick(&settings.language, windows::Win32::Globalization::GetUserDefaultUILanguage());
    register_classes();
    let hinst = GetModuleHandleW(None).unwrap_or_default();
    let msg_hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, w!("DeskboxesCore"), w!("Deskboxes core"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst.into()), None).expect("message window");
    events::set_target(msg_hwnd);
    crate::anim::start(msg_hwnd);

    // 1. Snapshot: one read, no shell calls before the first frame.
    let snap = std::fs::read(&paths.snapshot).ok().and_then(|b| Snapshot::decode(&b));
    let source_id = match &fixture {
        Some(d) => format!("fixture:{}", d.display()),
        None => "desktop".to_string(),
    };
    let snap = snap.filter(|s| s.source_id == source_id);

    // 2. Devices and the surface.
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            log::error!("no graphics device: {e}; native icons stay visible");
            hide::restore();
            return 3;
        }
    };
    log::info!("MEM after graphics devices: private/working set {:.1?} MB", mem_mb());
    let (mon_full, work_px) = primary_monitor();
    let (hwnd, origin_px) = match create_surface_window(desktop_mode, mon_full) {
        Ok(x) => x,
        Err(e) => {
            log::error!("cannot create the surface window: {e}");
            hide::restore();
            return 3;
        }
    };
    let dpi = GetDpiForWindow(hwnd).max(96);
    let scale = dpi as f32 / 96.0;
    let mut cr = RECT::default();
    let _ = GetClientRect(hwnd, &mut cr);
    let (wpx, hpx) = ((cr.right - cr.left) as u32, (cr.bottom - cr.top) as u32);
    let surf = Surface::new(&gpu, hwnd, wpx, hpx).ok();
    let size_dip = (wpx as f32 / scale, hpx as f32 / scale);
    let work = if desktop_mode {
        Rect::new(
            (work_px.left - mon_full.left) as f32 / scale,
            (work_px.top - mon_full.top) as f32 / scale,
            (work_px.right - work_px.left) as f32 / scale,
            (work_px.bottom - work_px.top) as f32 / scale,
        )
    } else {
        Rect::new(0.0, 0.0, size_dip.0, size_dip.1)
    };
    let icon_dip = reg::desktop_icon_size() as f32;
    let icon_px = (icon_dip * scale).round() as u32;
    let cell_px = snap
        .as_ref()
        .filter(|s| s.cell_w > 0)
        .map(|s| (s.cell_w, s.cell_h))
        .unwrap_or(((icon_dip * 1.6 * scale) as u32, (icon_dip * 2.1 * scale) as u32));
    let grid = Grid::new(work, cell_px.0 as f32 / scale, cell_px.1 as f32 / scale);
    let font = icon_title_font();
    let label_tf = gpu.text_format(&font.0, font.1, false).expect("text format");
    // Explorer's label lines are 1.25 em apart (15 px for the 12 px default font).
    unsafe {
        use windows::Win32::Graphics::DirectWrite::DWRITE_LINE_SPACING_METHOD_UNIFORM;
        let sp = (font.1 * 1.25 * scale).round() / scale;
        let _ = label_tf.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, sp, sp * 0.8);
    }
    set_look_metrics(&settings.look);
    let title_fam = if settings.look.title_font.is_empty() { font.0.clone() } else { settings.look.title_font.clone() };
    let tw = title_weight(&settings.look.title_weight);
    let title_tf = gpu.text_format_weight(&title_fam, settings.look.title_size, if tw >= 600 { 400 } else { (tw - 100).max(300) }).expect("text format");
    let title_tf_bold = gpu.text_format_weight(&title_fam, settings.look.title_size, tw).expect("text format");
    let title_line_h = (settings.look.title_size * 1.4).ceil();
    let tip_tf = gpu.text_format("Segoe UI", 12.0, false).expect("text format");
    let tip_tf_bold = gpu.text_format("Segoe UI", 12.0, true).expect("text format");
    let search_tf = gpu.text_format("Segoe UI", 13.0, false).expect("text format");
    let az_tf = gpu.text_format("Segoe UI", 10.5, true).expect("text format");
    let az_bar_tf = gpu.text_format("Segoe UI", 11.5, false).expect("text format");
    let az_bar_on_tf = gpu.text_format("Segoe UI", 11.5, true).expect("text format");
    let glyph_tf = gpu.text_format("Segoe Fluent Icons", 12.0, false).or_else(|_| gpu.text_format("Segoe MDL2 Assets", 12.0, false)).expect("text format");
    let line_h = gpu.label(&label_tf, "Ag", 200.0, 100.0, 1, true).map(|l| deskboxes_render::text_height(&l)).unwrap_or(font.1 * 1.33);
    let topology = format!("primary-{}x{}-{}", mon_full.right - mon_full.left, mon_full.bottom - mon_full.top, dpi);
    let lay = persist::load_layout(&paths.layouts, &topology).unwrap_or_default();
    let first_run = !persist::layout_path(&paths.layouts, &topology).exists();
    let seed: Vec<Item> = snap.as_ref().filter(|s| s.icon_px == icon_px).map(|s| s.items.clone()).unwrap_or_default();
    let first_icon_id = snap.as_ref().map(|s| s.icons.len() as u32).unwrap_or(0);
    let seeded = !seed.is_empty();
    let mut dirs = items::desktop_dirs(fixture.as_deref());
    if let (Some(_), Some(p)) = (&fixture, &args.public_dir) {
        dirs.push(items::DirSpec {
            path: p.clone(),
            source: Source::Public,
        });
    }
    let worker = worker::start(worker::Config {
        dirs: dirs.clone(),
        virtual_items: fixture.is_none(),
        icon_px,
        owner: hwnd.0 as isize,
        import_positions: first_run && fixture.is_none(),
        no_ui: args.no_ui_ops,
        seed,
        first_icon_id,
    });
    let light = reg::apps_use_light_theme();
    let mut app = App {
        args,
        paths,
        settings,
        lang,
        t0,
        msg_hwnd,
        hwnd,
        desktop_mode,
        gpu,
        surf,
        scale,
        mon: mon_full,
        origin_px,
        size_dip,
        work,
        grid,
        cell_px,
        icon_dip,
        icon_px,
        items: HashMap::new(),
        bitmaps: HashMap::new(),
        icon_imgs: HashMap::new(),
        layout: lay,
        hist: History::default(),
        topology,
        sel: Selection::default(),
        hover: None,
        drag: Drag::None,
        scroll: HashMap::new(),
        labels: HashMap::new(),
        own_drag: None,
        label_bmps: HashMap::new(),
        label_tf,
        title_tf,
        title_tf_bold,
        font,
        line_h,
        title_line_h,
        wall: None,
        backdrops: HashMap::new(),
        backdrop_down: 4,
        light,
        worker,
        host: None,
        host_child: None,
        host_starts: vec![],
        menu: None,
        rename: None,
        box_edit: None,
        settings_sent: String::new(),
        revealed: Default::default(),
        open_t: HashMap::new(),
        anim_last: None,
        pending_reveal: None,
        no_reveal: None,
        scroll_from: HashMap::new(),
        move_t0: Instant::now(),
        hdr_btn: None,
        search_btn_out: None,
        hdr_hover_t0: Instant::now(),
        tab_grow: None,
        btn_tip: None,
        vanishing: vec![],
        snap_glide: None,
        snap_delta: Pt { x: 0.0, y: 0.0 },
        hover_hdr: None,
        pending_tab: None,
        settings_page: None,
        pending_drop: None,
        known_before_drop: BTreeSet::new(),
        stats: Stats::default(),
        scanned_once: false,
        quitting: false,
        hide_after_scan: false,
        draw_times: Vec::new(),
        frame_marks: std::collections::VecDeque::new(),
        diag: false,
        drag_draw_times: Vec::new(),
        drag_intervals: Vec::new(),
        last_drag_frame: None,
        taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
        preview_loose: None,
        glide: HashMap::new(),
        glide_at: HashMap::new(),
        no_glide: Default::default(),
        tab_anim: None,
        anim_roll: false,
        tip: None,
        tip_tf,
        tip_tf_bold,
        glyph_tf,
        search: Default::default(),
        color_pick: None,
        az_at: None,
        blend_hover: (None, None, Instant::now()),
        desk_active: (false, Instant::now()),
        tucked: (false, Instant::now() - std::time::Duration::from_secs(5)),
        above: None,
        above_had_front: false,
        above_hotkey_ok: true,
        corner_since: None,
        folders_sent: vec![],
        desk_dirs: dirs.iter().map(|d| d.path.display().to_string()).collect(),
        folder_back_hover: None,
        title_btn_hover: None,
        style_clip: None,
        az_ind: None,
        az_tf,
        az_bar_tf,
        az_bar_on_tf,
        search_tf,
        tracking_leave: false,
        prefetched_sel: (BTreeSet::new(), None),
        host_prefetch: None,
        seeded,
    };
    if let Some(s) = snap {
        app.stats.snapshot_items = s.items.len();
        for (i, img) in s.icons.into_iter().enumerate() {
            if let Ok(b) = app.gpu.bitmap(&img) {
                app.bitmaps.insert(i as u32, b);
            }
            app.icon_imgs.insert(i as u32, img);
        }
        for it in s.items {
            app.items.insert(it.key.clone(), it);
        }
    }
    app.diag = app.args.diagnostics;
    app.load_wallpaper();
    app.draw();
    app.stats.first_paint_ms = t0.elapsed().as_secs_f64() * 1000.0;
    log::info!("MEM after first paint: private/working set {:.1?} MB", mem_mb());
    log::info!(
        "PHASE first_paint ms={:.1} snapshot_items={} desktop_mode={desktop_mode} dpi={dpi}",
        app.stats.first_paint_ms,
        app.stats.snapshot_items
    );

    // 3. Hide native icons only after our first frame is on screen, and only if that frame shows
    //    the desktop's items (from the snapshot); otherwise after the first scan is drawn.
    if desktop_mode && !app.args.no_hide && app.stats.snapshot_items == 0 {
        app.hide_after_scan = true;
        log::info!("no snapshot: native icons stay until the first scan is drawn");
    } else if desktop_mode && !app.args.no_hide {
        let th = Instant::now();
        let s = hide::set_hidden(true);
        app.stats.hide_ms = th.elapsed().as_secs_f64() * 1000.0;
        log::info!("PHASE hide_icons ms={:.1} state={s:?}", app.stats.hide_ms);
    }
    app.folder_sync();
    let _ = app.worker.send(Req::Scan);
    deskboxes_shell::watch::watch(dirs.iter().map(|d| d.path.clone()).collect(), || events::post(Ev::Rescan));
    if !app.args.no_host {
        SetTimer(Some(msg_hwnd), T_HOST, 300, None);
    }
    let _ = windows::Win32::System::Ole::RegisterDragDrop(hwnd, &crate::droptarget::DropTarget::create(hwnd));
    app.register_above_hotkey();
    app.above_poll();
    crate::update::start(app.paths.update.clone(), app.settings.update.auto);
    APP.with(|a| *a.borrow_mut() = Some(app));

    let mut msg = MSG::default();
    'pump: while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        if msg.message == WM_APP_DRAW {
            // Input first: posted frame requests come ahead of the pointer and keys in the queue,
            // so while an animation runs at the refresh rate they would starve them (a drag
            // would only move when the animation ends). Handle what is waiting, then draw.
            let mut m = MSG::default();
            while PeekMessageW(&mut m, None, 0, 0, PEEK_MESSAGE_REMOVE_TYPE(PM_REMOVE.0 | PM_QS_INPUT.0)).as_bool() {
                if m.message == WM_QUIT {
                    msg = m;
                    break 'pump;
                }
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    let code = msg.wParam.0 as i32;
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            app.shutdown();
        }
    });
    code
}

impl App {
    pub fn load_wallpaper(&mut self) {
        let mons = wallpaper::monitors();
        let m = mons.iter().find(|m| m.rect.left == self.mon.left && m.rect.top == self.mon.top).or(mons.first()).cloned();
        let (mw, mh) = ((self.mon.right - self.mon.left) as u32, (self.mon.bottom - self.mon.top) as u32);
        let (path, pos, color) = m.map(|m| (m.path, m.position, m.color)).unwrap_or_default();
        let bg = deskboxes_render::rgba((color & 0xff) as u8, ((color >> 8) & 0xff) as u8, ((color >> 16) & 0xff) as u8, 1.0);
        // We draw the wallpaper ourselves in both modes. On the desktop, Windows 11 (24H2+) paints
        // the wallpaper in a separate child of Progman that our full-screen surface covers, so it is
        // not visible through our transparent pixels (seen on 26300: a black desktop, 2026-11-19).
        // Decoded at its on-screen size, never larger.
        let img = if path.is_empty() {
            None
        } else {
            let scale = self
                .gpu
                .image_size(&path)
                .map(|(iw, ih)| {
                    let r = deskboxes_render::wallpaper_rect(pos, iw as f32, ih as f32, mw as f32, mh as f32);
                    ((r.right - r.left) / iw as f32).max((r.bottom - r.top) / ih as f32)
                })
                .unwrap_or(1.0)
                .min(1.0);
            self.gpu.load_image_scaled(&path, scale).map_err(|e| log::warn!("wallpaper {path}: {e}")).ok()
        };
        self.wall = self
            .gpu
            .offscreen(mw, mh, |dc| unsafe {
                dc.Clear(Some(&bg));
                if let Some((b, (ow, oh))) = &img {
                    let r = deskboxes_render::wallpaper_rect(pos, *ow as f32, *oh as f32, mw as f32, mh as f32);
                    deskboxes_render::draw_bitmap(dc, b, r, 1.0);
                }
                Ok(())
            })
            .map_err(|e| log::warn!("wallpaper surface: {e}"))
            .ok();
        self.backdrops.clear();
        let mut blurs: Vec<i32> = self.layout.boxes.iter().map(|b| b.style.blur as i32).collect();
        blurs.push(self.settings.default_style.blur as i32);
        blurs.sort();
        blurs.dedup();
        for b in blurs {
            match self
                .gpu
                .blurred_wallpaper(img.as_ref().map(|(b, o)| (b, *o)), bg, pos, mw, mh, self.backdrop_down, b as f32 * self.scale)
            {
                Ok(bm) => {
                    self.backdrops.insert(b, bm);
                }
                Err(e) => log::warn!("backdrop blur {b}: {e}"),
            }
        }
        log::info!("wallpaper '{path}' pos={pos} backdrops={}; MEM private/working set {:.1?} MB", self.backdrops.len(), mem_mb());
    }

    pub fn invalidate(&mut self) {
        crate::anim::request_frame();
    }

    // ---------------------------------------------------------------- geometry

    pub fn box_by_id(&self, id: u32) -> Option<&BoxModel> {
        self.layout.boxes.iter().find(|b| b.id == id)
    }

    /// Where a box's icons are: under its title (and an A–Z box's letter bar).
    pub fn content_rect(b: &BoxModel) -> Rect {
        let top = header_h() + crate::az::bar_h(b);
        Rect::new(b.rect.x, b.rect.y + top, b.rect.w, (b.rect.h - top).max(0.0))
    }

    pub fn box_cols(&self, b: &BoxModel) -> usize {
        (((b.rect.w - 2.0 * PAD) / self.grid.cell_w).floor() as usize).max(1)
    }

    /// Cell rects of the active tab's items of a box (scrolled; may extend past the content rect).
    pub fn box_item_cells(&self, b: &BoxModel) -> Vec<(ItemKey, Rect)> {
        let Some(tab) = b.active() else { return vec![] };
        let cols = self.box_cols(b);
        let c = Self::content_rect(b);
        let used = cols as f32 * self.grid.cell_w;
        let x0 = c.x + (c.w - used) / 2.0;
        let y0 = c.y + 2.0 - self.shown_scroll(b.id);
        self.shown_items(b, tab)
            .into_iter()
            .enumerate()
            .map(|(i, k)| {
                (
                    k.clone(),
                    Rect::new(x0 + (i % cols) as f32 * self.grid.cell_w, y0 + (i / cols) as f32 * self.grid.cell_h, self.grid.cell_w, self.grid.cell_h),
                )
            })
            .collect()
    }

    /// Where items dropped at `p` go in a box: the insertion index among the shown items and
    /// the top-left of the slot (for the insertion marker).
    pub fn box_drop_slot(&self, b: &BoxModel, p: Pt) -> (usize, Pt) {
        let cols = self.box_cols(b);
        let c = Self::content_rect(b);
        let x0 = c.x + (c.w - cols as f32 * self.grid.cell_w) / 2.0;
        let y0 = c.y + 2.0 - self.shown_scroll(b.id);
        let n = b.active().map(|t| t.items.iter().filter(|k| self.items.contains_key(*k)).count()).unwrap_or(0);
        let row = ((p.y - y0) / self.grid.cell_h).floor().max(0.0) as usize;
        let col = (((p.x - x0) / self.grid.cell_w).round().max(0.0) as usize).min(cols);
        let i = (row * cols + col).min(n);
        let (r, cc) = if i == row * cols + col { (row, col) } else { (i / cols, i % cols) };
        (
            i,
            Pt {
                x: x0 + cc as f32 * self.grid.cell_w,
                y: y0 + r as f32 * self.grid.cell_h,
            },
        )
    }

    /// An item the dragged items would be dropped *into* (a folder, a program, an archive): only
    /// while the pointer is on its icon, so dropping next to it in a box just places the items.
    pub fn drop_onto_item(&self, p: Pt, keys: &[ItemKey]) -> Option<ItemKey> {
        let Hit::Item { key, .. } = self.hit_test(p) else { return None };
        if keys.contains(&key) {
            return None;
        }
        let it = self.items.get(&key)?;
        let ext = std::path::Path::new(&it.parsing_name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let accepts = it.is_folder() || matches!(ext.as_str(), "exe" | "lnk" | "zip" | "bat" | "cmd");
        let (_, cell, _) = self.visible_cells().into_iter().find(|(k, _, _)| *k == key)?;
        let (icon, _) = self.item_parts(&key, cell, false);
        (accepts && icon.inflate(6.0).contains(p)).then_some(key)
    }

    /// Box under the pointer for a drop (body, header or one of its items).
    pub fn drop_box(&self, p: Pt) -> Option<u32> {
        match self.hit_test(p) {
            Hit::BoxBody { id } | Hit::Header { id, .. } | Hit::Item { in_box: Some(id), .. } | Hit::Edge { id, .. } => Some(id),
            _ => None,
        }
    }

    /// Where a box is scrolled as drawn now (gliding to its scroll position).
    pub fn shown_scroll(&self, id: u32) -> f32 {
        let to = self.scroll.get(&id).copied().unwrap_or(0.0);
        match self.scroll_from.get(&id) {
            Some((from, t0)) if self.settings.extras.smooth_scroll => from + (to - from) * ease_out(t0.elapsed().as_secs_f32() / SCROLL_S),
            _ => to,
        }
    }

    /// Scroll a box to `s` (gliding there from where it is drawn now, unless turned off).
    pub fn scroll_to(&mut self, id: u32, s: f32) {
        let from = self.shown_scroll(id);
        self.scroll.insert(id, s);
        if self.settings.extras.smooth_scroll && (from - s).abs() > 0.5 {
            self.scroll_from.insert(id, (from, Instant::now()));
            crate::anim::set(true);
        } else {
            self.scroll_from.remove(&id);
        }
        self.invalidate();
    }

    pub fn scroll_animating(&self) -> bool {
        self.scroll_from.values().any(|(_, t0)| t0.elapsed().as_secs_f32() < SCROLL_S)
    }

    pub fn max_scroll(&self, b: &BoxModel) -> f32 {
        let n = b.active().map(|t| t.items.iter().filter(|k| self.items.contains_key(*k)).count()).unwrap_or(0);
        let rows = n.div_ceil(self.box_cols(b)) as f32;
        (rows * self.grid.cell_h + 4.0 - Self::content_rect(b).h).max(0.0)
    }

    /// Every visible item with its cell rect and box, in display order (loose grid in
    /// column-major order, then boxes).
    pub fn visible_cells(&self) -> Vec<(ItemKey, Rect, Option<u32>)> {
        let mut loose: Vec<(&ItemKey, &Cell)> = self.layout.loose.iter().filter(|(k, _)| self.items.contains_key(*k)).collect();
        loose.sort_by_key(|(_, c)| self.grid.index_of(**c));
        let mut v: Vec<(ItemKey, Rect, Option<u32>)> = loose.into_iter().map(|(k, c)| (k.clone(), self.grid.cell_rect(*c), None)).collect();
        for b in &self.layout.boxes {
            if !self.fully_open(b) {
                continue;
            }
            v.extend(self.box_item_cells(b).into_iter().map(|(k, r)| (k, r, Some(b.id))));
        }
        v
    }

    /// Grid cell under a point, without clamping to the grid (a drop may want a cell off it).
    fn grid_cell_unclamped(&self, p: Pt) -> Cell {
        let g = &self.grid;
        Cell {
            col: ((p.x - g.origin.x) / g.cell_w).floor() as i32,
            row: ((p.y - g.origin.y) / g.cell_h).floor() as i32,
        }
    }

    pub fn order(&self) -> Vec<ItemKey> {
        self.visible_cells().into_iter().map(|(k, _, _)| k).collect()
    }

    /// Icon rect and label rect of an item in a cell.
    pub fn item_parts(&self, key: &ItemKey, cell: Rect, full: bool) -> (Rect, Rect) {
        // Where Explorer puts them (measured on its own desktop): the icon 7 DIPs into the cell,
        // the label 3 DIPs under it.
        let icon = Rect::new(cell.x + (cell.w - self.icon_dip) / 2.0, cell.y + 7.0, self.icon_dip, self.icon_dip);
        let lw = cell.w - 4.0;
        let lh = self.label_height(key, lw, full);
        (icon, Rect::new(cell.x + 2.0, icon.bottom() + 3.0, lw, lh))
    }

    pub fn label_height(&self, key: &ItemKey, w: f32, full: bool) -> f32 {
        match self.labels.get(&(key.clone(), full, w as i32)) {
            Some(l) => deskboxes_render::text_height(l),
            None => self.line_h * if full { 1.0 } else { 2.0 },
        }
    }

    pub fn label_layout(&mut self, key: &ItemKey, w: f32, full: bool) -> Option<TextLayout> {
        let k = (key.clone(), full, w as i32);
        if let Some(l) = self.labels.get(&k) {
            return Some(l.clone());
        }
        let text = self.items.get(key)?.display_name.clone();
        let l = self.gpu.label(&self.label_tf, &text, w, self.line_h, if full { 0 } else { 2 }, true).ok()?;
        self.labels.insert(k, l.clone());
        Some(l)
    }

    pub fn hit_rect(&self, key: &ItemKey, cell: Rect) -> Rect {
        let full = self.is_full_label(key);
        let (i, l) = self.item_parts(key, cell, full);
        Rect::new(cell.x + 1.0, i.y - 2.0, cell.w - 2.0, l.bottom() - i.y + 4.0)
    }

    pub fn is_full_label(&self, key: &ItemKey) -> bool {
        self.sel.focus.as_ref() == Some(key) && self.sel.is_selected(key)
    }

    // ---------------------------------------------------------------- roll-up

    /// Which screen (work area) edges a box touches.
    pub fn dock(&self, b: &BoxModel) -> Dock {
        let (w, e) = (self.work, 2.0);
        Dock {
            top: b.rect.y <= w.y + e,
            bottom: b.rect.bottom() >= w.bottom() - e,
            left: b.rect.x <= w.x + e,
            right: b.rect.right() >= w.right() - e,
        }
    }

    /// Rolls up while the pointer is away: rolled up by hand, or docked at the top or bottom
    /// screen edge (not the sides: a box there has nowhere to roll to).
    pub fn rolls_up(&self, b: &BoxModel) -> bool {
        let d = self.dock(b);
        b.rolled_up || (self.settings.rollup.edges && (d.top || d.bottom))
    }

    fn want_open(&self, b: &BoxModel) -> bool {
        let busy = matches!(self.drag, Drag::MoveBox { id, .. } | Drag::Resize { id, .. } if id == b.id) || self.box_edit.as_ref().map(|e| e.id == b.id).unwrap_or(false);
        !self.rolls_up(b) || busy || self.revealed.contains(&b.id) || self.search_holds_open(b)
    }

    /// How far open a box is drawn (eased), 0 = title only.
    pub fn openness(&self, b: &BoxModel) -> f32 {
        let t = self.open_t.get(&b.id).copied().unwrap_or(if self.want_open(b) { 1.0 } else { 0.0 });
        t * t * (3.0 - 2.0 * t)
    }

    pub fn fully_open(&self, b: &BoxModel) -> bool {
        self.openness(b) >= 0.999
    }

    /// The box as drawn now: its title bar plus the open part. A box docked at the bottom
    /// keeps its title at the bottom when rolled up and opens upward.
    pub fn shown_rect(&self, b: &BoxModel) -> Rect {
        let h = header_h() + (b.rect.h - header_h()).max(0.0) * self.openness(b);
        let d = self.dock(b);
        let o = self.snap_offset(b.id);
        if d.bottom && !d.top && self.rolls_up(b) {
            Rect::new(b.rect.x + o.x, b.rect.bottom() - h + o.y, b.rect.w, h)
        } else {
            Rect::new(b.rect.x + o.x, b.rect.y + o.y, b.rect.w, h)
        }
    }

    /// Where a snapping box is drawn relative to its rect (gliding to 0).
    pub fn snap_offset(&self, id: u32) -> Pt {
        match self.snap_glide {
            Some((g, from, t0)) if g == id => {
                let t = (t0.elapsed().as_secs_f32() / SNAP_GLIDE_S).min(1.0);
                let k = 1.0 - ease_out(t);
                Pt { x: from.x * k, y: from.y * k }
            }
            _ => Pt { x: 0.0, y: 0.0 },
        }
    }

    pub fn snap_animating(&self) -> bool {
        self.snap_glide.map(|(_, _, t0)| t0.elapsed().as_secs_f32() < SNAP_GLIDE_S).unwrap_or(false)
    }

    /// Advance the roll animations (called per frame); keeps frames coming while one runs.
    pub fn step_roll_anim(&mut self) {
        let now = Instant::now();
        let dt = self.anim_last.map(|t| now.duration_since(t).as_secs_f32()).unwrap_or(0.0).min(0.05);
        let targets: Vec<(u32, f32)> = self.layout.boxes.iter().map(|b| (b.id, if self.want_open(b) { 1.0 } else { 0.0 })).collect();
        self.open_t.retain(|id, _| targets.iter().any(|(t, _)| t == id));
        let mut active = false;
        for (id, tgt) in targets {
            let cur = *self.open_t.entry(id).or_insert(tgt);
            if cur != tgt {
                let step = dt / ROLL_ANIM_S;
                let n = if tgt > cur { (cur + step).min(tgt) } else { (cur - step).max(tgt) };
                self.open_t.insert(id, n);
                active |= n != tgt;
            }
        }
        self.anim_last = if active { Some(now) } else { None };
        self.anim_roll = active;
    }

    /// The loose icons where they are (or would settle with the box being dragged).
    pub fn loose_view(&self) -> &std::collections::BTreeMap<ItemKey, Cell> {
        self.preview_loose.as_ref().unwrap_or(&self.layout.loose)
    }

    /// Start glides for loose icons whose cell changed since the last frame (not ones the user
    /// just dropped, nor new ones), and drop finished glides.
    pub fn update_glides(&mut self) {
        let now = Instant::now();
        let mut at = std::mem::take(&mut self.glide_at);
        let mut glide = std::mem::take(&mut self.glide);
        let mut n = 0;
        for (k, c) in self.loose_view() {
            n += 1;
            let r = self.grid.cell_rect(*c);
            let tgt = Pt { x: r.x, y: r.y };
            if let Some(prev) = at.insert(k.clone(), tgt) {
                if prev != tgt && !self.no_glide.contains(k) {
                    let from = glide_pos(glide.get(k), prev, now);
                    glide.insert(k.clone(), (from, now));
                } else if prev != tgt {
                    glide.remove(k);
                }
            }
        }
        if at.len() != n {
            let lv = self.loose_view();
            at.retain(|k, _| lv.contains_key(k));
        }
        glide.retain(|_, (_, t0)| now.duration_since(*t0).as_secs_f32() < GLIDE_S);
        self.glide_at = at;
        self.glide = glide;
        self.no_glide.clear();
    }

    /// A loose icon's cell rect as drawn now (mid-glide: on its way to `cell`).
    pub fn glided(&self, k: &ItemKey, cell: Rect) -> Rect {
        let p = glide_pos(self.glide.get(k), Pt { x: cell.x, y: cell.y }, Instant::now());
        Rect::new(p.x, p.y, cell.w, cell.h)
    }

    /// Recompute where loose icons would settle with the dragged box where it is now.
    fn update_preview(&mut self) {
        let mut l = self.layout.clone();
        layout::settle(&mut l, &self.grid);
        self.preview_loose = Some(l.loose);
    }

    /// Whether anything is animating (keeps the frame clock running).
    pub fn animating(&self) -> bool {
        self.anim_roll
            || !self.glide.is_empty()
            || self.tab_anim.map(|(_, _, t0)| t0.elapsed().as_secs_f32() < TAB_ANIM_S).unwrap_or(false)
            || self.tip.as_ref().map(|t| t.4.elapsed().as_secs_f32() < TIP_FADE_S).unwrap_or(false)
            || self.search_animating()
            || self.az_animating()
            || self.snap_animating()
            || self.header_animating()
            || self.blend_animating()
            || self.tuck_animating()
            || self.above_animating()
            || self.scroll_animating()
            || (matches!(self.drag, Drag::MoveBox { .. }) && self.move_t0.elapsed().as_secs_f32() < LIFT_S)
            || !self.vanishing.is_empty()
    }

    /// The title size or corners changed: new title text formats and metrics, everything redrawn.
    pub fn apply_look(&mut self) {
        set_look_metrics(&self.settings.look);
        let sz = self.settings.look.title_size;
        let fam = if self.settings.look.title_font.is_empty() { self.font.0.clone() } else { self.settings.look.title_font.clone() };
        let w = title_weight(&self.settings.look.title_weight);
        // Other tabs' titles: a step lighter than the open one.
        let lighter = if w >= 600 { 400 } else { (w - 100).max(300) };
        if let (Ok(a), Ok(b)) = (self.gpu.text_format_weight(&fam, sz, lighter), self.gpu.text_format_weight(&fam, sz, w)) {
            self.title_tf = a;
            self.title_tf_bold = b;
        }
        self.labels_title_changed();
        self.title_line_h = (sz * 1.4).ceil();
        self.invalidate();
    }

    /// Header text widths depend on the font: tabs are measured again on the next frame.
    fn labels_title_changed(&mut self) {
        self.invalidate();
    }

    /// The roll-up and menu buttons in a box's title (Settings › Styles › Title-bar buttons),
    /// left of the magnifier and the lock.
    pub fn title_btn_rects(&self, b: &BoxModel) -> Option<[Rect; 2]> {
        if !self.settings.look.title_buttons {
            return None;
        }
        let a = self.header_layout(b).add;
        let step = a.w + 2.0;
        let lk = self.locks(b);
        let lock_left = !lk.tabs && (self.settings.lock_all || b.locks.any());
        let first = if lock_left { 3.0 } else { 2.0 };
        Some([a.offset(-(first + 1.0) * step, 0.0), a.offset(-first * step, 0.0)])
    }

    /// The title button under `p` (shown while the pointer is on that title): 0 roll, 1 menu.
    pub fn title_btn_at(&self, p: Pt) -> Option<(u32, u8)> {
        let (id, ..) = self.hover_hdr?;
        let b = self.box_by_id(id)?;
        let r = self.title_btn_rects(b)?;
        r.iter().position(|r| r.inflate(1.0).contains(p)).map(|i| (id, i as u8))
    }

    /// Roll a box up, or open a rolled-up one (and keep it open).
    pub fn toggle_roll(&mut self, id: u32) {
        let looks_open = self.box_by_id(id).map(|b| !b.rolled_up).unwrap_or(false) || self.revealed.contains(&id);
        self.revealed.remove(&id);
        self.pending_reveal = None;
        self.no_reveal = if looks_open { Some(id) } else { None };
        self.change_layout(|l, _| {
            if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) {
                b.rolled_up = looks_open;
            }
        });
    }

    /// A box's locks in effect ("Lock everything" locks it all).
    pub fn locks(&self, b: &BoxModel) -> deskboxes_core::model::BoxLocks {
        if self.settings.lock_all {
            deskboxes_core::model::BoxLocks::ALL
        } else if b.is_folder() {
            // A folder box shows one folder: no tabs.
            deskboxes_core::model::BoxLocks { tabs: true, ..b.locks }
        } else {
            b.locks
        }
    }

    fn locks_of(&self, id: u32) -> deskboxes_core::model::BoxLocks {
        self.box_by_id(id).map(|b| self.locks(b)).unwrap_or_default()
    }

    /// Whether icons `keys` may be dropped into box `to` (None: onto the desktop): neither where
    /// they come from nor where they go is locked.
    fn icons_movable(&self, keys: &[ItemKey], to: Option<u32>) -> bool {
        let locked_box = |id: u32| self.locks_of(id).icons;
        if let Some(t) = to {
            if locked_box(t) {
                return false;
            }
        } else if self.settings.lock_all {
            return false;
        }
        keys.iter().all(|k| match self.layout.boxes.iter().find(|b| b.tabs.iter().any(|t| t.items.contains(k))) {
            Some(b) => !self.locks(b).icons,
            None => !self.settings.lock_all,
        })
    }

    /// The custom color window for a box (tab None) or one of its tabs.
    fn open_color_picker(&mut self, id: u32, tab: Option<usize>) {
        let Some(h) = self.host.clone() else { return };
        let Some(b) = self.box_by_id(id) else { return };
        let cur = match tab {
            Some(t) => b.tabs.get(t).and_then(|t| t.color),
            None => b.style.color,
        };
        let hex = |c: Rgba| format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b);
        let (ar, ag, ab) = accent();
        let title = match tab {
            Some(t) => format!("{} – {}", t_(self.lang, S::TabColor), b.tabs.get(t).map(|x| x.title.as_str()).unwrap_or("")),
            None => format!("{} – {}", t_(self.lang, S::BoxColor), b.title()),
        };
        self.color_pick = Some((id, tab, cur));
        if let Some(c) = &self.host_child {
            unsafe {
                let _ = AllowSetForegroundWindow(c.id());
            }
        }
        let body = json!({
            "title": title,
            "current": cur.map(hex).unwrap_or_default(),
            "auto": if tab.is_some() { format!("#{ar:02X}{ag:02X}{ab:02X}") } else if self.light { "#F3F3F3".to_string() } else { "#202020".to_string() },
            "box": id,
            "key": tab.map(|t| format!("tab:{t}")).unwrap_or_else(|| "box".into()),
            "ok": t_(self.lang, S::Ok),
            "cancel": t_(self.lang, S::Cancel),
        });
        if let Err(e) = h.send("colorpicker", body) {
            log::warn!("color picker: {e}");
        }
    }

    /// A color from the picker: shown live while it changes; the final one is recorded once
    /// (undo), Cancel puts the starting color back.
    fn on_picked_color(&mut self, v: &Value) {
        let Some((id, tab, orig)) = self.color_pick else { return };
        let hx = v["value"].as_str().unwrap_or("").trim_start_matches('#').to_string();
        let color = u32::from_str_radix(&hx, 16).ok().filter(|_| hx.len() == 6).map(|x| Rgba {
            r: (x >> 16) as u8,
            g: (x >> 8) as u8,
            b: x as u8,
            a: 255,
        });
        let set = |l: &mut MonitorLayout, c: Option<Rgba>| {
            if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) {
                match tab {
                    Some(t) => {
                        if let Some(t) = b.tabs.get_mut(t) {
                            t.color = c;
                        }
                    }
                    None => b.style.color = c,
                }
            }
        };
        let fin = v["final"].as_bool().unwrap_or(false);
        // Back to the start first, so the change is recorded (or nothing is, on Cancel).
        set(&mut self.layout, orig);
        if !fin {
            set(&mut self.layout, color);
            self.invalidate();
            return;
        }
        self.color_pick = None;
        if !v["cancel"].as_bool().unwrap_or(false) && color != orig {
            self.change_layout(|l, _| set(l, color));
        } else {
            self.invalidate();
        }
    }

    /// Hide the tooltip (and forget a pending one).
    /// Only an icon's tooltip (the pointer moved to another icon or off it).
    fn hide_item_tip(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.msg_hwnd), T_TIP);
        }
        if self.tip.take().is_some() {
            self.invalidate();
        }
    }

    pub fn hide_tip(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.msg_hwnd), T_TIP);
            let _ = KillTimer(Some(self.msg_hwnd), T_BTNTIP);
        }
        // Title-button tooltips too (a click, the wheel): until the pointer moves to another one.
        if self.tip.take().is_some() | self.btn_tip.take().is_some() {
            self.invalidate();
        }
    }

    /// The pointer rested on an icon: ask the worker for Explorer's tooltip text.
    fn tip_due(&mut self) {
        // Privacy mode: no names in tooltips either.
        if !matches!(self.drag, Drag::None) || self.menu.is_some() || self.rename.is_some() || self.privacy_on() {
            return;
        }
        let Some(k) = self.hover.clone() else { return };
        if let Some(it) = self.items.get(&k) {
            let _ = self.worker.send(Req::InfoTip { key: k, path: it.parsing_name.clone() });
        }
    }

    fn on_info_tip(&mut self, key: ItemKey, text: String) {
        if self.hover.as_ref() != Some(&key) || !matches!(self.drag, Drag::None) || self.menu.is_some() {
            return;
        }
        let Some(name) = self.items.get(&key).map(|i| i.display_name.clone()) else { return };
        self.tip = Some((key, name, text, self.cursor_dip(), Instant::now()));
        self.invalidate();
    }

    fn reveal(&mut self, id: u32) {
        if self.revealed.insert(id) {
            unsafe {
                SetTimer(Some(self.msg_hwnd), T_ROLLCHECK, 200, None);
            }
            self.invalidate();
        }
    }

    /// The pointer is on a box (or dragging icons over it): open it if it is rolled up.
    fn hover_reveal(&mut self, p: Pt, dragging_items: bool) {
        let id = match self.hit_test(p) {
            Hit::Header { id, .. } | Hit::BoxBody { id } | Hit::Edge { id, .. } | Hit::Item { in_box: Some(id), .. } => Some(id),
            _ => None,
        };
        if let Some(h) = self.no_reveal {
            if id == Some(h) {
                return;
            }
            self.no_reveal = None;
        }
        let Some(b) = id.and_then(|id| self.box_by_id(id)) else {
            self.pending_reveal = None;
            return;
        };
        let id = b.id;
        if !self.rolls_up(b) || self.revealed.contains(&id) {
            return;
        }
        let mode = self.settings.rollup.reveal.clone();
        if dragging_items || mode == "delay" {
            if self.pending_reveal != Some(id) {
                self.pending_reveal = Some(id);
                unsafe {
                    SetTimer(Some(self.msg_hwnd), T_REVEAL, if dragging_items { 450 } else { 350 }, None);
                }
            }
        } else if mode == "hover" {
            self.reveal(id);
        }
    }

    pub fn cursor_dip(&self) -> Pt {
        let mut c = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut c);
        }
        self.screen_to_dip(c.x, c.y)
    }

    fn reveal_pending(&mut self) {
        let Some(id) = self.pending_reveal.take() else { return };
        let p = self.cursor_dip();
        if self.box_by_id(id).map(|b| self.shown_rect(b).inflate(4.0).contains(p)).unwrap_or(false) {
            self.reveal(id);
        }
    }

    /// Roll up again the boxes the pointer has left (not while one is moved, edited, or has
    /// its menu open).
    fn roll_check(&mut self) {
        let p = self.cursor_dip();
        let menu_box = match self.menu.as_ref().map(|(_, c)| &c.hit) {
            Some(Hit::Header { id, .. } | Hit::BoxBody { id } | Hit::Edge { id, .. } | Hit::Item { in_box: Some(id), .. }) => Some(*id),
            _ => None,
        };
        let keep: Vec<u32> = self
            .revealed
            .iter()
            .copied()
            .filter(|id| {
                let Some(b) = self.box_by_id(*id) else { return false };
                let busy = matches!(self.drag, Drag::MoveBox { id: d, .. } | Drag::Resize { id: d, .. } if d == *id) || matches!(self.drag, Drag::Items { .. });
                b.rect.inflate(12.0).contains(p) || busy || menu_box == Some(*id) || self.box_edit.as_ref().map(|e| e.id == *id).unwrap_or(false)
            })
            .collect();
        if keep.len() != self.revealed.len() {
            self.revealed = keep.into_iter().collect();
            self.invalidate();
        }
        if !self.revealed.is_empty() {
            unsafe {
                SetTimer(Some(self.msg_hwnd), T_ROLLCHECK, 200, None);
            }
        }
    }

    /// A box's title: tabs sized to their titles from the left (shrunk to fit, leaving room to
    /// grab the title and move the box), and the + button at the right.
    pub fn header_layout(&self, b: &BoxModel) -> HeaderLayout {
        let r = self.shown_rect(b);
        let add = Rect::new(r.right() - 30.0, r.y + 4.0, 24.0, header_h() - 8.0);
        let n = b.tabs.len();
        if n <= 1 {
            let left = self.settings.look.align == "left";
            return HeaderLayout {
                tabs: vec![if left && b.is_folder() {
                    // Room for the folder glyph (or back arrow) on the left.
                    Rect::new(r.x + 32.0, r.y, (r.w - 66.0).max(20.0), header_h())
                } else if left {
                    Rect::new(r.x + 4.0, r.y, (r.w - 38.0).max(20.0), header_h())
                } else {
                    Rect::new(r.x + 34.0, r.y, (r.w - 68.0).max(20.0), header_h())
                }],
                add,
            };
        }
        let mut ws: Vec<f32> = b
            .tabs
            .iter()
            .map(|t| {
                let w = self
                    .gpu
                    .label(&self.title_tf_bold, &t.title, 10_000.0, self.title_line_h, 1, false)
                    .map(|l| deskboxes_render::text_width(&l))
                    .unwrap_or(60.0);
                (w.min(180.0) + 24.0).max(48.0)
            })
            .collect();
        // A tab just added grows in from nothing; the tabs after it make room as it does.
        if let Some((gid, gi, t0)) = self.tab_grow {
            if gid == b.id {
                if let Some(w) = ws.get_mut(gi) {
                    *w *= ease_out(t0.elapsed().as_secs_f32() / TAB_GROW_S).max(0.05);
                }
            }
        }
        // Room at the right for the magnifier and +.
        let avail = r.w - 12.0 - 68.0 - TAB_GAP * (n - 1) as f32;
        let sum: f32 = ws.iter().sum();
        if sum > avail && sum > 0.0 {
            let k = avail / sum;
            for w in &mut ws {
                *w = (*w * k).max(28.0);
            }
        }
        let mut x = r.x + 6.0;
        let tabs = ws
            .iter()
            .map(|w| {
                let t = Rect::new(x, r.y + 4.0, *w, header_h() - 8.0);
                x += w + TAB_GAP;
                t
            })
            .collect();
        HeaderLayout { tabs, add }
    }

    /// Where a dragged tab `from` would go in its box's title for pointer x: an index in the
    /// list without it.
    pub fn tab_insert_index(&self, b: &BoxModel, from: usize, x: f32) -> usize {
        self.header_layout(b).tabs.iter().enumerate().filter(|(i, t)| *i != from && t.x + t.w / 2.0 < x).count()
    }

    /// Another box whose title is under `p` (for combining boxes and moving tabs).
    pub fn header_target(&self, except: u32, p: Pt) -> Option<u32> {
        self.layout
            .boxes
            .iter()
            .rev()
            .filter(|b| b.id != except && !self.locks(b).tabs)
            .find(|b| {
                let r = self.shown_rect(b);
                Rect::new(r.x, r.y, r.w, header_h()).inflate(2.0).contains(p)
            })
            .map(|b| b.id)
    }

    /// The pointer is on tab `t` of a box: open it after `ms` if it stays there.
    fn arm_tab_switch(&mut self, id: u32, t: usize, ms: u32) {
        if self.box_by_id(id).map(|b| b.active_tab == t).unwrap_or(true) {
            self.pending_tab = None;
            return;
        }
        if self.pending_tab != Some((id, t)) {
            self.pending_tab = Some((id, t));
            unsafe {
                SetTimer(Some(self.msg_hwnd), T_TABHOVER, ms, None);
            }
        }
    }

    fn tab_switch_pending(&mut self) {
        let Some((id, t)) = self.pending_tab.take() else { return };
        if let Hit::Header { id: h, tab: Some(ht), .. } = self.hit_test(self.cursor_dip()) {
            if h == id && ht == t {
                self.set_active_tab(id, t);
            }
        }
    }

    /// Show another tab (not an undo step).
    pub fn set_active_tab(&mut self, id: u32, t: usize) {
        let from = self.box_by_id(id).and_then(|b| self.header_layout(b).tabs.get(b.active_tab).copied());
        if let Some(b) = self.layout.boxes.iter_mut().find(|b| b.id == id) {
            if b.active_tab != t && t < b.tabs.len() {
                b.active_tab = t;
                if let Some(r) = from {
                    self.tab_anim = Some((id, r, Instant::now()));
                }
                self.scroll.remove(&id);
                self.layout_changed();
            }
        }
    }

    fn hdr_check(&mut self) {
        let Some((id, ..)) = self.hover_hdr else { return };
        match self.hit_test(self.cursor_dip()) {
            Hit::Header { id: h, .. } if h == id => unsafe {
                SetTimer(Some(self.msg_hwnd), T_HDRCHECK, 250, None);
            },
            _ => {
                self.hover_hdr = None;
                self.invalidate();
            }
        }
    }

    /// The title button under the pointer (search or +): hover, the search field opening, and
    /// a tooltip after a rest.
    fn update_hdr_btn(&mut self, p: Pt, hh: Option<(u32, Option<usize>, bool)>) {
        let btn = match hh {
            Some((id, _, true)) => Some((id, 2u8)),
            Some((id, _, false)) => self.box_by_id(id).filter(|b| self.header_search_rect(b).inflate(1.0).contains(p)).map(|_| (id, 1u8)),
            None => None,
        };
        if btn == self.hdr_btn.map(|(id, k, _)| (id, k)) {
            return;
        }
        if let Some((id, 1, _)) = self.hdr_btn {
            let e = self.search_expand(id);
            self.search_btn_out = Some((id, e, Instant::now()));
        }
        if let Some((id, 1)) = btn {
            if self.search_btn_out.map(|(o, _, _)| o == id).unwrap_or(false) {
                self.search_btn_out = None;
            }
        }
        self.hdr_btn = btn.map(|(id, k)| (id, k, Instant::now()));
        self.btn_tip = None;
        unsafe {
            let _ = KillTimer(Some(self.msg_hwnd), T_BTNTIP);
            if btn.is_some() {
                SetTimer(Some(self.msg_hwnd), T_BTNTIP, GetDoubleClickTime().clamp(200, 1500), None);
            }
        }
        crate::anim::set(true);
        self.invalidate();
    }

    /// Privacy mode is drawn (on, and the feature not turned off in Settings).
    pub fn privacy_on(&self) -> bool {
        self.settings.privacy.allowed && self.settings.privacy.on
    }

    fn set_privacy(&mut self, on: bool) {
        if self.settings.privacy.on == on {
            return;
        }
        self.settings.privacy.on = on;
        log::info!("privacy mode: {on}");
        let _ = persist::save_settings(&self.paths.settings, &self.settings);
        self.hide_tip();
        self.invalidate();
    }

    fn btn_tip_due(&mut self) {
        let Some((id, k, _)) = self.hdr_btn else { return };
        if !matches!(self.drag, Drag::None) || self.menu.is_some() || self.box_by_id(id).is_none() {
            return;
        }
        let text = if k == 1 { t(self.lang, S::SearchBox) } else { t(self.lang, S::AddTab) };
        self.btn_tip = Some((text.to_string(), self.cursor_dip(), Instant::now()));
        self.invalidate();
    }

    /// How far a box's search button has opened into a field (0..1, eased).
    pub fn search_expand(&self, id: u32) -> f32 {
        let opening = match self.hdr_btn {
            Some((h, 1, t0)) if h == id => ease_out(t0.elapsed().as_secs_f32() / SEARCH_EXPAND_S),
            _ => 0.0,
        };
        let closing = match self.search_btn_out {
            Some((h, from, t0)) if h == id => from * (1.0 - ease_out(t0.elapsed().as_secs_f32() / SEARCH_EXPAND_S)),
            _ => 0.0,
        };
        opening.max(closing)
    }

    /// A title button's hover highlight (0..1, fading in).
    pub fn btn_hover(&self, id: u32, k: u8) -> f32 {
        match self.hdr_btn {
            Some((h, kk, t0)) if h == id && kk == k => ease_out(t0.elapsed().as_secs_f32() / HOVER_FADE_S),
            _ => 0.0,
        }
    }

    pub fn header_animating(&self) -> bool {
        let busy = |t0: Instant, s: f32| t0.elapsed().as_secs_f32() < s;
        self.hdr_btn.map(|(_, _, t0)| busy(t0, SEARCH_EXPAND_S)).unwrap_or(false)
            || self.search_btn_out.map(|(_, _, t0)| busy(t0, SEARCH_EXPAND_S)).unwrap_or(false)
            || busy(self.hdr_hover_t0, HOVER_FADE_S)
            || self.tab_grow.map(|(_, _, t0)| busy(t0, TAB_GROW_S)).unwrap_or(false)
            || self.btn_tip.as_ref().map(|t| busy(t.2, TIP_FADE_S)).unwrap_or(false)
    }

    /// A new tab in a box, its title being edited.
    fn add_tab(&mut self, id: u32) {
        let title = t(self.lang, S::NewTabTitle).to_string();
        let mut ix = None;
        self.change_layout(|l, _| ix = layout::add_tab(l, id, &title));
        if let Some(tab) = ix {
            self.scroll.remove(&id);
            self.tab_grow = Some((id, tab, Instant::now()));
            crate::anim::set(true);
            self.begin_rename(RenameTarget::Box { id, tab });
        }
    }

    /// A dragged tab released at `cur`: reorder in its own title, move into the box whose title
    /// is under the pointer, or tear off into a new box there.
    fn drop_tab(&mut self, id: u32, tab: usize, cur: Pt) {
        let Some(b) = self.box_by_id(id) else { return };
        let own = Rect::new(b.rect.x, b.rect.y - 16.0, b.rect.w, header_h() + 32.0);
        if own.contains(cur) {
            let to = self.tab_insert_index(b, tab, cur.x);
            self.change_layout(|l, _| {
                layout::move_tab(l, id, tab, to);
            });
            return;
        }
        if let Some(dst) = self.header_target(id, cur) {
            self.change_layout(|l, _| {
                layout::move_tab_to_box(l, id, tab, dst);
            });
            self.scroll.remove(&dst);
            return;
        }
        let (w, h) = (b.rect.w, b.rect.h);
        let bounds = Rect::new(0.0, 0.0, self.size_dip.0, self.size_dip.1);
        let r = Rect::new(cur.x - w / 2.0, cur.y - header_h() / 2.0, w, h).clamp_into(&self.work.clamp_into(&bounds));
        self.change_layout(|l, _| {
            layout::split_tab(l, id, tab, r);
        });
    }

    pub fn hit_test(&self, p: Pt) -> Hit {
        if !self.settings.show_icons || self.on_pill(p) {
            return Hit::Empty;
        }
        for b in self.layout.boxes.iter().rev() {
            if self.is_tucked(Some(b)) {
                continue;
            }
            let r = self.shown_rect(b);
            let open = self.fully_open(b);
            let outer = r.inflate(EDGE / 2.0);
            if !outer.contains(p) {
                continue;
            }
            let (l, t, rr, bb) = (p.x < r.x + EDGE / 2.0, p.y < r.y + EDGE / 2.0, p.x > r.right() - EDGE / 2.0, p.y > r.bottom() - EDGE / 2.0);
            let lk = self.locks(b);
            if (l || t || rr || bb) && open && !lk.size {
                return Hit::Edge { id: b.id, l, t, r: rr, b: bb };
            }
            if p.y < r.y + header_h() {
                let hl = self.header_layout(b);
                if hl.add.inflate(2.0).contains(p) && !lk.tabs {
                    return Hit::Header { id: b.id, tab: None, add: true };
                }
                if self.hover_hdr.map(|h| h.0 == b.id).unwrap_or(false) && self.header_search_rect(b).inflate(1.0).contains(p) {
                    return Hit::Header { id: b.id, tab: None, add: false };
                }
                let tab = if b.tabs.len() > 1 {
                    hl.tabs.iter().position(|t| p.x >= t.x - TAB_GAP / 2.0 && p.x < t.right() + TAB_GAP / 2.0)
                } else {
                    None
                };
                return Hit::Header { id: b.id, tab, add: false };
            }
            if !open {
                return Hit::BoxBody { id: b.id };
            }
            let c = Self::content_rect(b);
            for (k, cell) in self.box_item_cells(b) {
                if cell.bottom() < c.y || cell.y > c.bottom() {
                    continue;
                }
                if self.hit_rect(&k, cell).contains(p) && c.contains(p) {
                    return Hit::Item { key: k, in_box: Some(b.id) };
                }
            }
            return Hit::BoxBody { id: b.id };
        }
        if self.is_tucked(None) {
            return Hit::Empty;
        }
        for (k, c) in self.layout.loose.iter() {
            if !self.items.contains_key(k) {
                continue;
            }
            let cell = self.grid.cell_rect(*c);
            if cell.inflate(2.0).contains(p) && self.hit_rect(k, cell).contains(p) {
                return Hit::Item { key: k.clone(), in_box: None };
            }
        }
        Hit::Empty
    }

    pub fn to_dip(&self, x: i32, y: i32) -> Pt {
        Pt {
            x: x as f32 / self.scale,
            y: y as f32 / self.scale,
        }
    }

    pub fn to_screen(&self, p: Pt) -> (i32, i32) {
        (self.origin_px.x + (p.x * self.scale).round() as i32, self.origin_px.y + (p.y * self.scale).round() as i32)
    }

    pub fn screen_to_dip(&self, x: i32, y: i32) -> Pt {
        self.to_dip(x - self.origin_px.x, y - self.origin_px.y)
    }

    // ---------------------------------------------------------------- model changes

    /// Put icons where the sorting rules say: `which` = those keys (None = every icon), `fresh`
    /// = they just appeared (unmatched ones go to the "new icons" tab). Icons go in name order.
    pub fn apply_rules(&mut self, which: Option<&BTreeSet<ItemKey>>, fresh: bool) -> bool {
        if self.layout.rules.is_empty() && !(fresh && self.layout.new_items_to.is_some()) {
            return false;
        }
        let mut its: Vec<&Item> = self.items.values().filter(|i| i.source != Source::Folder && which.map(|w| w.contains(&i.key)).unwrap_or(true)).collect();
        its.sort_by_cached_key(|i| i.display_name.to_lowercase());
        deskboxes_core::rules::apply(&mut self.layout, &its, fresh, deskboxes_core::rules::now_filetime())
    }

    /// "Sort the desktop now": one undo step.
    pub fn sort_now(&mut self) {
        let before = self.layout.clone();
        if self.apply_rules(None, false) {
            layout::evict_from_boxes(&mut self.layout, &self.grid);
            self.hist.record(&before);
            self.layout_changed();
        }
        log::info!("sort now: {} rule(s)", self.layout.rules.len());
    }

    /// Icons that appeared since the last scan (not ones dropped somewhere on purpose): sorted
    /// by the rules; with "keep sorted" every icon is.
    fn sort_new_items(&mut self, old: &BTreeSet<ItemKey>, dropped: &[ItemKey]) -> bool {
        let appeared: BTreeSet<ItemKey> = if old.is_empty() {
            BTreeSet::new()
        } else {
            self.items.keys().filter(|k| !old.contains(*k) && !dropped.contains(*k)).cloned().collect()
        };
        let mut moved = false;
        if self.layout.keep_sorted {
            moved |= self.apply_rules(None, false);
        }
        if !appeared.is_empty() {
            let m = self.apply_rules(Some(&appeared), true);
            if m {
                log::info!("rules: sorted new icon(s) of {}", appeared.len());
            }
            moved |= m;
        }
        moved
    }

    pub fn change_layout(&mut self, f: impl FnOnce(&mut MonitorLayout, &Grid)) {
        let before = self.layout.clone();
        f(&mut self.layout, &self.grid);
        if self.layout.keep_sorted {
            self.apply_rules(None, false);
        }
        layout::evict_from_boxes(&mut self.layout, &self.grid);
        if self.layout != before {
            self.hist.record(&before);
            self.layout_changed();
        }
    }

    pub fn layout_changed(&mut self) {
        // Undo, a saved layout or a deleted box can change what folder boxes show.
        self.folder_sync();
        if self.host.is_some() && self.settings_sig() != self.settings_sent {
            self.send_settings(false, None);
        }
        let mut blurs: Vec<i32> = self.layout.boxes.iter().map(|b| b.style.blur as i32).collect();
        blurs.retain(|b| !self.backdrops.contains_key(b));
        if !blurs.is_empty() {
            self.load_wallpaper();
        }
        unsafe {
            SetTimer(Some(self.msg_hwnd), T_SAVE, 1000, None);
        }
        self.invalidate();
    }

    pub fn save(&mut self) {
        let _ = persist::backup_layouts_daily(&self.paths.layouts, &self.paths.backups);
        if let Err(e) = persist::save_layout(&self.paths.layouts, &self.topology, &self.layout) {
            log::warn!("save layout: {e}");
        }
        self.save_snapshot();
        self.write_scene();
    }

    pub fn save_snapshot(&self) {
        if !self.scanned_once {
            return;
        }
        // Icon ids are renumbered densely for the snapshot.
        let used: std::collections::HashSet<u32> = self.items.values().map(|i| i.icon).collect();
        let mut ids: Vec<u32> = self.icon_imgs.keys().copied().filter(|id| used.contains(id)).collect();
        ids.sort();
        let remap: HashMap<u32, u32> = ids.iter().enumerate().map(|(i, id)| (*id, i as u32)).collect();
        let s = Snapshot {
            source_id: match &self.args.desktop_dir {
                Some(d) => format!("fixture:{}", d.display()),
                None => "desktop".into(),
            },
            dpi: (self.scale * 96.0) as u32,
            icon_px: self.icon_px,
            cell_w: self.cell_px.0,
            cell_h: self.cell_px.1,
            items: self
                .items
                .values()
                .cloned()
                .map(|mut i| {
                    i.icon = remap.get(&i.icon).copied().unwrap_or(0);
                    i
                })
                .collect(),
            icons: ids.iter().map(|id| self.icon_imgs[id].clone()).collect(),
        };
        if let Err(e) = persist::write_atomic(&self.paths.snapshot, &s.encode()) {
            log::warn!("save snapshot: {e}");
        }
    }

    /// Test/diagnostic output: every visible item and box in screen pixels.
    pub fn write_scene(&self) {
        let Some(p) = &self.args.scene_out else { return };
        let px = |r: Rect| {
            let (x, y) = self.to_screen(Pt { x: r.x, y: r.y });
            json!([x, y, (r.w * self.scale).round() as i32, (r.h * self.scale).round() as i32])
        };
        let items: Vec<Value> = self
            .visible_cells()
            .into_iter()
            .map(|(k, cell, b)| {
                let it = &self.items[&k];
                json!({ "key": k.0, "name": it.display_name, "path": it.parsing_name, "rect": px(self.hit_rect(&k, cell)), "box": b, "selected": self.sel.is_selected(&k) })
            })
            .collect();
        let boxes: Vec<Value> = self
            .layout
            .boxes
            .iter()
            .map(|b| json!({ "id": b.id, "rect": px(b.rect), "tabs": b.tabs.iter().map(|t| json!({"title": t.title, "items": t.items.len()})).collect::<Vec<_>>(), "active": b.active_tab }))
            .collect();
        let v = json!({ "items": items, "boxes": boxes, "scale": self.scale, "loose": self.layout.loose.len() });
        let _ = persist::write_atomic(p, serde_json::to_string_pretty(&v).unwrap_or_default().as_bytes());
    }

    /// Icons this scan removes, recorded where they are drawn now so they can fade out there.
    /// Not when the same scan adds icons (a rename or a move shows as remove + add).
    fn capture_vanishing(&mut self, items: &[Item], first: bool) {
        if first || !self.settings.show_icons {
            return;
        }
        let new: std::collections::HashSet<&ItemKey> = items.iter().map(|i| &i.key).collect();
        let removed: Vec<ItemKey> = self.items.keys().filter(|k| !new.contains(k)).cloned().collect();
        if removed.is_empty() || items.iter().any(|i| !self.items.contains_key(&i.key)) || removed.len() > 64 {
            return;
        }
        let now = Instant::now();
        for (k, cell, bx) in self.visible_cells() {
            if !removed.contains(&k) {
                continue;
            }
            let Some(it) = self.items.get(&k) else { continue };
            let full = self.is_full_label(&k);
            let (icon_rect, label) = self.item_parts(&k, cell, full);
            let lk = (k.clone(), full, (cell.w - 4.0) as i32);
            self.vanishing.push(Vanish {
                icon: self.bitmaps.get(&it.icon).cloned(),
                label: self.labels.get(&lk).map(|l| (l.clone(), self.label_bmps.get(&lk).cloned())),
                icon_rect,
                label_at: Pt { x: label.x, y: label.y },
                in_box: bx,
                t0: now,
            });
        }
        if !self.vanishing.is_empty() {
            crate::anim::set(true);
            self.invalidate();
        }
    }

    pub fn vanish_animating(&mut self) -> bool {
        self.vanishing.retain(|v| v.t0.elapsed().as_secs_f32() < VANISH_S);
        !self.vanishing.is_empty()
    }

    fn apply_scan(&mut self, items: Vec<Item>, icons: Vec<(u32, IconImage)>, native: Option<deskboxes_shell::view::NativeView>, ms: f64, first: bool) {
        let t = Instant::now();
        if first && !self.seeded {
            // The worker numbers icons independently of an unusable snapshot: replace the table.
            self.bitmaps.clear();
            self.icon_imgs.clear();
        }
        for (id, img) in icons {
            match self.gpu.bitmap(&img) {
                Ok(b) => {
                    self.bitmaps.insert(id, b);
                }
                Err(e) => log::warn!("icon upload: {e}"),
            }
            self.icon_imgs.insert(id, img);
        }
        let old: BTreeSet<ItemKey> = self.items.keys().cloned().collect();
        self.capture_vanishing(&items, first);
        let mut renamed = false;
        let mut new_items = HashMap::with_capacity(items.len());
        for it in items {
            if let Some(o) = self.items.get(&it.key) {
                renamed |= o.display_name != it.display_name;
            }
            new_items.insert(it.key.clone(), it);
        }
        self.items = new_items;
        if renamed || first {
            self.labels.clear();
            self.label_bmps.clear();
        }
        if let Some(nv) = &native {
            if nv.spacing.0 > 0 && nv.spacing.1 > 0 {
                let cp = (nv.spacing.0 as u32, nv.spacing.1 as u32);
                if cp != self.cell_px {
                    log::info!("grid cell from Explorer's view: {cp:?} px (was {:?})", self.cell_px);
                    self.cell_px = cp;
                    self.grid = Grid::new(self.work, cp.0 as f32 / self.scale, cp.1 as f32 / self.scale);
                }
            } else {
                log::warn!("Explorer view spacing unavailable; grid cell stays {:?}", self.cell_px);
            }
            if !nv.positions.is_empty() && self.layout.loose.is_empty() && self.layout.boxes.is_empty() {
                self.import_native_positions(&nv.positions);
            }
        } else if first {
            log::warn!("Explorer's desktop view unavailable; grid cell stays {:?} px", self.cell_px);
        }
        // New items from a drop go where they were dropped.
        let present: Vec<ItemKey> = {
            let mut v: Vec<&Item> = self.items.values().filter(|i| i.source != Source::Folder).collect();
            v.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
            v.into_iter().map(|i| i.key.clone()).collect()
        };
        let mut dropped: Vec<ItemKey> = vec![];
        if let Some((when, at, bx)) = self.pending_drop {
            if when.elapsed().as_secs() < 10 {
                let fresh: Vec<ItemKey> = present.iter().filter(|k| !old.contains(*k) && !self.known_before_drop.contains(*k)).cloned().collect();
                dropped = fresh.clone();
                if !fresh.is_empty() {
                    match bx {
                        Some(id) => {
                            layout::move_to_box(&mut self.layout, &fresh, id, None);
                        }
                        None => layout::move_to_grid(&mut self.layout, &fresh, self.grid.cell_at(at), &self.grid),
                    }
                    self.pending_drop = None;
                }
            } else {
                self.pending_drop = None;
            }
        }
        let changed = layout::reconcile(&mut self.layout, &present, &self.grid);
        let changed = self.sort_new_items(&old, &dropped) | changed;
        self.fill_folder_boxes();
        self.sel.set.retain(|k| self.items.contains_key(k));
        self.scanned_once = true;
        self.stats.items = self.items.len();
        self.stats.scan_ms = ms;
        if first {
            self.stats.reconcile_ms = self.t0.elapsed().as_secs_f64() * 1000.0;
            log::info!(
                "PHASE reconcile ms={:.1} items={} apply_ms={:.1}",
                self.stats.reconcile_ms,
                self.items.len(),
                t.elapsed().as_secs_f64() * 1000.0
            );
            if let Some(ms) = self.args.exit_after {
                unsafe {
                    SetTimer(Some(self.msg_hwnd), T_EXIT, ms, None);
                }
            }
        }
        if changed || first {
            unsafe {
                SetTimer(Some(self.msg_hwnd), T_SAVE, 1000, None);
            }
        }
        if first && self.hide_after_scan {
            self.hide_after_scan = false;
            self.draw();
            let th = Instant::now();
            let s = hide::set_hidden(true);
            self.stats.hide_ms = th.elapsed().as_secs_f64() * 1000.0;
            log::info!("PHASE hide_icons (after first scan) ms={:.1} state={s:?}", self.stats.hide_ms);
            return;
        }
        self.invalidate();
    }

    /// First run on the real desktop: keep the user's arrangement (Explorer's own positions).
    fn import_native_positions(&mut self, pos: &[(String, i32, i32)]) {
        let by_path: HashMap<String, ItemKey> = self.items.values().map(|i| (i.parsing_name.to_lowercase(), i.key.clone())).collect();
        let lv = hide::listview();
        let mut n = 0;
        for (p, x, y) in pos {
            let Some(k) = by_path.get(&p.to_lowercase()) else { continue };
            let mut pt = POINT { x: *x, y: *y };
            unsafe {
                let _ = ClientToScreen(lv, &mut pt);
            }
            let d = self.screen_to_dip(pt.x, pt.y);
            let c = self.grid.cell_at(Pt {
                x: d.x + self.grid.cell_w / 2.0 - self.grid.origin.x.min(0.0),
                y: d.y + self.grid.cell_h / 2.0 - 1.0,
            });
            if !self.layout.loose.values().any(|v| *v == c) {
                self.layout.loose.insert(k.clone(), c);
                n += 1;
            }
        }
        log::info!("imported {n} of {} native icon positions", pos.len());
    }

    // ---------------------------------------------------------------- host / menus

    fn start_host(&mut self) {
        let Some(exe) = find_host(&self.args) else {
            log::error!("shell host not found next to {}; context menus unavailable", exe_dir().display());
            return;
        };
        // A new host holds no prefetch.
        self.host_prefetch = None;
        self.host_starts.retain(|t| t.elapsed().as_secs() < 60);
        if self.host_starts.len() >= 5 {
            log::error!("shell host restarted 5 times in a minute; giving up until next start");
            return;
        }
        self.host_starts.push(Instant::now());
        let session = {
            let mut s = 0u32;
            unsafe {
                let _ = windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(std::process::id(), &mut s);
            }
            s
        };
        let name = format!("deskboxes-host-{session}-{}", std::process::id());
        match deskboxes_ipc::spawn_host(&exe, &name) {
            Ok(c) => {
                log::info!("shell host started pid={} {}", c.id(), exe.display());
                self.host_child = Some(c);
            }
            Err(e) => {
                log::error!("cannot start the shell host {}: {e}", exe.display());
                return;
            }
        }
        let lang = self.lang.code();
        let light = self.light;
        std::thread::spawn(move || {
            match deskboxes_ipc::HostClient::connect(&name, std::time::Duration::from_secs(15), |m| match m {
                Some(v) => events::post(Ev::HostMsg(v)),
                None => events::post(Ev::HostGone),
            }) {
                Ok(c) => {
                    let _ = c.send("hello", json!({ "proto": deskboxes_ipc::PROTO, "lang": lang, "theme": if light { "light" } else { "dark" } }));
                    events::post(Ev::HostReady(c));
                }
                Err(e) => {
                    log::error!("cannot connect to the shell host: {e}");
                    events::post(Ev::HostGone);
                }
            }
        });
    }

    fn folder_arg(&self) -> String {
        self.args.desktop_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default()
    }

    fn selection_paths(&self) -> Vec<String> {
        self.sel.ordered(&self.order()).iter().filter_map(|k| self.items.get(k)).map(|i| i.parsing_name.clone()).collect()
    }

    fn menu_request(&self, hit: &Hit, at: Pt, new_rect: Option<Rect>) -> Value {
        match hit {
            Hit::Item { .. } => {
                let items = self.selection_paths();
                json!({ "kind": "item", "items": items, "folder": self.folder_arg(), "shift": key_down(VK_SHIFT), "extra": self.item_extra(&items) })
            }
            _ => {
                let bid = match hit {
                    Hit::Header { id, .. } | Hit::BoxBody { id } | Hit::Edge { id, .. } => Some(*id),
                    _ => None,
                };
                let mut extra = match bid.and_then(|id| self.box_by_id(id)) {
                    Some(b) => self.box_menu(
                        b,
                        match hit {
                            Hit::Header { tab: Some(t), .. } => Some(*t),
                            _ => None,
                        },
                    ),
                    None => vec![json!({ "id": "dbx.newbox", "text": t(self.lang, S::NewBox), "glyph": "\u{E710}", "placement": "beforeNew" })],
                };
                let sub = vec![
                    json!({ "id": "dbx.sortnow", "text": t(self.lang, S::SortNow), "glyph": "\u{E8CB}" }),
                    json!({ "id": "dbx.rules", "text": t(self.lang, S::SortingRules), "glyph": "\u{E71C}" }),
                    json!({ "id": "dbx.settings", "text": t(self.lang, S::Settings), "glyph": "\u{E713}" }),
                    json!({ "separator": true }),
                    json!({ "id": "dbx.lockall", "text": t(self.lang, S::LockEverything), "glyph": "\u{E72E}", "checkable": true, "checked": self.settings.lock_all }),
                    json!({ "separator": true }),
                    json!({ "id": "dbx.disable", "text": t(self.lang, S::Disable) }),
                ];
                let mut sub = sub;
                // New folder box: one of the user's folders.
                let kf: Vec<Value> = items::known_folders()
                    .into_iter()
                    .map(|(n, p)| {
                        let s = match n {
                            "documents" => S::KfDocuments,
                            "downloads" => S::KfDownloads,
                            "pictures" => S::KfPictures,
                            "music" => S::KfMusic,
                            _ => S::KfVideos,
                        };
                        json!({ "id": format!("dbx.fbknown:{}", p.display()), "text": t(self.lang, s), "glyph": "\u{E8B7}" })
                    })
                    .collect();
                if !kf.is_empty() && bid.is_none() {
                    sub.insert(0, json!({ "id": "dbx.newfolderbox", "text": t(self.lang, S::NewFolderBox), "glyph": "\u{E8B7}", "children": kf }));
                }
                if self.settings.privacy.allowed {
                    sub.insert(5, json!({ "id": "dbx.privacy", "text": t(self.lang, S::PrivacyMode), "glyph": "\u{ED1A}", "checkable": true, "checked": self.settings.privacy.on }));
                }
                if self.settings.saved.in_menu {
                    // Saved layouts: save now, go back to one of the newest, or manage them.
                    let mut sv = vec![json!({ "id": "dbx.savenow", "text": t(self.lang, S::SaveLayoutNow), "glyph": "\u{E74E}" })];
                    let list = self.saved_layouts();
                    if !list.is_empty() {
                        sv.push(json!({ "separator": true }));
                        for s in list.iter().take(8) {
                            sv.push(json!({ "id": format!("dbx.restore:{}", s.id), "text": s.when, "glyph": "\u{E81C}" }));
                        }
                    }
                    sv.push(json!({ "separator": true }));
                    sv.push(json!({ "id": "dbx.savedpage", "text": t(self.lang, S::ManageSaved), "glyph": "\u{E713}" }));
                    sub.insert(2, json!({ "id": "dbx.savedmenu", "text": t(self.lang, S::SavedLayouts), "glyph": "\u{E81C}", "children": sv }));
                }
                extra.push(json!({ "id": "dbx.menu", "text": t(self.lang, S::BoxesSubmenu), "placement": "beforeNew", "children": sub, "icon": format!("{},0", std::env::current_exe().unwrap_or_default().display()) }));
                let _ = (at, new_rect);
                // View and Sort by, as Explorer's desktop has them (Windows' own text, "@dll,-id").
                let size = self.icon_dip.round() as u32;
                let view = vec![
                    json!({ "id": "iconsize.large", "text": "@ExplorerFrame.dll,-41729", "radio": true, "checked": size >= 40 }),
                    json!({ "id": "iconsize.medium", "text": "@ExplorerFrame.dll,-41730", "radio": true, "checked": (24..40).contains(&size) }),
                    json!({ "id": "iconsize.smallicon", "text": "@ExplorerFrame.dll,-41731", "radio": true, "checked": size < 24 }),
                    json!({ "separator": true }),
                    json!({ "id": "showdesktop", "text": t(self.lang, S::ShowDesktopIcons), "checkable": true, "checked": self.settings.show_icons }),
                ];
                let sort = vec![
                    json!({ "id": "sort.name", "text": "@propsys.dll,-38671" }),
                    json!({ "id": "sort.size", "text": "@propsys.dll,-38657" }),
                    json!({ "id": "sort.type", "text": "@propsys.dll,-38413" }),
                    json!({ "id": "sort.modified", "text": "@propsys.dll,-38407" }),
                ];
                // In a folder box: its folder's menu (New, Paste go there).
                if let Some(f) = bid.and_then(|id| self.box_by_id(id)).filter(|b| b.is_folder()).map(|b| b.folder_now().to_string()) {
                    return json!({ "kind": "background", "folder": f, "desktop": false, "shift": key_down(VK_SHIFT), "undo": self.hist.can_undo(), "redo": self.hist.can_redo(), "extra": extra, "own": { "view": view, "arrange": sort } });
                }
                json!({ "kind": "background", "folder": self.folder_arg(), "desktop": true, "shift": key_down(VK_SHIFT), "undo": self.hist.can_undo(), "redo": self.hist.can_redo(), "extra": extra, "own": { "view": view, "arrange": sort } })
            }
        }
    }

    /// What the settings window shows that the layout can change (not the slider positions it
    /// sets itself, see `apply_setting`).
    fn settings_sig(&self) -> String {
        let boxes: Vec<_> = self
            .layout
            .boxes
            .iter()
            .map(|b| (b.id, b.title(), &b.style, b.az, &b.tint, &b.blend, b.stay_visible, &b.folder, b.tabs.iter().map(|t| (t.id, &t.title)).collect::<Vec<_>>()))
            .collect();
        let sorting = (&self.layout.rules, self.layout.keep_sorted, self.layout.new_items_to);
        let look = (&self.settings.look.title_bar, &self.settings.look.align, self.settings.lock_all, self.settings.privacy.allowed, &self.settings.extras, &self.settings.az, &self.settings.search, &self.settings.icons, &self.settings.hide, &self.settings.saved, &self.settings.above, self.above_hotkey_ok, &self.settings.update, crate::update::status());
        serde_json::to_string(&(boxes, &self.settings.default_style, &self.settings.language, sorting, &self.settings.tabs, &self.settings.rollup, look)).unwrap_or_default()
    }

    fn autostart_on() -> bool {
        let run = reg::get_string(reg::RUN, crate::RUN_VALUE).is_some();
        let approved = reg::get_binary(reg::RUN_APPROVED, crate::RUN_VALUE)
            .map(|b| b.first().map(|x| x & 1 == 0).unwrap_or(true))
            .unwrap_or(true);
        run && approved
    }

    /// Send the settings window its data; `open` shows it, otherwise an open one is refreshed.
    pub fn send_settings(&mut self, open: bool, focus_box: Option<u32>) {
        let Some(h) = self.host.clone() else { return };
        let style = |st: &BoxStyle| json!({ "color": st.color.map(|c| format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)).unwrap_or_default(), "opacity": st.opacity, "blur": st.blur });
        let strings: serde_json::Map<String, Value> = settings_strings(self.lang).into_iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
        let colors: Vec<Value> = BOX_COLORS
            .iter()
            .enumerate()
            .map(|(i, (_, _, (r, g, b)))| json!({ "name": color_name(self.lang, i), "rgb": format!("#{r:02X}{g:02X}{b:02X}") }))
            .collect();
        let boxes: Vec<Value> = self.layout.boxes.iter().map(|b| json!({ "id": b.id, "title": b.title(), "style": style(&b.style), "az": b.az, "tint": b.tint, "blend": b.blend, "stayVisible": b.stay_visible })).collect();
        // Tabs rules can send icons to: each tab by its title (a plain box: the box title).
        let targets: Vec<Value> = self
            .layout
            .boxes
            .iter()
            .flat_map(|b| {
                b.tabs.iter().map(move |t| {
                    let name = if b.tabs.len() > 1 {
                        format!("{}   ({})", t.title, b.tabs.iter().map(|x| x.title.as_str()).collect::<Vec<_>>().join(" · "))
                    } else {
                        t.title.clone()
                    };
                    json!({ "id": t.id, "name": name })
                })
            })
            .collect();
        let categories: Vec<Value> = deskboxes_core::rules::CATEGORIES
            .iter()
            .map(|(id, _)| json!({ "id": id, "name": category_name(self.lang, id) }))
            .collect();
        let rules: Vec<Value> = self
            .layout
            .rules
            .iter()
            .map(|r| json!({ "id": r.id, "enabled": r.enabled, "kind": r.kind, "op": r.op, "value": r.value, "target": r.target }))
            .collect();
        let body = json!({
            "open": open,
            "focusBox": focus_box,
            "page": if open { self.settings_page.take() } else { None },
            "targets": targets,
            "categories": categories,
            "sorting": { "keep": self.layout.keep_sorted, "newTo": self.layout.new_items_to.unwrap_or(0), "rules": rules },
            "strings": strings,
            "colors": colors,
            "app": { "autostart": Self::autostart_on(), "language": self.settings.language, "version": env!("CARGO_PKG_VERSION"), "lockAll": self.settings.lock_all, "privacy": self.settings.privacy.allowed },
            "rollup": { "edges": self.settings.rollup.edges, "reveal": self.settings.rollup.reveal, "doubleClick": self.settings.rollup.double_click },
            "tabs": { "style": self.settings.tabs.style, "hoverSwitch": self.settings.tabs.hover_switch, "combine": self.settings.tabs.combine },
            "az": { "letterBar": self.settings.az.letter_bar, "letterTags": self.settings.az.letter_tags },
            "icons": { "tint": self.settings.icons.tint, "tintStrength": self.settings.icons.tint_strength, "blend": self.settings.icons.blend, "blendStrength": self.settings.icons.blend_strength, "desktop": self.settings.icons.desktop, "blendBoxes": self.settings.icons.blend_boxes, "fullHover": self.settings.icons.full_on_hover, "fullActive": self.settings.icons.full_when_active },
            "search": { "field": self.settings.search.field, "typeToSearch": self.settings.search.type_to_search },
            "hide": { "doubleClick": self.settings.hide.double_click },
            "update": {
                "auto": self.settings.update.auto,
                "status": crate::update::status(),
                "version": crate::update::current(),
            },
            "above": { "hotkey": self.settings.above.hotkey, "corner": self.settings.above.corner, "dim": self.settings.above.dim, "hotkeyOk": self.above_hotkey_ok },
            "folderBoxes": {
                "known": items::known_folders().iter().map(|(n, p)| json!({ "id": n, "path": p.display().to_string(), "name": t(self.lang, match *n {
                    "documents" => S::KfDocuments,
                    "downloads" => S::KfDownloads,
                    "pictures" => S::KfPictures,
                    "music" => S::KfMusic,
                    _ => S::KfVideos,
                }) })).collect::<Vec<_>>(),
                "list": self.layout.boxes.iter().filter(|b| b.is_folder()).map(|b| json!({ "id": b.id, "title": b.title(), "path": b.folder })).collect::<Vec<_>>(),
            },
            "saved": {
                "inMenu": self.settings.saved.in_menu,
                "list": self.saved_layouts().iter().map(|s| json!({ "id": s.id, "when": s.when, "boxes": s.boxes })).collect::<Vec<_>>(),
                "backups": self.backup_layouts().iter().map(|s| json!({ "id": s.id, "when": s.when, "boxes": s.boxes })).collect::<Vec<_>>(),
            },
            "extras": { "smoothScroll": self.settings.extras.smooth_scroll, "dragCount": self.settings.extras.drag_count, "liftBox": self.settings.extras.lift_box, "ctrlWheel": self.settings.extras.ctrl_wheel_size },
            "look": {
                "titleBar": self.settings.look.title_bar,
                "band": self.settings.look.band,
                "align": self.settings.look.align,
                "titleSize": self.settings.look.title_size,
                "radius": self.settings.look.radius,
                "titlesHover": self.settings.look.titles_on_hover,
                "titleFont": self.settings.look.title_font,
                "titleWeight": self.settings.look.title_weight,
                "outline": self.settings.look.outline,
                "outlineW": self.settings.look.outline_w,
                "titleButtons": self.settings.look.title_buttons,
                "scrollHover": self.settings.look.scroll_on_hover,
                "preset": self.settings.look.preset(&self.settings.tabs.style).unwrap_or(""),
                "presets": deskboxes_core::model::LOOK_PRESETS.iter().map(|p| json!({
                    "id": p.id, "titleBar": p.title_bar, "band": p.band, "align": p.align, "tabs": p.tabs, "titleSize": p.title_size, "radius": p.radius,
                })).collect::<Vec<_>>(),
            },
            "default": style(&self.settings.default_style),
            "boxes": boxes,
        });
        if open {
            if let Some(c) = &self.host_child {
                unsafe {
                    let _ = AllowSetForegroundWindow(c.id());
                }
            }
        }
        if let Err(e) = h.send("settings", body) {
            log::warn!("settings: {e}");
        }
        self.settings_sent = self.settings_sig();
    }

    /// A change made in the settings window, applied live.
    fn apply_setting(&mut self, v: &Value) {
        let (scope, key, val) = (v["scope"].as_str().unwrap_or(""), v["key"].as_str().unwrap_or(""), &v["value"]);
        let id = v["box"].as_f64().unwrap_or(0.0) as u32;
        if scope == "pick" {
            return self.on_picked_color(v);
        }
        log::info!("setting {scope}.{key} box={id} = {val}");
        let set_style = |st: &mut BoxStyle| match key {
            "color" => {
                let h = val.as_str().unwrap_or("").trim_start_matches('#');
                st.color = u32::from_str_radix(h, 16).ok().filter(|_| h.len() == 6).map(|x| Rgba {
                    r: (x >> 16) as u8,
                    g: (x >> 8) as u8,
                    b: x as u8,
                    a: 255,
                });
            }
            "opacity" => st.opacity = val.as_f64().unwrap_or(0.6).clamp(0.0, 1.0) as f32,
            "blur" => st.blur = val.as_f64().unwrap_or(60.0).clamp(0.0, 200.0) as f32,
            _ => {}
        };
        match (scope, key) {
            ("app", "autostart") => {
                if val.as_bool().unwrap_or(false) {
                    let exe = std::env::current_exe().unwrap_or_default();
                    let wd = exe.with_file_name("deskboxes-watchdog.exe");
                    let r = reg::set_string(reg::RUN, crate::RUN_VALUE, &format!("\"{}\" --launch", wd.display()));
                    // Turned off in Task Manager's Startup apps: on again.
                    reg::delete_value(reg::RUN_APPROVED, crate::RUN_VALUE);
                    log::info!("autostart on: {r:?}");
                } else {
                    reg::delete_value(reg::RUN, crate::RUN_VALUE);
                    log::info!("autostart off");
                }
            }
            ("app", "language") => {
                self.settings.language = val.as_str().unwrap_or("").to_string();
                self.lang = Lang::pick(&self.settings.language, unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() });
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.send_settings(false, None);
            }
            ("app", "disable") => self.on_command("dbx.disable", None),
            ("app", "privacy") => {
                self.settings.privacy.allowed = val.as_bool().unwrap_or(true);
                if !self.settings.privacy.allowed {
                    self.set_privacy(false);
                }
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.settings_sent = self.settings_sig();
            }
            ("app", "lockAll") => {
                self.settings.lock_all = val.as_bool().unwrap_or(false);
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.settings_sent = self.settings_sig();
                self.invalidate();
            }
            ("extras", k) => {
                let on = val.as_bool().unwrap_or(true);
                let ex = &mut self.settings.extras;
                match k {
                    "smoothScroll" => ex.smooth_scroll = on,
                    "dragCount" => ex.drag_count = on,
                    "liftBox" => ex.lift_box = on,
                    "ctrlWheel" => ex.ctrl_wheel_size = on,
                    _ => {}
                }
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.settings_sent = self.settings_sig();
                self.invalidate();
            }
            ("az", k) => {
                let on = val.as_bool().unwrap_or(true);
                match k {
                    "letterBar" => self.settings.az.letter_bar = on,
                    _ => self.settings.az.letter_tags = on,
                }
                crate::az::set_bar(self.settings.az.letter_bar);
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.settings_sent = self.settings_sig();
                self.invalidate();
            }
            ("search", k) => {
                let on = val.as_bool().unwrap_or(true);
                match k {
                    "field" => self.settings.search.field = on,
                    _ => self.settings.search.type_to_search = on,
                }
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.settings_sent = self.settings_sig();
                self.invalidate();
            }
            ("icons", k) => {
                let ic = &mut self.settings.icons;
                match k {
                    "tint" => ic.tint = val.as_str().unwrap_or("").to_string(),
                    "tintStrength" => ic.tint_strength = val.as_f64().unwrap_or(0.7).clamp(0.0, 1.0) as f32,
                    "blend" => ic.blend = val.as_bool().unwrap_or(false),
                    "blendStrength" => ic.blend_strength = val.as_f64().unwrap_or(0.8).clamp(0.0, 1.0) as f32,
                    "desktop" => ic.desktop = val.as_bool().unwrap_or(false),
                    "blendBoxes" => ic.blend_boxes = val.as_bool().unwrap_or(true),
                    "fullHover" => ic.full_on_hover = val.as_bool().unwrap_or(true),
                    "fullActive" => ic.full_when_active = val.as_bool().unwrap_or(true),
                    _ => {}
                }
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.settings_sent = self.settings_sig();
                self.invalidate();
            }
            ("box", "tint") | ("box", "blend") => {
                let v = val.as_str().unwrap_or("").to_string();
                let tint = key == "tint";
                self.change_layout(|l, _| {
                    if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) {
                        if tint {
                            b.tint = v;
                        } else {
                            b.blend = v;
                        }
                    }
                });
            }
            ("saved", k) => {
                let sid = v["box"].as_f64().unwrap_or(0.0) as u64;
                match k {
                    "now" => self.save_layout_now(),
                    "restore" => self.restore_layout(sid),
                    "delete" => self.delete_saved_layout(sid),
                    _ => {
                        self.settings.saved.in_menu = val.as_bool().unwrap_or(true);
                        let _ = persist::save_settings(&self.paths.settings, &self.settings);
                        self.settings_sent = self.settings_sig();
                    }
                }
            }
            ("folderbox", "new") => {
                let p = val.as_str().unwrap_or("").to_string();
                let at = Pt { x: self.work.x + 60.0, y: self.work.y + 60.0 };
                self.new_folder_box(&p, at);
                self.send_settings(false, None);
            }
            ("update", k) => match k {
                "auto" => {
                    self.settings.update.auto = val.as_bool().unwrap_or(true);
                    crate::update::set_auto(self.settings.update.auto);
                    let _ = persist::save_settings(&self.paths.settings, &self.settings);
                    self.settings_sent = self.settings_sig();
                }
                "now" => crate::update::check_now(),
                "install" => {
                    if crate::update::install_pending(&self.paths.update, self.args.desktop_dir.is_none() && !self.args.window) {
                        unsafe {
                            let _ = PostMessageW(Some(self.msg_hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                        }
                    }
                }
                _ => {}
            },
            ("above", k) => {
                match k {
                    "hotkey" => self.settings.above.hotkey = val.as_str().unwrap_or("").to_string(),
                    "corner" => self.settings.above.corner = val.as_str().unwrap_or("").to_string(),
                    "now" => {
                        self.set_above(true);
                        return;
                    }
                    _ => self.settings.above.dim = val.as_bool().unwrap_or(true),
                }
                self.register_above_hotkey();
                self.above_poll();
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.send_settings(false, None);
                self.invalidate();
            }
            ("hide", _) => {
                self.settings.hide.double_click = val.as_bool().unwrap_or(false);
                if !self.settings.hide.double_click {
                    self.set_tucked(false);
                }
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.settings_sent = self.settings_sig();
                self.invalidate();
            }
            ("box", "stayVisible") => {
                let on = val.as_bool().unwrap_or(false);
                self.change_layout(|l, _| {
                    if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) {
                        b.stay_visible = on;
                    }
                });
            }
            ("box", "az") => {
                let on = val.as_bool().unwrap_or(false);
                self.scroll.remove(&id);
                self.change_layout(|l, _| {
                    if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) {
                        b.az = on;
                    }
                });
            }
            ("tabs", k) => {
                match k {
                    "hoverSwitch" => self.settings.tabs.hover_switch = val.as_bool().unwrap_or(false),
                    "combine" => self.settings.tabs.combine = val.as_bool().unwrap_or(true),
                    _ => self.settings.tabs.style = val.as_str().unwrap_or("pills").to_string(),
                }
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.invalidate();
            }
            ("look", k) => {
                let lk = &mut self.settings.look;
                let mut refresh = false;
                match k {
                    "preset" => {
                        if let Some(pr) = deskboxes_core::model::LOOK_PRESETS.iter().find(|p| Some(p.id) == val.as_str()) {
                            lk.title_bar = pr.title_bar.into();
                            lk.band = pr.band;
                            lk.align = pr.align.into();
                            lk.title_size = pr.title_size;
                            lk.radius = pr.radius;
                            self.settings.tabs.style = pr.tabs.into();
                            refresh = true;
                        }
                    }
                    "titleBar" => lk.title_bar = val.as_str().unwrap_or("band").to_string(),
                    "align" => lk.align = val.as_str().unwrap_or("center").to_string(),
                    "band" => lk.band = val.as_f64().unwrap_or(0.22).clamp(0.0, 1.0) as f32,
                    "titleSize" => lk.title_size = val.as_f64().unwrap_or(13.0).clamp(9.0, 24.0) as f32,
                    "radius" => lk.radius = val.as_f64().unwrap_or(8.0).clamp(0.0, 20.0) as f32,
                    "titlesHover" => lk.titles_on_hover = val.as_bool().unwrap_or(false),
                    "scrollHover" => lk.scroll_on_hover = val.as_bool().unwrap_or(false),
                    "titleFont" => lk.title_font = val.as_str().unwrap_or("").to_string(),
                    "titleWeight" => lk.title_weight = val.as_str().unwrap_or("semibold").to_string(),
                    "outline" => lk.outline = val.as_str().unwrap_or("").to_string(),
                    "outlineW" => lk.outline_w = val.as_f64().unwrap_or(1.0).clamp(0.5, 4.0) as f32,
                    "titleButtons" => lk.title_buttons = val.as_bool().unwrap_or(false),
                    _ => {}
                }
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.apply_look();
                if refresh {
                    self.send_settings(false, None);
                } else {
                    self.settings_sent = self.settings_sig();
                }
            }
            ("rollup", k) => {
                match k {
                    "edges" => self.settings.rollup.edges = val.as_bool().unwrap_or(true),
                    "doubleClick" => self.settings.rollup.double_click = val.as_bool().unwrap_or(true),
                    _ => self.settings.rollup.reveal = val.as_str().unwrap_or("hover").to_string(),
                }
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.invalidate();
            }
            ("default", "applyAll") => {
                let st = self.settings.default_style.clone();
                self.change_layout(|l, _| {
                    for b in l.boxes.iter_mut() {
                        b.style = st.clone();
                    }
                });
                self.send_settings(false, None);
            }
            ("default", _) => {
                set_style(&mut self.settings.default_style);
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.settings_sent = self.settings_sig();
                // New blur values need their backdrop when a box gets created.
                self.layout_changed();
            }
            ("box", "title") => {
                let text = val.as_str().unwrap_or("").trim().to_string();
                if let (false, Some(tab)) = (text.is_empty(), self.box_by_id(id).map(|b| b.active_tab)) {
                    self.change_layout(|l, _| {
                        layout::rename_tab(l, id, tab, &text);
                    });
                }
            }
            ("sorting", "keep") => {
                let on = val.as_bool().unwrap_or(false);
                self.change_layout(|l, _| l.keep_sorted = on);
            }
            ("sorting", "newTo") => {
                let t = val.as_f64().unwrap_or(0.0) as u32;
                self.change_layout(|l, _| l.new_items_to = (t != 0).then_some(t));
            }
            ("sorting", "now") => self.sort_now(),
            ("rules", "add") => {
                // A new rule for the first tab not yet used by a rule (or the first tab).
                let used: Vec<u32> = self.layout.rules.iter().map(|r| r.target).collect();
                let tabs: Vec<u32> = self.layout.boxes.iter().flat_map(|b| b.tabs.iter().map(|t| t.id)).collect();
                let target = tabs.iter().copied().find(|t| !used.contains(t)).or(tabs.first().copied()).unwrap_or(0);
                self.change_layout(|l, _| {
                    deskboxes_core::rules::new_rule(l, "type", "documents", target);
                });
            }
            ("rules", "delete") => self.change_layout(|l, _| l.rules.retain(|r| r.id != id)),
            ("rules", "up") | ("rules", "down") => {
                let up = key == "up";
                self.change_layout(|l, _| {
                    if let Some(i) = l.rules.iter().position(|r| r.id == id) {
                        let j = if up { i.checked_sub(1) } else { Some(i + 1).filter(|j| *j < l.rules.len()) };
                        if let Some(j) = j {
                            l.rules.swap(i, j);
                        }
                    }
                });
            }
            ("rules", "kind") => {
                let k = val.as_str().unwrap_or("type").to_string();
                self.change_layout(|l, _| {
                    if let Some(r) = l.rules.iter_mut().find(|r| r.id == id) {
                        deskboxes_core::rules::set_kind(r, &k);
                    }
                });
            }
            ("rules", field) => {
                // Edits of a rule's fields: the window already shows them (no rebuild while typing).
                let field = field.to_string();
                self.change_layout(|l, _| {
                    if let Some(r) = l.rules.iter_mut().find(|r| r.id == id) {
                        match field.as_str() {
                            "enabled" => r.enabled = val.as_bool().unwrap_or(true),
                            "op" => r.op = val.as_str().unwrap_or("").to_string(),
                            "value" => r.value = val.as_str().unwrap_or("").to_string(),
                            "target" => r.target = val.as_f64().unwrap_or(0.0) as u32,
                            _ => {}
                        }
                    }
                });
            }
            ("box", "remove") => self.change_layout(|l, g| {
                layout::delete_box(l, id, g);
            }),
            ("box", _) => {
                if let Some(b) = self.layout.boxes.iter_mut().find(|b| b.id == id) {
                    set_style(&mut b.style);
                }
                // The window already shows this value (it sent it): no refresh mid-drag.
                self.settings_sent = self.settings_sig();
                self.layout_changed();
            }
            _ => {}
        }
    }

    /// Our entries in item menus (they depend only on the items, so a prefetched menu for the
    /// same items stays valid).
    fn item_extra(&self, items: &[String]) -> Value {
        let mut v = vec![json!({ "separator": true, "placement": "beforeProperties" })];
        let folder_items = items.iter().any(|p| self.items.values().any(|i| i.source == Source::Folder && i.parsing_name == *p));
        if !folder_items {
            v.push(json!({ "id": "dbx.boxfromsel", "text": t(self.lang, S::BoxFromSelection), "glyph": "\u{E8A9}", "placement": "beforeProperties" }));
        }
        if let [one] = items {
            if self.can_show_folder(one) {
                v.push(json!({ "id": "dbx.asfolderbox", "text": t(self.lang, S::FolderBoxShow), "glyph": "\u{E8B7}", "placement": "beforeProperties" }));
            }
        }
        json!(v)
    }

    /// The box's own entries, at the top of its menu: rename, color, transparency, blur, roll up,
    /// detach tab, remove.
    fn box_menu(&self, b: &BoxModel, at_tab: Option<usize>) -> Vec<Value> {
        let l = self.lang;
        let st = &b.style;
        let mut colors = vec![json!({ "id": "dbx.color:auto", "text": t(l, S::ColorAuto), "radio": true, "checked": st.color.is_none() })];
        for (i, (_, _, (r, g, bl))) in BOX_COLORS.iter().enumerate() {
            let on = st.color.map(|c| (c.r, c.g, c.b) == (*r, *g, *bl)).unwrap_or(false);
            colors.push(json!({ "id": format!("dbx.color:{i}"), "text": color_name(l, i), "radio": true, "checked": on, "color": format!("#{r:02X}{g:02X}{bl:02X}") }));
        }
        let custom = st.color.filter(|c| !BOX_COLORS.iter().any(|(_, _, (r, g, bl))| (c.r, c.g, c.b) == (*r, *g, *bl)));
        colors.push(json!({ "separator": true }));
        let mut cc = json!({ "id": "dbx.color:custom", "text": t(l, S::CustomColor), "radio": true, "checked": custom.is_some() });
        if let Some(c) = custom {
            cc["color"] = json!(format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b));
        }
        colors.push(cc);
        let cur_t = ((1.0 - st.opacity.clamp(0.0, 1.0)) * 100.0).round() as u32;
        let transp: Vec<Value> = TRANSPARENCY_PRESETS
            .iter()
            .map(|p| json!({ "id": format!("dbx.opacity:{p}"), "text": format!("{p}%"), "radio": true, "checked": cur_t == *p }))
            .collect();
        let blurs: Vec<Value> = BLUR_PRESETS
            .iter()
            .map(|(v, s)| json!({ "id": format!("dbx.blur:{v}"), "text": t(l, *s), "radio": true, "checked": st.blur.round() as i32 == *v }))
            .collect();
        // Everything about the box sits in one submenu at the top of its menu: the tab (the one
        // right-clicked, or the open one) first, then the box.
        let mut sub = vec![];
        if b.tabs.len() > 1 {
            let ti = at_tab.unwrap_or(b.active_tab).min(b.tabs.len() - 1);
            let tc = b.tabs[ti].color;
            let mut tcolors = vec![json!({ "id": "dbx.tabcolor:auto", "text": t(l, S::ColorAuto), "radio": true, "checked": tc.is_none() })];
            for (i, (_, _, (r, g, bl))) in BOX_COLORS.iter().enumerate() {
                let on = tc.map(|c| (c.r, c.g, c.b) == (*r, *g, *bl)).unwrap_or(false);
                tcolors.push(json!({ "id": format!("dbx.tabcolor:{i}"), "text": color_name(l, i), "radio": true, "checked": on, "color": format!("#{r:02X}{g:02X}{bl:02X}") }));
            }
            let custom = tc.filter(|c| !BOX_COLORS.iter().any(|(_, _, (r, g, bl))| (c.r, c.g, c.b) == (*r, *g, *bl)));
            tcolors.push(json!({ "separator": true }));
            let mut cc = json!({ "id": "dbx.tabcolor:custom", "text": t(l, S::CustomColor), "radio": true, "checked": custom.is_some() });
            if let Some(c) = custom {
                cc["color"] = json!(format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b));
            }
            tcolors.push(cc);
            // A right-clicked tab has Rename / Remove at the top of the menu: not again here.
            let tab_top = at_tab.is_some() && !self.locks(b).tabs;
            if !tab_top {
                sub.push(json!({ "id": "dbx.renamebox", "text": t(l, S::RenameTab), "glyph": "\u{E8AC}" }));
            }
            sub.push(json!({ "id": "dbx.tabcolormenu", "text": t(l, S::TabColor), "glyph": "\u{E790}", "children": tcolors }));
            sub.push(json!({ "id": "dbx.addtab", "text": t(l, S::AddTab), "glyph": "\u{E710}" }));
            sub.push(json!({ "id": "dbx.detachtab", "text": t(l, S::SplitTab), "glyph": "\u{E8A7}" }));
            if !tab_top {
                sub.push(json!({ "id": "dbx.removetab", "text": t(l, S::RemoveTab), "glyph": "\u{E711}" }));
            }
            sub.push(json!({ "separator": true }));
            sub.push(json!({ "id": "dbx.colormenu", "text": t(l, S::BoxColor), "glyph": "\u{E771}", "children": colors }));
        } else {
            sub.push(json!({ "id": "dbx.renamebox", "text": t(l, S::RenameBox), "glyph": "\u{E8AC}" }));
            sub.push(json!({ "id": "dbx.addtab", "text": t(l, S::AddTab), "glyph": "\u{E710}" }));
            sub.push(json!({ "id": "dbx.colormenu", "text": t(l, S::ColorMenu), "glyph": "\u{E790}", "children": colors }));
        }
        sub.push(json!({ "id": "dbx.copystyle", "text": t(l, S::CopyStyle), "glyph": "\u{E8C8}" }));
        sub.push(json!({ "id": "dbx.pastestyle", "text": t(l, S::PasteStyle), "glyph": "\u{E77F}", "enabled": self.style_clip.is_some() }));
        // Sorting: icon types that go into this tab automatically.
        let tab_id = b.tabs.get(at_tab.unwrap_or(b.active_tab).min(b.tabs.len().saturating_sub(1))).map(|t| t.id).unwrap_or(0);
        let mut auto = vec![];
        for (cat, _) in deskboxes_core::rules::CATEGORIES.iter() {
            let on = self.layout.rules.iter().any(|r| r.enabled && r.kind == "type" && r.op == *cat && r.target == tab_id);
            auto.push(json!({ "id": format!("dbx.rulecat:{cat}"), "text": category_name(l, cat), "checkable": true, "checked": on }));
        }
        auto.push(json!({ "separator": true }));
        auto.push(json!({ "id": "dbx.newhere", "text": t(l, S::NewIconsHere), "checkable": true, "checked": self.layout.new_items_to == Some(tab_id) }));
        auto.push(json!({ "separator": true }));
        auto.push(json!({ "id": "dbx.sortnow", "text": t(l, S::SortNow), "glyph": "\u{E8CB}" }));
        auto.push(json!({ "id": "dbx.rules", "text": t(l, S::SortingRules), "glyph": "\u{E71C}" }));
        sub.push(json!({ "id": "dbx.automenu", "text": t(l, S::SortHere), "glyph": "\u{E8CB}", "children": auto }));
        sub.push(json!({ "id": "dbx.az", "text": t(l, S::AzIndex), "checkable": true, "checked": b.az }));
        if self.settings.hide.double_click {
            sub.push(json!({ "id": "dbx.stayvisible", "text": t(l, S::StayVisible), "checkable": true, "checked": b.stay_visible }));
        }
        sub.push(json!({ "separator": true }));
        sub.push(json!({ "id": "dbx.opacitymenu", "text": t(l, S::Transparency), "glyph": "\u{E81E}", "children": transp }));
        sub.push(json!({ "id": "dbx.blurmenu", "text": t(l, S::Blur), "glyph": "\u{E8B9}", "children": blurs }));
        // The icons: their tint and blending into the wallpaper (this box, or as all boxes).
        let mut tints = vec![
            json!({ "id": "dbx.tint:", "text": t(l, S::AsAllBoxes), "radio": true, "checked": b.tint.is_empty() }),
            json!({ "id": "dbx.tint:off", "text": t(l, S::BlurNone), "radio": true, "checked": b.tint == "off" }),
            json!({ "id": "dbx.tint:box", "text": t(l, S::BoxColor), "radio": true, "checked": b.tint == "box" }),
            json!({ "separator": true }),
        ];
        for (i, (_, _, (r, g, bl))) in BOX_COLORS.iter().enumerate() {
            let hx = format!("#{r:02X}{g:02X}{bl:02X}");
            tints.push(json!({ "id": format!("dbx.tint:{hx}"), "text": color_name(l, i), "radio": true, "checked": b.tint.eq_ignore_ascii_case(&hx), "color": hx }));
        }
        sub.push(json!({ "id": "dbx.tintmenu", "text": t(l, S::IconTint), "glyph": "\u{E7E6}", "children": tints }));
        let bl = b.blend.as_str();
        sub.push(json!({ "id": "dbx.blendmenu", "text": t(l, S::BlendIn), "glyph": "\u{E91B}", "children": [
            { "id": "dbx.blend:", "text": t(l, S::AsAllBoxes), "radio": true, "checked": bl.is_empty() },
            { "id": "dbx.blend:on", "text": t(l, S::On), "radio": true, "checked": bl == "on" },
            { "id": "dbx.blend:off", "text": t(l, S::Off), "radio": true, "checked": bl == "off" },
        ] }));
        sub.push(json!({ "separator": true }));
        let lk = b.locks;
        sub.push(json!({ "id": "dbx.lockmenu", "text": t(l, S::LockMenu), "glyph": "\u{E72E}", "children": [
            { "id": "dbx.lock:position", "text": t(l, S::LockPosition), "checkable": true, "checked": lk.position },
            { "id": "dbx.lock:size", "text": t(l, S::LockSize), "checkable": true, "checked": lk.size },
            { "id": "dbx.lock:icons", "text": t(l, S::LockIcons), "checkable": true, "checked": lk.icons },
            { "id": "dbx.lock:tabs", "text": t(l, S::LockTabs), "checkable": true, "checked": lk.tabs },
            { "separator": true },
            { "id": "dbx.lockall", "text": t(l, S::LockEverything), "checkable": true, "checked": self.settings.lock_all },
        ] }));
        sub.push(json!({ "separator": true }));
        sub.push(json!({ "id": "dbx.removebox", "text": t(l, S::DeleteBox), "glyph": "\u{E74D}" }));
        sub.push(json!({ "separator": true }));
        sub.push(json!({ "id": "dbx.boxsettings", "text": t(l, S::MoreSettings), "glyph": "\u{E713}" }));
        if b.is_folder() {
            // No tabs and no sorting here: it shows its folder as it is.
            sub.retain(|e| !matches!(e["id"].as_str(), Some("dbx.addtab" | "dbx.automenu")));
        }
        let mut top = vec![json!({ "id": "dbx.boxmenu", "text": t(l, S::BoxSettings), "glyph": "\u{E8A9}", "children": sub, "placement": "top" })];
        if b.is_folder() {
            if !b.folder_at.is_empty() {
                top.push(json!({ "id": "dbx.folderup", "text": t(l, S::FolderUp), "glyph": "\u{E72B}", "placement": "top" }));
            }
            top.push(json!({ "id": "dbx.folderexplore", "text": t(l, S::FolderOpenExplorer), "glyph": "\u{EC50}", "placement": "top" }));
        }
        // Right-clicked a tab: what you do to a tab, right there.
        if at_tab.is_some() && b.tabs.len() > 1 && !self.locks(b).tabs {
            top.push(json!({ "id": "dbx.renamebox", "text": t(l, S::RenameTab), "glyph": "\u{E8AC}", "placement": "top" }));
            top.push(json!({ "id": "dbx.removetab", "text": t(l, S::RemoveTab), "glyph": "\u{E711}", "placement": "top" }));
        }
        top.push(json!({ "separator": true, "placement": "top" }));
        top
    }

    /// After the selection changed: build its menu ahead in the host, so a menu opened from the
    /// keyboard only has to draw (a right-click prefetches at button-down instead).
    pub fn selection_changed_check(&mut self) {
        if self.host.is_none() || self.sel.set.is_empty() {
            return;
        }
        if self.prefetched_sel.0 != self.sel.set || self.prefetched_sel.1 != self.sel.focus {
            self.prefetched_sel = (self.sel.set.clone(), self.sel.focus.clone());
            unsafe {
                SetTimer(Some(self.msg_hwnd), T_PREFETCH, 150, None);
            }
        }
    }

    fn prefetch_selection(&mut self) {
        if self.menu.is_some() || self.sel.set.is_empty() || !matches!(self.drag, Drag::None) {
            return;
        }
        let items = self.selection_paths();
        self.send_prefetch(items, false);
    }

    /// The pointer rested on an unselected item: a right-click would select only it, so build
    /// that menu now and the click only has to draw it.
    fn prefetch_hover(&mut self) {
        if self.menu.is_some() || !matches!(self.drag, Drag::None) {
            return;
        }
        let Some(k) = self.hover.clone() else { return };
        if self.sel.is_selected(&k) {
            return;
        }
        if let Some(i) = self.items.get(&k) {
            let items = vec![i.parsing_name.clone()];
            self.send_prefetch(items, false);
        }
    }

    fn prefetch(&mut self, hit: &Hit) {
        if let Hit::Item { .. } = hit {
            let items = self.selection_paths();
            self.send_prefetch(items, key_down(VK_SHIFT));
        }
    }

    /// Ask the host to build an item menu ahead, unless it already holds that exact one.
    fn send_prefetch(&mut self, items: Vec<String>, shift: bool) {
        let Some(h) = &self.host else { return };
        if items.is_empty() || self.host_prefetch.as_ref().map(|(i, s)| *i == items && *s == shift).unwrap_or(false) {
            return;
        }
        let _ = h.send(
            "prefetch",
            json!({ "kind": "item", "items": items, "folder": self.folder_arg(), "shift": shift, "extra": self.item_extra(&items) }),
        );
        self.host_prefetch = Some((items, shift));
    }

    fn show_menu(&mut self, hit: Hit, at: Pt, keyboard: bool, new_rect: Option<Rect>) {
        let Some(h) = self.host.clone() else {
            log::warn!("context menu requested before the shell host is ready");
            return;
        };
        let mut req = self.menu_request(&hit, at, new_rect);
        // The host uses up (or drops) its prefetch on every show.
        self.host_prefetch = None;
        let (x, y) = self.to_screen(at);
        if let Some(o) = req.as_object_mut() {
            o.insert("x".into(), json!(x));
            o.insert("y".into(), json!(y));
            o.insert("owner".into(), json!(self.hwnd.0 as isize));
            o.insert("keyboard".into(), json!(keyboard));
        }
        // We received the click, so we may hand the foreground to the host's menu window.
        if let Some(c) = &self.host_child {
            unsafe {
                let _ = AllowSetForegroundWindow(c.id());
            }
        }
        match h.send("show", req) {
            Ok(id) => self.menu = Some((id, MenuCtx { at, hit, new_rect })),
            Err(e) => log::warn!("show menu: {e}"),
        }
    }

    fn on_host_msg(&mut self, v: Value) {
        let t = v["t"].as_str().unwrap_or("");
        match t {
            "ready" => {
                self.stats.host_ready_ms = self.t0.elapsed().as_secs_f64() * 1000.0;
                log::info!("PHASE host_ready ms={:.1}", self.stats.host_ready_ms);
            }
            "shown" => log::info!("menu shown in {} ms", v["ms"]),
            "command" => {
                let cmd = v["cmd"].as_str().unwrap_or("").to_string();
                let ctx = self.menu.as_ref().map(|(_, c)| c.clone());
                self.on_command(&cmd, ctx);
            }
            "closed" => {
                log::info!("menu closed invoked={} {}", v["invoked"], v["error"]);
                if v["invoked"].as_str().map(|s| !s.is_empty()).unwrap_or(false) {
                    let _ = self.worker.send(Req::Scan);
                }
                self.menu = None;
                // Showing a menu uses up the host's prefetch: build the selection's menu again, so
                // the next Menu key / Shift+F10 on the same selection does not build at show.
                if !self.sel.set.is_empty() {
                    self.prefetched_sel = (self.sel.set.clone(), self.sel.focus.clone());
                    unsafe {
                        let _ = SetTimer(Some(self.msg_hwnd), T_PREFETCH, 150, None);
                    }
                }
            }
            "log" => log::info!("host: {}", v["msg"]),
            "setting" => self.apply_setting(&v),
            _ => {}
        }
    }

    fn on_command(&mut self, cmd: &str, ctx: Option<MenuCtx>) {
        log::info!("command {cmd}");
        let at = ctx.as_ref().map(|c| c.at).unwrap_or(Pt { x: 100.0, y: 100.0 });
        let box_id = match ctx.as_ref().map(|c| &c.hit) {
            Some(Hit::Header { id, .. }) | Some(Hit::BoxBody { id }) | Some(Hit::Edge { id, .. }) => Some(*id),
            _ => None,
        };
        let tab_of = |c: &Option<MenuCtx>| match c.as_ref().map(|c| &c.hit) {
            Some(Hit::Header { tab: Some(t), .. }) => Some(*t),
            _ => None,
        };
        match cmd {
            "rename" => {
                if let Some(k) = self.sel.ordered(&self.order()).first().cloned() {
                    self.begin_rename(RenameTarget::Item(k));
                }
            }
            "refresh" => {
                let _ = self.worker.send(Req::Scan);
            }
            "undo" => self.undo(),
            "redo" => self.redo(),
            "paste" | "pastelink" => {
                let (x, y) = self.to_screen(at);
                self.pending_drop = Some((Instant::now(), at, box_id));
                self.known_before_drop = self.items.keys().cloned().collect();
                let _ = self.worker.send(Req::Paste {
                    folder: self.args.desktop_dir.as_ref().map(|d| d.display().to_string()),
                    link: cmd == "pastelink",
                    pt: (x, y),
                });
            }
            "dbx.newbox" => {
                let title = t(self.lang, S::NewBoxTitle).to_string();
                let r = ctx
                    .and_then(|c| c.new_rect)
                    .unwrap_or(Rect::new(at.x, at.y, self.grid.cell_w * 4.0 + 2.0 * PAD, header_h() + self.grid.cell_h * 2.0 + 8.0));
                let bounds = Rect::new(0.0, 0.0, self.size_dip.0, self.size_dip.1);
                let r = r.clamp_into(&self.work.clamp_into(&bounds));
                let style = self.settings.default_style.clone();
                let mut id = 0;
                self.change_layout(|l, _| id = layout::create_box(l, r, &title, style));
                if let Some(bi) = self.layout.boxes.iter().position(|b| b.id == id) {
                    let _ = bi;
                    self.begin_rename(RenameTarget::Box { id, tab: 0 });
                }
            }
            "dbx.boxfromsel" => {
                let keys = self.sel.ordered(&self.order());
                if keys.is_empty() {
                    return;
                }
                // Sized for the items (up to 4 columns), where the loose ones were (their top-left).
                let n = keys.len();
                let cols = n.clamp(2, 4);
                let rows = n.div_ceil(cols);
                let (w, h) = (cols as f32 * self.grid.cell_w + 2.0 * PAD, header_h() + rows as f32 * self.grid.cell_h + 8.0);
                let origin = keys
                    .iter()
                    .filter_map(|k| self.layout.loose.get(k))
                    .map(|c| self.grid.cell_rect(*c))
                    .fold(None, |acc: Option<Pt>, r| Some(acc.map_or(Pt { x: r.x, y: r.y }, |p| Pt { x: p.x.min(r.x), y: p.y.min(r.y) })))
                    .unwrap_or(at);
                let bounds = Rect::new(0.0, 0.0, self.size_dip.0, self.size_dip.1);
                let r = Rect::new(origin.x, origin.y, w, h).clamp_into(&self.work.clamp_into(&bounds));
                let title = t(self.lang, S::NewBoxTitle).to_string();
                let style = self.settings.default_style.clone();
                let mut id = 0;
                self.change_layout(|l, _| {
                    id = layout::create_box(l, r, &title, style);
                    layout::move_to_box(l, &keys, id, None);
                });
                self.begin_rename(RenameTarget::Box { id, tab: 0 });
            }
            "dbx.renamebox" => {
                if let Some(id) = box_id {
                    let tab = tab_of(&ctx).or(self.box_by_id(id).map(|b| b.active_tab)).unwrap_or(0);
                    self.begin_rename(RenameTarget::Box { id, tab });
                }
            }
            "dbx.asfolderbox" => {
                if let [p] = self.selection_paths().as_slice() {
                    let p = p.clone();
                    let at = Pt { x: at.x + 16.0, y: at.y + 16.0 };
                    self.new_folder_box(&p, at);
                }
            }
            "dbx.folderup" => {
                if let Some(id) = box_id {
                    self.folder_back(id);
                }
            }
            "dbx.folderexplore" => {
                if let Some(f) = box_id.and_then(|id| self.box_by_id(id)).map(|b| b.folder_now().to_string()) {
                    deskboxes_shell::open_default(vec![f], self.hwnd.0 as isize);
                }
            }
            c if c.starts_with("dbx.fbknown:") => {
                let p = c["dbx.fbknown:".len()..].to_string();
                self.new_folder_box(&p, at);
            }
            "dbx.removebox" => {
                if let Some(id) = box_id {
                    self.change_layout(|l, g| {
                        layout::delete_box(l, id, g);
                    });
                }
            }
            "dbx.detachtab" => {
                if let Some(id) = box_id {
                    let tab = tab_of(&ctx).or(self.box_by_id(id).map(|b| b.active_tab)).unwrap_or(0);
                    if let Some(r) = self.box_by_id(id).map(|b| b.rect.offset(32.0, 32.0)) {
                        self.change_layout(|l, _| {
                            layout::split_tab(l, id, tab, r);
                        });
                    }
                }
            }
            "dbx.copystyle" => {
                if let Some(b) = box_id.and_then(|id| self.box_by_id(id)) {
                    self.style_clip = Some((b.style.clone(), b.tint.clone(), b.blend.clone()));
                }
            }
            "dbx.pastestyle" => {
                if let (Some(id), Some((st, tint, blend))) = (box_id, self.style_clip.clone()) {
                    self.change_layout(|l, _| {
                        if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) {
                            b.style = st;
                            b.tint = tint;
                            b.blend = blend;
                        }
                    });
                }
            }
            c if c.starts_with("dbx.tint:") || c.starts_with("dbx.blend:") => {
                let (k, v) = c["dbx.".len()..].split_once(':').unwrap_or(("", ""));
                if let Some(id) = box_id {
                    let (k, v) = (k.to_string(), v.to_string());
                    self.change_layout(|l, _| {
                        let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) else { return };
                        if k == "tint" {
                            b.tint = v;
                        } else {
                            b.blend = v;
                        }
                    });
                }
            }
            "dbx.savenow" => self.save_layout_now(),
            "dbx.savedpage" => {
                self.settings_page = Some("saved");
                self.send_settings(true, None);
            }
            c if c.starts_with("dbx.restore:") => {
                if let Ok(id) = c["dbx.restore:".len()..].parse::<u64>() {
                    self.restore_layout(id);
                }
            }
            "dbx.stayvisible" => {
                if let Some(id) = box_id {
                    self.change_layout(|l, _| {
                        if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) {
                            b.stay_visible = !b.stay_visible;
                        }
                    });
                }
            }
            "dbx.az" => {
                if let Some(id) = box_id {
                    self.scroll.remove(&id);
                    self.change_layout(|l, _| {
                        if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) {
                            b.az = !b.az;
                        }
                    });
                }
            }
            "dbx.privacy" => {
                let on = !self.settings.privacy.on && self.settings.privacy.allowed;
                self.set_privacy(on);
            }
            "dbx.lockall" => {
                self.settings.lock_all = !self.settings.lock_all;
                log::info!("lock everything: {}", self.settings.lock_all);
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.send_settings(false, None);
                self.invalidate();
            }
            c if c.starts_with("dbx.lock:") => {
                if let Some(id) = box_id {
                    let what = c.trim_start_matches("dbx.lock:").to_string();
                    self.change_layout(|l, _| {
                        if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) {
                            let f = match what.as_str() {
                                "position" => &mut b.locks.position,
                                "size" => &mut b.locks.size,
                                "icons" => &mut b.locks.icons,
                                _ => &mut b.locks.tabs,
                            };
                            *f = !*f;
                        }
                    });
                }
            }
            "dbx.addtab" => {
                if let Some(id) = box_id {
                    self.add_tab(id);
                }
            }
            "dbx.removetab" => {
                if let Some(id) = box_id {
                    let tab = tab_of(&ctx).or(self.box_by_id(id).map(|b| b.active_tab)).unwrap_or(0);
                    self.change_layout(|l, g| {
                        layout::remove_tab(l, id, tab, g);
                    });
                }
            }
            c @ ("dbx.tabcolor:custom" | "dbx.color:custom") => {
                if let Some(id) = box_id {
                    let tab = if c == "dbx.tabcolor:custom" { Some(tab_of(&ctx).or(self.box_by_id(id).map(|b| b.active_tab)).unwrap_or(0)) } else { None };
                    self.open_color_picker(id, tab);
                }
            }
            c if c.starts_with("dbx.tabcolor:") => {
                if let Some(id) = box_id {
                    let tab = tab_of(&ctx).or(self.box_by_id(id).map(|b| b.active_tab)).unwrap_or(0);
                    let color = c["dbx.tabcolor:".len()..]
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| BOX_COLORS.get(i))
                        .map(|(_, _, (r, g, bl))| Rgba { r: *r, g: *g, b: *bl, a: 255 });
                    self.change_layout(|l, _| {
                        if let Some(t) = l.boxes.iter_mut().find(|b| b.id == id).and_then(|b| b.tabs.get_mut(tab)) {
                            t.color = color;
                        }
                    });
                }
            }
            "dbx.sortnow" => self.sort_now(),
            c if c.starts_with("sort.") => self.sort_desktop(&c["sort.".len()..]),
            "iconsize.large" => self.set_icon_size(48),
            "iconsize.medium" => self.set_icon_size(32),
            "iconsize.smallicon" => self.set_icon_size(16),
            "showdesktop" => {
                self.settings.show_icons = !self.settings.show_icons;
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                self.sel.clear();
                self.invalidate();
            }
            "dbx.rules" => {
                self.settings_page = Some("rules");
                self.send_settings(true, None);
            }
            "dbx.newhere" => {
                if let Some(id) = box_id {
                    let tab = tab_of(&ctx).or(self.box_by_id(id).map(|b| b.active_tab)).unwrap_or(0);
                    if let Some(tid) = self.box_by_id(id).and_then(|b| b.tabs.get(tab)).map(|t| t.id) {
                        self.change_layout(|l, _| l.new_items_to = if l.new_items_to == Some(tid) { None } else { Some(tid) });
                    }
                }
            }
            c if c.starts_with("dbx.rulecat:") => {
                // Toggle "icons of this type go here"; turning it on sorts them in at once.
                if let Some(id) = box_id {
                    let tab = tab_of(&ctx).or(self.box_by_id(id).map(|b| b.active_tab)).unwrap_or(0);
                    let cat = c["dbx.rulecat:".len()..].to_string();
                    if let Some(tid) = self.box_by_id(id).and_then(|b| b.tabs.get(tab)).map(|t| t.id) {
                        let exists = self.layout.rules.iter().any(|r| r.kind == "type" && r.op == cat && r.target == tid);
                        let its: Vec<Item> = self.items.values().cloned().collect();
                        self.change_layout(|l, _| {
                            if exists {
                                l.rules.retain(|r| !(r.kind == "type" && r.op == cat && r.target == tid));
                            } else {
                                // The same type sent elsewhere before: this box replaces it.
                                l.rules.retain(|r| !(r.kind == "type" && r.op == cat));
                                deskboxes_core::rules::new_rule(l, "type", &cat, tid);
                                let mut refs: Vec<&Item> = its.iter().collect();
                                refs.sort_by_cached_key(|i| i.display_name.to_lowercase());
                                deskboxes_core::rules::apply(l, &refs, false, deskboxes_core::rules::now_filetime());
                            }
                        });
                    }
                }
            }
            "dbx.settings" => self.send_settings(true, None),
            "dbx.boxsettings" => {
                self.settings_page = Some("boxes");
                self.send_settings(true, box_id);
            }
            "dbx.rollup" => {
                if let Some(id) = box_id {
                    self.change_layout(|l, _| {
                        if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) {
                            b.rolled_up = !b.rolled_up;
                        }
                    });
                }
            }
            c if c.starts_with("dbx.color:") || c.starts_with("dbx.opacity:") || c.starts_with("dbx.blur:") => {
                let (k, v) = c["dbx.".len()..].split_once(':').unwrap_or(("", ""));
                if let Some(id) = box_id {
                    let (k, v) = (k.to_string(), v.to_string());
                    self.change_layout(|l, _| {
                        let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) else { return };
                        match k.as_str() {
                            "color" => b.style.color = v.parse::<usize>().ok().and_then(|i| BOX_COLORS.get(i)).map(|(_, _, (r, g, bl))| Rgba { r: *r, g: *g, b: *bl, a: 255 }),
                            "opacity" => b.style.opacity = 1.0 - v.parse::<f32>().unwrap_or(40.0).clamp(0.0, 100.0) / 100.0,
                            _ => b.style.blur = v.parse::<f32>().unwrap_or(60.0).max(0.0),
                        }
                    });
                }
            }
            "dbx.disable" => {
                self.settings.enabled = false;
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                log::info!("disabled by the user: restoring icons and exiting");
                unsafe {
                    let _ = PostMessageW(Some(self.msg_hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            other => log::info!("command {other} is not implemented in Phase 1"),
        }
    }

    /// Desktop "Sort by": the loose icons in that order (system icons, then folders, then
    /// files), filling the grid column by column. One undo step.
    fn sort_desktop(&mut self, by: &str) {
        let mut keys: Vec<&Item> = self.layout.loose.keys().filter_map(|k| self.items.get(k)).collect();
        let group = |i: &Item| match i.kind {
            ItemKind::Virtual => 0,
            ItemKind::Folder => 1,
            _ => 2,
        };
        let name = |i: &Item| i.display_name.to_lowercase();
        match by {
            "size" => keys.sort_by(|a, b| group(a).cmp(&group(b)).then(a.size.cmp(&b.size)).then_with(|| name(a).cmp(&name(b)))),
            "type" => keys.sort_by(|a, b| group(a).cmp(&group(b)).then_with(|| a.ext.to_lowercase().cmp(&b.ext.to_lowercase())).then_with(|| name(a).cmp(&name(b)))),
            "modified" => keys.sort_by(|a, b| group(a).cmp(&group(b)).then(b.modified.cmp(&a.modified)).then_with(|| name(a).cmp(&name(b)))),
            _ => keys.sort_by(|a, b| group(a).cmp(&group(b)).then_with(|| name(a).cmp(&name(b)))),
        }
        let order: Vec<ItemKey> = keys.into_iter().map(|i| i.key.clone()).collect();
        log::info!("sort the desktop by {by}: {} icon(s)", order.len());
        self.change_layout(|l, g| {
            layout::arrange_loose(l, g, &order);
        });
    }

    /// View > Large / Medium / Small icons: Explorer's desktop setting (so it stays the same
    /// without Deskboxes), then every icon again at the new size.
    fn set_icon_size(&mut self, dip: u32) {
        if self.icon_dip.round() as u32 == dip {
            return;
        }
        self.icon_dip = dip as f32;
        self.icon_px = (dip as f32 * self.scale).round() as u32;
        self.seeded = false;
        self.labels.clear();
        self.label_bmps.clear();
        log::info!("icon size -> {dip} DIP ({} px)", self.icon_px);
        let _ = self.worker.send(Req::IconSize { dip, px: self.icon_px });
        self.invalidate();
    }

    fn undo(&mut self) {
        if self.hist.undo(&mut self.layout) {
            self.layout_changed();
        }
    }

    fn redo(&mut self) {
        if self.hist.redo(&mut self.layout) {
            self.layout_changed();
        }
    }

    // ---------------------------------------------------------------- rename

    pub fn begin_rename(&mut self, target: RenameTarget) {
        self.end_rename(false);
        if let RenameTarget::Box { id, tab } = target {
            // Box titles are edited in place, in the header, drawn by us (themed with the box).
            let Some(b) = self.box_by_id(id) else { return };
            let title = b.tabs.get(tab).map(|t| t.title.clone()).unwrap_or_default();
            self.box_edit = Some(BoxEdit {
                id,
                tab,
                ed: TextEdit::new(&title),
                caret_on: true,
                selecting: false,
                hi: None,
            });
            unsafe {
                let root = GetAncestor(self.hwnd, GA_ROOT);
                let _ = SetForegroundWindow(root);
                let _ = SetFocus(Some(self.hwnd));
            }
            self.restart_caret();
            self.invalidate();
            return;
        }
        let (r, text) = match &target {
            RenameTarget::Item(k) => {
                let Some((_, cell, _)) = self.visible_cells().into_iter().find(|(x, _, _)| x == k) else { return };
                let (_, l) = self.item_parts(k, cell, true);
                (Rect::new(cell.x - 8.0, l.y - 2.0, cell.w + 16.0, self.line_h * 3.0 + 6.0), self.items[k].display_name.clone())
            }
            RenameTarget::Box { id, tab } => {
                let Some(b) = self.box_by_id(*id) else { return };
                let tw = b.rect.w / b.tabs.len().max(1) as f32;
                (
                    Rect::new(b.rect.x + tw * *tab as f32 + 8.0, b.rect.y + 4.0, tw - 16.0, header_h() - 8.0),
                    b.tabs.get(*tab).map(|t| t.title.clone()).unwrap_or_default(),
                )
            }
        };
        let (x, y) = self.to_screen(Pt { x: r.x, y: r.y });
        let (w, h) = ((r.w * self.scale) as i32, (r.h * self.scale) as i32);
        unsafe {
            let wt: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let multi = matches!(target, RenameTarget::Item(_));
            let style = WS_POPUP | WS_BORDER | WS_VISIBLE | WINDOW_STYLE((ES_AUTOHSCROLL | if multi { ES_CENTER | ES_MULTILINE | ES_AUTOVSCROLL } else { 0 }) as u32);
            let Ok(edit) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, w!("EDIT"), PCWSTR(wt.as_ptr()), style, x, y, w, h, Some(self.hwnd), None, None, None) else {
                return;
            };
            let font = CreateFontW(
                -((self.font.1 * self.scale) as i32),
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                &windows::core::HSTRING::from(self.font.0.as_str()),
            );
            SendMessageW(edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
            // Explorer selects the name without the extension.
            let sel_end = match (&target, text.rfind('.')) {
                (RenameTarget::Item(k), Some(dot)) if !self.items[k].is_folder() && dot > 0 => text[..dot].encode_utf16().count(),
                _ => text.encode_utf16().count(),
            };
            SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(sel_end as isize)));
            OLD_EDIT_PROC.with(|o| o.set(SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as isize)));
            let _ = SetForegroundWindow(edit);
            let _ = SetFocus(Some(edit));
            self.rename = Some(Rename { edit, target });
        }
    }

    fn restart_caret(&mut self) {
        if let Some(be) = self.box_edit.as_mut() {
            be.caret_on = true;
            unsafe {
                SetTimer(Some(self.msg_hwnd), T_CARET, GetCaretBlinkTime().clamp(250, 1200), None);
            }
        }
    }

    /// The in-place title field of a box tab: the whole title of a plain box, or the tab (at
    /// least 160 wide, kept inside the title).
    pub fn title_field(&self, b: &BoxModel, tab: usize) -> Rect {
        let r = self.shown_rect(b);
        if b.tabs.len() < 2 {
            return Rect::new(r.x + 6.0, r.y + 4.0, r.w - 12.0, header_h() - 8.0);
        }
        let t = self.header_layout(b).tabs.get(tab).copied().unwrap_or(Rect::new(r.x + 6.0, r.y + 4.0, 160.0, header_h() - 8.0));
        let w = t.w.max(160.0).min(r.w - 12.0);
        let x = t.x.min(r.right() - 6.0 - w).max(r.x + 6.0);
        Rect::new(x, r.y + 4.0, w, header_h() - 8.0)
    }

    /// The title being edited: its field, text layout, and the x the text starts at (centered
    /// while it fits, else scrolled so the caret stays visible).
    pub fn box_edit_layout(&self) -> Option<(Rect, TextLayout, f32)> {
        let be = self.box_edit.as_ref()?;
        let b = self.box_by_id(be.id)?;
        let field = self.title_field(b, be.tab);
        let l = self.gpu.label(&self.title_tf_bold, &be.ed.string(), 10_000.0, self.title_line_h, 0, false).ok()?;
        let w = deskboxes_render::text_width(&l);
        let inner = field.w - 16.0;
        let x = if w <= inner {
            field.x + (field.w - w) / 2.0
        } else {
            let cx = deskboxes_render::caret_x(&l, be.ed.utf16_pos(be.ed.caret));
            field.x + 8.0 - (cx - inner + 2.0).max(0.0)
        };
        Some((field, l, x))
    }

    /// Caret index in the title being edited for a point.
    fn box_edit_pos(&self, p: Pt) -> Option<usize> {
        let (_, l, x) = self.box_edit_layout()?;
        let u = deskboxes_render::caret_at(&l, p.x - x);
        self.box_edit.as_ref().map(|be| be.ed.char_pos(u))
    }

    /// A key while a box title is edited in place; every key is ours meanwhile.
    fn box_edit_key(&mut self, vk: VIRTUAL_KEY) {
        let (ctrl, shift) = (key_down(VK_CONTROL), key_down(VK_SHIFT));
        let owner = self.hwnd.0 as isize;
        let Some(be) = self.box_edit.as_mut() else { return };
        let ed = &mut be.ed;
        match vk {
            VK_RETURN => return self.end_rename(true),
            VK_ESCAPE => return self.end_rename(false),
            VK_BACK => ed.backspace(ctrl),
            VK_DELETE => ed.delete(ctrl),
            VK_LEFT => ed.left(ctrl, shift),
            VK_RIGHT => ed.right(ctrl, shift),
            VK_HOME | VK_UP => ed.move_to(0, shift),
            VK_END | VK_DOWN => {
                let n = ed.text.len();
                ed.move_to(n, shift)
            }
            _ if ctrl => match vk.0 as u8 {
                b'A' => ed.select_all(),
                b'C' | b'X' => {
                    let s = ed.selected();
                    if !s.is_empty() {
                        let _ = deskboxes_shell::view::set_clipboard_text(owner, &s);
                        if vk.0 as u8 == b'X' {
                            ed.backspace(false);
                        }
                    }
                }
                b'V' => {
                    if let Some(s) = deskboxes_shell::view::clipboard_text(owner) {
                        ed.insert(s.lines().next().unwrap_or(""));
                    }
                }
                _ => return,
            },
            _ => return, // characters arrive as WM_CHAR
        }
        self.restart_caret();
        self.invalidate();
    }

    fn box_edit_char(&mut self, c: u32) {
        let Some(be) = self.box_edit.as_mut() else { return };
        if (0xD800..0xDC00).contains(&c) {
            be.hi = Some(c as u16);
            return;
        }
        let s = if (0xDC00..0xE000).contains(&c) {
            be.hi.take().map(|h| String::from_utf16_lossy(&[h, c as u16]))
        } else {
            char::from_u32(c).filter(|ch| !ch.is_control()).map(|ch| ch.to_string())
        };
        if let Some(s) = s {
            be.ed.insert(&s);
            self.restart_caret();
            self.invalidate();
        }
    }

    pub fn end_rename(&mut self, commit: bool) {
        if let Some(be) = self.box_edit.take() {
            unsafe {
                let _ = KillTimer(Some(self.msg_hwnd), T_CARET);
            }
            let text = be.ed.string().trim().to_string();
            let (id, tab) = (be.id, be.tab);
            let old = self.box_by_id(id).and_then(|b| b.tabs.get(tab)).map(|t| t.title.clone());
            if commit && !text.is_empty() && old.as_deref() != Some(text.as_str()) {
                self.change_layout(|l, _| {
                    layout::rename_tab(l, id, tab, &text);
                });
            }
            self.invalidate();
        }
        let Some(r) = self.rename.take() else { return };
        unsafe {
            let n = GetWindowTextLengthW(r.edit);
            let mut buf = vec![0u16; n as usize + 1];
            let got = GetWindowTextW(r.edit, &mut buf);
            let text = String::from_utf16_lossy(&buf[..got as usize]).trim().to_string();
            let _ = DestroyWindow(r.edit);
            if !commit || text.is_empty() {
                return;
            }
            match r.target {
                RenameTarget::Item(k) => {
                    if let Some(it) = self.items.get(&k) {
                        if it.display_name != text && it.kind != ItemKind::Virtual {
                            let _ = self.worker.send(Req::Rename {
                                path: it.parsing_name.clone(),
                                new_name: text,
                            });
                        }
                    }
                }
                RenameTarget::Box { id, tab } => self.change_layout(|l, _| {
                    layout::rename_tab(l, id, tab, &text);
                }),
            }
        }
    }

    // ---------------------------------------------------------------- input

    fn on_lbutton_down(&mut self, p: Pt, dbl: bool) {
        self.hide_tip();
        if self.above_click_outside(p) {
            return;
        }
        if let Some(id) = self.folder_back_at(p) {
            self.end_rename(true);
            self.folder_back(id);
            return;
        }
        if let Some((id, which)) = self.title_btn_at(p) {
            self.end_rename(true);
            if which == 0 {
                self.toggle_roll(id);
            } else {
                self.show_menu(Hit::Header { id, tab: None, add: false }, p, false, None);
            }
            return;
        }
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
            SetCapture(self.hwnd);
        }
        // A click in the title field being edited moves the caret (double-click: a word).
        if self.box_edit_layout().map(|(f, _, _)| f.contains(p)).unwrap_or(false) {
            let i = self.box_edit_pos(p).unwrap_or(0);
            let shift = key_down(VK_SHIFT);
            if let Some(be) = self.box_edit.as_mut() {
                if dbl {
                    let (a, b) = be.ed.word_at(i);
                    be.ed.move_to(a, false);
                    be.ed.move_to(b, true);
                } else {
                    be.ed.move_to(i, shift);
                    be.selecting = true;
                }
            }
            self.restart_caret();
            self.invalidate();
            return;
        }
        if let Some(id) = self.az_bar_at(p) {
            self.drag = Drag::Scrub { id };
            self.az_jump(id, p.x);
            return;
        }
        let hit = self.hit_test(p);
        if self.search_click(p, &hit) {
            self.drag = Drag::None;
            return;
        }
        self.end_rename(true);
        let (ctrl, shift) = (key_down(VK_CONTROL), key_down(VK_SHIFT));
        if dbl {
            if let Hit::Item { key, .. } = &hit {
                if !self.sel.is_selected(key) {
                    self.sel.select_only(key);
                }
                // A folder in a folder box opens in the box.
                if let Some((id, path)) = self.folder_to_open(key) {
                    if self.folder_open(id, &path) {
                        self.drag = Drag::None;
                        return;
                    }
                }
                let paths = self.selection_paths();
                deskboxes_shell::open_default(paths, self.hwnd.0 as isize);
                // Found what was searched for: the search is done.
                self.close_search();
            } else if let (Hit::Header { id, add: false, .. }, true) = (&hit, self.settings.rollup.double_click) {
                // What looks open rolls up (also a box opened by hovering it); a rolled-up box
                // opens and stays open.
                let id = *id;
                let looks_open = self.box_by_id(id).map(|b| !b.rolled_up).unwrap_or(false) || self.revealed.contains(&id);
                self.revealed.remove(&id);
                self.pending_reveal = None;
                self.no_reveal = if looks_open { Some(id) } else { None };
                self.change_layout(|l, _| {
                    if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) {
                        b.rolled_up = looks_open;
                    }
                });
            } else if matches!(hit, Hit::Empty) && self.settings.hide.double_click {
                let on = !self.tucked.0;
                self.set_tucked(on);
            }
            self.drag = Drag::None;
            self.invalidate();
            return;
        }
        let was_selected = matches!(&hit, Hit::Item { key, .. } if self.sel.is_selected(key));
        match &hit {
            Hit::Item { key, .. } => {
                if shift {
                    self.sel.range(key, &self.order(), ctrl);
                } else if ctrl {
                    // toggled on button up (a Ctrl-drag copies the selection)
                    self.sel.focus = Some(key.clone());
                } else if !was_selected {
                    self.sel.select_only(key);
                } else {
                    self.sel.focus = Some(key.clone());
                }
            }
            Hit::Header { id, tab, add } => {
                if self.box_by_id(*id).map(|b| self.rolls_up(b)).unwrap_or(false) {
                    self.reveal(*id);
                }
                if let (Some(t), false) = (tab, *add) {
                    self.set_active_tab(*id, *t);
                }
            }
            _ => {}
        }
        self.drag = Drag::Pending {
            start: p,
            hit,
            ctrl,
            shift,
            was_selected,
        };
        self.invalidate();
    }

    fn on_mouse_move(&mut self, p: Pt) -> After {
        self.search_hover(p);
        let fb = self.folder_back_at(p);
        if fb != self.folder_back_hover {
            self.folder_back_hover = fb;
            self.invalidate();
        }
        let tb = self.title_btn_at(p);
        if tb != self.title_btn_hover {
            self.title_btn_hover = tb;
            self.invalidate();
        }
        if self.box_edit.as_ref().map(|b| b.selecting).unwrap_or(false) {
            if let Some(i) = self.box_edit_pos(p) {
                if let Some(be) = self.box_edit.as_mut() {
                    be.ed.move_to(i, true);
                }
                self.restart_caret();
                self.invalidate();
            }
            return After::None;
        }
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        let (cx, cy) = unsafe { (GetSystemMetrics(SM_CXDRAG) as f32 / self.scale, GetSystemMetrics(SM_CYDRAG) as f32 / self.scale) };
        self.drag = match drag {
            Drag::Pending {
                start,
                hit,
                ctrl,
                shift,
                was_selected,
            } => {
                if (p.x - start.x).abs() < cx && (p.y - start.y).abs() < cy {
                    Drag::Pending {
                        start,
                        hit,
                        ctrl,
                        shift,
                        was_selected,
                    }
                } else {
                    match hit {
                        Hit::Item { key, .. } => {
                            if !self.sel.is_selected(&key) {
                                if ctrl {
                                    self.sel.toggle(&key);
                                } else {
                                    self.sel.select_only(&key);
                                }
                            }
                            Drag::Items {
                                keys: self.sel.ordered(&self.order()),
                                start,
                                cur: p,
                            }
                        }
                        Hit::Header { id, tab: Some(_), add: false } if self.locks_of(id).tabs => Drag::None,
                        Hit::Header { id, tab: Some(tab), add: false } => Drag::Tab { id, tab, start, cur: p },
                        Hit::Header { add: true, .. } => Drag::None,
                        Hit::Header { id, .. } if self.locks_of(id).position => Drag::None,
                        Hit::Header { id, .. } => {
                            let orig = self.box_by_id(id).map(|b| b.rect).unwrap_or_default();
                            self.snap_delta = Pt { x: 0.0, y: 0.0 };
                            self.snap_glide = None;
                            self.move_t0 = Instant::now();
                            crate::anim::set(true);
                            Drag::MoveBox { id, start, orig, cur: p }
                        }
                        Hit::Edge { id, l, t, r, b } => {
                            let orig = self.box_by_id(id).map(|b| b.rect).unwrap_or_default();
                            Drag::Resize { id, start, orig, l, t, r, b }
                        }
                        hit @ (Hit::BoxBody { .. } | Hit::Empty) => {
                            let scope = if let Hit::BoxBody { id } = hit { Some(id) } else { None };
                            let base = if ctrl || shift { self.sel.set.clone() } else { BTreeSet::new() };
                            if !(ctrl || shift) {
                                self.sel.clear();
                            }
                            Drag::Rubber {
                                start,
                                cur: p,
                                base,
                                ctrl: ctrl || shift,
                                scope,
                            }
                        }
                    }
                }
            }
            Drag::Rubber { start, base, ctrl, scope, .. } => {
                let band = Rect::from_points(start, p);
                let hit: BTreeSet<ItemKey> = self
                    .visible_cells()
                    .into_iter()
                    .filter(|(k, cell, b)| {
                        if *b != scope {
                            return false;
                        }
                        let visible = b.and_then(|id| self.box_by_id(id)).map(|bx| Self::content_rect(bx).intersects(cell)).unwrap_or(true);
                        visible && self.hit_rect(k, *cell).intersects(&band)
                    })
                    .map(|(k, _, _)| k)
                    .collect();
                self.sel.rubber_band(&base, &hit, ctrl);
                Drag::Rubber { start, cur: p, base, ctrl, scope }
            }
            Drag::Items { keys, start, .. } => {
                // Leaving our surface for another window: hand over to OLE drag and drop.
                let (sx, sy) = self.to_screen(p);
                let over = unsafe { WindowFromPoint(POINT { x: sx, y: sy }) };
                let ours = over == self.hwnd || unsafe { GetAncestor(over, GA_ROOT) } == unsafe { GetAncestor(self.hwnd, GA_ROOT) } && !self.desktop_mode;
                let progman = hide::progman();
                let desktop_bg = self.desktop_mode && (over == progman || unsafe { GetAncestor(over, GA_ROOT) } == progman);
                if !ours && !desktop_bg {
                    let paths: Vec<String> = keys.iter().filter_map(|k| self.items.get(k)).map(|i| i.parsing_name.clone()).collect();
                    let ghost = self.ghost_pixels(&keys, start);
                    self.own_drag = Some((keys.clone(), start));
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    self.drag = Drag::None;
                    self.invalidate();
                    return After::OleDrag(paths, ghost);
                }
                self.hover_reveal(p, true);
                match self.hit_test(p) {
                    Hit::Header { id, tab: Some(t), .. } => self.arm_tab_switch(id, t, 450),
                    _ => self.pending_tab = None,
                }
                // Near the top or bottom of a box's icons: scroll it.
                if let Some(b) = self.drop_box(p).and_then(|id| self.box_by_id(id)) {
                    let (c, max, id) = (Self::content_rect(b), self.max_scroll(b), b.id);
                    let step = if p.y < c.y + 24.0 {
                        -12.0
                    } else if p.y > c.bottom() - 24.0 {
                        12.0
                    } else {
                        0.0
                    };
                    if step != 0.0 && max > 0.0 {
                        let s = self.scroll.entry(id).or_insert(0.0);
                        *s = (*s + step).clamp(0.0, max);
                    }
                }
                Drag::Items { keys, start, cur: p }
            }
            Drag::MoveBox { id, start, orig, .. } => {
                let raw = orig.offset(p.x - start.x, p.y - start.y);
                let mut r = raw;
                if !key_down(VK_SHIFT) {
                    // Snap to the screen edges, other boxes and the icon grid (Shift: no snapping).
                    let mut others: Vec<Rect> = self.layout.boxes.iter().filter(|b| b.id != id).map(|b| b.rect).collect();
                    let g = &self.grid;
                    for c in 0..=g.cols {
                        others.push(Rect::new(g.origin.x + c as f32 * g.cell_w, g.origin.y, 0.0, 0.0));
                    }
                    for rw in 0..=g.rows {
                        others.push(Rect::new(g.origin.x, g.origin.y + rw as f32 * g.cell_h, 0.0, 0.0));
                    }
                    r = layout::snap(r, &others, &self.work, 10.0);
                }
                // Snapping (or letting go) moves the box a few DIPs at once: glide them instead,
                // from where it is drawn now.
                let delta = Pt { x: r.x - raw.x, y: r.y - raw.y };
                if (delta.x - self.snap_delta.x).abs() > 0.5 || (delta.y - self.snap_delta.y).abs() > 0.5 {
                    let now_off = self.snap_offset(id);
                    let from = Pt { x: now_off.x + self.snap_delta.x - delta.x, y: now_off.y + self.snap_delta.y - delta.y };
                    self.snap_glide = Some((id, from, Instant::now()));
                    crate::anim::set(true);
                }
                self.snap_delta = delta;
                if let Some(b) = self.layout.boxes.iter_mut().find(|b| b.id == id) {
                    b.rect = r;
                }
                self.update_preview();
                Drag::MoveBox { id, start, orig, cur: p }
            }
            Drag::Resize { id, start, orig, l, t, r, b } => {
                let (dx, dy) = (p.x - start.x, p.y - start.y);
                let mut n = orig;
                let (minw, minh) = (self.grid.cell_w + 2.0 * PAD, header_h() + 40.0);
                if l {
                    let x = (orig.x + dx).min(orig.right() - minw);
                    n.w = orig.right() - x;
                    n.x = x;
                }
                if r {
                    n.w = (orig.w + dx).max(minw);
                }
                if t {
                    let y = (orig.y + dy).min(orig.bottom() - minh);
                    n.h = orig.bottom() - y;
                    n.y = y;
                }
                if b {
                    n.h = (orig.h + dy).max(minh);
                }
                // Whole icon columns and rows (Shift: free size).
                if !key_down(VK_SHIFT) {
                    let cols = ((n.w - 2.0 * PAD) / self.grid.cell_w).round().max(1.0);
                    let rows = ((n.h - header_h() - 8.0) / self.grid.cell_h).round().max(1.0);
                    let (w, h) = (cols * self.grid.cell_w + 2.0 * PAD, header_h() + rows * self.grid.cell_h + 8.0);
                    if l {
                        n.x = n.right() - w;
                    }
                    if t {
                        n.y = n.bottom() - h;
                    }
                    n.w = w;
                    n.h = h;
                }
                if let Some(bx) = self.layout.boxes.iter_mut().find(|bx| bx.id == id) {
                    bx.rect = n;
                }
                self.update_preview();
                Drag::Resize { id, start, orig, l, t, r, b }
            }
            Drag::RightRubber { start, .. } => Drag::RightRubber { start, cur: p },
            Drag::Scrub { id } => {
                self.az_jump(id, p.x);
                Drag::Scrub { id }
            }
            Drag::Tab { id, tab, start, .. } => {
                // Over another box: open it if it is rolled up, so its title can take the tab.
                if let Some(d) = self.header_target(id, p) {
                    self.hover_reveal(p, true);
                    let _ = d;
                }
                Drag::Tab { id, tab, start, cur: p }
            }
            Drag::None => {
                self.hover_reveal(p, false);
                self.blend_pointer(p);
                let hh = match self.hit_test(p) {
                    Hit::Header { id, tab, add } => Some((id, tab, add)),
                    _ => None,
                };
                if hh != self.hover_hdr {
                    self.hover_hdr = hh;
                    self.hdr_hover_t0 = Instant::now();
                    crate::anim::set(true);
                    if hh.is_some() {
                        unsafe {
                            SetTimer(Some(self.msg_hwnd), T_HDRCHECK, 250, None);
                        }
                    }
                    self.invalidate();
                }
                self.update_hdr_btn(p, hh);
                match hh {
                    Some((id, Some(t), false)) if self.settings.tabs.hover_switch => self.arm_tab_switch(id, t, 300),
                    _ => self.pending_tab = None,
                }
                let h = match self.hit_test(p) {
                    Hit::Item { key, .. } => Some(key),
                    _ => None,
                };
                if h != self.hover {
                    self.hide_item_tip();
                    unsafe {
                        match &h {
                            Some(k) if self.host.is_some() && !self.sel.is_selected(k) => {
                                SetTimer(Some(self.msg_hwnd), T_HOVER, HOVER_PREFETCH_MS, None);
                            }
                            _ => {
                                let _ = KillTimer(Some(self.msg_hwnd), T_HOVER);
                            }
                        }
                        if h.is_some() {
                            // Windows' tooltip delay (the double-click time, 500 ms by default).
                            SetTimer(Some(self.msg_hwnd), T_TIP, GetDoubleClickTime().clamp(200, 1500), None);
                        }
                    }
                    self.hover = h;
                    self.invalidate();
                }
                if !self.tracking_leave {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.hwnd,
                        dwHoverTime: 0,
                    };
                    self.tracking_leave = unsafe { TrackMouseEvent(&mut tme).is_ok() };
                }
                self.set_cursor(p);
                Drag::None
            }
        };
        if !matches!(self.drag, Drag::None) {
            self.invalidate();
        }
        After::None
    }

    fn set_cursor(&self, p: Pt) {
        if (self.on_pill(p) && !(self.search_query().trim().len() > 0 && p.x > self.pill_rect().right() - 34.0))
            || self.box_search_field().map(|(_, f)| f.contains(p) && p.x < f.right() - 26.0).unwrap_or(false)
        {
            unsafe {
                SetCursor(LoadCursorW(None, IDC_IBEAM).ok());
            }
            return;
        }
        if self.az_bar_at(p).is_some() {
            unsafe {
                SetCursor(LoadCursorW(None, IDC_HAND).ok());
            }
            return;
        }
        if let Some((_, 1, _)) = self.hdr_btn {
            unsafe {
                SetCursor(LoadCursorW(None, IDC_IBEAM).ok());
            }
            return;
        }
        let c = match self.hit_test(p) {
            Hit::Edge { l, t, r, b, .. } => match (l || r, t || b) {
                (true, true) => {
                    if (l && t) || (r && b) {
                        IDC_SIZENWSE
                    } else {
                        IDC_SIZENESW
                    }
                }
                (true, false) => IDC_SIZEWE,
                _ => IDC_SIZENS,
            },
            // The title moves the box (tabs move themselves, + adds one).
            Hit::Header { id, tab: None, add: false } if !self.locks_of(id).position => IDC_SIZEALL,
            _ => IDC_ARROW,
        };
        unsafe {
            SetCursor(LoadCursorW(None, c).ok());
        }
    }

    fn on_lbutton_up(&mut self, _p: Pt) {
        unsafe {
            let _ = ReleaseCapture();
        }
        if let Some(be) = self.box_edit.as_mut().filter(|b| b.selecting) {
            be.selecting = false;
            return;
        }
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        self.preview_loose = None;
        match drag {
            Drag::Pending { hit, ctrl, shift, .. } => match hit {
                Hit::Item { key, .. } => {
                    if ctrl && !shift {
                        self.sel.toggle(&key);
                    } else if !ctrl && !shift {
                        self.sel.select_only(&key);
                    }
                }
                Hit::Empty | Hit::BoxBody { .. } => {
                    if !ctrl {
                        self.sel.clear();
                    }
                }
                Hit::Header { id, add: true, .. } => self.add_tab(id),
                _ => {}
            },
            Drag::Items { keys, start, cur } => self.drop_items(keys, start, cur),
            Drag::Tab { id, tab, cur, .. } => self.drop_tab(id, tab, cur),
            Drag::MoveBox { id, cur, orig, .. } => {
                self.reveal(id);
                // Released over another box's header: combine into tabs.
                let target = if self.settings.tabs.combine && !self.locks_of(id).tabs { self.header_target(id, cur) } else { None };
                let final_rect = self.box_by_id(id).map(|b| b.rect);
                // The drag moved the box live; restore it so the change is recorded once (undo).
                if let Some(b) = self.layout.boxes.iter_mut().find(|b| b.id == id) {
                    b.rect = orig;
                }
                self.change_layout(|l, _| match target {
                    Some(dst) => {
                        layout::combine(l, id, dst);
                    }
                    None => {
                        if let Some(r) = final_rect {
                            layout::set_box_rect(l, id, r);
                        }
                    }
                });
            }
            Drag::Resize { id, orig, .. } => {
                self.reveal(id);
                let final_rect = self.box_by_id(id).map(|b| b.rect);
                if let Some(b) = self.layout.boxes.iter_mut().find(|b| b.id == id) {
                    b.rect = orig;
                }
                if let Some(r) = final_rect {
                    self.change_layout(|l, _| {
                        layout::set_box_rect(l, id, r);
                    });
                }
            }
            _ => {}
        }
        self.invalidate();
    }

    /// Esc or a lost capture: a box being moved or resized goes back where it was.
    pub(crate) fn cancel_drag(&mut self) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::MoveBox { id, orig, .. } | Drag::Resize { id, orig, .. } => {
                if let Some(b) = self.layout.boxes.iter_mut().find(|b| b.id == id) {
                    b.rect = orig;
                }
            }
            _ => {}
        }
        self.preview_loose = None;
    }

    pub fn drop_items(&mut self, keys: Vec<ItemKey>, start: Pt, cur: Pt) {
        // Where the user drops icons is exact: no glide for them.
        self.no_glide.extend(keys.iter().cloned());
        if let Some(key) = self.drop_onto_item(cur, &keys) {
            {
                // Onto a folder or program: Windows' own drop (moves or opens with).
                if let Some(t) = self.items.get(&key).map(|i| i.parsing_name.clone()) {
                    let paths: Vec<String> = keys.iter().filter_map(|k| self.items.get(k)).map(|i| i.parsing_name.clone()).collect();
                    let _ = self.worker.send(Req::DropOnItem {
                        target: t,
                        paths,
                        copy: key_down(VK_CONTROL),
                    });
                }
            }
            return;
        }
        // Onto a folder box: the files move into its folder. Out of one, anywhere else: nothing
        // (its files are not desktop icons).
        if let Some(id) = self.drop_box(cur).filter(|id| self.is_folder_box(*id)) {
            self.drop_into_folder_box(&keys, id, key_down(VK_CONTROL));
            return;
        }
        if keys.iter().any(|k| self.is_folder_item(k)) {
            log::info!("drop refused: items of a folder box");
            return;
        }
        if !self.icons_movable(&keys, self.drop_box(cur)) {
            log::info!("drop refused: locked");
            return;
        }
        match self.drop_box(cur) {
            Some(id) => {
                // The slot under the pointer, as an index into the tab's full list (items not on
                // disk right now keep their places).
                let index = self.box_by_id(id).and_then(|b| {
                    let header = cur.y < b.rect.y + header_h();
                    let (i, _) = self.box_drop_slot(b, cur);
                    let tab = b.active()?;
                    (!header).then(|| {
                        tab.items
                            .iter()
                            .enumerate()
                            .filter(|(_, k)| self.items.contains_key(*k))
                            .nth(i)
                            .map(|(j, _)| j)
                            .unwrap_or(tab.items.len())
                    })
                });
                // On a tab of the title: into that tab.
                let tab = match self.hit_test(cur) {
                    Hit::Header { id: h, tab: Some(t), .. } if h == id => Some(t),
                    _ => None,
                };
                self.change_layout(|l, _| match tab {
                    Some(t) => {
                        layout::move_to_tab(l, &keys, id, t, None);
                    }
                    None => {
                        layout::move_to_box(l, &keys, id, index);
                    }
                });
            }
            None => {
                // Loose grid: every item moves by the drag offset, so the group keeps its shape
                // (Explorer). Items not on screen (scrolled out in a box) go next to the cursor.
                let (dx, dy) = (cur.x - start.x, cur.y - start.y);
                let cells = self.visible_cells();
                let moves: Vec<(ItemKey, Cell)> = keys
                    .iter()
                    .map(|k| {
                        let want = match cells.iter().find(|(x, _, _)| x == k) {
                            Some((_, r, _)) => self.grid_cell_unclamped(Pt {
                                x: r.x + r.w / 2.0 + dx,
                                y: r.y + r.h / 2.0 + dy,
                            }),
                            None => self.grid_cell_unclamped(cur),
                        };
                        (k.clone(), want)
                    })
                    .collect();
                self.change_layout(|l, g| layout::move_to_grid_at(l, &moves, g));
            }
        }
    }

    fn on_rbutton_down(&mut self, p: Pt) {
        self.hide_tip();
        if self.above_click_outside(p) {
            return;
        }
        if self.on_pill(p) || self.box_search_field().map(|(_, f)| f.contains(p)).unwrap_or(false) {
            return;
        }
        self.end_rename(true);
        let hit = self.hit_test(p);
        if let Hit::Item { key, .. } = &hit {
            if !self.sel.is_selected(key) {
                self.sel.select_only(key);
            } else {
                self.sel.focus = Some(key.clone());
            }
            self.prefetch(&hit);
        } else if matches!(hit, Hit::Empty) {
            self.sel.clear();
            unsafe {
                SetCapture(self.hwnd);
            }
            self.drag = Drag::RightRubber { start: p, cur: p };
        }
        self.invalidate();
    }

    fn on_rbutton_up(&mut self, p: Pt) {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        unsafe {
            let _ = ReleaseCapture();
        }
        let hit = self.hit_test(p);
        let new_rect = match drag {
            Drag::RightRubber { start, cur } => {
                let r = Rect::from_points(start, cur);
                (r.w > self.grid.cell_w && r.h > header_h()).then_some(r)
            }
            _ => None,
        };
        self.invalidate();
        self.show_menu(hit, p, false, new_rect);
    }

    fn on_wheel(&mut self, p: Pt, delta: i16, ctrl: bool) {
        self.hide_tip();
        // Ctrl + wheel: icon size, a step per notch (small, medium, large).
        if ctrl && self.settings.extras.ctrl_wheel_size && delta != 0 {
            const SIZES: [u32; 3] = [16, 32, 48];
            let cur = self.icon_dip.round() as u32;
            let i = SIZES.iter().enumerate().min_by_key(|(_, s)| s.abs_diff(cur)).map(|(i, _)| i).unwrap_or(1);
            let j = if delta > 0 { (i + 1).min(SIZES.len() - 1) } else { i.saturating_sub(1) };
            self.set_icon_size(SIZES[j]);
            return;
        }
        if let Hit::BoxBody { id } | Hit::Item { in_box: Some(id), .. } = self.hit_test(p) {
            let Some(b) = self.box_by_id(id) else { return };
            let max = self.max_scroll(b);
            let s = self.scroll.get(&id).copied().unwrap_or(0.0);
            self.scroll_to(id, (s - delta as f32 / 120.0 * self.grid.cell_h).clamp(0.0, max));
            self.az_after_scroll(id);
        }
    }

    fn focus_cell(&self) -> Option<(ItemKey, Rect)> {
        let f = self.sel.focus.clone()?;
        self.visible_cells().into_iter().find(|(k, _, _)| *k == f).map(|(k, r, _)| (k, r))
    }

    fn on_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        let (ctrl, shift) = (key_down(VK_CONTROL), key_down(VK_SHIFT));
        if ctrl && vk.0 == b'F' as u16 {
            self.open_search(None);
            return true;
        }
        if vk == VK_ESCAPE && !self.search_query().trim().is_empty() {
            self.close_search();
            return true;
        }
        let order = self.order();
        match vk {
            VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN => {
                let cells = self.visible_cells();
                let next = match self.focus_cell() {
                    None => cells.first().map(|(k, _, _)| k.clone()),
                    Some((fk, fr)) => {
                        let (fx, fy) = (fr.x + fr.w / 2.0, fr.y + fr.h / 2.0);
                        cells
                            .iter()
                            .filter(|(k, _, _)| *k != fk)
                            .filter_map(|(k, r, _)| {
                                let (dx, dy) = (r.x + r.w / 2.0 - fx, r.y + r.h / 2.0 - fy);
                                let (main, side) = match vk {
                                    VK_LEFT => (-dx, dy),
                                    VK_RIGHT => (dx, dy),
                                    VK_UP => (-dy, dx),
                                    _ => (dy, dx),
                                };
                                (main > 1.0 && side.abs() < main * 2.0).then(|| (main + side.abs() * 3.0, k.clone()))
                            })
                            .min_by(|a, b| a.0.total_cmp(&b.0))
                            .map(|x| x.1)
                    }
                };
                if let Some(k) = next {
                    if shift {
                        self.sel.range(&k, &order, ctrl);
                    } else if ctrl {
                        self.sel.focus = Some(k);
                    } else {
                        self.sel.select_only(&k);
                    }
                }
            }
            VK_HOME | VK_END => {
                let k = if vk == VK_HOME { order.first() } else { order.last() }.cloned();
                if let Some(k) = k {
                    self.sel.select_only(&k);
                }
            }
            VK_SPACE if ctrl => {
                if let Some(f) = self.sel.focus.clone() {
                    self.sel.toggle(&f);
                }
            }
            VK_RETURN => {
                let one = (self.sel.set.len() == 1).then(|| self.sel.set.iter().next().cloned()).flatten();
                match one.and_then(|k| self.folder_to_open(&k)) {
                    Some((id, path)) if self.folder_open(id, &path) => {}
                    _ => deskboxes_shell::open_default(self.selection_paths(), self.hwnd.0 as isize),
                }
            }
            VK_BACK => {
                // Back in the folder box with the focus (or under the pointer).
                let id = self
                    .sel
                    .focus
                    .as_ref()
                    .and_then(|k| self.layout.boxes.iter().find(|b| b.is_folder() && b.contains(k).is_some()).map(|b| b.id))
                    .or_else(|| self.drop_box(self.cursor_dip()).filter(|id| self.is_folder_box(*id)));
                match id {
                    Some(id) => self.folder_back(id),
                    None => return false,
                }
            }
            VK_F2 => {
                if let Some(f) = self.sel.focus.clone().filter(|f| self.sel.is_selected(f)) {
                    self.begin_rename(RenameTarget::Item(f));
                }
            }
            VK_F5 => {
                let _ = self.worker.send(Req::Scan);
            }
            VK_F12 if ctrl && shift => {
                self.diag = !self.diag;
            }
            VK_DELETE => {
                let paths: Vec<String> = self
                    .sel
                    .ordered(&order)
                    .iter()
                    .filter_map(|k| self.items.get(k))
                    .filter(|i| i.kind != ItemKind::Virtual)
                    .map(|i| i.parsing_name.clone())
                    .collect();
                if !paths.is_empty() {
                    let _ = self.worker.send(Req::Delete { paths, recycle: !shift });
                }
            }
            _ if ctrl => match vk.0 as u8 {
                b'A' => {
                    self.sel.set = order.iter().cloned().collect();
                }
                b'Z' => self.undo(),
                b'Y' => self.redo(),
                b'C' | b'X' => {
                    let paths = self.selection_paths();
                    if !paths.is_empty() {
                        let _ = self.worker.send(Req::Clipboard { paths, cut: vk.0 as u8 == b'X' });
                    }
                }
                b'V' => {
                    let at = self.focus_cell().map(|(_, r)| Pt { x: r.x, y: r.bottom() }).unwrap_or(Pt {
                        x: self.work.x + 10.0,
                        y: self.work.y + 10.0,
                    });
                    self.on_command("paste", Some(MenuCtx { at, hit: Hit::Empty, new_rect: None }));
                }
                _ => return false,
            },
            _ => return false,
        }
        self.invalidate();
        true
    }

    // ---------------------------------------------------------------- lifecycle

    fn on_explorer_restart(&mut self) {
        log::info!("TaskbarCreated: Explorer restarted; recreating the surface");
        if !self.desktop_mode {
            return;
        }
        self.set_above(false);
        unsafe {
            if IsWindow(Some(self.hwnd)).as_bool() {
                let _ = DestroyWindow(self.hwnd);
            }
            let (mon, _) = primary_monitor();
            match create_surface_window(true, mon) {
                Ok((h, o)) => {
                    self.hwnd = h;
                    self.origin_px = o;
                    let (w, hh) = ((mon.right - mon.left) as u32, (mon.bottom - mon.top) as u32);
                    self.surf = Surface::new(&self.gpu, h, w, hh).map_err(|e| log::error!("surface: {e}")).ok();
                    let _ = windows::Win32::System::Ole::RegisterDragDrop(h, &crate::droptarget::DropTarget::create(h));
                    self.draw();
                    if !self.args.no_hide {
                        hide::set_hidden(true);
                    }
                }
                Err(e) => {
                    log::error!("cannot recreate the surface after an Explorer restart: {e}; restoring icons");
                    hide::restore();
                }
            }
        }
    }

    /// At session end: if the autostart would not bring Deskboxes back, show the native icons
    /// now so the next logon is never an empty desktop (ARCHITECTURE §5).
    fn on_query_end_session(&mut self) {
        if !self.desktop_mode || self.args.no_hide {
            return;
        }
        let run = reg::get_string(reg::RUN, crate::RUN_VALUE);
        let exe_ok = run
            .as_deref()
            .map(|r| std::path::Path::new(r.trim_start_matches('"').split('"').next().unwrap_or("")).exists())
            .unwrap_or(false);
        let approved = reg::get_binary(reg::RUN_APPROVED, crate::RUN_VALUE)
            .map(|b| b.first().map(|x| x & 1 == 0).unwrap_or(true))
            .unwrap_or(true);
        let intact = exe_ok && approved;
        log::info!("session end: autostart intact={intact} (run={run:?} approved={approved})");
        if !intact {
            hide::restore();
        }
    }

    pub fn shutdown(&mut self) {
        if self.quitting {
            return;
        }
        self.set_above(false);
        self.quitting = true;
        self.end_rename(false);
        self.save();
        if let Some(h) = &self.host {
            let _ = h.send("quit", json!({}));
        }
        if let Some(mut c) = self.host_child.take() {
            let _ = c.wait_timeout_ms(1500);
        }
        if self.desktop_mode && !self.args.no_hide {
            let s = hide::restore();
            log::info!("exit: native icons restored {s:?}");
        }
        self.write_stats();
        log::info!("exit");
    }

    fn write_stats(&mut self) {
        (self.stats.private_mb, self.stats.working_set_mb) = mem_mb();
        fn pct(v: &[f32]) -> (f64, f64, f64) {
            if v.is_empty() {
                return (0.0, 0.0, 0.0);
            }
            let mut v = v.to_vec();
            v.sort_by(|a, b| a.total_cmp(b));
            let q = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize] as f64;
            (q(0.5), q(0.95), q(1.0))
        }
        self.stats.draw_count = self.draw_times.len();
        (self.stats.draw_ms_p50, self.stats.draw_ms_p95, self.stats.draw_ms_max) = pct(&self.draw_times);
        self.stats.drag_frames = self.drag_draw_times.len();
        (self.stats.drag_draw_ms_p50, self.stats.drag_draw_ms_p95, self.stats.drag_draw_ms_max) = pct(&self.drag_draw_times);
        (self.stats.drag_interval_ms_p50, self.stats.drag_interval_ms_p95, self.stats.drag_interval_ms_max) = pct(&self.drag_intervals);
        log::info!("stats {}", serde_json::to_string(&self.stats).unwrap_or_default());
        if let Some(p) = &self.args.stats_out {
            let _ = std::fs::write(p, serde_json::to_string_pretty(&self.stats).unwrap_or_default());
        }
    }
}

trait WaitTimeout {
    fn wait_timeout_ms(&mut self, ms: u64) -> bool;
}
impl WaitTimeout for std::process::Child {
    fn wait_timeout_ms(&mut self, ms: u64) -> bool {
        let end = Instant::now() + std::time::Duration::from_millis(ms);
        while Instant::now() < end {
            if let Ok(Some(_)) = self.try_wait() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = self.kill();
        false
    }
}

thread_local! {
    static OLD_EDIT_PROC: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

extern "system" fn edit_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let parent = GetWindow(h, GW_OWNER).unwrap_or_default();
        match m {
            WM_KEYDOWN if w.0 == VK_RETURN.0 as usize => {
                let _ = PostMessageW(Some(parent), WM_APP_RENAME_END, WPARAM(1), LPARAM(0));
                return LRESULT(0);
            }
            WM_KEYDOWN if w.0 == VK_ESCAPE.0 as usize => {
                let _ = PostMessageW(Some(parent), WM_APP_RENAME_END, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
            // Ctrl+Backspace: delete the previous word (the plain EDIT control types a box char).
            WM_CHAR if w.0 == 13 || w.0 == 27 || w.0 == 0x7F => return LRESULT(0),
            WM_KEYDOWN if w.0 == VK_BACK.0 as usize && key_down(VK_CONTROL) => {
                let n = GetWindowTextLengthW(h);
                let mut buf = vec![0u16; n as usize + 1];
                let got = GetWindowTextW(h, &mut buf) as usize;
                let (mut a, mut b) = (0u32, 0u32);
                SendMessageW(h, EM_GETSEL, Some(WPARAM(&mut a as *mut u32 as usize)), Some(LPARAM(&mut b as *mut u32 as isize)));
                if a == b {
                    let ed = TextEdit::new(&String::from_utf16_lossy(&buf[..got]));
                    let from = ed.utf16_pos(ed.prev_word(ed.char_pos(b)));
                    SendMessageW(h, EM_SETSEL, Some(WPARAM(from as usize)), Some(LPARAM(b as isize)));
                }
                let empty = [0u16];
                SendMessageW(h, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(empty.as_ptr() as isize)));
                return LRESULT(0);
            }
            WM_KILLFOCUS => {
                let _ = PostMessageW(Some(parent), WM_APP_RENAME_END, WPARAM(1), LPARAM(0));
            }
            _ => {}
        }
        let old = OLD_EDIT_PROC.with(|o| o.get());
        let f: WNDPROC = std::mem::transmute(old);
        CallWindowProcW(f, h, m, w, l)
    }
}

extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let r = dispatch(h, m, w, l);
        match r {
            Some(Some(v)) => v,
            _ => DefWindowProcW(h, m, w, l),
        }
    }
}

/// `None` = App busy (re-entered) or unhandled → DefWindowProc.
unsafe fn dispatch(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> Option<Option<LRESULT>> {
    let mut after = After::None;
    let r = with(|app| {
        let is_surface = h == app.hwnd;
        if is_surface && (m == WM_SETFOCUS || m == WM_KILLFOCUS) {
            app.set_desk_active(m == WM_SETFOCUS);
        }
        if m == app.taskbar_created && m != 0 {
            app.on_explorer_restart();
            return Some(LRESULT(0));
        }
        match m {
            WM_NCHITTEST if is_surface => Some(LRESULT(HTCLIENT as isize)),
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK if is_surface => {
                let (x, y) = lparam_pt(l);
                app.on_lbutton_down(app.to_dip(x, y), m == WM_LBUTTONDBLCLK);
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE if is_surface => {
                let (x, y) = lparam_pt(l);
                after = app.on_mouse_move(app.to_dip(x, y));
                Some(LRESULT(0))
            }
            WM_LBUTTONUP if is_surface => {
                let (x, y) = lparam_pt(l);
                app.on_lbutton_up(app.to_dip(x, y));
                Some(LRESULT(0))
            }
            WM_RBUTTONDOWN if is_surface => {
                let (x, y) = lparam_pt(l);
                app.on_rbutton_down(app.to_dip(x, y));
                Some(LRESULT(0))
            }
            WM_RBUTTONUP if is_surface => {
                let (x, y) = lparam_pt(l);
                app.on_rbutton_up(app.to_dip(x, y));
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL if is_surface => {
                let (x, y) = lparam_pt(l);
                let p = app.screen_to_dip(x, y);
                app.on_wheel(p, ((w.0 >> 16) & 0xffff) as i16, w.0 & 0x0008 != 0);
                Some(LRESULT(0))
            }
            WM_CAPTURECHANGED if is_surface => {
                if !matches!(app.drag, Drag::None) && l.0 != app.hwnd.0 as isize {
                    app.cancel_drag();
                    app.invalidate();
                }
                Some(LRESULT(0))
            }
            WM_KEYDOWN if is_surface && app.search.focused => {
                app.search_key(VIRTUAL_KEY(w.0 as u16));
                Some(LRESULT(0))
            }
            WM_CHAR if is_surface && app.search.focused => {
                app.search_char(w.0 as u32);
                Some(LRESULT(0))
            }
            WM_KILLFOCUS if is_surface && app.search.focused => {
                app.blur_search();
                Some(LRESULT(0))
            }
            WM_KEYDOWN if is_surface && app.box_edit.is_some() => {
                app.box_edit_key(VIRTUAL_KEY(w.0 as u16));
                Some(LRESULT(0))
            }
            WM_CHAR if is_surface && app.box_edit.is_some() => {
                app.box_edit_char(w.0 as u32);
                Some(LRESULT(0))
            }
            WM_KILLFOCUS if is_surface && app.box_edit.is_some() => {
                app.end_rename(true);
                Some(LRESULT(0))
            }
            WM_CHAR if is_surface && app.rename.is_none() => {
                if app.type_to_search(w.0 as u32) {
                    Some(LRESULT(0))
                } else {
                    None
                }
            }
            WM_KEYDOWN | WM_SYSKEYDOWN if is_surface => {
                if w.0 == VK_ESCAPE.0 as usize && !matches!(app.drag, Drag::None) {
                    let _ = ReleaseCapture();
                    app.cancel_drag();
                    app.invalidate();
                    return Some(LRESULT(0));
                }
                if w.0 == VK_ESCAPE.0 as usize && app.is_above() {
                    app.set_above(false);
                    return Some(LRESULT(0));
                }
                if (w.0 == VK_F10.0 as usize && key_down(VK_SHIFT)) || w.0 == VK_APPS.0 as usize {
                    let (hit, at) = match app.focus_cell().filter(|(k, _)| app.sel.is_selected(k)) {
                        Some((k, r)) => (
                            Hit::Item { key: k, in_box: None },
                            Pt {
                                x: r.x + r.w / 2.0,
                                y: r.y + r.h / 2.0,
                            },
                        ),
                        None => (
                            Hit::Empty,
                            Pt {
                                x: app.work.x + 20.0,
                                y: app.work.y + 20.0,
                            },
                        ),
                    };
                    app.show_menu(hit, at, true, None);
                    return Some(LRESULT(0));
                }
                app.on_key(VIRTUAL_KEY(w.0 as u16)).then_some(LRESULT(0))
            }
            WM_SETCURSOR if is_surface => Some(LRESULT(1)),
            WM_MOUSELEAVE if is_surface => {
                app.tracking_leave = false;
                app.no_reveal = None;
                app.update_hdr_btn(Pt { x: -1.0e6, y: -1.0e6 }, None);
                app.hide_tip();
                app.search_hover(Pt { x: -1.0e6, y: -1.0e6 });
                if app.hover.take().is_some() || app.hover_hdr.take().is_some() {
                    app.invalidate();
                }
                Some(LRESULT(0))
            }
            WM_DESTROY if is_surface => {
                if !app.quitting {
                    log::warn!("surface window destroyed (Explorer exit?)");
                }
                Some(LRESULT(0))
            }
            WM_APP_EVENTS => {
                for ev in events::take() {
                    match ev {
                        Ev::Scanned {
                            items,
                            icons,
                            native,
                            ms,
                            first,
                            resolved,
                        } => {
                            app.stats.scan_resolved = resolved;
                            app.apply_scan(items, icons, native, ms, first)
                        }
                        Ev::Rescan => {
                            SetTimer(Some(app.msg_hwnd), T_FS, 75, None);
                        }
                        Ev::HostReady(c) => {
                            app.host = Some(c);
                        }
                        Ev::HostMsg(v) => app.on_host_msg(v),
                        Ev::InfoTip { key, text } => app.on_info_tip(key, text),
                        Ev::Update => {
                            if app.host.is_some() && app.settings_sig() != app.settings_sent {
                                app.send_settings(false, None);
                            }
                        }
                        Ev::HostGone => {
                            if !app.quitting {
                                log::warn!("shell host disconnected; restarting it");
                                app.host = None;
                                app.menu = None;
                                if let Some(mut c) = app.host_child.take() {
                                    let _ = c.wait_timeout_ms(200);
                                }
                                SetTimer(Some(app.msg_hwnd), T_HOST_RETRY, 1000, None);
                            }
                        }
                    }
                }
                Some(LRESULT(0))
            }
            WM_APP_DRAW => {
                crate::anim::PENDING.store(false, std::sync::atomic::Ordering::Release);
                app.draw();
                Some(LRESULT(0))
            }
            WM_APP_RENAME_END => {
                app.end_rename(w.0 == 1);
                app.invalidate();
                Some(LRESULT(0))
            }
            WM_TIMER => {
                let id = w.0;
                let _ = KillTimer(Some(h), id);
                match id {
                    T_HOST | T_HOST_RETRY => app.start_host(),
                    T_PREFETCH => app.prefetch_selection(),
                    T_HOVER => app.prefetch_hover(),
                    T_REVEAL => app.reveal_pending(),
                    T_ROLLCHECK => app.roll_check(),
                    T_TABHOVER => app.tab_switch_pending(),
                    T_HDRCHECK => app.hdr_check(),
                    T_TIP => app.tip_due(),
                    T_BTNTIP => app.btn_tip_due(),
                    T_ABOVE => {
                        app.corner_tick();
                        app.above_check_foreground();
                        app.above_poll();
                    }
                    T_CARET => {
                        if let Some(be) = app.box_edit.as_mut() {
                            be.caret_on = !be.caret_on;
                            SetTimer(Some(h), T_CARET, GetCaretBlinkTime().clamp(250, 1200), None);
                            app.invalidate();
                        } else if app.search_caret_tick() {
                            SetTimer(Some(h), T_CARET, GetCaretBlinkTime().clamp(250, 1200), None);
                        }
                    }
                    T_SAVE => app.save(),
                    T_FS => {
                        let _ = app.worker.send(Req::Scan);
                    }
                    T_EXIT => {
                        let _ = PostMessageW(Some(app.msg_hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                    _ => {}
                }
                Some(LRESULT(0))
            }
            0x0320 /* WM_DWMCOLORIZATIONCOLORCHANGED */ => {
                reset_accent();
                app.invalidate();
                None
            }
            WM_SETTINGCHANGE => {
                reset_accent();
                app.invalidate();
                let light = reg::apps_use_light_theme();
                if light != app.light || w.0 as u32 == SPI_SETDESKWALLPAPER.0 {
                    app.light = light;
                    app.load_wallpaper();
                    app.invalidate();
                }
                None
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED => {
                log::info!("display change: Phase 1 re-layout needs a restart (UNVERIFIED path)");
                None
            }
            WM_HOTKEY if w.0 as i32 == crate::above::HK_ABOVE => {
                app.toggle_above();
                Some(LRESULT(0))
            }
            WM_QUERYENDSESSION => {
                app.on_query_end_session();
                Some(LRESULT(1))
            }
            WM_CLOSE if h == app.msg_hwnd => {
                app.shutdown();
                PostQuitMessage(0);
                Some(LRESULT(0))
            }
            _ => None,
        }
    });
    if let After::OleDrag(paths, ghost) = after {
        ole_drag(h, paths, ghost);
    }
    r
}

/// Give a drag our own image (the in-app ghost), in place of the generic one Windows makes.
/// Windows owns the bitmap once it accepts it.
unsafe fn set_drag_image(obj: &windows::Win32::System::Com::IDataObject, ghost: (u32, u32, Vec<u8>, (i32, i32))) -> windows::core::Result<()> {
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::Shell::*;
    let (w, h, px, (ox, oy)) = ghost;
    let helper: IDragSourceHelper = windows::Win32::System::Com::CoCreateInstance(&CLSID_DragDropHelper, None, windows::Win32::System::Com::CLSCTX_INPROC_SERVER)?;
    // Keep Windows' "Move to …" / "Copy to …" captions under the image.
    if let Ok(h2) = windows::core::Interface::cast::<IDragSourceHelper2>(&helper) {
        let _ = h2.SetFlags(DSH_ALLOWDROPDESCRIPTIONTEXT.0 as u32);
    }
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let hbm = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
    std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len().min((w * h * 4) as usize));
    let img = SHDRAGIMAGE {
        sizeDragImage: SIZE { cx: w as i32, cy: h as i32 },
        ptOffset: POINT { x: ox, y: oy },
        hbmpDragImage: hbm,
        crColorKey: COLORREF(0xFFFF_FFFF),
    };
    if let Err(e) = helper.InitializeFromBitmap(&img, obj) {
        let _ = DeleteObject(hbm.into());
        return Err(e);
    }
    Ok(())
}

/// Drag items out to another window: Windows' own shell data object and drag loop, with our drag
/// image.
unsafe fn ole_drag(h: HWND, paths: Vec<String>, ghost: Option<(u32, u32, Vec<u8>, (i32, i32))>) {
    match deskboxes_shell::fileops::data_object(&paths) {
        Ok(obj) => {
            use windows::Win32::System::Ole::*;
            if let Some(g) = ghost {
                if let Err(e) = set_drag_image(&obj, g) {
                    log::warn!("drag image: {e}");
                }
            }
            let r = windows::Win32::UI::Shell::SHDoDragDrop(Some(h), &obj, None, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK);
            log::info!("OLE drag of {} item(s) -> {r:?}", paths.len());
            with(|app| {
                app.own_drag = None;
                let _ = app.worker.send(Req::Scan);
            });
        }
        Err(e) => {
            log::warn!("drag data object: {e}");
            with(|app| app.own_drag = None);
        }
    }
}
