# Deskboxes

<p align="center">
<picture><source media="(prefers-color-scheme: light)" srcset="res/settings/home_l.jpg"><img src="res/settings/home.jpg" alt="Boxes with tabs on a Windows 11 desktop"></picture>
</p>

Deskboxes is a desktop organizer for Windows 11. It groups your desktop icons into boxes that you
can move, resize, roll up, tab and color, so the desktop stays tidy and things stay easy to find.

Deskboxes only arranges how icons are shown. Your files stay where they are, in the Desktop
folder, and Explorer, the Recycle Bin and every other program see them exactly as before.

Installers are published at [deskboxes/releases](https://github.com/deskboxes/releases/releases/latest).
Installed copies update themselves.

## Features

- **Boxes.** Group icons into boxes. You can move, resize and snap boxes to screen edges, and give each one its own color, transparency and background blur.
- **Tabs.** Keep several groups of icons in one box and switch between them from the title bar.
- **Roll-up.** Fold a box down to its title. It opens when you point at it, click it or double-click its title.
- **Sorting rules.** New icons go into the right box by themselves, by file type, name, date or where a shortcut leads. Games from Steam, Epic, GOG and other stores, and their launchers, are types of their own.
- **A–Z index.** Keep a box in name order with a letter bar for jumping to a letter.
- **Folder boxes.** Show a folder's files in a box, and open its subfolders right there.
- **Desktop pages.** Keep more desktops side by side, each with its own boxes and icons. Switch pages with Alt+Left and Alt+Right, a touchpad swipe or by dragging the desktop.
- **Boxes above windows.** Bring your boxes up above the open windows with a shortcut or a screen corner. No window gets minimized.
- **Search.** Start typing on the desktop, or press Ctrl+F, to find any icon, whether loose or in a box.
- **Hide all.** Double-click an empty spot to show only the wallpaper.
- **Icon looks.** Tint icons, or let them blend into the wallpaper until you point at them.
- **Saved layouts.** Save how your boxes are arranged and return to that later, or to how they were on any of the last seven days.
- **Locks.** Stop boxes and icons from being moved or resized by accident.
- **Privacy mode.** Hide icon names when someone is looking.
- **Undo.** Ctrl+Z undoes layout changes and file moves, deletes, copies and renames. Ctrl+Y redoes them.
- **Windows context menus.** Right-clicking an icon or the desktop shows Windows' own menu, including the commands other apps add to it, plus Deskboxes' own commands.
- **English and Polish**, following the Windows display language.

## A closer look

<table>
<tr><td width="50%"><picture><source media="(prefers-color-scheme: light)" srcset="res/settings/tabs_l.jpg"><img src="res/settings/tabs.jpg" alt="Tabs"></picture><br><b>Tabs</b><br>Several groups of icons in one box.</td><td width="50%"><picture><source media="(prefers-color-scheme: light)" srcset="res/settings/rollup_l.jpg"><img src="res/settings/rollup.jpg" alt="Roll-up"></picture><br><b>Roll-up</b><br>Fold a box down to its title; it opens when you point at it.</td></tr>
<tr><td width="50%"><picture><source media="(prefers-color-scheme: light)" srcset="res/settings/rules_l.jpg"><img src="res/settings/rules.jpg" alt="Sorting rules"></picture><br><b>Sorting rules</b><br>New icons go to the right box by themselves.</td><td width="50%"><picture><source media="(prefers-color-scheme: light)" srcset="res/settings/pages_l.jpg"><img src="res/settings/pages.jpg" alt="Desktop pages"></picture><br><b>Desktop pages</b><br>More desktops side by side, each with its own boxes.</td></tr>
<tr><td width="50%"><picture><source media="(prefers-color-scheme: light)" srcset="res/settings/above_l.jpg"><img src="res/settings/above.jpg" alt="Boxes above windows"></picture><br><b>Boxes above windows</b><br>Your boxes over the open windows, with a shortcut.</td><td width="50%"><picture><source media="(prefers-color-scheme: light)" srcset="res/settings/search_l.jpg"><img src="res/settings/search.jpg" alt="Search"></picture><br><b>Search</b><br>Start typing on the desktop to find any icon.</td></tr>
<tr><td width="50%"><picture><source media="(prefers-color-scheme: light)" srcset="res/settings/blend_l.jpg"><img src="res/settings/blend.jpg" alt="Blend into wallpaper"></picture><br><b>Blend into wallpaper</b><br>Icons take the wallpaper's colors until you point at them.</td><td width="50%"><picture><source media="(prefers-color-scheme: light)" srcset="res/settings/folder_l.jpg"><img src="res/settings/folder.jpg" alt="Folder boxes"></picture><br><b>Folder boxes</b><br>A folder's files in a box, with its subfolders right there.</td></tr>
</table>

Settings has a short clip of each feature.

## Requirements

- Windows 11, 64-bit.
- A GPU with Direct3D 11. Without one, Deskboxes uses Windows' software renderer.

## Install and uninstall

1. Download `DeskboxesSetup.exe` from the [latest release](https://github.com/deskboxes/releases/releases/latest).
2. Run it. It installs for the current user only, in `%LOCALAPPDATA%\Programs\Deskboxes`, and doesn't need administrator rights.

Deskboxes starts when you sign in. To turn it off, right-click the desktop › Deskboxes › Turn off. To turn it back on, right-click the desktop › Turn on Deskboxes.

To uninstall, go to Settings › Apps › Installed apps › Deskboxes. Windows' own desktop icons come back as they were.

If Deskboxes ever leaves the desktop without icons, this command brings Windows' own icons back:

```powershell
& "$env:LOCALAPPDATA\Programs\Deskboxes\deskboxes.exe" --restore-icons
```

## Data and privacy

| What | Where |
| --- | --- |
| Settings, box layouts, saved layouts | `%APPDATA%\Deskboxes` |
| Icon cache and logs | `%LOCALAPPDATA%\Deskboxes` |

Deskboxes makes one network request: a daily update check against this project's public releases
on GitHub. You can turn it off in Settings › Updates. Nothing about your desktop, files or usage
is sent anywhere.

## Building from source

**Prerequisites**

- Rust (stable, MSVC toolchain).
- Visual Studio 2022 Build Tools 17.14 with the C++ desktop workload and the Universal Windows Platform build tools. The WinUI 3 shell host needs the latter.
- Windows SDK 10.0.26100.
- Node.js and ffmpeg. You need these only to render the Settings pictures and clips (`tools/art`).

**Build**

Run these from the repository root in PowerShell:

```powershell
.\build.ps1                  # Debug build of the workspace and the shell host
.\build.ps1 -Release         # Release build
.\build.ps1 -Installer       # Release build and out\Release\DeskboxesSetup.exe
.\build.ps1 -Only shellhost  # only the C++/WinUI shell host
```

**Test**

```powershell
cargo test -p deskboxes-core --quiet          # model unit tests
.\tools\core-check.ps1                         # desktop interactions, run against a generated fixture
.\tools\ipc-smoke.ps1                          # core ↔ shell host pipe
.\tools\menu-view-check.ps1 -Only txt -Quiet   # context menu compared with Explorer's
```

The automated checks run against generated fixture folders in their own windows. They never touch
the real desktop, and they leave a running copy of Deskboxes alone. For interactive work, use a
fixture and run the app in a window:

```powershell
.\target\debug\deskboxes.exe --desktop-dir <fixture folder> --window
```

Before any test on the real desktop, read the safety section in [TESTING.md](TESTING.md).

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

## Third-party components

See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). No copyleft components are used.
