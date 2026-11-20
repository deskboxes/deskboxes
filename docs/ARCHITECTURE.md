# Deskboxes — Architecture

Status: Phase 1 (proof build). Verified facts are marked **[verified: …]**; everything else is design or
**UNVERIFIED**. Measured results live in `REPORT.md`.

Dev/test machine: Windows 11 Pro 26H2, build **26300.9974**, 2560×1440 primary display.

---

## 1. Processes

| Process | Language | Role |
|---|---|---|
| `deskboxes.exe` | Rust | Core: model, desktop surfaces, rendering, enumeration, watchers, icons, input, selection, drag and drop, persistence, IPC client. |
| `deskboxes-watchdog.exe` | Rust (tiny, `windows` only) | Starts the core, watches it, restores native icons on crash, hang or abnormal exit. It is the autostart target. |
| `deskboxes-shellhost.exe` | C++/WinRT + WinUI 3 (Windows App SDK 2.7, self-contained) | Context-menu host (classic + modern enumeration, the WinUI menu, "Show more options", invocation), plus the settings window. |

**Why the menu runs in another process:** querying context menus loads third-party shell extensions (archivers,
antivirus, cloud clients) into the calling process. In the shell host, a hang or crash costs one menu. The core
keeps painting and restarts the host.

**Settings run in the shell host too (decision).** XAML is already initialized there and kept warm for the menu,
so the settings window opens instantly and we don't ship a second ~100 MB WinUI runtime. Trade-off: a crashing
shell extension also closes an open settings window. Settings are saved on every change, so nothing is lost.

**Startup chain:** HKCU `Run` → `deskboxes-watchdog.exe --launch` → spawns `deskboxes.exe --watchdog-pid <pid>`.
About 300 ms after first paint, the core spawns `deskboxes-shellhost.exe` at below-normal priority and connects to
its pipe.

**Who restarts whom:**
- The watchdog restarts the core: up to 3 times in 60 s, then gives up with icons restored.
- The core restarts the shell host whenever it exits.
- The core restarts the watchdog if it dies.

---

## 2. Threads and COM apartments (core)

| Thread | Apartment | Work |
|---|---|---|
| **UI** (main) | STA, `OleInitialize` | Message loop, all surface windows, input, DComp/D2D drawing, `RegisterDragDrop` targets, `DoDragDrop`. No shell enumeration or icon extraction. |
| **Shell workers** ×2–4 | STA each | `IShellFolder`/`IShellItem` display names and attributes, `IShellItemImageFactory::GetImage`, overlay indices. Bounded priority queue: visible items first, cancellable. |
| **FS thread** | MTA | `FindFirstFileExW` reconciliation, `ReadDirectoryChangesExW` (overlapped) on both desktop folders, 75 ms coalescing. |
| **Shell-notify** | (UI thread window) | `SHChangeNotifyRegister` messages arrive on the UI thread's hidden window and are forwarded to workers. |
| **IPC** | none | Named-pipe client to the shell host: blocking reads, posts results to the UI thread. |
| **Persistence** | none | Debounced (1 s) atomic writes of the layout and snapshot. |

**Rule:** the UI thread never makes a blocking shell call.

**One documented exception: drag and drop.** OLE requires `IDropTarget` calls on the thread that owns the
registered window. Forwarding a drag to an item's own drop target (`GetUIObjectOf(IID_IDropTarget)`) therefore
happens on the UI thread, which is also what Explorer does.

**Cross-process input:** our surfaces are children of Explorer's `Progman`, so Windows attaches our input queue to
Explorer's desktop thread. A stalled UI thread would also stall desktop keyboard focus in Explorer. This is one
more reason to keep the UI thread free of shell calls.

---

## 3. Desktop surface **[verified: build 26300, spike b]**

**Window hierarchy.** On 24H2+/26H2:
- `Progman` (`WS_EX_NOREDIRECTIONBITMAP`) is the desktop window. It has one child, `SHELLDLL_DefView`
  (`WS_EX_LAYERED`), which hosts `SysListView32`.
- The wallpaper is composited by Progman itself; the older `WorkerW` trick is not used.

**Our surface.** One window per monitor, created as a `WS_CHILD` of `Progman` with `WS_EX_NOREDIRECTIONBITMAP`,
inserted at the top of Progman's child z-order, above DefView.
- **Rendering:** D3D11 → D2D device context → `IDCompositionDesktopDevice` target for the window.
- **Verified:** visible over the wallpaper, covered by normal windows, and still visible after Win+D.
- Child windows never appear in Alt+Tab or on the taskbar.
- Win+M, peek, Task View and virtual desktops: Progman is shared by all virtual desktops, so its children show on
  every one. **UNVERIFIED** until the harness checks them.

**Hit-testing.** Our window covers the whole monitor and answers `HTCLIENT`, so every click on the desktop reaches
the core. DefView underneath is hidden (§5) and never sees input.

**Explorer restart.** A hidden top-level window (message-only windows don't receive broadcasts) listens for
`RegisterWindowMessage("TaskbarCreated")`. On it, the core:
1. finds the new Progman
2. recreates its surfaces from the in-memory scene (one frame, no shell calls)
3. re-applies `FWF_NOICONS`

**Other events:**
- **Sleep/resume and lock:** on device loss (`DXGI_ERROR_DEVICE_REMOVED`, `D2DERR_RECREATE_TARGET`) the core
  recreates the D3D/D2D/DComp devices and reuploads from the CPU-side icon cache.
- **Hotplug and DPI:** `WM_DISPLAYCHANGE` and `WM_DPICHANGED` trigger monitor re-enumeration and a new topology
  signature (§8), then re-layout.

**Why one surface per monitor, not one window per box:** one DComp tree per monitor, cheap hit-testing in the core
(uniform grid index), and no z-order fights between box windows.

---

## 4. Rendering

**DPI and units.** The process is per-monitor DPI aware v2. Every layout value is stored in **DIPs relative to the
monitor's work area**. Rendering is in device pixels: `dip × dpi / 96`.

**Scene graph per monitor (DirectComposition):**
- **Root visual**
  - **Content surface** (`IDCompositionVirtualSurface`, monitor size): box backgrounds, icons, labels, selection.
    Only dirty rects are redrawn.
  - **Rubber-band visual:** a small separate surface, moved by transform, so it costs no content redraw.
  - **Overlay visual:** in-place rename editor host, and the diagnostics overlay (`--diagnostics`).

**Box material: Mica.** The default box background is a Mica material that follows the system light/dark app
mode (user requirement). DWM system backdrops (`DWMWA_SYSTEMBACKDROP_TYPE`, `MicaController`) apply only to
top-level windows, not to a child of `Progman`. A box always sits directly on the wallpaper, though, and Mica is
itself "blurred wallpaper + tint", so the material is built from the wallpaper directly:
1. Read the wallpaper path and position per monitor with `IDesktopWallpaper`. Handle fill/fit/stretch/tile/
   center/span and solid colors.
2. Load it with WIC and blur it once with a large-radius D2D Gaussian blur into a per-monitor bitmap. Cache it in
   `%LOCALAPPDATA%\Deskboxes\wallpaper-<mon>.bin`.
3. Per box, clipped to its rounded rect: blurred wallpaper → luminosity layer → tint layer (color × tint opacity)
   → a faint noise texture. Light and dark use the published `MicaController` default tint colors and opacities
   (light `#F3F3F3`, dark `#202020`). These are UNVERIFIED against a real Mica window until a side-by-side
   screenshot comparison is done.
4. Text and glyph colors follow the theme: dark text on light Mica, white text on dark Mica. Explorer's text
   shadow is used only on the transparent (no-material) style.
5. Per-box overrides (`BoxStyle`): tint color, tint opacity, blur amount, or "no material" (plain color × opacity).
6. Rebuild the cache on `WM_SETTINGCHANGE(SPI_SETDESKWALLPAPER)` and on `IDesktopWallpaper` slideshow events.
   Re-tint without re-blurring on `UISettings.ColorValuesChanged` (light/dark switch).

**Icons.** Each unique icon image is an `ID2D1Bitmap1` (per-type entries shared, see §6). Phase 2 packs them into
texture atlases; Phase 1 keeps separate bitmaps (about 60 unique for 1,000 typical items).

**Labels (DirectWrite):**
- Font from `SPI_GETICONTITLELOGFONT` (Segoe UI 9 pt at 100 %), honoring text scaling.
- Layouts are cached per (text, width, DPI).
- Explorer rules: up to 2 lines with an ellipsis when unselected; full text when selected or focused.
- Text is white with a soft dark shadow (two-pass offset shadow in Phase 1; fidelity gap noted in REPORT).

**Animations** run only while active. Every animation checks `SPI_GETCLIENTAREAANIMATION`.

**No polling.** Redraws happen only on input, model change, wallpaper/theme change or an active animation.

**Theme.** `UISettings` provides accent and dark/light; high contrast via `SPI_GETHIGHCONTRAST`. Selection colors
copy Explorer's desktop:
- selected: white at 0x40 alpha fill with a 0x60 border
- hover: white at 0x20

---

## 5. Native icons hidden **[verified: build 26300, spike c]**

**Hide/show.** `IShellWindows::FindWindowSW(CSIDL_DESKTOP)` → `IServiceProvider(SID_STopLevelBrowser)` →
`IShellBrowser::QueryActiveShellView` → `IFolderView2::SetCurrentFolderFlags(FWF_NOICONS, on|off)`.
- Idempotent (not a toggle), applies live in about 13–44 ms.
- **Explorer itself persists** `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced\HideIcons`.
- This is the same state as Desktop → View → Show desktop icons, so nothing is patched or injected.

**No boot flash.** Because `HideIcons=1` stays persisted while Deskboxes is enabled, Explorer creates the desktop
list view already hidden at startup. Explorer-restart proxy result: 0 visible samples out of 7,887 at about 1 ms.

**Rejected alternatives:**
- Writing the registry value alone: it does not apply live.
- The DefView `WM_COMMAND 0x7402` toggle: it works, but it's a toggle, so racy. Kept only as the last-resort restore
  path.

**Order at startup:** paint our surface first, then hide native icons. The desktop is never empty and never shows
both sets for more than one frame.

**Restore paths.** Each ends with `FWF_NOICONS` off, verifies `HideIcons=0`, and falls back to the toggle:

| Path | Trigger |
|---|---|
| Clean exit / "Disable Deskboxes" | core |
| Crash, hang > 6 s, abnormal exit | watchdog (waits on the process handle; hang = `SendMessageTimeout(WM_NULL)` to the core's window every 3 s; on recovery the core re-hides) |
| `deskboxes --restore-icons` | panic command: asks a running core to quit, kills it after 2 s, restores |
| `deskboxes --uninstall` | restore + remove the Run entry |

**Never stuck with an empty desktop:**
1. **At session end** (`WM_QUERYENDSESSION`) the core checks that its autostart is intact. The Run value must exist,
   `StartupApproved\Run` must not mark it disabled, and the watchdog exe must exist. If not, icons are restored
   before logoff, so the next boot shows the native desktop.
2. **If the core fails at boot**, the watchdog restores the icons.
3. **If everything is gone** (binaries deleted without uninstalling), Explorer's own desktop right-click menu still
   offers View → Show desktop icons, because nothing of ours is in the way. This is documented in `TESTING.md` and
   the README.

---

## 6. Desktop contents

**Sources (merged):**
- `FOLDERID_Desktop` (may be redirected to OneDrive)
- `FOLDERID_PublicDesktop`
- virtual items enabled in Desktop Icon Settings: `HKCU\...\Explorer\HideDesktopIcons\NewStartPanel\{CLSID}`

**Test mode.** `--desktop-dir <path>` replaces both folders with a fixture folder and disables virtual items.

**Fast path vs shell path:**
- `FindFirstFileExW(FindExInfoBasic, FIND_FIRST_EX_LARGE_FETCH)` gives names, attributes, sizes and times. It is
  used for reconciliation and change detection.
- `IShellFolder` on the **desktop root** gives display names (`SIGDN_NORMALDISPLAY`: `desktop.ini` names, `.lnk`/`.url`
  names, the hide-extension setting) and the virtual items. Called only for new or changed items.
- Both are measured on 1,000 items in REPORT.

**Filters:** show-hidden (`Hidden`), protected OS files (`ShowSuperHidden`), and hide-extensions (`HideFileExt`).
These are read from `HKCU\...\Explorer\Advanced`, and changes are watched with `RegNotifyChangeKeyValue`.

**Identity.** `ItemKey = (volume serial, 128-bit FileId)` from `GetFileInformationByHandleEx(FileIdInfo)`.
- Handles are opened with `FILE_READ_ATTRIBUTES` only, plus `FILE_FLAG_BACKUP_SEMANTICS` (needed for folders) and
  `FILE_FLAG_OPEN_REPARSE_POINT`, so cloud placeholders never hydrate. The `ReadDirectoryChangesExW` extended
  records also carry FileIds.
- Fallback key: the parsing path.
- Virtual items: `::{CLSID}`.

**Duplicate names.** The same name can exist on the user and Public desktops. They are separate items with
different keys. Explorer shows both, and so do we, with the same display name.

---

## 7. Data model (`crates/core`, no Win32)

```text
Item        { key: ItemKey, parsing_name, display_name, kind: File|Folder|Shortcut|UrlFile|Virtual,
              ext, size, created, modified, attrs, source: User|Public|Virtual|Fixture }
Layout      { schema: u32, monitors: Map<TopologySig, MonitorLayout>, rules: Vec<Rule>, prefs }
MonitorLayout { monitors: Vec<MonitorDesc>, loose: Map<ItemKey, GridPos>, boxes: Vec<BoxModel>,
              default_target: Target }
BoxModel    { id, monitor_ix, rect_dip, title, title_mode, style: BoxStyle, tabs: Vec<Tab>, active_tab,
              rolled_up, auto_rollup_edge, icon_size, spacing }
Tab         { id, title, color, items: Vec<ItemKey> (ordered), sort: SortSpec }
BoxStyle    { material: Mica|Plain, color: Rgba?, opacity, blur, tint: Hsb?, scroll_autohide, anim }
Rule        { id, enabled, target: Target, cond: Cond }      // Cond = And/Or of type/name/target/date/size
UndoStack   { Vec<Op> } // metadata ops + references to shell file ops (rename etc.)
```

**Placement.** Each item appears exactly once: in the loose grid or in one box tab. Items not yet placed go to
`default_target`, or are matched by the rules.

**Operations** are pure functions on `MonitorLayout` and return an inverse op for undo:
- move item, create/rename/delete box, combine boxes into tabs, split a tab out, reorder tabs, roll up
- snapping math: box edges to other boxes and screen edges, sizes to whole icon cells, Shift disables

---

## 8. Persistence

| Data | Location | Format |
|---|---|---|
| Settings | `%APPDATA%\Deskboxes\settings.json` | JSON, `"schema": N` |
| Layouts | `%APPDATA%\Deskboxes\layouts\<sig>.json` | JSON, `"schema": N`, migrations `vN→vN+1` in core |
| Backups | `%APPDATA%\Deskboxes\backups\YYYY-MM-DD\` | copy of layouts; first change of each day; keep 7 |
| Snapshot | `%LOCALAPPDATA%\Deskboxes\snapshot.bin` | binary: header, items (key, names, kind, flags, icon ref, pos), icon pack per DPI/size class (premultiplied BGRA) |
| Logs | `%LOCALAPPDATA%\Deskboxes\logs\deskboxes-N.log` | rolling, 5 × 1 MB, startup phase timings |

- **Atomic writes:** write `X.tmp`, `FlushFileBuffers`, then `ReplaceFileW` (or `MoveFileExW(REPLACE_EXISTING |
  WRITE_THROUGH)`).
- **Topology signature:** a hash of the sorted monitor list (device path from `DISPLAYCONFIG_TARGET_DEVICE_NAME`,
  resolution, DPI, position). An unknown signature starts from the closest known layout, scaled per monitor.

---

## 9. Startup sequence

1. `main`: parse arguments, set per-monitor DPI v2, check the token's elevation (log it in debug), load the
   snapshot with one read.
2. Create D3D/D2D/DComp devices, find Progman, create the surfaces, upload snapshot icons, draw, commit
   (**first paint**). **No shell calls before this point.**
3. Hide native icons (`FWF_NOICONS`). Start the shell host about 300 ms later at low priority.
4. FS thread: enumerate both folders (fast path) → diff against the snapshot → shell workers resolve display names
   and icons for new/changed items → UI applies the changes in one frame (**full reconciliation**).
5. Start the watchers (`ReadDirectoryChangesExW`, `SHChangeNotifyRegister`, registry notifications).
6. Write the snapshot (debounced) after changes and on clean exit.

Targets (warm, 1,000 items): first paint < 250 ms, reconciliation < 1 s.

---

## 10. Input, selection, drag and drop

- **Hit-testing:** a uniform grid spatial index (cell = icon cell) per monitor, rebuilt on layout change, plus box
  rects in z-order.
- **Selection:** a set of `ItemKey` with an anchor and a focus.
  - Rubber band: tests the index; covered items toggle with Ctrl.
  - Auto-scroll: inside boxes.
  - Keyboard: arrows move in the grid by geometry, Home/End, type-to-select (1 s prefix buffer), Ctrl+A,
    Enter = invoke the default verb (through the host), F2 = in-place rename, Delete/Shift+Delete through the shell
    "delete" verb (host).
- **Drag source:** `SHCreateDataObject` with the items' PIDLs relative to the desktop root, plus a private format
  `Deskboxes.InternalDrag` (process id + item keys). `IDragSourceHelper::InitializeFromBitmap` with a drag image
  built from our icons. Then `DoDragDrop`.
- **Drop target** (`RegisterDragDrop` per surface):
  - Over an item that accepts drops (`SFGAO_DROPTARGET`): forward to its `IDropTarget`.
  - Over empty space: forward to the drop target of the desktop folder (or the fixture folder) and record the drop
    point for the item that will appear.
  - Internal drag without a file-op modifier: metadata move only.
  - Drag image via `IDropTargetHelper`.
- **Clipboard:** cut/copy through the shell's verbs (host) or `SHCreateDataObject` with `Preferred DropEffect`.
  `AddClipboardFormatListener` tracks cut state to ghost those icons. Paste uses the background menu's `paste` verb.
- **UI Automation:** each surface implements `IRawElementProviderSimple` with a fragment root. Items are list items
  with name, selection and invoke patterns. Phase 1 is basic; Phase 2 completes it.

---

## 11. IPC (core ↔ shell host)

- **Pipe:** `\\.\pipe\deskboxes-host-<session id>`, created by the host with a DACL for the current user SID only.
- **Framing:** `u32 length (LE)` + UTF-8 JSON object `{ "v": 1, "t": "<type>", "id": <u32>, ... }`. JSON on purpose:
  the C++ side parses it with `Windows.Data.Json` (no extra dependency), messages are small and rare, and logs stay
  readable. `v` is bumped for incompatible changes; unknown types are ignored.

| Direction | Type | Fields |
|---|---|---|
| core→host | `hello` | `proto`, `lang`, `theme` |
| core→host | `prefetch` | `kind: item\|background`, `folder`, `items[]` (parsing names), `shift` |
| core→host | `show` | as `prefetch` + `x`, `y` (physical px), `owner` (hwnd), `extra[]` (Deskboxes entries: id, text, glyph, placement), `classic: bool` |
| core→host | `settings` | open settings window |
| core→host | `quit` | |
| host→core | `ready` | `proto` |
| host→core | `shown` | `id`, `ms` (request → visible) |
| host→core | `closed` | `id`, `invoked` (verb/command or null) |
| host→core | `command` | `id`, `cmd` (Deskboxes-handled: `rename`, `view.*`, `sort.*`, `undo`, `configure`, `box.*`) |
| host→core | `log` | `level`, `msg` |

- Items travel as **parsing names**: full paths, or `::{CLSID}` for virtual items. The host rebuilds PIDLs with the
  desktop root `IShellFolder::ParseDisplayName`, so desktop items get desktop-relative PIDLs (that is what gives the
  desktop-specific menu).

---

## 12. Context menus (shell host)

**Nothing in the menu is specific to one PC.** The host derives every menu at runtime from the machine's own
registrations: classic `IContextMenu` verbs, packaged `IExplorerCommand` extensions, CommandStore handlers, and the
user's 26H2 context-menu preferences. The only fixed input is **Windows' own presentation policy**
(`shellhost/menu_policy.json`). It holds Windows canonical verb names, Windows string-resource references and
Windows menu-resource positions, never a third-party entry. `tools/menu-probe.ps1` reads Windows' real menu with UI
Automation; `tools/menu-dump.ps1` dumps ours with no UI and diffs the two. Verification only: the probe output never
feeds the menu.

**Item menu.** All items are children of the desktop root (user and Public desktops merge there), so one
`GetUIObjectOf(IID_IContextMenu)` covers mixed selections. In fixture mode the fixture folder is used instead.
The **primary item** (the one the user right-clicked or focused) goes first: Windows builds the verb list for it
**[verified: desktop multi-select and user+Public mixed selections]**.

**Classic verbs:**
1. `QueryContextMenu`: add `CMF_EXTENDEDVERBS` with Shift, and `CMF_CANRENAME` for item menus.
2. Initialize submenus with `HandleMenuMsg2(WM_INITMENUPOPUP)`.
3. Read text, state, `GetCommandString(GCS_VERBW)` and `MENUITEMINFO.hbmpItem`.

**Modern verbs (`IExplorerCommand`)** **[verified activatable from an unpackaged, non-elevated process; spike a]:**
- **Registrations:** `PackageManager.FindPackagesForUser("")` plus `AppxManifest.xml` →
  `desktop4/5:FileExplorerContextMenus` `ItemType` (`*`, `Directory`, `Directory\Background`, `Drive`, `.ext`) →
  CLSID. Cached in the host and refreshed on package install/uninstall events.
- **A new instance per menu** (cached class factory). Some handlers keep per-instance state: a reused instance
  returned its subcommands twice **[verified]**.
- **Call order as Explorer:** `GetState(fOkToBeSlow=FALSE)`, and `GetState(TRUE)` only on `E_PENDING`; then
  `GetTitle`, `GetIcon`, `GetFlags`, `EnumSubCommands`. A root whose subcommands are all hidden is not shown.
- **Site:** every command gets our view as its site (`IObjectWithSite` → `IServiceProvider` →
  `SID_SFolderView`/`IFolderView::GetFolder`). The site object is non-agile so handler calls arrive on the host STA.
- **Background menus pass no item array**; handlers find the folder through the site **[verified: with the folder as
  an array, PowerRename and AMD hide; with neither array nor site, both Terminal entries hide; only "no array +
  site" reproduces Windows]**.
- **Order:** by the display name of the app that declares the extension (its Start-menu name, e.g. "Notepad",
  "Photos"), falling back to the package display name **[verified on three menus; full-name order is wrong]**.

**Placement on 26H2** **[verified registry, build 26300]:**
- Windows 11 26H2 has Settings → Personalization → Context menu.
- A packaged extension sits at the **top level** if
  `HKCU\...\Explorer\ContextMenu\Preferences\extensions\<PackageFamilyName>\Enabled = 1`; otherwise it goes into
  **"App extensions"**, which always ends with **"Manage extensions"**.
- Built-in verbs have their own toggles under `...\Preferences\verbs\<verb>`.
- **Feature detection:** the `ContextMenu\Preferences` key exists, or the build is ≥ 26300.
  - Without the feature (the end user's current PC): everything is top level and **"Show more options"** is shown.
  - With it: **"App extensions"** and **"Customize menu"** are shown.

**Windows' promotion rules** (policy file; verified on 26300 unless marked):
- Command row: cut, copy, rename, Share (CommandStore `Windows.ModernShare`, files only), delete.
- Promoted classic verbs are shown **in Windows' order**, not the classic menu's: open, openas, opencontaining,
  runas, copyaspath, Compress to (CommandStore), rotate90, rotate270, setdesktopwallpaper, pintohome,
  PinToStartScreen, pintohomefile, New, properties. `print` and `edit` are not promoted.
- `openas` is always a submenu ("Open with"). An item's `New` verb (Office documents) becomes Windows' New-item
  (ShellNew) submenu.
- Verbs registered with `OnlyInBrowserWindow` (`opennewtab`, `opennewwindow`) appear only in folder windows, never
  on the desktop.
- `LegacyDisable` verbs are hidden. If that hides the default verb, the next static verb of the ProgID is shown
  first, then a separator (seen on `.url`; general rule UNVERIFIED).
- Background menus carry no "Enter" accelerator. On the desktop, the view's own entries (View, Sort by, Refresh,
  plus Paste/Paste shortcut/Undo/Redo when applicable) come first. The core implements them; their text is read
  from DefView's menu resource (shell32 MENU 215), so it is Windows' own localized text.
- Known difference: in a folder-window background Windows shows New and Properties without a separator. This only
  matters for Phase 2 folder-view boxes.

**General rules, not this PC.** The promotion rules above are patterns seen across file types (documents, images,
executables, shortcuts, folders, drives, backgrounds), keyed by Windows' canonical verb names, never by an app or a
ProgID. Fallbacks:
- A classic verb the policy does not name (a third-party `shell\<verb>` or a shell extension's item) stays in the
  classic menu and is reachable through "Show more options", as in Windows. It is never dropped and never promoted
  by guesswork.
- An unknown file type gets the same rules as any other type: its own default verb first, then the promoted verbs it
  actually registers.
- All text comes from the OS: classic verb text from the handler's `IContextMenu` (localized by Windows or the
  handler), modern text from `GetTitle`, fixed entries from Windows string resources (`@shell32.dll,-N` in the
  policy). Deskboxes ships no menu strings of its own, except its own commands (box creation, settings).

**Diagnostic on any PC.** `tools/menu-probe.ps1` (Windows' real menu through UI Automation) and
`tools/menu-dump.ps1` / `tools/menu-view-check.ps1` (ours) ship as diagnostics. Run on the end user's PC (TESTING.md),
any mismatch is a missing or wrong *general* rule, so the fix goes into `menu_policy.json` or the builder as a rule,
never as a per-PC exception.

**View details** **[verified against Explorer on 26300]:**
- Icons: Explorer's own SVGs from the File Explorer package
  (`FileExplorerExtensions\Assets\images\<contrast>\<theme>\<name>.svg`), found at runtime through `PackageManager`
  and mapped by the policy's `iconAssets` names. The default verb shows the default app's icon
  (`AssocQueryString(ASSOCSTR_APPICONREFERENCE)`, packaged PNGs at their `targetsize-16` variant).
- The row has separators between its buttons (Windows' pitch: 65 px).
- Access keys that collide get numbered in display order, row first (C1, C2). KeyTips sit above the row and to the
  right of the list. The list's overflow popup needs the row's root as its `AccessKeyScopeOwner`.
- "Open with": recommended handlers sorted by name, then "Search the Microsoft Store" (unless policy disables it),
  then "Choose another app", without separators.
- Near the bottom of the screen the flyout moves the row to the bottom.
- The core talks to the host over a named pipe (`hello`, `prefetch`, `show`, `quit`; replies `ready`, `shown`,
  `command`, `closed`). Enter/double-click needs an `invoke` message (default verb without a menu).

**Result:** 23 of 24 recorded cases are identical to Windows (rows, lists, accelerators, submenu flags, App
extensions contents), including all 11 real-desktop cases. See `docs/fidelity/deskboxes/summary.json`.

**Built-ins** (Open, Copy as path, Pin to Quick access, Pin to Start, Compress to, Share, Properties, Cut, Copy,
Rename, Delete) are **invoked through the classic `IContextMenu` by canonical verb**, so behavior is Windows' own.
Rename is the exception: it is sent to the core as `command:rename` for in-place editing.

**Rendering.** Explorer's modern menu on 26H2 is a WinUI **`CommandBarFlyout`** **[verified with UI Automation:
`Microsoft.UI.Content.PopupWindowSiteBridge` windows; primary commands are `AppBarButton`s in the row, the list is
the overflow `AppBarButton`s; AutomationId = canonical verb]**. The host uses the same control, shown from a
transparent top-level window at the cursor:
- Primary commands: the command row (Cut/Copy/Rename/Share/Delete), placed on the side nearest the anchor (Windows
  puts it at the top or bottom depending on where the menu opens).
- Secondary commands: the list, with `AppBarButton` + Flyout for submenus, Segoe Fluent Icons glyphs or the
  handlers' own icons, accelerator text, access keys from the `&` mnemonics (KeyTips when opened from the keyboard).
- System backdrop: the default flyout material.

**"Show more options"** (and Shift+right-click where Windows does it): `TrackPopupMenuEx` on the classic `HMENU`,
owned by a host window, forwarding `WM_INITMENUPOPUP`, `WM_DRAWITEM`, `WM_MEASUREITEM` and `WM_MENUCHAR` to
`IContextMenu3`.

**Speed:**
- The core sends `prefetch` on right-button-down and, debounced 150 ms, on selection change.
- The host builds the model off its UI thread (one STA worker per request, results marshaled back).
- Slow handlers get a 150 ms budget. Late entries are inserted while the menu is open, which is Windows' behavior.

---

## 13. How the §6 features fit

| Feature | Where |
|---|---|
| Boxes: create/rename/move/resize/delete; title modes; color, opacity, blur; scrollbars; per-box icon size, spacing; tint; animations | `core::layout` ops + `BoxStyle`; render draws; box menu via host `extra[]` |
| Tabs (combine, reorder with Shift+drag, split out, hover switch, colors, styles) | `core::layout::{combine, split_tab, move_tab}`; `TabStyle` enum; render |
| Roll-up (double-click title, edge auto-roll-up, reveal on hover or click with delay) | `BoxModel.rolled_up`, `auto_rollup_edge`; animation in render |
| Loose icons (grid, auto-arrange, align to grid, remembered positions) | `MonitorLayout.loose` + `core::grid` |
| Sorting rules (type/name/target/date/size; order; preview; apply now / auto) | `core::rules` (pure, unit-tested); shortcut targets resolved by shell workers |
| Layout backups (daily ×7, manual save/restore), per-monitor-configuration layouts | `core::persist` + `TopologySig` |
| Snapping (boxes and edges, gap, cell sizes, Shift off) | `core::snap` (pure, unit-tested) |
| Settings window (WinUI 3, Mica, NavigationView, pl/en) | shell host `SettingsWindow` |
| View / Sort by / Undo / New in the background menu | host shows them; View/Sort/Undo are `command` messages to the core; New is Windows' ShellNew submenu from the background `IContextMenu` |

**Explicitly out of scope:** wallpaper-adaptive colors, folder-mirroring boxes, the "bring desktop to front" peek,
double-click-to-hide-all, multiple swipeable pages, Linux.

---

## 14. Repository

```text
crates/core       pure model, ops, rules, snap, undo, persistence, migrations (unit tests)
crates/ipc        protocol types + framing (Rust side); docs/IPC.md is the shared reference for C++
crates/i18n       pl/en string tables + language selection
crates/shell      Win32/COM: enumeration, identity, watchers, icons, file ops, native icon hiding
crates/render     D3D/D2D/DWrite/DComp scene, icon bitmaps, text, wallpaper blur
crates/desktop    deskboxes.exe: surfaces, attach, input, selection, DnD, IPC client, app state
crates/watchdog   deskboxes-watchdog.exe
crates/setup      deskboxes-setup.exe: installer and uninstaller (§16)
shellhost/        deskboxes-shellhost (C++/WinRT, WinUI 3, MSBuild .vcxproj)
tools/            fixture-gen, run-as-user, menu-probe, explorer-restart-test.ps1
spikes/           throwaway experiments (a) (b) (c), kept for reference
```

---

## 15. Phase 1 as built (differences from the design above)

The sections above are the design for the full product. The Phase 1 proof build implements a
subset, and in places a simpler form. Everything here was checked against the code on
2026-11-19.

| Design (section) | Phase 1 as built |
|---|---|
| Core restarts the watchdog (§1) | Not implemented. The watchdog supervises the core: restart ≤ 3 times in 60 s, hang check by `WM_NULL` every 3 s with a 6 s timeout, icons restored before each restart and when it gives up, stop on `WM_CLOSE`. Verified by `tools/watchdog-check.ps1`. |
| Shell workers ×2–4 STA with a priority queue (§2) | One STA worker thread. It is event-driven (`MsgWaitForMultipleObjectsEx`, no polling) and coalesces queued scans. New or changed entries are resolved on up to 8 MTA threads in parallel (display name, file id, system icon index). Icon images are then extracted on the STA, de-duplicated by system icon index. Cold reconcile of 1,000 items fell from 2.5 s to 0.8 s. |
| FS thread with overlapped `ReadDirectoryChangesExW`, `SHChangeNotifyRegister`, registry notifications (§2, §6, §9) | One blocking `ReadDirectoryChangesW` thread per folder. A change triggers a rescan after 75 ms (the fast path, ~1 ms for 1,000 items); only changed entries go through the shell. Not done: `SHChangeNotifyRegister`, `RegNotifyChangeKeyValue` (the filters are read at each scan). |
| Persistence thread (§2) | Saves run on the UI thread from a 1 s debounce timer: atomic tmp + flush + rename. |
| Virtual surface with dirty rects, separate rubber-band visual (§4) | The whole surface is redrawn per frame, and only on input or model change (idle CPU 0). Draw time is ~3 ms per frame with 2,000 items (Release). |
| Mica: blurred wallpaper → luminosity → tint → noise (§4) | Blurred wallpaper → tint at the box opacity, with no luminosity or noise layer. The wallpaper is decoded at ¼ of its on-screen size for the backdrop. Tint values are UNVERIFIED against a real Mica window. Wallpaper and theme changes are followed through `WM_SETTINGCHANGE`; slideshow events are not. |
| First paint, then hide (§5) | As designed. One addition: without a snapshot (first run), Explorer's icons stay until our first scan is drawn, so the cold scan never shows an empty desktop. Verified on the real desktop: 0 empty-desktop samples. |
| Display names from the desktop root `IShellFolder` (§6) | `SHCreateItemFromParsingName(path)` → `SIGDN_NORMALDISPLAY` per item: the same names for file-system items. Virtual items come from the `NewStartPanel` CLSIDs. |
| Logs `deskboxes-N.log`, 5 × 1 MB (§8) | `deskboxes.log` rotated at 1 MB to `deskboxes.1.log`. The host writes `shellhost.log` and the watchdog `watchdog.log`. |
| Layout file per topology hash (§8) | One file per primary monitor: `primary-<w>x<h>-<dpi>.json`. Multi-monitor and the topology signature are Phase 2. |
| Type-to-select, UI Automation provider (§10) | Not implemented. |
| Drag source `SHCreateDataObject` + private format (§10) | The shell's data object for the items (`BHID_DataObject`). Inside our surface a drag is a metadata move. Once the cursor leaves the window, it becomes an OLE drag with the shell's data object. Drops onto a **folder** item use `IFileOperation` (Windows' copy engine). The folder's own `IDropTarget` returned S_OK without moving anything when driven from our worker; that is recorded, and the cause is not known. Other drop targets (programs, archives, the Recycle Bin) get the item's own `IDropTarget`. |
| Cut ghosting via `AddClipboardFormatListener` (§10) | Not implemented. Hidden items are drawn faded, as Explorer does. |
| IPC messages (§11) | Implemented: `hello`/`ready`, `prefetch`, `show`, `quit`; `shown`, `command`, `closed`. Not implemented: `settings`, `log`, `invoke`. Enter and double-click open items in the core with `ShellExecuteEx` (default verb, `SEE_MASK_INVOKEIDLIST`). The pipe is `deskboxes-host-<session>-<core pid>`, with a DACL for the current user's SID only. |
| Host builds menus off its UI thread, 150 ms budget for slow handlers (§12) | Menus are built on the host's UI thread (STA). The core prefetches when the pointer rests 150 ms on an unselected item (a right-click would select it), on right-button-down unless the host already holds that exact menu, and 150 ms after the selection settles or a menu closes (for the Menu key and Shift+F10). A show uses the prefetch up. Selections are resolved by parsing each parent folder once and the names relative to it: 100 items in 4 ms, previously 148 ms. Slow-handler budgets and late insertion are Phase 2. |
| Settings window (§1, §13) | Not implemented. `settings.json` is edited by hand; "Disable Deskboxes" is in the desktop background menu (Deskboxes submenu). |
| Mixed selections (§12) | On the real desktop, user and Public desktop items share the desktop root, so one menu covers them. In fixture mode two unrelated folders have no common parent, and the host builds no menu for such a selection. This is a fixture-only limitation; `perf-check` uses one folder. |

**No dialogs ever.** All three processes call `SetErrorMode(SEM_FAILCRITICALERRORS |
SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX)`. The shell host also installs an unhandled-
exception filter that logs the exception code and module offset, then exits; the core restarts it.
This was added after a host crash showed a Windows "System Error" box (REPORT §5).

---

## 16. Install layout (`crates/setup`)

```text
%LOCALAPPDATA%\Programs\Deskboxes\    (FOLDERID_UserProgramFiles; the user may pick another folder)
  deskboxes.exe, deskboxes-watchdog.exe, deskboxes-shellhost.exe + Windows App SDK runtime
  msvcp140.dll, vcruntime140.dll, vcruntime140_1.dll   app-local VC++ runtime
  uninstall.exe                          setup without the payload
  install.json                           version + every file setup wrote (for updates and uninstall)
HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Deskboxes   Installed apps entry
HKCU\...\Run  Deskboxes = "<dir>\deskboxes-watchdog.exe" --launch
%APPDATA%\Microsoft\Windows\Start Menu\Programs\Deskboxes.lnk -> watchdog --launch --enable
```

User data (`%APPDATA%\Deskboxes`, `%LOCALAPPDATA%\Deskboxes`) is not part of the install;
the uninstaller removes it only when asked. Nothing goes to HKLM; the manifest is `asInvoker`.

**Payload.** `DeskboxesSetup.exe` = the setup program, then `[u32 header length][JSON header:
version, files, chunks][LZMS chunks of 8 MB, each with a CRC32]`, then a 32-byte footer
(`DBXSETUP`, format, offset, length). Compression is Windows' `cabinet.dll` (Compression API),
so setup needs no compression crate; packing runs the chunks in parallel.

**Update.** Close the app (WM_CLOSE to the watchdog's and core's windows, then terminate what
still runs from the install folder), restore native icons, write files (a file in use is
renamed to `*.old~N` and removed on the next run), delete files in the old `install.json` that
the new version lacks, register, and start it. When setup itself runs elevated, it starts
Deskboxes through Explorer, so Deskboxes never runs elevated.

**Window.** Plain Win32 with no redirection bitmap; Direct2D draws into a DirectComposition
surface over Mica (`DWMSBT_MAINWINDOW`, frame extended into the whole client area, caption
buttons drawn by us). Colors are WinUI 3's theme resources and the accent from `UISettings`.
