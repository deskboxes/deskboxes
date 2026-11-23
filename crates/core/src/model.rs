//! Data model (docs/ARCHITECTURE.md §7).

use crate::geom::{Cell, Rect};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Stable identity of a desktop item.
/// - `f:<volume serial hex>:<FileId hex>` for files and folders (survives renames and moves),
/// - `p:<lower-case parsing path>` when no FileId is available,
/// - `v:<::{CLSID}>` for virtual items (This PC, Recycle Bin, ...).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ItemKey(pub String);

impl ItemKey {
    pub fn file(volume: u64, file_id: u128) -> Self {
        ItemKey(format!("f:{volume:x}:{file_id:032x}"))
    }
    pub fn path(p: &str) -> Self {
        ItemKey(format!("p:{}", p.to_lowercase()))
    }
    pub fn virtual_item(parsing: &str) -> Self {
        ItemKey(format!("v:{}", parsing.to_lowercase()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemKind {
    File,
    Folder,
    Shortcut,
    UrlFile,
    Virtual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    User,
    Public,
    Virtual,
    Fixture,
    /// In a folder a folder box shows (not on the desktop).
    Folder,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub key: ItemKey,
    /// Full path, or `::{CLSID}` for virtual items. What the shell host and file ops receive.
    pub parsing_name: String,
    /// Explorer's name for it (SIGDN_NORMALDISPLAY: hides .lnk/.url and, by setting, extensions).
    pub display_name: String,
    pub kind: ItemKind,
    pub ext: String,
    pub size: u64,
    /// FILETIME (100 ns since 1601) of the last write.
    pub modified: u64,
    pub attrs: u32,
    pub source: Source,
    /// Index into the icon cache (shared by items with the same system icon).
    pub icon: u32,
}

impl Item {
    pub fn is_folder(&self) -> bool {
        self.kind == ItemKind::Folder
    }
}

/// Box background material. `Mica` follows the system light/dark mode (user requirement).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Material {
    Mica,
    Plain,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// Per-box look: color, background transparency and blur are user-adjustable per box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoxStyle {
    pub material: Material,
    /// Tint color; `None` = the theme's Mica tint.
    pub color: Option<Rgba>,
    /// Background opacity 0..1 (tint strength over the blurred wallpaper).
    pub opacity: f32,
    /// Blur radius in DIPs (Mica uses a large blur).
    pub blur: f32,
    /// Title text color; `None` = theme.
    pub title_color: Option<Rgba>,
}

impl Default for BoxStyle {
    fn default() -> Self {
        BoxStyle {
            material: Material::Mica,
            color: None,
            opacity: 0.6,
            blur: 60.0,
            title_color: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub id: u32,
    pub title: String,
    /// Items in display order.
    pub items: Vec<ItemKey>,
    /// Tab color (its pill / underline); `None` = neutral.
    #[serde(default)]
    pub color: Option<Rgba>,
    /// The box's background while this tab is open; `None` = the box's own color.
    #[serde(default)]
    pub box_color: Option<Rgba>,
    /// A folder box's tab: the folder it shows ("" = the box's `folder`, as before tabs).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder: String,
    /// The subfolder this tab is in now ("" = `folder` itself).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder_at: String,
}

impl Tab {
    pub fn new(id: u32, title: &str) -> Tab {
        Tab {
            id,
            title: title.to_string(),
            items: vec![],
            color: None,
            box_color: None,
            folder: String::new(),
            folder_at: String::new(),
        }
    }
}

/// How a box shows its icons (Explorer's View menu).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// Icons in a grid, the name under each (at the box's icon size).
    Icons,
    /// Small icons with the name to the right, in columns.
    List,
    /// One row per item: name, date modified, type, size.
    Details,
    /// A bigger icon with the name, type and size to the right.
    Tiles,
}

impl View {
    pub fn parse(s: &str) -> View {
        match s {
            "list" => View::List,
            "details" => View::Details,
            "tiles" => View::Tiles,
            _ => View::Icons,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            View::Icons => "",
            View::List => "list",
            View::Details => "details",
            View::Tiles => "tiles",
        }
    }
}

/// Explorer's icon sizes for a box (DIPs), smallest first: small, medium, large, extra large.
pub const BOX_ICON_SIZES: [u32; 4] = [32, 48, 96, 256];
/// The steps Ctrl + mouse wheel goes through in a box, as in a File Explorer window.
pub const BOX_WHEEL_SIZES: [u32; 9] = [32, 40, 48, 64, 80, 96, 128, 192, 256];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoxModel {
    pub id: u32,
    /// Position and size in DIPs relative to the monitor's top-left.
    pub rect: Rect,
    pub style: BoxStyle,
    /// One tab = a plain box (its title is the box title); two or more = a tab header.
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    #[serde(default)]
    pub rolled_up: bool,
    /// What the user cannot change by hand.
    #[serde(default)]
    pub locks: BoxLocks,
    /// Icons shown by name, with an A–Z index when they do not all fit.
    #[serde(default)]
    pub az: bool,
    /// Icon tint of this box: "" = as all boxes (Settings), "off", "box" (the box's color) or
    /// "#RRGGBB".
    #[serde(default)]
    pub tint: String,
    /// Icons blend into the wallpaper: "" = as all boxes (Settings), "on" or "off".
    #[serde(default)]
    pub blend: String,
    /// Stays on the desktop when everything is hidden with a double-click.
    #[serde(default)]
    pub stay_visible: bool,
    /// A folder box: shows this folder's contents (its files stay where they are); "" = a
    /// normal box.
    #[serde(default)]
    pub folder: String,
    /// The subfolder it is showing now ("" = `folder` itself). Before folder boxes had tabs;
    /// each tab keeps its own now (`Tab::folder_at`).
    #[serde(default)]
    pub folder_at: String,
    /// How it shows its icons: "" = icons, "list", "details" or "tiles" (`View`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub view: String,
    /// Icon size in DIPs for the icons view; 0 = the desktop's size.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub icon_size: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// A box's locks: each stops one kind of change by hand (menus still work).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoxLocks {
    /// The box cannot be moved.
    pub position: bool,
    /// The box cannot be resized.
    pub size: bool,
    /// Icons cannot be dragged into, out of or around the box.
    pub icons: bool,
    /// Tabs cannot be added, dragged, torn off or combined.
    pub tabs: bool,
}

impl BoxLocks {
    pub const ALL: BoxLocks = BoxLocks { position: true, size: true, icons: true, tabs: true };
    pub fn any(&self) -> bool {
        self.position || self.size || self.icons || self.tabs
    }
}

impl BoxModel {
    pub fn is_folder(&self) -> bool {
        !self.folder.is_empty()
    }
    pub fn view(&self) -> View {
        View::parse(&self.view)
    }
    /// The folder tab `ti` of a folder box shows (its own, or the box's).
    pub fn tab_root(&self, ti: usize) -> &str {
        match self.tabs.get(ti) {
            Some(t) if !t.folder.is_empty() => &t.folder,
            _ => &self.folder,
        }
    }
    /// The folder tab `ti` is in now (a subfolder of `tab_root`, or it).
    pub fn tab_now(&self, ti: usize) -> &str {
        match self.tabs.get(ti) {
            Some(t) if !t.folder_at.is_empty() => &t.folder_at,
            Some(t) if ti == 0 && t.folder.is_empty() && !self.folder_at.is_empty() => &self.folder_at,
            _ => self.tab_root(ti),
        }
    }
    /// Whether tab `ti` is in a subfolder (its back arrow shows).
    pub fn tab_in_sub(&self, ti: usize) -> bool {
        let (r, n) = (self.tab_root(ti), self.tab_now(ti));
        !n.is_empty() && !n.trim_end_matches('\\').eq_ignore_ascii_case(r.trim_end_matches('\\'))
    }
    /// The folder the open tab of a folder box shows now.
    pub fn folder_now(&self) -> &str {
        self.tab_now(self.active_tab)
    }
    /// The open tab's own folder (where Back stops).
    pub fn folder_root(&self) -> &str {
        self.tab_root(self.active_tab)
    }
    /// Tab `ti` shows `root`, in its subfolder `at` ("" = root).
    pub fn set_tab_folder(&mut self, ti: usize, root: &str, at: &str) {
        if ti == 0 {
            self.folder_at.clear();
        }
        if let Some(t) = self.tabs.get_mut(ti) {
            t.folder = root.to_string();
            t.folder_at = at.to_string();
        }
    }
    /// Every tab of a folder box names its folder (older files kept it on the box).
    pub fn own_folders(&mut self) {
        if !self.is_folder() {
            return;
        }
        for ti in 0..self.tabs.len() {
            let (r, n) = (self.tab_root(ti).to_string(), self.tab_now(ti).to_string());
            let at = if n.eq_ignore_ascii_case(&r) { String::new() } else { n };
            self.set_tab_folder(ti, &r, &at);
        }
    }
    pub fn title(&self) -> &str {
        self.tabs.get(self.active_tab).map(|t| t.title.as_str()).unwrap_or("")
    }
    pub fn active(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }
    pub fn contains(&self, k: &ItemKey) -> Option<(usize, usize)> {
        for (ti, t) in self.tabs.iter().enumerate() {
            if let Some(i) = t.items.iter().position(|x| x == k) {
                return Some((ti, i));
            }
        }
        None
    }
}

/// Where an item lives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Place {
    Grid(Cell),
    Box { box_id: u32, tab: u32 },
}

/// Layout of one monitor topology (docs/ARCHITECTURE.md §8: one file per topology signature).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MonitorLayout {
    pub boxes: Vec<BoxModel>,
    /// Loose items and their grid cells (primary monitor in Phase 1).
    pub loose: BTreeMap<ItemKey, Cell>,
    #[serde(default)]
    pub next_id: u32,
    /// Sorting rules, in order (the first match wins).
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Keep every icon that matches a rule in its tab, also after it is moved out by hand.
    #[serde(default)]
    pub keep_sorted: bool,
    /// Tab id that new icons matching no rule go to (`None`: where they appear).
    #[serde(default)]
    pub new_items_to: Option<u32>,
    /// Loose icons a box pushed aside, and the cell each came from: they go back there once no
    /// box covers it (until the user moves them).
    #[serde(default)]
    pub homes: BTreeMap<ItemKey, Cell>,
    /// Folder boxes: the order icons were dragged into (or sorted by), per folder (lowercase
    /// path). A folder without one is shown folders first, then by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub folder_orders: BTreeMap<String, Vec<ItemKey>>,
    /// Desktop pages, in order (empty: one unnamed page). The page shown is `page`: its boxes and
    /// icons are `boxes` / `loose` / `homes` above, the other pages' wait in `parked` (see
    /// `pages.rs`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<PageMeta>,
    #[serde(default)]
    pub page: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parked: BTreeMap<u32, PageContent>,
}

/// A desktop page: its id and the name it was given ("" = none: "Page N" is shown).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PageMeta {
    pub id: u32,
    #[serde(default)]
    pub name: String,
}

/// A page not shown now: its boxes and loose icons (as `MonitorLayout`'s own).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PageContent {
    #[serde(default)]
    pub boxes: Vec<BoxModel>,
    #[serde(default)]
    pub loose: BTreeMap<ItemKey, Cell>,
    #[serde(default)]
    pub homes: BTreeMap<ItemKey, Cell>,
}

/// A sorting rule: icons matching the condition go into tab `target` (a tab id).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub id: u32,
    pub enabled: bool,
    /// "type", "name", "ext", "age" or "size".
    pub kind: String,
    /// type: category id; name: "contains" | "starts" | "ends" | "is"; age: "newer" | "older"
    /// (days); size: "larger" | "smaller" (MB).
    pub op: String,
    /// name: text; ext: list like "pdf, docx"; age: days; size: MB.
    pub value: String,
    pub target: u32,
}

impl Default for Rule {
    fn default() -> Self {
        Rule {
            id: 0,
            enabled: true,
            kind: "type".into(),
            op: "documents".into(),
            value: String::new(),
            target: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub schema: u32,
    pub topology: String,
    pub monitor: MonitorLayout,
}

pub const LAYOUT_SCHEMA: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema: u32,
    /// "Disable Deskboxes": the core restores native icons and exits; the watchdog does not restart it.
    pub enabled: bool,
    /// "pl", "en", or "" = follow Windows.
    pub language: String,
    pub default_style: BoxStyle,
    pub rollup: RollupSettings,
    pub tabs: TabSettings,
    pub privacy: PrivacySettings,
    pub extras: ExtrasSettings,
    /// View > Show desktop icons (ours: Explorer's own flag is how its icons stay hidden).
    pub show_icons: bool,
    /// How box titles look (all boxes).
    pub look: LookSettings,
    /// "Lock everything": no box moves, resizes or changes its icons or tabs, and desktop icons
    /// stay where they are.
    pub lock_all: bool,
    /// A–Z boxes: the letter bar and the letter tags.
    pub az: AzSettings,
    /// The desktop search field.
    pub search: SearchSettings,
    /// Icon tint and blending into the wallpaper (all boxes; each box can differ).
    pub icons: IconSettings,
    /// Hide all with a double-click on the bare desktop.
    pub hide: HideSettings,
    /// Saved layouts.
    pub saved: SavedSettings,
    /// Boxes above windows.
    pub above: AboveSettings,
    /// Updates from GitHub.
    pub update: UpdateSettings,
    /// Desktop pages.
    pub pages: PagesSettings,
}

/// Settings › Desktop pages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PagesSettings {
    /// Pages at all (off: one desktop; turning it off with several pages puts everything on one).
    pub enabled: bool,
    /// Alt+Left / Alt+Right, and Alt+wheel, on the desktop.
    pub alt_keys: bool,
    /// A touchpad's two-finger swipe left or right (its horizontal wheel).
    pub swipe: bool,
    /// Dragging the desktop with the middle button.
    pub middle_drag: bool,
    /// Dragging the desktop with both buttons held.
    pub both_drag: bool,
    /// Pulling the desktop from its left or right screen edge.
    pub edge_pull: bool,
    /// Icons or a box dragged to the screen's left or right edge and held there go to the page
    /// beside (a new one past the last).
    pub edge_drag: bool,
    /// Pages slide (off: a quick fade).
    pub animate: bool,
    /// A light bounce as a page settles.
    pub bounce: bool,
    /// The page's name at the top of the screen for a moment after a switch.
    pub show_name: bool,
    /// Where new desktop files appear: "current" (the page shown) or "first".
    pub new_files: String,
}

impl Default for PagesSettings {
    fn default() -> Self {
        PagesSettings {
            enabled: true,
            alt_keys: true,
            swipe: true,
            middle_drag: true,
            both_drag: true,
            edge_pull: true,
            edge_drag: true,
            animate: true,
            bounce: true,
            show_name: true,
            new_files: "current".into(),
        }
    }
}

/// Settings › General › Updates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Look for a new version once a day, download it, install it on the next start.
    pub auto: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        UpdateSettings { auto: true }
    }
}

/// Settings › Boxes above windows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AboveSettings {
    /// The shortcut that brings the boxes above the windows ("" = none): "win+shift+d", ...
    pub hotkey: String,
    /// The screen corner that does it when the pointer rests there ("" = none): tl, tr, bl, br.
    pub corner: String,
    /// The windows behind are dimmed a little.
    pub dim: bool,
}

impl Default for AboveSettings {
    fn default() -> Self {
        AboveSettings {
            hotkey: "win+shift+d".into(),
            corner: String::new(),
            dim: true,
        }
    }
}

/// Settings › Saved layouts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedSettings {
    /// Deskboxes › Saved layouts in the desktop menu.
    pub in_menu: bool,
}

impl Default for SavedSettings {
    fn default() -> Self {
        SavedSettings { in_menu: true }
    }
}

/// Settings › Hide all.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HideSettings {
    /// Double-clicking the bare desktop hides the boxes and icons (and brings them back).
    pub double_click: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            schema: 1,
            enabled: true,
            language: String::new(),
            default_style: BoxStyle::default(),
            rollup: RollupSettings::default(),
            tabs: TabSettings::default(),
            privacy: PrivacySettings::default(),
            extras: ExtrasSettings::default(),
            show_icons: true,
            look: LookSettings::default(),
            lock_all: false,
            az: AzSettings::default(),
            search: SearchSettings::default(),
            icons: IconSettings::default(),
            hide: HideSettings::default(),
            saved: SavedSettings::default(),
            above: AboveSettings::default(),
            update: UpdateSettings::default(),
            pages: PagesSettings::default(),
        }
    }
}

/// How an A–Z box shows its letters (Settings › A–Z index).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AzSettings {
    /// A row of the box's first letters under its title: the one you are at is marked; click
    /// one (or drag along the row) to jump there.
    pub letter_bar: bool,
    /// The first icon of each letter has the letter beside it.
    pub letter_tags: bool,
}

impl Default for AzSettings {
    fn default() -> Self {
        AzSettings { letter_bar: true, letter_tags: true }
    }
}

/// How icons are drawn (Settings › Look › Icons).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IconSettings {
    /// Tint of the icons in boxes: "" = none, "box" = each box's color, or "#RRGGBB".
    pub tint: String,
    /// How strongly icons take the tint, 0..1.
    pub tint_strength: f32,
    /// Icons in boxes blend into the wallpaper: they take the colors under them and fade, and
    /// come back when the pointer is on the box.
    pub blend: bool,
    /// How far they blend in, 0..1.
    pub blend_strength: f32,
    /// The tint and the blending apply to desktop icons outside boxes too.
    pub desktop: bool,
    /// Blending boxes also fade their background and title.
    pub blend_boxes: bool,
    /// Pointing at a box brings it back in full color.
    pub full_on_hover: bool,
    /// Everything comes back in full color while the desktop is in use (it has the keyboard
    /// focus: after a click on it, until another window is used).
    pub full_when_active: bool,
    /// Version of the icon look: 2 = phone-style tint (an older file's strength is raised to
    /// full on load, the old 70% left the icons' own colors showing).
    #[serde(default)]
    pub fx_v: u32,
}

impl Default for IconSettings {
    fn default() -> Self {
        IconSettings {
            tint: String::new(),
            tint_strength: 1.0,
            blend: false,
            blend_strength: 0.8,
            desktop: false,
            blend_boxes: true,
            full_on_hover: true,
            full_when_active: true,
            fx_v: 2,
        }
    }
}

/// The desktop search field (Settings › Search).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchSettings {
    /// The field is always on the desktop (off: it shows only while searching, from Ctrl+F or
    /// typing).
    pub field: bool,
    /// Typing on the desktop starts a search.
    pub type_to_search: bool,
}

impl Default for SearchSettings {
    fn default() -> Self {
        SearchSettings { field: true, type_to_search: true }
    }
}

/// How box titles look.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LookSettings {
    /// The title bar: "band" (a darker band), "solid" (an opaque band), "line" (a thin line under
    /// the title) or "clear" (nothing: the title sits on the box).
    pub title_bar: String,
    /// How dark the band is, 0..1 ("band").
    pub band: f32,
    /// "center" or "left" (a box with one title; tabs always start at the left).
    pub align: String,
    /// Title text size in DIPs; the title bar's height follows it.
    pub title_size: f32,
    /// Corner radius in DIPs.
    pub radius: f32,
    /// Box titles show only while the pointer is on the box (rolled-up boxes always show theirs).
    pub titles_on_hover: bool,
    /// Scroll bars show only while the pointer is on the box.
    pub scroll_on_hover: bool,
    /// Title font family ("" = Windows' icon font, as the desktop's labels).
    pub title_font: String,
    /// Weight of the box titles (and the open tab): "light", "normal", "semibold", "bold".
    pub title_weight: String,
    /// The box's outline: "" (a faint line, as before), "none", "accent", "box" (the box's color),
    /// "white", "black".
    pub outline: String,
    /// Outline width in DIPs.
    pub outline_w: f32,
    /// Roll-up and menu buttons in a box's title while the pointer is on it.
    pub title_buttons: bool,
}

impl Default for LookSettings {
    fn default() -> Self {
        LookSettings {
            title_bar: "band".into(),
            band: 0.22,
            align: "center".into(),
            title_size: 13.0,
            radius: 8.0,
            titles_on_hover: false,
            scroll_on_hover: false,
            title_font: String::new(),
            title_weight: "semibold".into(),
            outline: String::new(),
            outline_w: 1.0,
            title_buttons: false,
        }
    }
}

/// A style preset: title bar, band, alignment, tab style, title size, corners.
pub struct LookPreset {
    pub id: &'static str,
    pub title_bar: &'static str,
    pub band: f32,
    pub align: &'static str,
    pub tabs: &'static str,
    pub title_size: f32,
    pub radius: f32,
}

/// The style presets (our own), in the order the settings window shows them.
pub const LOOK_PRESETS: &[LookPreset] = &[
    LookPreset { id: "standard", title_bar: "band", band: 0.22, align: "center", tabs: "pills", title_size: 13.0, radius: 8.0 },
    LookPreset { id: "minimal", title_bar: "clear", band: 0.22, align: "left", tabs: "underline", title_size: 13.0, radius: 8.0 },
    LookPreset { id: "classic", title_bar: "line", band: 0.22, align: "center", tabs: "segments", title_size: 13.0, radius: 4.0 },
    LookPreset { id: "bold", title_bar: "solid", band: 0.22, align: "left", tabs: "pills", title_size: 15.0, radius: 6.0 },
    LookPreset { id: "soft", title_bar: "band", band: 0.10, align: "center", tabs: "underline", title_size: 13.0, radius: 14.0 },
    LookPreset { id: "sharp", title_bar: "band", band: 0.38, align: "left", tabs: "segments", title_size: 12.0, radius: 0.0 },
];

impl LookSettings {
    /// The preset these settings (with the tab style) are, if any.
    pub fn preset(&self, tabs: &str) -> Option<&'static str> {
        LOOK_PRESETS
            .iter()
            .find(|p| {
                p.title_bar == self.title_bar
                    && (p.title_bar != "band" || (p.band - self.band).abs() < 0.005)
                    && p.align == self.align
                    && p.tabs == tabs
                    && (p.title_size - self.title_size).abs() < 0.01
                    && (p.radius - self.radius).abs() < 0.01
            })
            .map(|p| p.id)
    }
}

/// Rolling boxes up to their title bar.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RollupSettings {
    /// Boxes docked at a screen edge roll up while the pointer is away.
    pub edges: bool,
    /// How a rolled-up box opens: "hover", "delay" (hover a moment) or "click" (its title).
    pub reveal: String,
    /// Double-clicking a title rolls the box up / down.
    pub double_click: bool,
}

impl Default for RollupSettings {
    fn default() -> Self {
        RollupSettings {
            edges: true,
            reveal: "hover".into(),
            double_click: true,
        }
    }
}

/// Privacy mode: icons drawn as blank outlines and names as dashes, for sharing the screen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacySettings {
    /// The feature is there (its desktop menu entry). Off: privacy mode can't be turned on.
    pub allowed: bool,
    /// Privacy mode is on (only the desktop menu turns it on).
    pub on: bool,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        PrivacySettings { allowed: true, on: false }
    }
}

/// Small touches, each can be turned off (Settings › Look › Little touches).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtrasSettings {
    /// Boxes scroll smoothly (wheel, letter index, search) instead of a row at once.
    pub smooth_scroll: bool,
    /// Dragging several icons shows how many next to the pointer.
    pub drag_count: bool,
    /// A box being moved casts a soft shadow, as if lifted.
    pub lift_box: bool,
    /// Ctrl + mouse wheel makes the icons bigger or smaller (as on Explorer's desktop).
    pub ctrl_wheel_size: bool,
}

impl Default for ExtrasSettings {
    fn default() -> Self {
        ExtrasSettings { smooth_scroll: true, drag_count: true, lift_box: true, ctrl_wheel_size: true }
    }
}

/// Tabs in box titles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabSettings {
    /// "pills", "underline" or "segments".
    pub style: String,
    /// Pointing at a tab for a moment opens it.
    pub hover_switch: bool,
    /// A box dropped by its title onto another box's title becomes a tab there.
    pub combine: bool,
}

impl Default for TabSettings {
    fn default() -> Self {
        TabSettings {
            style: "pills".into(),
            hover_switch: false,
            combine: true,
        }
    }
}
