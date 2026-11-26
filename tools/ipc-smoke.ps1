<#
.SYNOPSIS
  Plays the core's side of the shell-host pipe (docs/ARCHITECTURE.md §11) against a NON-elevated
  deskboxes-shellhost --pipe: hello/ready, show (1 item and 100 items, cold / warm / prefetched),
  dismiss with Escape, and checks the host exits when the core disconnects.
  Reports request->visible times ("shown.ms", measured in the host) against the §7 targets.
  Touches only fixture files in %TEMP% and the host's own windows.
#>
param(
    [string]$Host_ = (Join-Path $PSScriptRoot '..\out\Debug\shellhost\deskboxes-shellhost.exe'),
    [int]$X = 900, [int]$Y = 300,
    [int]$Repeat = 3
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing
Add-Type -Path (Join-Path $PSScriptRoot 'lib\TestInput.cs')
. (Join-Path $PSScriptRoot 'lib\MenuUia.ps1')
. (Join-Path $PSScriptRoot 'lib\MenuFixtures.ps1')
$Host_ = (Resolve-Path $Host_).Path
$runAs = (Resolve-Path (Join-Path $PSScriptRoot '..\target\debug\run-as-user.exe')).Path
$elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

$fx = Join-Path ([IO.Path]::GetTempPath()) 'deskboxes-ipcsmoke'
New-MenuFixtures $fx
$many = Join-Path $fx 'many'; New-Item -ItemType Directory $many | Out-Null
1..100 | ForEach-Object { Set-Content (Join-Path $many ("plik {0:D3}.txt" -f $_)) 'x' -NoNewline }

$name = "deskboxes-host-smoke-$PID"
# Hosts running already (the real Deskboxes' menus) are left alone; only ours is stopped.
$otherHosts = @(Get-Process deskboxes-shellhost -ErrorAction SilentlyContinue | ForEach-Object Id)
function Our-Hosts { Get-Process deskboxes-shellhost -ErrorAction SilentlyContinue | Where-Object { $otherHosts -notcontains $_.Id } }
if ($elevated) { & $runAs --no-wait -- $Host_ --pipe $name | Out-Null } else { Start-Process $Host_ -ArgumentList '--pipe', $name | Out-Null }

$pipe = [System.IO.Pipes.NamedPipeClientStream]::new('.', $name, [System.IO.Pipes.PipeDirection]::InOut)
$results = [ordered]@{}
try {
    $pipe.Connect(10000)
    $hostProc = Our-Hosts | Select-Object -First 1
    $script:nextId = 1
    function Send-Msg([hashtable]$m) {
        $m.v = 1; if (-not $m.id) { $m.id = $script:nextId++ }
        $b = [Text.Encoding]::UTF8.GetBytes(($m | ConvertTo-Json -Compress -Depth 6))
        $pipe.Write([BitConverter]::GetBytes([uint32]$b.Length), 0, 4); $pipe.Write($b, 0, $b.Length); $pipe.Flush()
        $m.id
    }
    function Read-Msg([int]$timeoutMs = 10000) {
        $len = [byte[]]::new(4)
        $t = $pipe.ReadAsync($len, 0, 4)
        if (-not $t.Wait($timeoutMs)) { throw "no message within $timeoutMs ms" }
        if ($t.Result -lt 4) { throw "the host closed the pipe (see %LOCALAPPDATA%\Deskboxes\logs\shellhost.log)" }
        $n = [BitConverter]::ToUInt32($len, 0); $buf = [byte[]]::new($n); $got = 0
        while ($got -lt $n) { $r = $pipe.Read($buf, $got, $n - $got); if ($r -le 0) { throw "the host closed the pipe mid-message" }; $got += $r }
        [Text.Encoding]::UTF8.GetString($buf) | ConvertFrom-Json
    }
    function Expect([string]$type, [int]$timeoutMs = 10000) {
        while ($true) { $m = Read-Msg $timeoutMs; if ($m.t -eq $type) { return $m }; Write-Host "  (got $($m.t))" }
    }
    function Show-And-Dismiss([hashtable]$req, [string]$label) {
        $req.t = 'show'; $req.x = $X; $req.y = $Y
        $id = Send-Msg $req
        $shown = Expect 'shown'
        $wins = @(); $sw = [Diagnostics.Stopwatch]::StartNew()
        while (-not $wins -and $sw.ElapsedMilliseconds -lt 3000) { Start-Sleep -Milliseconds 30; $wins = Get-MenuWindows $hostProc.Id }
        $m = if ($wins) { Read-Menu $wins } else { $null }
        if ($wins) { [TestInput]::Focus($wins[0].Current.NativeWindowHandle) | Out-Null }
        [TestInput]::Keys(0x1B, 0x801B) | Out-Null
        $closed = Expect 'closed'
        $items = if ($m) { @($m.row).Count + @($m.list | Where-Object id -ne 'separator').Count } else { 0 }
        Write-Host ("  {0,-28} shown {1,6:N1} ms   entries {2,2}   closed: {3}" -f $label, $shown.ms, $items, $(if ($null -eq $closed.invoked) { 'dismissed' } else { $closed.invoked }))
        [pscustomobject]@{ label = $label; ms = [math]::Round($shown.ms, 1); entries = $items; dismissed = ($null -eq $closed.invoked) }
    }

    [void](Send-Msg @{ t = 'hello'; proto = 1; lang = 'en'; theme = 'system' })
    $ready = Expect 'ready'
    Write-Host "ready (proto $($ready.proto)); waiting for prewarm"
    Start-Sleep -Milliseconds 2500

    $one = @{ kind = 'item'; items = @(Join-Path $fx 'notatka.txt') }
    $hundred = @{ kind = 'item'; items = @(Get-ChildItem $many -File | Sort-Object Name | ForEach-Object FullName) }
    $rows = @()
    $rows += Show-And-Dismiss $one.Clone() '1 item (first .txt menu)'
    for ($i = 0; $i -lt $Repeat; $i++) { $rows += Show-And-Dismiss $one.Clone() '1 item (warm)' }
    for ($i = 0; $i -lt $Repeat; $i++) {
        $p = $one.Clone(); $p.t = 'prefetch'; [void](Send-Msg $p); Start-Sleep -Milliseconds 400
        $rows += Show-And-Dismiss $one.Clone() '1 item (prefetched)'
    }
    $rows += Show-And-Dismiss $hundred.Clone() '100 items (first)'
    for ($i = 0; $i -lt $Repeat; $i++) { $rows += Show-And-Dismiss $hundred.Clone() '100 items (warm)' }
    $bg = @{ kind = 'background'; desktop = $true; own = @{ view = @(@{ id = 'view.large'; text = 'Large icons'; radio = $true }, @{ id = 'view.medium'; text = 'Medium icons'; radio = $true; checked = $true }); arrange = @(@{ id = 'sort.name'; text = 'Name'; radio = $true; checked = $true }) } }
    $rows += Show-And-Dismiss $bg.Clone() 'desktop background'
    # The host builds the next desktop menu ahead a moment after one closes.
    for ($i = 0; $i -lt $Repeat; $i++) { Start-Sleep -Milliseconds 900; $rows += Show-And-Dismiss $bg.Clone() 'desktop background (again)' }

    $pipe.Dispose()
    $exited = $hostProc.WaitForExit(5000)
    Write-Host "host exited after core disconnect: $exited"
    $results.rows = $rows; $results.hostExitedOnDisconnect = $exited
}
finally {
    $pipe.Dispose()
    Our-Hosts | Stop-Process -Force
    Remove-Item $fx -Recurse -Force -ErrorAction SilentlyContinue
}
$rows | Group-Object label | ForEach-Object {
    $ms = $_.Group.ms
    [pscustomobject]@{ Case = $_.Name; Runs = $ms.Count; MedianMs = ($ms | Sort-Object)[[int][math]::Floor(($ms.Count - 1) / 2)]; MaxMs = ($ms | Measure-Object -Maximum).Maximum; Entries = $_.Group[0].entries }
} | Format-Table -AutoSize | Out-String | Write-Host
Write-Host "Targets (§7): 1 item < 120 ms, 100 items < 250 ms, request -> menu visible."
