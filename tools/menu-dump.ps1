#Requires -Version 7
<#
.SYNOPSIS
  Dumps the menu Deskboxes builds (no UI) for the menu-probe fixture cases and compares it with
  what Windows itself showed (docs/fidelity/explorer/menus.json from tools/menu-probe.ps1).

.DESCRIPTION
  Deskboxes derives its menu at runtime from the machine's registrations; nothing in it is
  specific to this PC. This script is VERIFICATION only: it reports differences, it never
  feeds anything back into the menu.

  - Runs deskboxes-shellhost --dump-menu --json NON-elevated (via tools/run-as-user when this
    shell is elevated).
  - Compares row and list by (id, name, accelerator, submenu), in order.
  - Writes docs/fidelity/deskboxes/<case>.json and prints a per-case verdict.
#>
param(
    [string]$Host_ = (Join-Path $PSScriptRoot '..\out\Debug\shellhost\deskboxes-shellhost.exe'),
    [string]$Truth = (Join-Path $PSScriptRoot '..\docs\fidelity\explorer\menus.json'),
    [string]$OutDir = (Join-Path $PSScriptRoot '..\docs\fidelity\deskboxes'),
    [string[]]$Only,
    [switch]$Quiet
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'lib\MenuFixtures.ps1')
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
# Real-desktop cases recorded by menu-probe -DesktopItems carry the item paths they used.
foreach ($t in $truthByCase.Values) {
    if ($t.case -like 'desktop_*' -and $t.items) {
        if (@($t.items | Where-Object { -not (Test-Path -LiteralPath $_) })) { Write-Warning "$($t.case): recorded item missing; skipped"; continue }
        $cases[$t.case] = @('--items') + @($t.items)
    }
}

. (Join-Path $PSScriptRoot 'lib\MenuCompare.ps1')

$summary = @()
foreach ($k in $cases.Keys) {
    if ($Only -and $k -notin $Only) { continue }
    $out = Join-Path $OutDir "$k.json"
    $args_ = @('--dump-menu', '--json', '--out', $out) + $cases[$k]
    if ($elevated) { & $runAs -- $Host_ @args_ | Out-Null } else { & $Host_ @args_ | Out-Null }
    if (-not (Test-Path $out)) { Write-Warning "${k}: no output"; continue }
    $ours = Get-Content $out -Raw -Encoding utf8 | ConvertFrom-Json
    $t = $truthByCase[$k]
    if (-not $t) { $summary += [pscustomobject]@{ Case = $k; Row = 'n/a'; List = 'n/a'; WarmMs = [math]::Round($ours.msTotal, 1); ColdMs = $ours.coldMs; Elevated = $ours.elevated }; continue }
    $r = Compare-Seq 'row' $ours.row $t.row
    $exp = Get-ExpectedList $k $t
    $tl = $exp.list; $browserNote = $exp.filtered
    $l = Compare-Seq 'list' $ours.list $tl
    $listState = if (-not $l.Equal) { 'DIFF' } elseif ($browserNote) { 'same*' } else { 'same' }
    # Submenus Windows' probe opened (-Expand): compare child names in order.
    $subs = @(); $subState = '-'
    if ($t.submenus) {
        $subState = 'same'
        foreach ($p in $t.submenus.PSObject.Properties) {
            $mine = $ours.list | Where-Object { $_.id -eq $p.Name } | Select-Object -First 1
            if (-not $mine) { continue }
            if (-not $mine.children) { $subs += [pscustomobject]@{ What = "$($p.Name) >"; Equal = $true; Windows = @(); Ours = @(); Note = 'children built on open (not in dump)' }; continue }
            $w = @($p.Value | ForEach-Object { $_.name }); $o = @($mine.children | Where-Object { $_.id -ne 'separator' } | ForEach-Object { $_.name })
            $eq = ($w -join "`n") -eq ($o -join "`n")
            if (-not $eq) { $subState = 'DIFF' }
            $subs += [pscustomobject]@{ What = "$($p.Name) >"; Equal = $eq; Windows = $w; Ours = $o }
        }
    }
    $summary += [pscustomobject]@{ Case = $k; Row = $(if ($r.Equal) { 'same' } else { 'DIFF' }); List = $listState; Submenus = $subState; WarmMs = [math]::Round($ours.msTotal, 1); ColdMs = $ours.coldMs; Elevated = $ours.elevated }
    if (-not $Quiet) {
        foreach ($x in @($r, $l) + $subs) {
            if ($x.Equal) { continue }
            Write-Host "--- $k $($x.What): Windows  vs  Deskboxes" -ForegroundColor Yellow
            $n = [math]::Max($x.Windows.Count, $x.Ours.Count)
            for ($i = 0; $i -lt $n; $i++) {
                $w = if ($i -lt $x.Windows.Count) { $x.Windows[$i] } else { '' }
                $o = if ($i -lt $x.Ours.Count) { $x.Ours[$i] } else { '' }
                $mark = if ($w -eq $o) { '  ' } else { '!!' }
                '{0} {1,-62} {2}' -f $mark, $w, $o
            }
        }
    }
}
$summary | Format-Table -AutoSize | Out-String | Write-Host
Write-Host "same* = equal after removing entries that exist only in Explorer folder windows (OnlyInBrowserWindow verbs, the folder view's own View/Sort by/Group by/Refresh)."
$summary | ConvertTo-Json | Set-Content (Join-Path $OutDir 'summary.json') -Encoding utf8
Remove-Item $fx -Recurse -Force -ErrorAction SilentlyContinue
