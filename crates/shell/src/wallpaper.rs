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
