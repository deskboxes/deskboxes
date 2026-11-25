//! The wallpaper as decoded for the main screen, kept between starts (`start_wallpaper_prefetch`):
//! read back instead of decoded again. At sign-in the decode took 750-880 ms on the dev PC (a
//! 2560x1440 PNG, about 50 ms otherwise), most of the time before the first frame; a slower PC
//! took longer still. Reading the pixels back needs no image codec and next to no CPU.
//!
//! File: `wallpaper.cache` next to the snapshot: "DBXWALL1", the key's length (u32 LE) and the
//! key (the picture's path, size and time, its position and the screen's size), the decoded
//! width and height and the picture's own (u32 LE each), then the premultiplied BGRA rows.

use deskboxes_core::snapshot::IconImage;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"DBXWALL1";
/// Larger pictures are not kept (an 8K screen's would be 130 MB).
const MAX_BYTES: usize = 40 << 20;

/// What the cached picture must be for: None when the file cannot be read.
pub fn key(path: &str, pos: i32, mw: f32, mh: f32) -> Option<String> {
    let md = std::fs::metadata(path).ok()?;
    let t = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some(format!("{}|{}|{t}|{pos}|{}x{}", path.to_lowercase(), md.len(), mw as u32, mh as u32))
}

/// The cached picture, if it was made for `key`: (pixels, the picture's own size).
pub fn read(file: &Path, key: &str) -> Option<(IconImage, (u32, u32))> {
    let mut f = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(file).ok()?);
    let mut magic = [0u8; 8];
    f.read_exact(&mut magic).ok()?;
    if &magic != MAGIC {
        return None;
    }
    let mut n = [0u8; 4];
    let mut u32_of = |f: &mut dyn Read| -> Option<u32> {
        f.read_exact(&mut n).ok()?;
        Some(u32::from_le_bytes(n))
    };
    let klen = u32_of(&mut f)? as usize;
    if klen != key.len() {
        return None;
    }
    let mut k = vec![0u8; klen];
    f.read_exact(&mut k).ok()?;
    if k != key.as_bytes() {
        return None;
    }
    let (w, h, ow, oh) = (u32_of(&mut f)?, u32_of(&mut f)?, u32_of(&mut f)?, u32_of(&mut f)?);
    let bytes = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
    if bytes == 0 || bytes > MAX_BYTES {
        return None;
    }
    let mut bgra = vec![0u8; bytes];
    f.read_exact(&mut bgra).ok()?;
    Some((IconImage { w, h, bgra }, (ow, oh)))
}

/// Kept for the next start, on a thread of its own a while later (not competing with the start
/// itself), written whole or not at all.
pub fn write_later(file: PathBuf, key: String, img: IconImage, orig: (u32, u32)) {
    if img.bgra.len() > MAX_BYTES || img.bgra.len() != img.w as usize * img.h as usize * 4 {
        return;
    }
    let _ = std::thread::Builder::new().name("wallpaper-cache".into()).spawn(move || {
        unsafe {
            use windows::Win32::System::Threading::*;
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST);
        }
        std::thread::sleep(std::time::Duration::from_secs(10));
        let tmp = file.with_extension("cache.tmp");
        let r = (|| -> std::io::Result<()> {
            let mut f = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&tmp)?);
            f.write_all(MAGIC)?;
            f.write_all(&(key.len() as u32).to_le_bytes())?;
            f.write_all(key.as_bytes())?;
            for v in [img.w, img.h, orig.0, orig.1] {
                f.write_all(&v.to_le_bytes())?;
            }
            f.write_all(&img.bgra)?;
            f.into_inner().map_err(|e| e.into_error())?.sync_all()?;
            std::fs::rename(&tmp, &file)
        })();
        if let Err(e) = r {
            log::warn!("wallpaper cache: {e}");
            let _ = std::fs::remove_file(&tmp);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_key() {
        let dir = std::env::temp_dir().join(format!("dbx-wallcache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("wallpaper.cache");
        let img = IconImage { w: 3, h: 2, bgra: (0..24).collect() };
        let tmp = file.with_extension("cache.tmp");
        // (The writer's body, without its wait.)
        let mut f = std::fs::File::create(&tmp).unwrap();
        f.write_all(MAGIC).unwrap();
        f.write_all(&3u32.to_le_bytes()).unwrap();
        f.write_all(b"k|1").unwrap();
        for v in [3u32, 2, 30, 20] {
            f.write_all(&v.to_le_bytes()).unwrap();
        }
        f.write_all(&img.bgra).unwrap();
        drop(f);
        std::fs::rename(&tmp, &file).unwrap();
        let (got, orig) = read(&file, "k|1").unwrap();
        assert_eq!((got.w, got.h, orig), (3, 2, (30, 20)));
        assert_eq!(got.bgra, img.bgra);
        assert!(read(&file, "k|2").is_none());
        assert!(key(dir.join("none.png").to_str().unwrap(), 4, 1.0, 1.0).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
