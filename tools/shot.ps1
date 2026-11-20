# shot.ps1: screenshot of the primary screen (or a region) to a PNG, for checking the real
# desktop by eye during development. Usage: shot.ps1 -Out file.png [-X 0 -Y 0 -W 800 -H 600] [-Scale 0.5]
param([Parameter(Mandatory)][string]$Out, [int]$X = 0, [int]$Y = 0, [int]$W = 0, [int]$H = 0, [double]$Scale = 1.0)
Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type -Namespace Dpi -Name A -MemberDefinition '[DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(System.IntPtr v);'
[void][Dpi.A]::SetProcessDpiAwarenessContext([IntPtr]-4)
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
if ($W -le 0) { $W = $b.Width - $X }; if ($H -le 0) { $H = $b.Height - $Y }
$bmp = [Drawing.Bitmap]::new($W, $H)
$g = [Drawing.Graphics]::FromImage($bmp); $g.CopyFromScreen($X, $Y, 0, 0, $bmp.Size); $g.Dispose()
if ($Scale -ne 1.0) {
    $s = [Drawing.Bitmap]::new([int]($W * $Scale), [int]($H * $Scale))
    $g = [Drawing.Graphics]::FromImage($s); $g.InterpolationMode = 'HighQualityBicubic'; $g.DrawImage($bmp, 0, 0, $s.Width, $s.Height); $g.Dispose()
    $bmp.Dispose(); $bmp = $s
}
$bmp.Save($Out, [Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
"$Out ${W}x${H}"
