# Third-party notices

Deskboxes is built from the components below. The versions are those in `Cargo.lock` and
`shellhost/packages.config` at the time of writing (2026-11-19). The full license texts ship
with each package. For Rust crates they are in the crate sources fetched by cargo; for NuGet
packages they are under `shellhost/packages/<package>/`. A release package must include those
texts next to the binaries.

No GPL/LGPL or other copyleft component is used.

## Rust crates (core `deskboxes.exe`, `deskboxes-watchdog.exe`, `deskboxes-setup.exe`, tools)

| Crate | Version | License | Source |
|---|---|---|---|
| windows, windows-core, windows-result, windows-strings, windows-link, windows-implement, windows-interface, windows-collections, windows-future, windows-threading | 0.62.2 / 0.62.2 / 0.4.1 / 0.5.1 / 0.2.1 / 0.60.2 / 0.59.3 / 0.3.2 / 0.3.2 / 0.2.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| windows-numerics | 0.3.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| serde, serde_core, serde_derive | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde_json | 1.0.154 | MIT OR Apache-2.0 | https://github.com/serde-rs/json |
| log | 0.4.35 | MIT OR Apache-2.0 | https://github.com/rust-lang/log |
| itoa | 1.0.19 | MIT OR Apache-2.0 | https://github.com/dtolnay/itoa |
| zmij | 1.0.23 | MIT | https://github.com/dtolnay/zmij |
| memchr | 2.8.3 | Unlicense OR MIT | https://github.com/BurntSushi/memchr |
| proc-macro2, quote, syn (2.x and 3.x), unicode-ident | 1.0.107, 1.0.47, 2.0.119 / 3.0.10, 1.0.27 | MIT OR Apache-2.0 (unicode-ident: also Unicode-3.0) | https://github.com/dtolnay | 

proc-macro2, quote, syn and unicode-ident are build-time only (procedural macros); they are
not in the shipped binaries.

## C++ shell host (`deskboxes-shellhost.exe`)

| Package | Version | License | Shipped? |
|---|---|---|---|
| Microsoft.Windows.CppWinRT | 3.0.260818.1 | MIT | build-time only (headers and code generator) |
| Microsoft.WindowsAppSDK.Base / Foundation / InteractiveExperiences / WinUI / Runtime | 2.7.0 | Microsoft Software License Terms, "Microsoft Windows App SDK" | yes: the self-contained runtime DLLs, .winmd and .pri files next to the host |
| Microsoft.Web.WebView2 | 1.0.3719.77 | BSD-style (Microsoft) | yes: `Microsoft.Web.WebView2.Core.dll` is copied in by the Windows App SDK; Deskboxes does not use WebView2 |
| Microsoft.Windows.SDK.BuildTools, Microsoft.Windows.SDK.BuildTools.MSIX | 10.0.26100.4654, 1.7.251221100 | Microsoft SDK license terms | build-time only |

On the Windows App SDK license: it is not an open-source license. Its section 3,
"Distributable code", permits redistributing every file the NuGet package places next to the
application ("binplaced"), in self-contained deployments too. The redistributor must add
significant primary functionality, must not use Microsoft's trademarks to market the app, and
must indemnify Microsoft as section 3(b) states. The spec (§1.5) requires this choice: Windows
App SDK 2.7, self-contained. The package's `NOTICE.txt` files list further components inside
the runtime. They must ship with a release.

## Microsoft Visual C++ runtime (installer)

| Files | Version | License | Shipped? |
|---|---|---|---|
| msvcp140.dll, vcruntime140.dll, vcruntime140_1.dll | 14.44.35211.0 | Visual Studio 2022 license terms, "Distributable Code" (the files listed in the Build Tools' `redist.txt`) | yes: app-local, next to the binaries, copied by `build.ps1 -Installer` from `VC\Redist\MSVC\<version>\x64\Microsoft.VC143.CRT` |

`deskboxes-setup.exe` itself links the runtime statically. The same distributable-code
conditions apply as for the Windows App SDK above (significant primary functionality, no
Microsoft trademarks in marketing).

## Windows components used at run time (not redistributed)

These are part of Windows and are called through public APIs. Deskboxes does not ship them:
Direct2D, DirectWrite, Direct3D 11, DirectComposition, WIC, the Shell (IShellItem,
IContextMenu, IExplorerCommand, IFileOperation, IDesktopWallpaper, IFolderView2), OLE drag and
drop, UI Automation (test tools only), and the Compression API (`cabinet.dll`, installer).

## Trademarks

Windows, Windows 11, Explorer and WinUI are trademarks of Microsoft. Deskboxes is an
independent clean-room program. It is not affiliated with, or derived from, any other desktop
organizer.
