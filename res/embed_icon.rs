// Shared by the build scripts of deskboxes.exe and deskboxes-watchdog.exe (`include!`d): compile
// res/deskboxes.rc (the app icon) with the Windows SDK's rc.exe and link the .res into the binary.
// Without an SDK the build goes on without the icon (a warning says so).

fn embed_icon() {
    use std::path::PathBuf;
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let res = manifest.join("..").join("..").join("res");
    println!("cargo:rerun-if-changed={}", res.join("deskboxes.rc").display());
    println!("cargo:rerun-if-changed={}", res.join("deskboxes.ico").display());
    let kits = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut vers: Vec<PathBuf> = std::fs::read_dir(&kits)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("x64").join("rc.exe").exists()).collect())
        .unwrap_or_default();
    vers.sort();
    let Some(rc) = vers.last().map(|v| v.join("x64").join("rc.exe")) else {
        println!("cargo:warning=rc.exe not found (Windows SDK): building without the app icon");
        return;
    };
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("deskboxes.res");
    let ok = std::process::Command::new(&rc)
        .arg("/nologo")
        .arg("/i")
        .arg(&res)
        .arg("/fo")
        .arg(&out)
        .arg(res.join("deskboxes.rc"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        println!("cargo:rustc-link-arg-bins={}", out.display());
    } else {
        println!("cargo:warning=rc.exe failed: building without the app icon");
    }
}
