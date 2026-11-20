//! Spike (b): desktop-layer attach on the current Windows 11 build.
//!
//! Creates a WS_EX_NOREDIRECTIONBITMAP child window of `Progman`, placed at the top of
//! Progman's child z-order (above SHELLDLL_DefView), renders with D3D11 + D2D +
//! DirectComposition, and verifies with screen captures:
//!   1. our pixels are visible on the desktop,
//!   2. a normal top-level window covers them (we are below normal windows),
//!   3. after Win+D ("show desktop") our pixels are still visible.
//! Optional `--no-wind` skips the Win+D step.
//! Writes evidence crops (BMP) into the directory given by `--out <dir>`.
//!
//! Throwaway code; results are recorded in REPORT.md.

use std::time::Duration;
use windows::core::{w, Interface, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const PROBE: (u8, u8, u8) = (13, 201, 77); // distinctive opaque color

static mut OUT_DIR: String = String::new();

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let no_wind = args.iter().any(|a| a == "--no-wind");
    let out = args
        .iter()
        .position(|a| a == "--out")
        .map(|i| args[i + 1].clone())
        .unwrap_or_else(|| ".".into());
    unsafe {
        OUT_DIR = out;
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        run(no_wind);
    }
}

unsafe fn class_of(h: HWND) -> String {
    let mut b = [0u16; 128];
    let n = GetClassNameW(h, &mut b);
    String::from_utf16_lossy(&b[..n as usize])
}

unsafe fn dump_children(parent: HWND, label: &str) {
    println!("children of {label} {:?}:", parent.0);
    let mut c = GetWindow(parent, GW_CHILD).unwrap_or_default();
    while !c.0.is_null() {
        let mut r = RECT::default();
        let _ = GetWindowRect(c, &mut r);
        println!(
            "   {:?} {:<20} vis={} ex=0x{:08x} rect=({},{})-({},{})",
            c.0,
            class_of(c),
            IsWindowVisible(c).as_bool(),
            GetWindowLongW(c, GWL_EXSTYLE) as u32,
            r.left, r.top, r.right, r.bottom
        );
        c = GetWindow(c, GW_HWNDNEXT).unwrap_or_default();
    }
}

extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match m {
            WM_NCHITTEST => LRESULT(HTCLIENT as isize),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(h, &mut ps);
                // only used by the red "normal window" test window
                let br = CreateSolidBrush(COLORREF(0x000000FF)); // red (BGR)
                FillRect(hdc, &ps.rcPaint, br);
                let _ = DeleteObject(br.into());
                let _ = EndPaint(h, &ps);
                LRESULT(0)
            }
            _ => DefWindowProcW(h, m, w, l),
        }
    }
}

unsafe fn pump_for(d: Duration) {
    let end = std::time::Instant::now() + d;
    while std::time::Instant::now() < end {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Capture a screen region (virtual-screen coords) to BGRA rows.
unsafe fn capture(x: i32, y: i32, w: i32, h: i32) -> Vec<u8> {
    let screen = GetDC(None);
    let mem = CreateCompatibleDC(Some(screen));
    let bmp = CreateCompatibleBitmap(screen, w, h);
    let old = SelectObject(mem, bmp.into());
    let _ = BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT);
    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut buf = vec![0u8; (w * h * 4) as usize];
    GetDIBits(mem, bmp, 0, h as u32, Some(buf.as_mut_ptr() as _), &mut bi, DIB_RGB_COLORS);
    SelectObject(mem, old);
    let _ = DeleteObject(bmp.into());
    let _ = DeleteDC(mem);
    ReleaseDC(None, screen);
    buf
}

unsafe fn save_bmp(name: &str, w: i32, h: i32, bgra: &[u8]) {
    let path = format!("{}\\{}", OUT_DIR, name);
    let mut f = Vec::new();
    let data_len = bgra.len() as u32;
    f.extend_from_slice(b"BM");
    f.extend_from_slice(&(54 + data_len).to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(&54u32.to_le_bytes());
    f.extend_from_slice(&40u32.to_le_bytes());
    f.extend_from_slice(&w.to_le_bytes());
    f.extend_from_slice(&(-h).to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&32u16.to_le_bytes());
    f.extend_from_slice(&[0u8; 24]);
    f.extend_from_slice(bgra);
    let _ = std::fs::write(&path, f);
    println!("   saved {path}");
}

unsafe fn probe(label: &str, x: i32, y: i32) -> (u8, u8, u8) {
    let (w, h) = (200, 120);
    let px = capture(x - 40, y - 40, w, h);
    let i = ((40 * w + 40) * 4) as usize;
    let rgb = (px[i + 2], px[i + 1], px[i]);
    println!("   probe[{label}] at ({x},{y}) = {:?}", rgb);
    save_bmp(&format!("layer_{label}.bmp"), w, h, &px);
    rgb
}

fn near(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
    (a.0 as i32 - b.0 as i32).abs() < 8 && (a.1 as i32 - b.1 as i32).abs() < 8 && (a.2 as i32 - b.2 as i32).abs() < 8
}

unsafe fn send_win_d() {
    let k = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: vk, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, ..Default::default() },
        },
    };
    let seq = [k(VK_LWIN, false), k(VK_D, false), k(VK_D, true), k(VK_LWIN, true)];
    SendInput(&seq, std::mem::size_of::<INPUT>() as i32);
}

unsafe fn run(no_wind: bool) {
    let progman = FindWindowW(w!("Progman"), None).expect("Progman");
    let defview = FindWindowExW(Some(progman), None, w!("SHELLDLL_DefView"), None).unwrap_or_default();
    println!("Progman={:?} ex=0x{:08x}  DefView={:?}", progman.0, GetWindowLongW(progman, GWL_EXSTYLE) as u32, defview.0);
    dump_children(progman, "Progman (before)");

    let hinst = GetModuleHandleW(None).unwrap();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: hinst.into(),
        lpszClassName: w!("DeskboxesSpikeLayer"),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap(),
        ..Default::default()
    };
    RegisterClassW(&wc);

    // Place a 480x300 panel near the primary monitor's top-left work area.
    let mut wa = RECT::default();
    let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut wa as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    let (sx, sy, w, h) = (wa.left + 200, wa.top + 150, 480, 300);
    let mut pt = POINT { x: sx, y: sy };
    let _ = ScreenToClient(progman, &mut pt);

    let t0 = std::time::Instant::now();
    let hwnd = CreateWindowExW(
        WS_EX_NOREDIRECTIONBITMAP,
        w!("DeskboxesSpikeLayer"),
        w!("Deskboxes spike layer"),
        WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
        pt.x, pt.y, w, h,
        Some(progman),
        None,
        Some(hinst.into()),
        None,
    )
    .expect("CreateWindowExW child of Progman");
    let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    println!("created child {:?} in {:?}", hwnd.0, t0.elapsed());
    dump_children(progman, "Progman (after)");

    // --- D3D11 / D2D / DComp ---
    let mut dev: Option<ID3D11Device> = None;
    D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut dev), None, None).expect("D3D11");
    let dxgi: IDXGIDevice = dev.unwrap().cast().unwrap();
    let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).unwrap();
    let d2d = factory.CreateDevice(&dxgi).unwrap();
    let dcomp: IDCompositionDesktopDevice = DCompositionCreateDevice2(&d2d).unwrap();
    let target = dcomp.CreateTargetForHwnd(hwnd, true).expect("CreateTargetForHwnd");
    let visual = dcomp.CreateVisual().unwrap();
    let surface = dcomp.CreateSurface(w as u32, h as u32, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_ALPHA_MODE_PREMULTIPLIED).unwrap();
    let mut off = POINT::default();
    let dc: ID2D1DeviceContext = surface.BeginDraw(None, &mut off).unwrap();
    let (ox, oy) = (off.x as f32, off.y as f32);
    dc.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
    // translucent panel (premultiplied: rgb * a)
    let a = 0.55f32;
    let panel = dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.12 * a, g: 0.16 * a, b: 0.22 * a, a }, None).unwrap();
    dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: ox, top: oy, right: ox + w as f32, bottom: oy + h as f32 }, radiusX: 8.0, radiusY: 8.0 }, &panel);
    let probe_b = dc.CreateSolidColorBrush(&D2D1_COLOR_F { r: PROBE.0 as f32 / 255.0, g: PROBE.1 as f32 / 255.0, b: PROBE.2 as f32 / 255.0, a: 1.0 }, None).unwrap();
    dc.FillRectangle(&D2D_RECT_F { left: ox + 20.0, top: oy + 20.0, right: ox + 100.0, bottom: oy + 100.0 }, &probe_b);
    surface.EndDraw().unwrap();
    visual.SetContent(&surface).unwrap();
    target.SetRoot(&visual).unwrap();
    dcomp.Commit().unwrap();
    println!("first commit at {:?}", t0.elapsed());

    pump_for(Duration::from_millis(700));
    let (px, py) = (sx + 60, sy + 60);
    let r1 = probe("1_attached", px, py);
    let ok1 = near(r1, PROBE);

    // Normal top-level window covering the probe.
    let cover = CreateWindowExW(
        WS_EX_TOOLWINDOW,
        w!("DeskboxesSpikeLayer"),
        w!("cover"),
        WS_POPUP | WS_VISIBLE,
        sx, sy, 200, 200,
        None, None, Some(hinst.into()), None,
    )
    .unwrap();
    let _ = SetWindowPos(cover, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
    pump_for(Duration::from_millis(500));
    let r2 = probe("2_covered", px, py);
    let ok2 = near(r2, (255, 0, 0));
    let _ = DestroyWindow(cover);
    pump_for(Duration::from_millis(300));

    let mut ok3 = None;
    if !no_wind {
        send_win_d();
        pump_for(Duration::from_millis(1500));
        let r3 = probe("3_after_win_d", px, py);
        ok3 = Some(near(r3, PROBE));
        println!("   our window visible={} parent={:?}", IsWindowVisible(hwnd).as_bool(), GetParent(hwnd).map(|p| p.0).ok());
        send_win_d(); // restore the user's windows
        pump_for(Duration::from_millis(1200));
    }

    println!("RESULT attached_visible={ok1} below_normal_windows={ok2} survives_win_d={ok3:?}");
    let _ = DestroyWindow(hwnd);
    pump_for(Duration::from_millis(200));
    dump_children(progman, "Progman (cleanup)");
    let _ = PCWSTR::null();
}
