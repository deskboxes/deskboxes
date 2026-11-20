# Deskboxes — testing (Phase 1)

Two parts:
- **A. Automated checks.** The scripts the unattended run used. Re-run them after changes.
- **B. Manual checklist.** Numbered, so results can be reported back as "B7 ok, B12 fails: …".
  Section B.11 is for the end user's PC.

## Safety first (read before any real-desktop test)

- **Panic command:** `deskboxes --restore-icons` (in the folder with `deskboxes.exe`). It stops
  the watchdog and the core, and shows Explorer's icons. It prints `native icons visible=true`
  when done.
- **Without our files:** right-click the desktop → View → Show desktop icons. That is Windows' own
  setting, and Deskboxes only switches it.
- **Uninstall:** `deskboxes --uninstall`. It restores the icons and removes the autostart entry.
- **Turn off without uninstalling:** the desktop background menu → Deskboxes → Disable
  Deskboxes. It stays off across reboots (autostart does not bring it back). Starting Deskboxes
  from the Start menu, or setup's "Open Deskboxes now", turns it on again (`--enable`).
- **Layout and data live in:**
  - `%APPDATA%\Deskboxes\` for settings, layouts and backups
  - `%LOCALAPPDATA%\Deskboxes\` for the snapshot and logs in `logs\`
  - Deskboxes never writes into a Desktop folder.

Build first with `.\build.ps1 -Release`. Binaries:
- `target\release\deskboxes.exe`
- `target\release\deskboxes-watchdog.exe`
- `out\Release\shellhost\deskboxes-shellhost.exe`, plus the Windows App SDK files next to it

The core finds the host in `out\<Config>\shellhost\` when run from the repository.

---

## A. Automated checks (fixture mode, own windows only)

These scripts follow three rules:
- They run the app **non-elevated**, through `tools\run-as-user.exe`.
- They act only on generated fixtures in `%TEMP%`.
- They send input only to their own windows. `TestInput.Allowed` refuses any click or key that
  would land on another process's window.

| Script | What it checks | Result 2026-11-19 |
|---|---|---|
| `tools\core-check.ps1` | 1,000-item fixture with 2 boxes and a tab pair. Covers: click / Ctrl+click / rubber band / Ctrl+A / arrows; drag into a box, undo, drag on the grid; tab switch; context menu for `.txt` compared with Explorer's recorded menu; background menu; rename, drop onto a folder and delete through the shell; persistence across restarts | all pass |
| `tools\perf-check.ps1` | the §7 numbers, Release (REPORT.md §3) | see REPORT |
| `tools\watchdog-check.ps1` | crash → icons restored and restart; hang (suspended core) → detected and restarted; gives up after 3 restarts in 60 s; `--restore-icons` stops both | all pass |
| `tools\real-desktop-check.ps1` | **real desktop**, short: cold and warm start, each exiting by itself after 3 s; samples every ~20 ms for an empty desktop; restore verified by query | 0 empty-desktop samples; icons restored |
| `tools\menu-key-check.ps1` | regression check: menu opened with the Menu key, closed, then a right-click; the shell host must survive | pass, 3 of 3 rounds |
| `tools\menu-probe.ps1` | **diagnostic**: Windows' own menus read through UI Automation (Explorer window on a fixture, plus the real desktop with `-Desktop`) | — |
| `tools\menu-dump.ps1`, `tools\menu-view-check.ps1` | our menus (model dump; WinUI view) diffed against the probe | 23 of 24 identical |

Fixtures: `target\debug\fixture-gen.exe create <dir> 1000 [--public <dir2> --dups 10]`
creates them, and `... remove <dir>` deletes exactly what was created. The tool refuses Desktop
folders.

One-command fixture session (window mode, your own desktop untouched):

```powershell
.\target\debug\fixture-gen.exe create $env:TEMP\dbx-try 1000 --public $env:TEMP\dbx-try-public --dups 10
```

```powershell
.\target\release\deskboxes.exe --desktop-dir $env:TEMP\dbx-try --public-dir $env:TEMP\dbx-try-public --window --diagnostics
```

```powershell
.\target\debug\fixture-gen.exe remove $env:TEMP\dbx-try
```

---

## B. Manual checklist

Run on the real desktop unless noted. Start with
`target\release\deskboxes-watchdog.exe --launch`: this is the autostart path, and the watchdog
starts the core. To test autostart itself, run `deskboxes --install-autostart` once, and undo it
afterwards with `deskboxes --uninstall`.

Note what you see, and for timings whether it *felt* instant. The diagnostics overlay
(Ctrl+Shift+F12, or `--diagnostics`) shows frame times, item counts and startup phases.

### B.1 Start, first paint, hiding (§3 items 2–4)
1. First start (no snapshot yet): Explorer's icons stay until our icons are drawn, then switch.
   No empty desktop at any moment, and both sets never show at once for more than a blink.
2. Second start: the switch happens within about 0.1 s of launch.
3. Your existing icon arrangement was kept: positions imported from Explorer on the first run.
4. Win+D, then Win+D again: our icons stay visible on the desktop.
5. Peek (hover the far right of the taskbar, if enabled): our icons behave like Explorer's.
6. Virtual desktops (Win+Tab → new desktop): icons are visible on each. *UNVERIFIED.*

### B.2 Safety net (§3 item 3)
7. Task Manager → end `deskboxes.exe`. Within about a second Explorer's icons come back, and
   then Deskboxes restarts (the watchdog). Repeat 4 times within a minute: after the 4th, it
   stays stopped with Explorer's icons visible.
8. `deskboxes --restore-icons`: everything stops and Explorer's icons are visible.
9. Explorer restart: `taskkill /f /im explorer.exe`, then `start explorer.exe`. Our icons come
   back on the new desktop, Explorer's stay hidden, and no empty desktop remains.
10. Sign out and back in (autostart installed): no flash of Explorer's icons at logon.
11. Disable Deskboxes from the background menu: icons are restored, and it does not come back
    after sign-in.

### B.3 Icons and labels (§3 item 5)
12. Icons match Explorer's: type icons, shortcut arrows, program icons, folder icons, OneDrive
    status overlays.
13. Labels: two lines with an ellipsis; the full name when selected; Polish diacritics correct.
14. Light/dark switch (Settings → Personalization → Colors): box Mica tint and text follow.
    *Light theme is UNVERIFIED.*
15. Change the wallpaper: the box background (blurred wallpaper) follows.

### B.4 Selection and keyboard (§3 item 6)
16. Click, Ctrl+click, Shift+click, rubber band (also from inside a box), Ctrl+A.
17. Arrows, Home/End, Enter (opens), F2 (rename in place), Delete (to the Recycle Bin),
    Shift+Delete (Windows asks), Ctrl+Z / Ctrl+Y, Ctrl+C / X / V.
18. Type-to-select (typing a name's first letters). *Not implemented in Phase 1.*

### B.5 Drag and drop (§3 item 7)
19. Drag icons on the grid, into a box, out of a box, between boxes. Files are not moved:
    check in File Explorer.
20. Drag a file onto a folder icon: it moves into the folder. Onto a program: the program opens
    it. Onto the Recycle Bin: it is deleted.
21. Drag from File Explorer onto the desktop and into a box: the file is moved or copied as
    Windows would, and appears where it was dropped.
22. Drag from the desktop into File Explorer, an e-mail or a browser upload field.

### B.6 Context menus (§3 item 1, the most important)
23. Right-click a file, a folder, a shortcut, a picture, an `.exe`, several mixed items, and the
    desktop background. Compare each with Explorer's menu for the same item in a folder window:
    same entries, order, icons, accelerators and submenus.
24. Third-party entries on this PC: NanaZip, "Open in Terminal", the "App extensions" submenu
    and "Customize menu" (26H2).
25. "Show more options" / Shift+right-click: the classic menu with all shell extensions.
26. Menu key and Shift+F10 on a selection: the menu opens with access-key hints (KeyTips).
27. Run every entry once: Open, Open with, Copy as path, Compress to ZIP, Share, Properties,
    Cut/Copy/Paste, Rename, Delete, New → folder.
28. Speed: the menu opens as fast as Explorer's (REPORT §3 has the measured numbers).

### B.7 File operations (§3 item 8)
29. Create, rename and delete files in File Explorer while Deskboxes runs: the desktop
    updates within about 0.1 s.
30. Rename on the desktop (F2): the item keeps its place and its box.
31. OneDrive desktop: online-only files keep their cloud status and do not download.
    *UNVERIFIED: this PC's desktop is not on OneDrive.*

### B.8 Layout (§3 item 9)
32. Create a box (background menu → "Create box here"), rename it, move and resize it, roll it
    up (double-click the title), delete it (items go back to the desktop, files untouched).
33. Combine two boxes into tabs: Phase 1 has the model and the tab bar, and "Split tab" in the
    menu. Combining by drag is Phase 2.
34. Restart Deskboxes: boxes, tabs and positions are the same.
35. Change the display resolution or DPI: *UNVERIFIED in Phase 1* (re-layout needs a restart).
36. Monitor plug/unplug, sleep/resume: *UNVERIFIED.*

### B.9 Language (§3 item 10)
37. Polish Windows: Deskboxes' own entries ("Utwórz pole tutaj", "Wyłącz Deskboxes") are
    Polish, and Windows' entries are Windows' own Polish text.
38. English Windows, or `"language": "en"` in settings.json: English.

### B.10 Diagnostics to send back
39. On any menu difference, run these and send `docs\fidelity\` plus `%LOCALAPPDATA%\Deskboxes\logs\`:

```powershell
.\tools\menu-probe.ps1 -Desktop
```

```powershell
.\tools\menu-view-check.ps1
```

### B.11 The end user's PC (Polish Windows 11, about 779 desktop items)

Run B.1–B.9 there, plus:

40. Before the first start, note the Windows version (`winver`) and whether Settings →
    Personalization → Context menu exists. Without it (pre-26H2), menus must show **"Pokaż
    więcej opcji"** ("Show more options"), not "App extensions"/"Customize menu".
41. Kaspersky's entries ("Sprawdź…", "Sprawdź reputację w KSN" or similar) appear where
    Explorer shows them, and work.
42. "Open project in ChatGPT" (packaged extension) appears where Explorer shows it, and works.
43. "Pokaż więcej opcji" opens the classic menu with every extension, the same as Explorer's.
44. 779 items: the first start keeps the existing arrangement; the second start is instant;
    nothing is lost or duplicated (compare the item count with File Explorer).
45. Leave it running for a day of normal use; then send the logs folder.

### B.12 Round 4 additions (2026-11-19 night)

46. Title bar of a box with tabs: pointing at the magnifier opens it into a small "Search" field
    (text cursor); + and every tab (colored and open ones too) fade in a highlight; resting on
    search or + shows a tooltip, which a click hides.
47. + adds a tab that grows in. Right-click a tab: "Rename tab" and "Remove tab" at the top
    (not repeated in Box settings).
48. Move a box over loose icons: they step aside together to one side (the shorter way out),
    along their own rows or columns, in order; move it away and each goes back to its own cell.
49. A–Z box (Box settings › A–Z): see 57 (redesigned in round 5).
50. Background menu › Deskboxes › Privacy mode: icons become dashed outlines, names dashes, no
    name tooltips; the same entry turns it off. Settings › Privacy mode off removes
    the entry and turns privacy mode off.
51. Settings: switching pages slides the new page's cards in; the navigation marker glides.
52. Settings › Look › Little touches, each with a switch: smooth scrolling in boxes, a count
    next to the pointer when dragging several icons, a soft shadow under a box being moved,
    Ctrl + wheel stepping the icon size (small / medium / large).
53. Marquee from the wallpaper selects only loose icons; from inside a box, only that box's.
    Double-click a title: the box rolls up and stays up until the pointer leaves and returns.
54. Deleting an icon: it shrinks and fades out. Rolling a box out: its icons fade in with it.
    A box snapping to an edge glides the last few pixels.
55. Menus: Paste is in the top icon row of the background menu; New › Folder has the yellow
    folder icon; no "..." on entries; icons as soft as Explorer's.

### B.13 Round 5 additions (2026-11-20)

56. Settings navigation: General, Look (opens to Title bar and tabs / Box colors / Little
    touches), Boxes, a "Features" heading, then Tabs, Roll-up, Sorting rules, A–Z index, Search,
    Locks, Privacy mode. Each feature page starts with an accent tile and a description.
57. A–Z box: a letter row under the title; the current letter is bright with an accent line
    under it, which glides while scrolling; clicking or dragging along the row jumps. The first
    icon of each letter has a small letter at its top left. Icons use the box's full width.
    Settings › A–Z index: Letter bar / Letter tags switches, and an A–Z switch per box.
58. Settings › Search › Search field on the desktop off: the field is gone until Ctrl+F or
    typing; Type to search off: typing on the desktop does nothing.
59. Background menu › New: Folder, Shortcut and the file types with no divider, as in Windows.

### B.14 Round 6 additions (2026-11-20)

60. Privacy mode on: box and tab titles are dashes; an A–Z box's letter row is dots.
61. Settings › Look › Icons › Icon tint: icons in boxes take the color (the box's own, or a
    preset); Tint strength changes how much.
62. Blend into wallpaper on: icons become frosted glass (blurred wallpaper through their shape) and names fade; pointing at a
    box brings its icons back; with "Desktop icons too", desktop icons blend as well and come
    back one by one under the pointer.
63. Each box: "Color as all boxes" / None / a color, and "Blending as all boxes" / Blend in /
    Don't blend in, overriding the settings for all boxes.

### B.15 Round 7 additions (2026-11-20)

64. Box settings › Icon tint › a color: that box's icons take it; As all boxes follows Settings.
    Box settings › Blend into wallpaper › Off: that box stays solid while others blend.
65. Blend on, pointer away: the box background, border and title fade with the icons; pointing
    at the box brings it all back. Clicking the desktop (keyboard focus) brings every box back.
66. Look › Titles only on pointing: no title until the pointer is on the box. Scroll bars only
    on pointing: the scroll bar likewise.
67. Copy box style on one box, Paste box style on another: same color, transparency, blur, tint
    and blending. Paste is greyed out until something was copied.

### B.16 Round 8 additions (2026-11-20)

68. Settings › Hide all on: double-click empty desktop: boxes, icons and the search field fade
    away; double-click again (or type) brings them back. Off: double-click does nothing.
69. Box settings › Stay visible when hiding all: that box stays when everything else hides.
70. Deskboxes › Saved layouts › Save the layout now; change something (A–Z off); Saved layouts ›
    the time: the box is back as it was. Undo takes the restore back.
71. Settings › Saved layouts: Save adds a row; the bin deletes it; a daily backup's "Go back to
    this" restores that day's layout; "In the desktop menu" off removes the submenu.

### B.17 Installer (2026-11-20)

Build it with `.\build.ps1 -Installer` (Release, everything). The result is
`out\Release\DeskboxesSetup.exe` (about 21 MB): one file, no administrator rights, English or
Polish following Windows' display language (`--lang pl|en` to force one).

72. Run `DeskboxesSetup.exe` on a PC without Deskboxes. The window looks like Settings (Mica,
    the accent color, light or dark like Windows). Install → progress → "Deskboxes is ready";
    with "Open Deskboxes now" on, Finish starts it and the boxes appear.
73. It goes to `%LOCALAPPDATA%\Programs\Deskboxes` (Change… picks another folder; a
    `Deskboxes` folder is made inside it). Start menu › Deskboxes starts it; it starts again
    after signing out and in (switch "Start with Windows").
74. Run the installer again: the title says "Update Deskboxes" (newer version) or "Deskboxes is
    already installed" (same version). It closes the running Deskboxes, replaces the files and
    starts it again. Your boxes stay.
75. Settings › Apps › Installed apps › Deskboxes › Uninstall: the uninstaller window. With
    "Also remove my boxes and settings" off, `%APPDATA%\Deskboxes` stays. Afterwards the
    desktop shows the usual icons, the install folder is gone (a few seconds after Close), and
    Deskboxes is gone from Start and from Installed apps.
76. Deskboxes › Disable Deskboxes, then start it from Start: it is on again (`--enable`).
77. Quiet use (for scripts): `DeskboxesSetup.exe --quiet [--dir DIR] [--no-autostart]
    [--no-start-menu] [--launch]`; `uninstall.exe --uninstall --quiet [--remove-data]`. Exit
    codes 0 done, 1 failed, 2 Deskboxes could not be closed, 3 nothing to install. Log:
    `%TEMP%\deskboxes-setup.log`.

Tests here used a folder in the temp directory (`--dir`), a test data root (`--data-root`, so
"remove my boxes and settings" never touches the real `%APPDATA%\Deskboxes`) and
`DBX_SETUP_THEME=light|dark` to see both themes without changing Windows.

### B.18 Round 9 additions (2026-11-20)

78. Settings opens on Home: a picture banner, buttons (Sort the desktop now, Save the layout
    now, Sorting rules), a tile per feature with its picture; a tile's switch turns that
    feature on and off, the arrow opens its page. Each page shows a picture next to its text.
    Switch Windows to light mode (Settings › Personalization › Colors) and open Settings again:
    the pictures are the light versions (light wallpaper, light boxes).
79. Win+Shift+D: the boxes come over the open windows, which dim. Use a box (open an icon,
    right-click, rename, drag an icon onto a window). Esc, a click outside the boxes, or
    Alt+Tab puts them back behind the windows. Settings › Boxes above windows: pick another
    key or None; pick a corner and push the pointer into it; "Dim the windows" off.
80. Right-click a folder (not on the desktop itself, e.g. one inside Documents) › Show as a
    folder box, or Deskboxes › New folder box › Documents. The box lists the folder, folders
    first. Double-click a subfolder: it opens in the box and the title shows its name with a
    back arrow; the arrow or Backspace goes back.
81. Drag a desktop file onto the folder box: it moves into that folder (Ctrl: copied) and shows
    there. Dragging a file from the folder box onto the bare desktop is refused. Right-click the
    folder box's background: New makes the new item in that folder; Open in File Explorer.
    Delete the folder box: the folder and its files stay.
82. Settings › Look › Title bar and tabs: Title font, Title weight, Outline (and width) change
    every box at once; Faint / As desktop labels / Semibold are the look from before.
83. Title-bar buttons on: point at a box's title: a chevron and ⋯ appear left of the
    magnifier. The chevron rolls the box up (and opens it again); ⋯ opens the box's menu.

### B.19 Updates (2026-11-20)

84. Settings › General › Updates: "Update automatically" is on. In a build-folder copy the card
    says the copy doesn't update itself and Check now is greyed out.
85. In an installed copy: Check now → "Checking…", then "You have the newest version (X)" with
    the time. Offline: "No connection to GitHub…".
86. After a newer release is published: within two minutes of starting (or Check now) the card
    shows "Version Y is ready" and Install now. Install now: Deskboxes closes, comes back as Y
    within a few seconds, boxes unchanged. Or quit and start Deskboxes: it updates first.
87. Update automatically off: no daily check (Check now still works).

Testing without publishing: `DBX_UPDATE_FEED=http://127.0.0.1:PORT/latest.json` (plain http only
on 127.0.0.1/localhost) and `DBX_UPDATE_DELAY=SECONDS` (first check; default 120) in the
environment of an installed copy; serve `latest.json` and the installer with
`python -m http.server --bind 127.0.0.1`. Make the newer installer with the workspace version
raised in `Cargo.toml` (`tools\release.ps1 -Version X` without `-Publish` writes both files).
