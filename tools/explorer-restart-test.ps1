#Requires -Version 7
<#
.SYNOPSIS
  Boot-flash proxy test: restart Explorer while native desktop icons are hidden and
  record whether the desktop list view is ever visible during startup.

.DESCRIPTION
  A real logon cannot be automated in an unattended run (logging off ends the session),
  so an Explorer restart is the proxy: Explorer reads the same per-user state
  (HKCU\...\Explorer\Advanced\HideIcons) when it creates the desktop view at startup.

  Steps: hide icons (IFolderView2 FWF_NOICONS) -> start a ~1 ms poller of
  Progman/SHELLDLL_DefView/SysListView32 visibility -> taskkill explorer -> start
  explorer at normal integrity (tools/run-as-user) unless Windows auto-restarts it ->
  wait -> report -> ALWAYS restore icons and verify.

  Run from the repo root after `cargo build`. Touches the real desktop briefly.
#>
param([int]$WatchSeconds = 15, [switch]$KeepHidden)
$ErrorActionPreference = 'Stop'
$bin = Join-Path $PSScriptRoot '..\target\debug' | Resolve-Path
$tmp = [IO.Path]::GetTempPath()

Add-Type -TypeDefinition @'
using System; using System.Collections.Generic; using System.Diagnostics; using System.Runtime.InteropServices; using System.Threading;
public static class FlashPoll {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr FindWindowW(string c, string n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr FindWindowExW(IntPtr p, IntPtr a, string c, string n);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
  [DllImport("winmm.dll")] static extern uint timeBeginPeriod(uint p);
  public static List<string> Log = new List<string>();
  static volatile bool stop; static Thread t;
  public static int VisibleSamples, Samples;
  static IntPtr FindLv() {
    IntPtr pm = FindWindowW("Progman", null); if (pm == IntPtr.Zero) return IntPtr.Zero;
    IntPtr dv = FindWindowExW(pm, IntPtr.Zero, "SHELLDLL_DefView", null);
    if (dv == IntPtr.Zero) { IntPtr ww = IntPtr.Zero; while ((ww = FindWindowExW(IntPtr.Zero, ww, "WorkerW", null)) != IntPtr.Zero) { dv = FindWindowExW(ww, IntPtr.Zero, "SHELLDLL_DefView", null); if (dv != IntPtr.Zero) break; } }
    if (dv == IntPtr.Zero) return IntPtr.Zero;
    return FindWindowExW(dv, IntPtr.Zero, "SysListView32", null);
  }
  public static void Start() {
    stop = false; Log.Clear(); VisibleSamples = 0; Samples = 0; timeBeginPeriod(1);
    var sw = Stopwatch.StartNew();
    t = new Thread(() => { string last = null;
      while (!stop) { IntPtr lv = FindLv(); string s = lv == IntPtr.Zero ? "no-listview" : (IsWindowVisible(lv) ? "VISIBLE" : "hidden");
        Samples++; if (s == "VISIBLE") VisibleSamples++;
        if (s != last) { lock (Log) Log.Add(string.Format("{0,7} ms  {1}  (lv=0x{2:X})", sw.ElapsedMilliseconds, s, lv.ToInt64())); last = s; }
        Thread.Sleep(1); } });
    t.IsBackground = true; t.Priority = ThreadPriority.Highest; t.Start();
  }
  public static void Stop() { stop = true; t.Join(); }
}
'@

function Invoke-Spike([string[]]$a) {
    $o = Join-Path $tmp 'deskboxes_hi.txt'
    & "$bin\run-as-user.exe" --out $o -- "$bin\spike-hide-icons.exe" @a | Out-Null
    Get-Content $o
}
function Wait-Shell {
    for ($i = 0; $i -lt 100; $i++) {
        $pm = Get-Process explorer -ErrorAction SilentlyContinue
        if ($pm -and ([FlashPoll]::Log.Count -ge 0) -and (Invoke-Spike @('state') | Select-String 'listview=0x[1-9A-F]')) { return $true }
        Start-Sleep -Milliseconds 200
    }
    return $false
}

$result = [ordered]@{}
try {
    "== hide icons"; Invoke-Spike @('fv', 'on')
    [FlashPoll]::Start()
    Start-Sleep -Milliseconds 500
    "== taskkill explorer"; taskkill /f /im explorer.exe | Out-Null
    Start-Sleep -Seconds 3
    if (-not (Get-Process explorer -ErrorAction SilentlyContinue)) {
        "== explorer did not auto-restart; starting it at normal integrity"
        & "$bin\run-as-user.exe" --no-wait -- "$env:WINDIR\explorer.exe" | Out-Null
    } else { "== explorer auto-restarted" }
    Start-Sleep -Seconds $WatchSeconds
    [FlashPoll]::Stop()
    "== list-view visibility transitions during restart:"
    [FlashPoll]::Log
    $result.VisibleSamples = [FlashPoll]::VisibleSamples
    $result.Samples = [FlashPoll]::Samples
    "== state after restart"; Invoke-Spike @('state')
}
finally {
    if (-not $KeepHidden) {
        if (-not (Get-Process explorer -ErrorAction SilentlyContinue)) { & "$bin\run-as-user.exe" --no-wait -- "$env:WINDIR\explorer.exe" | Out-Null; Start-Sleep 5 }
        "== restore icons"; Invoke-Spike @('fv', 'off')
        $st = Invoke-Spike @('state')
        if ($st -notmatch 'lv_visible=true') { "fv off did not restore; using toggle"; Invoke-Spike @('toggle') }
    }
    $tray = (Get-Process explorer -ErrorAction SilentlyContinue).Count
    "== explorer processes: $tray"
}
"RESULT flash_samples={0}/{1}" -f $result.VisibleSamples, $result.Samples
