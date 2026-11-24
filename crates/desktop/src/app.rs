//! The core's UI thread: windows, state, input, menus, boxes, persistence (ARCHITECTURE §2–§10).

use crate::events::{self, Ev, WM_APP_DRAW, WM_APP_EVENTS, WM_APP_REFOCUS, WM_APP_RENAME_END};
use crate::textedit::TextEdit;
use crate::worker::{self, Req};
use crate::{Args, Paths};
use deskboxes_core::layout::{self, FileOp, FileStep, History, Step};
use deskboxes_core::persist;
use deskboxes_core::selection::Selection;
use deskboxes_core::snapshot::{IconImage, Snapshot};
use deskboxes_core::*;
use deskboxes_i18n::{category_name, color_name, settings_strings, t, t as t_, Lang, BOX_COLORS, S};
use deskboxes_render::{Bitmap, Gpu, Surface, TextLayout};
use deskboxes_shell::{autostart, hide, items, reg, turnon, wallpaper};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::Instant;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{EM_GETLINECOUNT, EM_GETSEL, EM_HIDEBALLOONTIP, EM_REPLACESEL, EM_SETMARGINS, EM_SETSEL, EM_UNDO};
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
/// Dragged icons rested between two others for a moment: those make room.
const T_ROOM: usize = 17;
/// A box scrolled under a drag: Windows asks the drop target again (see `drag_nudge`).
const T_DRAGNUDGE: usize = 18;
/// The pointer rested on an icon: show its tooltip.
const T_TIP: usize = 14;
/// The pointer rested on a title button (search, +): its tooltip.
const T_BTNTIP: usize = 15;
/// Boxes above windows: the screen corner and whether another app came to the front.
pub const T_ABOVE: usize = 16;
/// Boxes above windows: a drag under way (the region follows the boxes; its end is noticed).
pub const T_ABOVE_DRAG: usize = 19;
/// A menu asked of the shell host has not shown in time: the host is stuck, start a new one.
const T_MENU_WAIT: usize = 20;
/// How long a menu may take to show before the shell host counts as stuck, ms (a cold menu with
/// slow shell extensions on a slow PC takes a few seconds; a stuck host never shows one).
const MENU_WAIT_MS: u32 = 8000;
/// Labels of icons out of view are made while idle.
pub const T_PREP: usize = 40;
/// Settings changed from the settings window are written a moment later (sliders send many).
const T_SAVE_SETTINGS: usize = 41;
/// The wallpaper changed: load it again once the changes settle.
const T_WALLPAPER: usize = 42;
/// The taskbar check again, a moment after the desktop got the front.
const T_TASKBAR: usize = 43;
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
/// A new icon (a file made, a delete undone) growing and fading in, s.
pub const APPEAR_S: f32 = 0.22;
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
/// The box background crossfading to another tab's color, s.
/// A name field fading in.
pub const EDIT_FADE_S: f32 = 0.12;
pub const TAB_COLOR_S: f32 = 0.22;
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
        /// Where it started: a box (selects only that box's icons) or the desktop.
        scope: Option<u32>,
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

/// A context menu as asked for (`App::show_menu`'s arguments).
#[derive(Clone, Debug)]
pub struct MenuAsk {
    pub hit: Hit,
    pub at: Pt,
    pub keyboard: bool,
    pub new_rect: Option<Rect>,
}

/// Screen edges a box touches.
#[derive(Clone, Copy, Debug, Default)]
pub struct Dock {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

/// A box (tab) title being edited in place in its header, drawn by us.
pub struct BoxEdit {
    pub id: u32,
    pub tab: usize,
    /// When the field appeared (it fades in).
    pub since: Instant,
    pub ed: TextEdit,
    pub caret_on: bool,
    /// Mouse button held in the field: drag selects.
    pub selecting: bool,
    /// First half of a surrogate pair typed (WM_CHAR delivers them one at a time).
    pub hi: Option<u16>,
}

/// An icon's name being edited: Windows' own text box under the icon (as Explorer's desktop
/// has it: the name wraps onto more lines, the field grows with it, the I-beam, arrows,
/// selecting with the mouse, Ctrl+Z, the clipboard all work).
pub struct ItemRename {
    /// The field's frame (rounded, Fluent: see rename_frame_proc) and the text box in it.
    pub frame: HWND,
    pub edit: HWND,
    pub key: ItemKey,
    pub since: Instant,
    font: HFONT,
    brush: HBRUSH,
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
    /// Bumped whenever `items` changes (cached orders and layouts check it).
    pub items_gen: u64,
    /// `items_gen` all labels were made for (`prep_more`).
    pub prep_gen: u64,
    /// Settings changed and not yet written (`save_settings_soon`).
    pub settings_dirty: bool,
    /// Each tab's shown order (A–Z sorted), cached: see `az::Shown`.
    pub shown_cache: RefCell<HashMap<u32, crate::az::Shown>>,
    /// While a frame is painted (nothing changes meanwhile): each tab's display order once,
    /// without checking its fingerprint again (hashing every item) on each use.
    pub paint_memo: RefCell<Option<HashMap<u32, (std::rc::Rc<Vec<ItemKey>>, std::rc::Rc<Vec<(char, usize)>>)>>>,
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
    /// Boxes' own views: icons at other sizes, type names, Details texts.
    pub views: crate::views::ViewState,
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
    /// The decoded wallpaper as laid out: (image and its original size, background color,
    /// position, monitor w, h), kept for making blur levels.
    pub wall_src: Option<(Option<(Bitmap, (u32, u32))>, deskboxes_render::Color, i32, u32, u32)>,
    pub wall_sig: String,
    /// The wallpaper drawn: (file, position, color) for this monitor.
    pub wall_used: (String, i32, u32),
    /// A small blurred copy of the wallpaper as pixels: w, h, BGRA, monitor px per sample (the
    /// color behind a blending icon).
    pub wall_grid: Option<(u32, u32, Vec<u8>, f32)>,
    /// Recolored icons (tint, blending), keyed by icon, color and kind: see `iconfx`.
    pub fx_cache: RefCell<HashMap<(u32, u32, u8), Bitmap>>,
    pub light: bool,
    pub worker: worker::WorkerTx,
    pub host: Option<deskboxes_ipc::HostClient>,
    pub host_child: Option<std::process::Child>,
    pub host_starts: Vec<Instant>,
    pub menu: Option<(u32, MenuCtx)>,
    /// A menu asked of the shell host and not shown (or closed) yet: its id, and how to ask again.
    pub menu_wait: Option<(u32, MenuAsk)>,
    /// A menu to ask again once a new shell host is ready (the old one never showed it), since.
    pub menu_replay: Option<(Instant, MenuAsk)>,
    /// An icon created from the "New" menu: sorted by the rules once its name is given.
    pub sort_after_rename: Option<ItemKey>,
    /// An icon's name being edited (Windows' text box).
    pub rename: Option<ItemRename>,
    /// Boxes above windows: where its window is from the surface's top-left (px), the wallpaper
    /// backdrops put aside meanwhile, and what is behind the boxes then (sharp).
    pub above_off: (i32, i32),
    /// A folder picker asked for (folder boxes).
    pub pick_req: Option<crate::folderbox::PickReq>,
    pub saved_backdrops: Option<HashMap<i32, Bitmap>>,
    pub above_wall: Option<Bitmap>,
    /// Windows' drag and drop over our surface: where, the icon that would take it, the box slot.
    pub drop_at: Option<Pt>,
    pub drop_hl: Option<ItemKey>,
    drop_slot: Option<(u32, usize)>,
    /// Windows' drag and drop could not start for this drag: ours, on the surface.
    ole_fallback: bool,
    /// Icons an undo is bringing back, and where they go (until they show up, 10 s at most).
    returning: Option<(Instant, Vec<(u32, ItemKey, Place, usize)>)>,
    /// Edit this icon's name once the layout is redrawn (it was just created).
    pub pending_rename: Option<ItemKey>,
    /// "New" chosen in a menu: when, where (point, box), and the icons there were before.
    pub pending_new: Option<(Instant, Pt, Option<u32>, BTreeSet<ItemKey>)>,
    pub box_edit: Option<BoxEdit>,
    /// What the settings window last got (boxes, styles, default style, language): resent when
    /// the layout changes it, so an open window stays current.
    pub settings_sent: String,
    /// The update status in the data last sent to the settings window (JSON).
    pub update_sent: String,
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
    /// Icons that have just appeared (see `APPEAR_S`), since when.
    pub appearing: HashMap<ItemKey, Instant>,
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
    /// The icon snapshot is out of date (items or icons changed since it was written).
    pub snapshot_dirty: bool,
    /// The snapshot being written (waited for on the way out).
    pub snapshot_job: Option<std::thread::JoinHandle<()>>,
    /// The page a drop or paste waiting for its files was made on.
    pub pending_drop_page: Option<u32>,
    /// The menu being shown is a replay of one a stuck shell host never showed (and the one
    /// waited for is: `menu_wait_replay`).
    pub replaying_menu: bool,
    pub menu_wait_replay: bool,
    /// The box of the icon last clicked (None: a loose one): which folder box a folder in it
    /// opens in, when the same folder shows in two.
    pub item_box: Option<u32>,
    /// The last frame was drawn and committed (Explorer's icons are only hidden behind one).
    pub frame_ok: bool,
    /// Sort into the default boxes after the first scan (a fresh install; see `template`).
    pub template_pending: bool,
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
    /// Our icons dragged in between two loose ones: the dragged icons, where they would go in
    /// and which way the icons there move to make room (shown in `preview_loose`).
    pub room: Option<(Vec<ItemKey>, Cell, (i32, i32))>,
    /// Our icons dragged inside their folder box: the box, the dragged icons and its order with
    /// them where they would go (the cell under the pointer); shown once the pointer rests
    /// (`box_room`), the others gliding aside as on the desktop.
    pub(crate) box_want: Option<(u32, Vec<ItemKey>, std::rc::Rc<Vec<ItemKey>>)>,
    pub box_room: Option<(u32, Vec<ItemKey>, std::rc::Rc<Vec<ItemKey>>)>,
    /// Boxes whose icons glide when their order changes (one icons are being dragged in), and
    /// for each icon where it was in the box's content (unscrolled) and its glide.
    glide_boxes: BTreeSet<u32>,
    box_glide_at: HashMap<ItemKey, Pt>,
    box_glide: HashMap<ItemKey, (Pt, Instant)>,
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
    pub color_pick: Option<(u32, Option<(usize, bool)>, Option<Rgba>)>,
    /// A tab just opened in a box whose tabs have their own colors: box, the color it had
    /// (None = automatic), since (the background crossfades).
    pub tab_color_from: Option<(u32, Option<Rgba>, Instant)>,
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
    /// A drag (ours, or from an app) under way while the boxes are up, since: the windows behind
    /// take drops outside the boxes (see `above::above_drag_begin`).
    pub above_drag: Option<Instant>,
    /// The dim fading out for a drag (true) or back in after it (false), since.
    pub above_undim: Option<(bool, Instant)>,
    /// The boxes-above window's region now (px rects; empty: the whole window).
    pub above_rgn: Vec<(i32, i32, i32, i32)>,
    /// Hit-testing for the desktop window under the boxes-above window (its loose icons count).
    pub hit_desk: bool,
    /// Desktop pages: the slide between them, the page's name, gestures (`pages.rs`).
    pub pages: crate::pages::PageState,
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

thread_local! {
    static DRAG_WHEEL: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

/// While something is dragged over us: a low-level mouse hook that hands the wheel to
/// `App::drag_wheel` (Windows' drag and drop holds the mouse, so the wheel never reaches our
/// window). Off again when the drag leaves or ends.
pub fn drag_wheel_hook(on: bool) {
    unsafe extern "system" fn proc(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 && w.0 as u32 == WM_MOUSEWHEEL {
            let m = &*(l.0 as *const MSLLHOOKSTRUCT);
            let delta = ((m.mouseData >> 16) & 0xffff) as i16;
            // Only a look here (all input waits while the hook runs): the box scrolls when the
            // message comes.
            if let Some(Some(h)) = with(|app| app.drag_wheel_over(m.pt.x, m.pt.y).then_some(app.msg_hwnd)) {
                let xy = (m.pt.x as u32 as u64) | ((m.pt.y as u32 as u64) << 32);
                let _ = PostMessageW(Some(h), crate::events::WM_APP_DRAGWHEEL, WPARAM(delta as u16 as usize), LPARAM(xy as isize));
                return LRESULT(1);
            }
        }
        CallNextHookEx(None, code, w, l)
    }
    DRAG_WHEEL.with(|h| unsafe {
        if on && h.get() == 0 {
            let module = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
            match SetWindowsHookExW(WH_MOUSE_LL, Some(proc), module, 0) {
                Ok(hk) => h.set(hk.0 as isize),
                Err(e) => log::warn!("drag wheel hook: {e}"),
            }
        } else if !on && h.get() != 0 {
            let _ = UnhookWindowsHookEx(HHOOK(h.get() as *mut _));
            h.set(0);
        }
    });
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
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(rename_frame_proc),
        hInstance: hinst.into(),
        lpszClassName: w!("DeskboxesRenameFrame"),
        hCursor: LoadCursorW(None, IDC_IBEAM).unwrap_or_default(),
        ..Default::default()
    };
    RegisterClassExW(&wc);
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
            WS_CHILD | WS_CLIPSIBLINGS,
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
            WS_POPUP | WS_BORDER,
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

/// The wallpaper read (from the registry) and decoded on a thread of its own, started first thing
/// (`start_wallpaper_prefetch`); the first `load_wallpaper` takes it.
struct WallPrefetch {
    path: String,
    pos: i32,
    color: u32,
    img: Option<(IconImage, (u32, u32))>,
    ms: f64,
}

static WALL_PREFETCH: std::sync::Mutex<Option<std::thread::JoinHandle<WallPrefetch>>> = std::sync::Mutex::new(None);

fn start_wallpaper_prefetch(mon: RECT) {
    let (mw, mh) = ((mon.right - mon.left) as f32, (mon.bottom - mon.top) as f32);
    let h = std::thread::spawn(move || {
        let t = Instant::now();
        let init = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED) };
        let (mut path, mut pos, color) = wallpaper::from_registry();
        if let Ok(p) = std::env::var("DESKBOXES_WALLPAPER") {
            path = p;
            pos = 4;
        }
        // Decoded at its on-screen size, never larger.
        let img = if path.is_empty() {
            None
        } else {
            deskboxes_render::decode_image_cpu(&path, |iw, ih| {
                let r = deskboxes_render::wallpaper_rect(pos, iw as f32, ih as f32, mw, mh);
                ((r.right - r.left) / iw as f32).max((r.bottom - r.top) / ih as f32)
            })
            .map_err(|e| log::warn!("wallpaper {path}: {e}"))
            .ok()
        };
        if init.is_ok() {
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
        WallPrefetch { path, pos, color, img, ms: t.elapsed().as_secs_f64() * 1000.0 }
    });
    if let Ok(mut g) = WALL_PREFETCH.lock() {
        *g = Some(h);
    }
}

/// How long ago this user signed in to Windows, s (for a start at sign-in: how long it took).
fn seconds_since_sign_in() -> Option<f64> {
    unsafe {
        use windows::Win32::System::RemoteDesktop::*;
        let mut buf = windows::core::PWSTR::null();
        let mut n = 0u32;
        WTSQuerySessionInformationW(Some(WTS_CURRENT_SERVER_HANDLE), WTS_CURRENT_SESSION, WTSSessionInfo, &mut buf, &mut n).ok()?;
        if buf.is_null() || (n as usize) < std::mem::size_of::<WTSINFOW>() {
            return None;
        }
        let logon = (*(buf.0 as *const WTSINFOW)).LogonTime;
        WTSFreeMemory(buf.0 as _);
        let now = windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime();
        let now = ((now.dwHighDateTime as i64) << 32) | now.dwLowDateTime as i64;
        (logon > 0 && now > logon).then(|| (now - logon) as f64 / 1e7)
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
    // A fresh install (no settings written yet): the first scan sorts the desktop into boxes.
    let fresh_install = !paths.settings.exists();
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
    let real_desktop = args.desktop_dir.is_none() && !args.window;
    if !settings.enabled {
        log::info!("Deskboxes is disabled in settings: restoring native icons and exiting");
        hide::restore();
        if real_desktop {
            add_turn_on_verb(&settings);
        }
        return 0;
    }
    if real_desktop {
        turnon::remove_verb();
        // Started at sign-in at above-normal priority (the logon task) so the desktop comes up
        // fast; back to normal once it is up.
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(30));
            use windows::Win32::System::Threading::*;
            let _ = SetPriorityClass(GetCurrentProcess(), NORMAL_PRIORITY_CLASS);
        });
    }
    // The UI thread (this one) a step above the rest: on a busy PC (startup apps, a slower CPU)
    // the selection rectangle and drags kept up badly behind other work (round 11, i5 + 1060).
    // It is idle unless you use the desktop; scans and icons stay on normal worker threads.
    unsafe {
        use windows::Win32::System::Threading::*;
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
    // The wallpaper read and decoded on its own thread from here on: at sign-in that took most of
    // the time before the first frame (2.4 s of 2.7 s on the dev PC), and nothing was on screen
    // until it was done.
    start_wallpaper_prefetch(primary_monitor().0);
    let since_sign_in = if real_desktop { seconds_since_sign_in() } else { None };
    let desktop_mode = !args.window;
    let fixture = args.desktop_dir.clone();
    let lang = Lang::pick(&settings.language, windows::Win32::Globalization::GetUserDefaultUILanguage());
    register_classes();
    let hinst = GetModuleHandleW(None).unwrap_or_default();
    let msg_hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, w!("DeskboxesCore"), w!("Deskboxes core"), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst.into()), None).expect("message window");
    events::set_target(msg_hwnd);
    events::watch_wallpaper(msg_hwnd);
    events::watch_foreground(msg_hwnd);
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
    let t_devices = t0.elapsed().as_secs_f64() * 1000.0;
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
    let t_window = t0.elapsed().as_secs_f64() * 1000.0;
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
    let t_text = t0.elapsed().as_secs_f64() * 1000.0;
    let topology = format!("primary-{}x{}-{}", mon_full.right - mon_full.left, mon_full.bottom - mon_full.top, dpi);
    let lay = match persist::load_layout_checked(&paths.layouts, &topology) {
        Ok(l) => l.unwrap_or_default(),
        Err(e) => {
            // Not overwritten unseen: a copy stays next to it (a newer version's layout, say).
            let kept = persist::keep_unreadable(&persist::layout_path(&paths.layouts, &topology));
            log::warn!("layout {topology} unreadable ({e}): starting empty; the file is kept as {kept:?}");
            Default::default()
        }
    };
    let first_run = !persist::layout_path(&paths.layouts, &topology).exists();
    let template_pending = args.first_run_template || (fresh_install && first_run && args.desktop_dir.is_none());
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
        items_gen: 0,
        prep_gen: u64::MAX,
        settings_dirty: false,
        shown_cache: RefCell::new(HashMap::new()),
        paint_memo: RefCell::new(None),
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
        views: Default::default(),
        label_tf,
        title_tf,
        title_tf_bold,
        font,
        line_h,
        title_line_h,
        wall: None,
        backdrops: HashMap::new(),
        backdrop_down: 4,
        wall_src: None,
        wall_sig: String::new(),
        wall_used: Default::default(),
        wall_grid: None,
        fx_cache: RefCell::new(HashMap::new()),
        light,
        worker,
        host: None,
        host_child: None,
        host_starts: vec![],
        menu: None,
        menu_wait: None,
        menu_replay: None,
        sort_after_rename: None,
        rename: None,
        returning: None,
        above_off: (0, 0),
        pick_req: None,
        saved_backdrops: None,
        above_wall: None,
        drop_at: None,
        drop_hl: None,
        drop_slot: None,
        ole_fallback: false,
        pending_rename: None,
        pending_new: None,
        box_edit: None,
        settings_sent: String::new(),
        update_sent: String::new(),
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
        appearing: HashMap::new(),
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
        snapshot_dirty: false,
        snapshot_job: None,
        pending_drop_page: None,
        replaying_menu: false,
        menu_wait_replay: false,
        item_box: None,
        frame_ok: false,
        template_pending,
        draw_times: Vec::new(),
        frame_marks: std::collections::VecDeque::new(),
        diag: false,
        drag_draw_times: Vec::new(),
        drag_intervals: Vec::new(),
        last_drag_frame: None,
        taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
        preview_loose: None,
        room: None,
        box_want: None,
        box_room: None,
        glide_boxes: BTreeSet::new(),
        box_glide_at: HashMap::new(),
        box_glide: HashMap::new(),
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
        tab_color_from: None,
        az_at: None,
        blend_hover: (None, None, Instant::now()),
        desk_active: (false, Instant::now()),
        tucked: (false, Instant::now() - std::time::Duration::from_secs(5)),
        above: None,
        above_had_front: false,
        above_drag: None,
        above_undim: None,
        above_rgn: vec![],
        hit_desk: false,
        pages: Default::default(),
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
    let t_init = t0.elapsed().as_secs_f64() * 1000.0;
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
            app.items_gen += 1;
        }
    }
    app.diag = app.args.diagnostics;
    let t_snapshot = t0.elapsed().as_secs_f64() * 1000.0;
    app.load_wallpaper();
    let t_wall = t0.elapsed().as_secs_f64() * 1000.0;
    app.draw();
    // Shown only now, with its first frame: created visible, the empty surface covered the
    // desktop (gray) until the first frame, seconds at sign-in.
    let _ = ShowWindow(app.hwnd, if desktop_mode { SW_SHOWNA } else { SW_SHOW });
    app.stats.first_paint_ms = t0.elapsed().as_secs_f64() * 1000.0;
    log::info!(
        "PHASE startup_steps devices={t_devices:.0} window={t_window:.0} text={t_text:.0} init={t_init:.0} snapshot={t_snapshot:.0} wallpaper={t_wall:.0} shown={:.0} (ms since start){}",
        app.stats.first_paint_ms,
        since_sign_in.map(|s| format!("; started {s:.1} s after sign-in")).unwrap_or_default()
    );
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
    } else if desktop_mode && !app.args.no_hide && !app.frame_ok {
        app.hide_after_scan = true;
        log::warn!("the first frame was not drawn: native icons stay until a scan is drawn");
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
    /// This monitor's wallpaper from Explorer (`IDesktopWallpaper`): (file, position, color).
    fn monitor_wallpaper(&self) -> (String, i32, u32) {
        let mons = wallpaper::monitors();
        let m = mons.iter().find(|m| m.rect.left == self.mon.left && m.rect.top == self.mon.top).or(mons.first()).cloned();
        let (mut path, mut pos, color) = m.map(|m| (m.path, m.position, m.color)).unwrap_or_default();
        // DESKBOXES_WALLPAPER: a test copy on another picture (filled), for screenshots.
        if let Ok(p) = std::env::var("DESKBOXES_WALLPAPER") {
            path = p;
            pos = 4;
        }
        (path, pos, color)
    }

    pub fn load_wallpaper(&mut self) {
        let (mw, mh) = ((self.mon.right - self.mon.left) as u32, (self.mon.bottom - self.mon.top) as u32);
        let t = Instant::now();
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
        // At start: what the prefetch thread read and decoded (see `start_wallpaper_prefetch`).
        let pre = WALL_PREFETCH.lock().ok().and_then(|mut g| g.take()).and_then(|h| h.join().ok());
        let prefetched = pre.is_some();
        let (path, pos, color, img, how) = match pre {
            Some(p) => {
                let img = p.img.and_then(|(im, o)| self.gpu.bitmap(&im).map_err(|e| log::warn!("wallpaper upload: {e}")).ok().map(|b| (b, o)));
                let how = format!("prefetched in {:.0} ms, waited {:.0} ms", p.ms, ms(t));
                (p.path, p.pos, p.color, img, how)
            }
            None => {
                let (path, pos, color) = self.monitor_wallpaper();
                let t_info = ms(t);
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
                let how = format!("info {t_info:.0} ms, decoded {:.0} ms", ms(t) - t_info);
                (path, pos, color, img, how)
            }
        };
        let t_draw = Instant::now();
        let bg = deskboxes_render::rgba((color & 0xff) as u8, ((color >> 8) & 0xff) as u8, ((color >> 16) & 0xff) as u8, 1.0);
        // We draw the wallpaper ourselves in both modes. On the desktop, Windows 11 (24H2+) paints
        // the wallpaper in a separate child of Progman that our full-screen surface covers, so it is
        // not visible through our transparent pixels (seen on 26300: a black desktop, 2026-11-19).
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
        const GRID: u32 = 16;
        self.wall_grid = self
            .gpu
            .blurred_wallpaper(img.as_ref().map(|(b, o)| (b, *o)), bg, pos, mw, mh, GRID, 40.0 * self.scale)
            .and_then(|b| self.gpu.pixels_of(&b))
            .map(|(w, h, px)| (w, h, px, GRID as f32))
            .map_err(|e| log::warn!("wallpaper colors: {e}"))
            .ok();
        self.fx_cache.borrow_mut().clear();
        self.wall_src = Some((img, bg, pos, mw, mh));
        if prefetched {
            // Read from the registry, not from Explorer: Explorer's answer is checked a little
            // later (`wallpaper_check`), when it is less busy and the desktop is on screen.
            self.wall_sig = String::new();
            unsafe {
                SetTimer(Some(self.msg_hwnd), T_WALLPAPER, 3000, None);
            }
        } else {
            self.wall_sig = Self::wallpaper_sig();
        }
        let t_fx = ms(t_draw);
        self.ensure_backdrops();
        log::info!(
            "wallpaper '{path}' pos={pos} backdrops={} ({how}; drawn and colors {t_fx:.0} ms, backdrops {:.0} ms); MEM private/working set {:.1?} MB",
            self.backdrops.len(),
            ms(t_draw) - t_fx,
            mem_mb()
        );
        self.wall_used = (path, pos, color);
    }

    /// Blurred copies of the wallpaper for the blur levels in use, made from the decoded
    /// wallpaper kept in memory (a blur slider used to read and decode the file per step).
    /// Levels no box uses any more are dropped.
    pub fn ensure_backdrops(&mut self) {
        // (Every page's boxes: the one beside slides in with its glass.)
        let mut blurs: Vec<i32> = deskboxes_core::pages::all_boxes(&self.layout).map(|(_, b)| b.style.blur as i32).collect();
        blurs.push(self.settings.default_style.blur as i32);
        blurs.sort();
        blurs.dedup();
        self.backdrops.retain(|k, _| blurs.contains(k));
        let Some((img, bg, pos, mw, mh)) = &self.wall_src else { return };
        for b in blurs {
            if self.backdrops.contains_key(&b) {
                continue;
            }
            match self
                .gpu
                .blurred_wallpaper(img.as_ref().map(|(b, o)| (b, *o)), *bg, *pos, *mw, *mh, self.backdrop_down, b as f32 * self.scale)
            {
                Ok(bm) => {
                    self.backdrops.insert(b, bm);
                }
                Err(e) => log::warn!("backdrop blur {b}: {e}"),
            }
        }
    }

    /// What the wallpaper is now (file, its time and size, layout, color): a change means load
    /// it again. Windows does not always tell top-level windows (Settings › Personalization).
    pub fn wallpaper_sig() -> String {
        let mons = wallpaper::monitors();
        let mut s = String::new();
        for m in &mons {
            let meta = std::fs::metadata(&m.path).ok();
            let t = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis()).unwrap_or(0);
            let n = meta.map(|m| m.len()).unwrap_or(0);
            s.push_str(&format!("{}|{t}|{n}|{}|{};", m.path, m.position, m.color));
        }
        s
    }

    /// The wallpaper may have changed: load it again if it did.
    pub fn wallpaper_check(&mut self) {
        let sig = Self::wallpaper_sig();
        if sig == self.wall_sig {
            return;
        }
        // The first check after a start that drew the wallpaper the registry names: Explorer's
        // answer is the same one (the usual case), nothing to load again.
        if self.wall_sig.is_empty() && self.monitor_wallpaper() == self.wall_used {
            self.wall_sig = sig;
            return;
        }
        log::info!("wallpaper changed");
        self.load_wallpaper();
        self.fx_cache.borrow_mut().clear();
        self.invalidate();
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
        let head = if b.view() == View::Details { crate::views::DETAILS_HEAD } else { 0.0 };
        let top = header_h() + crate::az::bar_h(b) + head;
        Rect::new(b.rect.x, b.rect.y + top, b.rect.w, (b.rect.h - top).max(0.0))
    }

    pub fn box_cols(&self, b: &BoxModel) -> usize {
        let m = self.metrics(Some(b));
        (((b.rect.w - 2.0 * PAD) / m.cell_w).floor() as usize).max(1)
    }

    /// Cell rects of the active tab's items of a box (scrolled; may extend past the content rect).
    pub fn box_item_cells(&self, b: &BoxModel) -> Vec<(ItemKey, Rect)> {
        self.box_cells_in(b, f32::MIN, f32::MAX).into_iter().map(|(_, k, r)| (k, r)).collect()
    }

    /// Like `box_item_cells`, only the rows reaching into `y0..y1` (what can be seen or hit):
    /// (index in the shown order, item, cell). A box of 800 icons shows about 50.
    pub fn box_cells_in(&self, b: &BoxModel, y0: f32, y1: f32) -> Vec<(usize, ItemKey, Rect)> {
        let Some(tab) = b.active() else { return vec![] };
        let m = self.metrics(Some(b));
        let cols = self.box_cols(b);
        let c = Self::content_rect(b);
        let used = cols as f32 * m.cell_w;
        let x0 = c.x + (c.w - used) / 2.0;
        let top = c.y + 2.0 - self.shown_scroll(b.id);
        // Icons dragged in their folder box: the order with them where they would go.
        let items = match &self.box_room {
            Some((id, _, order)) if *id == b.id => order.clone(),
            _ => self.shown_items(b, tab),
        };
        let ch = m.cell_h.max(1.0);
        let first_row = if y0 == f32::MIN { 0 } else { ((y0 - top) / ch).floor().max(0.0) as usize };
        let last_row = if y1 == f32::MAX { usize::MAX } else { ((y1 - top) / ch).floor().max(0.0) as usize };
        let start = first_row.saturating_mul(cols).min(items.len());
        let end = last_row.saturating_add(1).saturating_mul(cols).min(items.len());
        (start..end)
            .map(|i| {
                (
                    i,
                    items[i].clone(),
                    Rect::new(x0 + (i % cols) as f32 * m.cell_w, top + (i / cols) as f32 * m.cell_h, m.cell_w, m.cell_h),
                )
            })
            .collect()
    }

    /// Where items dropped at `p` go in a box: the insertion index among the shown items and
    /// the top-left of the slot (for the insertion marker).
    pub fn box_drop_slot(&self, b: &BoxModel, p: Pt) -> (usize, Pt) {
        let m = self.metrics(Some(b));
        let cols = self.box_cols(b);
        let c = Self::content_rect(b);
        let x0 = c.x + (c.w - cols as f32 * m.cell_w) / 2.0;
        let y0 = c.y + 2.0 - self.shown_scroll(b.id);
        let n = b.active().map(|t| t.items.iter().filter(|k| self.items.contains_key(*k)).count()).unwrap_or(0);
        // One column of rows: above or below a row (by its middle); else between two cells.
        let (row, col) = if m.row() && cols == 1 {
            (((p.y - y0) / m.cell_h).round().max(0.0) as usize, 0)
        } else {
            (((p.y - y0) / m.cell_h).floor().max(0.0) as usize, (((p.x - x0) / m.cell_w).round().max(0.0) as usize).min(cols))
        };
        let i = (row * cols + col).min(n);
        let (r, cc) = if i == row * cols + col { (row, col) } else { (i / cols, i % cols) };
        (
            i,
            Pt {
                x: x0 + cc as f32 * m.cell_w,
                y: y0 + r as f32 * m.cell_h,
            },
        )
    }

    /// An item the dragged items would be dropped *into* (a folder, a program, an archive): only
    /// while the pointer is on its icon, so dropping next to it in a box just places the items.
    pub fn drop_onto_item(&self, p: Pt, keys: &[ItemKey]) -> Option<ItemKey> {
        let key = self.onto_icon(p, keys)?;
        let it = self.items.get(&key)?;
        let ext = std::path::Path::new(&it.parsing_name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let accepts = it.is_folder() || matches!(ext.as_str(), "exe" | "lnk" | "zip" | "bat" | "cmd") || is_recycle_bin(&it.parsing_name);
        accepts.then_some(key)
    }

    /// The icon (not one of `keys`) whose own picture is under `p`: what a drop there goes onto
    /// (a folder, a program), not in next to.
    pub fn onto_icon(&self, p: Pt, keys: &[ItemKey]) -> Option<ItemKey> {
        let Hit::Item { key, .. } = self.hit_test(p) else { return None };
        if keys.contains(&key) {
            return None;
        }
        let (_, cell, bx) = self.visible_cells().into_iter().find(|(k, _, _)| *k == key)?;
        let m = self.metrics_of(bx);
        let (icon, _) = self.item_parts(&key, cell, false, &m);
        // Only the icon itself takes the drop, not the room around it: the gap between two
        // icons is where they go in between (Explorer's insertion).
        let zone = if m.row() {
            Rect::new(cell.x, cell.y + cell.h * 0.25, cell.w, cell.h * 0.5)
        } else {
            let side = (cell.w * 0.2).max(10.0);
            let x0 = icon.x.max(cell.x + side);
            let x1 = icon.right().min(cell.right() - side);
            Rect::new(x0, icon.y, (x1 - x0).max(4.0), icon.h)
        };
        zone.contains(p).then_some(key)
    }

    /// A drop on the loose grid at `p` that goes in between icons: the cell under the pointer
    /// has another icon, so the dropped ones go before it or after it and the icons after move
    /// along their line (never to another one). In a row of icons (neighbours left or right,
    /// none above or below; the dragged icons count, where they were): left half before, right
    /// half after, and the row moves to the right. Otherwise in a column: upper half before,
    /// lower half after, and the column moves down. Returns the cell and whether it is a row.
    /// None: an ordinary move.
    pub fn grid_insert_at(&self, p: Pt, keys: &[ItemKey]) -> Option<(Cell, (i32, i32))> {
        if self.drop_box(p).is_some() {
            return None;
        }
        let g = &self.grid;
        let fx = (p.x - g.origin.x) / g.cell_w;
        let fy = (p.y - g.origin.y) / g.cell_h;
        let under = Cell { col: fx.floor() as i32, row: fy.floor() as i32 };
        if under.col < 0 || under.col >= g.cols || under.row < 0 || under.row >= g.rows {
            return None;
        }
        let others: std::collections::HashSet<Cell> = self.layout.loose.iter().filter(|(k, _)| !keys.contains(k) && self.items.contains_key(*k)).map(|(_, c)| *c).collect();
        if !others.contains(&under) {
            return None;
        }
        let all: std::collections::HashSet<Cell> = self.layout.loose.iter().filter(|(k, _)| self.items.contains_key(*k)).map(|(_, c)| *c).collect();
        let near = |dc: i32, dr: i32| all.contains(&Cell { col: under.col + dc, row: under.row + dr });
        // Along a row or down a column: moved within its own row, the row; within its own
        // column, the column (in a block of icons a move to the side pushed the column down,
        // round 13). From elsewhere: a row of icons or a column of them; in a block (or alone),
        // the nearer edge of the icon under the pointer, its sides or its top and bottom.
        let olds: Vec<Cell> = keys.iter().filter_map(|k| self.layout.loose.get(k)).copied().collect();
        let along = if olds.iter().any(|o| o.row == under.row) {
            true
        } else if olds.iter().any(|o| o.col == under.col) {
            false
        } else {
            match (near(-1, 0) || near(1, 0), near(0, -1) || near(0, 1)) {
                (true, false) => true,
                (false, true) => false,
                _ => (fx - fx.floor() - 0.5).abs() > (fy - fy.floor() - 0.5).abs(),
            }
        };
        // The line between two icons the pointer is nearest to.
        let b = if along { fx.round() as i32 } else { fy.round() as i32 };
        // Moved along its own line, toward the end: the icons it passes move back into the cell
        // it left, the same as when it moves toward the start (they moved on past it, leaving
        // its cell empty, round 13). Otherwise the icons from the line on move on.
        let back = keys.iter().filter_map(|k| self.layout.loose.get(k)).any(|o| if along { o.row == under.row && o.col < b } else { o.col == under.col && o.row < b });
        match (along, back) {
            (true, true) => Some((Cell { col: b - 1, row: under.row }, (-1, 0))),
            (true, false) => (b < g.cols).then_some((Cell { col: b, row: under.row }, (1, 0))),
            (false, true) => Some((Cell { col: under.col, row: (b - 1).clamp(0, g.rows - 1) }, (0, -1))),
            (false, false) => Some((Cell { col: under.col, row: b.clamp(0, g.rows - 1) }, (0, 1))),
        }
    }

    /// Our icons dragged to `p` (None: the drag left or ended): in between two loose icons,
    /// after a moment there, those make room as the drop would place them (a phone's home
    /// screen); onto an icon that takes them in (a folder, a program), a box or open space,
    /// nothing moves.
    pub fn make_room(&mut self, p: Option<Pt>, keys: &[ItemKey]) {
        // In their own folder box: its order.
        let bw = p.and_then(|p| self.box_room_order(p, keys)).map(|(id, o)| (id, keys.to_vec(), std::rc::Rc::new(o)));
        let at = if bw.is_some() { None } else { p.filter(|p| self.drop_onto_item(*p, keys).is_none()).and_then(|p| self.grid_insert_at(p, keys)) };
        let same_grid = at == self.room.as_ref().map(|(_, c, r)| (*c, *r));
        let same_box = bw.as_ref().map(|(i, _, o)| (*i, o.clone())) == self.box_want.as_ref().map(|(i, _, o)| (*i, o.clone()));
        if same_grid && same_box {
            return;
        }
        self.room = at.map(|(c, dir)| (keys.to_vec(), c, dir));
        self.box_want = bw;
        if self.room.is_none() && self.preview_loose.take().is_some() {
            self.invalidate();
        }
        if self.box_want.is_none() && self.box_room.take().is_some() {
            self.invalidate();
        }
        if self.room.is_some() || self.box_want.is_some() {
            unsafe {
                SetTimer(Some(self.msg_hwnd), T_ROOM, 140, None);
            }
        }
    }

    /// The pointer stayed where the dragged icons would go in: the icons there move aside.
    fn room_due(&mut self) {
        if self.own_drag.is_none() && !matches!(self.drag, Drag::Items { .. }) {
            self.room = None;
            self.box_want = None;
            return;
        }
        if let Some(bw) = self.box_want.clone() {
            // Where its icons are now, so they glide from there.
            if self.glide_boxes.insert(bw.0) {
                self.track_box_glides();
            }
            self.box_room = Some(bw);
            self.invalidate();
        }
        let Some((keys, at, dir)) = self.room.clone() else { return };
        let mut l = self.layout.clone();
        layout::insert_along(&mut l, &keys, at, dir, &self.grid);
        layout::settle(&mut l, &self.grid);
        // The gap stays empty: the dragged icons go there only when dropped (they are with the
        // pointer). They stay where they were, unless an icon making room moved in there.
        let others: std::collections::HashSet<Cell> = l.loose.iter().filter(|(k, _)| !keys.contains(k)).map(|(_, c)| *c).collect();
        for k in &keys {
            match self.layout.loose.get(k) {
                Some(c) if !others.contains(c) => {
                    l.loose.insert(k.clone(), *c);
                }
                _ => {
                    l.loose.remove(k);
                }
            }
        }
        self.preview_loose = Some(l.loose);
        self.invalidate();
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
        (rows * self.metrics(Some(b)).cell_h + 4.0 - Self::content_rect(b).h).max(0.0)
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

    /// The items that can be seen now (loose ones, and the rows of open boxes in view), for the
    /// per-frame preparation: a box of 800 icons prepares the 50 it shows, not all of them.
    pub fn cells_in_view(&self) -> Vec<(ItemKey, Rect, Option<u32>)> {
        let mut v: Vec<(ItemKey, Rect, Option<u32>)> = self
            .layout
            .loose
            .iter()
            .filter(|(k, _)| self.items.contains_key(*k))
            .map(|(k, c)| (k.clone(), self.grid.cell_rect(*c), None))
            .collect();
        for b in &self.layout.boxes {
            if !self.fully_open(b) {
                continue;
            }
            let c = Self::content_rect(b);
            v.extend(self.box_cells_in(b, c.y, c.bottom()).into_iter().map(|(_, k, r)| (k, r, Some(b.id))));
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

    /// Icon rect and label rect of an item in a cell (of a box with metrics `m`).
    pub fn item_parts(&self, key: &ItemKey, cell: Rect, full: bool, m: &crate::views::Metrics) -> (Rect, Rect) {
        if m.row() {
            return self.row_parts(cell, m);
        }
        // Where Explorer puts them (measured on its own desktop): the icon 7 DIPs into the cell,
        // the label 3 DIPs under it.
        let icon = Rect::new(cell.x + (cell.w - m.icon) / 2.0, cell.y + 7.0, m.icon, m.icon);
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

    /// The name's layout as `label_spec` says (a row view's one line, or under the icon).
    pub fn label_for(&mut self, key: &ItemKey, w: f32, full: bool, row: bool) -> Option<TextLayout> {
        if row {
            self.row_label(key, w)
        } else {
            self.label_layout(key, w, full)
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

    pub fn hit_rect(&self, key: &ItemKey, cell: Rect, m: &crate::views::Metrics) -> Rect {
        if m.row() {
            return Rect::new(cell.x + 1.0, cell.y + 1.0, cell.w - 2.0, cell.h - 2.0);
        }
        let full = self.is_full_label(key);
        let (i, l) = self.item_parts(key, cell, full, m);
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
        self.track_box_glides();
        self.no_glide.clear();
    }

    /// Glides of the icons in boxes icons are being dragged in (see `glide_boxes`): where each
    /// is in its box's content, unscrolled (scrolling moves them all, not one).
    fn track_box_glides(&mut self) {
        let now = Instant::now();
        let mut at = std::mem::take(&mut self.box_glide_at);
        let mut glide = std::mem::take(&mut self.box_glide);
        for id in self.glide_boxes.clone() {
            let Some(b) = self.box_by_id(id) else { continue };
            if !self.fully_open(b) {
                continue;
            }
            let c = Self::content_rect(b);
            let s = self.shown_scroll(id);
            let m = self.metrics(Some(b));
            for (_, k, r) in self.box_cells_in(b, c.y - m.cell_h, c.bottom() + m.cell_h) {
                let tgt = Pt { x: r.x - c.x, y: r.y - c.y + s };
                if let Some(prev) = at.insert(k.clone(), tgt) {
                    if prev != tgt && !self.no_glide.contains(&k) {
                        let from = glide_pos(glide.get(&k), prev, now);
                        glide.insert(k, (from, now));
                    }
                }
            }
        }
        glide.retain(|_, (_, t0)| now.duration_since(*t0).as_secs_f32() < GLIDE_S);
        self.box_glide_at = at;
        self.box_glide = glide;
        // The drag is over and they have arrived: no more tracking.
        if self.box_want.is_none() && self.box_room.is_none() && self.box_glide.is_empty() && self.own_drag.is_none() && !matches!(self.drag, Drag::Items { .. }) {
            self.glide_boxes.clear();
            self.box_glide_at.clear();
        }
    }

    /// A box's icon cell as drawn now (mid-glide: on its way to `r`).
    pub fn box_glided(&self, b: &BoxModel, k: &ItemKey, r: Rect) -> Rect {
        let Some(g) = self.box_glide.get(k) else { return r };
        let c = Self::content_rect(b);
        let tgt = Pt { x: r.x - c.x, y: r.y - c.y + self.shown_scroll(b.id) };
        let p = glide_pos(Some(g), tgt, Instant::now());
        r.offset(p.x - tgt.x, p.y - tgt.y)
    }

    /// One of the icons being dragged in their folder box: not drawn (where it would go is
    /// left empty).
    pub fn is_box_room_gap(&self, id: u32, k: &ItemKey) -> bool {
        self.box_room.as_ref().is_some_and(|(b, keys, _)| *b == id && keys.contains(k))
    }

    /// A loose icon's cell rect as drawn now (mid-glide: on its way to `cell`).
    pub fn glided(&self, k: &ItemKey, cell: Rect) -> Rect {
        let p = glide_pos(self.glide.get(k), Pt { x: cell.x, y: cell.y }, Instant::now());
        Rect::new(p.x, p.y, cell.w, cell.h)
    }

    /// Recompute where loose icons would settle with the dragged box where it is now.
    pub(crate) fn update_preview(&mut self) {
        // Settling works on the page shown alone: the other pages are not copied along (on every
        // pointer move of a box drag, with many full pages, that was most of the work).
        let parked = std::mem::take(&mut self.layout.parked);
        let mut l = self.layout.clone();
        self.layout.parked = parked;
        layout::settle(&mut l, &self.grid);
        self.preview_loose = Some(l.loose);
    }

    /// Whether anything is animating (keeps the frame clock running).
    pub fn animating(&self) -> bool {
        self.anim_roll
            || !self.glide.is_empty()
            || !self.box_glide.is_empty()
            || self.tab_anim.map(|(_, _, t0)| t0.elapsed().as_secs_f32() < TAB_ANIM_S).unwrap_or(false)
            || self.tab_color_from.map(|(_, _, t0)| t0.elapsed().as_secs_f32() < TAB_COLOR_S).unwrap_or(false)
            || self.tip.as_ref().map(|t| t.4.elapsed().as_secs_f32() < TIP_FADE_S).unwrap_or(false)
            || self.search_animating()
            || self.az_animating()
            || self.snap_animating()
            || self.header_animating()
            || self.blend_animating()
            || self.tuck_animating()
            || self.above_animating()
            || self.pages_animating()
            || self.scroll_animating()
            || (matches!(self.drag, Drag::MoveBox { .. }) && self.move_t0.elapsed().as_secs_f32() < LIFT_S)
            || !self.vanishing.is_empty()
            || !self.appearing.is_empty()
            || self.box_edit.as_ref().map(|e| e.since.elapsed().as_secs_f32() < EDIT_FADE_S).unwrap_or(false)
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
            // The old formats' pointers could be reused by new ones: no stale layouts.
            self.gpu.forget_labels();
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
        } else {
            b.locks
        }
    }

    pub(crate) fn locks_of(&self, id: u32) -> deskboxes_core::model::BoxLocks {
        self.box_by_id(id).map(|b| self.locks(b)).unwrap_or_default()
    }

    /// Whether icons `keys` may be dropped into box `to` (None: onto the desktop): neither where
    /// they come from nor where they go is locked.
    pub(crate) fn icons_movable(&self, keys: &[ItemKey], to: Option<u32>) -> bool {
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

    /// The custom color window for a box (tab None), one of its tabs (false), or the box's
    /// background behind one of its tabs (true).
    fn open_color_picker(&mut self, id: u32, tab: Option<(usize, bool)>) {
        let Some(h) = self.host.clone() else { return };
        let Some(b) = self.box_by_id(id) else { return };
        let cur = match tab {
            Some((t, false)) => b.tabs.get(t).and_then(|t| t.color),
            Some((t, true)) => b.tabs.get(t).and_then(|t| t.box_color.or(b.style.color)),
            None => b.style.color,
        };
        let hex = |c: Rgba| format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b);
        let (ar, ag, ab) = accent();
        let title = match tab {
            Some((t, false)) => format!("{} – {}", t_(self.lang, S::TabColor), b.tabs.get(t).map(|x| x.title.as_str()).unwrap_or("")),
            Some((t, true)) => format!("{} – {}", t_(self.lang, S::BoxColor), b.tabs.get(t).map(|x| x.title.as_str()).unwrap_or("")),
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
            "auto": if matches!(tab, Some((_, false))) { format!("#{ar:02X}{ag:02X}{ab:02X}") } else if self.light { "#F3F3F3".to_string() } else { "#202020".to_string() },
            "box": id,
            "key": match tab { Some((t, false)) => format!("tab:{t}"), Some((t, true)) => format!("tabbox:{t}"), None => "box".into() },
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
                    Some((t, false)) => {
                        if let Some(t) = b.tabs.get_mut(t) {
                            t.color = c;
                        }
                    }
                    Some((t, true)) => {
                        if let Some(t) = b.tabs.get_mut(t) {
                            t.box_color = c;
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

    /// The icons a selection band touches: in its box (`scope`), or loose on the desktop.
    fn band_hits(&self, band: Rect, scope: Option<u32>) -> BTreeSet<ItemKey> {
        self.cells_in_view()
            .into_iter()
            .filter(|(k, cell, b)| {
                // (A cell far from the band can't be touched: its long name reaches a cell down at most.)
                if *b != scope || !cell.intersects(&band.inflate(self.grid.cell_h)) {
                    return false;
                }
                let visible = b.and_then(|id| self.box_by_id(id)).map(|bx| Self::content_rect(bx).intersects(cell)).unwrap_or(true);
                visible && self.hit_rect(k, *cell, &self.metrics_of(*b)).intersects(&band)
            })
            .map(|(k, _, _)| k)
            .collect()
    }

    /// Something dragged over us (our icons or files from an app): a rolled-up box opens, a tab
    /// held over switches, a box's icons scroll near its top or bottom.
    fn drag_hover(&mut self, p: Pt) {
        self.hover_reveal(p, true);
        match self.hit_test(p) {
            Hit::Header { id, tab: Some(t), .. } => self.arm_tab_switch(id, t, 450),
            _ => self.pending_tab = None,
        }
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
                self.invalidate();
            }
        }
    }

    /// Windows' drag and drop over our surface (`p` None: it left or dropped), and the icon that
    /// would take the drop (highlighted, as Explorer does).
    pub fn ole_drag_over(&mut self, p: Option<Pt>, hl: Option<ItemKey>) {
        if let Some(p) = p {
            self.drag_hover(p);
        } else {
            self.pending_tab = None;
            self.pending_reveal = None;
        }
        // Redrawn only when the feedback changes: the insertion marker moves by slot.
        let slot = p.and_then(|p| self.drop_box(p).and_then(|id| self.box_by_id(id)).map(|b| (b.id, self.box_drop_slot(b, p).0)));
        if hl != self.drop_hl || slot != self.drop_slot || p.is_some() != self.drop_at.is_some() {
            self.drop_slot = slot;
            self.drop_hl = hl;
            self.invalidate();
        }
        self.drop_at = p;
        // Our icons in between others make room; not over an icon that takes the drop (Windows'
        // own target, highlighted), nor for files from an app.
        match self.own_drag.as_ref().map(|(k, _)| k.clone()).filter(|_| self.drop_hl.is_none()) {
            Some(keys) if p.is_some() => self.make_room(p, &keys),
            _ => self.make_room(None, &[]),
        }
        drag_wheel_hook(p.is_some());
        self.page_edge_over(p);
    }

    /// The wheel turned while something is dragged over us (Windows' drag holds the mouse, so
    /// the wheel never reaches our window; see `drag_wheel_hook`): a box under the pointer
    /// scrolls. The drag image is Windows', on the pointer, so it stays there; the insertion
    /// marker and the icon that takes the drop follow the scrolled icons. True: handled.
    pub fn drag_wheel(&mut self, x: i32, y: i32, delta: i16) {
        let p = self.screen_to_dip(x, y);
        self.on_wheel(p, delta, false);
        self.drop_slot = None;
        self.invalidate();
        unsafe {
            SetTimer(Some(self.msg_hwnd), T_DRAGNUDGE, 80, None);
        }
    }

    /// The wheel at screen `x`, `y` is for `drag_wheel`: something is dragged over a box there.
    pub fn drag_wheel_over(&self, x: i32, y: i32) -> bool {
        self.drop_at.is_some() && self.drop_box(self.screen_to_dip(x, y)).is_some()
    }

    /// After scrolling under a drag: Windows asks the drop target again on a mouse move, so a
    /// move of nothing makes the icon now under the pointer the one that takes the drop. Once
    /// the wheel rests, outside the mouse hook (input sent from inside it waits for the hook
    /// itself: the wheel froze for a moment each notch).
    fn drag_nudge(&mut self) {
        if self.drop_at.is_none() {
            return;
        }
        unsafe {
            let i = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: MOUSEEVENTF_MOVE, ..Default::default() } } };
            SendInput(&[i], std::mem::size_of::<INPUT>() as i32);
        }
    }

    /// The window under the pointer is our surface (or the desktop it sits on).
    pub fn pointer_over_us(&self) -> bool {
        let mut c = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut c);
            let w = WindowFromPoint(c);
            w == self.hwnd || GetAncestor(w, GA_ROOT) == GetAncestor(self.hwnd, GA_ROOT)
        }
    }

    /// A box's color as shown now: its open tab's box color, else that tab's own color (a
    /// colored tab colors the box behind it), else the box's (None = automatic).
    pub fn box_color(b: &BoxModel) -> Option<Rgba> {
        b.tabs.get(b.active_tab).and_then(|t| t.box_color.or(t.color)).or(b.style.color)
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
        // Over another window (an app above the box): the box is not pointed at any more.
        let over_us = self.pointer_over_us();
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
                // Busy with it: moved, resized, icons dragged (ours or Windows' drag), a selection
                // band started in it (it stays open however far the band goes).
                let busy = matches!(self.drag, Drag::MoveBox { id: d, .. } | Drag::Resize { id: d, .. } if d == *id)
                    || matches!(self.drag, Drag::Items { .. })
                    || matches!(self.drag, Drag::Rubber { scope: Some(s), .. } if s == *id)
                    || matches!(self.drag, Drag::RightRubber { scope: Some(s), .. } if s == *id)
                    || self.own_drag.is_some()
                    || self.drop_at.map(|q| b.rect.inflate(12.0).contains(q)).unwrap_or(false);
                (over_us && b.rect.inflate(12.0).contains(p)) || busy || menu_box == Some(*id) || self.box_edit.as_ref().map(|e| e.id == *id).unwrap_or(false)
            })
            .collect();
        if keep.len() != self.revealed.len() {
            self.revealed = keep.into_iter().collect();
            self.invalidate();
        }
        // The pointer is over another window now (maybe still above the box): the box is not
        // pointed at any more, so it blends into the wallpaper again.
        if !over_us {
            self.blend_pointer(Pt { x: -1.0e6, y: -1.0e6 });
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
        let mut ws: Vec<f32> = (0..b.tabs.len())
            .map(|i| {
                let w = self
                    .gpu
                    .label(&self.title_tf_bold, &self.tab_label(b, i), 10_000.0, self.title_line_h, 1, false)
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
        // Room at the right for the magnifier and +, and at the left for a folder box's folder
        // glyph or back arrow.
        let lead = if b.is_folder() { 28.0 } else { 0.0 };
        let avail = r.w - 12.0 - 68.0 - lead - TAB_GAP * (n - 1) as f32;
        let sum: f32 = ws.iter().sum();
        if sum > avail && sum > 0.0 {
            let k = avail / sum;
            for w in &mut ws {
                *w = (*w * k).max(28.0);
            }
        }
        let mut x = r.x + 6.0 + lead;
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
                let before = b.tabs.get(b.active_tab).and_then(|x| x.box_color).or(b.style.color);
                let after = b.tabs.get(t).and_then(|x| x.box_color).or(b.style.color);
                if before != after {
                    self.tab_color_from = Some((id, before, Instant::now()));
                }
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

    /// A new tab in a box, its title being edited. A folder box's new tab shows the open tab's
    /// folder and is named after it, as in File Explorer.
    fn add_tab(&mut self, id: u32) {
        let folder = self.box_by_id(id).filter(|b| b.is_folder()).map(|b| b.folder_root().to_string());
        let title = match &folder {
            Some(f) => f.trim_end_matches('\\').rsplit('\\').next().unwrap_or("").to_string(),
            None => t(self.lang, S::NewTabTitle).to_string(),
        };
        let mut ix = None;
        self.change_layout(|l, _| ix = layout::add_tab(l, id, &title));
        if let Some(tab) = ix {
            self.scroll.remove(&id);
            self.tab_grow = Some((id, tab, Instant::now()));
            crate::anim::set(true);
            if folder.is_some() {
                self.folder_moved(id);
            } else {
                self.begin_rename(RenameTarget::Box { id, tab });
            }
        }
    }

    /// What a tab says: its title (a folder box's tab in a subfolder: that folder's name).
    pub fn tab_label(&self, b: &BoxModel, i: usize) -> String {
        if b.is_folder() && b.tab_in_sub(i) {
            return b.tab_now(i).trim_end_matches('\\').rsplit('\\').next().unwrap_or("").to_string();
        }
        b.tabs.get(i).map(|t| t.title.clone()).unwrap_or_default()
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
            let m = self.metrics(Some(b));
            for (_, k, cell) in self.box_cells_in(b, p.y - 3.0 * m.cell_h, p.y + m.cell_h) {
                if cell.bottom() < c.y || cell.y > c.bottom() {
                    continue;
                }
                if self.hit_rect(&k, cell, &m).contains(p) && c.contains(p) {
                    return Hit::Item { key: k, in_box: Some(b.id) };
                }
            }
            return Hit::BoxBody { id: b.id };
        }
        // Hidden, or under the boxes-above window (not drawn there: they are on the desktop
        // window behind, which hit-tests them itself).
        if self.is_tucked(None) || (self.is_above() && !self.hit_desk) {
            return Hit::Empty;
        }
        for (k, c) in self.layout.loose.iter() {
            if !self.items.contains_key(k) {
                continue;
            }
            let cell = self.grid.cell_rect(*c);
            if cell.inflate(2.0).contains(p) && self.hit_rect(k, cell, &self.metrics(None)).contains(p) {
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

    /// A point in our window's client area (the boxes-above window starts lower when the
    /// taskbar is at the top).
    pub fn client_dip(&self, x: i32, y: i32) -> Pt {
        self.to_dip(x + self.above_off.0, y + self.above_off.1)
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

    /// "Sort the desktop now": one undo step. With no rules yet: the default boxes, with a rule
    /// for each (it did nothing before, which said nothing about what it is for).
    pub fn sort_now(&mut self) {
        if self.layout.rules.is_empty() {
            self.apply_template();
            return;
        }
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
        // (The first scan with nothing known before: everything is "new", nothing appeared. A
        // desktop scanned empty before is another thing: its first file did appear.)
        let appeared: BTreeSet<ItemKey> = if old.is_empty() && !self.scanned_once {
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
        // (A drag that took things to another page: recorded against the layout before it, so
        // one undo puts them back on their page.)
        let before = self.pages.before.take().unwrap_or_else(|| self.layout.clone());
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
        // Another page now (undo, a saved layout): it slides in.
        self.page_check_shown();
        // Undo, a saved layout or a deleted box can change what folder boxes show.
        self.folder_sync();
        if self.host.is_some() && self.settings_sig() != self.settings_sent {
            self.send_settings(false, None);
        }
        self.ensure_backdrops();
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

    /// The icon snapshot (for a fast next start), when its items or icons changed since it was
    /// last written (a layout change alone does not touch it). Encoded and written off the UI
    /// thread.
    pub fn save_snapshot(&mut self) {
        if !self.scanned_once || !self.snapshot_dirty {
            return;
        }
        self.snapshot_dirty = false;
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
        let path = self.paths.snapshot.clone();
        // (One write at a time: two would share the temporary file.)
        static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let r = std::thread::Builder::new().name("snapshot".into()).spawn(move || {
            let _g = WRITING.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = persist::write_atomic(&path, &s.encode()) {
                log::warn!("save snapshot: {e}");
            }
        });
        match r {
            Ok(j) => self.snapshot_job = Some(j),
            Err(e) => log::warn!("save snapshot: no thread: {e}"),
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
                json!({ "key": k.0, "name": it.display_name, "path": it.parsing_name, "rect": px(self.hit_rect(&k, cell, &self.metrics_of(b))), "box": b, "selected": self.sel.is_selected(&k) })
            })
            .collect();
        let boxes: Vec<Value> = self
            .layout
            .boxes
            .iter()
            .map(|b| {
                let hl = self.header_layout(b);
                let tabs: Vec<Value> = b.tabs.iter().enumerate().map(|(i, t)| json!({"title": t.title, "items": t.items.len(), "rect": hl.tabs.get(i).map(|r| px(*r))})).collect();
                json!({ "id": b.id, "rect": px(b.rect), "tabs": tabs, "active": b.active_tab, "view": b.view, "icon_size": b.icon_size })
            })
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
            let m = self.metrics_of(bx);
            let (lw, full, row) = self.label_spec(&k, cell, &m);
            let (icon_rect, label) = self.item_parts(&k, cell, full, &m);
            let lk = App::lkey(&k, lw, full, row);
            self.vanishing.push(Vanish {
                icon: self.icon_bitmap(it, &m).map(|(_, b)| b.clone()),
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
        self.appearing.retain(|_, t0| t0.elapsed().as_secs_f32() < APPEAR_S);
        !self.vanishing.is_empty() || !self.appearing.is_empty()
    }

    /// Icons this scan adds grow and fade in where they go (the mirror of `capture_vanishing`).
    /// Not on the first scan, not for many at once, and not when the same scan removes icons
    /// (a rename or a move shows as remove + add).
    fn capture_appearing(&mut self, old: &BTreeSet<ItemKey>, first: bool) {
        if first || !self.scanned_once || old.is_empty() || !self.settings.show_icons || !crate::pages::windows_animates() {
            return;
        }
        if old.iter().any(|k| !self.items.contains_key(k)) {
            return;
        }
        let added: Vec<ItemKey> = self.items.keys().filter(|k| !old.contains(*k)).cloned().collect();
        if added.is_empty() || added.len() > 64 {
            return;
        }
        let now = Instant::now();
        for k in added {
            self.appearing.insert(k, now);
        }
        crate::anim::set(true);
        self.invalidate();
    }

    /// How far an appearing icon is (eased 0..1; 1 = there, or not appearing).
    pub fn appear_t(&self, k: &ItemKey) -> f32 {
        match self.appearing.get(k) {
            Some(t0) => ease_out(t0.elapsed().as_secs_f32() / APPEAR_S),
            None => 1.0,
        }
    }

    /// Icon images no item uses any more (thumbnails of files gone or changed, a folder box's
    /// last folder) go, on the CPU and the GPU: unless the worker may hand their id out again
    /// (`keep`), or a size of one still used is made from them.
    pub fn evict_icons(&mut self, keep: &[u32]) {
        let mut live: std::collections::HashSet<u32> = keep.iter().copied().collect();
        live.extend(self.items.values().map(|i| i.icon));
        self.views.sized.retain(|(base, _), _| live.contains(base));
        self.views.pending.retain(|(base, _)| live.contains(base));
        live.extend(self.views.sized.values().copied());
        let (n_img, n_bmp) = (self.icon_imgs.len(), self.bitmaps.len());
        self.icon_imgs.retain(|id, _| live.contains(id));
        self.bitmaps.retain(|id, _| live.contains(id));
        let (d_img, d_bmp) = (n_img - self.icon_imgs.len(), n_bmp - self.bitmaps.len());
        if d_img + d_bmp > 0 {
            log::info!("icons let go: {d_img} image(s), {d_bmp} bitmap(s); {} / {} kept", self.icon_imgs.len(), self.bitmaps.len());
            self.snapshot_dirty = true;
        }
    }

    fn apply_scan(&mut self, items: Vec<Item>, icons: Vec<(u32, IconImage)>, native: Option<deskboxes_shell::view::NativeView>, ms: f64, first: bool) {
        let t = Instant::now();
        // What the icon snapshot holds may have changed.
        self.snapshot_dirty = true;
        if first && !self.seeded {
            // The worker numbers icons independently of an unusable snapshot: replace the table.
            self.bitmaps.clear();
            self.icon_imgs.clear();
            self.views.forget_icons();
        }
        self.views.forget_texts();
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
        self.capture_appearing(&old, first);
        self.items_gen += 1;
        self.shown_cache.borrow_mut().clear();
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
                    // On the page it was dropped on, even if another is shown by now.
                    let g = self.grid;
                    let page = self.pending_drop_page.take().filter(|p| deskboxes_core::pages::index_of(&self.layout, *p).is_some()).unwrap_or(self.layout.page);
                    deskboxes_core::pages::with_page(&mut self.layout, page, |l| match bx {
                        Some(id) => {
                            layout::move_to_box(l, &fresh, id, None);
                        }
                        None => layout::move_to_grid(l, &fresh, g.cell_at(at), &g),
                    });
                    self.pending_drop = None;
                }
            } else {
                self.pending_drop = None;
            }
        }
        let mut created: Option<ItemKey> = None;
        if let Some((when, at, bx, before)) = self.pending_new.clone() {
            if when.elapsed().as_secs() < 10 {
                let fresh: Vec<ItemKey> = present.iter().filter(|k| !before.contains(*k) && !old.contains(*k)).cloned().collect();
                if let Some(k) = fresh.first() {
                    match bx.filter(|id| self.box_by_id(*id).map(|b| !b.is_folder()).unwrap_or(false)) {
                        Some(id) => {
                            layout::move_to_box(&mut self.layout, &fresh, id, None);
                        }
                        None => layout::move_to_grid(&mut self.layout, &fresh, self.grid.cell_at(at), &self.grid),
                    }
                    dropped.extend(fresh.iter().cloned());
                    self.no_glide.extend(fresh.iter().cloned());
                    created = Some(k.clone());
                    self.pending_new = None;
                }
            } else {
                self.pending_new = None;
            }
        }
        // Icons an undo brought back: where they were.
        let back = self.place_returning(&old, &present);
        dropped.extend(back.iter().cloned());
        self.no_glide.extend(back);
        // A new icon named after "New": the rules sort it now.
        let renamed_new = self.sort_after_rename.clone().filter(|k| self.rename.as_ref().map(|r| &r.key) != Some(k) && (self.scanned_once || !old.is_empty()));
        let changed = self.reconcile_pages(&present);
        let changed = self.sort_new_items(&old, &dropped) | changed;
        let changed = match renamed_new {
            Some(k) if self.items.contains_key(&k) => {
                self.sort_after_rename = None;
                let one: BTreeSet<ItemKey> = std::iter::once(k).collect();
                self.apply_rules(Some(&one), true) | changed
            }
            _ => changed,
        };
        self.fill_folder_boxes();
        if first && self.template_pending {
            self.template_pending = false;
            log::info!("first run: the default boxes");
            self.apply_template();
        }
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
        if let Some(k) = created {
            self.sel.set = std::iter::once(k.clone()).collect();
            self.sel.focus = Some(k.clone());
            self.sort_after_rename = Some(k.clone());
            self.pending_rename = Some(k);
        }
        if changed || first {
            unsafe {
                SetTimer(Some(self.msg_hwnd), T_SAVE, 1000, None);
            }
        }
        if first && self.hide_after_scan {
            self.hide_after_scan = false;
            self.draw();
            if !self.frame_ok {
                log::warn!("the first scan could not be drawn: Explorer's icons stay");
                return;
            }
            let th = Instant::now();
            let s = hide::set_hidden(true);
            self.stats.hide_ms = th.elapsed().as_secs_f64() * 1000.0;
            log::info!("PHASE hide_icons (after first scan) ms={:.1} state={s:?}", self.stats.hide_ms);
            return;
        }
        if let Some(k) = self.pending_rename.take() {
            self.begin_rename(RenameTarget::Item(k));
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
                    None => vec![],
                };
                // Deskboxes' own things on the desktop all live in one submenu, in groups.
                let mut sub = vec![];
                if bid.is_none() {
                    sub.push(json!({ "id": "dbx.newbox", "text": t(self.lang, S::NewBox), "glyph": "\u{E710}" }));
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
                    // Any folder: Windows' folder picker.
                    let mut kf = kf;
                    if !kf.is_empty() {
                        kf.push(json!({ "separator": true }));
                    }
                    kf.push(json!({ "id": "dbx.fbchoose", "text": t(self.lang, S::FolderBoxChoose), "glyph": "\u{ED25}" }));
                    sub.push(json!({ "id": "dbx.newfolderbox", "text": t(self.lang, S::NewFolderBox), "glyph": "\u{E8B7}", "children": kf }));
                    // Desktop pages: go to one, make, rename or delete one.
                    if self.pages_on() {
                        sub.push(json!({ "id": "dbx.pagesmenu", "text": t(self.lang, S::PagesMenu), "glyph": crate::pages::PAGES_GLYPH, "children": self.pages_menu_entries() }));
                    }
                    sub.push(json!({ "separator": true }));
                }
                sub.push(json!({ "id": "dbx.sortnow", "text": t(self.lang, self.sort_now_text()), "glyph": "\u{E8CB}" }));
                sub.push(json!({ "id": "dbx.rules", "text": t(self.lang, S::SortingRules), "glyph": "\u{E71C}" }));
                sub.push(json!({ "separator": true }));
                if self.settings.saved.in_menu {
                    // Saved layouts: save now, go back to one of the newest, or manage them.
                    let mut sv = vec![json!({ "id": "dbx.savenow", "text": t(self.lang, S::SaveLayoutNow), "glyph": "\u{E74E}" })];
                    let list = self.newest_saved_ids(8);
                    if !list.is_empty() {
                        sv.push(json!({ "separator": true }));
                        for id in list {
                            sv.push(json!({ "id": format!("dbx.restore:{id}"), "text": crate::saved::shown(id), "glyph": "\u{E81C}" }));
                        }
                    }
                    sv.push(json!({ "separator": true }));
                    sv.push(json!({ "id": "dbx.savedpage", "text": t(self.lang, S::ManageSaved), "glyph": "\u{E713}" }));
                    sub.push(json!({ "id": "dbx.savedmenu", "text": t(self.lang, S::SavedLayouts), "glyph": "\u{E81C}", "children": sv }));
                }
                if self.settings.privacy.allowed {
                    sub.push(json!({ "id": "dbx.privacy", "text": t(self.lang, S::PrivacyMode), "glyph": "\u{ED1A}", "checkable": true, "checked": self.settings.privacy.on }));
                }
                sub.push(json!({ "id": "dbx.lockall", "text": t(self.lang, S::LockEverything), "glyph": "\u{E72E}", "checkable": true, "checked": self.settings.lock_all }));
                sub.push(json!({ "separator": true }));
                sub.push(json!({ "id": "dbx.settings", "text": t(self.lang, S::Settings), "glyph": "\u{E713}" }));
                sub.push(json!({ "id": "dbx.disable", "text": t(self.lang, S::Disable), "glyph": "\u{E7E8}" }));
                extra.push(json!({ "id": "dbx.menu", "text": t(self.lang, S::BoxesSubmenu), "placement": "beforeNew", "children": sub, "icon": format!("{},0", std::env::current_exe().unwrap_or_default().display()) }));
                let _ = (at, new_rect);
                // View and Sort by, as Explorer's desktop has them (Windows' own text, "@dll,-id").
                let size = self.icon_dip.round() as u32;
                let all = json!({ "id": "dbx.allviewmenu", "text": t(self.lang, S::AllBoxesView), "glyph": "\u{E8A9}", "children": self.view_entries("dbx.allview:", &self.all_boxes_preset()) });
                // A box: its own view (as a File Explorer window's), then all boxes'. The desktop:
                // its icon size, as Explorer's desktop has it, then all boxes'.
                let view = match bid.and_then(|id| self.box_by_id(id)) {
                    Some(b) => {
                        let mut v = self.view_entries("dbx.view:", crate::views::preset_of(b));
                        v.push(json!({ "separator": true }));
                        v.push(all);
                        v
                    }
                    None => vec![
                        json!({ "id": "iconsize.large", "text": "@ExplorerFrame.dll,-41729", "radio": true, "checked": size >= 40 }),
                        json!({ "id": "iconsize.medium", "text": "@ExplorerFrame.dll,-41730", "radio": true, "checked": (24..40).contains(&size) }),
                        json!({ "id": "iconsize.smallicon", "text": "@ExplorerFrame.dll,-41731", "radio": true, "checked": size < 24 }),
                        json!({ "separator": true }),
                        all,
                        json!({ "separator": true }),
                        json!({ "id": "showdesktop", "text": t(self.lang, S::ShowDesktopIcons), "checkable": true, "checked": self.settings.show_icons }),
                    ],
                };
                // A folder box's menu is its folder's (Windows' own, without our View): its view
                // goes at the top with the box's entries, right under Box settings.
                if bid.and_then(|id| self.box_by_id(id)).map(|b| b.is_folder()).unwrap_or(false) {
                    let at = extra.iter().position(|e| e["id"] == "dbx.boxmenu").map(|i| i + 1).unwrap_or(0);
                    extra.insert(at, json!({ "id": "dbx.viewmenu", "text": t(self.lang, S::ViewMenu), "glyph": "\u{E8A9}", "children": view.clone(), "placement": "top" }));
                }
                let sort = vec![
                    json!({ "id": "sort.name", "text": "@propsys.dll,-38671" }),
                    json!({ "id": "sort.size", "text": "@propsys.dll,-38657" }),
                    json!({ "id": "sort.type", "text": "@propsys.dll,-38413" }),
                    json!({ "id": "sort.modified", "text": "@propsys.dll,-38407" }),
                ];
                // In a folder box: its folder's menu (New, Paste go there).
                if let Some(f) = bid.and_then(|id| self.box_by_id(id)).filter(|b| b.is_folder()).map(|b| b.folder_now().to_string()) {
                    return json!({ "kind": "background", "folder": f, "desktop": false, "shift": key_down(VK_SHIFT), "undo": self.hist.can_undo(), "redo": self.hist.can_redo(), "undo_text": self.undo_text(true), "redo_text": self.undo_text(false), "extra": extra, "own": { "view": view, "arrange": sort } });
                }
                json!({ "kind": "background", "folder": self.folder_arg(), "desktop": true, "shift": key_down(VK_SHIFT), "undo": self.hist.can_undo(), "redo": self.hist.can_redo(), "undo_text": self.undo_text(true), "redo_text": self.undo_text(false), "extra": extra, "own": { "view": view, "arrange": sort } })
            }
        }
    }

    /// What "Sort now" does: by the rules, or (none yet) into new boxes by type.
    fn sort_now_text(&self) -> S {
        if self.layout.rules.is_empty() {
            S::SortByType
        } else {
            S::SortNow
        }
    }

    /// Explorer's views as menu entries (`prefix` + preset id), `cur` checked.
    fn view_entries(&self, prefix: &str, cur: &str) -> Vec<Value> {
        let l = self.lang;
        let e = |id: &str, s: S| json!({ "id": format!("{prefix}{id}"), "text": t(l, s), "radio": true, "checked": cur == id });
        vec![
            e("xl", S::ViewXl),
            e("large", S::ViewLarge),
            e("medium", S::ViewMedium),
            e("small", S::ViewSmall),
            json!({ "separator": true }),
            e("list", S::ViewList),
            e("details", S::ViewDetails),
            e("tiles", S::ViewTiles),
            json!({ "separator": true }),
            e("desktop", S::ViewAsDesktop),
        ]
    }

    /// The view every box has, if they all have the same one ("" else).
    fn all_boxes_preset(&self) -> String {
        let mut v = self.layout.boxes.iter().map(crate::views::preset_of);
        match v.next() {
            Some(first) if v.all(|x| x == first) => first.to_string(),
            _ => String::new(),
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
        serde_json::to_string(&(boxes, &self.settings.default_style, &self.settings.language, sorting, &self.settings.tabs, &self.settings.rollup, look, self.pages_sig())).unwrap_or_default()
    }

    fn autostart_on() -> bool {
        autostart::is_on()
    }

    /// Send the settings window its data; `open` shows it, otherwise an open one is refreshed.
    pub fn send_settings(&mut self, open: bool, focus_box: Option<u32>) {
        let Some(h) = self.host.clone() else { return };
        // What is sent is noted before it is built: the updater changes its status on its own
        // thread, and a change made meanwhile (a download going from 100% to "ready") must still
        // count as not sent, or Settings kept showing 100% until it was opened again.
        let sig = self.settings_sig();
        let upd = crate::update::status();
        let style = |st: &BoxStyle| json!({ "color": st.color.map(|c| format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b)).unwrap_or_default(), "opacity": st.opacity, "blur": st.blur });
        let strings: serde_json::Map<String, Value> = settings_strings(self.lang).into_iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
        let colors: Vec<Value> = BOX_COLORS
            .iter()
            .enumerate()
            .map(|(i, (_, _, (r, g, b)))| json!({ "name": color_name(self.lang, i), "rgb": format!("#{r:02X}{g:02X}{b:02X}") }))
            .collect();
        let boxes: Vec<Value> = self.layout.boxes.iter().map(|b| json!({ "id": b.id, "title": b.title(), "style": style(&b.style), "az": b.az, "tint": b.tint, "blend": b.blend, "stayVisible": b.stay_visible })).collect();
        // Tabs rules can send icons to: each tab by its title (a plain box: the box title), on
        // every desktop page (another page's: its name after it).
        let cur_page = self.layout.page;
        let page_names: HashMap<u32, String> = deskboxes_core::pages::ids(&self.layout).into_iter().map(|p| (p, self.page_title(p))).collect();
        // (Not a folder box's: desktop icons never go into one.)
        let targets: Vec<Value> = deskboxes_core::pages::all_boxes(&self.layout)
            .filter(|(_, b)| !b.is_folder())
            .flat_map(|(p, b)| {
                let page = (p != cur_page).then(|| page_names.get(&p).cloned()).flatten();
                b.tabs.iter().map(move |t| {
                    let mut name = if b.tabs.len() > 1 {
                        format!("{}   ({})", t.title, b.tabs.iter().map(|x| x.title.as_str()).collect::<Vec<_>>().join(" · "))
                    } else {
                        t.title.clone()
                    };
                    if let Some(pn) = &page {
                        name = format!("{name}   — {pn}");
                    }
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
                "status": &upd,
                "version": crate::update::current(),
            },
            "above": { "hotkey": self.settings.above.hotkey, "corner": self.settings.above.corner, "dim": self.settings.above.dim, "hotkeyOk": self.above_hotkey_ok },
            "pages": self.pages_json(),
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
        self.settings_sent = sig;
        self.update_sent = serde_json::to_string(&upd).unwrap_or_default();
    }

    /// A change made in the settings window, applied live.
    /// Write settings.json a moment from now: a slider in the settings window sends a value
    /// per step, and writing each one made dragging it slow on slower PCs.
    pub fn flush_settings(&mut self) {
        if std::mem::take(&mut self.settings_dirty) {
            let _ = persist::save_settings(&self.paths.settings, &self.settings);
        }
    }

    pub fn save_settings_soon(&mut self) {
        self.settings_dirty = true;
        unsafe {
            SetTimer(Some(self.msg_hwnd), T_SAVE_SETTINGS, 400, None);
        }
    }

    fn apply_setting(&mut self, v: &Value) {
        let (scope, key, val) = (v["scope"].as_str().unwrap_or(""), v["key"].as_str().unwrap_or(""), &v["value"]);
        let id = v["box"].as_f64().unwrap_or(0.0) as u32;
        if scope == "pick" {
            return self.on_picked_color(v);
        }
        if scope == "dialog" {
            return self.on_dialog(v);
        }
        if scope == "pages" {
            log::info!("setting pages.{key} = {val}");
            return self.apply_pages_setting(key, val);
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
                    let wd = std::env::current_exe().unwrap_or_default().with_file_name("deskboxes-watchdog.exe");
                    log::info!("autostart on: {:?}", autostart::enable(&wd));
                } else {
                    autostart::disable();
                    log::info!("autostart off");
                }
            }
            ("app", "language") => {
                self.settings.language = val.as_str().unwrap_or("").to_string();
                self.lang = Lang::pick(&self.settings.language, unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() });
                self.save_settings_soon();
                self.send_settings(false, None);
            }
            ("app", "disable") => self.on_command("dbx.disable", None),
            ("app", "privacy") => {
                self.settings.privacy.allowed = val.as_bool().unwrap_or(true);
                if !self.settings.privacy.allowed {
                    self.set_privacy(false);
                }
                self.save_settings_soon();
                self.settings_sent = self.settings_sig();
            }
            ("app", "lockAll") => {
                self.settings.lock_all = val.as_bool().unwrap_or(false);
                self.save_settings_soon();
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
                self.save_settings_soon();
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
                self.save_settings_soon();
                self.settings_sent = self.settings_sig();
                self.invalidate();
            }
            ("search", k) => {
                let on = val.as_bool().unwrap_or(true);
                match k {
                    "field" => self.settings.search.field = on,
                    _ => self.settings.search.type_to_search = on,
                }
                self.save_settings_soon();
                self.settings_sent = self.settings_sig();
                self.invalidate();
            }
            ("icons", k) => {
                let ic = &mut self.settings.icons;
                match k {
                    "tint" => ic.tint = val.as_str().unwrap_or("").to_string(),
                    "tintStrength" => ic.tint_strength = val.as_f64().unwrap_or(1.0).clamp(0.0, 1.0) as f32,
                    "blend" => ic.blend = val.as_bool().unwrap_or(false),
                    "blendStrength" => ic.blend_strength = val.as_f64().unwrap_or(0.8).clamp(0.0, 1.0) as f32,
                    "desktop" => ic.desktop = val.as_bool().unwrap_or(false),
                    "blendBoxes" => ic.blend_boxes = val.as_bool().unwrap_or(true),
                    "fullHover" => ic.full_on_hover = val.as_bool().unwrap_or(true),
                    "fullActive" => ic.full_when_active = val.as_bool().unwrap_or(true),
                    _ => {}
                }
                self.save_settings_soon();
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
                        self.save_settings_soon();
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
                    self.save_settings_soon();
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
                self.save_settings_soon();
                self.send_settings(false, None);
                self.invalidate();
            }
            ("hide", _) => {
                self.settings.hide.double_click = val.as_bool().unwrap_or(false);
                if !self.settings.hide.double_click {
                    self.set_tucked(false);
                }
                self.save_settings_soon();
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
                self.save_settings_soon();
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
                self.save_settings_soon();
                // Only fonts and sizes need new text formats (a slider sends many values).
                if refresh || matches!(k, "titleSize" | "titleFont" | "titleWeight" | "radius") {
                    self.apply_look();
                } else {
                    set_look_metrics(&self.settings.look);
                    self.invalidate();
                }
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
                self.save_settings_soon();
                self.invalidate();
            }
            ("default", "applyAll") => {
                let st = self.settings.default_style.clone();
                // Every page's boxes (one undo step).
                self.change_layout(|l, _| {
                    for b in l.boxes.iter_mut().chain(l.parked.values_mut().flat_map(|c| c.boxes.iter_mut())) {
                        b.style = st.clone();
                    }
                });
                self.send_settings(false, None);
            }
            ("default", _) => {
                set_style(&mut self.settings.default_style);
                self.save_settings_soon();
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
                let tabs: Vec<u32> = self.layout.boxes.iter().filter(|b| !b.is_folder()).flat_map(|b| b.tabs.iter().map(|t| t.id)).collect();
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
                    deskboxes_core::rules::order_rule(l, id);
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
                    // Another type: a narrower one (games) moves ahead of a broader one (programs)
                    // that would take its icons first.
                    if field == "op" {
                        deskboxes_core::rules::order_rule(l, id);
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
            if self.pages_on() && self.icons_movable(&self.sel.ordered(&self.order()), None) {
                v.push(self.move_to_page_entry("dbx.pageitems:", Some("beforeProperties")));
            }
        }
        if let [one] = items {
            // A folder in a folder box: open it in a new tab of that box (as Explorer's).
            let in_fb = self.items.values().find(|i| i.parsing_name == *one).and_then(|i| self.folder_to_open(&i.key)).is_some();
            if in_fb && self.can_show_folder(one) {
                v.push(json!({ "id": "dbx.opennewtab", "text": t(self.lang, S::OpenNewTab), "glyph": "\u{E8A7}", "placement": "top" }));
            }
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
        // Everything about the box sits in one submenu at the top of its menu, in short groups:
        // the box and its tabs, then Look, Sorting and Lock, then delete and more settings.
        let palette = |prefix: &str, cur: Option<Rgba>| -> Vec<Value> {
            let mut v = vec![json!({ "id": format!("{prefix}auto"), "text": t(l, S::ColorAuto), "radio": true, "checked": cur.is_none() })];
            for (i, (_, _, (r, g, bl))) in BOX_COLORS.iter().enumerate() {
                let on = cur.map(|c| (c.r, c.g, c.b) == (*r, *g, *bl)).unwrap_or(false);
                v.push(json!({ "id": format!("{prefix}{i}"), "text": color_name(l, i), "radio": true, "checked": on, "color": format!("#{r:02X}{g:02X}{bl:02X}") }));
            }
            let custom = cur.filter(|c| !BOX_COLORS.iter().any(|(_, _, (r, g, bl))| (c.r, c.g, c.b) == (*r, *g, *bl)));
            v.push(json!({ "separator": true }));
            let mut cc = json!({ "id": format!("{prefix}custom"), "text": t(l, S::CustomColor), "radio": true, "checked": custom.is_some() });
            if let Some(c) = custom {
                cc["color"] = json!(format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b));
            }
            v.push(cc);
            v
        };
        let multi = b.tabs.len() > 1;
        let mut sub = vec![];
        if !multi {
            sub.push(json!({ "id": "dbx.renamebox", "text": t(l, S::RenameBox), "glyph": "\u{E8AC}" }));
        }
        sub.push(json!({ "id": "dbx.addtab", "text": t(l, S::AddTab), "glyph": "\u{E710}" }));
        if multi {
            let ti = at_tab.unwrap_or(b.active_tab).min(b.tabs.len() - 1);
            let tab = &b.tabs[ti];
            // A right-clicked tab has Rename / Remove at the top of the menu: not again here.
            let tab_top = at_tab.is_some() && !self.locks(b).tabs;
            let mut this = vec![];
            if !tab_top {
                this.push(json!({ "id": "dbx.renamebox", "text": t(l, S::RenameTab), "glyph": "\u{E8AC}" }));
            }
            this.push(json!({ "id": "dbx.tabcolormenu", "text": t(l, S::TabColor), "glyph": "\u{E790}", "children": palette("dbx.tabcolor:", tab.color) }));
            this.push(json!({ "id": "dbx.tabboxmenu", "text": t(l, S::TabBoxColor), "glyph": "\u{E771}", "children": palette("dbx.tabbox:", tab.box_color) }));
            this.push(json!({ "separator": true }));
            this.push(json!({ "id": "dbx.detachtab", "text": t(l, S::SplitTab), "glyph": "\u{E8A7}" }));
            if !tab_top {
                this.push(json!({ "id": "dbx.removetab", "text": t(l, S::RemoveTab), "glyph": "\u{E711}" }));
            }
            sub.push(json!({ "id": "dbx.thistab", "text": format!("{} ({})", t(l, S::ThisTab), tab.title), "glyph": "\u{E8A9}", "children": this }));
        }
        sub.push(json!({ "separator": true }));
        // Look: colors, glass, icons, copying the look.
        let mut look = vec![
            json!({ "id": "dbx.colormenu", "text": t(l, if multi { S::ColorAllTabs } else { S::ColorMenu }), "glyph": "\u{E790}", "children": colors }),
            json!({ "id": "dbx.opacitymenu", "text": t(l, S::Transparency), "glyph": "\u{E81E}", "children": transp }),
            json!({ "id": "dbx.blurmenu", "text": t(l, S::Blur), "glyph": "\u{E8B9}", "children": blurs }),
            json!({ "separator": true }),
        ];
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
        look.push(json!({ "id": "dbx.tintmenu", "text": t(l, S::IconTint), "glyph": "\u{E7E6}", "children": tints }));
        let bl = b.blend.as_str();
        look.push(json!({ "id": "dbx.blendmenu", "text": t(l, S::BlendIn), "glyph": "\u{E91B}", "children": [
            { "id": "dbx.blend:", "text": t(l, S::AsAllBoxes), "radio": true, "checked": bl.is_empty() },
            { "id": "dbx.blend:on", "text": t(l, S::On), "radio": true, "checked": bl == "on" },
            { "id": "dbx.blend:off", "text": t(l, S::Off), "radio": true, "checked": bl == "off" },
        ] }));
        if self.settings.hide.double_click {
            look.push(json!({ "id": "dbx.stayvisible", "text": t(l, S::StayVisible), "checkable": true, "checked": b.stay_visible }));
        }
        look.push(json!({ "separator": true }));
        look.push(json!({ "id": "dbx.copystyle", "text": t(l, S::CopyStyle), "glyph": "\u{E8C8}" }));
        look.push(json!({ "id": "dbx.pastestyle", "text": t(l, S::PasteStyle), "glyph": "\u{E77F}", "enabled": self.style_clip.is_some() }));
        sub.push(json!({ "id": "dbx.lookmenu", "text": t(l, S::LookMenu), "glyph": "\u{E771}", "children": look }));
        // Sorting: icon types that go into this tab automatically, A–Z.
        let tab_id = b.tabs.get(at_tab.unwrap_or(b.active_tab).min(b.tabs.len().saturating_sub(1))).map(|t| t.id).unwrap_or(0);
        let mut auto = vec![];
        for (cat, _) in deskboxes_core::rules::CATEGORIES.iter() {
            let on = self.layout.rules.iter().any(|r| r.enabled && r.kind == "type" && r.op == *cat && r.target == tab_id);
            auto.push(json!({ "id": format!("dbx.rulecat:{cat}"), "text": category_name(l, cat), "checkable": true, "checked": on }));
        }
        let sorting = vec![
            json!({ "id": "dbx.automenu", "text": t(l, S::SortHere), "glyph": "\u{E8CB}", "children": auto }),
            json!({ "id": "dbx.newhere", "text": t(l, S::NewIconsHere), "checkable": true, "checked": self.layout.new_items_to == Some(tab_id) }),
            json!({ "id": "dbx.az", "text": t(l, S::AzIndex), "checkable": true, "checked": b.az }),
            json!({ "separator": true }),
            json!({ "id": "dbx.sortnow", "text": t(l, self.sort_now_text()), "glyph": "\u{E8CB}" }),
            json!({ "id": "dbx.rules", "text": t(l, S::SortingRules), "glyph": "\u{E71C}" }),
        ];
        sub.push(json!({ "id": "dbx.sortmenu", "text": t(l, S::SortingMenu), "glyph": "\u{E8CB}", "children": sorting }));
        let lk = b.locks;
        sub.push(json!({ "id": "dbx.lockmenu", "text": t(l, S::LockMenu), "glyph": "\u{E72E}", "children": [
            { "id": "dbx.lock:position", "text": t(l, S::LockPosition), "checkable": true, "checked": lk.position },
            { "id": "dbx.lock:size", "text": t(l, S::LockSize), "checkable": true, "checked": lk.size },
            { "id": "dbx.lock:icons", "text": t(l, S::LockIcons), "checkable": true, "checked": lk.icons },
            { "id": "dbx.lock:tabs", "text": t(l, S::LockTabs), "checkable": true, "checked": lk.tabs },
            { "separator": true },
            { "id": "dbx.lockall", "text": t(l, S::LockEverything), "checkable": true, "checked": self.settings.lock_all },
        ] }));
        // (A box locked in place stays on its page too.)
        if self.pages_on() && !self.locks(b).position {
            sub.push(self.move_to_page_entry("dbx.pagebox:", None));
        }
        sub.push(json!({ "separator": true }));
        sub.push(json!({ "id": "dbx.removebox", "text": t(l, S::DeleteBox), "glyph": "\u{E74D}" }));
        sub.push(json!({ "id": "dbx.boxsettings", "text": t(l, S::MoreSettings), "glyph": "\u{E713}" }));
        if b.is_folder() {
            // No sorting rules here: it shows its folder as it is.
            sub.retain(|e| !matches!(e["id"].as_str(), Some("dbx.sortmenu")));
        }
        let mut top = vec![json!({ "id": "dbx.boxmenu", "text": t(l, S::BoxSettings), "glyph": "\u{E8A9}", "children": sub, "placement": "top" })];
        if b.is_folder() {
            if b.tab_in_sub(b.active_tab) {
                top.push(json!({ "id": "dbx.folderup", "text": t(l, S::FolderUp), "glyph": "\u{E72B}", "placement": "top" }));
            }
            top.push(json!({ "id": "dbx.folderexplore", "text": t(l, S::FolderOpenExplorer), "glyph": "\u{EC50}", "placement": "top" }));
            top.push(json!({ "id": "dbx.fbchange", "text": t(l, S::FolderBoxChange), "glyph": "\u{ED25}", "placement": "top" }));
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

    pub(crate) fn show_menu(&mut self, hit: Hit, at: Pt, keyboard: bool, new_rect: Option<Rect>) {
        let Some(h) = self.host.clone() else {
            log::warn!("context menu requested before the shell host is ready");
            return;
        };
        // (Our own entries only: the page name's menu.)
        let mut req = match self.pages.menu_own.take() {
            Some(v) => json!({ "kind": "own", "items": [], "folder": "", "extra": v }),
            None => self.menu_request(&hit, at, new_rect),
        };
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
        let ask = MenuAsk { hit: hit.clone(), at, keyboard, new_rect };
        match h.send("show", req) {
            Ok(id) => {
                log::info!("menu {id} asked for");
                self.menu = Some((id, MenuCtx { at, hit, new_rect }));
                self.menu_wait = Some((id, ask));
                self.menu_wait_replay = self.replaying_menu;
                unsafe {
                    SetTimer(Some(self.msg_hwnd), T_MENU_WAIT, MENU_WAIT_MS, None);
                }
            }
            Err(e) => log::warn!("show menu: {e}"),
        }
    }

    /// The host answered about menu `id` (shown or closed): it is not stuck.
    fn menu_answered(&mut self, id: f64) {
        if self.menu_wait.as_ref().map(|(w, _)| *w as f64 == id).unwrap_or(false) {
            self.menu_wait = None;
            unsafe {
                let _ = KillTimer(Some(self.msg_hwnd), T_MENU_WAIT);
            }
        }
    }

    /// A menu never showed: the shell host is stuck (it was, after Deskboxes was turned off and on,
    /// until it was ended by hand). End it; a new one starts and shows the menu.
    fn menu_wait_due(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.msg_hwnd), T_MENU_WAIT);
        }
        let Some((id, ask)) = self.menu_wait.take() else { return };
        log::warn!("menu {id} not shown in {MENU_WAIT_MS} ms: the shell host is stuck; starting a new one");
        // Shown again by the new host once; a menu that hangs that one too is let go (else one
        // click could use up every host restart).
        if std::mem::take(&mut self.menu_wait_replay) {
            log::warn!("menu {id} was already a replay: not asked for again");
        } else {
            self.menu_replay = Some((Instant::now(), ask));
        }
        self.menu = None;
        if let Some(c) = self.host_child.as_mut() {
            let _ = c.kill();
        }
    }

    fn on_host_msg(&mut self, v: Value) {
        let t = v["t"].as_str().unwrap_or("");
        match t {
            "ready" => {
                self.stats.host_ready_ms = self.t0.elapsed().as_secs_f64() * 1000.0;
                log::info!("PHASE host_ready ms={:.1}", self.stats.host_ready_ms);
                // Tests: Settings opened as soon as the host is there.
                if self.args.open_settings {
                    self.send_settings(true, None);
                }
            }
            "shown" => {
                log::info!("menu {} shown in {} ms", v["id"], v["ms"]);
                self.menu_answered(v["id"].as_f64().unwrap_or(-1.0));
            }
            "command" => {
                let cmd = v["cmd"].as_str().unwrap_or("").to_string();
                let ctx = self.menu.as_ref().map(|(_, c)| c.clone());
                self.on_command(&cmd, ctx);
            }
            // A menu closed for a newer one (a right-click elsewhere while it was open): the new
            // one is the open menu now.
            "closed" if self.menu.as_ref().map(|(id, _)| v["id"].as_f64() != Some(*id as f64)).unwrap_or(false) => {
                log::info!("menu {} closed for the next one", v["id"]);
                self.menu_answered(v["id"].as_f64().unwrap_or(-1.0));
            }
            "closed" => {
                self.menu_answered(v["id"].as_f64().unwrap_or(-1.0));
                log::info!("menu closed invoked={} {}", v["invoked"], v["error"]);
                if v["invoked"].as_str().map(is_new_verb).unwrap_or(false) {
                    // Windows' "New" made a file or folder: it goes where the menu was opened (or
                    // into that box) and its name is edited at once, as on Explorer's desktop.
                    if let Some((_, c)) = &self.menu {
                        let bx = match &c.hit {
                            Hit::Header { id, .. } | Hit::BoxBody { id } | Hit::Edge { id, .. } | Hit::Item { in_box: Some(id), .. } => Some(*id),
                            _ => None,
                        };
                        self.pending_new = Some((Instant::now(), c.at, bx, self.items.keys().cloned().collect()));
                    }
                }
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
            c if c.starts_with("dbx.page") => self.page_command(c, box_id),
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
            // Windows' Delete, run by us so it can be undone.
            "delete" => self.delete_selection(!key_down(VK_SHIFT)),
            "paste" | "pastelink" => {
                let (x, y) = self.to_screen(at);
                self.pending_drop = Some((Instant::now(), at, box_id));
                self.pending_drop_page = Some(self.layout.page);
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
            "dbx.fbchoose" => self.ask_folder(None, at),
            "dbx.fbchange" => {
                if let Some(id) = box_id {
                    self.ask_folder(Some(id), at);
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
            "dbx.opennewtab" => {
                let target = self.sel.focus.clone().or_else(|| self.sel.set.iter().next().cloned()).and_then(|k| self.folder_to_open(&k));
                if let Some((id, path)) = target {
                    let mut ix = None;
                    self.change_layout(|l, _| ix = layout::open_in_new_tab(l, id, &path));
                    if let Some(tab) = ix {
                        self.tab_grow = Some((id, tab, Instant::now()));
                        crate::anim::set(true);
                        self.folder_moved(id);
                    }
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
            c @ ("dbx.tabcolor:custom" | "dbx.color:custom" | "dbx.tabbox:custom") => {
                if let Some(id) = box_id {
                    let t = tab_of(&ctx).or(self.box_by_id(id).map(|b| b.active_tab)).unwrap_or(0);
                    let tab = match c {
                        "dbx.tabcolor:custom" => Some((t, false)),
                        "dbx.tabbox:custom" => Some((t, true)),
                        _ => None,
                    };
                    self.open_color_picker(id, tab);
                }
            }
            c if c.starts_with("dbx.tabbox:") => {
                // The box's background behind this tab ("auto": the box's own color).
                if let Some(id) = box_id {
                    let tab = tab_of(&ctx).or(self.box_by_id(id).map(|b| b.active_tab)).unwrap_or(0);
                    let color = c["dbx.tabbox:".len()..]
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| BOX_COLORS.get(i))
                        .map(|(_, _, (r, g, bl))| Rgba { r: *r, g: *g, b: *bl, a: 255 });
                    self.change_layout(|l, _| {
                        if let Some(t) = l.boxes.iter_mut().find(|b| b.id == id).and_then(|b| b.tabs.get_mut(tab)) {
                            t.box_color = color;
                        }
                    });
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
            // Sort by: in a box, that box's open tab (a folder box's folder); else the desktop.
            c if c.starts_with("sort.") => match box_id {
                Some(id) if self.is_folder_box(id) => self.sort_folder_box(id, &c["sort.".len()..]),
                Some(id) => self.sort_box_tab(id, &c["sort.".len()..]),
                None => self.sort_desktop(&c["sort.".len()..]),
            },
            c if c.starts_with("dbx.view:") || c.starts_with("dbx.allview:") => {
                let (k, v) = c.split_once(':').unwrap_or(("", ""));
                if let Some((view, size)) = crate::views::preset(v) {
                    if k == "dbx.allview" {
                        self.set_all_box_views(view, size);
                    } else {
                        self.set_box_view(&box_id.into_iter().collect::<Vec<_>>(), view, size);
                    }
                }
            }
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
                            "color" => {
                                // The whole box (every tab) takes it.
                                b.style.color = v.parse::<usize>().ok().and_then(|i| BOX_COLORS.get(i)).map(|(_, _, (r, g, bl))| Rgba { r: *r, g: *g, b: *bl, a: 255 });
                                for t in b.tabs.iter_mut() {
                                    t.box_color = None;
                                }
                            }
                            "opacity" => b.style.opacity = 1.0 - v.parse::<f32>().unwrap_or(40.0).clamp(0.0, 100.0) / 100.0,
                            _ => b.style.blur = v.parse::<f32>().unwrap_or(60.0).max(0.0),
                        }
                    });
                }
            }
            "dbx.disable" => {
                self.settings.enabled = false;
                let _ = persist::save_settings(&self.paths.settings, &self.settings);
                if self.desktop_mode && self.args.desktop_dir.is_none() {
                    add_turn_on_verb(&self.settings);
                }
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

    /// A box's Sort by: its open tab's icons in that order. One undo step.
    fn sort_box_tab(&mut self, id: u32, by: &str) {
        let Some(tab) = self.box_by_id(id).and_then(|b| b.active()) else { return };
        let mut its: Vec<&Item> = tab.items.iter().filter_map(|k| self.items.get(k)).collect();
        crate::folderbox::sort_items(&mut its, by);
        let order: Vec<ItemKey> = its.into_iter().map(|i| i.key.clone()).collect();
        log::info!("sort box {id} by {by}: {} icon(s)", order.len());
        self.change_layout(|l, _| {
            if let Some(t) = l.boxes.iter_mut().find(|b| b.id == id).and_then(|b| b.tabs.get_mut(b.active_tab)) {
                // Icons not on disk right now keep their places at the end.
                let rest: Vec<ItemKey> = t.items.iter().filter(|k| !order.contains(k)).cloned().collect();
                t.items = order.iter().cloned().chain(rest).collect();
            }
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
        self.views.forget_icons();
        self.labels.clear();
        self.label_bmps.clear();
        log::info!("icon size -> {dip} DIP ({} px)", self.icon_px);
        // Explorer's desktop gets the size too only when this is the real desktop (a fixture
        // or a window keeps it to itself).
        let explorer = self.desktop_mode && self.args.desktop_dir.is_none();
        let _ = self.worker.send(Req::IconSize { dip, px: self.icon_px, explorer });
        self.invalidate();
    }

    /// Delete the selected files (Delete, the menu's Delete): into the Recycle Bin, or for good
    /// with Windows' own confirmation.
    pub fn delete_selection(&mut self, recycle: bool) {
        let order = self.order();
        let paths: Vec<String> = self
            .sel
            .ordered(&order)
            .iter()
            .filter_map(|k| self.items.get(k))
            .filter(|i| i.kind != ItemKind::Virtual)
            .map(|i| i.parsing_name.clone())
            .collect();
        if !paths.is_empty() {
            let _ = self.worker.send(Req::Delete { paths, recycle });
        }
    }

    /// Undo goes back through what was done, as Explorer's: icon moves and box changes here,
    /// and moves into folders, the Recycle Bin and renames on disk.
    fn undo(&mut self) {
        match self.hist.undo(&mut self.layout) {
            Some(Step::Layout) => self.layout_restored(),
            Some(Step::Files(s)) => self.run_file_step(s, true),
            None => {}
        }
    }

    fn redo(&mut self) {
        match self.hist.redo(&mut self.layout) {
            Some(Step::Layout) => self.layout_restored(),
            Some(Step::Files(s)) => self.run_file_step(s, false),
            None => {}
        }
    }

    /// The desktop's own items (not a folder box's), by name: what reconciliation places.
    pub fn present_keys(&self) -> Vec<ItemKey> {
        let mut v: Vec<&Item> = self.items.values().filter(|i| i.source != Source::Folder).collect();
        v.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
        v.into_iter().map(|i| i.key.clone()).collect()
    }

    /// A layout from the undo history is back. It was saved before files came or went since:
    /// they are placed or let go now, as a scan would. Pages it has come back with stay
    /// reachable (an undone "turn pages off").
    fn layout_restored(&mut self) {
        if self.scanned_once {
            let present = self.present_keys();
            self.reconcile_pages(&present);
        }
        if deskboxes_core::pages::count(&self.layout) > 1 && !self.settings.pages.enabled {
            log::info!("undo brought back {} pages: pages on again", deskboxes_core::pages::count(&self.layout));
            self.settings.pages.enabled = true;
            self.save_settings_soon();
            self.send_settings(false, None);
        }
        self.layout_changed();
    }

    /// The menu's "Undo Move" / "Redo Delete" for a file step next ("" for a layout change: plain Undo).
    fn undo_text(&self, undo: bool) -> &'static str {
        let next = if undo { self.hist.next_undo() } else { self.hist.next_redo() };
        let s = match next.flatten() {
            Some(FileOp::Moved(_)) => if undo { S::UndoMove } else { S::RedoMove },
            Some(FileOp::Copied(_)) => if undo { S::UndoCopy } else { S::RedoCopy },
            Some(FileOp::Recycled(_)) => if undo { S::UndoDelete } else { S::RedoDelete },
            Some(FileOp::Renamed(..)) => if undo { S::UndoRename } else { S::RedoRename },
            None => return "",
        };
        t(self.lang, s)
    }

    fn run_file_step(&mut self, s: FileStep, undo: bool) {
        // Icons coming back (an undone move or delete) go back where they were.
        if undo && matches!(s.op, FileOp::Moved(_) | FileOp::Recycled(_)) && !s.places.is_empty() {
            self.returning = Some((Instant::now(), s.places.clone()));
        }
        let _ = self.worker.send(Req::UndoFiles { op: s.op, undo });
    }

    /// A new file operation went through: into the undo history with where its icons were (on
    /// whichever page).
    pub fn on_file_done(&mut self, op: FileOp) {
        let froms: Vec<String> = match &op {
            FileOp::Moved(v) | FileOp::Recycled(v) => v.iter().map(|m| m.from.to_lowercase()).collect(),
            FileOp::Copied(_) | FileOp::Renamed(..) => vec![],
        };
        let mut places = vec![];
        for it in self.items.values().filter(|i| froms.contains(&i.parsing_name.to_lowercase())) {
            if let Some((page, p, ix)) = deskboxes_core::pages::find_item(&self.layout, &it.key) {
                places.push((page, it.key.clone(), p, ix));
            }
        }
        log::info!("undo step: {op:?}, {} icon place(s)", places.len());
        self.hist.record_files(FileStep { op, places });
    }

    /// An undo or redo of a file operation finished: the history keeps what really went through
    /// (new names, bin files), or offers the same step again if nothing did.
    pub fn on_file_step_done(&mut self, op: Option<FileOp>, undo: bool) {
        if op.is_none() {
            log::warn!("{} of a file operation: nothing went through", if undo { "undo" } else { "redo" });
            if undo {
                self.returning = None;
            }
        }
        self.hist.files_done(undo, op);
    }

    /// Icons an undo brought back (from a folder, the Recycle Bin): put at their old place.
    fn place_returning(&mut self, old: &BTreeSet<ItemKey>, present: &[ItemKey]) -> Vec<ItemKey> {
        let Some((when, mut places)) = self.returning.take() else { return vec![] };
        if when.elapsed().as_secs() >= 10 {
            return vec![];
        }
        let (mut back, rest): (Vec<_>, Vec<_>) = places.drain(..).partition(|(_, k, _, _)| present.contains(k) && !old.contains(k));
        if !rest.is_empty() {
            self.returning = Some((when, rest));
        }
        back.sort_by_key(|(_, _, _, ix)| *ix);
        if !back.is_empty() {
            log::info!("undo: {} icon(s) back in place", back.len());
        }
        let g = self.grid;
        for (page, k, p, ix) in &back {
            // On the page it was on (a box's: the page that box is on now), whichever is shown.
            let page = match p {
                Place::Box { box_id, .. } => deskboxes_core::pages::page_of_box(&self.layout, *box_id).unwrap_or(*page),
                Place::Grid(_) => *page,
            };
            let page = if deskboxes_core::pages::index_of(&self.layout, page).is_some() { page } else { self.layout.page };
            deskboxes_core::pages::with_page(&mut self.layout, page, |l| match p {
                Place::Grid(c) => layout::move_to_grid_at(l, &[(k.clone(), *c)], &g),
                Place::Box { box_id, tab } => {
                    if let Some(ti) = l.boxes.iter().find(|b| b.id == *box_id).and_then(|b| b.tabs.iter().position(|t| t.id == *tab)) {
                        layout::move_to_tab(l, std::slice::from_ref(k), *box_id, ti, Some(*ix));
                    }
                }
            });
        }
        back.into_iter().map(|(_, k, _, _)| k).collect()
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
                since: Instant::now(),
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
        let RenameTarget::Item(k) = target else { return };
        let Some(it) = self.items.get(&k) else { return };
        let Some((_, cell, bx)) = self.visible_cells().into_iter().find(|(x, _, _)| *x == k) else { return };
        let m = self.metrics_of(bx);
        let (_, label) = self.item_parts(&k, cell, true, &m);
        let text = it.display_name.clone();
        // Explorer selects the name without the extension.
        let sel_end = match text.rfind('.') {
            Some(dot) if dot > 0 && !it.is_folder() => text[..dot].encode_utf16().count(),
            _ => text.encode_utf16().count(),
        };
        // A Fluent text box (rounded, an accent line under it, Windows' padding: RENAME_PAD), a
        // little wider than the icon's cell, its text where the name was (so nothing jumps), kept
        // on the screen (a row view: over the name, to the row's end).
        let (pl, pt) = (RENAME_PAD.0, RENAME_PAD.1);
        let fw = if m.row() { (cell.right() - label.x + pl + 2.0).max(96.0) } else { cell.w + 2.0 * pl + 6.0 };
        let fx = (if m.row() { label.x - pl - 1.0 } else { cell.x - pl - 3.0 }).clamp(0.0, (self.size_dip.0 - fw).max(0.0));
        let (x, y) = self.to_screen(Pt { x: fx, y: label.y - pt - 1.0 });
        let w = (fw * self.scale).round() as i32;
        let dark = !self.light;
        // As long as Windows lets a name be: 255 characters, and the whole path within MAX_PATH.
        let limit = {
            let p = std::path::Path::new(&it.parsing_name);
            let parent = if it.parsing_name.starts_with("::") { None } else { p.parent().map(|d| d.to_string_lossy().encode_utf16().count()) };
            let room = parent.map(|n| 259usize.saturating_sub(n + 1)).unwrap_or(255).min(255);
            room.max(text.encode_utf16().count()).max(1)
        };
        unsafe {
            let wt: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let s = self.scale;
            let frame = match CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, w!("DeskboxesRenameFrame"), w!(""), WS_POPUP | WS_CLIPCHILDREN, x, y, w, 24, Some(self.hwnd), None, None, None) {
                Ok(f) => f,
                Err(_) => return,
            };
            // Multi-line without horizontal scrolling: long names wrap, centred, as on Explorer's desktop.
            let style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE((if m.row() { ES_LEFT } else { ES_CENTER } | ES_MULTILINE | ES_AUTOVSCROLL) as u32);
            let (ix, iy) = ((pl * s).round() as i32, (pt * s).round() as i32);
            let Ok(edit) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("EDIT"), PCWSTR(wt.as_ptr()), style, ix, iy, (w - 2 * ix).max(8), 20, Some(frame), None, None, None) else {
                let _ = DestroyWindow(frame);
                return;
            };
            SendMessageW(edit, 0x00C5 /* EM_LIMITTEXT */, Some(WPARAM(limit)), Some(LPARAM(0)));
            let font = CreateFontW(
                -((self.font.1 * self.scale).round() as i32),
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
            SendMessageW(edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(0)));
            // One line's height in this font.
            let line_px = {
                let dc = GetDC(Some(edit));
                let old = SelectObject(dc, font.into());
                let mut tm = TEXTMETRICW::default();
                let _ = GetTextMetricsW(dc, &mut tm);
                SelectObject(dc, old);
                ReleaseDC(Some(edit), dc);
                tm.tmHeight.max(1)
            };
            SendMessageW(edit, EM_SETMARGINS, Some(WPARAM((EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize)), Some(LPARAM(0)));
            SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(sel_end as isize)));
            let look = RenameLook::new(dark, s);
            let brush = CreateSolidBrush(COLORREF(look.bg));
            SetWindowLongPtrW(edit, GWLP_USERDATA, line_px as isize);
            RENAME_OLD_PROC.with(|o| o.set(SetWindowLongPtrW(edit, GWLP_WNDPROC, rename_proc as *const () as isize)));
            RENAME_LOOK.with(|l| *l.borrow_mut() = Some((look, brush)));
            RENAME_TIP.with(|t| *t.borrow_mut() = t_(self.lang, S::BadNameChars).encode_utf16().chain(std::iter::once(0)).collect());
            // Rounded as Windows 11's own text boxes, with the outline of a control.
            let corner = windows::Win32::Graphics::Dwm::DWMWCP_ROUNDSMALL;
            let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(frame, windows::Win32::Graphics::Dwm::DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as _, 4);
            let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(frame, windows::Win32::Graphics::Dwm::DWMWA_BORDER_COLOR, &look.stroke as *const _ as _, 4);
            self.rename = Some(ItemRename { frame, edit, key: k, since: Instant::now(), font, brush });
            fit_rename(edit, line_px);
            let _ = ShowWindow(frame, SW_SHOWNOACTIVATE);
            let _ = SetForegroundWindow(frame);
            let _ = SetFocus(Some(edit));
        }
        // Its label under the field is not drawn meanwhile.
        self.invalidate();
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
        if let Some(r) = self.rename.take() {
            let text = unsafe {
                let n = GetWindowTextLengthW(r.edit);
                let mut buf = vec![0u16; n as usize + 1];
                let got = GetWindowTextW(r.edit, &mut buf);
                // (The tip about characters a name can't have goes with it.)
                SendMessageW(r.edit, EM_HIDEBALLOONTIP, None, None);
                let _ = DestroyWindow(r.frame);
                RENAME_LOOK.with(|l| *l.borrow_mut() = None);
                let _ = DeleteObject(r.font.into());
                let _ = DeleteObject(r.brush.into());
                String::from_utf16_lossy(&buf[..got as usize])
            };
            // A pasted line break is no part of a name.
            let text: String = text.chars().filter(|c| *c != '\r' && *c != '\n').collect::<String>().trim().to_string();
            let k = r.key;
            if let Some(it) = self.items.get(&k).filter(|it| commit && !text.is_empty() && it.display_name != text && it.kind != ItemKind::Virtual) {
                let _ = self.worker.send(Req::Rename { path: it.parsing_name.clone(), new_name: text });
            } else if self.sort_after_rename.as_ref() == Some(&k) {
                // Not renamed (Esc, or the name kept): the rules can sort it now.
                self.sort_after_rename = None;
                self.sort_created(&k);
            }
            self.invalidate();
        }
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
    }

    /// An icon made with "New", named now: where the sorting rules put new icons (gliding there).
    fn sort_created(&mut self, k: &ItemKey) {
        let one: BTreeSet<ItemKey> = std::iter::once(k.clone()).collect();
        let before = self.layout.clone();
        if self.apply_rules(Some(&one), true) {
            layout::evict_from_boxes(&mut self.layout, &self.grid);
            self.hist.record(&before);
            self.layout_changed();
            log::info!("rules: sorted the new icon after its rename");
        }
    }

    // ---------------------------------------------------------------- input

    fn on_lbutton_down(&mut self, p: Pt, dbl: bool) {
        self.hide_tip();
        if self.above_click_outside(p) {
            return;
        }
        if self.page_lbutton_down(p, dbl) {
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
        if let Hit::Item { in_box, .. } = &hit {
            self.item_box = *in_box;
        }
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
                // A rolled-up box opens and stays open (roll-up off for it), even when hovering
                // has already slid it open; any other box rolls up.
                let id = *id;
                let roll = self.box_by_id(id).map(|b| !b.rolled_up).unwrap_or(false);
                self.revealed.remove(&id);
                self.pending_reveal = None;
                self.no_reveal = if roll { Some(id) } else { None };
                self.change_layout(|l, _| {
                    if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) {
                        b.rolled_up = roll;
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
        if self.page_gesture_move(p) {
            return After::None;
        }
        if matches!(self.drag, Drag::MoveBox { .. } | Drag::Items { .. }) {
            self.page_edge_over(Some(p));
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
                            self.ole_fallback = false;
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
                let hit = self.band_hits(Rect::from_points(start, p), scope);
                self.sel.rubber_band(&base, &hit, ctrl);
                Drag::Rubber { start, cur: p, base, ctrl, scope }
            }
            Drag::Items { keys, start, .. } => {
                // Windows' own drag and drop from the start, as on Explorer's desktop: its drag
                // image, its "Move to …" / "Open with …" captions over folders, the Recycle Bin
                // and programs, the same over other apps (the drop target below handles our
                // surface). Our own drag only if Windows' could not start, until the surface is left.
                let (sx, sy) = self.to_screen(p);
                let over = unsafe { WindowFromPoint(POINT { x: sx, y: sy }) };
                let ours = over == self.hwnd || unsafe { GetAncestor(over, GA_ROOT) } == unsafe { GetAncestor(self.hwnd, GA_ROOT) } && !self.desktop_mode;
                let progman = hide::progman();
                let desktop_bg = self.desktop_mode && (over == progman || unsafe { GetAncestor(over, GA_ROOT) } == progman);
                if !self.ole_fallback || (!ours && !desktop_bg) {
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
                self.drag_hover(p);
                self.make_room(Some(p), &keys);
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
                // This box's own cells (a row view: rows, its width free).
                let m = self.metrics_of(Some(id));
                let (snap_w, cell_h, head) = match self.box_by_id(id) {
                    Some(bx) => (!m.row(), m.cell_h, Self::content_rect(bx).y - bx.rect.y),
                    None => (true, m.cell_h, header_h()),
                };
                let cell_w = if m.row() { self.grid.cell_w } else { m.cell_w };
                let (minw, minh) = (cell_w + 2.0 * PAD, header_h() + 40.0);
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
                    let cols = ((n.w - 2.0 * PAD) / cell_w).round().max(1.0);
                    let rows = ((n.h - head - 8.0) / cell_h).round().max(1.0);
                    let w = if snap_w { cols * cell_w + 2.0 * PAD } else { n.w };
                    let h = head + rows * cell_h + 8.0;
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
            // The right button's band selects too (Explorer's); its menu is then for them.
            Drag::RightRubber { start, scope, .. } => {
                let hit = self.band_hits(Rect::from_points(start, p), scope);
                self.sel.rubber_band(&BTreeSet::new(), &hit, false);
                Drag::RightRubber { start, cur: p, scope }
            }
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
                self.page_pill_hover(p);
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
        // A folder box's back arrow and the title buttons are buttons, not the title to move.
        if self.folder_back_at(p).is_some() || self.title_btn_at(p).is_some() {
            unsafe {
                SetCursor(LoadCursorW(None, IDC_ARROW).ok());
            }
            return;
        }
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
        if self.page_button_up(false) {
            return;
        }
        unsafe {
            let _ = ReleaseCapture();
        }
        if let Some(be) = self.box_edit.as_mut().filter(|b| b.selecting) {
            be.selecting = false;
            return;
        }
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        self.preview_loose = None;
        self.room = None;
        self.box_want = None;
        self.box_room = None;
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
        // (A drag that went to other pages: done.)
        self.page_dropped();
        self.page_drag_ended();
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
        self.room = None;
        self.box_want = None;
        self.box_room = None;
        // Taken to another page: back to its own, as it was.
        if self.own_drag.is_none() {
            self.page_drag_cancelled();
            self.page_drag_ended();
        }
    }

    pub fn drop_items(&mut self, keys: Vec<ItemKey>, start: Pt, cur: Pt) {
        // Where the user drops icons is exact: no glide from where they were before the drag
        // (see `settle_dropped` for the last bit from the drag image into place).
        self.no_glide.extend(keys.iter().cloned());
        let let_go: Vec<(ItemKey, Pt)> = {
            let cells = self.drag_cells();
            keys.iter()
                .filter_map(|k| cells.iter().find(|(x, _, _)| x == k).map(|(_, r, _)| (k.clone(), Pt { x: r.x + cur.x - start.x, y: r.y + cur.y - start.y })))
                .collect()
        };
        if let Some(key) = self.drop_onto_item(cur, &keys) {
            {
                // Onto a folder or program: Windows' own drop (moves or opens with).
                if let Some(t) = self.items.get(&key).map(|i| i.parsing_name.clone()) {
                    let paths: Vec<String> = keys.iter().filter_map(|k| self.items.get(k)).filter(|i| i.kind != ItemKind::Virtual).map(|i| i.parsing_name.clone()).collect();
                    if is_recycle_bin(&t) {
                        // Onto the Recycle Bin: deleted as Explorer does (Shift: permanently,
                        // with Windows' confirmation).
                        if !paths.is_empty() {
                            let _ = self.worker.send(Req::Delete { paths, recycle: !key_down(VK_SHIFT) });
                        }
                        return;
                    }
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
        // Onto a folder box: the files move into its folder (its own ones: they go where they
        // were dropped, in the order you make). Out of one, anywhere else: nothing (its files
        // are not desktop icons).
        if let Some(id) = self.drop_box(cur).filter(|id| self.is_folder_box(*id)) {
            if !self.reorder_folder_box(&keys, id, cur) {
                self.drop_into_folder_box(&keys, id, key_down(VK_CONTROL));
            }
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
            Some(id) if self.box_room_order(cur, &keys).is_some() => {
                // Where the others made room: before the first icon after them in that order, as
                // an index into the tab's full list (icons not on disk keep their places).
                let (_, order) = self.box_room_order(cur, &keys).unwrap();
                let ks: Vec<ItemKey> = order.iter().filter(|k| keys.contains(k)).cloned().collect();
                let at = order.iter().position(|k| keys.contains(k)).unwrap_or(order.len());
                let next = order[at..].iter().find(|k| !keys.contains(k)).cloned();
                let index = self.box_by_id(id).and_then(|b| b.active()).map(|t| next.and_then(|n| t.items.iter().position(|k| *k == n)).unwrap_or(t.items.len()));
                self.change_layout(|l, _| {
                    layout::move_to_box(l, &ks, id, index);
                });
            }
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
            None if self.grid_insert_at(cur, &keys).is_some() => {
                // Onto another icon or the gap between two: in between them, in the order shown
                // (where they made room while dragged).
                let (at, dir) = self.grid_insert_at(cur, &keys).unwrap();
                let order = self.order();
                let mut ks = keys.clone();
                ks.sort_by_key(|k| order.iter().position(|o| o == k).unwrap_or(usize::MAX));
                log::info!("insert {} icon(s) at {at:?}, the others moving {dir:?}", ks.len());
                self.change_layout(|l, g| layout::insert_along(l, &ks, at, dir, g));
            }
            None => {
                // Loose grid: every item moves by the drag offset, so the group keeps its shape
                // (Explorer). Items not on screen (scrolled out in a box) go next to the cursor.
                let (dx, dy) = (cur.x - start.x, cur.y - start.y);
                let cells = self.drag_cells();
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
        self.settle_dropped(&let_go);
    }

    /// Icons just dropped glide the last bit from where they were let go (the drag image showed
    /// them there) into their places, instead of jumping; on the desktop and in boxes alike.
    fn settle_dropped(&mut self, let_go: &[(ItemKey, Pt)]) {
        if let_go.is_empty() || !crate::pages::windows_animates() {
            return;
        }
        let now = Instant::now();
        let cells = self.visible_cells();
        let mut any = false;
        for (k, p) in let_go {
            let Some((_, r, bx)) = cells.iter().find(|(x, _, _)| x == k) else { continue };
            // Far away (another page, scrolled out of sight) or already there: no glide.
            let (dx, dy) = (r.x - p.x, r.y - p.y);
            if dx.abs() < 0.5 && dy.abs() < 0.5 || dx.hypot(dy) > 600.0 {
                continue;
            }
            match bx {
                None => {
                    self.glide_at.insert(k.clone(), Pt { x: r.x, y: r.y });
                    self.glide.insert(k.clone(), (*p, now));
                }
                Some(id) => {
                    let Some(b) = self.box_by_id(*id) else { continue };
                    let c = Self::content_rect(b);
                    let s = self.shown_scroll(*id);
                    self.glide_boxes.insert(*id);
                    self.box_glide_at.insert(k.clone(), Pt { x: r.x - c.x, y: r.y - c.y + s });
                    self.box_glide.insert(k.clone(), (Pt { x: p.x - c.x, y: p.y - c.y + s }, now));
                }
            }
            any = true;
        }
        if any {
            self.invalidate();
        }
    }

    fn on_rbutton_down(&mut self, p: Pt) {
        self.hide_tip();
        if self.above_click_outside(p) {
            return;
        }
        if self.page_rbutton_down(p) {
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
        } else if let Hit::Empty | Hit::BoxBody { .. } = hit {
            // A band from the desktop selects loose icons; from inside a box, that box's (as the
            // left button's does).
            let scope = if let Hit::BoxBody { id } = hit { Some(id) } else { None };
            self.sel.clear();
            unsafe {
                SetCapture(self.hwnd);
            }
            self.drag = Drag::RightRubber { start: p, cur: p, scope };
        }
        self.invalidate();
    }

    fn on_rbutton_up(&mut self, p: Pt) {
        if self.page_button_up(true) {
            return;
        }
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        unsafe {
            let _ = ReleaseCapture();
        }
        let mut hit = self.hit_test(p);
        if let Hit::Item { in_box, .. } = &hit {
            self.item_box = *in_box;
        }
        let new_rect = match drag {
            Drag::RightRubber { start, cur, scope } => {
                // Icons the band selected: their menu (Explorer's). None: the menu of where it
                // started: the box's, or the desktop's with "Create box here" the band's size.
                if let Some(k) = self.sel.ordered(&self.order()).first().cloned() {
                    self.sel.focus = Some(k.clone());
                    hit = Hit::Item { key: k, in_box: scope };
                    None
                } else if let Some(id) = scope {
                    hit = Hit::BoxBody { id };
                    None
                } else {
                    let r = Rect::from_points(start, cur);
                    (r.w > self.grid.cell_w && r.h > header_h()).then_some(r)
                }
            }
            _ => None,
        };
        self.invalidate();
        self.show_menu(hit, p, false, new_rect);
    }

    fn on_wheel(&mut self, p: Pt, delta: i16, ctrl: bool) {
        self.hide_tip();
        // Alt+wheel: the pages.
        if self.page_wheel(delta) {
            return;
        }
        // Ctrl + wheel over a box: that box's view and icon size, a step per notch (as in a File
        // Explorer window). Elsewhere: the desktop's icon size (small, medium, large).
        if ctrl && self.settings.extras.ctrl_wheel_size && delta != 0 {
            if let Some(id) = self.drop_box(p) {
                self.wheel_box_view(id, delta);
                return;
            }
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
            // A row a notch (three rows of a list), as Explorer scrolls.
            let m = self.metrics(Some(b));
            let step = if m.row() { m.cell_h * 3.0 } else { m.cell_h };
            self.scroll_to(id, (s - delta as f32 / 120.0 * step).clamp(0.0, max));
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
            VK_DELETE => self.delete_selection(!shift),
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
                    if self.surf.is_some() && self.frame_ok {
                        if !self.args.no_hide {
                            hide::set_hidden(true);
                        }
                    } else {
                        // Nothing on screen to show the icons: Explorer's own stay, and the
                        // watchdog starts a fresh core (a nonzero exit is a crash to it).
                        log::error!("no frame after the Explorer restart: restoring icons, exiting for a restart");
                        hide::restore();
                        PostQuitMessage(4);
                    }
                }
                Err(e) => {
                    log::error!("cannot recreate the surface after an Explorer restart: {e}; restoring icons");
                    hide::restore();
                }
            }
        }
    }

    /// Another window came to the front.
    /// - The desktop itself: after "Show desktop", Windows can put the desktop window above the
    ///   taskbar when it gets the front back from a window that closed (our menu), and the
    ///   taskbar stays hidden until Start is opened (seen 2026-11-20). Put the taskbar back on top.
    /// - Another app: the desktop's selection goes, as the user asked.
    pub fn on_foreground(&mut self, fg: HWND) {
        if fg.0.is_null() {
            return;
        }
        let class = unsafe {
            let mut b = [0u16; 64];
            let n = GetClassNameW(fg, &mut b) as usize;
            String::from_utf16_lossy(&b[..n])
        };
        let desktop = matches!(class.as_str(), "Progman" | "WorkerW");
        if desktop {
            if self.desktop_mode {
                keep_taskbar_on_top();
                unsafe {
                    SetTimer(Some(self.msg_hwnd), T_TASKBAR, 300, None);
                }
            }
            return;
        }
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(fg, Some(&mut pid));
        }
        let ours = pid == std::process::id() || self.host_child.as_ref().map(|c| c.id() == pid).unwrap_or(false);
        // The taskbar, Start, search, the tray, task switching: not "another app".
        let shell = matches!(
            class.as_str(),
            "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "NotifyIconOverflowWindow" | "TopLevelWindowForOverflowXamlIsland" | "XamlExplorerHostIslandWindow" | "Windows.UI.Core.CoreWindow" | "ForegroundStaging" | "MultitaskingViewFrame" | "TaskSwitcherWnd" | "XamlExplorerHostIslandWindow_WASDK"
        );
        if ours || shell || !matches!(self.drag, Drag::None) || self.sel.set.is_empty() {
            return;
        }
        self.sel.set.clear();
        self.invalidate();
    }

    /// At session end: if the autostart would not bring Deskboxes back, show the native icons
    /// now so the next logon is never an empty desktop (ARCHITECTURE §5).
    fn on_query_end_session(&mut self) {
        if !self.desktop_mode || self.args.no_hide {
            return;
        }
        let intact = autostart::intact();
        log::info!("session end: autostart intact={intact} ({:?})", autostart::target());
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
        self.flush_settings();
        self.save();
        // The snapshot is written on a thread of its own: finished before the process goes.
        if let Some(j) = self.snapshot_job.take() {
            let _ = j.join();
        }
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
    // The folder picker for a folder box: modal, so outside the App's borrow (our windows keep
    // drawing and answering meanwhile).
    if m == crate::events::WM_APP_PICKFOLDER {
        let req = with(|app| app.pick_req.take().map(|r| (r, app.hwnd, t(app.lang, S::FolderBoxChoose).trim_end_matches('…').to_string(), app.pick_start(&r)))).flatten();
        if let Some((r, owner, title, start)) = req {
            if let Some(p) = crate::folderbox::pick_folder(owner, &title, &start) {
                with(|app| app.apply_pick(r, &p));
            }
        }
        return Some(Some(LRESULT(0)));
    }
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
                app.on_lbutton_down(app.client_dip(x, y), m == WM_LBUTTONDBLCLK);
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE if is_surface => {
                let (x, y) = lparam_pt(l);
                after = app.on_mouse_move(app.client_dip(x, y));
                Some(LRESULT(0))
            }
            WM_LBUTTONUP if is_surface => {
                let (x, y) = lparam_pt(l);
                app.on_lbutton_up(app.client_dip(x, y));
                Some(LRESULT(0))
            }
            WM_RBUTTONDOWN if is_surface => {
                let (x, y) = lparam_pt(l);
                app.on_rbutton_down(app.client_dip(x, y));
                Some(LRESULT(0))
            }
            WM_RBUTTONUP if is_surface => {
                let (x, y) = lparam_pt(l);
                app.on_rbutton_up(app.client_dip(x, y));
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL if is_surface => {
                let (x, y) = lparam_pt(l);
                let p = app.screen_to_dip(x, y);
                app.on_wheel(p, ((w.0 >> 16) & 0xffff) as i16, w.0 & 0x0008 != 0);
                Some(LRESULT(0))
            }
            // A touchpad's two-finger swipe (or a tilt wheel): the pages.
            WM_MOUSEHWHEEL if is_surface => {
                app.page_hwheel(((w.0 >> 16) & 0xffff) as i16);
                Some(LRESULT(0))
            }
            WM_MBUTTONDOWN | WM_MBUTTONDBLCLK if is_surface => {
                let (x, y) = lparam_pt(l);
                app.page_mbutton(true, app.client_dip(x, y));
                Some(LRESULT(0))
            }
            WM_MBUTTONUP if is_surface => {
                let (x, y) = lparam_pt(l);
                app.page_mbutton(false, app.client_dip(x, y));
                Some(LRESULT(0))
            }
            WM_CAPTURECHANGED if is_surface && app.pages.gesture.is_some() => {
                if l.0 != app.hwnd.0 as isize {
                    app.page_gesture_lost();
                }
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
            WM_APP_RENAME_END => {
                // The name field lost the focus just after it opened (after "New", Windows' own
                // handler may take the focus for a moment): it keeps the focus instead of closing.
                let fresh = app.rename.as_ref().map(|r| r.since.elapsed().as_millis() < 500).unwrap_or(false);
                if w.0 == 2 && fresh {
                    let _ = PostMessageW(Some(h), WM_APP_REFOCUS, WPARAM(0), LPARAM(0));
                } else if app.rename.is_some() {
                    app.end_rename(w.0 != 0);
                }
                Some(LRESULT(0))
            }
            WM_APP_REFOCUS => {
                if let Some(r) = app.rename.as_ref() {
                    let _ = SetForegroundWindow(r.frame);
                    let _ = SetFocus(Some(r.edit));
                }
                Some(LRESULT(0))
            }
            WM_CHAR if is_surface => {
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
                if m == WM_SYSKEYDOWN && app.page_key(VIRTUAL_KEY(w.0 as u16)) {
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
                // Into another window: boxes the pointer was on blend into the wallpaper again.
                app.blend_pointer(Pt { x: -1.0e6, y: -1.0e6 });
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
                            keep,
                        } => {
                            app.stats.scan_resolved = resolved;
                            app.apply_scan(items, icons, native, ms, first);
                            app.evict_icons(&keep);
                        }
                        Ev::Rescan => {
                            SetTimer(Some(app.msg_hwnd), T_FS, 75, None);
                        }
                        Ev::HostReady(c) => {
                            app.host = Some(c);
                            // The menu the stuck host never showed, if it was asked for just now.
                            if let Some((t0, ask)) = app.menu_replay.take() {
                                if t0.elapsed().as_secs() < 20 {
                                    log::info!("showing the menu the old shell host did not");
                                    app.replaying_menu = true;
                                    app.show_menu(ask.hit, ask.at, ask.keyboard, ask.new_rect);
                                    app.replaying_menu = false;
                                }
                            }
                        }
                        Ev::HostMsg(v) => app.on_host_msg(v),
                        Ev::InfoTip { key, text } => app.on_info_tip(key, text),
                        Ev::FileDone { op } => app.on_file_done(op),
                        Ev::FileStepDone { op, undo } => app.on_file_step_done(op, undo),
                        Ev::IconsAt { px, map, icons } => app.apply_sized_icons(px, map, icons),
                        Ev::Update => {
                            // (The update status is compared on its own too: a setting applied from
                            // the window notes the whole state as sent without sending it.)
                            let upd = serde_json::to_string(&crate::update::status()).unwrap_or_default();
                            if app.host.is_some() && (app.settings_sig() != app.settings_sent || upd != app.update_sent) {
                                app.send_settings(false, None);
                            }
                        }
                        Ev::HostGone => {
                            if !app.quitting {
                                log::warn!("shell host disconnected; restarting it");
                                app.host = None;
                                app.menu = None;
                                app.menu_wait = None;
                                let _ = KillTimer(Some(app.msg_hwnd), T_MENU_WAIT);
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
            crate::events::WM_APP_DRAGWHEEL => {
                let xy = l.0 as u64;
                app.drag_wheel((xy & 0xffff_ffff) as u32 as i32, (xy >> 32) as u32 as i32, w.0 as u16 as i16);
                Some(LRESULT(0))
            }
            crate::events::WM_APP_FOREGROUND => {
                app.on_foreground(HWND(l.0 as _));
                Some(LRESULT(0))
            }
            crate::events::WM_APP_WALLPAPER => {
                SetTimer(Some(app.msg_hwnd), T_WALLPAPER, 400, None);
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
                    T_ROOM => app.room_due(),
                    T_DRAGNUDGE => app.drag_nudge(),
                    T_HDRCHECK => app.hdr_check(),
                    T_TIP => app.tip_due(),
                    T_BTNTIP => app.btn_tip_due(),
                    T_ABOVE => {
                        app.corner_tick();
                        app.above_check_foreground();
                        app.above_poll();
                    }
                    T_ABOVE_DRAG => app.above_drag_tick(),
                    crate::pages::T_PAGE_EDGE | crate::pages::T_PAGE_SWIPE | crate::pages::T_PAGE_PILL => app.page_timer(id),
                    T_MENU_WAIT => app.menu_wait_due(),
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
                    T_PREP => {
                        let _ = KillTimer(Some(h), T_PREP);
                        app.prep_more();
                    }
                    T_SAVE_SETTINGS => app.flush_settings(),
                    T_TASKBAR => keep_taskbar_on_top(),
                    T_WALLPAPER => {
                        let _ = KillTimer(Some(h), T_WALLPAPER);
                        app.wallpaper_check();
                    }
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
                if light != app.light {
                    app.light = light;
                    app.load_wallpaper();
                    app.fx_cache.borrow_mut().clear();
                    app.invalidate();
                } else if w.0 as u32 == SPI_SETDESKWALLPAPER.0 || w.0 == 0 {
                    SetTimer(Some(app.msg_hwnd), T_WALLPAPER, 250, None);
                }
                None
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED => {
                log::info!("display change: Phase 1 re-layout needs a restart (UNVERIFIED path)");
                None
            }
            WM_HOTKEY if w.0 as i32 == crate::above::HK_ABOVE => {
                // Only over an app: on the desktop the boxes are already in front.
                if app.is_above() || !app.desktop_in_front() {
                    app.toggle_above();
                } else {
                    log::info!("boxes above windows: shortcut ignored (the desktop is in front)");
                }
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
        with(|app| app.above_drag_begin());
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
                app.ole_drag_over(None, None);
                app.above_drag_end();
                app.page_drag_ended();
                let _ = app.worker.send(Req::Scan);
            });
        }
        Err(e) => {
            // (Items Windows can't drag:) our own drag on the surface instead.
            log::warn!("drag data object: {e}");
            with(|app| {
                if let Some((keys, start)) = app.own_drag.take() {
                    app.ole_fallback = true;
                    app.drag = Drag::Items { keys, start, cur: app.cursor_dip() };
                    let _ = SetCapture(h);
                }
            });
        }
    }
}

/// The Recycle Bin's parsing name (`::{645FF040-5081-101B-9F08-00AA002F954E}`).
pub fn is_recycle_bin(parsing: &str) -> bool {
    parsing.to_ascii_uppercase().contains("645FF040-5081-101B-9F08-00AA002F954E")
}

/// "Turn on Deskboxes" in Windows' classic desktop menu while Deskboxes is off (Windows 11's main
/// menu gets it from the installer's menu package).
fn add_turn_on_verb(settings: &deskboxes_core::Settings) {
    let lang = Lang::pick(&settings.language, unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() });
    let exe = std::env::current_exe().unwrap_or_default();
    turnon::add_verb(t(lang, S::TurnOn), &exe.with_file_name("deskboxes-watchdog.exe"), &exe);
    turnon::pin_modern_verb();
}

/// A verb of Windows' "New" submenu: a file type (".txt", ".docx"), a folder or a shortcut.
fn is_new_verb(v: &str) -> bool {
    v == "new" || (v.starts_with('.') && v.len() > 1) || v.eq_ignore_ascii_case("NewFolder") || v.eq_ignore_ascii_case("NewLink") || v.eq_ignore_ascii_case("NewShortcut")
}

/// The taskbar(s) back above the desktop window if the desktop got above them (see
/// `App::on_foreground`). No activation, no move: only the z-order, and only when it is wrong.
fn keep_taskbar_on_top() {
    unsafe {
        let Ok(progman) = FindWindowW(w!("Progman"), None) else { return };
        let mut trays = vec![];
        if let Ok(t) = FindWindowW(w!("Shell_TrayWnd"), None) {
            trays.push(t);
        }
        let mut prev: Option<HWND> = None;
        while let Ok(t) = FindWindowExW(None, prev, w!("Shell_SecondaryTrayWnd"), None) {
            trays.push(t);
            prev = Some(t);
        }
        for t in trays {
            // Is the desktop window above this taskbar?
            let mut h = GetWindow(t, GW_HWNDPREV).unwrap_or_default();
            let mut n = 0;
            let mut above = false;
            while !h.0.is_null() && n < 400 {
                if h == progman {
                    above = true;
                    break;
                }
                h = GetWindow(h, GW_HWNDPREV).unwrap_or_default();
                n += 1;
            }
            if above {
                let _ = SetWindowPos(t, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                log::info!("the desktop window was above the taskbar: taskbar put back on top");
            }
        }
    }
}

thread_local! {
    static RENAME_OLD_PROC: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
    /// The name field's colors and its background brush (one field at a time).
    static RENAME_LOOK: RefCell<Option<(RenameLook, HBRUSH)>> = const { RefCell::new(None) };
    /// What the tip says when a character a file name can't have is typed or pasted.
    static RENAME_TIP: RefCell<Vec<u16>> = const { RefCell::new(Vec::new()) };
}

/// The name field's padding around its text (DIPs: sides, top, bottom with the accent line),
/// as Windows 11's text boxes have, scaled to the icon labels' smaller font.
const RENAME_PAD: (f32, f32, f32) = (7.0, 4.0, 6.0);

/// The name field's colors, as Windows 11's text box in focus (COLORREF: 0x00BBGGRR).
#[derive(Clone, Copy)]
struct RenameLook {
    bg: u32,
    fg: u32,
    stroke: u32,
    accent: u32,
    /// The accent line's height, px.
    line: i32,
}

impl RenameLook {
    fn new(dark: bool, scale: f32) -> Self {
        let rgb = |r: u8, g: u8, b: u8| (b as u32) << 16 | (g as u32) << 8 | r as u32;
        // The accent as Windows' controls use it: from the system's accent palette (light 3, 2,
        // 1, the color, dark 1, 2, 3), its second lighter shade in the dark theme and first darker
        // one in the light theme; made from the accent where there is no palette.
        let pal = reg::get_binary(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent", "AccentPalette").filter(|p| p.len() >= 28);
        let accent = match pal {
            Some(p) => {
                let i = if dark { 4 } else { 16 };
                rgb(p[i], p[i + 1], p[i + 2])
            }
            None => {
                let (r, g, b) = accent();
                let mix = |c: u8, to: u8, k: f32| (c as f32 + (to as f32 - c as f32) * k).round() as u8;
                if dark { rgb(mix(r, 255, 0.45), mix(g, 255, 0.45), mix(b, 255, 0.45)) } else { rgb(mix(r, 0, 0.12), mix(g, 0, 0.12), mix(b, 0, 0.12)) }
            }
        };
        RenameLook {
            bg: if dark { rgb(0x1F, 0x1F, 0x1F) } else { rgb(0xFF, 0xFF, 0xFF) },
            fg: if dark { rgb(0xFF, 0xFF, 0xFF) } else { rgb(0x1A, 0x1A, 0x1A) },
            stroke: if dark { rgb(0x3A, 0x3A, 0x3A) } else { rgb(0xD9, 0xD9, 0xD9) },
            accent,
            // Two pixels at 100% over the outline's own bottom pixel (DWM draws it on top).
            line: (2.0 * scale).round().max(1.0) as i32 + 1,
        }
    }
}

/// The name field's frame: its background, the accent line along the bottom (the outline and
/// the rounded corners are DWM's), and the text box's colors.
extern "system" fn rename_frame_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let look = RENAME_LOOK.with(|v| *v.borrow());
        match m {
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(h, &mut ps);
                if let Some((lk, brush)) = look {
                    let mut r = RECT::default();
                    let _ = GetClientRect(h, &mut r);
                    FillRect(dc, &r, brush);
                    let line = CreateSolidBrush(COLORREF(lk.accent));
                    let lr = RECT { top: r.bottom - lk.line, ..r };
                    FillRect(dc, &lr, line);
                    let _ = DeleteObject(line.into());
                }
                let _ = EndPaint(h, &ps);
                return LRESULT(0);
            }
            WM_CTLCOLOREDIT => {
                if let Some((lk, brush)) = look {
                    let hdc = HDC(w.0 as _);
                    SetTextColor(hdc, COLORREF(lk.fg));
                    SetBkColor(hdc, COLORREF(lk.bg));
                    return LRESULT(brush.0 as isize);
                }
            }
            // A click on the padding: the caret stays in the text.
            WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
            WM_LBUTTONDOWN => {
                if let Some(e) = GetWindow(h, GW_CHILD).ok() {
                    let _ = SetFocus(Some(e));
                }
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(h, m, w, l)
    }
}

/// The name field (and its frame) as tall as its lines (one at least, twelve at most), as
/// Explorer's grows.
fn fit_rename(edit: HWND, line_px: i32) {
    unsafe {
        let lines = SendMessageW(edit, EM_GETLINECOUNT, None, None).0.clamp(1, 12) as i32;
        let Ok(frame) = GetParent(edit) else { return };
        let scale = GetDpiForWindow(frame).max(96) as f32 / 96.0;
        let (pt, pb) = ((RENAME_PAD.1 * scale).round() as i32, (RENAME_PAD.2 * scale).round() as i32);
        let mut er = RECT::default();
        let mut fr = RECT::default();
        let _ = GetWindowRect(edit, &mut er);
        let _ = GetWindowRect(frame, &mut fr);
        let h = lines * line_px;
        if er.bottom - er.top != h {
            let _ = SetWindowPos(edit, None, 0, 0, er.right - er.left, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        let fh = pt + h + pb;
        if fr.bottom - fr.top != fh {
            let _ = SetWindowPos(frame, None, 0, 0, fr.right - fr.left, fh, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            let _ = InvalidateRect(Some(frame), None, true);
        }
    }
}

/// Explorer's tip under the caret: the characters a file name can't have.
fn rename_tip(edit: HWND) {
    use windows::Win32::UI::Controls::{EDITBALLOONTIP, EM_SHOWBALLOONTIP, TTI_NONE};
    unsafe {
        let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(MB_OK);
        RENAME_TIP.with(|t| {
            let t = t.borrow();
            if t.is_empty() {
                return;
            }
            let empty = [0u16];
            let tip = EDITBALLOONTIP { cbStruct: std::mem::size_of::<EDITBALLOONTIP>() as u32, pszTitle: PCWSTR(empty.as_ptr()), pszText: PCWSTR(t.as_ptr()), ttiIcon: TTI_NONE };
            SendMessageW(edit, EM_SHOWBALLOONTIP, None, Some(LPARAM(&tip as *const _ as isize)));
        });
    }
}

/// What may go into a name from pasted text: its first line, without the characters a file
/// name can't have (and whether any were left out).
fn rename_paste_text(s: &str) -> (String, bool) {
    let line = s.split(['\r', '\n']).find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut bad = false;
    let out = line
        .chars()
        .filter(|c| {
            let ok = !"\\/:*?\"<>|".contains(*c) && !c.is_control();
            bad |= !ok;
            ok
        })
        .collect();
    (out, bad)
}

/// Windows' text box for an icon's name: Enter keeps the name, Esc goes back, no line breaks, no
/// characters a file name can't have, Ctrl+A and Ctrl+Backspace as in Explorer; it grows with
/// its lines and tells the core when it is done.
extern "system" fn rename_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        // The core's window owns the field's frame.
        let owner = GetParent(h).ok().and_then(|f| GetWindow(f, GW_OWNER).ok()).unwrap_or_default();
        let ctrl = key_down(VK_CONTROL);
        match m {
            WM_KEYDOWN if w.0 == VK_RETURN.0 as usize => {
                let _ = PostMessageW(Some(owner), WM_APP_RENAME_END, WPARAM(1), LPARAM(0));
                return LRESULT(0);
            }
            WM_KEYDOWN if w.0 == VK_ESCAPE.0 as usize => {
                let _ = PostMessageW(Some(owner), WM_APP_RENAME_END, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
            WM_KEYDOWN if ctrl && w.0 == b'A' as usize => {
                SendMessageW(h, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                return LRESULT(0);
            }
            // Ctrl+Backspace: delete the previous word (the plain text box types a box char).
            WM_KEYDOWN if ctrl && w.0 == VK_BACK.0 as usize => {
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
                fit_rename(h, rename_line_px(h));
                return LRESULT(0);
            }
            WM_CHAR if w.0 == 13 || w.0 == 10 || w.0 == 27 || w.0 == 0x7F || w.0 == 1 => return LRESULT(0),
            // Characters no file name can have: refused with Explorer's tip.
            WM_CHAR if char::from_u32(w.0 as u32).map(|c| "\\/:*?\"<>|".contains(c)).unwrap_or(false) => {
                rename_tip(h);
                return LRESULT(0);
            }
            // Pasted: one line, without those characters (the tip says why some went).
            WM_PASTE => {
                if let Some(s) = deskboxes_shell::view::clipboard_text(owner.0 as isize) {
                    let (text, bad) = rename_paste_text(&s);
                    let wt: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
                    SendMessageW(h, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(wt.as_ptr() as isize)));
                    if bad {
                        rename_tip(h);
                    }
                    fit_rename(h, rename_line_px(h));
                }
                return LRESULT(0);
            }
            WM_KILLFOCUS => {
                // (The click went to the field's own frame: still editing.)
                let to = HWND(w.0 as _);
                if GetParent(h).map(|f| f == to).unwrap_or(false) {
                    let _ = SetFocus(Some(h));
                    return LRESULT(0);
                }
                SendMessageW(h, EM_HIDEBALLOONTIP, None, None);
                let _ = PostMessageW(Some(owner), WM_APP_RENAME_END, WPARAM(2), LPARAM(0));
            }
            _ => {}
        }
        let old = RENAME_OLD_PROC.with(|o| o.get());
        let f: WNDPROC = std::mem::transmute(old);
        let r = CallWindowProcW(f, h, m, w, l);
        if matches!(m, WM_CHAR | WM_KEYDOWN | WM_PASTE | WM_CUT | WM_CLEAR | WM_UNDO | EM_UNDO) {
            fit_rename(h, rename_line_px(h));
        }
        r
    }
}

/// The name field's line height (kept on the window when it was made).
fn rename_line_px(h: HWND) -> i32 {
    let v = unsafe { GetWindowLongPtrW(h, GWLP_USERDATA) } as i32;
    if v > 0 { v } else { 16 }
}
