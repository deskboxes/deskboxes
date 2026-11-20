#Requires -Version 7
<#
.SYNOPSIS
  Records what Windows' own modern context menu shows, per item type, using UI Automation.

.DESCRIPTION
  Ground truth for Deskboxes' menu rules (docs/REPORT.md, "Menu inclusion/order table").
  - Creates fixture items in a temp folder (never the real Desktop).
  - Opens that folder in a real File Explorer window.
  - For each case: selects the item(s) through Shell.Application, presses the Menu key (VK_APPS)
    in that window, waits, reads the popup menu via UI Automation (names, accelerators,
    submenus, command-row buttons), captures a PNG of the menu, then presses Escape.
  - Optional -Desktop: also reads the REAL desktop background menu (read-only: the menu is
    opened and closed with Escape; nothing is invoked). Uses Win+D to reveal the desktop and
    Win+D again to restore windows.

  Synthetic input goes only to the Explorer window this script opened (and, with -Desktop,
  the desktop). Output: JSON + PNGs in -OutDir.
#>
param(
    [string]$OutDir = (Join-Path $PSScriptRoot '..\docs\fidelity\explorer'),
    [switch]$Desktop,       # real desktop background menu (read-only)
    [switch]$DesktopItems,  # real desktop ITEM menus on items that already exist there (read-only)
    [switch]$NoFolder,      # skip the fixture-folder cases
    [string[]]$Only,
    [string[]]$Expand       # AutomationIds of submenus to open and record (e.g. New, Windows.ExtensionsFlyout)
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path

Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class In {
  [StructLayout(LayoutKind.Sequential)] struct KI { public ushort vk; public ushort sc; public uint fl; public uint t; public IntPtr x; }
  [StructLayout(LayoutKind.Explicit, Size=40)] struct INP { [FieldOffset(0)] public uint type; [FieldOffset(8)] public KI k; }
  [DllImport("user32.dll")] static extern uint SendInput(uint n, INP[] i, int sz);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool f);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  public static void Keys(params ushort[] seq) { // positive = down, vk|0x8000 = up
    var a = new INP[seq.Length];
    for (int i = 0; i < seq.Length; i++) { a[i].type = 1; a[i].k.vk = (ushort)(seq[i] & 0x7FFF); a[i].k.fl = (seq[i] & 0x8000) != 0 ? 2u : 0u; }
    SendInput((uint)a.Length, a, Marshal.SizeOf(typeof(INP)));
  }
  public static void Focus(IntPtr h) {
    uint me = GetCurrentThreadId(), fg = GetWindowThreadProcessId(GetForegroundWindow(), IntPtr.Zero);
    AttachThreadInput(me, fg, true); ShowWindow(h, 9); BringWindowToTop(h); SetForegroundWindow(h); AttachThreadInput(me, fg, false);
  }
}
'@


. (Join-Path $PSScriptRoot 'lib\MenuFixtures.ps1')

Add-Type -Path (Join-Path $PSScriptRoot 'lib\TestInput.cs')
. (Join-Path $PSScriptRoot 'lib\MenuUia.ps1')

function Probe-Menu([string]$name, [IntPtr]$focusHwnd, [int]$explorerPid, [scriptblock]$prepare, [ushort[]]$keys) {
    & $prepare
    [In]::Focus($focusHwnd); Start-Sleep -Milliseconds 350
    $sw = [Diagnostics.Stopwatch]::StartNew()
    [In]::Keys($keys)
    $wins = @()
    while ($sw.ElapsedMilliseconds -lt 4000 -and -not $wins) {
        Start-Sleep -Milliseconds 40
        $wins = Get-MenuWindows $explorerPid
    }
    $appeared = $sw.ElapsedMilliseconds
    if (-not $wins) { Write-Warning "[$name] no menu appeared"; [In]::Keys(0x1B, 0x801B); return $null }
    Start-Sleep -Milliseconds 700 # let late (slow) entries arrive
    $wins = Get-MenuWindows $explorerPid
    $m = Read-Menu $wins
    # Optionally open submenus (by AutomationId) and record what they contain.
    $subs = if ($Expand) { Expand-Submenus $explorerPid $m $Expand (Join-Path $OutDir $name) } else { [ordered]@{} }
    # capture the union of the menu windows
    $l = ($wins | ForEach-Object { $_.Current.BoundingRectangle.X } | Measure-Object -Minimum).Minimum
    $t = ($wins | ForEach-Object { $_.Current.BoundingRectangle.Y } | Measure-Object -Minimum).Minimum
    $rr = ($wins | ForEach-Object { $_.Current.BoundingRectangle.Right } | Measure-Object -Maximum).Maximum
    $bb = ($wins | ForEach-Object { $_.Current.BoundingRectangle.Bottom } | Measure-Object -Maximum).Maximum
    Save-Capture ([System.Windows.Rect]::new($l, $t, $rr - $l, $bb - $t)) (Join-Path $OutDir "$name.png")
    [In]::Keys(0x1B, 0x801B); Start-Sleep -Milliseconds 300
    $res = [pscustomobject]@{ case = $name; appearedMs = $appeared; rowPosition = $m.rowPosition; row = $m.row; list = $m.list }
    if ($subs.Count) { $res | Add-Member submenus ([pscustomobject]$subs) }
    $res
}

# ---------------------------------------------------------------------------
$results = [System.Collections.Generic.List[object]]::new()
$shell = New-Object -ComObject Shell.Application
$fx = Join-Path ([IO.Path]::GetTempPath()) 'deskboxes-menuprobe'
if (-not $NoFolder) {
New-MenuFixtures $fx
Start-Process explorer.exe $fx
$win = $null
for ($i = 0; $i -lt 50 -and -not $win; $i++) {
    Start-Sleep -Milliseconds 200
    $win = @($shell.Windows()) | Where-Object { try { $_.Document.Folder.Self.Path -eq $fx } catch { $false } } | Select-Object -First 1
}
if (-not $win) { throw 'Explorer window for fixture folder not found' }
$hwnd = [IntPtr]$win.HWND
$expPid = Get-WindowPid $hwnd
$doc = $win.Document
Start-Sleep -Milliseconds 800

function Select-Items([string[]]$names) {
    $doc.SelectItem($doc.Folder.ParseName($names[0]), 1 + 4 + 8 + 16)
    foreach ($n in $names | Select-Object -Skip 1) { $doc.SelectItem($doc.Folder.ParseName($n), 1) }
}
function Clear-Selection { foreach ($i in @($doc.SelectedItems())) { $doc.SelectItem($i, 0) } }

$VK_APPS = [uint16]0x5D
$cases = $MenuCases
foreach ($k in $cases.Keys) {
    if ($Only -and $k -notin $Only) { continue }
    $names = $cases[$k]
    $r = Probe-Menu $k $hwnd $expPid { Select-Items $names } @($VK_APPS, ($VK_APPS -bor 0x8000))
    if ($r) { $results.Add($r) }
    # Shift+F10 on the same selection, to compare with the Menu key
    if ($k -eq 'pdf') {
        $r2 = Probe-Menu 'pdf_shiftF10' $hwnd $expPid { Select-Items $names } @(0x10, 0x79, 0x8079, 0x8010)
        if ($r2) { $results.Add($r2) }
    }
}
if (-not $Only -or 'folderbg' -in $Only) {
    $r = Probe-Menu 'folder_background' $hwnd $expPid { Clear-Selection } @($VK_APPS, ($VK_APPS -bor 0x8000))
    if ($r) { $results.Add($r) }
}
$win.Quit()
}
$VK_APPS = [uint16]0x5D

if ($Desktop -or $DesktopItems) {
    # REAL desktop (read-only): menus are opened with the Menu key and closed with Escape;
    # nothing is invoked, nothing is created on the desktop. Win+D reveals the desktop and
    # Win+D again restores the windows.
    # PowerShell passes $null to a .NET string parameter as "", so use [NullString]::Value.
    $N = [NullString]::Value
    $pm = [TestInput]::FindWindowW('Progman', $N)
    $dv = [TestInput]::FindWindowExW($pm, [IntPtr]::Zero, 'SHELLDLL_DefView', $N)
    if ($dv -eq [IntPtr]::Zero) {   # after some wallpaper transitions DefView lives under a WorkerW
        $w = [IntPtr]::Zero
        do { $w = [TestInput]::FindWindowExW([IntPtr]::Zero, $w, 'WorkerW', $N); if ($w -ne [IntPtr]::Zero) { $dv = [TestInput]::FindWindowExW($w, [IntPtr]::Zero, 'SHELLDLL_DefView', $N) } } while ($w -ne [IntPtr]::Zero -and $dv -eq [IntPtr]::Zero)
    }
    $lv = [TestInput]::FindWindowExW($dv, [IntPtr]::Zero, 'SysListView32', $N)
    if ($lv -eq [IntPtr]::Zero) { throw 'desktop list view not found' }
    $dpid = Get-WindowPid $pm
    [In]::Keys(0x5B, 0x44, 0x8044, 0x805B); Start-Sleep -Milliseconds 1200   # Win+D
    try {
    # The desktop list view exposes no UIA children, so selection goes through the shell
    # (IFolderView2::SelectItem on the desktop view; spikes/hide-icons `select`).
    $hideIcons = (Resolve-Path (Join-Path $PSScriptRoot '..\target\debug\spike-hide-icons.exe')).Path
    function Select-DesktopItems([string[]]$paths) {
        $o = & $hideIcons select @paths 2>&1
        if ($LASTEXITCODE -ne 0) { throw "desktop select failed: $o" }
    }
    function Clear-DesktopSelection { & $hideIcons deselect | Out-Null }

    if ($Desktop -and (-not $Only -or 'desktop_background' -in $Only)) {
        $r = Probe-Menu 'desktop_background' $lv $dpid { Clear-DesktopSelection } @($VK_APPS, ($VK_APPS -bor 0x8000))
        if ($r) { $results.Add($r) }
    }
    if ($DesktopItems) {
        $ud = [Environment]::GetFolderPath('Desktop'); $pd = [Environment]::GetFolderPath('CommonDesktopDirectory')
        function First-Of([string]$dir, [string]$ext) {
            Get-ChildItem -LiteralPath $dir -Force | Where-Object { -not ($_.Attributes -band [IO.FileAttributes]::Hidden) } |
                Where-Object { if ($ext -eq '<dir>') { $_.PSIsContainer } else { -not $_.PSIsContainer -and $_.Extension -eq $ext } } |
                Sort-Object Name | Select-Object -First 1 -ExpandProperty FullName
        }
        $repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
        $dcases = [ordered]@{
            'desktop_folder' = @($(if ((Split-Path $repo -Parent) -eq $ud) { $repo } else { First-Of $ud '<dir>' }))
            'desktop_docx'   = @(First-Of $ud '.docx')
            'desktop_exe'    = @(First-Of $ud '.exe')
            'desktop_lnk'    = @(First-Of $ud '.lnk')
            'desktop_publnk' = @(First-Of $pd '.lnk')
            'desktop_png'    = @(First-Of $ud '.png')
            'desktop_url'    = @(First-Of $ud '.url')
            'desktop_zip'    = @(First-Of $ud '.zip')
        }
        $dcases['desktop_multi'] = @($dcases['desktop_docx'] + $dcases['desktop_png'])
        $dcases['desktop_mixed'] = @($dcases['desktop_folder'] + $dcases['desktop_lnk'] + $dcases['desktop_publnk'])
        foreach ($k in $dcases.Keys) {
            if ($Only -and $k -notin $Only) { continue }
            $paths = @($dcases[$k] | Where-Object { $_ })
            if (-not $paths) { Write-Warning "[$k] no such item on this desktop; skipped"; continue }
            $r = Probe-Menu $k $lv $dpid { Select-DesktopItems $paths } @($VK_APPS, ($VK_APPS -bor 0x8000))
            if ($r) { $r | Add-Member items $paths; $results.Add($r) }
        }
        Clear-DesktopSelection
    }
    } finally {
        [In]::Keys(0x1B, 0x801B)                                                  # close any open menu
        [In]::Keys(0x5B, 0x44, 0x8044, 0x805B); Start-Sleep -Milliseconds 800   # restore windows
    }
}

Remove-Item $fx -Recurse -Force -ErrorAction SilentlyContinue
$json = Join-Path $OutDir 'menus.json'
# Merge with earlier runs: cases re-probed now replace the old entries of the same name.
$all = [ordered]@{}
if (Test-Path $json) { foreach ($o in (Get-Content $json -Raw -Encoding utf8 | ConvertFrom-Json)) { $all[$o.case] = $o } }
foreach ($r in $results) { $all[$r.case] = $r }
@($all.Values) | ConvertTo-Json -Depth 6 | Set-Content $json -Encoding utf8
foreach ($r in $results) {
    "=== $($r.case)  (menu after $($r.appearedMs) ms; command row: $($r.rowPosition))"
    "  row: " + (($r.row | ForEach-Object { "$($_.name)[$($_.id)]" }) -join ' | ')
    foreach ($e in $r.list) { "  {0,-42} {1,-28} {2}{3}" -f $e.name, $e.id, $e.accel, $(if ($e.submenu) { '  >' } else { '' }) }
    if ($r.submenus) {
        foreach ($p in $r.submenus.PSObject.Properties) {
            "  [$($p.Name) >]"
            foreach ($e in $p.Value) { "      {0,-38} {1,-28} {2}{3}" -f $e.name, $e.id, $e.accel, $(if ($e.submenu) { '  >' } else { '' }) }
        }
    }
}
