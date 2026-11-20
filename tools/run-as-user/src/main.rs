//! `run-as-user [--no-wait] [--out <file>] [--cwd <dir>] -- <program> [args...]`
//!
//! Test-harness tool. Claude Code runs this project's automated tests from an ELEVATED
//! terminal, but Deskboxes must be tested at normal user integrity (UIPI blocks drag
//! and drop between elevated windows and Explorer; shell behaviour differs too).
//!
//! Method: take the token of the shell process (the process owning GetShellWindow(),
//! i.e. the user's non-elevated explorer.exe), duplicate it as a primary token and
//! start the program with CreateProcessWithTokenW. Elevated administrators hold
//! SeImpersonatePrivilege, which that call requires. The child gets the same
//! medium-integrity, filtered-admin token as anything started from Explorer.
//!
//! stdout/stderr: CreateProcessWithTokenW goes through the Secondary Logon service and
//! does not inherit our handles, so with `--out <file>` the command is wrapped in
//! `cmd.exe /d /s /c "<cmd> > "<file>" 2>&1"`.
//!
//! Exit code: the child's exit code (unless --no-wait).

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Security::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};

fn quote(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '"']) {
        return a.to_string();
    }
    let mut s = String::from("\"");
    let mut bs = 0;
    for c in a.chars() {
        match c {
            '\\' => bs += 1,
            '"' => {
                s.push_str(&"\\".repeat(bs * 2 + 1));
                s.push('"');
                bs = 0;
            }
            _ => {
                s.push_str(&"\\".repeat(bs));
                s.push(c);
                bs = 0;
            }
        }
    }
    s.push_str(&"\\".repeat(bs * 2));
    s.push('"');
    s
}

fn find_process_in_session(name: &str) -> Option<u32> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    unsafe {
        let mut my_session = 0u32;
        ProcessIdToSessionId(GetCurrentProcessId(), &mut my_session).ok()?;
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = None;
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let n = String::from_utf16_lossy(&e.szExeFile[..e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0)]);
                let mut s = 0u32;
                if n.eq_ignore_ascii_case(name)
                    && ProcessIdToSessionId(e.th32ProcessID, &mut s).is_ok()
                    && s == my_session
                {
                    found = Some(e.th32ProcessID);
                    break;
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut wait = true;
    let mut out: Option<String> = None;
    let mut cwd: Option<String> = None;
    while !args.is_empty() && args[0] != "--" {
        match args[0].as_str() {
            "--no-wait" => {
                wait = false;
                args.remove(0);
            }
            "--out" => {
                out = Some(args[1].clone());
                args.drain(0..2);
            }
            "--cwd" => {
                cwd = Some(args[1].clone());
                args.drain(0..2);
            }
            other => {
                eprintln!("run-as-user: unknown option {other}");
                std::process::exit(2);
            }
        }
    }
    if args.first().map(|s| s.as_str()) == Some("--") {
        args.remove(0);
    }
    if args.is_empty() {
        eprintln!("usage: run-as-user [--no-wait] [--out file] [--cwd dir] -- program [args...]");
        std::process::exit(2);
    }
    let inner = args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ");
    let cmdline = match &out {
        Some(f) => format!("cmd.exe /d /s /c \"{inner} > \"{f}\" 2>&1\""),
        None => inner,
    };

    unsafe {
        let shell = GetShellWindow();
        let mut pid = 0u32;
        if shell.0.is_null() {
            // Explorer is not running (e.g. during an Explorer-restart test): borrow the
            // token of sihost.exe (Shell Infrastructure Host) in our session, which runs
            // non-elevated as the same user.
            match find_process_in_session("sihost.exe") {
                Some(p) => pid = p,
                None => {
                    eprintln!("run-as-user: no shell window and no sihost.exe in this session");
                    std::process::exit(3);
                }
            }
        } else {
            GetWindowThreadProcessId(shell, Some(&mut pid));
        }
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).expect("OpenProcess(shell)");
        let mut tok = HANDLE::default();
        OpenProcessToken(proc, TOKEN_DUPLICATE, &mut tok).expect("OpenProcessToken(shell)");
        let mut primary = HANDLE::default();
        DuplicateTokenEx(
            tok,
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT | TOKEN_ADJUST_SESSIONID,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        )
        .expect("DuplicateTokenEx");

        let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut pi = PROCESS_INFORMATION::default();
        let mut cl: Vec<u16> = cmdline.encode_utf16().chain(Some(0)).collect();
        let cwd_h = cwd.as_ref().map(|c| HSTRING::from(c.as_str()));
        let r = CreateProcessWithTokenW(
            primary,
            CREATE_PROCESS_LOGON_FLAGS(0),
            None,
            Some(PWSTR(cl.as_mut_ptr())),
            // CREATE_NO_WINDOW: the cmd.exe wrapper used for --out must not show a console
            // window (it would cover the desktop in screenshot-based tests).
            CREATE_UNICODE_ENVIRONMENT | if out.is_some() { CREATE_NO_WINDOW } else { PROCESS_CREATION_FLAGS(0) },
            None,
            cwd_h.as_ref().map(|h| windows::core::PCWSTR(h.as_ptr())).unwrap_or(windows::core::PCWSTR::null()),
            &si,
            &mut pi,
        );
        if let Err(e) = r {
            eprintln!("run-as-user: CreateProcessWithTokenW failed: {e}");
            std::process::exit(4);
        }
        println!("run-as-user: pid {}", pi.dwProcessId);
        let mut code = 0u32;
        if wait {
            WaitForSingleObject(pi.hProcess, INFINITE);
            let _ = GetExitCodeProcess(pi.hProcess, &mut code);
        }
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);
        let _ = CloseHandle(primary);
        let _ = CloseHandle(tok);
        let _ = CloseHandle(proc);
        std::process::exit(code as i32);
    }
}
