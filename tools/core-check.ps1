<#
.SYNOPSIS
  End-to-end check of deskboxes.exe in fixture mode (--desktop-dir, --window): first paint,
  reconciliation, 2 boxes + a tab pair (seeded layout), selection (click, Ctrl+click, rubber band,
  Ctrl+A, arrows), drag into a box, undo, tab switch, persistence across restarts, context menu
  through the shell host (compared with Explorer's recorded menu), rename / delete / drop onto a
  folder through the shell (fixture files only, FOF_NO_UI).

  Runs the app NON-elevated (tools/run-as-user). Input goes only to the app's own topmost test
  window; screenshots are crops of that window. Output: docs/fidelity/core/*.png + results.json.
#>
param(
    [int]$N = 1000,
    [string]$Exe = (Join-Path $PSScriptRoot '..\target\debug\deskboxes.exe'),
    [string]$OutDir = (Join-Path $PSScriptRoot '..\docs\fidelity\core'),
    [switch]$KeepFixture
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing
Add-Type -Path (Join-Path $PSScriptRoot 'lib\TestInput.cs')
. (Join-Path $PSScriptRoot 'lib\MenuUia.ps1')
. (Join-Path $PSScriptRoot 'lib\MenuCompare.ps1')
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$Exe = (Resolve-Path $Exe).Path
$runAs = (Resolve-Path (Join-Path $PSScriptRoot '..\target\debug\run-as-user.exe')).Path
$gen = (Resolve-Path (Join-Path $PSScriptRoot '..\target\debug\fixture-gen.exe')).Path
$truth = @{}; foreach ($t in (Get-Content (Join-Path $PSScriptRoot '..\docs\fidelity\explorer\menus.json') -Raw -Encoding utf8 | ConvertFrom-Json)) { $truth[$t.case] = $t }

$fx = Join-Path $env:TEMP 'dbx-fx'; $pub = Join-Path $env:TEMP 'dbx-fx-public'
if (-not (Test-Path "$fx.fixture.json")) { & $gen create $fx $N --public $pub --dups 10 | Write-Host }
$prof = Join-Path $env:TEMP 'dbx-core-profile'
Remove-Item $prof -Recurse -Force -ErrorAction SilentlyContinue
$scene = Join-Path $prof 'scene.json'
$results = [ordered]@{}
function Note($k, $v) { $script:results[$k] = $v; Write-Host ("{0,-28} {1}" -f $k, ($v | ConvertTo-Json -Compress -Depth 4)) }

function Start-Core([string[]]$extra = @()) {
    Remove-Item $scene -ErrorAction SilentlyContinue
    $a = @('--desktop-dir', $fx, '--public-dir', $pub, '--window', '--profile', $prof, '--scene-out', $scene, '--no-ui-ops') + $extra
    & $runAs --no-wait -- $Exe @a | Out-Null
    $sw = [Diagnostics.Stopwatch]::StartNew(); $p = $null
    # Only the instance this started: the real Deskboxes may be running too and is never touched.
    while (-not $p -and $sw.ElapsedMilliseconds -lt 5000) { Start-Sleep -Milliseconds 50; $p = Get-Process deskboxes -ErrorAction SilentlyContinue | Where-Object { $script:otherPids -notcontains $_.Id } | Select-Object -First 1 }
    $p
}
# The menu host(s) of our test instance (its child processes), never the real Deskboxes' host.
function Our-Hosts($p) { @(Get-CimInstance Win32_Process -Filter "Name='deskboxes-shellhost.exe' AND ParentProcessId=$($p.Id)" | ForEach-Object { Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue }) }
function Get-Scene { for ($i = 0; $i -lt 20; $i++) { try { return (Get-Content $scene -Raw -Encoding utf8 | ConvertFrom-Json) } catch { Start-Sleep -Milliseconds 50 } }; $null }
function Wait-Scene([scriptblock]$cond, [int]$ms = 8000) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $ms) { if (Test-Path $scene) { $s = Get-Scene; if ($s -and (& $cond $s)) { return $s } }; Start-Sleep -Milliseconds 60 }
    Get-Scene
}
function Center($it) { @([int]($it.rect[0] + $it.rect[2] / 2), [int]($it.rect[1] + 20)) }
function Item($s, [string]$like) { $s.items | Where-Object { $_.name -like $like } | Select-Object -First 1 }
function Sel($s) { @($s.items | Where-Object selected) }
function Win($p) { [TestInput]::TopLevel($p.Id, 'DeskboxesTestWindow') | Select-Object -First 1 }
function Capture($p, [string]$name) {
    $h = Win $p; if (-not $h) { return }
    [TestInput]::Focus($h) | Out-Null; Start-Sleep -Milliseconds 300
    $r = $UIA::FromHandle($h).Current.BoundingRectangle
    $b = [Drawing.Bitmap]::new([int]$r.Width, [int]$r.Height); $g = [Drawing.Graphics]::FromImage($b)
    $g.CopyFromScreen([int]$r.X, [int]$r.Y, 0, 0, $b.Size); $g.Dispose(); $b.Save((Join-Path $OutDir "$name.png")); $b.Dispose()
}
# Menu capture strictly inside the menu windows (our host's windows), no margin.
function Capture-Menu($wins, [string]$name) {
    $l = ($wins | ForEach-Object { $_.Current.BoundingRectangle.X } | Measure-Object -Minimum).Minimum
    $t = ($wins | ForEach-Object { $_.Current.BoundingRectangle.Y } | Measure-Object -Minimum).Minimum
    $r = ($wins | ForEach-Object { $_.Current.BoundingRectangle.Right } | Measure-Object -Maximum).Maximum
    $b = ($wins | ForEach-Object { $_.Current.BoundingRectangle.Bottom } | Measure-Object -Maximum).Maximum
    $bm = [Drawing.Bitmap]::new([int]($r - $l), [int]($b - $t)); $g = [Drawing.Graphics]::FromImage($bm)
    $g.CopyFromScreen([int]$l, [int]$t, 0, 0, $bm.Size); $g.Dispose(); $bm.Save((Join-Path $OutDir "$name.png")); $bm.Dispose()
}
function Stop-Core($p) {
    $core = [IntPtr]::Zero
    do {
        $core = [TestInput]::FindWindowExW([IntPtr]::Zero, $core, 'DeskboxesCore', [NullString]::Value)
        if ($core -ne [IntPtr]::Zero -and [TestInput]::Pid($core) -eq $p.Id) { [TestInput]::PostMessageW($core, 0x10, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null }
    } while ($core -ne [IntPtr]::Zero)
    if (-not $p.WaitForExit(8000)) { Write-Warning 'core did not exit; killing'; $p.Kill(); return 'killed' }
    'clean' 
}
function Click($xy, [int]$btn = 0) { [TestInput]::Click($xy[0], $xy[1], $btn); Start-Sleep -Milliseconds 250 }
function Keys([uint16[]]$k) { [TestInput]::Keys($k) | Out-Null; Start-Sleep -Milliseconds 300 }
$VK = @{ CTRL = 0x11; SHIFT = 0x10; RIGHT = 0x27; F2 = 0x71; DEL = 0x2E; ENTER = 0x0D; ESC = 0x1B; A = 0x41; Z = 0x5A }
function Chord($mod, $key) { Keys @($mod, $key, ($key -bor 0x8000), ($mod -bor 0x8000)) }

$otherPids = @(Get-Process deskboxes, deskboxes-shellhost -ErrorAction SilentlyContinue | ForEach-Object Id)
if ($otherPids) { Write-Host "Deskboxes is running (pid $($otherPids -join ', ')): left alone; the check uses its own instance" }
$saveClip = $null; try { $saveClip = Get-Clipboard -Raw -ErrorAction Stop } catch {}
try {
    # ---- run 1: cold start, layout file created -------------------------------------------
    $p = Start-Core @('--stats-out', (Join-Path $prof 'stats1.json'))
    $s = Wait-Scene { param($s) $s.items.Count -gt 0 -and $s.loose -ge ($N) } 20000
    Start-Sleep -Milliseconds 1500
    Note 'run1.exit' (Stop-Core $p)
    $st1 = Get-Content (Join-Path $prof 'stats1.json') | ConvertFrom-Json
    Note 'run1.stats' $st1
    $layoutFile = Get-ChildItem (Join-Path $prof 'layouts') -Filter *.json | Select-Object -First 1
    Note 'run1.layoutFile' $layoutFile.Name

    # ---- seed: 2 boxes, the second a tab pair; loose items only where no box is -------------
    $L = Get-Content $layoutFile.FullName -Raw -Encoding utf8 | ConvertFrom-Json
    $keys = @($L.monitor.loose.PSObject.Properties.Name | Sort-Object)
    $cell = @(75, 98); $rows = 10
    $loose = [ordered]@{}; $i = 0
    foreach ($k in $keys[0..119]) { $loose[$k] = @{ col = [int][math]::Floor($i / $rows); row = $i % $rows }; $i++ }
    $style = @{ material = 'Mica'; color = $null; opacity = 0.8; blur = 60.0; title_color = $null }
    $b1 = @{ id = 10; rect = @{ x = 1110.0; y = 16.0; w = 470.0; h = 520.0 }; style = $style; tabs = @(@{ id = 11; title = 'Dokumenty'; items = @($keys[120..719]) }); active_tab = 0; rolled_up = $false }
    $b2 = @{ id = 12; rect = @{ x = 1110.0; y = 556.0; w = 470.0; h = 420.0 }; style = $style; tabs = @(@{ id = 13; title = 'Zdjęcia'; items = @($keys[720..869]) }, @{ id = 14; title = 'Muzyka'; items = @($keys[870..($keys.Count - 1)]) }); active_tab = 0; rolled_up = $false }
    $L.monitor = @{ boxes = @($b1, $b2); loose = $loose; next_id = 20 }
    [IO.File]::WriteAllText($layoutFile.FullName, ($L | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))

    # ---- run 2: interactive -------------------------------------------------------------------
    $p = Start-Core @('--stats-out', (Join-Path $prof 'stats2.json'))
    $s = Wait-Scene { param($s) $s.boxes.Count -eq 2 -and $s.items.Count -gt 100 } 20000
    Start-Sleep -Milliseconds 2500   # reconciliation
    $s = Get-Scene
    Note 'seed.boxes' @($s.boxes | ForEach-Object { "$($_.id):" + (($_.tabs | ForEach-Object { "$($_.title)=$($_.items)" }) -join '|') })
    $win = Win $p; [TestInput]::Focus($win) | Out-Null
    # Input guard: clicks and keys only reach the core, its shell host and their menus.
    [TestInput]::Allowed.Clear(); [void][TestInput]::Allowed.Add($p.Id); Our-Hosts $p | ForEach-Object { [void][TestInput]::Allowed.Add($_.Id) }
    Capture $p 'boxes'

    $looseItems = @($s.items | Where-Object { $null -eq $_.box })
    $a = $looseItems[0]; $b = $looseItems[11]
    Click (Center $a); $s = Wait-Scene { param($s) (Sel $s).Count -eq 1 } 2000
    Note 'select.click' (@(Sel $s).key -eq $a.key)
    [TestInput]::Keys(@($VK.CTRL)) | Out-Null; Click (Center $b); [TestInput]::Keys(@($VK.CTRL -bor 0x8000)) | Out-Null
    $s = Wait-Scene { param($s) (Sel $s).Count -eq 2 } 2000
    Note 'select.ctrlclick' ((Sel $s).Count -eq 2)
    $wr = $UIA::FromHandle($win).Current.BoundingRectangle
    $emptyPt = @([int]($wr.X + 13 * 75 + 37), [int]($wr.Y + 700))   # column 13 is left free by the seed
    Click $emptyPt; $s = Wait-Scene { param($s) (Sel $s).Count -eq 0 } 2000
    Note 'select.clear' ((Sel $s).Count -eq 0)
    $c = $looseItems | Where-Object { $_.rect[1] -lt ($wr.Y + 300) } | Sort-Object { $_.rect[0] } -Descending | Select-Object -First 1
    [TestInput]::Drag($emptyPt[0], [int]($wr.Y + 20), $c.rect[0] - 80, [int]($wr.Y + 260), 0, 12); Start-Sleep -Milliseconds 300
    $s = Get-Scene; Note 'select.rubberband' ((Sel $s).Count)
    Capture $p 'rubberband'
    Click (Center $a); Chord $VK.CTRL $VK.A; $s = Get-Scene
    Note 'select.ctrlA' ("{0}/{1}" -f (Sel $s).Count, $s.items.Count)
    Click $emptyPt
    Click (Center $a); Keys @($VK.RIGHT, ($VK.RIGHT -bor 0x8000)); $s = Get-Scene
    $right = Sel $s
    Note 'keyboard.right' ($right.Count -eq 1 -and $right[0].rect[0] -gt $a.rect[0] -and [math]::Abs($right[0].rect[1] - $a.rect[1]) -lt 10)

    # Drag a loose item into the first box, then undo.
    $d = $looseItems[5]; $bx = $s.boxes[0]
    $from = Center $d; $to = @([int]($bx.rect[0] + $bx.rect[2] / 2), [int]($bx.rect[1] + $bx.rect[3] - 30))
    [TestInput]::Drag($from[0], $from[1], $to[0], $to[1], 0, 15)
    $s = Wait-Scene { param($s) ($s.items | Where-Object key -eq $d.key).box -eq $bx.id } 3000
    Note 'dnd.intoBox' (($s.items | Where-Object key -eq $d.key).box -eq $bx.id)
    Capture $p 'dragged-into-box'
    Chord $VK.CTRL $VK.Z
    $s = Wait-Scene { param($s) $null -eq ($s.items | Where-Object key -eq $d.key).box } 3000
    Note 'undo' ($null -eq ($s.items | Where-Object key -eq $d.key).box)
    # Drag a loose item to an empty grid cell (column 14).
    $e = $looseItems[2]; $from = Center $e
    [TestInput]::Drag($from[0], $from[1], $emptyPt[0], [int]($wr.Y + 400), 0, 15)
    $s = Wait-Scene { param($s) ($s.items | Where-Object key -eq $e.key).rect[0] -gt ($wr.X + 13 * 75 - 5) } 3000
    Note 'dnd.grid' ((($s.items | Where-Object key -eq $e.key).rect[0]) -gt ($wr.X + 13 * 75 - 5))

    # Tab switch on the second box (tab pair).
    $b2s = $s.boxes[1]
    $t2 = $b2s.tabs[1].rect
    if ($t2) { Click @([int]($t2[0] + $t2[2] / 2), [int]($t2[1] + $t2[3] / 2)) }
    else { Click @([int]($b2s.rect[0] + $b2s.rect[2] * 3 / 4), [int]($b2s.rect[1] + 15)) }
    $s = Wait-Scene { param($s) $s.boxes[1].active -eq 1 } 2000
    Note 'tabs.switch' ($s.boxes[1].active -eq 1)
    Capture $p 'tab-switched'

    # Context menu on a .txt item through the shell host; compare with Explorer's recorded menu.
    $txt = $s.items | Where-Object { $_.path -like '*.txt' -and $null -eq $_.box } | Select-Object -First 1
    $hostP = Our-Hosts $p | Select-Object -First 1
    Click (Center $txt) 1
    $sw = [Diagnostics.Stopwatch]::StartNew(); $wins = @()
    while (-not $wins -and $sw.ElapsedMilliseconds -lt 5000) { Start-Sleep -Milliseconds 40; if ($hostP) { $wins = Get-MenuWindows $hostP.Id } }
    Note 'menu.appearedMs' $sw.ElapsedMilliseconds
    if ($wins) {
        Start-Sleep -Milliseconds 400; $wins = Get-MenuWindows $hostP.Id
        $m = Read-Menu $wins
        Capture-Menu $wins 'menu-txt'
        $exp = Get-ExpectedList 'txt' $truth['txt']
        $rc = Compare-Seq 'row' $m.row $truth['txt'].row; $lc = Compare-Seq 'list' $m.list $exp.list
        Note 'menu.txt' @{ row = $rc.Equal; list = $lc.Equal }
        if (-not $lc.Equal) { $lc.Diff | Format-Table | Out-String | Write-Host }
        [TestInput]::Focus($wins[0].Current.NativeWindowHandle) | Out-Null; Keys @($VK.ESC, ($VK.ESC -bor 0x8000))
    } else { Note 'menu.txt' 'NO MENU' }
    [TestInput]::Focus($win) | Out-Null

    # Background menu (with the "Create box here" entry), captured.
    Click @($emptyPt[0], [int]($wr.Y + 120)) 1   # high enough that the whole menu lies over our window
    Start-Sleep -Milliseconds 900
    if ($hostP) { $wins = Get-MenuWindows $hostP.Id; if ($wins) { $m = Read-Menu $wins; Capture-Menu $wins 'menu-background'; Note 'menu.background' @($m.list | ForEach-Object { $_.name }); [TestInput]::Focus($wins[0].Current.NativeWindowHandle) | Out-Null; Keys @($VK.ESC, ($VK.ESC -bor 0x8000)) } }
    [TestInput]::Focus($win) | Out-Null

    # Rename through the shell (F2, type, Enter), then back.
    $s = Get-Scene
    $r = $s.items | Where-Object { $_.path -like '*.txt' -and $null -eq $_.box } | Select-Object -Skip 1 -First 1
    $newName = 'Zmieniona nazwa ąę'
    Click (Center $r); Keys @($VK.F2, ($VK.F2 -bor 0x8000)); Start-Sleep -Milliseconds 300
    Add-Type -AssemblyName System.Windows.Forms
    [System.Windows.Forms.SendKeys]::SendWait($newName); Keys @($VK.ENTER, ($VK.ENTER -bor 0x8000))
    $newPath = Join-Path (Split-Path $r.path) "$newName.txt"
    $sw = [Diagnostics.Stopwatch]::StartNew(); while (-not (Test-Path -LiteralPath $newPath) -and $sw.ElapsedMilliseconds -lt 4000) { Start-Sleep -Milliseconds 100 }
    $renamed = Test-Path -LiteralPath $newPath
    $s = Wait-Scene { param($s) [bool]($s.items | Where-Object path -eq $newPath) } 4000
    Note 'fileop.rename' @{ onDisk = $renamed; inScene = [bool]($s.items | Where-Object path -eq $newPath); sameKey = (($s.items | Where-Object path -eq $newPath).key -eq $r.key) }
    if ($renamed) { Rename-Item -LiteralPath $newPath -NewName (Split-Path $r.path -Leaf) }

    # Drop a file onto a folder item (shell drop target: moves it into the folder), then back.
    $s = Wait-Scene { param($s) [bool]($s.items | Where-Object path -eq $r.path) } 4000
    $folder = $s.items | Where-Object { (Test-Path -LiteralPath $_.path -PathType Container) -and $null -eq $_.box -and $_.path -like "$fx*" } | Select-Object -First 1
    $mv = $s.items | Where-Object { $_.path -like '*.csv' -and $null -eq $_.box } | Select-Object -First 1
    $from = Center $mv; $to = Center $folder
    [TestInput]::Drag($from[0], $from[1], $to[0], $to[1], 0, 15)
    $moved = Join-Path $folder.path (Split-Path $mv.path -Leaf)
    $sw = [Diagnostics.Stopwatch]::StartNew(); while (-not (Test-Path -LiteralPath $moved) -and $sw.ElapsedMilliseconds -lt 5000) { Start-Sleep -Milliseconds 100 }
    Note 'fileop.dropOnFolder' (Test-Path -LiteralPath $moved)
    if (Test-Path -LiteralPath $moved) { Move-Item -LiteralPath $moved -Destination $mv.path }

    # Permanent delete (Shift+Delete, FOF_NO_UI) of one fixture file.
    $s = Wait-Scene { param($s) [bool]($s.items | Where-Object path -eq $mv.path) } 4000
    $del = $s.items | Where-Object { $_.path -like '*.mp3' -and $null -eq $_.box } | Select-Object -First 1
    Click (Center $del); Keys @($VK.SHIFT, $VK.DEL, ($VK.DEL -bor 0x8000), ($VK.SHIFT -bor 0x8000))
    $sw = [Diagnostics.Stopwatch]::StartNew(); while ((Test-Path -LiteralPath $del.path) -and $sw.ElapsedMilliseconds -lt 4000) { Start-Sleep -Milliseconds 100 }
    $s = Wait-Scene { param($s) -not ($s.items | Where-Object path -eq $del.path) } 4000
    Note 'fileop.delete' @{ goneOnDisk = -not (Test-Path -LiteralPath $del.path); goneInScene = -not ($s.items | Where-Object path -eq $del.path) }
    if (-not (Test-Path -LiteralPath $del.path)) { [IO.File]::WriteAllText($del.path, 'deskboxes fixture') }

    Start-Sleep -Milliseconds 1500 # debounced save
    Note 'run2.exit' (Stop-Core $p)
    $st2 = Get-Content (Join-Path $prof 'stats2.json') | ConvertFrom-Json
    Note 'run2.stats' $st2

    # ---- run 3: persistence and warm start -----------------------------------------------------
    $p = Start-Core @('--exit-after', '1500', '--stats-out', (Join-Path $prof 'stats3.json'))
    $s = Wait-Scene { param($s) $s.boxes.Count -eq 2 } 15000
    $p.WaitForExit(20000) | Out-Null
    $s = Get-Scene
    Note 'persist.boxes' ($s.boxes.Count -eq 2 -and $s.boxes[1].tabs.Count -eq 2 -and $s.boxes[1].active -eq 1)
    Note 'persist.dndGrid' ((($s.items | Where-Object key -eq $e.key).rect[0]) -gt ($wr.X + 13 * 75 - 5))
    Note 'run3.stats' (Get-Content (Join-Path $prof 'stats3.json') | ConvertFrom-Json)
}
finally {
    Get-Process deskboxes, deskboxes-shellhost -ErrorAction SilentlyContinue | Where-Object { $otherPids -notcontains $_.Id } | Stop-Process -Force -ErrorAction SilentlyContinue
    if ($null -ne $saveClip) { Set-Clipboard -Value $saveClip }
    if (-not $KeepFixture) { & $gen remove $fx | Write-Host }
    [IO.File]::WriteAllText((Join-Path $OutDir 'results.json'), ($results | ConvertTo-Json -Depth 6), [Text.UTF8Encoding]::new($false))
}

