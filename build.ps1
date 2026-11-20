<#
.SYNOPSIS
  Builds Deskboxes: the Rust workspace (core, watchdog, tools) and the C++/WinRT shell host.
.EXAMPLE
  .\build.ps1                    # Debug, everything
  .\build.ps1 -Release           # Release, everything
  .\build.ps1 -Only shellhost    # just the shell host
.NOTES
  Needs the Step 0 toolchain (setup.ps1). Fetches crates and NuGet packages; installs nothing.
#>
param(
    [switch]$Release,
    [ValidateSet('all', 'rust', 'shellhost')][string]$Only = 'all',
    # Build the installer: implies -Release and -Only all.
    [switch]$Installer
)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
if ($Installer) { $Release = $true; $Only = 'all' }
$config = if ($Release) { 'Release' } else { 'Debug' }
# A shell started before rustup was installed may not have cargo on PATH yet.
if (-not (Get-Command cargo -ErrorAction SilentlyContinue) -and (Test-Path "$env:USERPROFILE\.cargo\bin\cargo.exe")) {
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
}

function Step($name, [scriptblock]$body) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    Write-Host "== $name" -ForegroundColor Cyan
    & $body
    if ($LASTEXITCODE -ne 0) { throw "$name failed (exit $LASTEXITCODE)" }
    Write-Host ("   ok in {0:n1} s" -f $sw.Elapsed.TotalSeconds)
}

if ($Only -in 'all', 'rust') {
    Step "cargo build ($config)" {
        Push-Location $root
        try { if ($Release) { cargo build --workspace --release } else { cargo build --workspace } }
        finally { Pop-Location }
    }
}

if ($Only -in 'all', 'shellhost') {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $msbuild = & $vswhere -products * -latest -requires Microsoft.Component.MSBuild -find 'MSBuild\**\Bin\amd64\MSBuild.exe' | Select-Object -First 1
    if (-not $msbuild) { throw 'MSBuild not found; run setup.ps1 (Step 0) first.' }
    $sh = Join-Path $root 'shellhost'
    Step 'nuget restore (shellhost)' { nuget restore (Join-Path $sh 'packages.config') -PackagesDirectory (Join-Path $sh 'packages') -NonInteractive -Verbosity quiet }
    Step "msbuild shellhost ($config)" {
        & $msbuild (Join-Path $sh 'DeskboxesShellHost.vcxproj') /p:Configuration=$config /p:Platform=x64 /m /nologo /v:m
    }
}
if ($Installer) {
    # Stage what gets installed (one flat folder: the core and the watchdog find the shell host
    # next to themselves), then pack it into the setup program.
    $stage = Join-Path $root 'out\Release\setup-stage'
    $setupOut = Join-Path $root 'out\Release\DeskboxesSetup.exe'
    Step 'stage installer files' {
        if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
        New-Item -ItemType Directory -Force $stage | Out-Null
        $sh = Join-Path $root 'out\Release\shellhost'
        # The shell host's output minus build leftovers (symbols, import libraries).
        Get-ChildItem $sh -Recurse -File | Where-Object { $_.Extension -notin '.pdb', '.lib', '.exp', '.ilk', '.iobj', '.ipdb' } | ForEach-Object {
            $rel = $_.FullName.Substring($sh.Length + 1)
            $dst = Join-Path $stage $rel
            New-Item -ItemType Directory -Force (Split-Path $dst) | Out-Null
            Copy-Item $_.FullName $dst
        }
        foreach ($exe in 'deskboxes.exe', 'deskboxes-watchdog.exe') { Copy-Item (Join-Path $root "target\release\$exe") $stage }
        # The Visual C++ runtime next to the programs (app-local, as Microsoft's redist.txt allows),
        # so the end user's PC needs no Visual C++ Redistributable.
        $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
        $vs = & $vswhere -products * -latest -property installationPath
        $crt = Get-ChildItem (Join-Path $vs 'VC\Redist\MSVC\*\x64\Microsoft.VC14*.CRT') -Directory | Sort-Object FullName | Select-Object -Last 1
        if (-not $crt) { throw 'Visual C++ redistributable files (VC\Redist\MSVC\*\x64\Microsoft.VC14*.CRT) not found' }
        foreach ($dll in 'msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll') { Copy-Item (Join-Path $crt.FullName $dll) $stage }
        $global:LASTEXITCODE = 0
    }
    Step 'pack installer' {
        $ver = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
        $p = Start-Process (Join-Path $root 'target\release\deskboxes-setup.exe') -ArgumentList '--pack', "`"$stage`"", "`"$setupOut`"", '--app-version', $ver -NoNewWindow -Wait -PassThru
        $global:LASTEXITCODE = $p.ExitCode
    }
    Write-Host ("Installer: {0} ({1:n1} MB)" -f $setupOut, ((Get-Item $setupOut).Length / 1MB)) -ForegroundColor Green
}
Write-Host "Build finished: out\$config\ and target\$($config.ToLower())\" -ForegroundColor Green
