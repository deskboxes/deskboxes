# release.ps1: make a Deskboxes release.
#
#   tools\release.ps1 -Version 0.2.0            build it: version bump, installer, latest.json and
#                                               the release notes in out\Release\release (nothing
#                                               leaves this PC)
#   tools\release.ps1 -Version 0.2.0 -Publish   the same, then commit "Deskboxes 0.2.0", tag v0.2.0,
#                                               push both and create the GitHub release with the
#                                               installer, latest.json and the notes
#   ... -Publish -Legacy                        also publish it to the old deskboxes/releases
#                                               repository, which copies up to 0.1.3 read
#
# A revision of a release is X.Y.Z-N (0.1.7-1): update.rs compares the numbers split at "." and
# "-", so every copy (0.1.4 on) takes 0.1.7-1 for newer than 0.1.7.
#
# Release notes: docs/releases/<version>.md (see docs/releases/TEMPLATE.md). The script puts the
# installer's SHA-256 table at the top, as "## Installer hashes".
#
# Installed copies read https://github.com/deskboxes/deskboxes/releases/latest/download/latest.json
# once a day (crates/desktop/src/update.rs): {version, url, sha256, size}. They download the
# installer from `url`, check the size and SHA-256, and install it the next time Deskboxes starts.
param(
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+(-\d+)?$')][string]$Version,
    [switch]$Publish,
    [switch]$Legacy
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$repo = 'deskboxes/deskboxes'
$legacyRepo = 'deskboxes/releases'
$gh = (Get-Command gh -ErrorAction SilentlyContinue).Source
if (-not $gh) { $gh = 'C:\Program Files\GitHub CLI\gh.exe' }
Set-Location $root

$notesFile = Join-Path $root "docs\releases\$Version.md"
if (-not (Test-Path $notesFile)) { throw "Write the release notes first: $notesFile (see docs\releases\TEMPLATE.md)." }

if ($Publish) {
    # Only the version bump may be uncommitted: the release commit must be exactly what was built.
    $dirty = @(git status --porcelain | Where-Object { $_ -notmatch '^ M (Cargo\.toml|Cargo\.lock)$' })
    if ($dirty) { throw "Commit or stash these first:`n$($dirty -join "`n")" }
    if ((git rev-parse --abbrev-ref HEAD) -ne 'main') { throw 'Releases are made from main.' }
    if (git tag -l "v$Version") { throw "v$Version is already tagged." }
    & $gh release view "v$Version" --repo $repo *> $null
    if ($LASTEXITCODE -eq 0) { throw "v$Version is already on $repo." }
}

# 1. Version: the workspace version is every crate's version (and what update.rs compares).
$toml = Join-Path $root 'Cargo.toml'
$text = [IO.File]::ReadAllText($toml)
$old = [regex]::Match($text, '(?m)^version = "([^"]+)"').Groups[1].Value
if ($old -ne $Version) {
    $text = [regex]::Replace($text, '(?m)^version = "[^"]+"', "version = `"$Version`"", 1)
    [IO.File]::WriteAllText($toml, $text, (New-Object Text.UTF8Encoding $false))
    Write-Host "version $old -> $Version"
}

# 2. Installer (Release build of everything).
& (Join-Path $root 'build.ps1') -Installer
if ($LASTEXITCODE) { throw 'build failed' }

# 3. The release files.
$out = Join-Path $root 'out\Release\release'
New-Item -ItemType Directory -Force $out | Out-Null
Get-ChildItem $out | Remove-Item -Recurse -Force
$setup = Join-Path $out 'DeskboxesSetup.exe'
Copy-Item (Join-Path $root 'out\Release\DeskboxesSetup.exe') $setup
$f = Get-Item $setup
$sha = (Get-FileHash $setup -Algorithm SHA256).Hash
function Write-Feed($r, $path) {
    [ordered]@{
        version = $Version
        url     = "https://github.com/$r/releases/download/v$Version/DeskboxesSetup.exe"
        sha256  = $sha.ToLower()
        size    = $f.Length
    } | ConvertTo-Json | Set-Content $path -Encoding utf8NoBOM
}
Write-Feed $repo (Join-Path $out 'latest.json')

# The notes: the installer hashes first, then the notes as written.
$body = (Get-Content $notesFile -Raw -Encoding utf8).TrimEnd()
$hashes = @"
## Installer hashes

| Description | Filename | sha256 hash |
| --- | --- | --- |
| Per user - x64 | [DeskboxesSetup.exe](https://github.com/$repo/releases/download/v$Version/DeskboxesSetup.exe) | $sha |
"@
$notes = Join-Path $out 'notes.md'
Set-Content $notes ($hashes + "`n`n" + $body + "`n") -Encoding utf8NoBOM
Write-Host ("{0}: {1:n1} MB, sha256 {2}" -f $setup, ($f.Length / 1MB), $sha.ToLower()) -ForegroundColor Green

if (-not $Publish) {
    Write-Host "Not published (add -Publish). Notes: $notes"
    return
}

# 4. Code: commit the bump, tag, push.
git add Cargo.toml Cargo.lock
git diff --cached --quiet
if ($LASTEXITCODE) {
    git commit -q -m "Deskboxes $Version"
}
git tag -a "v$Version" -m "Deskboxes $Version"
git push -q origin main "v$Version"
if ($LASTEXITCODE) { throw 'git push failed' }

# 5. The release: "latest" is what installed copies look at.
& $gh release create "v$Version" $setup (Join-Path $out 'latest.json') --repo $repo --title "Deskboxes $Version" --notes-file $notes --verify-tag --latest
if ($LASTEXITCODE) { throw 'gh release create failed' }
Write-Host "Published: https://github.com/$repo/releases/tag/v$Version" -ForegroundColor Green

# 6. The old releases repository (copies up to 0.1.3 read its feed): the same installer, its own
#    latest.json, and notes that Settings can show (no table) with a pointer to the new home.
if ($Legacy) {
    $lo = Join-Path $out 'legacy'
    New-Item -ItemType Directory -Force $lo | Out-Null
    Copy-Item $setup $lo
    Write-Feed $legacyRepo (Join-Path $lo 'latest.json')
    $moved = "Deskboxes now lives at https://github.com/$repo, together with its source code. Full notes and downloads: https://github.com/$repo/releases/tag/v$Version"
    # Settings up to 0.1.3 shows a rule line as text.
    $legacyBody = ($body -split "`r?`n" | Where-Object { $_.Trim() -ne '---' }) -join "`n"
    Set-Content (Join-Path $lo 'notes.md') ($legacyBody + "`n`n" + $moved + "`n") -Encoding utf8NoBOM
    & $gh release create "v$Version" (Join-Path $lo 'DeskboxesSetup.exe') (Join-Path $lo 'latest.json') --repo $legacyRepo --title "Deskboxes $Version" --notes-file (Join-Path $lo 'notes.md') --latest
    if ($LASTEXITCODE) { throw "gh release create on $legacyRepo failed" }
    Write-Host "Also published: https://github.com/$legacyRepo/releases/tag/v$Version" -ForegroundColor Green
}
