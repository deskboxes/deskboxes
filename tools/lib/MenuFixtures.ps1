# Shared by tools/menu-probe.ps1 (reads Windows' own menu) and tools/menu-dump.ps1 (reads ours),
# so both look at exactly the same files. Names use Polish diacritics on purpose.

function New-MenuFixtures([string]$dir) {
    Add-Type -AssemblyName System.Drawing
    if (Test-Path $dir) { Remove-Item $dir -Recurse -Force }
    New-Item -ItemType Directory $dir | Out-Null
    Set-Content "$dir\Raport roczny.pdf" '%PDF-1.4' -NoNewline
    Set-Content "$dir\Budżet.xlsx" 'x' -NoNewline
    Set-Content "$dir\Pismo.docx" 'x' -NoNewline
    Set-Content "$dir\notatka.txt" 'notatka' -NoNewline
    New-Item -ItemType Directory "$dir\Folder testowy" | Out-Null
    $bmp = [Drawing.Bitmap]::new(64, 48); $g = [Drawing.Graphics]::FromImage($bmp); $g.Clear([Drawing.Color]::SteelBlue); $g.Dispose()
    $bmp.Save("$dir\zdjęcie.png", [Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
    Compress-Archive -Path "$dir\notatka.txt" -DestinationPath "$dir\archiwum.zip"
    Copy-Item "$env:WINDIR\System32\where.exe" "$dir\program.exe"
    $ws = New-Object -ComObject WScript.Shell
    $l = $ws.CreateShortcut("$dir\Notatnik.lnk"); $l.TargetPath = "$env:WINDIR\System32\notepad.exe"; $l.Save()
    Set-Content "$dir\Strona.url" "[InternetShortcut]`r`nURL=https://example.com/`r`n"
}

$MenuCases = [ordered]@{
    'pdf'    = @('Raport roczny.pdf')
    'xlsx'   = @('Budżet.xlsx')
    'docx'   = @('Pismo.docx')
    'txt'    = @('notatka.txt')
    'folder' = @('Folder testowy')
    'png'    = @('zdjęcie.png')
    'zip'    = @('archiwum.zip')
    'exe'    = @('program.exe')
    'lnk'    = @('Notatnik.lnk')
    'url'    = @('Strona.url')
    'multi3' = @('Raport roczny.pdf', 'Budżet.xlsx', 'Pismo.docx')
    'mixed'  = @('Folder testowy', 'Raport roczny.pdf', 'Notatnik.lnk')
}
