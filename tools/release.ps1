# release.ps1: make a Deskboxes release.
#
#   tools\release.ps1 -Version 0.2.0            build it: version bump, installer, latest.json in
#                                               out\Release\release (nothing leaves this PC)
#   tools\release.ps1 -Version 0.2.0 -Publish   the same, then commit "Deskboxes 0.2.0", tag v0.2.0,
#                                               push both to the private code repository and put the
#                                               installer and latest.json on the public releases page
#
# Installed copies read https://github.com/deskboxes/releases/releases/latest/download/latest.json
# once a day (crates/desktop/src/update.rs): {version, url, sha256, size}. They download the
# installer from `url`, check the size and SHA-256, and install it the next time Deskboxes starts.
param(
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version,
    [switch]$Publish,
    [string]$Notes = ''
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$repo = 'deskboxes/releases'
$gh = (Get-Command gh -ErrorAction SilentlyContinue).Source
if (-not $gh) { $gh = 'C:\Program Files\GitHub CLI\gh.exe' }
Set-Location $root

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
Get-ChildItem $out | Remove-Item -Force
$setup = Join-Path $out 'DeskboxesSetup.exe'
Copy-Item (Join-Path $root 'out\Release\DeskboxesSetup.exe') $setup
$f = Get-Item $setup
$feed = [ordered]@{
    version = $Version
    url     = "https://github.com/$repo/releases/download/v$Version/DeskboxesSetup.exe"
    sha256  = (Get-FileHash $setup -Algorithm SHA256).Hash.ToLower()
    size    = $f.Length
}
$feed | ConvertTo-Json | Set-Content (Join-Path $out 'latest.json') -Encoding utf8NoBOM
Write-Host ("{0}: {1:n1} MB, sha256 {2}" -f $setup, ($f.Length / 1MB), $feed.sha256) -ForegroundColor Green

if (-not $Publish) {
    Write-Host 'Not published (add -Publish).'
    return
}

# 4. Code: commit the bump, tag, push (the code repository is private).
git add Cargo.toml Cargo.lock
git diff --cached --quiet
if ($LASTEXITCODE) {
    git commit -q -m "Deskboxes $Version" -m 'Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'
}
git tag -a "v$Version" -m "Deskboxes $Version"
git push -q origin main "v$Version"
if ($LASTEXITCODE) { throw 'git push failed' }

# 5. The public release: "latest" is what installed copies look at. A release needs a commit to
#    tag, so an empty releases repository first gets a short README (author: the no-reply address,
#    never a personal email).
& $gh api "repos/$repo/commits" --silent *> $null
if ($LASTEXITCODE) {
    $readme = "# Deskboxes`n`nDownloads for Deskboxes, a desktop organizer for Windows 11. Get **DeskboxesSetup.exe** from the [latest release](https://github.com/$repo/releases/latest).`n"
    $who = @('-f', 'author[name]=ffuzze', '-f', 'author[email]=359155851+ffuzze@users.noreply.github.com',
        '-f', 'committer[name]=ffuzze', '-f', 'committer[email]=359155851+ffuzze@users.noreply.github.com')
    & $gh api -X PUT "repos/$repo/contents/README.md" -f message='Downloads page' -f "content=$([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($readme)))" @who --silent
    if ($LASTEXITCODE) { throw 'could not create the README in the releases repository' }
}
if (-not $Notes) { $Notes = "Deskboxes $Version. Download DeskboxesSetup.exe and run it; installed copies update themselves." }
& $gh release create "v$Version" $setup (Join-Path $out 'latest.json') --repo $repo --title "Deskboxes $Version" --notes $Notes --latest
if ($LASTEXITCODE) { throw 'gh release create failed' }
Write-Host "Published: https://github.com/$repo/releases/tag/v$Version" -ForegroundColor Green
