// Shared helpers for the PowerShell test harnesses (menu-probe, fidelity captures).
// Loaded with: Add-Type -Path tools/lib/TestInput.cs
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;

public static class TestInput
{
    [StructLayout(LayoutKind.Sequential)] struct KI { public ushort vk; public ushort sc; public uint fl; public uint t; public IntPtr x; }
    [StructLayout(LayoutKind.Sequential)] struct MI { public int dx; public int dy; public uint data; public uint fl; public uint t; public IntPtr x; }
    [StructLayout(LayoutKind.Explicit, Size = 40)] struct INP { [FieldOffset(0)] public uint type; [FieldOffset(8)] public KI k; [FieldOffset(8)] public MI m; }
    [DllImport("user32.dll")] static extern uint SendInput(uint n, INP[] i, int sz);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr pid);
    [DllImport("user32.dll")] static extern bool AttachThreadInput(uint a, uint b, bool f);
    [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr h);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowExW(IntPtr p, IntPtr a, string c, string n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    public delegate bool EP(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EP cb, IntPtr l);
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint flags);

    /// Input guard. When non-empty, every synthetic click/drag is refused unless the top-level
    /// window under the point belongs to one of these processes, and every key press unless the
    /// foreground window does. Harnesses add the pids of the windows they created (app under
    /// test, its menu host, a fixture Explorer window), so input never lands in the user's own
    /// windows when focus moves unexpectedly.
    public static HashSet<int> Allowed = new HashSet<int>();
    public static int PidAt(int x, int y) { var p = new POINT { X = x, Y = y }; return Pid(GetAncestor(WindowFromPoint(p), 2)); }
    public static void CheckPoint(int x, int y)
    {
        if (Allowed.Count == 0) return;
        int p = PidAt(x, y);
        // Our desktop surface is a child of Progman: the window itself is ours, its root is not.
        int own = Pid(WindowFromPoint(new POINT { X = x, Y = y }));
        if (!Allowed.Contains(p) && !Allowed.Contains(own)) throw new InvalidOperationException("input refused: the window at " + x + "," + y + " belongs to pid " + p + ", not to a test process");
    }
    public static void CheckForeground()
    {
        if (Allowed.Count == 0) return;
        int p = Pid(GetForegroundWindow());
        if (!Allowed.Contains(p)) throw new InvalidOperationException("input refused: the foreground window belongs to pid " + p + ", not to a test process");
    }

    /// Keys: vk = key down, vk|0x8000 = key up.
    public static uint Keys(params ushort[] seq)
    {
        CheckForeground();
        var a = new INP[seq.Length];
        for (int i = 0; i < seq.Length; i++) { a[i].type = 1; a[i].k.vk = (ushort)(seq[i] & 0x7FFF); a[i].k.fl = (seq[i] & 0x8000) != 0 ? 2u : 0u; }
        return SendInput((uint)a.Length, a, Marshal.SizeOf(typeof(INP)));
    }
    /// Mouse click at screen pixel. button: 0 left, 1 right.
    public static void Click(int x, int y, int button)
    {
        CheckPoint(x, y);
        SetCursorPos(x, y);
        var a = new INP[2];
        a[0].type = 0; a[0].m.fl = button == 1 ? 0x0008u : 0x0002u;
        a[1].type = 0; a[1].m.fl = button == 1 ? 0x0010u : 0x0004u;
        SendInput(2, a, Marshal.SizeOf(typeof(INP)));
    }
    public static void Move(int x, int y) { SetCursorPos(x, y); }
    /// Mouse wheel at screen pixel: clicks (+ away from you, - toward you), one notch each.
    public static void Wheel(int x, int y, int clicks)
    {
        CheckPoint(x, y);
        SetCursorPos(x, y);
        var a = new INP[1];
        a[0].type = 0; a[0].m.fl = 0x0800u; a[0].m.data = unchecked((uint)(clicks * 120));
        SendInput(1, a, Marshal.SizeOf(typeof(INP)));
    }
    /// One mouse button transition. button: 0 left, 1 right.
    public static void Button(int button, bool down)
    {
        if (down) { POINT c; GetCursorPos(out c); CheckPoint(c.X, c.Y); }
        var a = new INP[1];
        a[0].type = 0; a[0].m.fl = button == 1 ? (down ? 0x0008u : 0x0010u) : (down ? 0x0002u : 0x0004u);
        SendInput(1, a, Marshal.SizeOf(typeof(INP)));
    }
    /// Press at (x0,y0), move in steps to (x1,y1), release.
    public static void Drag(int x0, int y0, int x1, int y1, int button, int steps)
    {
        CheckPoint(x0, y0); CheckPoint(x1, y1);
        SetCursorPos(x0, y0); System.Threading.Thread.Sleep(30);
        Button(button, true); System.Threading.Thread.Sleep(30);
        for (int i = 1; i <= steps; i++) { SetCursorPos(x0 + (x1 - x0) * i / steps, y0 + (y1 - y0) * i / steps); System.Threading.Thread.Sleep(15); }
        System.Threading.Thread.Sleep(50);
        Button(button, false);
    }
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
    public static bool Focus(IntPtr h)
    {
        uint me = GetCurrentThreadId(), fg = GetWindowThreadProcessId(GetForegroundWindow(), IntPtr.Zero);
        AttachThreadInput(me, fg, true); BringWindowToTop(h); bool r = SetForegroundWindow(h); AttachThreadInput(me, fg, false);
        return r;
    }
    public static int Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return (int)p; }
    public static string Cls(IntPtr h) { var s = new StringBuilder(256); GetClassNameW(h, s, 256); return s.ToString(); }
    public static List<IntPtr> TopLevel(int pid, string cls)
    {
        var r = new List<IntPtr>();
        EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if ((pid == 0 || p == pid) && IsWindowVisible(h) && (cls == null || Cls(h) == cls)) r.Add(h); return true; }, IntPtr.Zero);
        return r;
    }
}
