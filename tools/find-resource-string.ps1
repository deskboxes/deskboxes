#Requires -Version 7
<#
.SYNOPSIS
  Finds Windows string-table resources matching a regex (after removing '&' mnemonics), so the
  host can show Windows' own localized text via "@dll,-id" references (menu_policy.json).
.EXAMPLE
  .\tools\find-resource-string.ps1 -Pattern '^(View|Refresh)$' -Dll shell32.dll, windows.storage.dll
#>
param(
    [Parameter(Mandatory)][string]$Pattern,
    [string[]]$Dll = @('shell32.dll', 'windows.storage.dll', 'Windows.UI.FileExplorer.dll', 'ExplorerFrame.dll')
)
Add-Type -Namespace DbxRes -Name S -MemberDefinition @'
[DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern IntPtr LoadLibraryExW(string f, IntPtr h, uint fl);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int LoadStringW(IntPtr h, uint id, System.Text.StringBuilder b, int n);
'@
$sb = [Text.StringBuilder]::new(1024)
foreach ($d in $Dll) {
    $path = if ([IO.Path]::IsPathRooted($d)) { $d } else { Join-Path "$env:WINDIR\System32" $d }
    $h = [DbxRes.S]::LoadLibraryExW($path, [IntPtr]::Zero, 0x22) # DATAFILE | IMAGE_RESOURCE
    if ($h -eq [IntPtr]::Zero) { Write-Warning "cannot load $path"; continue }
    for ($i = 1; $i -lt 65536; $i++) {
        [void]$sb.Clear()
        if ([DbxRes.S]::LoadStringW($h, $i, $sb, 1024) -gt 0) {
            $s = $sb.ToString()
            if (($s -replace '&', '') -match $Pattern) { [pscustomobject]@{ Dll = $d; Id = $i; Text = $s; Ref = "@$d,-$i" } }
        }
    }
}
