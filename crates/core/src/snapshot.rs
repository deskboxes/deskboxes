//! Snapshot: everything the first frame needs, in one file read (docs/ARCHITECTURE.md §8, §9).
//! Items with their display names and icon references, plus the icon pixels (premultiplied BGRA)
//! for one DPI / icon size. Little-endian binary; a version bump discards old snapshots.

use crate::model::{Item, ItemKey, ItemKind, Source};

pub const MAGIC: &[u8; 4] = b"DBXS";
pub const VERSION: u32 = 10;

#[derive(Clone, Debug, PartialEq)]
pub struct IconImage {
    pub w: u32,
    pub h: u32,
    /// Premultiplied BGRA, w*h*4 bytes.
    pub bgra: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// Identifies what the snapshot was taken of (desktop folders or the fixture dir).
    pub source_id: String,
    pub dpi: u32,
    pub icon_px: u32,
    /// Explorer's desktop icon spacing in physical px when the snapshot was taken (0 = unknown),
    /// so the first frame lays out the grid without asking Explorer.
    pub cell_w: u32,
    pub cell_h: u32,
    pub items: Vec<Item>,
    pub icons: Vec<IconImage>,
}

struct W(Vec<u8>);
impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn s(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
    }
}

struct R<'a>(&'a [u8], usize);
impl R<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let v = self.0.get(self.1..self.1 + n)?;
        self.1 += n;
        Some(v)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn s(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
}

fn kind_code(k: ItemKind) -> u8 {
    match k {
        ItemKind::File => 0,
        ItemKind::Folder => 1,
        ItemKind::Shortcut => 2,
        ItemKind::UrlFile => 3,
        ItemKind::Virtual => 4,
    }
}
fn kind_of(c: u8) -> ItemKind {
    match c {
        1 => ItemKind::Folder,
        2 => ItemKind::Shortcut,
        3 => ItemKind::UrlFile,
        4 => ItemKind::Virtual,
        _ => ItemKind::File,
    }
}
fn source_code(s: Source) -> u8 {
    match s {
        Source::User => 0,
        Source::Public => 1,
        Source::Virtual => 2,
        Source::Fixture => 3,
        Source::Folder => 4,
    }
}
fn source_of(c: u8) -> Source {
    match c {
        1 => Source::Public,
        2 => Source::Virtual,
        3 => Source::Fixture,
        4 => Source::Folder,
        _ => Source::User,
    }
}

impl Snapshot {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W(Vec::with_capacity(64 * 1024 + self.icons.iter().map(|i| i.bgra.len()).sum::<usize>()));
        w.0.extend_from_slice(MAGIC);
        w.u32(VERSION);
        w.s(&self.source_id);
        w.u32(self.dpi);
        w.u32(self.icon_px);
        w.u32(self.cell_w);
        w.u32(self.cell_h);
        w.u32(self.items.len() as u32);
        for it in &self.items {
            w.s(&it.key.0);
            w.s(&it.parsing_name);
            w.s(&it.display_name);
            w.u8(kind_code(it.kind));
            w.u8(source_code(it.source));
            w.s(&it.ext);
            w.u64(it.size);
            w.u64(it.modified);
            w.u32(it.attrs);
            w.u32(it.icon);
            w.s(&it.link);
        }
        w.u32(self.icons.len() as u32);
        for ic in &self.icons {
            w.u32(ic.w);
            w.u32(ic.h);
            w.0.extend_from_slice(&ic.bgra);
        }
        w.0
    }

    pub fn decode(b: &[u8]) -> Option<Snapshot> {
        let mut r = R(b, 0);
        if r.take(4)? != MAGIC || r.u32()? != VERSION {
            return None;
        }
        let mut s = Snapshot { source_id: r.s()?, dpi: r.u32()?, icon_px: r.u32()?, cell_w: r.u32()?, cell_h: r.u32()?, ..Default::default() };
        let n = r.u32()? as usize;
        s.items.reserve(n);
        for _ in 0..n {
            let key = ItemKey(r.s()?);
            let parsing_name = r.s()?;
            let display_name = r.s()?;
            let kind = kind_of(r.u8()?);
            let source = source_of(r.u8()?);
            let ext = r.s()?;
            s.items.push(Item { key, parsing_name, display_name, kind, ext, size: r.u64()?, modified: r.u64()?, attrs: r.u32()?, source, icon: r.u32()?, link: r.s()? });
        }
        let m = r.u32()? as usize;
        for _ in 0..m {
            let (w, h) = (r.u32()?, r.u32()?);
            let bgra = r.take((w * h * 4) as usize)?.to_vec();
            s.icons.push(IconImage { w, h, bgra });
        }
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let s = Snapshot {
            source_id: "fixture:C:\\x".into(),
            dpi: 144,
            icon_px: 72,
            cell_w: 90,
            cell_h: 120,
            items: vec![Item {
                key: ItemKey::file(0xabc, 42),
                parsing_name: "C:\\x\\Zażółć gęślą jaźń.txt".into(),
                display_name: "Zażółć gęślą jaźń.txt".into(),
                kind: ItemKind::Shortcut,
                ext: ".lnk".into(),
                size: 7,
                modified: 133_000_000_000_000_000,
                attrs: 0x20,
                source: Source::Fixture,
                icon: 0,
                link: "C:\\Gry\\Wiedźmin 3\\witcher3.exe -skip".into(),
            }],
            icons: vec![IconImage { w: 2, h: 1, bgra: vec![1, 2, 3, 4, 5, 6, 7, 8] }],
        };
        let b = s.encode();
        assert_eq!(Snapshot::decode(&b), Some(s));
        assert_eq!(Snapshot::decode(&b[..b.len() - 1]), None);
    }
}
