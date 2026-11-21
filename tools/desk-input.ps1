# desk-input.ps1: guarded input on the real desktop for checking Deskboxes by eye during
# development. Every click must land on a Deskboxes window (core surface or its menu host).
#   desk-input.ps1 rclick 'X,Y' | click 'X,Y' | wheel 'X,Y,NOTCHES' | cwheel 'X,Y,NOTCHES' (Ctrl held) | dclick X Y | drag X0 Y0 X1 Y1 | key VK[,VK...] | esc
# A press-move-release stays in the process it started in: a drag from a test window that ends
# over the real desktop is a real drop (it moved two test files into the Desktop folder once,
# 2026-11-19). -Cross allows it (for drag images over other apps; cancel those with esc).
# -Only PID: every press, click and wheel must land in that process (a test window): when the
# window is minimized or covered, the input is refused instead of reaching the real desktop.
param([Parameter(Mandatory)][string]$Op, [string]$Args1 = '', [int]$AlsoPid = 0, [switch]$Cross, [int]$Only = 0)
$A = @($Args1 -split '[ ,]+' | Where-Object { $_ } | ForEach-Object { [int]$_ })
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type -Path (Join-Path $PSScriptRoot 'lib\TestInput.cs')
Add-Type -Namespace Dpi -Name A -MemberDefinition '[DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(System.IntPtr v);'
[void][Dpi.A]::SetProcessDpiAwarenessContext([IntPtr]-4)
# Every core (the real one and a test instance of ours can both run).
$cores = @(Get-Process deskboxes -ErrorAction Stop)
[TestInput]::Allowed.Clear(); foreach ($c in $cores) { [void][TestInput]::Allowed.Add($c.Id) }
Get-Process deskboxes-shellhost -ErrorAction SilentlyContinue | ForEach-Object { [void][TestInput]::Allowed.Add($_.Id) }
if ($AlsoPid) { [void][TestInput]::Allowed.Add($AlsoPid) }  # a test window of ours (drop target)
$held = Join-Path $env:TEMP 'desk-input-held.txt'
if ($Only) {
    [void][TestInput]::Allowed.Add($Only)
    if ($Op -in 'rclick', 'click', 'wheel', 'cwheel', 'dclick', 'drag', 'rdrag', 'down', 'path', 'move', 'up' -and $A.Count -ge 2) {
        for ($i = 0; $i + 1 -lt $A.Count -and ($Op -notin 'wheel', 'cwheel' -or $i -eq 0); $i += 2) {
            $p = [TestInput]::PidAt($A[$i], $A[$i + 1])
            # Our menu host (a menu the test window opened) is fine too.
            $menu = @(Get-Process deskboxes-shellhost -ErrorAction SilentlyContinue | ForEach-Object Id)
            if ($p -ne $Only -and $p -notin $menu) { throw "refused: $($A[$i]),$($A[$i + 1]) is in pid $p, not in $Only (-Only)" }
        }
    }
}
function Assert-Same([int]$x0, [int]$y0, [int]$x1, [int]$y1) {
    if ($Cross) { return }
    $a = [TestInput]::PidAt($x0, $y0); $b = [TestInput]::PidAt($x1, $y1)
    if ($a -ne $b) { throw "refused: $x0,$y0 (pid $a) and $x1,$y1 (pid $b) are in different processes (use -Cross)" }
}
switch ($Op) {
    'rclick' { [TestInput]::Click($A[0], $A[1], 1) }
    'click' { [TestInput]::Click($A[0], $A[1], 0) }
    'wheel' { [TestInput]::Wheel($A[0], $A[1], $A[2]) }
    # Ctrl+wheel (icon size): Ctrl held around the wheel.
    'cwheel' { [void][TestInput]::Keys([uint16[]]@(0x11)); Start-Sleep -Milliseconds 30; [TestInput]::Wheel($A[0], $A[1], $A[2]); Start-Sleep -Milliseconds 30; [void][TestInput]::Keys([uint16[]]@(0x8011)) }
    'dclick' { [TestInput]::Click($A[0], $A[1], 0); Start-Sleep -Milliseconds 60; [TestInput]::Click($A[0], $A[1], 0) }
    'drag' { Assert-Same $A[0] $A[1] $A[2] $A[3]; [TestInput]::Drag($A[0], $A[1], $A[2], $A[3], 0, 30) }
    'rdrag' { Assert-Same $A[0] $A[1] $A[2] $A[3]; [TestInput]::Drag($A[0], $A[1], $A[2], $A[3], 1, 30) }
    { $_ -in 'esc', 'key' } {
        # Keys go to the foreground window: allow it only if it is ours or the desktop (Progman /
        # WorkerW, whose focused child is our surface).
        $fg = [TestInput]::GetForegroundWindow(); $cls = [TestInput]::Cls($fg)
        if (-not [TestInput]::Allowed.Contains([TestInput]::Pid($fg)) -and $cls -notin 'Progman', 'WorkerW') { throw "keys refused: foreground is $cls (pid $([TestInput]::Pid($fg)))" }
        [TestInput]::Allowed.Clear()
        $seq = if ($_ -eq 'esc') { @(0x1B, 0x801B) } else { $A }
        [void][TestInput]::Keys([uint16[]]$seq)
    }
    'move' {
        if (Test-Path $held) { $h = @((Get-Content $held) -split ',' | ForEach-Object { [int]$_ }); Assert-Same $h[0] $h[1] $A[0] $A[1] }
        [TestInput]::Move($A[0], $A[1])
    }
    # Split drag (to look at the drag feedback in between): press at X0,Y0 and move to X1,Y1 in
    # steps; 'up' moves to X,Y and releases.
    'down' {
        [TestInput]::CheckPoint($A[0], $A[1])
        if ($A.Count -ge 4) { Assert-Same $A[0] $A[1] $A[2] $A[3] }
        "$($A[0]),$($A[1])" | Set-Content $held
        [TestInput]::Move($A[0], $A[1]); Start-Sleep -Milliseconds 30; [TestInput]::Button(0, $true)
        for ($s = 1; $s -le 25; $s++) { [TestInput]::Move($A[0] + ($A[2] - $A[0]) * $s / 25, $A[1] + ($A[3] - $A[1]) * $s / 25); Start-Sleep -Milliseconds 15 }
    }
    'up' {
        if (Test-Path $held) {
            $h = @((Get-Content $held) -split ',' | ForEach-Object { [int]$_ }); Remove-Item $held
            if (-not $Cross -and [TestInput]::PidAt($h[0], $h[1]) -ne [TestInput]::PidAt($A[0], $A[1])) {
                # Never drop across processes: cancel the drag (Esc), release where it started.
                [void][TestInput]::Keys([uint16[]]@(0x1B, 0x801B)); Start-Sleep -Milliseconds 100
                [TestInput]::Move($h[0], $h[1]); Start-Sleep -Milliseconds 30; [TestInput]::Button(0, $false)
                throw "refused: release at $($A[0]),$($A[1]) is in another process than the press; drag cancelled"
            }
        }
        [TestInput]::Move($A[0], $A[1]); Start-Sleep -Milliseconds 30; [TestInput]::Button(0, $false)
    }
    'path' {
        # Press at the first point, move through the others in steps, release at the last.
        for ($i = 0; $i -lt $A.Count; $i += 2) { [TestInput]::CheckPoint($A[$i], $A[$i + 1]); Assert-Same $A[0] $A[1] $A[$i] $A[$i + 1] }
        [TestInput]::Move($A[0], $A[1]); Start-Sleep -Milliseconds 30; [TestInput]::Button(0, $true)
        for ($i = 2; $i -lt $A.Count; $i += 2) {
            $x0 = $A[$i - 2]; $y0 = $A[$i - 1]; $x1 = $A[$i]; $y1 = $A[$i + 1]
            for ($s = 1; $s -le 25; $s++) { [TestInput]::Move($x0 + ($x1 - $x0) * $s / 25, $y0 + ($y1 - $y0) * $s / 25); Start-Sleep -Milliseconds 15 }
            Start-Sleep -Milliseconds 150
        }
        [TestInput]::Button(0, $false)
    }
}
