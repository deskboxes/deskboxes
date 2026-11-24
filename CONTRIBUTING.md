# Contributing to Deskboxes

Thanks for your interest in Deskboxes! Bug reports, ideas and pull requests are all welcome.

- **Found a bug?** Open a [bug report](https://github.com/deskboxes/deskboxes/issues/new?template=bug_report.yml).
- **Have an idea?** Open a [feature request](https://github.com/deskboxes/deskboxes/issues/new?template=feature_request.yml), or start a [discussion](https://github.com/deskboxes/deskboxes/discussions) if it's still taking shape.
- **Found a security issue?** Please follow [SECURITY.md](SECURITY.md) instead of opening an issue.

Before you start on a larger change, please open an issue or a discussion first so we can agree on the approach. That saves you from writing code that can't be merged.

## License and contribution terms

Deskboxes is source-available under the [PolyForm Strict License 1.0.0](LICENSE.md). That license lets you read, build and use the code for noncommercial purposes, but not distribute it or make changes to it.

**Permission to contribute.** As an addition to that license, you may fork this repository on GitHub and make changes in your fork, only to prepare pull requests to this repository.

**Terms for contributions.** By submitting a pull request, an issue with code in it, or any other contribution, you agree that:

1. The contribution is your own work, or you have the right to submit it.
2. You grant the project owner (GitHub user [ffuzze](https://github.com/ffuzze)) a perpetual, worldwide, non-exclusive, royalty-free, irrevocable license to use, copy, change, sublicense and distribute your contribution, as part of Deskboxes or otherwise, under any license and including commercially (for example in a paid Microsoft Store version).
3. You keep the copyright in your contribution.

## Building

**Prerequisites**

- Windows 11, 64-bit.
- Rust (stable, MSVC toolchain).
- Visual Studio 2022 Build Tools 17.14 with the **Desktop development with C++** workload and the **Universal Windows Platform build tools**. The WinUI 3 shell host needs the latter.
- Windows SDK 10.0.26100.
- Node.js and ffmpeg, only to render the Settings pictures and clips (`tools/art`).

Run these from the repository root in PowerShell:

```powershell
.\build.ps1                  # Debug build of the workspace and the shell host
.\build.ps1 -Release         # Release build
.\build.ps1 -Installer       # Release build and out\Release\DeskboxesSetup.exe
.\build.ps1 -Only shellhost  # only the C++/WinUI shell host
```

## Testing safely

Deskboxes replaces the icons on your real desktop, so develop against a **fixture**: a folder of test files shown in a normal window. Your own desktop is never touched.

```powershell
.\target\debug\deskboxes.exe --desktop-dir <fixture folder> --window
```

Run the app non-elevated. Before any test on the real desktop, read the safety section in [TESTING.md](TESTING.md). If your desktop is ever left without icons, this brings Windows' own icons back:

```powershell
.\target\debug\deskboxes.exe --restore-icons
```

Checks to run before a pull request:

```powershell
cargo test -p deskboxes-core --quiet          # model unit tests
.\tools\core-check.ps1                         # desktop interactions, against a generated fixture
.\tools\ipc-smoke.ps1                          # core <-> shell host pipe
.\tools\menu-view-check.ps1 -Only txt -Quiet   # context menu compared with Explorer's
```

The automated checks run against generated fixture folders in their own windows. They never touch the real desktop, and they leave a running copy of Deskboxes alone.

## Repository layout

| Path | Contents |
| --- | --- |
| `crates/core` | The model, with no Win32 code: layout, selection, sorting rules, undo, pages and persistence, with unit tests. |
| `crates/desktop` | `deskboxes.exe`: the desktop surface, input, drag and drop, drawing and app state. |
| `crates/render` | Direct3D, Direct2D, DirectWrite and DirectComposition drawing; icon bitmaps; wallpaper blur. |
| `crates/shell` | Explorer integration: enumeration, icons and thumbnails, file operations, folder watching, native icon hiding, autostart. |
| `crates/ipc` | The protocol between the core and the shell host. `shellhost/Ipc.cpp` is the C++ side. |
| `crates/i18n` | English and Polish strings. |
| `crates/watchdog` | `deskboxes-watchdog.exe`: starts the core, restarts it after a crash and restores icons if it can't. |
| `crates/setup` | The installer and uninstaller. |
| `crates/menuext` | The Explorer command that turns Deskboxes on from the desktop menu. |
| `shellhost/` | `deskboxes-shellhost.exe` (C++/WinRT, WinUI 3): context menus, dialogs and the Settings window. |
| `tools/` | Test scripts, fixture generator, Settings artwork renderer and release script. |
| `docs/` | [Architecture](docs/ARCHITECTURE.md), performance notes and visual fidelity references. |

[AGENTS.md](AGENTS.md) has a quick map of where each part of the app lives.

## Pull request guidelines

- Keep a pull request to one change. Small pull requests are reviewed faster.
- Match the style of the code around your change. Comments explain *why*, not *what*.
- User-facing text goes in `crates/i18n`, in English and Polish. If you don't speak Polish, add the English text and say so in the pull request.
- Every option you add needs a way to turn it off in Settings.
- Fill in the pull request template, including how you tested.

## Code of conduct

This project follows the [Code of Conduct](CODE_OF_CONDUCT.md). By taking part, you agree to follow it.
