# Save the real shell icons (and picture thumbnails) of the scene items as 256 px PNGs with alpha
# in tools\art\icons, for art.html. The items come from assets.ps1 (shortcuts to popular
# installed apps, Office documents, pictures, folders), so the pictures show what a desktop really has.
param([string]$Items = "$env:TEMP\dbx-shots-master", [int]$Size = 256)
$ErrorActionPreference = 'Stop'
$out = Join-Path $PSScriptRoot 'icons'
New-Item -ItemType Directory -Force $out | Out-Null
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
public static class ShellPic {
    [ComImport, Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IShellItemImageFactory { [PreserveSig] int GetImage(SIZE size, int flags, out IntPtr hbmp); }
    [StructLayout(LayoutKind.Sequential)] struct SIZE { public int cx, cy; }
    [StructLayout(LayoutKind.Sequential)] struct BITMAP { public int bmType, bmWidth, bmHeight, bmWidthBytes; public short bmPlanes, bmBitsPixel; public IntPtr bmBits; }
    [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
    static extern void SHCreateItemFromParsingName(string path, IntPtr bc, [MarshalAs(UnmanagedType.LPStruct)] Guid riid, [MarshalAs(UnmanagedType.Interface)] out IShellItemImageFactory f);
    [DllImport("gdi32.dll")] static extern int GetObject(IntPtr h, int n, out BITMAP b);
    [DllImport("gdi32.dll")] static extern bool DeleteObject(IntPtr h);
    public static void Save(string path, int size, bool iconOnly, string png) {
        IShellItemImageFactory f;
        SHCreateItemFromParsingName(path, IntPtr.Zero, typeof(IShellItemImageFactory).GUID, out f);
        IntPtr hb;
        // SIIGBF_BIGGERSIZEOK 1, SIIGBF_ICONONLY 4
        int hr = f.GetImage(new SIZE { cx = size, cy = size }, iconOnly ? 4 : 0, out hb);
        if (hr != 0) throw new Exception("GetImage " + path + " " + hr);
        BITMAP b; GetObject(hb, Marshal.SizeOf(typeof(BITMAP)), out b);
        int w = b.bmWidth, h = Math.Abs(b.bmHeight);
        var bmp = new Bitmap(w, h, PixelFormat.Format32bppArgb);
        var px = new byte[b.bmWidthBytes * h];
        Marshal.Copy(b.bmBits, px, 0, px.Length);
        bool anyAlpha = false;
        for (int i = 3; i < px.Length; i += 4) if (px[i] != 0) { anyAlpha = true; break; }
        for (int y = 0; y < h; y++) {
            int row = b.bmHeight > 0 ? h - 1 - y : y;   // bottom-up unless the height is negative
            for (int x = 0; x < w; x++) {
                int o = row * b.bmWidthBytes + x * 4;
                int a = anyAlpha ? px[o + 3] : 255;
                int r = px[o + 2], g = px[o + 1], bl = px[o];
                if (a > 0 && a < 255) { r = Math.Min(255, r * 255 / a); g = Math.Min(255, g * 255 / a); bl = Math.Min(255, bl * 255 / a); }
                bmp.SetPixel(x, y, Color.FromArgb(a, r, g, bl));
            }
        }
        DeleteObject(hb);
        bmp.Save(png, ImageFormat.Png);
        bmp.Dispose();
    }
}
'@
$n = 0
foreach ($f in Get-ChildItem $Items) {
    $name = [IO.Path]::GetFileNameWithoutExtension($f.Name)
    if ($f.PSIsContainer) { $name = 'folder_' + $f.Name } elseif ($f.Extension -ne '.lnk') { $name = $f.Name }
    $png = Join-Path $out ($name + '.png')
    # Pictures show their thumbnail, as on the desktop; everything else its icon.
    $thumb = $f.Extension -eq '.jpg'
    [ShellPic]::Save($f.FullName, $Size, -not $thumb, $png)
    $n++
}
"$n icons in $out"
