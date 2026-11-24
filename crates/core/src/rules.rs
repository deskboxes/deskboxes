//! Sorting rules: which box tab an icon goes into, by its type, name, shortcut target,
//! extension, age or size. Rules only change the layout (where an icon is shown); files are
//! never touched.

use crate::layout::{move_to_tab, place_of};
use crate::model::*;

/// Icon types a "type" rule can match: id and the extensions (lowercase, no dot) it covers.
/// Folders, web links, games, game launchers and system icons are matched by kind or by where
/// they lead instead (empty lists here). Programs stay second (`in_category` reads its list).
pub const CATEGORIES: [(&str, &[&str]); 16] = [
    ("folders", &[]),
    ("programs", &["exe", "lnk", "appref-ms", "pif"]),
    ("weblinks", &["url", "website"]),
    ("games", &[]),
    ("launchers", &[]),
    (
        "documents",
        &[
            "pdf", "doc", "docx", "docm", "odt", "rtf", "txt", "md", "xls", "xlsx", "xlsm", "csv", "ods", "ppt", "pptx", "odp", "pages", "numbers", "key", "epub", "xps", "oxps",
        ],
    ),
    ("pdf", &["pdf"]),
    ("word", &["doc", "docx", "docm", "dot", "dotx", "odt", "rtf"]),
    ("excel", &["xls", "xlsx", "xlsm", "xlsb", "csv", "ods"]),
    ("powerpoint", &["ppt", "pptx", "pptm", "pps", "ppsx", "odp"]),
    (
        "images",
        &[
            "jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "heif", "tif", "tiff", "svg", "ico", "raw", "cr2", "cr3", "nef", "arw", "dng", "psd", "avif", "jxl",
        ],
    ),
    ("music", &["mp3", "wav", "flac", "m4a", "aac", "ogg", "opus", "wma", "aif", "aiff", "mid", "midi"]),
    ("videos", &["mp4", "mkv", "mov", "avi", "wmv", "webm", "m4v", "flv", "mpg", "mpeg", "3gp", "ts", "mts"]),
    ("archives", &["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "tgz", "iso", "cab"]),
    ("installers", &["msi", "msix", "msixbundle", "appx", "appxbundle"]),
    ("system", &[]),
];

fn ext_of(it: &Item) -> String {
    it.ext.trim_start_matches('.').to_ascii_lowercase()
}

/// File name as the user knows it: Explorer's name, plus the real file name (with extension).
fn names(it: &Item) -> [String; 2] {
    let file = std::path::Path::new(&it.parsing_name).file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
    [it.display_name.to_lowercase(), file]
}

/// An internet shortcut (.url, .website): a web link, or a link into an app (steam:// ...).
fn url_file(it: &Item) -> bool {
    it.kind == ItemKind::UrlFile || matches!(ext_of(it).as_str(), "url" | "website")
}

/// Where a shortcut leads, lowercase ("" for other icons and unread links).
/// The Steam game a link starts (`steam://rungameid/1091500`, `steam://run/...`,
/// `steam.exe -applaunch 1091500`): its app id, for its cover.
pub fn steam_app_id(link: &str) -> Option<u32> {
    let l = link.trim().to_ascii_lowercase();
    let rest = ["steam://rungameid/", "steam://run/", "steam://launch/"]
        .iter()
        .find_map(|p| l.strip_prefix(p))
        .or_else(|| l.find("-applaunch ").map(|i| l[i + "-applaunch ".len()..].trim_start()))?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    // (A shortcut to a mod or a non-Steam game has a 64-bit id: no cover of its own.)
    digits.parse::<u32>().ok().filter(|id| *id > 0)
}

fn link_of(it: &Item) -> String {
    if it.kind == ItemKind::Folder || it.kind == ItemKind::Virtual {
        return String::new();
    }
    it.link.trim().to_lowercase()
}

/// A link to a website. (An internet shortcut whose link could not be read counts as one, as
/// all of them did before links were read.)
fn web_link(link: &str) -> bool {
    link.is_empty() || link.starts_with("http://") || link.starts_with("https://")
}

/// Links that start a game: a game store's "play" link, or the arguments a launcher is given to
/// start one (Steam, GOG Galaxy, Riot Client, Battle.net).
const GAME_LINKS: &[&str] = &[
    "steam://rungameid/",
    "steam://run/",
    "steam://launch/",
    "-applaunch ",
    "com.epicgames.launcher://apps/",
    "uplay://launch/",
    "origin://launchgame/",
    "origin2://game/launch",
    "link2ea://launchgame/",
    "battlenet://",
    "amazon-games://play/",
    "itch://caves/",
    "heroic://launch/",
    "playnite://playnite/start/",
    "/command=rungame",
    "--launch-product=",
    "--exec=\"launch ",
    "--exec=launch ",
    // Minecraft: its own launcher, and the Store editions (by their app ids).
    "\\minecraftlauncher.exe",
    "\\minecraft.exe",
    "microsoft.4297127d64ec6_",
    "microsoft.minecraftuwp_",
];

/// Game stores' own apps: their program files, folders, Store app ids and links into them.
const LAUNCHER_LINKS: &[&str] = &[
    "\\steam.exe",
    "\\epicgameslauncher.exe",
    "\\epic games\\launcher\\",
    "\\galaxyclient.exe",
    "\\battle.net launcher.exe",
    "\\battle.net.exe",
    "\\riot games\\riot client\\",
    "\\riotclientservices.exe",
    "\\ubisoftconnect.exe",
    "\\upc.exe",
    "\\uplay.exe",
    "\\eadesktop.exe",
    "\\ealauncher.exe",
    "\\origin.exe",
    "\\rockstar games\\launcher\\",
    "\\amazon games\\app\\",
    "\\amazon games.exe",
    "\\itch.exe",
    "\\heroic.exe",
    "\\playnite.desktopapp.exe",
    "\\playnite.fullscreenapp.exe",
    "microsoft.gamingapp_",
    "steam://",
    "com.epicgames.launcher://",
    "uplay://",
    "origin://",
    "origin2://",
    "link2ea://",
    "goggalaxy://",
    "amazon-games://",
    "itch://",
    "heroic://",
    "playnite://",
];

/// Folders game stores install games into (a shortcut straight to the game's program).
const GAME_DIRS: &[&str] = &[
    "\\steamapps\\common\\",
    "\\epic games\\",
    "\\gog games\\",
    "\\gog galaxy\\games\\",
    "\\ubisoft game launcher\\games\\",
    "\\ea games\\",
    "\\origin games\\",
    "\\riot games\\",
    "\\rockstar games\\",
    "\\xboxgames\\",
    "\\amazon games\\library\\",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gaming {
    Game,
    Launcher,
}

/// Whether a link (lowercase) starts a game, opens a game store's app, or neither. A launcher
/// told to start a game counts as that game.
fn gaming(link: &str) -> Option<Gaming> {
    if link.is_empty() {
        None
    } else if GAME_LINKS.iter().any(|p| link.contains(p)) {
        Some(Gaming::Game)
    } else if LAUNCHER_LINKS.iter().any(|p| link.contains(p)) {
        Some(Gaming::Launcher)
    } else if GAME_DIRS.iter().any(|p| link.contains(p)) {
        Some(Gaming::Game)
    } else {
        None
    }
}

pub fn in_category(it: &Item, cat: &str) -> bool {
    match cat {
        "folders" => it.kind == ItemKind::Folder,
        "weblinks" => url_file(it) && web_link(&link_of(it)),
        "games" => gaming(&link_of(it)) == Some(Gaming::Game),
        "launchers" => gaming(&link_of(it)) == Some(Gaming::Launcher),
        "system" => it.kind == ItemKind::Virtual,
        // (Internet shortcuts into apps, like steam://, are shortcuts too, not websites.)
        "programs" => it.kind == ItemKind::Shortcut || CATEGORIES[1].1.contains(&ext_of(it).as_str()) || (url_file(it) && !web_link(&link_of(it))),
        _ => {
            if it.kind == ItemKind::Folder || it.kind == ItemKind::Virtual {
                return false;
            }
            let e = ext_of(it);
            CATEGORIES.iter().find(|(id, _)| *id == cat).map(|(_, exts)| exts.contains(&e.as_str())).unwrap_or(false)
        }
    }
}

/// FILETIME (100 ns since 1601) for now.
pub fn now_filetime() -> u64 {
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    (unix + 11_644_473_600) * 10_000_000
}

fn number(v: &str) -> Option<f64> {
    v.trim().replace(',', ".").parse::<f64>().ok().filter(|x| x.is_finite() && *x >= 0.0)
}

/// A name or link condition: `op` = "contains" | "starts" | "ends" | "is" on lowercase text.
fn text_matches(op: &str, text: &str, v: &str) -> bool {
    match op {
        "starts" => text.starts_with(v),
        "ends" => text.ends_with(v),
        "is" => text == v,
        _ => text.contains(v),
    }
}

pub fn matches(r: &Rule, it: &Item, now: u64) -> bool {
    if !r.enabled {
        return false;
    }
    match r.kind.as_str() {
        "type" => in_category(it, &r.op),
        "name" => {
            let v = r.value.trim().to_lowercase();
            if v.is_empty() {
                return false;
            }
            names(it).iter().any(|n| text_matches(&r.op, n, &v))
        }
        "link" => {
            let v = r.value.trim().to_lowercase();
            let link = link_of(it);
            !v.is_empty() && !link.is_empty() && text_matches(&r.op, &link, &v)
        }
        "ext" => {
            let e = ext_of(it);
            !e.is_empty()
                && r.value
                    .split([',', ';', ' '])
                    .map(|x| x.trim().trim_start_matches("*.").trim_start_matches('.').to_ascii_lowercase())
                    .any(|x| !x.is_empty() && x == e)
        }
        "age" => {
            let Some(days) = number(&r.value) else { return false };
            if it.modified == 0 || it.kind == ItemKind::Virtual {
                return false;
            }
            let age_days = now.saturating_sub(it.modified) as f64 / (10_000_000.0 * 86_400.0);
            if r.op == "older" {
                age_days > days
            } else {
                age_days <= days
            }
        }
        "size" => {
            let Some(mb) = number(&r.value) else { return false };
            if it.kind == ItemKind::Folder || it.kind == ItemKind::Virtual {
                return false;
            }
            let size_mb = it.size as f64 / (1024.0 * 1024.0);
            if r.op == "smaller" {
                size_mb < mb
            } else {
                size_mb > mb
            }
        }
        _ => false,
    }
}

/// The box and tab index holding tab id `tab`.
pub fn find_tab(l: &MonitorLayout, tab: u32) -> Option<(u32, usize)> {
    l.boxes.iter().find_map(|b| b.tabs.iter().position(|t| t.id == tab).map(|i| (b.id, i)))
}

/// Where rules send an item: the first enabled rule that matches and whose tab still exists (on
/// any desktop page) and can take icons (a rule pointing at a folder box is passed over, so the
/// next one that matches still applies).
pub fn target(l: &MonitorLayout, it: &Item, now: u64) -> Option<u32> {
    l.rules.iter().filter(|r| crate::pages::sortable_tab(l, r.target).is_some()).find(|r| matches(r, it, now)).map(|r| r.target)
}

/// Put `items` where the rules say (in the given order). `fresh` = they just appeared on the
/// desktop: those matching no rule go to the "new icons" tab, if one is set. Items already in
/// their tab stay where they are. Returns whether anything moved.
pub fn apply(l: &mut MonitorLayout, items: &[&Item], fresh: bool, now: u64) -> bool {
    let mut changed = false;
    for it in items {
        let dest = target(l, it, now).or(if fresh { l.new_items_to.filter(|t| crate::pages::sortable_tab(l, *t).is_some()) } else { None });
        let Some(tab) = dest else { continue };
        // (The tab may be on another desktop page: done there.)
        let Some((page, box_id, ti)) = crate::pages::find_tab(l, tab) else { continue };
        let there = crate::pages::with_page(l, page, |l| matches!(place_of(l, &it.key), Some(Place::Box { tab: t, .. }) if t == tab)).unwrap_or(false);
        if there {
            continue;
        }
        changed |= crate::pages::with_page(l, page, |l| move_to_tab(l, std::slice::from_ref(&it.key), box_id, ti, None)).unwrap_or(false);
    }
    changed
}

/// Icon types inside another (all or nearly all of the first are also the second). The first
/// rule that matches wins, so a rule for the narrower type behind one for the broader type would
/// hardly ever get an icon.
const WITHIN: [(&str, &str); 6] = [("games", "programs"), ("launchers", "programs"), ("pdf", "documents"), ("word", "documents"), ("excel", "documents"), ("powerpoint", "documents")];

/// Rule `narrow` is for a type inside rule `broad`'s type.
fn within(narrow: &Rule, broad: &Rule) -> bool {
    narrow.kind == "type" && broad.kind == "type" && WITHIN.contains(&(narrow.op.as_str(), broad.op.as_str()))
}

/// Move rule `id` where a broader type rule does not hide it: a rule for a narrower type goes
/// just before the first rule above it for a broader one (games before programs); a rule for a
/// broader type, just after the last narrower one below it. Other rules keep their order.
/// Returns whether it moved.
pub fn order_rule(l: &mut MonitorLayout, id: u32) -> bool {
    let Some(i) = l.rules.iter().position(|r| r.id == id) else { return false };
    let to = if let Some(j) = (0..i).find(|&j| within(&l.rules[i], &l.rules[j])) {
        j
    } else if let Some(k) = (i + 1..l.rules.len()).rev().find(|&k| within(&l.rules[k], &l.rules[i])) {
        // (Index `k` once rule `i` is out of the list: right after the narrower rule.)
        k
    } else {
        return false;
    };
    let r = l.rules.remove(i);
    l.rules.insert(to, r);
    true
}

/// A new rule (defaults for its kind) sending icons to tab `target`. It goes last, or ahead of
/// rules for a broader type (see `order_rule`).
pub fn new_rule(l: &mut MonitorLayout, kind: &str, op: &str, target: u32) -> u32 {
    let id = crate::layout::alloc_id(l);
    let mut r = Rule { id, target, ..Rule::default() };
    set_kind(&mut r, kind);
    if !op.is_empty() {
        r.op = op.to_string();
    }
    l.rules.push(r);
    order_rule(l, id);
    id
}

/// Change a rule's kind, resetting its condition to that kind's defaults.
pub fn set_kind(r: &mut Rule, kind: &str) {
    let (op, value) = match kind {
        "name" | "link" => ("contains", ""),
        "ext" => ("", ""),
        "age" => ("newer", "7"),
        "size" => ("larger", "100"),
        _ => ("documents", ""),
    };
    r.kind = if matches!(kind, "name" | "link" | "ext" | "age" | "size") { kind.to_string() } else { "type".to_string() };
    r.op = op.to_string();
    r.value = value.to_string();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Grid, Rect};
    use crate::layout::{create_box, reconcile};

    #[test]
    fn steam_app_ids() {
        assert_eq!(steam_app_id("steam://rungameid/1091500"), Some(1091500));
        assert_eq!(steam_app_id("  STEAM://run/945360/"), Some(945360));
        assert_eq!(steam_app_id("steam://launch/730/dialog"), Some(730));
        assert_eq!(steam_app_id(r#""C:\Program Files (x86)\Steam\steam.exe" -applaunch 284160 -fullscreen"#), Some(284160));
        // A non-Steam game's shortcut (64-bit id), Big Picture, a website: none.
        assert_eq!(steam_app_id("steam://rungameid/12345678901234567890"), None);
        assert_eq!(steam_app_id("steam://open/bigpicture"), None);
        assert_eq!(steam_app_id("https://store.steampowered.com/app/730"), None);
        assert_eq!(steam_app_id(""), None);
    }

    #[test]
    fn covers_view_round_trips() {
        use crate::model::View;
        assert_eq!(View::parse("covers"), View::Covers);
        assert_eq!(View::Covers.id(), "covers");
    }

    fn item(name: &str, kind: ItemKind, size: u64, modified: u64) -> Item {
        let ext = std::path::Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
        Item {
            key: ItemKey::path(&format!("C:\\Users\\u\\Desktop\\{name}")),
            parsing_name: format!("C:\\Users\\u\\Desktop\\{name}"),
            display_name: name.to_string(),
            kind,
            ext,
            size,
            modified,
            attrs: 0,
            source: Source::User,
            icon: 0,
            link: String::new(),
        }
    }

    /// A .lnk or .url leading to `link`.
    fn shortcut(name: &str, link: &str) -> Item {
        let kind = if name.ends_with(".url") { ItemKind::UrlFile } else { ItemKind::Shortcut };
        Item { link: link.into(), ..item(name, kind, 500, now_filetime()) }
    }

    fn rule(kind: &str, op: &str, value: &str) -> Rule {
        Rule {
            id: 1,
            enabled: true,
            kind: kind.into(),
            op: op.into(),
            value: value.into(),
            target: 0,
        }
    }

    #[test]
    fn games_launchers_and_websites() {
        let games = [
            shortcut("Cyberpunk 2077.url", "steam://rungameid/1091500"),
            shortcut("Fortnite.url", "com.epicgames.launcher://apps/fn%3A4fe75bbc%3AFortnite?action=launch&silent=true"),
            shortcut("Assassin's Creed.url", "uplay://launch/635/0"),
            shortcut("Dota 2.lnk", r"C:\Program Files (x86)\Steam\steam.exe -applaunch 570"),
            shortcut("Witcher 3.lnk", r"C:\Program Files (x86)\GOG Galaxy\GalaxyClient.exe /command=runGame /gameId=1207664643"),
            shortcut("VALORANT.lnk", r"C:\Riot Games\Riot Client\RiotClientServices.exe --launch-product=valorant --launch-patchline=live"),
            shortcut("Portal 2.lnk", r"D:\SteamLibrary\steamapps\common\Portal 2\portal2.exe"),
            shortcut("Hades.lnk", r"C:\Program Files\Epic Games\Hades\x64\Hades.exe"),
            shortcut("Minecraft Launcher.lnk", r"::{4234D49B-0245-4DF3-B780-3893943456E1}\Microsoft.4297127D64EC6_8wekyb3d8bbwe!Minecraft"),
        ];
        for g in &games {
            assert!(in_category(g, "games") && !in_category(g, "launchers") && !in_category(g, "weblinks"), "{}", g.display_name);
            assert!(in_category(g, "programs"), "{}", g.display_name);
        }
        let launchers = [
            shortcut("Steam.lnk", r"C:\Program Files (x86)\Steam\steam.exe"),
            shortcut("Epic Games Launcher.lnk", r"C:\Program Files (x86)\Epic Games\Launcher\Portal\Binaries\Win64\EpicGamesLauncher.exe"),
            shortcut("Battle.net.lnk", r"C:\Program Files (x86)\Battle.net\Battle.net Launcher.exe"),
            shortcut("Riot Client.lnk", r"C:\Riot Games\Riot Client\RiotClientServices.exe"),
            shortcut("Xbox.lnk", r"::{4234D49B-0245-4DF3-B780-3893943456E1}\Microsoft.GamingApp_8wekyb3d8bbwe!Microsoft.Xbox.App"),
            shortcut("Steam Big Picture.url", "steam://open/bigpicture"),
        ];
        for s in &launchers {
            assert!(in_category(s, "launchers") && !in_category(s, "games"), "{}", s.display_name);
            assert!(in_category(s, "programs"), "{}", s.display_name);
        }
        // Apps and websites are neither.
        let word = shortcut("Word.lnk", r"C:\Program Files\Microsoft Office\root\Office16\WINWORD.EXE");
        assert!(in_category(&word, "programs") && !in_category(&word, "games") && !in_category(&word, "launchers"));
        let site = shortcut("Steam Store.url", "https://store.steampowered.com/");
        assert!(in_category(&site, "weblinks") && !in_category(&site, "programs") && !in_category(&site, "games"));
        // An internet shortcut not read (a file only in the cloud) is still a web link.
        let unread = shortcut("Somewhere.url", "");
        assert!(in_category(&unread, "weblinks") && !in_category(&unread, "programs"));
        // Shortcut target conditions (any case).
        assert!(matches(&rule("link", "contains", "SteamApps"), &games[6], now_filetime()));
        assert!(matches(&rule("link", "starts", "steam://"), &games[0], now_filetime()));
        assert!(matches(&rule("link", "ends", "live"), &games[5], now_filetime()));
        assert!(matches(&rule("link", "is", "https://store.steampowered.com/"), &site, now_filetime()));
        assert!(!matches(&rule("link", "contains", "steam"), &word, now_filetime()));
        assert!(!matches(&rule("link", "contains", ""), &games[0], now_filetime()));
        assert!(!matches(&rule("link", "contains", "x"), &unread, now_filetime()));
    }

    #[test]
    fn narrower_type_rules_go_ahead_of_broader_ones() {
        let now = now_filetime();
        let g = Grid::new(Rect::new(0.0, 0.0, 750.0, 500.0), 75.0, 100.0);
        let game = shortcut("Cyberpunk 2077.url", "steam://rungameid/1091500");
        let steam = shortcut("Steam.lnk", r"C:\Program Files (x86)\Steam\steam.exe");
        let app = shortcut("Word.lnk", r"C:\Office\WINWORD.EXE");
        let site = shortcut("News.url", "https://example.com/");
        let mut l = MonitorLayout::default();
        reconcile(&mut l, &[game.key.clone(), steam.key.clone(), app.key.clone(), site.key.clone()], &g);
        let apps = create_box(&mut l, Rect::new(450.0, 300.0, 150.0, 200.0), "Apps", BoxStyle::default());
        let gms = create_box(&mut l, Rect::new(600.0, 300.0, 150.0, 200.0), "Games", BoxStyle::default());
        let tab = |l: &MonitorLayout, id: u32| l.boxes.iter().find(|x| x.id == id).unwrap().tabs[0].id;
        let (apps_t, games_t) = (tab(&l, apps), tab(&l, gms));
        // The apps box takes programs and web links (as the "Make boxes" template sets it up).
        new_rule(&mut l, "type", "programs", apps_t);
        new_rule(&mut l, "type", "weblinks", apps_t);
        apply(&mut l, &[&game, &steam, &app, &site], false, now);
        assert_eq!(place_of(&l, &game.key), Some(Place::Box { box_id: apps, tab: apps_t }));
        // Games added later still win over programs; launchers stay with the apps until asked.
        let gr = new_rule(&mut l, "type", "games", games_t);
        assert_eq!(l.rules.iter().map(|r| r.op.as_str()).collect::<Vec<_>>(), ["games", "programs", "weblinks"]);
        assert!(apply(&mut l, &[&game, &steam, &app, &site], false, now));
        assert_eq!(place_of(&l, &game.key), Some(Place::Box { box_id: gms, tab: games_t }));
        assert_eq!(place_of(&l, &steam.key), Some(Place::Box { box_id: apps, tab: apps_t }));
        assert_eq!(place_of(&l, &app.key), Some(Place::Box { box_id: apps, tab: apps_t }));
        assert_eq!(place_of(&l, &site.key), Some(Place::Box { box_id: apps, tab: apps_t }));
        new_rule(&mut l, "type", "launchers", games_t);
        assert!(apply(&mut l, &[&game, &steam, &app, &site], false, now));
        assert_eq!(place_of(&l, &steam.key), Some(Place::Box { box_id: gms, tab: games_t }));
        // A rule turned into a broader type moves below the narrower ones (Settings' type list).
        let first = l.rules[0].id;
        assert_eq!(first, gr);
        l.rules[0].op = "programs".into();
        l.rules.iter_mut().find(|r| r.op == "launchers").unwrap().op = "games".into();
        assert!(order_rule(&mut l, first));
        assert_eq!(l.rules.iter().map(|r| r.op.as_str()).collect::<Vec<_>>(), ["games", "programs", "programs", "weblinks"]);
        assert!(!order_rule(&mut l, first));
        // Other kinds of rule are left where they are put.
        let n = new_rule(&mut l, "name", "", apps_t);
        assert_eq!(l.rules.last().map(|r| r.id), Some(n));
    }

    #[test]
    fn categories_and_conditions() {
        let now = now_filetime();
        let day = 10_000_000u64 * 86_400;
        let pdf = item("Invoice 2026.pdf", ItemKind::File, 3 << 20, now - 2 * day);
        let xl = item("budget.XLSX", ItemKind::File, 1 << 20, now - 40 * day);
        let dir = item("Projects", ItemKind::Folder, 0, now);
        let lnk = item("Steam.lnk", ItemKind::Shortcut, 1000, now);
        assert!(in_category(&pdf, "pdf") && in_category(&pdf, "documents") && !in_category(&pdf, "word"));
        assert!(in_category(&xl, "excel") && !in_category(&xl, "images"));
        assert!(in_category(&dir, "folders") && !in_category(&dir, "documents"));
        assert!(in_category(&lnk, "programs"));
        assert!(matches(&rule("name", "starts", "invoice"), &pdf, now));
        assert!(!matches(&rule("name", "is", "invoice"), &pdf, now));
        assert!(matches(&rule("name", "ends", ".pdf"), &pdf, now));
        assert!(matches(&rule("ext", "", "docx, .xlsx"), &xl, now));
        assert!(matches(&rule("age", "newer", "7"), &pdf, now) && !matches(&rule("age", "newer", "7"), &xl, now));
        assert!(matches(&rule("age", "older", "30"), &xl, now));
        assert!(matches(&rule("size", "larger", "2"), &pdf, now) && matches(&rule("size", "smaller", "2"), &xl, now));
        assert!(!matches(
            &Rule {
                enabled: false,
                ..rule("type", "pdf", "")
            },
            &pdf,
            now
        ));
    }

    #[test]
    fn apply_moves_matching_items_first_rule_wins() {
        let now = now_filetime();
        let g = Grid::new(Rect::new(0.0, 0.0, 750.0, 500.0), 75.0, 100.0);
        let a = item("a.pdf", ItemKind::File, 1, now);
        let b = item("b.docx", ItemKind::File, 1, now);
        let c = item("c.png", ItemKind::File, 1, now);
        let mut l = MonitorLayout::default();
        reconcile(&mut l, &[a.key.clone(), b.key.clone(), c.key.clone()], &g);
        let docs = create_box(&mut l, Rect::new(450.0, 300.0, 150.0, 200.0), "Docs", BoxStyle::default());
        let pdfs = create_box(&mut l, Rect::new(600.0, 300.0, 150.0, 200.0), "PDF", BoxStyle::default());
        let tab = |l: &MonitorLayout, id: u32| l.boxes.iter().find(|x| x.id == id).unwrap().tabs[0].id;
        let (docs_t, pdfs_t) = (tab(&l, docs), tab(&l, pdfs));
        new_rule(&mut l, "type", "pdf", pdfs_t);
        new_rule(&mut l, "type", "documents", docs_t);
        assert!(apply(&mut l, &[&a, &b, &c], false, now));
        assert_eq!(place_of(&l, &a.key), Some(Place::Box { box_id: pdfs, tab: pdfs_t }));
        assert_eq!(place_of(&l, &b.key), Some(Place::Box { box_id: docs, tab: docs_t }));
        assert!(matches!(place_of(&l, &c.key), Some(Place::Grid(_))));
        // nothing left to do
        assert!(!apply(&mut l, &[&a, &b, &c], false, now));
        // new icons matching no rule go to the "new icons" tab
        l.new_items_to = Some(docs_t);
        assert!(apply(&mut l, &[&c], true, now));
        assert_eq!(place_of(&l, &c.key), Some(Place::Box { box_id: docs, tab: docs_t }));
    }
}
