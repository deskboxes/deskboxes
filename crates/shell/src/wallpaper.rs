//! Wallpaper per monitor through `IDesktopWallpaper` (input for the Mica emulation, §4).

use windows::Win32::Foundation::RECT;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;

#[derive(Clone, Debug)]
pub struct MonitorWallpaper {
    pub rect: RECT,
    /// Empty = solid color.
    pub path: String,
    /// DWPOS_*: 0 center, 1 tile, 2 stretch, 3 fit, 4 fill, 5 span.
    pub position: i32,
    /// COLORREF (0x00BBGGRR) of the background color.
    pub color: u32,
}

pub fn monitors() -> Vec<MonitorWallpaper> {
    unsafe {
        let Ok(dw) = CoCreateInstance::<_, IDesktopWallpaper>(&DesktopWallpaper, None, CLSCTX_ALL) else { return vec![] };
        let pos = dw.GetPosition().map(|p| p.0).unwrap_or(4);
        let color = dw.GetBackgroundColor().map(|c| c.0).unwrap_or(0);
        let n = dw.GetMonitorDevicePathCount().unwrap_or(0);
        let mut v = Vec::new();
        for i in 0..n {
            let Ok(id) = dw.GetMonitorDevicePathAt(i) else { continue };
            let rect = dw.GetMonitorRECT(windows::core::PCWSTR(id.0)).unwrap_or_default();
            let path = dw.GetWallpaper(windows::core::PCWSTR(id.0)).ok().and_then(|p| {
                let s = p.to_string().ok();
                CoTaskMemFree(Some(p.0 as _));
                s
            });
            CoTaskMemFree(Some(id.0 as _));
            v.push(MonitorWallpaper { rect, path: path.unwrap_or_default(), position: pos, color });
        }
        v
    }
}

/// The desktop's wallpaper as the registry has it: (file, DWPOS_* position, COLORREF), read
/// without `IDesktopWallpaper`. That one is served by Explorer, which at sign-in is busy starting
/// and can keep a caller waiting for seconds. One wallpaper for all monitors (the last one set):
/// good for the first frame; the caller checks with `monitors()` later.
pub fn from_registry() -> (String, i32, u32) {
    use crate::reg;
    let path = reg::get_string(r"Control Panel\Desktop", "WallPaper").unwrap_or_default();
    let style = reg::get_string(r"Control Panel\Desktop", "WallpaperStyle").and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(10);
    let tile = reg::get_string(r"Control Panel\Desktop", "TileWallpaper").map(|s| s.trim() == "1").unwrap_or(false);
    let position = match style {
        0 if tile => 1,
        0 => 0,
        2 => 2,
        6 => 3,
        22 => 5,
        _ => 4,
    };
    let color = reg::get_string(r"Control Panel\Colors", "Background")
        .map(|s| {
            let c: Vec<u32> = s.split_whitespace().filter_map(|v| v.parse::<u32>().ok()).map(|v| v.min(255)).collect();
            if c.len() == 3 {
                c[0] | c[1] << 8 | c[2] << 16
            } else {
                0
            }
        })
        .unwrap_or(0);
    let path = if path.is_empty() || std::path::Path::new(&path).exists() { path } else { String::new() };
    (path, position, color)
}
