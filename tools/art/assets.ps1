# Build the master folder of desktop items for the Settings pictures: shortcuts to popular installed
# apps (Win32 .lnk copied from the Start menu, packaged apps as AppsFolder IDList links), documents,
# pictures (crops of the wallpaper) and folders.
param([string]$Out = "$env:TEMP\dbx-shots-master")
$ErrorActionPreference = 'Stop'
Remove-Item $Out -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory $Out | Out-Null
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class AppLink {
    [ComImport, Guid("000214F9-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IShellLinkW {
        void GetPath(IntPtr a, int b, IntPtr c, int d); void GetIDList(out IntPtr p); void SetIDList(IntPtr p);
        void GetDescription(IntPtr a, int b); void SetDescription([MarshalAs(UnmanagedType.LPWStr)] string s);
        void GetWorkingDirectory(IntPtr a, int b); void SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string s);
        void GetArguments(IntPtr a, int b); void SetArguments([MarshalAs(UnmanagedType.LPWStr)] string s);
        void GetHotkey(out short h); void SetHotkey(short h); void GetShowCmd(out int c); void SetShowCmd(int c);
        void GetIconLocation(IntPtr a, int b, out int c); void SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string s, int i);
        void SetRelativePath([MarshalAs(UnmanagedType.LPWStr)] string s, int r); void Resolve(IntPtr h, int f);
        void SetPath([MarshalAs(UnmanagedType.LPWStr)] string s);
    }
    [ComImport, Guid("0000010b-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IPersistFile {
        void GetClassID(out Guid g); [PreserveSig] int IsDirty();
        void Load([MarshalAs(UnmanagedType.LPWStr)] string f, int m);
        void Save([MarshalAs(UnmanagedType.LPWStr)] string f, bool r);
        void SaveCompleted([MarshalAs(UnmanagedType.LPWStr)] string f); void GetCurFile(out IntPtr f);
    }
    [ComImport, Guid("00021401-0000-0000-C000-000000000046")] class ShellLink {}
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    static extern int SHParseDisplayName(string name, IntPtr bc, out IntPtr pidl, uint sfgao, out uint psfgao);
    public static void Make(string aumid, string file) {
        IntPtr pidl; uint o;
        int hr = SHParseDisplayName("shell:AppsFolder\\" + aumid, IntPtr.Zero, out pidl, 0, out o);
        if (hr != 0) throw new Exception("parse " + aumid + " " + hr);
        var l = (IShellLinkW)new ShellLink();
        l.SetIDList(pidl);
        ((IPersistFile)l).Save(file, true);
        Marshal.FreeCoTaskMem(pidl);
    }
}
'@
$sm = @("$env:ProgramData\Microsoft\Windows\Start Menu\Programs", "$env:APPDATA\Microsoft\Windows\Start Menu\Programs")
$win32 = 'Steam', 'Spotify', 'Discord', 'Visual Studio Code', 'Google Chrome', 'Firefox', 'Word', 'Excel', 'PowerPoint', 'OneNote', 'Telegram', 'Obsidian', 'Blender 5.3', 'VLC media player', 'File Explorer'
foreach ($n in $win32) {
    $f = Get-ChildItem $sm -Recurse -Filter "$n.lnk" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($f) { Copy-Item $f.FullName (Join-Path $Out "$n.lnk") } else { Write-Warning "no $n" }
}
if (Test-Path "$Out\Blender 5.3.lnk") { Rename-Item "$Out\Blender 5.3.lnk" 'Blender.lnk' }
if (Test-Path "$Out\Visual Studio Code.lnk") { Rename-Item "$Out\Visual Studio Code.lnk" 'VS Code.lnk' }
if (Test-Path "$Out\VLC media player.lnk") { Rename-Item "$Out\VLC media player.lnk" 'VLC.lnk' }
$uwp = @{
    'Terminal' = 'Microsoft.WindowsTerminal_8wekyb3d8bbwe!App'; 'Calculator' = 'Microsoft.WindowsCalculator_8wekyb3d8bbwe!App'
    'Photos' = 'Microsoft.Windows.Photos_8wekyb3d8bbwe!App'; 'Notepad' = 'Microsoft.WindowsNotepad_8wekyb3d8bbwe!App'
    'Settings' = 'windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel'; 'Microsoft Store' = 'Microsoft.WindowsStore_8wekyb3d8bbwe!App'
    'Snipping Tool' = 'Microsoft.ScreenSketch_8wekyb3d8bbwe!App'; 'Xbox' = 'Microsoft.GamingApp_8wekyb3d8bbwe!Microsoft.Xbox.App'; 'Camera' = 'Microsoft.WindowsCamera_8wekyb3d8bbwe!App'
}
foreach ($k in $uwp.Keys) { [AppLink]::Make($uwp[$k], (Join-Path $Out "$k.lnk")) }

# Documents: real (tiny) files of each type, so Windows gives them their usual icons.
$docs = 'Report.docx', 'Cover letter.docx', 'Resume.docx', 'Letter.docx', 'Contract.docx', 'Manual.docx', 'Pitch.pptx', 'Pitch v2.pptx', 'Slides.pptx', 'Budget.xlsx', 'Sales Q1.xlsx',
        'Data.xlsx', 'Invoice 039.xlsx', 'Invoice 040.xlsx', 'Invoice 041.xlsx', 'Invoice 042.xlsx', 'Receipt.xlsx', 'Notes.txt', 'Ideas.txt', 'Plan.md', 'Journal.md', 'Taxes.xlsx', 'Meeting.one'
foreach ($d in $docs) { Set-Content -LiteralPath (Join-Path $Out $d) -Value 'x' -NoNewline }
# Pictures: Windows' own wallpapers made small, so the thumbnails look like photos.
Add-Type -AssemblyName System.Drawing
$src = 'ThemeA\img20', 'ThemeB\img24', 'ThemeC\img28', 'ThemeD\img32', 'ThemeA\img22', 'ThemeB\img26', 'ThemeC\img30', 'ThemeD\img34'
$i = 0
foreach ($p in 'Holiday.jpg', 'Beach.jpg', 'Hiking.jpg', 'Sunset.jpg', 'Map.jpg', 'Photo.jpg', 'Lake.jpg', 'Mountains.jpg') {
    $wp = [System.Drawing.Image]::FromFile("C:\Windows\Web\Wallpaper\$($src[$i]).jpg")
    $b = New-Object System.Drawing.Bitmap 480, 320
    $g = [System.Drawing.Graphics]::FromImage($b); $g.InterpolationMode = 'HighQualityBicubic'
    $g.DrawImage($wp, 0, 0, 480, 320)
    $b.Save((Join-Path $Out $p), [System.Drawing.Imaging.ImageFormat]::Jpeg); $g.Dispose(); $b.Dispose(); $wp.Dispose(); $i++
}
foreach ($d in 'Archive', 'Invoices', 'Music', 'Photos', 'Projects', 'Recipes', 'Art', 'Movies', 'Podcasts', 'School', 'Scans', 'Taxes', 'Travel', 'Trips', 'Website', 'Games') {
    New-Item -ItemType Directory (Join-Path $Out $d) | Out-Null
}
Get-ChildItem $Out | Measure-Object | ForEach-Object { "$($_.Count) items in $Out" }
