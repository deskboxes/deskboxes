# menu-key-check.ps1: regression check for the shell host crash after a menu opened with the
# Menu key was closed (2026-11-19). Rounds of: select an item, Menu key, Esc, right-click another
# item; the host must stay alive and every menu must open. Input is guarded (TestInput.Allowed).
param([ValidateSet('release', 'debug')][string]$Config = 'release', [int]$Rounds = 3)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Exe = Join-Path $root "target\$Config\deskboxes.exe"
$runAs = Join-Path $root 'target\debug\run-as-user.exe'
$gen = Join-Path $root 'target\debug\fixture-gen.exe'
$fx = Join-Path $env:TEMP 'dbx-mk'; $prof = Join-Path $env:TEMP 'dbx-mk-profile'; $scene = Join-Path $prof 'scene.json'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type -Path (Join-Path $PSScriptRoot 'lib\TestInput.cs')
. (Join-Path $PSScriptRoot 'lib\MenuUia.ps1')
# A popup can close between listing and reading it: that one is not an open menu.
function Menus($hp) { @([TestInput]::TopLevel($hp, 'Microsoft.UI.Content.PopupWindowSiteBridge') | Where-Object { try { $UIA::FromHandle($_).Current.BoundingRectangle.Width -gt 100 } catch { $false } }).Count }
function WaitMenu($hp, [bool]$open, [int]$ms = 4000) { $sw = [Diagnostics.Stopwatch]::StartNew(); while ($sw.ElapsedMilliseconds -lt $ms) { if (((Menus $hp) -gt 0) -eq $open) { return $true }; Start-Sleep -Milliseconds 40 }; $false }
if (Get-Process deskboxes -ErrorAction SilentlyContinue) { throw 'a Deskboxes process is already running' }
& $gen create $fx 60 | Out-Null
Remove-Item $prof -Recurse -Force -ErrorAction SilentlyContinue
$res = [ordered]@{}
try {
    & $runAs --no-wait -- $Exe --desktop-dir $fx --window --profile $prof --scene-out $scene --no-ui-ops | Out-Null
    Start-Sleep -Seconds 4
    $p = Get-Process deskboxes | Select-Object -First 1
    $hp = Get-Process deskboxes-shellhost | Select-Object -First 1
    [TestInput]::Allowed.Clear(); [void][TestInput]::Allowed.Add($p.Id); [void][TestInput]::Allowed.Add($hp.Id)
    $win = [TestInput]::TopLevel($p.Id, 'DeskboxesTestWindow') | Select-Object -First 1
    $s = Get-Content $scene -Raw -Encoding utf8 | ConvertFrom-Json
    $its = @($s.items | Where-Object { $_.path -like '*.txt' -or $_.path -like '*.pdf' } | Select-Object -First ($Rounds * 2))
    function Ctr($it) { @([int]($it.rect[0] + $it.rect[2] / 2), [int]($it.rect[1] + 20)) }
    for ($r = 0; $r -lt $Rounds; $r++) {
        [TestInput]::Focus($win) | Out-Null; Start-Sleep -Milliseconds 300
        $a = Ctr $its[2 * $r]; [TestInput]::Click($a[0], $a[1], 0); Start-Sleep -Milliseconds 300
        [TestInput]::Keys(@(0x5D, 0x805D)) | Out-Null
        $k = WaitMenu $hp.Id $true
        Start-Sleep -Milliseconds 600   # KeyTips shown
        [TestInput]::Keys(@(0x1B, 0x801B)) | Out-Null
        $closed = WaitMenu $hp.Id $false
        Start-Sleep -Milliseconds 1500
        $alive = -not $hp.HasExited
        [TestInput]::Focus($win) | Out-Null; Start-Sleep -Milliseconds 300
        $b = Ctr $its[2 * $r + 1]; [TestInput]::Click($b[0], $b[1], 1)
        $m = if ($alive) { WaitMenu $hp.Id $true } else { $false }
        if ($m) { [TestInput]::Keys(@(0x1B, 0x801B)) | Out-Null; WaitMenu $hp.Id $false | Out-Null }
        $res["round$r"] = [ordered]@{ menuKeyOpened = $k; closed = $closed; hostAliveAfter = $alive; rightClickOpened = $m }
        Write-Host ("round {0}: {1}" -f $r, ($res["round$r"] | ConvertTo-Json -Compress))
    }
    $res.hostAliveAtEnd = -not $hp.HasExited
}
finally {
    [TestInput]::Allowed.Clear(); [void][TestInput]::Allowed.Add(-1)
    $core = [TestInput]::FindWindowW('DeskboxesCore', [NullString]::Value)
    if ($core -ne [IntPtr]::Zero) { [TestInput]::PostMessageW($core, 0x10, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null; Start-Sleep -Seconds 2 }
    Get-Process deskboxes, deskboxes-shellhost -ErrorAction SilentlyContinue | Stop-Process -Force
    & $gen remove $fx | Out-Null
    Remove-Item $prof -Recurse -Force -ErrorAction SilentlyContinue
}
$res | ConvertTo-Json -Depth 4
