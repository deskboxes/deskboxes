# Comparison rules shared by tools/menu-dump.ps1 (our model) and tools/menu-view-check.ps1 (our view).

function Key($e) {
    if ($e.id -eq 'separator') { return '----' }
    $id = $e.id
    if ($id -match '^\d+$') { $id = '#ext' }             # Explorer numbers extension commands (320xx)
    if ($id -match '^\{[0-9A-Fa-f-]+\}$') { $id = '#ext' } # we show their CLSID/canonical GUID
    '{0} | {1} | {2} | {3}' -f $id, $e.name, $e.accel, ($(if ($e.submenu) { '>' } else { '' }))
}

function Compare-Seq([string]$what, $ours, $theirs) {
    $a = @($ours | ForEach-Object { Key $_ }); $b = @($theirs | ForEach-Object { Key $_ })
    $d = Compare-Object $b $a -SyncWindow 0
    $sameOrder = ($a -join "`n") -eq ($b -join "`n")
    [pscustomobject]@{ What = $what; Equal = $sameOrder; Windows = $b; Ours = $a; Diff = $d }
}

function Get-ExpectedList([string]$case, $truth) {
    # Fixture cases run in an Explorer FOLDER WINDOW. Entries that exist only there are not
    # expected on the desktop: verbs registered with OnlyInBrowserWindow (opennewtab,
    # opennewwindow) and the folder view's own background entries (View/Sort by/Group by/
    # Refresh, which Deskboxes supplies itself on the desktop). Returns @{ list; filtered }.
    $tl = $truth.list
    if ($case -like 'desktop_*') { return @{ list = $tl; filtered = $false } }
    $browserOnly = @('opennewtab', 'opennewwindow') + $(if ($case -eq 'folder_background') { @('view', 'arrange', 'groupby', 'refresh') } else { @() })
    $tl2 = @($tl | Where-Object { $_.id -notin $browserOnly })
    $clean = [System.Collections.Generic.List[object]]::new()
    foreach ($e in $tl2) { if ($e.id -eq 'separator' -and ($clean.Count -eq 0 -or $clean[$clean.Count - 1].id -eq 'separator')) { continue }; $clean.Add($e) }
    @{ list = $clean; filtered = ($tl2.Count -ne @($tl).Count) }
}
