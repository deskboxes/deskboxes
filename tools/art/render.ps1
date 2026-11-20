param([string[]]$Scenes = @(), [string[]]$Themes = @('dark', 'light'), [switch]$Thumbs, [switch]$NoThumbs)
# Render art.html scenes with headless Chrome (its own throwaway profile in %TEMP%) into out\NAME_THEME[_t].png;
# then export.py makes the Settings pictures in res\settings. Needs Chrome and Python with Pillow.
$a = Split-Path $MyInvocation.MyCommand.Path
$prof = Join-Path $env:TEMP 'deskboxes-art-chrome'
$chrome = 'C:\Program Files\Google\Chrome\Application\chrome.exe'
$all = 'home', 'tabs', 'rollup', 'rules', 'az', 'folder', 'above', 'hide', 'saved', 'search', 'locks', 'privacy', 'boxes', 'colors', 'icons', 'look'
$tiles = 'tabs', 'rollup', 'rules', 'az', 'icons', 'colors', 'folder', 'above', 'hide', 'saved', 'search', 'locks', 'privacy'
if (-not $Scenes) { $Scenes = $all }
New-Item -ItemType Directory -Force "$a\out" | Out-Null
$url = 'file:///' + ($a -replace '\\', '/') + '/art.html'
foreach ($s in $Scenes) {
    foreach ($t in $Themes) {
        $jobs = @(@{ thumb = 0; suf = '' })
        if (-not $NoThumbs -and $tiles -contains $s) { $jobs += @{ thumb = 1; suf = '_t' } }
        if ($Thumbs) { $jobs = $jobs | Where-Object { $_.thumb -eq 1 } }
        foreach ($j in $jobs) {
            $size = if ($s -eq 'home') { '1920,534' } else { '1600,900' }
            $out = "$a\out\${s}_$t$($j.suf).png"
            Remove-Item $out -ErrorAction SilentlyContinue
            Start-Process -Wait -FilePath $chrome -ArgumentList @('--headless=new', '--disable-gpu', '--hide-scrollbars', '--no-first-run', '--force-device-scale-factor=1',
                "--user-data-dir=`"$prof`"", "--window-size=$size", '--virtual-time-budget=2500', "--screenshot=`"$out`"", "`"$url#scene=$s&theme=$t&thumb=$($j.thumb)`"")
            if (-not (Test-Path $out)) { Write-Warning "no $out" }
        }
    }
}
'rendered'
