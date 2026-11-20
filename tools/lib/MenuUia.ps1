# UI Automation helpers shared by tools/menu-probe.ps1 (Windows' own menu) and
# tools/menu-view-check.ps1 (our view): find a process's menu windows, read them, capture them.
# Requires UIAutomationClient/Types, System.Drawing and lib/TestInput.cs to be loaded.

$UIA = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]
$Cond = [System.Windows.Automation.Condition]::TrueCondition
$MenuItemCond = [System.Windows.Automation.PropertyCondition]::new($UIA::ControlTypeProperty, [System.Windows.Automation.ControlType]::MenuItem)

function Get-MenuWindows([int]$explorerPid) {
    # Windows 11 26H2: the modern menu is a WinUI CommandBarFlyout hosted in top-level
    # "Microsoft.UI.Content.PopupWindowSiteBridge" windows of the explorer.exe process that
    # owns the target window (folder windows run in separate /factory processes). The
    # primary-command row and the secondary list are separate bridge windows; several
    # more are pre-created and stay tiny. Return the ones that contain menu items.
    @([TestInput]::TopLevel($explorerPid, 'Microsoft.UI.Content.PopupWindowSiteBridge') + [TestInput]::TopLevel($explorerPid, '#32768') |
        ForEach-Object { $UIA::FromHandle($_) } |
        Where-Object { $_.Current.BoundingRectangle.Width -gt 100 -and $null -ne $_.FindFirst($TS::Descendants, $MenuItemCond) })
}
function Get-WindowPid([IntPtr]$h) { [TestInput]::Pid($h) }

function Read-Menu($windows) {
    # Collect unique MenuItems (the row's tree also contains the overflow list), sorted:
    # primary row first (group 'row'), then the list top-to-bottom; separators inferred
    # from vertical gaps between consecutive list items.
    $seen = @{}; $items = @()
    foreach ($w in $windows) {
        foreach ($m in $w.FindAll($TS::Descendants, $MenuItemCond)) {
            $ci = $m.Current; $r = $ci.BoundingRectangle
            if ($r.IsEmpty) { continue }
            $key = "$($ci.Name)|$([int]$r.X)|$([int]$r.Y)"
            if ($seen[$key]) { continue }; $seen[$key] = 1
            $exp = $null
            try { $exp = $m.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern).Current.ExpandCollapseState.ToString() } catch {}
            $items += [pscustomobject]@{ group = $(if ($r.Width -lt 100) { 'row' } else { 'list' }); name = $ci.Name; id = $ci.AutomationId; accel = $ci.AcceleratorKey; cls = $ci.ClassName; submenu = ($exp -and $exp -ne 'LeafNode'); enabled = $ci.IsEnabled; x = [int]$r.X; y = [int]$r.Y; w = [int]$r.Width; h = [int]$r.Height }
        }
    }
    $row = @($items | Where-Object group -eq 'row' | Sort-Object x)
    $list = @($items | Where-Object group -eq 'list' | Sort-Object y)
    $out = [System.Collections.Generic.List[object]]::new()
    $prev = $null
    foreach ($i in $list) {
        if ($prev -and ($i.y - ($prev.y + $prev.h)) -gt 4) { $out.Add([pscustomobject]@{ group = 'list'; name = '----'; id = 'separator' }) }
        $out.Add($i); $prev = $i
    }
    $rowPos = 'none'
    if ($row.Count -and $list.Count) { $rowPos = $(if ($row[0].y -lt $list[0].y) { 'top' } else { 'bottom' }) }
    [pscustomobject]@{ rowPosition = $rowPos; row = $row; list = $out }
}

function Save-Capture($rect, [string]$path) {
    if ($rect.Width -le 0) { return }
    $b = [Drawing.Bitmap]::new([int]$rect.Width + 24, [int]$rect.Height + 24)
    $g = [Drawing.Graphics]::FromImage($b)
    $g.CopyFromScreen([int]$rect.X - 12, [int]$rect.Y - 12, 0, 0, $b.Size)
    $g.Dispose(); $b.Save($path, [Drawing.Imaging.ImageFormat]::Png); $b.Dispose()
}

function Save-WindowsCapture($wins, [string]$path) {
    # Capture the union of the given menu windows (row, list and any open submenu).
    if (-not $wins) { return }
    $l = ($wins | ForEach-Object { $_.Current.BoundingRectangle.X } | Measure-Object -Minimum).Minimum
    $t = ($wins | ForEach-Object { $_.Current.BoundingRectangle.Y } | Measure-Object -Minimum).Minimum
    $r = ($wins | ForEach-Object { $_.Current.BoundingRectangle.Right } | Measure-Object -Maximum).Maximum
    $b = ($wins | ForEach-Object { $_.Current.BoundingRectangle.Bottom } | Measure-Object -Maximum).Maximum
    Save-Capture ([System.Windows.Rect]::new($l, $t, $r - $l, $b - $t)) $path
}

function Expand-Submenus([int]$procId, $menu, [string[]]$ids, [string]$capturePrefix) {
    # Open submenus the way a keyboard user does: Down until the item has focus, Right to open,
    # read the new items, Left to close. (UIA ExpandCollapsePattern.Expand dismisses Explorer's
    # whole menu for "Open with", so it is not used.) Returns an ordered map id -> items.
    $subs = [ordered]@{}
    $before = @{}; foreach ($i in @($menu.row) + @($menu.list)) { $before["$($i.name)|$($i.x)|$($i.y)"] = 1 }
    foreach ($xid in $ids) {
        if (-not @($menu.list | Where-Object { $_.id -eq $xid })) { continue }   # not in this menu
        $found = $false
        for ($n = 0; $n -lt 40; $n++) {
            $f = $null; try { $f = [System.Windows.Automation.AutomationElement]::FocusedElement } catch {}
            if ($f -and $f.Current.AutomationId -eq $xid) { $found = $true; break }
            [TestInput]::Keys(0x28, 0x8028) | Out-Null; Start-Sleep -Milliseconds 60     # Down
        }
        if (-not $found) { Write-Warning "submenu $xid : item never got focus"; continue }
        [TestInput]::Keys(0x27, 0x8027) | Out-Null                                       # Right
        Start-Sleep -Milliseconds 900
        $wins = Get-MenuWindows $procId
        if ($capturePrefix) { Save-WindowsCapture $wins "$capturePrefix.$xid.png" }
        $m2 = Read-Menu $wins
        $subs[$xid] = @($m2.list | Where-Object { $_.id -ne 'separator' -and -not $before["$($_.name)|$($_.x)|$($_.y)"] } |
            ForEach-Object { [pscustomobject]@{ name = $_.name; id = $_.id; accel = $_.accel; submenu = $_.submenu } })
        Write-Host "  [$xid >] $(@($subs[$xid]).Count) item(s) in $(@($wins).Count) window(s)"
        [TestInput]::Keys(0x25, 0x8025) | Out-Null; Start-Sleep -Milliseconds 300       # Left: close it
    }
    $subs
}

function Write-TextRetry([string]$path, [string]$text) {
    # Indexers / AV briefly lock freshly written files; retry instead of failing the run.
    for ($i = 0; $i -lt 10; $i++) {
        try { [IO.File]::WriteAllText($path, $text, [Text.UTF8Encoding]::new($true)); return } catch { Start-Sleep -Milliseconds 200 }
    }
    throw "cannot write $path"
}
