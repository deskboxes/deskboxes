# drop-sink.ps1: a small window that accepts file drops (as COPY, so nothing moves) and logs what
# it received, to check dragging out of Deskboxes into another application.
param([string]$Log = "$env:TEMP\dbx-dropsink.log", [int]$X = 1500, [int]$Y = 300)
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Remove-Item $Log -ErrorAction SilentlyContinue
$f = [Windows.Forms.Form]::new()
$f.Text = 'Deskboxes drop test'; $f.StartPosition = 'Manual'; $f.Location = [Drawing.Point]::new($X, $Y)
$f.Size = [Drawing.Size]::new(420, 300); $f.TopMost = $true; $f.AllowDrop = $true; $f.BackColor = [Drawing.Color]::FromArgb(40, 40, 60)
$f.Add_DragEnter({ param($s, $e) if ($e.Data.GetDataPresent([Windows.Forms.DataFormats]::FileDrop)) { $e.Effect = 'Copy' }; Add-Content $Log "enter allowed=$($e.AllowedEffect) formats=$(($e.Data.GetFormats()) -join ',')" })
$f.Add_DragOver({ param($s, $e) if ($e.Data.GetDataPresent([Windows.Forms.DataFormats]::FileDrop)) { $e.Effect = 'Copy' } })
$f.Add_DragDrop({ param($s, $e) $files = $e.Data.GetData([Windows.Forms.DataFormats]::FileDrop); Add-Content $Log "drop files=$($files -join '|')" })
Add-Type -Namespace Sink -Name W -MemberDefinition '[DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr h, int c);'
# Started with a hidden console: the first ShowWindow takes the hidden state, so show it again.
$f.Add_Shown({ [void][Sink.W]::ShowWindow($f.Handle, 1); $f.Activate(); Add-Content $Log "ready pid=$PID" })
[Windows.Forms.Application]::Run($f)
