<#
.SYNOPSIS
  Opens Deskboxes' OWN context menu view (deskboxes-shellhost --show-menu) for each fixture
  case, reads it with UI Automation exactly like tools/menu-probe.ps1 reads Explorer's, captures
  a PNG, closes it with Escape, and compares with docs/fidelity/explorer/menus.json.

  - Runs the host NON-elevated (via tools/run-as-user when this shell is elevated).
  - Touches only the host's own windows and fixture files in %TEMP%; nothing is invoked.
  - Output: docs/fidelity/deskboxes-view/<case>.json + .png, and a summary table.
#>
param(
    [string]$Host_ = (Join-Path $PSScriptRoot '..\out\Debug\shellhost\deskboxes-shellhost.exe'),
    [string]$Truth = (Join-Path $PSScriptRoot '..\docs\fidelity\explorer\menus.json'),
    [string]$OutDir = (Join-Path $PSScriptRoot '..\docs\fidelity\deskboxes-view'),
    [string[]]$Only,
    [int]$X = 900, [int]$Y = 300,
    [switch]$Keyboard,
    [string[]]$Expand,          # AutomationIds of submenus to open and record
    [switch]$Quiet
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing
Add-Type -Path (Join-Path $PSScriptRoot 'lib\TestInput.cs')
. (Join-Path $PSScriptRoot 'lib\MenuUia.ps1')
. (Join-Path $PSScriptRoot 'lib\MenuFixtures.ps1')
. (Join-Path $PSScriptRoot 'lib\MenuCompare.ps1')
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$Host_ = (Resolve-Path $Host_).Path
$runAs = (Resolve-Path (Join-Path $PSScriptRoot '..\target\debug\run-as-user.exe')).Path
$elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

$fx = Join-Path ([IO.Path]::GetTempPath()) 'deskboxes-menuprobe'
New-MenuFixtures $fx
$cases = [ordered]@{}
foreach ($k in $MenuCases.Keys) { $cases[$k] = @('--items') + @($MenuCases[$k] | ForEach-Object { Join-Path $fx $_ }) }
$cases['folder_background'] = @('--background', '--folder', $fx)
$cases['desktop_background'] = @('--background')
$truthByCase = @{}
if (Test-Path $Truth) { foreach ($t in (Get-Content $Truth -Raw -Encoding utf8 | ConvertFrom-Json)) { $truthByCase[$t.case] = $t } }
foreach ($t in $truthByCase.Values) {
    if ($t.case -like 'desktop_*' -and $t.items -and -not @($t.items | Where-Object { -not (Test-Path -LiteralPath $_) })) {
        $cases[$t.case] = @('--items') + @($t.items)
    }
}

# Hosts running already (the real Deskboxes' menus) are left alone; only the ones this starts go.
$otherHosts = @(Get-Process deskboxes-shellhost -ErrorAction SilentlyContinue | ForEach-Object Id)
function Our-Hosts { Get-Process deskboxes-shellhost -ErrorAction SilentlyContinue | Where-Object { $otherHosts -notcontains $_.Id } }
function Stop-Hosts { Our-Hosts | Stop-Process -Force -ErrorAction SilentlyContinue }

$summary = @()
try {
foreach ($k in $cases.Keys) {
    if ($Only -and $k -notin $Only) { continue }
    Stop-Hosts
    $args_ = @('--show-menu', '--at', $X, $Y) + $(if ($Keyboard) { @('--keyboard') } else { @() }) + $cases[$k]
    $sw = [Diagnostics.Stopwatch]::StartNew()
    if ($elevated) { & $runAs --no-wait -- $Host_ @args_ | Out-Null } else { Start-Process $Host_ -ArgumentList $args_ | Out-Null }
    $p = $null
    while (-not $p -and $sw.ElapsedMilliseconds -lt 5000) { Start-Sleep -Milliseconds 30; $p = Our-Hosts | Select-Object -First 1 }
    if (-not $p) { Write-Warning "[$k] host did not start"; continue }
    $wins = @()
    while (-not $wins -and $sw.ElapsedMilliseconds -lt 8000) { Start-Sleep -Milliseconds 40; $wins = Get-MenuWindows $p.Id }
    $appeared = $sw.ElapsedMilliseconds
    if (-not $wins) { Write-Warning "[$k] no menu window appeared"; Stop-Hosts; $summary += [pscustomobject]@{ Case = $k; Row = 'NO MENU' }; continue }
    Start-Sleep -Milliseconds 500 # entrance animation
    $wins = Get-MenuWindows $p.Id
    $m = Read-Menu $wins
    $subs = if ($Expand) { Expand-Submenus $p.Id $m $Expand (Join-Path $OutDir $k) } else { [ordered]@{} }
    $l = ($wins | ForEach-Object { $_.Current.BoundingRectangle.X } | Measure-Object -Minimum).Minimum
    $t_ = ($wins | ForEach-Object { $_.Current.BoundingRectangle.Y } | Measure-Object -Minimum).Minimum
    $rr = ($wins | ForEach-Object { $_.Current.BoundingRectangle.Right } | Measure-Object -Maximum).Maximum
    $bb = ($wins | ForEach-Object { $_.Current.BoundingRectangle.Bottom } | Measure-Object -Maximum).Maximum
    Save-Capture ([System.Windows.Rect]::new($l, $t_, $rr - $l, $bb - $t_)) (Join-Path $OutDir "$k.png")
    # Close with Escape (keyboard focus is in our flyout); the host exits when the menu closes.
    [TestInput]::Focus($wins[0].Current.NativeWindowHandle) | Out-Null
    [TestInput]::Keys(0x1B, 0x801B) | Out-Null
    $exited = $p.WaitForExit(3000)
    if (-not $exited) { Write-Warning "[$k] host did not exit on Escape; killed"; Stop-Hosts }
    $res = [pscustomobject]@{ case = $k; appearedMs = $appeared; rowPosition = $m.rowPosition; row = $m.row; list = $m.list; escapeClosed = $exited }
    if ($subs.Count) { $res | Add-Member submenus ([pscustomobject]$subs) }
    Write-TextRetry (Join-Path $OutDir "$k.json") ($res | ConvertTo-Json -Depth 6)

    $t = $truthByCase[$k]
    if (-not $t) { $summary += [pscustomobject]@{ Case = $k; Row = 'n/a'; List = 'n/a'; RowPos = $m.rowPosition; AppearedMs = $appeared; Esc = $exited }; continue }
    $r = Compare-Seq 'row' $m.row $t.row
    $exp = Get-ExpectedList $k $t
    $lc = Compare-Seq 'list' $m.list $exp.list
    $subState = '-'
    if ($subs.Count -and $t.submenus) {
        $subState = 'same'
        foreach ($sp in $subs.Keys) {
            $tw = $t.submenus.$sp
            if ($null -eq $tw) { continue }
            if ((@($tw | ForEach-Object { $_.name }) -join "`n") -ne (@($subs[$sp] | ForEach-Object { $_.name }) -join "`n")) { $subState = 'DIFF'; if (-not $Quiet) { Write-Host "--- $k $sp >: Windows [$(@($tw.name) -join ', ')] vs ours [$(@($subs[$sp].name) -join ', ')]" -ForegroundColor Yellow } }
        }
    }
    $summary += [pscustomobject]@{ Case = $k; Row = $(if ($r.Equal) { 'same' } else { 'DIFF' }); List = $(if (-not $lc.Equal) { 'DIFF' } elseif ($exp.filtered) { 'same*' } else { 'same' }); Submenus = $subState; RowPos = "$($m.rowPosition)/$($t.rowPosition)"; AppearedMs = $appeared; Esc = $exited }
    if (-not $Quiet) {
        foreach ($cmp in @($r, $lc)) {
            if ($cmp.Equal) { continue }
            Write-Host "--- $k $($cmp.What): Windows  vs  Deskboxes view" -ForegroundColor Yellow
            $n = [math]::Max($cmp.Windows.Count, $cmp.Ours.Count)
            for ($i = 0; $i -lt $n; $i++) {
                $w = if ($i -lt $cmp.Windows.Count) { $cmp.Windows[$i] } else { '' }
                $o = if ($i -lt $cmp.Ours.Count) { $cmp.Ours[$i] } else { '' }
                '{0} {1,-62} {2}' -f $(if ($w -eq $o) { '  ' } else { '!!' }), $w, $o
            }
        }
    }
}
} finally {
    Stop-Hosts
    Remove-Item $fx -Recurse -Force -ErrorAction SilentlyContinue
}
$summary | Format-Table -AutoSize | Out-String | Write-Host
Write-Host "RowPos = ours/Windows. AppearedMs includes process start + XAML init (not the menu latency target; see the host log)."
