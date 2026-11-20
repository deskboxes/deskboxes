//! fixture-gen: create and remove exactly N generated desktop items (test fixtures).
//!
//! ```text
//! fixture-gen create DIR N [--public DIR2 --dups K]
//!     N items in DIR (files of common types, folders, .lnk and .url shortcuts, Polish names with
//!     diacritics). With --public, K items in DIR2 get the same names as items in DIR (the user and
//!     Public desktops can hold the same name). Writes a manifest DIR.fixture.json next to DIR
//!     (never inside it) listing every path it created.
//! fixture-gen remove DIR
//!     Deletes exactly the paths in the manifest (and DIR / DIR2 if they are then empty).
//! ```
//! Refuses to create in a non-empty folder it did not create, and refuses Desktop folders.

use serde_json::json;
use std::path::{Path, PathBuf};
use windows::core::{Interface, HSTRING};
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;

const WORDS: &[&str] = &[
    "Zażółć gęślą jaźń", "Sprawozdanie roczne", "Faktura", "Zdjęcia z wakacji", "Łódź", "Kraków",
    "Święta", "Notatki", "Przepisy babci", "Rachunki", "Ćwiczenia", "Żółw", "Ważne dokumenty",
    "Umowa najmu", "Muzyka", "Podanie", "Wniosek", "Gry", "Budżet domowy", "Źródła", "Ogród",
    "Samochód", "Lekarz", "Szkoła", "Prezentacja", "Report", "Invoice", "Photos", "Notes",
];

/// (extension, bytes). Content only has to be plausible enough for the shell's type icons.
fn payload(ext: &str) -> Vec<u8> {
    match ext {
        ".png" => vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0, 5, 0, 1, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ],
        ".url" => b"[InternetShortcut]\r\nURL=https://example.com/\r\n".to_vec(),
        ".txt" => "Plik testowy Deskboxes — zażółć gęślą jaźń.\r\n".as_bytes().to_vec(),
        _ => b"deskboxes fixture".to_vec(),
    }
}

const EXTS: &[&str] = &[".txt", ".docx", ".xlsx", ".pdf", ".png", ".jpg", ".zip", ".mp3", ".pptx", ".csv", "", ".lnk", ".url"];

fn manifest_path(dir: &Path) -> PathBuf {
    let mut s = dir.as_os_str().to_owned();
    s.push(".fixture.json");
    PathBuf::from(s)
}

fn refuse_desktop(dir: &Path) {
    let l = dir.to_string_lossy().to_lowercase();
    let user_desktop = std::env::var("USERPROFILE").map(|u| format!("{}\\desktop", u.to_lowercase())).unwrap_or_default();
    let public = std::env::var("PUBLIC").map(|u| format!("{}\\desktop", u.to_lowercase())).unwrap_or_default();
    if l.trim_end_matches('\\') == user_desktop || l.trim_end_matches('\\') == public || l.contains("onedrive") && l.ends_with("desktop") {
        eprintln!("refusing to use a Desktop folder: {}", dir.display());
        std::process::exit(2);
    }
}

unsafe fn make_link(path: &Path, target: &str) -> windows::core::Result<()> {
    let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
    sl.SetPath(&HSTRING::from(target))?;
    let pf: IPersistFile = sl.cast()?;
    pf.Save(&HSTRING::from(path.as_os_str()), true)
}

fn create(dir: &Path, n: usize, public: Option<(&Path, usize)>) {
    refuse_desktop(dir);
    if let Some((p, _)) = public {
        refuse_desktop(p);
    }
    let m = manifest_path(dir);
    if dir.exists() && std::fs::read_dir(dir).map(|mut d| d.next().is_some()).unwrap_or(false) && !m.exists() {
        eprintln!("{} exists, is not empty and was not created by fixture-gen", dir.display());
        std::process::exit(2);
    }
    if m.exists() {
        remove(dir);
    }
    let mut created: Vec<String> = Vec::new();
    let mut dirs_made = vec![];
    for d in std::iter::once(dir).chain(public.map(|p| p.0)) {
        if !d.exists() {
            std::fs::create_dir_all(d).expect("create fixture dir");
            dirs_made.push(d.display().to_string());
        }
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let notepad = format!("{}\\System32\\notepad.exe", std::env::var("SystemRoot").unwrap_or("C:\\Windows".into()));
    let mut names = vec![];
    for i in 0..n {
        let ext = EXTS[i % EXTS.len()];
        let name = format!("{} {:04}{}", WORDS[i % WORDS.len()], i, ext);
        let p = dir.join(&name);
        let ok = match ext {
            "" => std::fs::create_dir(&p).is_ok(),
            ".lnk" => unsafe { make_link(&p, &notepad).is_ok() },
            _ => std::fs::write(&p, payload(ext)).is_ok(),
        };
        if ok {
            created.push(p.display().to_string());
            names.push(name);
        } else {
            eprintln!("could not create {}", p.display());
        }
    }
    if let Some((pd, k)) = public {
        for name in names.iter().filter(|n| !n.ends_with(".lnk")).take(k) {
            let p = pd.join(name);
            let ok = if Path::new(name).extension().is_none() { std::fs::create_dir(&p).is_ok() } else { std::fs::write(&p, payload(&format!(".{}", Path::new(name).extension().unwrap().to_string_lossy()))).is_ok() };
            if ok {
                created.push(p.display().to_string());
            }
        }
    }
    let v = json!({ "tool": "fixture-gen", "dir": dir.display().to_string(), "public": public.map(|p| p.0.display().to_string()), "created": created, "dirs": dirs_made });
    std::fs::write(&m, serde_json::to_string_pretty(&v).unwrap()).expect("write manifest");
    println!("created {} item(s); manifest {}", v["created"].as_array().map(|a| a.len()).unwrap_or(0), m.display());
}

fn remove(dir: &Path) {
    let m = manifest_path(dir);
    let Ok(text) = std::fs::read_to_string(&m) else {
        eprintln!("no manifest {}", m.display());
        std::process::exit(2);
    };
    let v: serde_json::Value = serde_json::from_str(&text).expect("manifest json");
    let mut removed = 0;
    let mut missing = 0;
    for p in v["created"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
        let path = Path::new(p);
        let r = if path.is_dir() { std::fs::remove_dir(path) } else { std::fs::remove_file(path) };
        match r {
            Ok(()) => removed += 1,
            Err(_) => missing += 1,
        }
    }
    for d in v["dirs"].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
        let _ = std::fs::remove_dir(d); // only if empty
    }
    let _ = std::fs::remove_file(&m);
    println!("removed {removed} item(s), {missing} already gone");
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(|s| s.as_str()) {
        Some("create") if a.len() >= 3 => {
            let dir = PathBuf::from(&a[1]);
            let n: usize = a[2].parse().expect("N");
            let pub_dir = a.iter().position(|x| x == "--public").map(|i| PathBuf::from(&a[i + 1]));
            let dups = a.iter().position(|x| x == "--dups").map(|i| a[i + 1].parse().unwrap_or(10)).unwrap_or(10);
            create(&dir, n, pub_dir.as_deref().map(|p| (p, dups)));
        }
        Some("remove") if a.len() >= 2 => remove(Path::new(&a[1])),
        _ => {
            eprintln!("usage: fixture-gen create DIR N [--public DIR2 --dups K] | remove DIR");
            std::process::exit(1);
        }
    }
}
