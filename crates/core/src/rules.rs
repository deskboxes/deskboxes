//! Sorting rules: which box tab an icon goes into, by its type, name, extension, age or size.
//! Rules only change the layout (where an icon is shown); files are never touched.

use crate::layout::{move_to_tab, place_of};
use crate::model::*;

/// Icon types a "type" rule can match: id and the extensions (lowercase, no dot) it covers.
/// Folders, web links and system icons are matched by kind instead (empty lists here).
pub const CATEGORIES: [(&str, &[&str]); 14] = [
    ("folders", &[]),
    ("programs", &["exe", "lnk", "appref-ms", "pif"]),
    ("weblinks", &["url", "website"]),
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

pub fn in_category(it: &Item, cat: &str) -> bool {
    match cat {
        "folders" => it.kind == ItemKind::Folder,
        "weblinks" => it.kind == ItemKind::UrlFile || matches!(ext_of(it).as_str(), "url" | "website"),
        "system" => it.kind == ItemKind::Virtual,
        "programs" => it.kind == ItemKind::Shortcut || CATEGORIES[1].1.contains(&ext_of(it).as_str()),
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
            names(it).iter().any(|n| match r.op.as_str() {
                "starts" => n.starts_with(&v),
                "ends" => n.ends_with(&v),
                "is" => *n == v,
                _ => n.contains(&v),
            })
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
/// any desktop page).
pub fn target(l: &MonitorLayout, it: &Item, now: u64) -> Option<u32> {
    l.rules.iter().filter(|r| crate::pages::find_tab(l, r.target).is_some()).find(|r| matches(r, it, now)).map(|r| r.target)
}

/// Put `items` where the rules say (in the given order). `fresh` = they just appeared on the
/// desktop: those matching no rule go to the "new icons" tab, if one is set. Items already in
/// their tab stay where they are. Returns whether anything moved.
pub fn apply(l: &mut MonitorLayout, items: &[&Item], fresh: bool, now: u64) -> bool {
    let mut changed = false;
    for it in items {
        let dest = target(l, it, now).or(if fresh { l.new_items_to.filter(|t| crate::pages::find_tab(l, *t).is_some()) } else { None });
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

/// A new rule (defaults for its kind) sending icons to tab `target`.
pub fn new_rule(l: &mut MonitorLayout, kind: &str, op: &str, target: u32) -> u32 {
    let id = crate::layout::alloc_id(l);
    let mut r = Rule { id, target, ..Rule::default() };
    set_kind(&mut r, kind);
    if !op.is_empty() {
        r.op = op.to_string();
    }
    l.rules.push(r);
    id
}

/// Change a rule's kind, resetting its condition to that kind's defaults.
pub fn set_kind(r: &mut Rule, kind: &str) {
    let (op, value) = match kind {
        "name" => ("contains", ""),
        "ext" => ("", ""),
        "age" => ("newer", "7"),
        "size" => ("larger", "100"),
        _ => ("documents", ""),
    };
    r.kind = if matches!(kind, "name" | "ext" | "age" | "size") { kind.to_string() } else { "type".to_string() };
    r.op = op.to_string();
    r.value = value.to_string();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Grid, Rect};
    use crate::layout::{create_box, reconcile};

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
        }
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
        let rule = |kind: &str, op: &str, value: &str| Rule {
            id: 1,
            enabled: true,
            kind: kind.into(),
            op: op.into(),
            value: value.into(),
            target: 0,
        };
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
