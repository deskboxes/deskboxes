# Working on Deskboxes

## Find the implementation

| Area | Start here |
| --- | --- |
| Layout, selection, sorting, undo, persistence | `crates/core/src` contains the Win32-independent model. |
| Desktop input, drag and drop, drawing | `crates/desktop/src` owns the running app. Start with `wndproc` in `app.rs` for window messages, `draw.rs` for scene drawing, and `crates/render` for graphics primitives. |
| Explorer integration | `crates/shell` handles icons, file operations, folder watching, and native icon visibility. `crates/watchdog` handles crash recovery. |
| Menus, Settings, IPC | `shellhost` owns the C++/WinUI menus and Settings window. For wire changes, check both `crates/ipc/src/lib.rs` and `shellhost/Ipc.cpp`. `crates/menuext` implements the Explorer command to turn Deskboxes on. |
| Installation, updates, strings | `crates/setup` owns the installer, `crates/desktop/src/update.rs` the update client, and `crates/i18n` the Rust language tables. |

Search the relevant source directory first. For design or history, list headings with `rg -n '^## ' docs/ARCHITECTURE.md REPORT.md` and read the matching section. Architecture §15 records the Phase 1 implementation. Later rounds in the report supersede parts of it. Check current behavior in source.

## Verify a change

Run commands from the repository root in PowerShell.

- **Model changes:** `cargo test -p deskboxes-core --quiet`. For a desktop app compile check, use `cargo check -p deskboxes`.
- **Builds:** `.\build.ps1 -Only shellhost` builds the C++ host. `.\build.ps1` builds the full Debug workspace and host, including helpers used by fixture tests. Use `-Release` for performance checks or `-Installer` to build the installer.
- **Integration:** After a Debug build, use `.\tools\core-check.ps1` for desktop interactions, `.\tools\ipc-smoke.ps1` for the host pipe, or `.\tools\menu-view-check.ps1 -Only txt -Quiet` for the text-file menu. Use `TESTING.md` §A to select other checks and inspect the script's parameters for its expected build configuration.
- **Desktop safety:** Use generated fixtures in a temporary directory and run the app non-elevated with `--desktop-dir <fixture> --window` for interactive checks. Before a real-desktop or Explorer-restart test, read the safety section in `TESTING.md`. The recovery command is `deskboxes.exe --restore-icons`.
