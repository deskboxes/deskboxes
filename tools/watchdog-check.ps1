# watchdog-check.ps1: exercises deskboxes-watchdog.exe against a fixture core in window mode.
#   crash  -> icons restored, core restarted
#   hang   -> (core suspended) detected by two WM_NULL timeouts, core killed and restarted
#   3 restarts in 60 s, then one more failure -> watchdog gives up, exits
#   deskboxes --restore-icons -> watchdog and core both stop, no restart
# Runs the app non-elevated through run-as-user.exe. Leaves no processes or fixture files behind.
param([string]$Config = "debug")
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$bin = Join-Path $root "target\$Config"
$runas = Join-Path $root "target\debug\run-as-user.exe"
$fx = Join-Path $env:TEMP "dbx-wd"
$prof = Join-Path $env:TEMP "dbx-wd-profile"
$log = Join-Path $env:LOCALAPPDATA "Deskboxes\logs\watchdog.log"
$results = [ordered]@{}

Add-Type -Namespace WdChk -Name N -MemberDefinition @'
[DllImport("ntdll.dll")] public static extern int NtSuspendProcess(IntPtr h);
[DllImport("ntdll.dll")] public static extern int NtResumeProcess(IntPtr h);
'@

function Core-Pids { @(Get-Process deskboxes -ErrorAction SilentlyContinue | ForEach-Object Id) }
function Wd-Pids { @(Get-Process deskboxes-watchdog -ErrorAction SilentlyContinue | ForEach-Object Id) }
function Wait-NewCore([int[]]$old, [int]$ms) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) {
        $n = Core-Pids | Where-Object { $old -notcontains $_ }
        if ($n) { return @($n)[0] }
        Start-Sleep -Milliseconds 200
    }
    return $null
}
function Log-Since([long]$offset) {
    if (-not (Test-Path $log)) { return "" }
    $all = [IO.File]::ReadAllText($log)
    if ($offset -gt $all.Length) { $offset = 0 }
    return $all.Substring([int]$offset)
}
function Start-Watchdog {
    & $runas --no-wait -- (Join-Path $bin "deskboxes-watchdog.exe") --launch --desktop-dir $fx --window --no-host --no-ui-ops --profile $prof | Out-Null
}

if ((Core-Pids) -or (Wd-Pids)) { throw "a Deskboxes process is already running" }
& (Join-Path $root "target\debug\fixture-gen.exe") create $fx 50 | Out-Host
$logStart = if (Test-Path $log) { (Get-Item $log).Length } else { 0 }
try {
    # A. crash
    Start-Watchdog
    $p1 = Wait-NewCore @() 15000
    if (-not $p1) { throw "core did not start" }
    Start-Sleep -Seconds 2
    Stop-Process -Id $p1 -Force
    $p2 = Wait-NewCore @($p1) 10000
    $results.crashRestart = [bool]$p2

    # B. hang: suspend the core once it has answered a check (before, it counts as starting); the
    # watchdog must see WM_NULL time out twice in a row (3 s period, 6 s timeout)
    Start-Sleep -Seconds 5
    $h = (Get-Process -Id $p2).Handle
    [void][WdChk.N]::NtSuspendProcess($h)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p3 = Wait-NewCore @($p1, $p2) 30000
    $results.hangRestart = [bool]$p3
    $results.hangDetectMs = $sw.ElapsedMilliseconds
    if (Get-Process -Id $p2 -ErrorAction SilentlyContinue) { [void][WdChk.N]::NtResumeProcess($h); Stop-Process -Id $p2 -Force }

    # C. third failure -> 4th start; D. fourth failure -> give up
    Start-Sleep -Seconds 2
    Stop-Process -Id $p3 -Force
    $p4 = Wait-NewCore @($p1, $p2, $p3) 10000
    $results.thirdRestart = [bool]$p4
    Start-Sleep -Seconds 2
    Stop-Process -Id $p4 -Force
    Start-Sleep -Seconds 3
    $results.gaveUp = (-not (Wd-Pids)) -and (-not (Core-Pids))
    $text = Log-Since $logStart
    $results.logCrash = $text -match "restoring icons, restarting"
    $results.logHang = $text -match "core hung"
    $results.logGaveUp = $text -match "giving up"

    # E. --restore-icons stops both, no restart
    $mark = (Get-Item $log).Length
    Start-Watchdog
    $p5 = Wait-NewCore @() 15000
    Start-Sleep -Seconds 2
    $out = & $runas -- (Join-Path $bin "deskboxes.exe") --restore-icons
    Start-Sleep -Seconds 4
    $results.restoreIconsStopsAll = [bool]$p5 -and (-not (Wd-Pids)) -and (-not (Core-Pids))
    $results.logStopRequested = (Log-Since $mark) -match "stop requested"
}
finally {
    Get-Process deskboxes-watchdog, deskboxes -ErrorAction SilentlyContinue | ForEach-Object {
        try { [void][WdChk.N]::NtResumeProcess($_.Handle) } catch {}
        Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
    }
    & (Join-Path $root "target\debug\fixture-gen.exe") remove $fx | Out-Host
    Remove-Item -Recurse -Force $prof -ErrorAction SilentlyContinue
}
"--- watchdog.log (this run) ---"
Log-Since $logStart
"--- results ---"
$results.GetEnumerator() | ForEach-Object { "{0,-22} {1}" -f $_.Key, $_.Value }
