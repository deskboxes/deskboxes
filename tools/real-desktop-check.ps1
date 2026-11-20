# real-desktop-check.ps1: a SHORT run of deskboxes.exe on the real desktop (Progman child surface,
# native icons hidden), cold (no snapshot) then warm (snapshot). Each run exits by itself
# (--exit-after) and restores icons; afterwards --restore-icons is run and the state is verified
# by query (registry HideIcons, folder-view NOICONS flag, list view visibility). No screenshots:
# the real desktop shows the user's own content.
# A "gap" sample = Explorer's list view hidden while no visible Deskboxes surface exists
# (an empty desktop). Sampling is ~every 20 ms, so gaps shorter than that can be missed.
param([string]$Config = "debug")
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$bin = Join-Path $root "target\$Config"
$runas = Join-Path $root "target\debug\run-as-user.exe"
$work = Join-Path $env:TEMP "dbx-real"
$prof = Join-Path $work "profile"
New-Item -ItemType Directory -Force $work | Out-Null

Add-Type -Namespace RdChk -Name W -MemberDefinition @'
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string n);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowExW(IntPtr p, IntPtr a, string c, string n);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
[DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
'@
$N = [NullString]::Value
function ListView {
    $pm = [RdChk.W]::FindWindowW("Progman", $N)
    $dv = [RdChk.W]::FindWindowExW($pm, [IntPtr]::Zero, "SHELLDLL_DefView", $N)
    if ($dv -eq [IntPtr]::Zero) {
        $w = [IntPtr]::Zero
        while (($w = [RdChk.W]::FindWindowExW([IntPtr]::Zero, $w, "WorkerW", $N)) -ne [IntPtr]::Zero) {
            $dv = [RdChk.W]::FindWindowExW($w, [IntPtr]::Zero, "SHELLDLL_DefView", $N)
            if ($dv -ne [IntPtr]::Zero) { break }
        }
    }
    [RdChk.W]::FindWindowExW($dv, [IntPtr]::Zero, "SysListView32", $N)
}
function Surface {
    $pm = [RdChk.W]::FindWindowW("Progman", $N)
    [RdChk.W]::FindWindowExW($pm, [IntPtr]::Zero, "DeskboxesSurface", $N)
}
function State { (& (Join-Path $bin "spike-hide-icons.exe") state 2>&1) -join " " }

function Run-Once([string]$name) {
    $stats = Join-Path $work "$name-stats.json"
    Remove-Item $stats -ErrorAction SilentlyContinue
    & $runas --no-wait -- (Join-Path $bin "deskboxes.exe") --profile $prof --no-host --exit-after 3000 --stats-out $stats | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $samples = 0; $gaps = 0; $hiddenSeen = $false; $surfSeen = $false; $firstHiddenMs = $null; $firstSurfMs = $null
    $started = $false
    while ($sw.ElapsedMilliseconds -lt 40000) {
        $running = [bool](Get-Process deskboxes -ErrorAction SilentlyContinue)
        if ($running) { $started = $true } elseif ($started) { break }
        $lvVis = [RdChk.W]::IsWindowVisible((ListView))
        $s = Surface
        $sVis = ($s -ne [IntPtr]::Zero) -and [RdChk.W]::IsWindowVisible($s)
        if ($sVis -and -not $surfSeen) { $surfSeen = $true; $firstSurfMs = $sw.ElapsedMilliseconds }
        if (-not $lvVis -and -not $hiddenSeen) { $hiddenSeen = $true; $firstHiddenMs = $sw.ElapsedMilliseconds }
        if ($running -and -not $lvVis -and -not $sVis) { $gaps++ }
        $samples++
        Start-Sleep -Milliseconds 20
    }
    $st = if (Test-Path $stats) { Get-Content $stats -Raw | ConvertFrom-Json } else { $null }
    [pscustomobject]@{
        run = $name; samples = $samples; emptyDesktopSamples = $gaps
        surfaceSeenMs = $firstSurfMs; nativeHiddenSeenMs = $firstHiddenMs
        firstPaintMs = $st.first_paint_ms; hideMs = $st.hide_ms; reconcileMs = $st.reconcile_ms
        items = $st.items; snapshotItems = $st.snapshot_items; workingSetMb = $st.working_set_mb; privateMb = $st.private_mb
        afterExit = State
    }
}

if (Get-Process deskboxes, deskboxes-watchdog -ErrorAction SilentlyContinue) { throw "a Deskboxes process is already running" }
"before:  $(State)"
try {
    Run-Once "cold" | Format-List | Out-String -Width 200
    Run-Once "warm" | Format-List | Out-String -Width 200
}
finally {
    $r = & $runas -- (Join-Path $bin "deskboxes.exe") --restore-icons
    Get-Process deskboxes, deskboxes-shellhost, deskboxes-watchdog -ErrorAction SilentlyContinue | Stop-Process -Force
    "restore: $r"
    "after:   $(State)"
    "listview visible: $([RdChk.W]::IsWindowVisible((ListView)))  surface left: $((Surface) -ne [IntPtr]::Zero)"
    "processes left: $(@(Get-Process deskboxes, deskboxes-shellhost, deskboxes-watchdog -ErrorAction SilentlyContinue).Count)"
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
