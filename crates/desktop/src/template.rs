//! The default sorting template: boxes for apps, games, folders, documents, media and
//! installers & archives, each with a "type" rule, made for the kinds of icons the desktop has.
//! It runs once on a fresh install (no settings, no layout yet), and whenever "Sort now" is used
//! with no rules. The games box shows its games as covers. One undo step; the boxes and rules are ordinary ones the user can change or
//! delete.

use crate::app::{header_h, App, PAD};
use deskboxes_core::geom::Rect;
use deskboxes_core::*;
use deskboxes_i18n::{t, S};

/// The template's boxes: title and the rule categories (`rules::CATEGORIES`) that go there.
const GROUPS: [(S, &[&str]); 6] = [
    (S::TplApps, &["programs", "weblinks"]),
    (S::TplGames, &["games", "launchers"]),
    (S::TplFolders, &["folders"]),
    (S::TplDocuments, &["documents"]),
    (S::TplMedia, &["images", "videos", "music"]),
    (S::TplInstallers, &["installers", "archives"]),
];

/// The order `GROUPS` take their icons in: games before apps (games are programs too).
const CLAIM: [usize; 6] = [1, 0, 2, 3, 4, 5];

/// Gap between the template's boxes and from the screen edge.
const GAP: f32 = 14.0;

impl App {
    /// Make the template's boxes for the loose icons there are (system icons and other files
    /// stay on the desktop, moved together at the top left). Returns how many boxes were made.
    pub fn apply_template(&mut self) -> usize {
        let candidates: Vec<Item> = self
            .layout
            .loose
            .keys()
            .filter_map(|k| self.items.get(k))
            .filter(|i| i.source != Source::Folder && i.kind != ItemKind::Virtual)
            .cloned()
            .collect();
        let mut used: std::collections::HashSet<ItemKey> = Default::default();
        let mut counts = [0usize; GROUPS.len()];
        for g in CLAIM {
            let cats = GROUPS[g].1;
            // A games box only for games (it then takes their launchers along): a launcher
            // alone stays with the apps.
            if cats.contains(&"games") && !candidates.iter().any(|i| rules::in_category(i, "games")) {
                continue;
            }
            let hits: Vec<ItemKey> = candidates.iter().filter(|i| !used.contains(&i.key) && cats.iter().any(|c| rules::in_category(i, c))).map(|i| i.key.clone()).collect();
            counts[g] = hits.len();
            used.extend(hits);
        }
        let plan: Vec<(String, &[&str], usize)> = GROUPS.iter().zip(counts).filter(|(_, n)| *n > 0).map(|((title, cats), n)| (t(self.lang, *title).to_string(), *cats, n)).collect();
        if plan.is_empty() {
            log::info!("template: no icons to sort");
            return 0;
        }
        // Boxes down the right side of the screen, a column at a time, clear of existing boxes.
        let (cw, ch, work) = (self.grid.cell_w, self.grid.cell_h, self.work);
        let mut rects: Vec<Rect> = vec![];
        let (mut right, mut y, mut col_w) = (work.right() - GAP, work.y + GAP, 0.0f32);
        for (_, cats, n) in &plan {
            // The games box shows covers (taller cells, two rows at most).
            let covers = cats.contains(&"games");
            let ((cw, ch), max_rows) = if covers { (crate::covers::cell_size(self.line_h), 2.0) } else { ((cw, ch), 3.0) };
            let cols = (*n).clamp(2, 4) as f32;
            let rows = (*n as f32 / cols).ceil().clamp(1.0, max_rows);
            let (w, h) = (cols * cw + 2.0 * PAD, header_h() + rows * ch + 8.0);
            let mut r;
            loop {
                if y + h > work.bottom() - GAP && y > work.y + GAP {
                    right -= col_w + GAP;
                    y = work.y + GAP;
                    col_w = 0.0;
                }
                r = Rect::new(right - w, y, w, h);
                match self.layout.boxes.iter().map(|b| b.rect).chain(rects.iter().copied()).find(|o| o.inflate(GAP / 2.0).intersects(&r)) {
                    Some(o) if o.bottom() + GAP + h <= work.bottom() - GAP => y = o.bottom() + GAP,
                    Some(_) => y = work.bottom(),
                    None => break,
                }
                if right - w < work.x {
                    break;
                }
            }
            col_w = col_w.max(w);
            y = r.bottom() + GAP;
            rects.push(r);
        }
        let style = self.settings.default_style.clone();
        let mut refs: Vec<&Item> = self.items.values().filter(|i| i.source != Source::Folder).collect();
        refs.sort_by_cached_key(|i| i.display_name.to_lowercase());
        let all: Vec<Item> = refs.into_iter().cloned().collect();
        let loose_order: Vec<ItemKey> = self.order().into_iter().filter(|k| self.layout.loose.contains_key(k)).collect();
        let made = plan.len();
        self.change_layout(|l, g| {
            for ((title, cats, _), r) in plan.iter().zip(&rects) {
                let id = layout::create_box(l, *r, title, style.clone());
                if cats.contains(&"games") {
                    if let Some(b) = l.boxes.iter_mut().find(|b| b.id == id) {
                        b.view = "covers".into();
                    }
                }
                let tab = l.boxes.iter().find(|b| b.id == id).map(|b| b.tabs[0].id).unwrap_or(0);
                for c in cats.iter() {
                    rules::new_rule(l, "type", c, tab);
                }
            }
            let refs: Vec<&Item> = all.iter().collect();
            rules::apply(l, &refs, false, rules::now_filetime());
            layout::arrange_loose(l, g, &loose_order);
        });
        log::info!("template: {made} box(es) made");
        made
    }
}
