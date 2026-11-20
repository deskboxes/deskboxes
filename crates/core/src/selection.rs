//! Selection: a set of items with an anchor (Shift ranges) and a focus (keyboard).

use crate::model::ItemKey;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub set: BTreeSet<ItemKey>,
    pub anchor: Option<ItemKey>,
    pub focus: Option<ItemKey>,
}

impl Selection {
    pub fn clear(&mut self) {
        self.set.clear();
        self.anchor = None;
    }
    pub fn is_selected(&self, k: &ItemKey) -> bool {
        self.set.contains(k)
    }
    /// Plain click.
    pub fn select_only(&mut self, k: &ItemKey) {
        self.set.clear();
        self.set.insert(k.clone());
        self.anchor = Some(k.clone());
        self.focus = Some(k.clone());
    }
    /// Ctrl+click.
    pub fn toggle(&mut self, k: &ItemKey) {
        if !self.set.remove(k) {
            self.set.insert(k.clone());
        }
        self.anchor = Some(k.clone());
        self.focus = Some(k.clone());
    }
    /// Shift+click: the range between the anchor and `k` in `order` (display order).
    pub fn range(&mut self, k: &ItemKey, order: &[ItemKey], additive: bool) {
        let anchor = self.anchor.clone().unwrap_or_else(|| k.clone());
        let a = order.iter().position(|x| *x == anchor);
        let b = order.iter().position(|x| x == k);
        if !additive {
            self.set.clear();
        }
        if let (Some(a), Some(b)) = (a, b) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            self.set.extend(order[lo..=hi].iter().cloned());
        } else {
            self.set.insert(k.clone());
        }
        self.anchor = Some(anchor);
        self.focus = Some(k.clone());
    }
    /// Rubber band: `base` = selection when the drag started (Ctrl keeps and toggles it).
    pub fn rubber_band(&mut self, base: &BTreeSet<ItemKey>, hit: &BTreeSet<ItemKey>, ctrl: bool) {
        self.set = if ctrl { base.symmetric_difference(hit).cloned().collect() } else { hit.clone() };
    }
    /// Selected keys in display order, the focused (primary) one first: the shell builds the menu
    /// for the most recently selected item (verified, docs/ARCHITECTURE.md §12).
    pub fn ordered(&self, order: &[ItemKey]) -> Vec<ItemKey> {
        let mut v: Vec<ItemKey> = order.iter().filter(|k| self.set.contains(*k)).cloned().collect();
        if let Some(f) = &self.focus {
            if let Some(i) = v.iter().position(|k| k == f) {
                let k = v.remove(i);
                v.insert(0, k);
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn k(s: &str) -> ItemKey {
        ItemKey(s.into())
    }
    #[test]
    fn click_ctrl_shift() {
        let order: Vec<ItemKey> = ["a", "b", "c", "d"].iter().map(|s| k(s)).collect();
        let mut s = Selection::default();
        s.select_only(&k("b"));
        s.range(&k("d"), &order, false);
        assert_eq!(s.set.len(), 3);
        s.toggle(&k("c"));
        assert!(!s.is_selected(&k("c")));
        // Focus moved to c, which is now unselected: plain display order.
        assert_eq!(s.ordered(&order), vec![k("b"), k("d")]);
        s.toggle(&k("a"));
        assert_eq!(s.ordered(&order), vec![k("a"), k("b"), k("d")]);
    }
}
