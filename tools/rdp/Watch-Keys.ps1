<#
.SYNOPSIS
  Read-only diagnostic for Remote Desktop keyboard problems (docs/research/rdp-keyboard.md):
  logs the toggle, IME and modifier keys this session receives, and the IME state they cause.

.DESCRIPTION
  A low-level keyboard hook (WH_KEYBOARD_LL) in this session logs, for the keys that switch input
  modes only, the virtual-key code, scan code and flags as the session's key table produced them:
  Caps Lock / Eisu (scan code 0x3A), Hankaku/Zenkaku (0x29), Katakana/Hiragana (0x70), Henkan
  (0x79), Muhenkan (0x7B), Shift, Ctrl, Alt, and the IME virtual keys (VK_KANA, VK_KANJI,
  VK_IME_ON/OFF, VK_CONVERT, VK_NONCONVERT, VK_PROCESSKEY, VK_DBE_*). Every other key is logged as
  "other key" with no code, so typed text is never recorded.

  Whenever it changes, the foreground window's class name, its input language (HKL), the IME
  open status and conversion mode (WM_IME_CONTROL on its default IME window) and the Caps Lock /
  Kana toggle bits are logged too. At the start the log records whether this is a Remote Desktop
  session, GetKeyboardType (in a remote session: what the client reported, not the table the
  session types with) and the API view of the table for scan code 0x3A (0x14 = 101-style Caps
  Lock, 0xF0 = 106-style Eisu).

  Nothing is written to the system and nothing is sent anywhere: the hook passes every key on
  unchanged, and the log goes only to -Out.

.PARAMETER Seconds
  How long to watch (default 120).

.PARAMETER Out
  The log file (default: mklm-watch-keys.log in %TEMP%). Overwritten.

.NOTES
  Usage (E2/E3 of docs/research/rdp-keyboard.md):
  1. In the Remote Desktop session, open a PowerShell window yourself (not from a service, a
     scheduled task or a sandboxed tool: those run in another session or without a desktop).
  2. powershell -NoProfile -ExecutionPolicy Bypass -File Watch-Keys.ps1 -Seconds 120
  3. Click into Notepad and press, about 2 seconds apart: Eisu alone x3, Shift+Eisu x2,
     Ctrl+Eisu, Alt+Eisu, Hankaku/Zenkaku, Alt+Hankaku/Zenkaku. Wait until "stopped".
  4. Read the log: on a 106/109 (JIS) table, Eisu alone arrives as vk=0xF0 and Shift+Eisu as
     vk=0x14; on a 101/102 (US) table it is the other way round, and Hankaku/Zenkaku is vk=0xC0.
     A key-down with no key-up is a client trait (the key-up never reached the session).
  The toggle bits are read from a background thread and can lag the foreground window; check
  Caps Lock by typing a letter with the IME off.
#>
[CmdletBinding()]
param(
    [int]$Seconds = 120,
    [string]$Out = (Join-Path $env:TEMP 'mklm-watch-keys.log')
)

$ErrorActionPreference = 'Stop'

$src = @'
using System; using System.IO; using System.Runtime.InteropServices; using System.Text; using System.Diagnostics;
public static class MklmWatchKeys {
  delegate IntPtr HookProc(int code, IntPtr wParam, IntPtr lParam);
  [StructLayout(LayoutKind.Sequential)] struct KBDLLHOOKSTRUCT { public uint vkCode, scanCode, flags, time; public IntPtr extra; }
  [StructLayout(LayoutKind.Sequential)] struct MSG { public IntPtr hwnd; public uint message; public IntPtr wParam, lParam; public uint time; public int x, y; }
  [DllImport("user32.dll", SetLastError=true)] static extern IntPtr SetWindowsHookEx(int id, HookProc proc, IntPtr hMod, uint thread);
  [DllImport("user32.dll")] static extern bool UnhookWindowsHookEx(IntPtr h);
  [DllImport("user32.dll")] static extern IntPtr CallNextHookEx(IntPtr h, int code, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] static extern bool PeekMessage(out MSG m, IntPtr h, uint min, uint max, uint remove);
  [DllImport("user32.dll")] static extern bool TranslateMessage(ref MSG m);
  [DllImport("user32.dll")] static extern IntPtr DispatchMessage(ref MSG m);
  [DllImport("user32.dll")] static extern uint MsgWaitForMultipleObjects(uint n, IntPtr[] h, bool all, uint ms, uint mask);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode)] static extern IntPtr GetModuleHandle(string name);
  [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] static extern bool AttachThreadInput(uint a, uint b, bool attach);
  [DllImport("user32.dll")] static extern short GetKeyState(int vk);
  [DllImport("user32.dll")] static extern IntPtr GetKeyboardLayout(uint thread);
  [DllImport("user32.dll")] static extern int GetKeyboardType(int flag);
  [DllImport("user32.dll")] static extern uint MapVirtualKeyExW(uint code, uint mapType, IntPtr hkl);
  [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
  [DllImport("imm32.dll")] static extern IntPtr ImmGetDefaultIMEWnd(IntPtr h);
  [DllImport("user32.dll")] static extern IntPtr SendMessageTimeout(IntPtr h, uint msg, IntPtr w, IntPtr l, uint flags, uint timeout, out IntPtr result);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder sb, int n);

  // Mode keys only: Shift, Ctrl, Alt (both sides), Caps Lock, Kana, IME on/off, Kanji, Convert,
  // Nonconvert, ProcessKey, the DBE keys, and Hankaku/Zenkaku as the 101 table names it (VK_OEM_3).
  static readonly uint[] Vks = { 0x10, 0x11, 0x12, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0x14, 0x15, 0x16, 0x19, 0x1A,
                                 0x1C, 0x1D, 0xE5, 0xC0, 0xF0, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6 };
  // Eisu/Caps, Hankaku/Zenkaku, Katakana/Hiragana, Henkan, Muhenkan, Shift, Ctrl, Alt.
  static readonly uint[] Scans = { 0x3A, 0x29, 0x70, 0x79, 0x7B, 0x2A, 0x36, 0x1D, 0x38 };
  static StreamWriter w; static Stopwatch sw; static HookProc keep;

  static void Log(string s) { w.WriteLine(string.Format("{0,7}ms {1}", sw.ElapsedMilliseconds, s)); }
  static IntPtr Hook(int code, IntPtr wp, IntPtr lp) {
    if (code >= 0) {
      var k = (KBDLLHOOKSTRUCT)Marshal.PtrToStructure(lp, typeof(KBDLLHOOKSTRUCT));
      int m = (int)wp;
      string msg = m == 0x100 ? "DOWN" : m == 0x101 ? "UP" : m == 0x104 ? "SYSDOWN" : m == 0x105 ? "SYSUP" : "0x" + m.ToString("X");
      // VK_OEM_3 is also a text key on the 101 table (`): log it only for scan code 0x29.
      bool mode = (Array.IndexOf(Vks, k.vkCode) >= 0 && (k.vkCode != 0xC0 || k.scanCode == 0x29))
                  || Array.IndexOf(Scans, k.scanCode) >= 0;
      if (mode)
        Log(string.Format("KEY {0,-7} vk=0x{1:X2} sc=0x{2:X2} flags=0x{3:X2}{4}{5}", msg, k.vkCode, k.scanCode, k.flags,
          (k.flags & 0x01) != 0 ? " EXTENDED" : "", (k.flags & 0x10) != 0 ? " INJECTED" : ""));
      else
        Log("KEY " + msg + " (other key" + ((k.flags & 0x10) != 0 ? ", injected" : "") + ")");
    }
    return CallNextHookEx(IntPtr.Zero, code, wp, lp);
  }
  static string State() {
    IntPtr fg = GetForegroundWindow(); uint pid; uint t = GetWindowThreadProcessId(fg, out pid);
    var cls = new StringBuilder(128); GetClassName(fg, cls, 128);
    uint me = GetCurrentThreadId(); bool att = t != 0 && t != me && AttachThreadInput(me, t, true);
    int caps = GetKeyState(0x14) & 1; int kana = GetKeyState(0x15) & 1;
    if (att) AttachThreadInput(me, t, false);
    IntPtr hkl = GetKeyboardLayout(t); IntPtr ime = ImmGetDefaultIMEWnd(fg);
    IntPtr open = (IntPtr)(-1), conv = (IntPtr)(-1);
    if (ime != IntPtr.Zero) {
      SendMessageTimeout(ime, 0x283, (IntPtr)5, IntPtr.Zero, 2, 200, out open);   // IMC_GETOPENSTATUS
      SendMessageTimeout(ime, 0x283, (IntPtr)1, IntPtr.Zero, 2, 200, out conv);   // IMC_GETCONVERSIONMODE
    }
    return string.Format("fg={0} pid={1} caps={2} kana={3} hkl={4:X8} imeOpen={5} conv=0x{6:X}", cls, pid, caps, kana, (long)hkl, (long)open, (long)conv);
  }
  public static void Run(int seconds, string path) {
    w = new StreamWriter(path, false, new UTF8Encoding(false)); w.AutoFlush = true; sw = Stopwatch.StartNew();
    uint vk3A = MapVirtualKeyExW(0x3A, 1, GetKeyboardLayout(0));   // MAPVK_VSC_TO_VK
    Log(string.Format("session: remote={0} GetKeyboardType={1}/{2}/{3} table-0x3A=0x{4:X2} ({5}, API view)",
      GetSystemMetrics(0x1000) != 0, GetKeyboardType(0), GetKeyboardType(1), GetKeyboardType(2), vk3A,
      vk3A == 0x14 ? "101-style" : vk3A == 0xF0 ? "106-style" : "other"));
    keep = Hook; IntPtr h = SetWindowsHookEx(13, keep, GetModuleHandle(null), 0);
    if (h == IntPtr.Zero) { Log("SetWindowsHookEx failed " + Marshal.GetLastWin32Error()); w.Close(); return; }
    Log("started; watching for " + seconds + " s");
    string last = null; MSG msg;
    try {
      while (sw.ElapsedMilliseconds < seconds * 1000L) {
        MsgWaitForMultipleObjects(0, null, false, 50, 0x04FF);
        while (PeekMessage(out msg, IntPtr.Zero, 0, 0, 1)) { TranslateMessage(ref msg); DispatchMessage(ref msg); }
        string s = State(); if (s != last) { Log("STATE " + s); last = s; }
      }
    } finally {
      UnhookWindowsHookEx(h); Log("stopped"); w.Close();
    }
  }
}
'@
if (-not ('MklmWatchKeys' -as [type])) { Add-Type -TypeDefinition $src }
Write-Host "Watching mode keys for $Seconds s; log: $Out (other keys are logged without their codes)."
[MklmWatchKeys]::Run($Seconds, $Out)
Write-Host "Stopped. Log: $Out"
