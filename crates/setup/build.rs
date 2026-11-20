//! Build script of deskboxes-setup:
//! - compiles setup.rc (app icon + manifest: asInvoker, so Windows' installer detection never asks
//!   for administrator rights; per-monitor DPI v2) with the Windows SDK's rc.exe,
//! - links the C runtime's vcruntime part statically, so the setup exe runs on a PC without the
//!   Visual C++ Redistributable (the Universal CRT is part of Windows 10 and 11).

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let res = manifest.join("..").join("..").join("res");
    for f in ["setup.rc", "setup.manifest"] {
        println!("cargo:rerun-if-changed={}", manifest.join(f).display());
    }
    println!("cargo:rerun-if-changed={}", res.join("deskboxes.ico").display());
    println!("cargo:rerun-if-changed={}", res.join("deskboxes-256.png").display());

    // Static vcruntime, dynamic UCRT: the dynamic CRT's startup code (msvcrt.lib) stays, only the
    // vcruntime140.dll import library is swapped for the static one.
    for a in ["/NODEFAULTLIB:vcruntime.lib", "/NODEFAULTLIB:vcruntimed.lib", "/NODEFAULTLIB:libvcruntimed.lib", "/DEFAULTLIB:libvcruntime.lib", "/IGNORE:4098"] {
        println!("cargo:rustc-link-arg-bins={a}");
    }

    let kits = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut vers: Vec<PathBuf> = std::fs::read_dir(&kits)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("x64").join("rc.exe").exists()).collect())
        .unwrap_or_default();
    vers.sort();
    let Some(rc) = vers.last().map(|v| v.join("x64").join("rc.exe")) else {
        println!("cargo:warning=rc.exe not found (Windows SDK): building setup without icon and manifest");
        return;
    };
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("setup.res");
    let ok = std::process::Command::new(&rc)
        .arg("/nologo")
        .arg("/i")
        .arg(&res)
        .arg("/i")
        .arg(&manifest)
        .arg("/fo")
        .arg(&out)
        .arg(manifest.join("setup.rc"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        println!("cargo:rustc-link-arg-bins={}", out.display());
    } else {
        println!("cargo:warning=rc.exe failed: building setup without icon and manifest");
    }
}
