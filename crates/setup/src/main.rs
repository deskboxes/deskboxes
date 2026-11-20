//! deskboxes-setup: installs Deskboxes for the current user (no administrator rights), and is its
//! uninstaller (`uninstall.exe` in the install folder is this program without the payload).
//!
//! ```text
//! DeskboxesSetup.exe                       the installer window
//! DeskboxesSetup.exe --quiet               install with the defaults, no window
//!     [--dir DIR] [--no-autostart] [--no-start-menu] [--launch]
//! uninstall.exe --uninstall                the uninstaller window (Settings › Apps runs this)
//! uninstall.exe --uninstall --quiet [--remove-data]
//!     [--lang en|pl]                       language (default: Windows' display language)
//!     [--data-root DIR]                    tests: data folders are DIR\Roaming\Deskboxes, DIR\Local\Deskboxes
//! deskboxes-setup.exe --pack DIR OUT.exe   build the installer: this program + every file under DIR
//!     [--app-version X]
//! DeskboxesSetup.exe --list                print what the installer carries
//! ```
//! Exit codes: 0 done, 1 failed, 2 Deskboxes could not be closed, 3 nothing to install.
//! Log: %TEMP%\deskboxes-setup.log.

#![windows_subsystem = "windows"]

mod gfx;
mod ops;
mod payload;
mod text;
mod theme;
mod ui;

use std::path::PathBuf;

#[derive(Default, Debug)]
struct Args {
    uninstall: bool,
    quiet: bool,
    dir: Option<PathBuf>,
    no_autostart: bool,
    no_start_menu: bool,
    launch: bool,
    remove_data: bool,
    data_root: Option<PathBuf>,
    lang: Option<String>,
    pack: Option<(PathBuf, PathBuf)>,
    app_version: Option<String>,
    list: bool,
}

fn parse() -> Args {
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut a = Args::default();
    let mut i = 0;
    while i < v.len() {
        let next = |i: &mut usize| {
            *i += 1;
            v.get(*i).cloned().unwrap_or_default()
        };
        match v[i].to_ascii_lowercase().as_str() {
            "--uninstall" | "/uninstall" => a.uninstall = true,
            "--quiet" | "/quiet" | "/s" | "--silent" => a.quiet = true,
            "--dir" => a.dir = Some(next(&mut i).into()),
            "--no-autostart" => a.no_autostart = true,
            "--no-start-menu" => a.no_start_menu = true,
            "--launch" => a.launch = true,
            "--remove-data" => a.remove_data = true,
            "--data-root" => a.data_root = Some(next(&mut i).into()),
            "--lang" => a.lang = Some(next(&mut i)),
            "--pack" => {
                let d = next(&mut i);
                let o = next(&mut i);
                a.pack = Some((d.into(), o.into()));
            }
            "--app-version" => a.app_version = Some(next(&mut i)),
            "--list" => a.list = true,
            other => ops::log(&format!("unknown argument {other}")),
        }
        i += 1;
    }
    a
}

/// Print to the console that started us, if any (we are a GUI program).
fn console() {
    use windows::Win32::System::Console::*;
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn main() {
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::*;
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    }
    let args = parse();
    let me = std::env::current_exe().unwrap_or_default();
    ops::log(&format!("deskboxes-setup {} {:?} elevated={}", env!("CARGO_PKG_VERSION"), std::env::args().skip(1).collect::<Vec<_>>(), ops::token_elevated()));

    if let Some((dir, out)) = &args.pack {
        console();
        let ver = args.app_version.clone().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
        match payload::pack(dir, out, &ver) {
            Ok((n, bytes, packed)) => {
                println!("packed {n} files, {:.1} MB -> {:.1} MB payload: {}", bytes as f64 / 1048576.0, packed as f64 / 1048576.0, out.display());
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("pack failed: {e}");
                std::process::exit(1);
            }
        }
    }

    let payload = match payload::Payload::open(&me) {
        Ok(p) => p,
        Err(e) => {
            ops::log(&format!("cannot read the payload: {e}"));
            None
        }
    };
    if args.list {
        console();
        match &payload {
            Some(p) => {
                println!("Deskboxes {} - {} files, {} bytes, {} chunks", p.header.app_version, p.header.files.len(), p.header.total(), p.header.chunks.len());
                for f in &p.header.files {
                    println!("{:>10}  {}", f.size, f.path);
                }
            }
            None => println!("no payload"),
        }
        std::process::exit(0);
    }

    let lang = text::Lang::detect(args.lang.as_deref());
    // Without files to install, inside an install folder (uninstall.exe), we are the uninstaller.
    let installed = ops::installed();
    let in_install_dir = installed.as_ref().map(|i| me.starts_with(&i.dir)).unwrap_or(false);
    let uninstall = args.uninstall || (payload.is_none() && in_install_dir);

    if args.quiet {
        deskboxes_shell::com_sta();
        let report = |r: ops::Report| {
            if let ops::Report::Step(s) = r {
                ops::log(&format!("step {s:?}"));
            }
        };
        let code = if uninstall {
            let dir = args.dir.clone().or_else(|| installed.as_ref().map(|i| i.dir.clone()));
            match dir {
                None => {
                    ops::log("nothing to uninstall");
                    3
                }
                Some(dir) => {
                    let o = ops::UninstallOptions { dir: dir.clone(), remove_data: args.remove_data, data_root: args.data_root.clone() };
                    let r = ops::uninstall(&o, &report);
                    ops::schedule_self_delete(&dir);
                    exit_code(r.map(|_| ()))
                }
            }
        } else {
            match &payload {
                None => {
                    ops::log("no payload: nothing to install");
                    3
                }
                Some(p) => {
                    let dir = args.dir.clone().or_else(|| installed.as_ref().map(|i| i.dir.clone())).unwrap_or_else(ops::default_dir);
                    let o = ops::InstallOptions {
                        dir,
                        autostart: !args.no_autostart,
                        start_menu: !args.no_start_menu,
                        launch: args.launch,
                        shortcut_desc: text::t(lang, text::T::ShortcutDesc).into(),
                    };
                    exit_code(ops::install(p, &o, &report).map(|_| ()))
                }
            }
        };
        ops::log(&format!("quiet: exit {code}"));
        std::process::exit(code);
    }

    let code = ui::run(ui::Init {
        lang,
        mode: if uninstall { ui::Mode::Uninstall } else { ui::Mode::Install },
        payload,
        dir: args.dir.clone(),
        data_root: args.data_root.clone(),
    });
    std::process::exit(code);
}

fn exit_code(r: Result<(), ops::Failure>) -> i32 {
    match r {
        Ok(()) => 0,
        Err(f) => {
            ops::log(&format!("failed: {f:?}"));
            match f {
                ops::Failure::StillRunning => 2,
                ops::Failure::NoPayload => 3,
                _ => 1,
            }
        }
    }
}
