//! HKCU reads (and the one HKCU write the product owns: its Run entry).

use crate::{pcw, wide};
use windows::Win32::System::Registry::*;

pub const ADVANCED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
pub const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

pub fn get_dword(key: &str, value: &str) -> Option<u32> {
    let (k, v) = (wide(key), wide(value));
    let mut d = 0u32;
    let mut sz = 4u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, pcw(&k), pcw(&v), RRF_RT_REG_DWORD, None, Some(&mut d as *mut _ as _), Some(&mut sz)) };
    r.is_ok().then_some(d)
}

pub fn get_string(key: &str, value: &str) -> Option<String> {
    let (k, v) = (wide(key), wide(value));
    let mut sz = 0u32;
    unsafe {
        if RegGetValueW(HKEY_CURRENT_USER, pcw(&k), pcw(&v), RRF_RT_REG_SZ, None, None, Some(&mut sz)).is_err() {
            return None;
        }
        let mut buf = vec![0u16; (sz as usize).div_ceil(2) + 1];
        RegGetValueW(HKEY_CURRENT_USER, pcw(&k), pcw(&v), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut sz)).ok().ok()?;
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]))
    }
}

pub fn get_binary(key: &str, value: &str) -> Option<Vec<u8>> {
    let (k, v) = (wide(key), wide(value));
    let mut sz = 0u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, pcw(&k), pcw(&v), RRF_RT_REG_BINARY, None, None, Some(&mut sz)).ok().ok()?;
        let mut buf = vec![0u8; sz as usize];
        RegGetValueW(HKEY_CURRENT_USER, pcw(&k), pcw(&v), RRF_RT_REG_BINARY, None, Some(buf.as_mut_ptr() as _), Some(&mut sz)).ok().ok()?;
        buf.truncate(sz as usize);
        Some(buf)
    }
}

pub fn set_string(key: &str, value: &str, data: &str) -> windows::core::Result<()> {
    let (k, v) = (wide(key), wide(value));
    let d: Vec<u16> = data.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, pcw(&k), pcw(&v), REG_SZ.0, Some(d.as_ptr() as _), (d.len() * 2) as u32).ok() }
}

pub fn delete_value(key: &str, value: &str) -> bool {
    let (k, v) = (wide(key), wide(value));
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, pcw(&k), pcw(&v)).is_ok() }
}

/// Windows app mode: true = light (`AppsUseLightTheme`, default light).
pub fn apps_use_light_theme() -> bool {
    get_dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme").map(|v| v != 0).unwrap_or(true)
}

/// Explorer's desktop icon size in pixels at 96 DPI (View → Large/Medium/Small; 48 by default).
pub fn desktop_icon_size() -> u32 {
    get_dword(r"Software\Microsoft\Windows\Shell\Bags\1\Desktop", "IconSize").filter(|v| (16..=256).contains(v)).unwrap_or(48)
}
