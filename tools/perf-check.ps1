<#
.SYNOPSIS
  §7 performance measurements in fixture mode (--desktop-dir, --window), Release build by default.

  1,000 items: cold start, 5 warm starts (first paint, reconciliation, memory), then an interactive
  run with 2 boxes + a tab pair: idle CPU (core + shell host), right-click -> menu visible with 1
  and with 100 items selected, file created / renamed on disk -> item shown.
  2,000 items: rubber-band drags with draw time per frame and the interval between frames.

  Runs the app NON-elevated (run-as-user). Input goes only to the app's own test window and to
  its menus. No screenshots. Removes the fixture afterwards (unless -KeepFixture).
  Output: docs/perf/perf-<config>.json plus a printed summary.
#>
param(
    [ValidateSet('release', 'debug')][string]$Config = 'release',
    [switch]$KeepFixture
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Exe = Join-Path $root "target\$Config\deskboxes.exe"
$runAs = Join-Path $root 'target\debug\run-as-user.exe'
$gen = Join-Path $root 'target\debug\fixture-gen.exe'
$outDir = Join-Path $root 'docs\perf'; New-Item -ItemType Directory -Force $outDir | Out-Null
$fx = Join-Path $env:TEMP 'dbx-perf'; $pub = Join-Path $env:TEMP 'dbx-perf-public'
$prof = Join-Path $env:TEMP 'dbx-perf-profile'
$scene = Join-Path $prof 'scene.json'
Add-Type -Path (Join-Path $PSScriptRoot 'lib\TestInput.cs')
Add-Type -TypeDefinition @'
using System; using System.Diagnostics; using System.Runtime.InteropServices; using System.Text; using System.Threading;
public static class PerfHelp {
    [StructLayout(LayoutKind.Sequential)] struct MI { public int dx, dy; public uint data, fl, time; public IntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct INP { public uint type; public MI m; }
    [DllImport("user32.dll")] static extern uint SendInput(uint n, INP[] a, int cb);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    /// Visible menu windows of the shell host (WinUI popup bridges or classic #32768), larger than a stub.
    public static int Popups(int pid) {
        int n = 0;
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p != pid || !IsWindowVisible(h)) return true;
            var s = new StringBuilder(128); GetClassNameW(h, s, 128); var c = s.ToString();
            if (c != "Microsoft.UI.Content.PopupWindowSiteBridge" && c != "#32768") return true;
            RECT r; GetWindowRect(h, out r);
            if (r.Right - r.Left > 100 && r.Bottom - r.Top > 30) n++;
            return true;
        }, IntPtr.Zero);
        return n;
    }
    [StructLayout(LayoutKind.Explicit, Size = 40)] struct KINP { [FieldOffset(0)] public uint type; [FieldOffset(8)] public ushort vk; [FieldOffset(10)] public ushort scan; [FieldOffset(12)] public uint fl; [FieldOffset(16)] public uint time; [FieldOffset(24)] public IntPtr extra; }
    [DllImport("user32.dll", EntryPoint = "SendInput")] static extern uint SendKeys(uint n, KINP[] a, int cb);
    static double WaitMenu(Stopwatch sw, int pid, int before, int timeoutMs) {
        while (sw.ElapsedMilliseconds < timeoutMs) { if (Popups(pid) > before) return sw.Elapsed.TotalMilliseconds; Thread.Yield(); }
        return -1;
    }
    static void Btn(uint fl) { var a = new INP[1]; a[0].type = 0; a[0].m.fl = fl; SendInput(1, a, Marshal.SizeOf(typeof(INP))); }
    /// Pointer to (x,y), rest dwellMs, right-click with no hold; ms from the button-down input
    /// until a menu window of pid is visible.
    public static double RightClickToMenu(int x, int y, int pid, int dwellMs, int timeoutMs) {
        SetCursorPos(x, y); Thread.Sleep(dwellMs);
        int before = Popups(pid);
        var sw = Stopwatch.StartNew();
        Btn(0x0008); Btn(0x0010);
        return WaitMenu(sw, pid, before, timeoutMs);
    }
    /// Right button held holdMs (a human click); ms from the button-UP input (menus open on release).
    public static double RightHoldToMenu(int x, int y, int pid, int holdMs, int timeoutMs) {
        SetCursorPos(x, y); Thread.Sleep(80);
        int before = Popups(pid);
        Btn(0x0008); Thread.Sleep(holdMs);
        var sw = Stopwatch.StartNew();
        Btn(0x0010);
        return WaitMenu(sw, pid, before, timeoutMs);
    }
    /// Press and release a key (0x5D: the Menu key); ms from the key-down input.
    public static double KeyToMenu(ushort vk, int pid, int timeoutMs) {
        int before = Popups(pid);
        var a = new KINP[2]; a[0].type = 1; a[0].vk = vk; a[1].type = 1; a[1].vk = vk; a[1].fl = 2;
        var sw = Stopwatch.StartNew();
        SendKeys(2, a, Marshal.SizeOf(typeof(KINP)));
        return WaitMenu(sw, pid, before, timeoutMs);
    }
}
'@

$results = [ordered]@{}
function Note($k, $v) { $script:results[$k] = $v; Write-Host ("{0,-30} {1}" -f $k, ($v | ConvertTo-Json -Compress -Depth 4)) }
function Step([string]$m) { Write-Host ("[{0:HH:mm:ss.fff}] {1}" -f (Get-Date), $m) }
function Median([double[]]$v) { $s = @($v | Sort-Object); if (-not $s.Count) { return $null }; $s[[int][math]::Floor(($s.Count - 1) / 2)] }

function Start-Core([string[]]$extra = @()) {
    Remove-Item $scene -ErrorAction SilentlyContinue
    $a = @('--desktop-dir', $fx, '--window', '--profile', $prof, '--scene-out', $scene, '--no-ui-ops') + $extra
    & $runAs --no-wait -- $Exe @a | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew(); $p = $null
    while (-not $p -and $sw.ElapsedMilliseconds -lt 5000) { Start-Sleep -Milliseconds 20; $p = Get-Process deskboxes -ErrorAction SilentlyContinue | Select-Object -First 1 }
    $p
}
function Get-Scene { for ($i = 0; $i -lt 20; $i++) { try { return (Get-Content $scene -Raw -Encoding utf8 | ConvertFrom-Json) } catch { Start-Sleep -Milliseconds 50 } }; $null }
function Wait-Scene([scriptblock]$cond, [int]$ms = 8000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (Test-Path $scene) { $s = Get-Scene; if ($s -and (& $cond $s)) { return $s } }; Start-Sleep -Milliseconds 60 }
    Get-Scene
}
function Stop-Core($p) {
    $core = [TestInput]::FindWindowW('DeskboxesCore', [NullString]::Value)
    if ($core -ne [IntPtr]::Zero) { [TestInput]::PostMessageW($core, 0x10, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null }
    if (-not $p.WaitForExit(8000)) { $p.Kill(); return 'killed' }
    'clean'
}
function Run-Timed([string]$name, [int]$exitAfter = 1500) {
    $st = Join-Path $prof "$name.json"
    $p = Start-Core @('--exit-after', $exitAfter, '--stats-out', $st, '--no-host')
    if (-not $p.WaitForExit(30000)) { $p.Kill(); throw "$name did not exit" }
    Get-Content $st -Raw | ConvertFrom-Json
}
# Desktop mode (the real configuration: the surface on the desktop layer, native icons hidden),
# showing the fixture. Short: exits by itself 1.5 s after reconciliation, restoring the icons;
# the restore is verified by query afterwards. Separate profile (the monitor-sized layout).
$dprof = "$prof-desk"
function Run-Desktop([string]$name) {
    $st = Join-Path $dprof "$name.json"
    & $runAs --no-wait -- $Exe --desktop-dir $fx --profile $dprof --no-host --exit-after 1500 --stats-out $st | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew(); $p = $null
    while (-not $p -and $sw.ElapsedMilliseconds -lt 5000) { Start-Sleep -Milliseconds 20; $p = Get-Process deskboxes -ErrorAction SilentlyContinue | Select-Object -First 1 }
    if ($p -and -not $p.WaitForExit(30000)) { $p.Kill() }
    $state = (& $hideTool state 2>&1) -join ' '
    if ($state -notmatch 'lv_visible=true') { & $runAs -- $Exe --restore-icons | Out-Null; $state = 'RESTORED BY --restore-icons: ' + ((& $hideTool state 2>&1) -join ' ') }
    $j = Get-Content $st -Raw | ConvertFrom-Json
    [ordered]@{ firstPaintMs = [math]::Round($j.first_paint_ms, 1); hideMs = [math]::Round($j.hide_ms, 1); reconcileMs = [math]::Round($j.reconcile_ms, 1); privateMb = [math]::Round($j.private_mb, 1); workingSetMb = [math]::Round($j.working_set_mb, 1); nativeIconsAfter = ($state -match 'lv_visible=true') }
}
$hideTool = Join-Path $root 'target\debug\spike-hide-icons.exe'
function New-Fixture([int]$n) {
    Remove-Item $dprof -Recurse -Force -ErrorAction SilentlyContinue
    if (Test-Path "$fx.fixture.json") { & $gen remove $fx | Out-Null }
    & $gen create $fx $n | Write-Host   # one folder: a menu needs one parent folder (mixed fixture folders have none; REPORT)
    Remove-Item $prof -Recurse -Force -ErrorAction SilentlyContinue
}
function Seed-Layout([int]$boxOneCount) {
    $layoutFile = Get-ChildItem (Join-Path $prof 'layouts') -Filter *.json | Select-Object -First 1
    $L = Get-Content $layoutFile.FullName -Raw -Encoding utf8 | ConvertFrom-Json
    $keys = @($L.monitor.loose.PSObject.Properties.Name | Sort-Object)
    $loose = [ordered]@{}; $i = 0
    foreach ($k in $keys[0..119]) { $loose[$k] = @{ col = [int][math]::Floor($i / 10); row = $i % 10 }; $i++ }
    $style = @{ material = 'Mica'; color = $null; opacity = 0.8; blur = 60.0; title_color = $null }
    $e1 = 119 + $boxOneCount; $rest = $keys.Count - 1; $mid = $e1 + [int](($rest - $e1) / 2)
    $b1 = @{ id = 10; rect = @{ x = 1110.0; y = 16.0; w = 470.0; h = 520.0 }; style = $style; tabs = @(@{ id = 11; title = 'Dokumenty'; items = @($keys[120..$e1]) }); active_tab = 0; rolled_up = $false }
    $b2 = @{ id = 12; rect = @{ x = 1110.0; y = 556.0; w = 470.0; h = 420.0 }; style = $style; tabs = @(@{ id = 13; title = 'Zdjęcia'; items = @($keys[($e1 + 1)..$mid]) }, @{ id = 14; title = 'Muzyka'; items = @($keys[($mid + 1)..$rest]) }); active_tab = 0; rolled_up = $false }
    $L.monitor = @{ boxes = @($b1, $b2); loose = $loose; next_id = 20 }
    [IO.File]::WriteAllText($layoutFile.FullName, ($L | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
}
function Win($p) { [TestInput]::TopLevel($p.Id, 'DeskboxesTestWindow') | Select-Object -First 1 }
function WinRect($h) { $r = New-Object PerfHelp+RECT; [PerfHelp]::GetWindowRect($h, [ref]$r) | Out-Null; $r }
function Center($it) { @([int]($it.rect[0] + $it.rect[2] / 2), [int]($it.rect[1] + 20)) }
# All input is guarded (TestInput.Allowed): refused unless it targets our own test processes.
$script:front = [IntPtr]::Zero
function Front { if ($script:front -ne [IntPtr]::Zero -and [TestInput]::GetForegroundWindow() -ne $script:front) { [TestInput]::Focus($script:front) | Out-Null; Start-Sleep -Milliseconds 200 } }
function Click($xy, [int]$btn = 0) { Front; [TestInput]::Click($xy[0], $xy[1], $btn); Start-Sleep -Milliseconds 250 }
function RClick($xy, $dwell = 80) { Front; [TestInput]::CheckPoint($xy[0], $xy[1]); [math]::Round([PerfHelp]::RightClickToMenu($xy[0], $xy[1], $hostP.Id, $dwell, 5000), 1) }
function RHold($xy) { Front; [TestInput]::CheckPoint($xy[0], $xy[1]); [math]::Round([PerfHelp]::RightHoldToMenu($xy[0], $xy[1], $hostP.Id, 100, 5000), 1) }
function MenuKey([int]$menuPid) { Front; [TestInput]::CheckForeground(); [math]::Round([PerfHelp]::KeyToMenu(0x5D, $menuPid, 5000), 1) }
function Esc { [TestInput]::Keys(@(0x1B, 0x801B)) | Out-Null }
function Wait-NoMenu($hostPid) { $sw = [Diagnostics.Stopwatch]::StartNew(); while ([PerfHelp]::Popups($hostPid) -gt 0 -and $sw.ElapsedMilliseconds -lt 3000) { Start-Sleep -Milliseconds 30 }; Start-Sleep -Milliseconds 300 }

# ---- machine -----------------------------------------------------------------------------------
$cv = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$gpu = @(Get-CimInstance Win32_VideoController | ForEach-Object { "$($_.Name) @ $($_.CurrentRefreshRate) Hz" })
Note 'machine' ([ordered]@{
        # ProductName still says "Windows 10" on Windows 11; the build number decides.
        windows = "$(if ([int]$cv.CurrentBuild -ge 22000) { $cv.ProductName -replace "Windows 10", "Windows 11" } else { $cv.ProductName }) $($cv.DisplayVersion) build $($cv.CurrentBuild).$($cv.UBR)"
        cpu = "$($cpu.Name.Trim()) ($($cpu.NumberOfCores) cores / $($cpu.NumberOfLogicalProcessors) threads)"
        ramGb = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 1)
        gpu = $gpu; config = $Config
    })
if (Get-Process deskboxes, deskboxes-watchdog -ErrorAction SilentlyContinue) { throw 'a Deskboxes process is already running' }

$created = @()
try {
    # ---- 1,000 items: startup ------------------------------------------------------------------
    New-Fixture 1000
    $cold = Run-Timed 'cold' 2500
    Note 'n1000.cold' ([ordered]@{ firstPaintMs = $cold.first_paint_ms; reconcileMs = $cold.reconcile_ms; items = $cold.items; privateMb = $cold.private_mb; workingSetMb = $cold.working_set_mb })
    $warm = 1..5 | ForEach-Object { Run-Timed "warm$_" 1500 }
    Note 'n1000.warm' ([ordered]@{
            firstPaintMs = @($warm.first_paint_ms | ForEach-Object { [math]::Round($_, 1) })
            reconcileMs = @($warm.reconcile_ms | ForEach-Object { [math]::Round($_, 1) })
            medianFirstPaintMs = [math]::Round((Median $warm.first_paint_ms), 1)
            medianReconcileMs = [math]::Round((Median $warm.reconcile_ms), 1)
            medianPrivateMb = [math]::Round((Median $warm.private_mb), 1)
            medianWorkingSetMb = [math]::Round((Median $warm.working_set_mb), 1)
        })

    $dc = Run-Desktop 'desk-cold'; $dw = @(1..3 | ForEach-Object { Run-Desktop "desk-warm$_" })
    Note 'n1000.desktopMode' ([ordered]@{ cold = $dc; warm = $dw; medianWarmFirstPaintMs = (Median $dw.firstPaintMs); medianWarmPrivateMb = (Median $dw.privateMb) })

    # ---- 1,000 items: interactive (boxes, host) -------------------------------------------------
    Seed-Layout 600
    $st = Join-Path $prof 'interactive.json'
    $p = Start-Core @('--stats-out', $st)
    $s = Wait-Scene { param($s) $s.boxes.Count -eq 2 } 15000
    $sw = [Diagnostics.Stopwatch]::StartNew(); $hostP = $null
    while (-not $hostP -and $sw.ElapsedMilliseconds -lt 15000) { Start-Sleep -Milliseconds 200; $hostP = Get-Process deskboxes-shellhost -ErrorAction SilentlyContinue | Select-Object -First 1 }
    Start-Sleep -Seconds 4   # host prewarm, reconciliation, settle
    $procs = @($p, $hostP) | Where-Object { $_ }
    $c0 = ($procs | ForEach-Object { $_.Refresh(); $_.TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum
    Start-Sleep -Seconds 10
    $c1 = ($procs | ForEach-Object { $_.Refresh(); $_.TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum
    $coreCpu = $p.TotalProcessorTime.TotalMilliseconds
    Note 'idleCpu' ([ordered]@{ cpuMsIn10s = [math]::Round($c1 - $c0, 1); percentOfOneCore = [math]::Round(($c1 - $c0) / 100, 2); processes = @($procs | ForEach-Object ProcessName) })

    [TestInput]::Allowed.Clear(); [void][TestInput]::Allowed.Add($p.Id); [void][TestInput]::Allowed.Add($hostP.Id)
    $win = Win $p; $script:front = $win; [TestInput]::Focus($win) | Out-Null; Start-Sleep -Milliseconds 300
    $wr = WinRect $win
    $emptyPt = @([int]($wr.Left + 13 * 75 + 37), [int]($wr.Top + 300))
    $s = Get-Scene
    $loose = @($s.items | Where-Object { $null -eq $_.box } | Sort-Object { $_.rect[0] }, { $_.rect[1] })
    # Menus. Four triggers: right-click with no hold after the pointer rested 80 ms (from
    # button-down; worst case: shorter than the 150 ms hover prefetch, so the build starts at
    # button-down), the same after a 300 ms rest (hover prefetch done), right-click held 100 ms
    # (from button-up, when menus open), Menu key (from key-down). One item: three files;
    # 100 items: Shift range.
    $targets = @($loose | Where-Object { $_.path -like '*.txt' -or $_.path -like '*.docx' -or $_.path -like '*.pdf' } | Select-Object -First 3)
    $m1 = [ordered]@{ noHold = @(); rest300 = @(); hold100 = @(); menuKey = @() }
    foreach ($it in $targets) {
        $c = Center $it
        Step "menu1 $($it.name)"
        Click $emptyPt; $m1.noHold += RClick $c; Esc; Wait-NoMenu $hostP.Id
        Step 'rest'
        Click $emptyPt; Start-Sleep -Milliseconds 300; $m1.rest300 += RClick $c 300; Esc; Wait-NoMenu $hostP.Id
        Step 'hold'
        Click $emptyPt; $m1.hold100 += RHold $c; Esc; Wait-NoMenu $hostP.Id
        Step 'key'
        Click $c; Start-Sleep -Milliseconds 100
        $m1.menuKey += MenuKey $hostP.Id; Esc; Wait-NoMenu $hostP.Id
    }
    Note 'menu.oneItemMs' ([ordered]@{ noHold = $m1.noHold; rest300 = $m1.rest300; hold100 = $m1.hold100; menuKey = $m1.menuKey; medianNoHold = (Median $m1.noHold); medianRest300 = (Median $m1.rest300); medianHold100 = (Median $m1.hold100); medianMenuKey = (Median $m1.menuKey) })
    Click $emptyPt
    Click (Center $loose[0])
    [TestInput]::Keys(@(0x10)) | Out-Null; Click (Center $loose[99]); [TestInput]::Keys(@(0x8010)) | Out-Null
    $s = Wait-Scene { param($s) @($s.items | Where-Object selected).Count -ge 100 } 3000
    $sel100 = @($s.items | Where-Object selected)
    $m100 = [ordered]@{ noHold = @(); hold100 = @(); menuKey = @() }
    foreach ($k in 0..2) {
        $c = Center $loose[50 + $k]
        $m100.noHold += RClick $c; Esc; Wait-NoMenu $hostP.Id
        $m100.hold100 += RHold $c; Esc; Wait-NoMenu $hostP.Id
        $m100.menuKey += MenuKey $hostP.Id; Esc; Wait-NoMenu $hostP.Id
    }
    Note 'menu.manyItemsMs' ([ordered]@{ selected = $sel100.Count; noHold = $m100.noHold; hold100 = $m100.hold100; menuKey = $m100.menuKey; medianNoHold = (Median $m100.noHold); medianHold100 = (Median $m100.hold100); medianMenuKey = (Median $m100.menuKey) })
    Click $emptyPt

    # Reference: Windows' own menu for the same files in an Explorer window on the fixture folder
    # (Menu key, from key-down). Explorer's view differs from ours; the menu machinery is the same.
    $shell = New-Object -ComObject Shell.Application
    Start-Process explorer.exe $fx
    $ew = $null
    for ($i = 0; $i -lt 50 -and -not $ew; $i++) { Start-Sleep -Milliseconds 200; $ew = @($shell.Windows()) | Where-Object { try { $_.Document.Folder.Self.Path -eq $fx } catch { $false } } | Select-Object -First 1 }
    if ($ew) {
        $eh = [IntPtr]$ew.HWND; $epid = [TestInput]::Pid($eh); $doc = $ew.Document
        [void][TestInput]::Allowed.Add($epid); $script:front = $eh
        Start-Sleep -Seconds 2
        function ExpSelect([string[]]$names) {
            $doc.SelectItem($doc.Folder.ParseName($names[0]), 1 + 4 + 8 + 16)
            foreach ($n in $names | Select-Object -Skip 1) { $doc.SelectItem($doc.Folder.ParseName($n), 1) }
        }
        $e1 = @(); $e100 = @()
        foreach ($it in $targets) {
            ExpSelect @([IO.Path]::GetFileName($it.path)); Front; Start-Sleep -Milliseconds 400
            $e1 += MenuKey $epid; Esc; Wait-NoMenu $epid
        }
        $names100 = @($sel100 | ForEach-Object { [IO.Path]::GetFileName($_.path) } | Where-Object { $_ })
        foreach ($k in 0..2) {
            ExpSelect $names100; Front; Start-Sleep -Milliseconds 400
            $e100 += MenuKey $epid; Esc; Wait-NoMenu $epid
        }
        Note 'explorer.menuKeyMs' ([ordered]@{ oneItem = $e1; manyItems = $e100; selectedMany = $names100.Count; medianOne = (Median $e1); medianMany = (Median $e100) })
        $ew.Quit()
        [void][TestInput]::Allowed.Remove($epid)
    }
    else { Note 'explorer.menuKeyMs' 'Explorer window for the fixture not found' }
    $script:front = $win; Front

    # File change on disk -> item shown (scene written after the frame that shows it).
    $fs = @()
    foreach ($k in 1..3) {
        $name = "zz-perf-new-$k.txt"; $path = Join-Path $fx $name; $created += $path
        $sw = [Diagnostics.Stopwatch]::StartNew()
        [IO.File]::WriteAllText($path, 'perf')
        while ($sw.ElapsedMilliseconds -lt 5000) { try { if ([IO.File]::ReadAllText($scene).Contains($name)) { break } } catch {}; Start-Sleep -Milliseconds 2 }
        $fs += [math]::Round($sw.Elapsed.TotalMilliseconds, 1)
        Start-Sleep -Milliseconds 400
    }
    $ren = @()
    foreach ($k in 1..3) {
        $old = Join-Path $fx "zz-perf-new-$k.txt"; $newName = "zz-perf-renamed-$k.txt"; $new = Join-Path $fx $newName; $created += $new
        $sw = [Diagnostics.Stopwatch]::StartNew()
        [IO.File]::Move($old, $new)
        while ($sw.ElapsedMilliseconds -lt 5000) { try { if ([IO.File]::ReadAllText($scene).Contains($newName)) { break } } catch {}; Start-Sleep -Milliseconds 2 }
        $ren += [math]::Round($sw.Elapsed.TotalMilliseconds, 1)
        Start-Sleep -Milliseconds 400
    }
    Note 'fileChange' ([ordered]@{ createdMs = $fs; renamedMs = $ren; median = (Median ($fs + $ren)) })
    foreach ($f in $created) { Remove-Item -LiteralPath $f -ErrorAction SilentlyContinue }
    Start-Sleep -Milliseconds 500
    Note 'n1000.interactive.exit' (Stop-Core $p)
    $si = Get-Content $st -Raw | ConvertFrom-Json
    Note 'n1000.interactive.stats' ([ordered]@{ privateMb = $si.private_mb; workingSetMb = $si.working_set_mb; drawCount = $si.draw_count; drawMsP50 = $si.draw_ms_p50; drawMsP95 = $si.draw_ms_p95; drawMsMax = $si.draw_ms_max; hostReadyMs = $si.host_ready_ms })

    # ---- 2,000 items: rubber band ---------------------------------------------------------------
    New-Fixture 2000
    $cold2 = Run-Timed 'cold2000' 2500
    Note 'n2000.cold' ([ordered]@{ firstPaintMs = $cold2.first_paint_ms; reconcileMs = $cold2.reconcile_ms; items = $cold2.items; privateMb = $cold2.private_mb })
    $dc2 = Run-Desktop 'desk2000-cold'; $dw2 = Run-Desktop 'desk2000-warm'
    Note 'n2000.desktopMode' ([ordered]@{ cold = $dc2; warm = $dw2 })
    Seed-Layout 1200
    $st = Join-Path $prof 'rubber.json'
    $p = Start-Core @('--stats-out', $st, '--no-host')
    $s = Wait-Scene { param($s) $s.boxes.Count -eq 2 } 15000
    Start-Sleep -Seconds 2
    [TestInput]::Allowed.Clear(); [void][TestInput]::Allowed.Add($p.Id)
    $win = Win $p; $script:front = $win; [TestInput]::Focus($win) | Out-Null; Start-Sleep -Milliseconds 300
    $wr = WinRect $win
    $x0 = [int]($wr.Left + 13 * 75 + 37)
    foreach ($k in 0..2) {
        # From the free column across all 120 loose items to the top-left, and back down.
        Front
        [TestInput]::Drag($x0, [int]($wr.Top + 980), [int]($wr.Left + 8), [int]($wr.Top + 8 + $k * 20), 0, 90)
        Start-Sleep -Milliseconds 300
        Click @($x0, [int]($wr.Top + 300))
    }
    $s = Get-Scene
    Note 'n2000.items' $s.items.Count
    Note 'n2000.rubber.exit' (Stop-Core $p)
    $sr = Get-Content $st -Raw | ConvertFrom-Json
    Note 'n2000.rubber' ([ordered]@{ frames = $sr.drag_frames; drawMsP50 = $sr.drag_draw_ms_p50; drawMsP95 = $sr.drag_draw_ms_p95; drawMsMax = $sr.drag_draw_ms_max; intervalMsP50 = $sr.drag_interval_ms_p50; intervalMsP95 = $sr.drag_interval_ms_p95; intervalMsMax = $sr.drag_interval_ms_max; privateMb = $sr.private_mb })
}
finally {
    [TestInput]::Allowed.Clear(); [void][TestInput]::Allowed.Add(-1)   # refuse all input from here on
    Get-Process deskboxes, deskboxes-shellhost -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    foreach ($f in $created) { Remove-Item -LiteralPath $f -ErrorAction SilentlyContinue }
    if (-not $KeepFixture -and (Test-Path "$fx.fixture.json")) { & $gen remove $fx | Write-Host }
    Remove-Item $prof -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item $dprof -Recurse -Force -ErrorAction SilentlyContinue
}
$results | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $outDir "perf-$Config.json") -Encoding utf8
"saved docs\perf\perf-$Config.json"
