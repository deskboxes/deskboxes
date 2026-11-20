# Deskboxes — Phase 1 report

Date: 2026-11-19. Unattended run after Step 0, on the developer's PC.

**Labels.** VERIFIED means it was run and observed, on this PC, by the named check. UNVERIFIED
means it is implemented or claimed but was not run and observed. Every number below was measured
in this run; the raw data is in `docs/perf/perf-release.json` and `docs/fidelity/`.

Related documents:
- `docs/ARCHITECTURE.md`: the design; §15 lists where Phase 1 differs from it
- `TESTING.md`: the automated checks and the manual checklist (B.11 is for the end user's PC)
- `THIRD_PARTY_NOTICES.md`: licenses

## 1. Summary

The proof build works end to end on a 1,000-item fixture and, in short restored runs, on the real
desktop:
- the context menu is built at run time from this PC's own registrations and matches Windows'
  menu in 23 of 24 recorded cases
- first paint comes from a snapshot, then the native icons are hidden, with no empty desktop seen
- a watchdog restores the icons and restarts the core after a crash or hang
- layout is metadata only, with 2 boxes and a tab pair that persist across restarts

**Budgets missed:**
- A right-click on an item the pointer reached less than 150 ms before takes ~145 ms to show
  its menu; the target is 120 ms. After a normal pause (300 ms) it takes 54 ms.
- In window mode (the test harness only), a 1,000-item run uses ~110 MB private memory. In
  desktop mode, the real configuration, it uses 73 MB.

**Not verified on this PC at all** (they need a person, another machine, or events I could not
cause unattended): sleep/resume, lock, monitor hotplug, DPI change, virtual desktops, sign-out
(boot flash), OneDrive placeholders, Polish Windows, and the end user's third-party menu entries.

## 2. Status of the §3 items

| # | Item | Status | How it was checked | Notes |
|---|---|---|---|---|
| 1 | Modern context menu fidelity | **PARTIAL** | `menu-probe` reads Windows' own menus through UI Automation; `menu-dump` (model) and `menu-view-check` (our WinUI view, read the same way) diff ours against them | 23 of 24 cases identical in entries, order, accelerators, enabled state and submenus. That includes NanaZip (submenu), both "Open in Terminal" entries, "Open in Terminal (Administrator)" (folders), "App extensions" and "Customize menu". The difference is `folder_background` (separator placement, fixture folder window only; §4). Item heights and pitches are identical. Menu width differs in 4 cases (§4). Missed: speed target for 1 item without a pause. Light theme, 150% scaling, running every entry, and the Shift / "Show more options" classic menu: UNVERIFIED. |
| 2 | Desktop-layer surface | **PARTIAL** | Spike (b) on the real desktop with captures; the core uses the same attach | VERIFIED: a child of `Progman` above `SHELLDLL_DefView`, visible, covered by normal windows, still visible after Win+D. Not in Alt+Tab or the taskbar because it is a child window (by construction, not tested). Explorer restart re-attach (`TaskbarCreated`): implemented, UNVERIFIED. UNVERIFIED: Win+M, peek, Task View, virtual desktops, sleep/resume, lock/unlock, hotplug, DPI change (Phase 1 needs a restart for a new DPI). |
| 3 | Native icons hidden with no flash; watchdog; panic command | **PARTIAL** | `real-desktop-check.ps1` (real desktop, sampled every ~20 ms); `watchdog-check.ps1`; spike (c) | VERIFIED: 0 empty-desktop samples at cold and warm start; icons restored after every run; crash → icons back, then restart; hang detected in 7.0 s; gives up after 3 restarts in 60 s; `--restore-icons` stops both processes. Spike (c): 0 of 7,887 samples showed icons during an Explorer restart with icons hidden. UNVERIFIED: no flash at sign-in (needs a sign-out). |
| 4 | Instant first paint from a snapshot | **VERIFIED** | `perf-check.ps1`, desktop mode, 1,000 items, Release | Warm: first paint 124 ms after process start; the call that then hides the native icons takes 20 ms; reconcile done at 155 ms. |
| 5 | Real Windows icons | **PARTIAL** | `core-check.ps1` scene, captures in `docs/fidelity/core/` | System image-list icons with overlays and shortcut arrows, 2-line labels with an ellipsis, the full name when selected. Not implemented: thumbnails. Only 96 DPI was tested. OneDrive overlays UNVERIFIED (this desktop is not on OneDrive). |
| 6 | Selection | **PARTIAL** | `core-check.ps1`; rubber band over 2,000 items in `perf-check.ps1` | VERIFIED: click, Ctrl+click, Shift range, rubber band, Ctrl+A, arrow keys. Rubber band over 2,000 items: draw p95 3.3 ms, max 3.8 ms, against a 6.9 ms frame at 144 Hz. Frames are drawn per input event, so the interval follows the input rate (p50 14 ms). Dropped frames were not measured with DWM frame statistics. Not implemented: type-to-select. |
| 7 | Drag and drop | **PARTIAL** | `core-check.ps1` | VERIFIED: into a box, back onto the grid, undo; onto a folder item (the file moves through `IFileOperation`). Drags out to other applications and drops from Explorer are implemented (OLE) but UNVERIFIED: the harness only acts on its own windows. |
| 8 | Basic file operations | **PARTIAL** | `core-check.ps1` | VERIFIED: F2 rename in place (keeps identity and place), delete through the shell with no UI on a fixture, file changes on disk shown in ~110 ms. Enter (open), Cut/Copy/Paste, and Delete to the Recycle Bin with Windows' own prompts are implemented but UNVERIFIED: the unattended rules forbid dialogs. |
| 9 | Layout model (grid, 2 boxes, tab pair, persistence) | **VERIFIED** | `core-check.ps1` runs 1–3 | Loose grid with Explorer's spacing (75×98 px read from the desktop's view), two boxes, one tab pair (tab switch), persisted across two restarts. Layout is metadata only: files are never moved for layout. |
| 10 | Polish and English | **PARTIAL** | Unit strings; English observed on this PC | Our own strings exist in Polish and English, follow the Windows display language, and can be overridden with `"language"` in `settings.json`. Windows' own entries come from Windows, so they are in Windows' language. Polish UI on Polish Windows: UNVERIFIED (this PC is English). |

Unit tests: 11 pass (`cargo test -p deskboxes-core`): grid order, clamping, boxes vs grid cells,
combine/split/delete tabs, reconcile, snapping, undo/redo, persistence round trip, selection,
snapshot round trip.

## 3. Performance (§7)

**Machine:**
- Windows 11 Pro 26H2, build 26300.9974, English UI
- AMD Ryzen 7 9800X3D (8 cores / 16 threads), 30.9 GB RAM
- AMD Radeon RX 7900 XTX, 2560×1440 at 144 Hz, 96 DPI (100%)

**How it was measured:**
- Release build, `tools/perf-check.ps1`, non-elevated.
- Medians of 3–5 runs.
- Fixtures were generated in `%TEMP%`.
- "Desktop mode" is the real configuration: a child of the desktop, Explorer's icons hidden. It
  used a fixture folder through `--desktop-dir`, and Explorer's icons were checked visible after
  each run.

| Metric | Target | Measured | |
|---|---|---|---|
| Start → first paint, warm, 1,000 items | < 250 ms | desktop mode **124 ms**; window mode 174 ms | met |
| Full reconciliation, 1,000 items | < 1 s | cold (no snapshot, every item through the shell) **726 ms**; warm 155 ms | met |
| Idle CPU | ~0% | **0.0 ms CPU in 10 s** (core + shell host) | met |
| Private memory, 1,000 items | < 100 MB | desktop mode **73 MB**; window mode 111 MB (it keeps a full-size wallpaper copy) | met in the real configuration |
| Rubber band over 2,000 items | refresh rate, no dropped frames | draw p95 **3.3 ms**, max 3.8 ms, against a 6.9 ms frame | met for draw time; dropped frames not measured |
| Right-click → menu, 1 item | < 120 ms | **54 ms** after a 300 ms pause; **145 ms** when clicked 80 ms after arriving; 48 ms from button-up with a 100 ms press; Menu key 46 ms | met with a pause; **missed** without one |
| Right-click → menu, 100 items | < 250 ms | **151 ms** with no pause; Menu key 43 ms | met |
| File change → icon updated | < 200 ms | **110 ms** median (create 98 ms, rename 111 ms) | met |

**For comparison, Windows' own Explorer menu** (fixture folder window, Menu key): 64 ms for 1
item, 56 ms for 100 items. A right-click on Explorer's desktop could not be timed without
driving Explorer's own window, which the unattended rules exclude.

**2,000 items:**
- cold reconcile 1,320 ms
- desktop mode: warm first paint 150 ms, private memory 101 MB
- 1,660 of the 2,000 items fit on the 2560×1440 window's grid; the rest were scrolled out of view

**Where the right-click time goes** (shell host log, Release):
- Building one item's menu takes 50–80 ms:
  - parsing the item: ~2 ms
  - classic `IContextMenu` handlers: 34–57 ms
  - packaged `IExplorerCommand` handlers: 12–30 ms
- Drawing the WinUI menu takes ~45 ms.
- The core asks for the build at three points: when the pointer has rested on an item for 150 ms,
  at right-button-down, and 150 ms after the selection settles. A click that arrives after one of
  these builds only has to draw.
- A click within 150 ms of reaching the item pays build plus draw.
- Phase 2 plan: build the classic and packaged parts on two threads, and let slow handlers'
  entries arrive late, as Windows does (§7, item 1).

**Startup phases** are logged in `%LOCALAPPDATA%\Deskboxes\logs\deskboxes.log` and shown by
the diagnostics overlay (Ctrl+Shift+F12 or `--diagnostics`):
- process start
- graphics devices
- wallpaper
- snapshot loaded
- first paint
- icons hidden
- scan done
- host ready

**Memory work done in this run:**
- The blurred backdrop decodes the wallpaper at ¼ of its on-screen size. A full 2560×1440 decode
  had cost ~60 MB.
- Desktop mode keeps no full-size wallpaper copy.

## 4. Menu fidelity differences

Compared against Windows' own menus read through UI Automation on this PC, in dark theme at 100%
scaling:
- 11 desktop cases: a file of each type, a folder, a shortcut, a Public desktop shortcut, a
  multi-selection, a mixed selection, and the desktop background
- 13 fixture-folder cases

Data: `docs/fidelity/explorer/`, `docs/fidelity/deskboxes/`, `docs/fidelity/deskboxes-view/`.

| Difference | Where | Cause / plan |
|---|---|---|
| Separator placement in the folder-background menu | `folder_background` only: a fixture folder window, not the desktop | Explorer's folder-window background menu groups some entries differently. It does not affect the desktop. Phase 2, low priority. |
| Entries that exist only in Explorer folder windows ("Open in new tab", "Open in new window", …) | `folder`, `zip`, `mixed` (marked same*) | Correct: Windows' desktop menu does not show them either. Our desktop cases are identical to Windows' desktop menu. |
| Menu width: ours is wider by 102 px (`desktop_folder`), 31 px (`desktop_mixed`), 3 px (`desktop_png`, `png`) | width only; item heights, row pitch (65 px) and list pitch (32 / 41 px) are identical | Explorer's `desktop_folder` menu stays 251 px wide although "Open in Terminal (Administrator)" is longer. Most likely Explorer sizes the menu before late-arriving packaged entries and trims them. UNVERIFIED; investigate in Phase 2. |
| Light theme, 150% scaling | not compared | UNVERIFIED. The menu uses WinUI theme resources, so it should follow the theme; it was not observed. |
| Animation timing, font weight, padding at pixel level | not compared numerically | Captures for side-by-side review: `docs/fidelity/explorer/*.png` and `docs/fidelity/deskboxes-view/*.png`. |

## 5. Incidents and safety findings

1. **Shell host crash with a Windows "System Error" box (fixed).**
   - What happened: during a perf run, closing a menu opened with the Menu key crashed the host
     (0xc0000005). Windows showed "deskboxes-shellhost.exe - System Error" from csrss, which
     took the focus.
   - Consequence: the harness's next simulated clicks were not checked against the target window.
     Most landed on that box, and some may have gone elsewhere. This cannot be verified without a
     screenshot of the user's screen.
   - Fixes:
     - The crash itself: the KeyTips mode is left before the window closes, and the close is
       deferred to the dispatcher.
     - All three processes set `SetErrorMode` so Windows shows no crash boxes.
     - The host logs and exits through an unhandled-exception filter.
     - Every simulated click and key in the harness now checks that the target window belongs to
       a process under test (`TestInput.Allowed`), and refuses otherwise.
   - Regression check: `menu-key-check.ps1` passes.
   - **That System Error box may still be on the screen** (around 1088, 647). It cannot be
     closed programmatically; pressing OK is harmless.
2. **Some diagnostic `--dump-menu` runs of the shell host ran elevated.** They were started from
   the elevated tool shell rather than through `run-as-user`, while measuring how fast selections
   resolve. They only read menus, and their log lines say "elevated". The app under test, and
   every check in TESTING Part A, ran non-elevated.

## 6. Spikes (§9.2)

- **(a) Third-party modern entries from an unpackaged process: works.**
  - `spikes/explorer-command` activated packaged `IExplorerCommand` handlers (NanaZip, Terminal)
    from a non-elevated, unpackaged process.
  - It read title, icon, state, flags and subcommands, and invoked them.
  - This is the basis of the host's packaged-entry support. No per-extension code exists; any
    registered extension is handled the same way.
- **(b) Desktop layer on build 26300: works.**
  - `spikes/desktop-layer` creates a `WS_EX_NOREDIRECTIONBITMAP` child of `Progman` above
    `SHELLDLL_DefView`, rendered with D3D11 + D2D + DirectComposition.
  - Screen captures confirmed three things: it is visible, normal windows cover it, and it stays
    visible after Win+D (`docs/spikes/layer_*.png`).
  - On this build, `SHELLDLL_DefView` can sit under a `WorkerW` instead of `Progman`. The core
    looks in both places.
- **(c) Hiding native icons: `IFolderView2::SetCurrentFolderFlags(FWF_NOICONS)` was chosen.**
  - Three options were compared in `spikes/hide-icons`:
    - the folder-view flag
    - the registry `HideIcons` value only
    - the "Show desktop icons" command sent to the desktop view
  - The flag is idempotent, takes 13–44 ms, writes no file, and is what "View → Show desktop
    icons" itself changes. That makes it the user's own way back if Deskboxes is gone.
  - During an Explorer restart with icons hidden, `tools/explorer-restart-test.ps1` sampled the
    desktop list view about every 1 ms: 0 of 7,887 samples were visible.

## 7. Known risks

1. **Third-party handlers on the end user's PC** (Kaspersky, "Open project in ChatGPT", pre-26H2
   "Show more options"). The design is generic, but none of these were seen running. A slow or
   crashing handler crashes or delays only the shell host, not the desktop: the core restarts the
   host. A handler that hangs blocks menus until the host is restarted. Phase 1 has no per-handler
   time budget.
2. **Undocumented Windows behavior.**
   - The order and grouping of the modern menu, and its 26H2 preferences, were derived from
     observation. `menu_policy.json` isolates them, and `menu-probe` checks them on any machine.
   - The `Progman`/`WorkerW` layout can change in a Windows update.
3. **Pre-26H2 Windows on the end user's PC.** The menu policy has a pre-26H2 profile ("Show more
   options" instead of "App extensions"/"Customize menu"). It was never run on such a build.
4. **Explorer restart and session events** (sleep, lock, display changes) are handled only
   partly, and none was observed. Worst case: our icons are gone while Explorer's stay hidden.
   Mitigations:
   - the watchdog
   - `--restore-icons`
   - "View → Show desktop icons" in Windows' own menu
5. **Folder drop through the folder's own `IDropTarget`** returned success without moving
   anything when driven from our worker; the cause is unknown. Drops onto folders use
   `IFileOperation` instead (Windows' copy engine, same prompts).
6. **The Windows App SDK license** is not open source; it permits self-contained redistribution
   under conditions (see `THIRD_PARTY_NOTICES.md`).
7. **Memory in window mode** (harness only) is over 100 MB because of the full-size wallpaper
   copy. That does not affect the real configuration.
8. **Menu staleness.** A prefetched menu is reused at show time if the selection is the same,
   even if state that changes a menu changed in between (for example a "Pin to Start" state
   changed elsewhere). Phase 2: discard prefetches after a few seconds, or on clipboard and
   shell-change notifications.

## 8. Not implemented in Phase 1

These have data-model or architecture support (`docs/ARCHITECTURE.md`):
- settings window
- type-to-select
- UI Automation provider (accessibility)
- the core restarting the watchdog
- multi-monitor layouts
- DPI change without a restart
- cut ghosting
- thumbnails
- combining boxes into tabs by dragging (the model and "Split tab" exist)
- sorting rules
- auto roll-up
- scrollbars that auto-hide
- per-box icon size and tint
- daily layout backups
- tray icon

In fixture mode, a selection spanning the user and Public fixture folders gets no menu: the two
folders have no common parent. On the real desktop both live under the desktop root, so this
cannot happen there.

## 9. Phase 2 plan

Estimates are in developer-days of agent-assisted work, verified as in Phase 1. They are
estimates, not measurements.

| # | Work | Estimate |
|---|---|---|
| 1 | Menu speed and robustness: build the classic and packaged parts on two STA threads; a per-handler time budget with late insertion; discard prefetches when they go stale; menu width as Windows sizes it | 4–6 |
| 2 | End user's PC: run TESTING B.11 there with the developer; fix what differs (Kaspersky, ChatGPT entry, pre-26H2 profile, 779 items) | 3–5 |
| 3 | Desktop-layer robustness: Explorer restart re-attach, sleep/resume, lock, virtual desktops, DPI change, monitor hotplug, multi-monitor layouts keyed by topology | 6–9 |
| 4 | Settings window (WinUI 3, Mica, NavigationView, Polish/English) plus per-box color, opacity and blur; real Mica layers (luminosity, noise) checked against a WinUI window in light and dark | 6–9 |
| 5 | Tabs and boxes: combine by drag, Shift+drag tabs, tab styles and per-tab colors, auto roll-up at edges, hover reveal, auto-hiding scrollbars, per-box icon size and spacing | 7–10 |
| 6 | Sorting rules (type, name, shortcut target, date, size), a preview, ordering, automatic mode | 6–8 |
| 7 | Layout management: daily backups for 7 days, manual save/restore, per-monitor-configuration layouts | 2–3 |
| 8 | Input and accessibility: type-to-select, UI Automation provider, cut ghosting, thumbnails | 5–7 |
| 9 | Shell watching: `SHChangeNotifyRegister`, overlapped `ReadDirectoryChangesExW`, OneDrive placeholder tests on a OneDrive desktop | 3–4 |
| 10 | Safety and install: core and watchdog supervise each other; per-user installer and uninstaller (done, §16); code signing (SmartScreen) | 3–5 |
| | **Total** | **45–66** |

**Needs a decision from the developer:**
- The reference screenshots show "folder view" and search boxes.
- The spec lists "boxes that mirror other folders" as out of scope.
- Phase 2 above leaves them out until the developer decides.

## 10. Step 0 prerequisites (installed 2026-11-18, developer present)

| Component | Version |
|---|---|
| Visual Studio 2022 Build Tools | 17.14.45, MSVC 14.44.35207, workloads VCTools and UniversalBuildTools, component groups UWP.VC.BuildTools and WindowsAppDevelopment.VC.BuildTools |
| Windows SDK | 10.0.26100.0 |
| Rust | rustc 1.100.0, cargo 1.100.0, rustup 1.29.3, toolchain stable-x86_64-pc-windows-msvc |
| NuGet CLI | 8.0.0.137 |
| Git | 2.56.0.windows.4 |
| PowerShell | 7.7.0 |
| From NuGet at build time | Windows App SDK 2.7.0 (self-contained), C++/WinRT 3.0.260818.1, SDK BuildTools 10.0.26100.4654 |

**Repeat on another machine:** `setup.ps1` (elevated, with someone present), then the build:

```powershell
.\build.ps1 -Release
```

**Fixture in one command** (window mode; leaves the real desktop alone):

```powershell
.\target\debug\fixture-gen.exe create $env:TEMP\dbx-try 1000 --public $env:TEMP\dbx-try-public --dups 10
```

`TESTING.md` has the run and remove commands.

**Note found during Step 0:** the "Windows Store" C++ app type for x64 comes only with the
`UniversalBuildTools` workload. Without it, WinUI C++ builds fail with "OutputPath is not set".

## 11. Round 4 (2026-11-19 night, unattended)

Everything below is VERIFIED by looking at it on this PC (screenshots of the real desktop, or of
the fixture window where a drag or a test box was needed), unless marked otherwise.
`TESTING.md` B.12 lists the checks.

**Asked for, done:**
- Memory: the shell host leaked every menu (handler reference cycles), about 850 MB after a
  day of use. Closed menus are now freed and idle memory is trimmed. Measured at the end of the
  night: core ~85 MB working set, ~105 MB private; shell host ~45 MB private before any Settings
  window, ~190 MB private after Settings and many menus (WinUI stays loaded), its working set
  trimmed to ~1 MB a few seconds after a menu or window closes. No feature was cut for this. The second pair of processes seen on the user's screen was my test fixture.
- Menu icons are rasterized at 4x and anti-aliased, as soft as Explorer's; New › Folder shows the
  real folder icon; Paste is in the background menu's top row; no "..." on entries.
- Marquee scope (wallpaper → loose icons only; a box → that box only). Double-click roll-up stays
  up until the pointer leaves and returns.
- Animations: icons fade in while a box rolls out; deleted icons shrink and fade; a box snapping to
  an edge glides; Settings pages slide in; new tabs grow in. The snap glide is too short (90 ms)
  to catch in screenshots: UNVERIFIED by eye, the code path runs.
- Title buttons: hover highlight (fading), the search button opens into a "Search" field with a
  text cursor, tooltips on search and +. Tabs: hover on every tab, colored and open ones too.
  Right-click a tab: Rename / Remove at the top.
- Icons stepping aside for a box move as one group, to the side that is the shortest way out, each
  along its row or column, in order (`layout::step_aside`, unit test
  `covered_icons_step_aside_together_in_order`). Remembered cells are kept.
- A–Z index redesigned: slim rail, accent dot on the current letter (sliding), letters swell under
  the pointer, bubble beside the rail. Fixed: a name starting with "#" put a second "H" on the
  index.
- Privacy mode (our own design): background menu › Deskboxes › Privacy mode. Icons are drawn as
  dashed outlines and names as dashes (also in the drag image); name tooltips are off. Box and tab
  titles stay readable. Settings › General › Privacy mode turns the feature off entirely.

**Added without being asked (each can be turned off in Settings › Look › Little touches):**
smooth scrolling in boxes, a count badge when dragging several icons, a soft shadow under a box
being moved, Ctrl + wheel for icon size (as on Explorer's desktop).

**Incidents:**
- Two fixture files (`Ugli.txt`, `Kale.txt`) were dropped into the Desktop folder by a test drag
  that ended outside the fixture window. They were moved back the same minute. `desk-input.ps1`
  now refuses a press and release in different processes.
- A later test meant for the fixture reached the real desktop (the fixture had been stopped).
  It only drew a selection rectangle; nothing moved. `desk-input.ps1 -Only PID` now refuses input
  outside the test window.

**Test leftovers removed:** the test box (tabs "New box", "docs", "New tab") and its two sorting
rules; the user's document is back as a loose icon. The user's own boxes were not touched.

## 12. Round 5 (2026-11-20)

VERIFIED on this PC by screenshots (real desktop for Settings and menus, the fixture window for
the A–Z box). `TESTING.md` B.13 lists the checks.

- Settings switches sit at the card's right edge with "On"/"Off" on their left, as in Windows
  Settings.
- Settings is laid out per feature: General; Look (opens to Title bar and tabs, Box colors,
  Little touches); Boxes; then a "Features" heading with one page each for Tabs, Roll-up,
  Sorting rules, A–Z index, Search, Locks and Privacy mode. Each feature page starts with an
  accent tile and what the feature does and how to reach it.
- A–Z redesigned as the user picked (letter bar + letter tags, no pills): a row of the box's
  first letters under its title (the current one bright with a short accent line that glides;
  click or drag along the row to jump) and a small dim letter beside the first icon of each
  letter. The icon grid keeps the box's full width, so columns stay even. A–Z page: switches
  for the bar and the tags, and an A–Z switch per box.
- New switches (added without being asked, each off-able): Search › "Search field on the
  desktop" (off: the field shows only while searching) and "Type to search".
- Context submenus: the gap on the left of the Deskboxes and Box settings submenus is the check
  column WinUI keeps when a submenu has switches (Lock everything, Privacy mode). Explorer's own
  menus do the same (File Explorer › View and View › Show: unchecked items with icons keep the
  column), so it stays. Found while comparing: our New submenu had a divider after Shortcut that
  Windows' New submenu does not have (docs/fidelity/explorer/*.New.png); removed.
- `desk-input.ps1` allows every running core (the real one and the fixture), so the real
  desktop can be tested while the fixture runs.

## 13. Round 6 (2026-11-20)

VERIFIED by screenshots (fixture window for the icons and privacy mode, real desktop for Settings).

- Privacy mode also hides box and tab titles (a dash per word) and turns the A–Z letter row into
  dots, so no first letters show.
- Icon tint (Settings › Look › Icons): None, the box's own color, or a preset color, with a
  strength; icons keep their light and shade (a color matrix on the GPU).
- Blend into wallpaper (our name; the user asked for the feature by a trademarked name, which
  stays out of the product): icons turn into frosted glass (the blurred wallpaper seen through
  their shape, a little lighter, with a faint trace of the icon; reworked after the user found
  the first, tinted version too opaque) and labels fade; pointing at a box brings its icons back (fading), pointing at a desktop icon brings that
  icon back. Strength slider. "Desktop icons too" applies tint and blend to icons outside boxes.
- Each box can follow the setting for all boxes or have its own tint and blending (Settings ›
  Look › Icons › Each box), and from Box settings › Icon tint / Blend into wallpaper (round 7).

## 14. Round 7 (2026-11-20, unattended)

VERIFIED by screenshots on the fixture window. Everything here except the box-menu entries was
added without being asked, so each has a switch (Settings) to turn it off; all default to the
earlier behaviour except where noted.

- Box settings (right-click menu) › Icon tint (As all boxes / None / Box color / presets) and
  Blend into wallpaper (As all boxes / On / Off): the per-box choices, as the user asked.
- Blend into wallpaper also fades the box itself (background, border and title) by the same
  amount, so a blended box sits in the wallpaper (Settings › Look › Icons › "Boxes blend in
  too", on). "Full color on pointing" and "Full color while the desktop is in use" (keyboard
  focus on the desktop) bring everything back; both on.
- Look › "Titles only on pointing" and "Scroll bars only on pointing" (both off by default):
  the title fades in when the pointer is on the box; rolled-up, renamed or searched boxes keep
  their title.
- Box settings › Copy box style / Paste box style: color, transparency, blur, title color, icon
  tint and blending from one box onto another (tabbed boxes too).
- `desk-input.ps1 -Only PID` also accepts our menu host, so menus opened from the fixture can
  be clicked.

## 15. Round 8 (2026-11-20, unattended): ideas from the organizer docs, under our own names

VERIFIED by screenshots on the fixture window (menus and Settings opened from it).

- Hide all (Settings › Hide all; off by default): double-clicking the bare desktop fades the
  boxes, desktop icons and search field away (0.25 s), leaving the wallpaper; double-clicking
  again, or typing to search, brings them back. Hidden things let clicks through. A box can stay
  visible (Box settings › Stay visible when hiding all, shown only when the feature is on, and
  per box on the Settings page). Not remembered across restarts, so a restart never leaves an
  empty-looking desktop.
- Saved layouts (Settings › Saved layouts; Deskboxes › Saved layouts in the desktop menu, which
  a switch removes): save the layout now, go back to a saved one or to one of the daily backups
  (backups/YYYY-MM-DD, kept 7 days, existed already), delete a saved one. Files go to
  `%APPDATA%\Deskboxes\saved\<screen setup>\YYYYMMDDHHMMSS.json`; restoring replaces only our
  layout (icons on the desktop now that the saved layout doesn't know are placed as new ones)
  and is one Undo step. Never touches a Desktop folder.
- Not done: a hotkey that shows the boxes above windows. Our surface is a child of the desktop
  window (§3 of ARCHITECTURE), so it cannot be raised alone; it would need a second, top-level
  window drawing the boxes.

## 16. Installer (2026-11-20, unattended)

`crates/setup` → `deskboxes-setup.exe`, packed by `.\build.ps1 -Installer` into
`out\Release\DeskboxesSetup.exe`: 21.0 MB, a 0.76 MB program plus the app (244 files,
62.1 MB) compressed with Windows' own LZMS (cabinet.dll). This starts on §9 item 10; code
signing is still open (an unsigned download gets a SmartScreen warning).

- Per user, no administrator rights (`asInvoker`): `%LOCALAPPDATA%\Programs\Deskboxes`, HKCU
  only (Installed apps entry, `Run`, a Start menu shortcut). Ships the app-local VC++ runtime
  so a fresh PC needs nothing else; setup itself links the runtime statically.
- The window is drawn like the Settings window: Mica, Fluent colors and controls (cards,
  switches, accent and standard buttons, focus rings), Segoe UI Variable, the Windows accent,
  light and dark, entrance and hover animations (off when Windows animations are off),
  keyboard (Tab, Space, Enter, Esc). English and Polish.
- Updating closes the running Deskboxes (its windows first, then the processes running from the
  install folder), restores the native icons, replaces the files (files in use are renamed
  aside and removed later), removes files the new version no longer has, and starts it again.
- `uninstall.exe` is the same program without the app; it closes Deskboxes, restores the
  icons, removes what setup added (the `Run` value and the shortcut only if they are ours),
  optionally the user's boxes and settings, and deletes itself.
- New core flag `--enable`: starting Deskboxes from Start or from setup turns it back on after
  "Disable Deskboxes" (which otherwise lasts across restarts).

VERIFIED (non-elevated, into a temp folder): quiet install 0.8 s with every file
hash-identical to the build; the Installed apps entry, `Run` and the shortcut; the GUI pages
(welcome, progress, done, update/reinstall titles, error with "Try again", uninstall confirm,
removed); Finish starts the installed copy (core, watchdog, shell host from the install
folder, icons hidden, `--enable` logged); GUI uninstall of the running copy (icons back,
folder, key, `Run` and shortcut gone); `--remove-data` against a test data root; Polish and the
light theme (screenshots); Tab/Space/Esc.

UNVERIFIED: a real install into `%LOCALAPPDATA%\Programs` (tests used a temp folder so this
PC's own setup was not disturbed), a PC without Visual Studio, the end user's Polish PC,
SmartScreen on a downloaded copy, 150% scaling.

## 17. Round 9 (2026-11-20, unattended): Settings with pictures, boxes above windows, folder boxes, more looks

VERIFIED by screenshots on the showcase test desktop (`%TEMP%\dbx-show`, never the user's
files); menus and Settings were opened from it.

- Settings redesign: a Home page (a banner made from a real screenshot, quick actions, a tile
  per feature with a close-up picture and its switch, the keyboard shortcuts) and a picture
  beside each page's description. The pictures (`res/settings/*.jpg`) were shot from the
  showcase test desktop, so they show this PC's wallpaper.
- Boxes above windows (Settings › Boxes above windows): a hotkey (Win+Shift+D by default; three
  others to pick from, or none; Settings warns when another program has it) or a screen corner
  (off by default) brings the boxes over the open windows, with the windows dimmed (a switch).
  Esc, a click outside the boxes, or switching to another window puts them back. It is a second,
  top-level window the box surface is moved into while it is up (it closes §15's "not done").
  Right-click menus, renaming, dragging items out to other windows and drops all work there.
- Folder boxes (Settings › Folder boxes; a folder's right-click menu › Show as a folder box;
  Deskboxes › New folder box › Documents/Downloads/Pictures/Music/Videos): a box showing a
  folder's files, folders first then by name (2 before 10). A folder in it opens right there
  (double-click or Enter); the arrow in the title (or Backspace) goes back, never above the
  box's own folder. Dropping a file on it moves it into that folder (Ctrl copies); the folder
  is watched, so changes show at once. Its files can't be dropped loose on the desktop, and it
  takes no rules or tabs. Desktop folders, cloud-only folders and network folders are refused;
  nothing is ever downloaded to list a folder. Deleting the box leaves the folder alone.
- More box looks (Settings › Look › Title bar and tabs, all defaulting to the look before):
  title font (as desktop labels, or one of six), title weight (light, regular, semibold, bold),
  outline (faint as before, none, accent color, the box's color, white, black) and its width,
  and title-bar buttons: roll-up and ⋯ (the box's menu) in a box's title while the pointer is
  on it (off by default).
- Not done: a title at the bottom of a box. About 40 places assume the title is on top
  (hit-testing, roll-up, tabs, snapping); worth doing on its own if wanted.

UNVERIFIED: the corner trigger with a second monitor, boxes above windows over a full-screen
game (it doesn't come up while a full-screen program has the screen only if Windows refuses
the foreground), folder boxes on a OneDrive-backed Documents with files not on this PC (they
are skipped by attribute, tested only with local folders).

Later the same day: the Settings pictures are drawn now, not screenshots. `tools/art/art.html` lays out
each scene (big frosted boxes, icons drawn for it, the pointer, keycaps) on the wallpaper the user
chose (light and dark versions); `tools/art/render.ps1` renders them with headless Chrome at twice
the size and `tools/art/export.py` writes `res/settings`: each picture in a dark and a light
version (`NAME.jpg`, `NAME_l.jpg`), and Settings shows the one matching its theme. The Styles page
got a picture too. VERIFIED in the dark theme on the test desktop; the light theme's pictures are
checked as images only (this PC runs dark).
