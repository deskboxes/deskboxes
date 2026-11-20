#Requires -Version 7
<#
.SYNOPSIS
  Repeats Deskboxes "Step 0": installs and verifies the build prerequisites.

.DESCRIPTION
  Run once, in an ELEVATED PowerShell 7 session, while someone is at the PC
  (installers may show progress windows). It is safe to re-run: anything already
  installed is skipped.

  Installs (via winget / the VS installer):
    - Visual Studio 2022 Build Tools with:
        Microsoft.VisualStudio.Workload.VCTools                          (MSVC v143, CMake, etc.)
        Microsoft.VisualStudio.Component.VC.Tools.x86.x64                (MSVC v143 x64/x86)
        Microsoft.VisualStudio.Component.Windows11SDK.26100              (Windows 11 SDK 10.0.26100)
        Microsoft.VisualStudio.Workload.UniversalBuildTools              (WinUI app development build tools)
        Microsoft.VisualStudio.ComponentGroup.UWP.VC.BuildTools          (C++ v143 "Windows Store" app type, x64)
        Microsoft.VisualStudio.ComponentGroup.WindowsAppDevelopment.VC.BuildTools (C++ WinUI build tools)
      IDs from https://learn.microsoft.com/visualstudio/install/workload-component-id-vs-build-tools
      NOTE: on the original dev PC, adding only the two ComponentGroups (without the
      UniversalBuildTools workload) installed the "Windows Store" C++ app type for
      ARM64/ARM64EC only, and WinUI C++ builds failed with "OutputPath is not set".
    - Rust via rustup (stable-x86_64-pc-windows-msvc)
    - NuGet CLI (packages.config restore for the C++ shell host)
    - Git

  The Windows App SDK and C++/WinRT come from NuGet inside the project and are
  deployed SELF-CONTAINED, so no system-wide Windows App SDK runtime is installed.
#>
[CmdletBinding()]
param([switch]$SkipVerify)
$ErrorActionPreference = 'Stop'

function Test-Admin {
    ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)
}
function Update-SessionPath {
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
                [Environment]::GetEnvironmentVariable('Path', 'User')
}
function Install-Winget([string]$Id, [string[]]$Extra = @()) {
    Write-Host "winget install $Id" -ForegroundColor Cyan
    winget install --id $Id --exact --source winget --accept-package-agreements --accept-source-agreements @Extra
    if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne -1978335189) { # -1978335189 = already installed
        throw "winget install $Id failed with $LASTEXITCODE"
    }
}

if (-not (Test-Admin)) { throw 'Run setup.ps1 from an elevated PowerShell 7 session.' }

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vsComponents = @(
    'Microsoft.VisualStudio.Workload.VCTools',
    'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
    'Microsoft.VisualStudio.Component.Windows11SDK.26100',
    'Microsoft.VisualStudio.Workload.UniversalBuildTools',
    'Microsoft.VisualStudio.ComponentGroup.UWP.VC.BuildTools',
    'Microsoft.VisualStudio.ComponentGroup.WindowsAppDevelopment.VC.BuildTools'
)
$addArgs = ($vsComponents | ForEach-Object { "--add $_" }) -join ' '

# ---- Visual Studio 2022 Build Tools ---------------------------------------
$vsPath = if (Test-Path $vswhere) { & $vswhere -latest -products * -version '[17.0,18.0)' -property installationPath } else { $null }
if (-not $vsPath) {
    Install-Winget 'Microsoft.VisualStudio.2022.BuildTools' @('--override', "$addArgs --includeRecommended --passive --norestart --wait")
} else {
    Write-Host "VS 2022 found at $vsPath; ensuring components" -ForegroundColor Cyan
    $setupExe = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\setup.exe"
    $p = Start-Process $setupExe -Wait -PassThru -ArgumentList (@('modify', '--installPath', "`"$vsPath`"") +
        ($vsComponents | ForEach-Object { '--add', $_ }) + @('--includeRecommended', '--passive', '--norestart'))
    # NOTE: no --wait here: that is a bootstrapper flag; setup.exe rejects it with exit code 87.
    if ($p.ExitCode -notin 0, 3010) { throw "VS installer modify failed: $($p.ExitCode)" }
    if ($p.ExitCode -eq 3010) { Write-Warning 'VS installer requests a reboot.' }
}

# ---- Rust, NuGet, Git ------------------------------------------------------
Update-SessionPath
if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) { Install-Winget 'Rustlang.Rustup' @('--silent') }
if (-not (Get-Command nuget -ErrorAction SilentlyContinue))  { Install-Winget 'Microsoft.NuGet' @('--silent') }
if (-not (Get-Command git -ErrorAction SilentlyContinue))    { Install-Winget 'Git.Git' @('--silent') }
Update-SessionPath
rustup toolchain install stable-x86_64-pc-windows-msvc --profile default
rustup default stable-x86_64-pc-windows-msvc

if ($SkipVerify) { return }

# ---- Verify ------------------------------------------------------------------
$vsPath = & $vswhere -latest -products * -version '[17.0,18.0)' -property installationPath
& "$vsPath\Common7\Tools\Launch-VsDevShell.ps1" -Arch amd64 -HostArch amd64 -SkipAutomaticLocation | Out-Null
$rows = @(
    [pscustomobject]@{ Tool = 'Visual Studio';  Value = (& $vswhere -latest -products * -property catalog_productDisplayVersion) }
    [pscustomobject]@{ Tool = 'cl.exe';         Value = ((cl.exe 2>&1 | Select-Object -First 1) -replace '.*Version ', '') }
    [pscustomobject]@{ Tool = 'Windows SDK';    Value = ((Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\Include").Name -join ', ') }
    [pscustomobject]@{ Tool = 'cargo';          Value = (cargo --version) }
    [pscustomobject]@{ Tool = 'nuget';          Value = ((nuget help | Select-Object -First 1)) }
    [pscustomobject]@{ Tool = 'git';            Value = (git --version) }
)
$rows | Format-Table -AutoSize
Write-Host 'Prerequisites installed. Build with .\build.ps1' -ForegroundColor Green
