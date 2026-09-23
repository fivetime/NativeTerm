# Drag the TEST NativeTerm's main window by its title bar to each edge of
# its monitor with the mouse (so Windows' own Aero Snap happens), release,
# move the pointer away, touch the edge, and report the window's place.
# Only the test instance (command line names the scratchpad) is touched;
# before every button press the window under the pointer and the
# foreground window are checked to be it.
param([string]$Edges = "left right top")
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class U {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr c);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, IntPtr e);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
  [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint f);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h, uint f);
  [DllImport("user32.dll")] public static extern bool GetMonitorInfoW(IntPtr m, ref MONITORINFO i);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out RECT r, int s);
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L; public int T; public int R; public int B; }
  [StructLayout(LayoutKind.Sequential)] public struct MONITORINFO { public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags; }
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public static IntPtr Largest(uint pid) {
    IntPtr best = IntPtr.Zero; long area = 0;
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); RECT r;
      if (p == pid && IsWindowVisible(h) && GetWindowRect(h, out r)) { long a = (long)(r.R - r.L) * (r.B - r.T); if (a > area) { area = a; best = h; } } return true; }, IntPtr.Zero);
    return best;
  }
}
"@
[void][U]::SetProcessDpiAwarenessContext([IntPtr](-4))
$scratch = "C:\Users\Simon\AppData\Local\Temp\claude\C--Users-Simon\6d2d9fe8-96e9-451f-934c-f298b26cb311\scratchpad"
$exe = "C:\MyProjects\RustProjects\NativeTerm\target\debug\nativeterm.exe"
$log = "$scratch\win_dock.log"
Remove-Item $log -ErrorAction SilentlyContinue
$env:NATIVETERM_DOCK_LOG = "1"
$p = Start-Process $exe -ArgumentList @("--terminal-dir", "C:\MyProjects\RustProjects\terminal-1.26.2581.0", "--ssh-dir", "$scratch\ssh-test", "--data-dir", "$scratch\gui-data-drag") -RedirectStandardError $log -PassThru
Start-Sleep -Seconds 8
$h = [U]::Largest([uint32]$p.Id)
if ($h -eq [IntPtr]::Zero) { "no window"; Stop-Process -Id $p.Id -Force; exit 1 }
function Frame { $r = New-Object U+RECT; [void][U]::DwmGetWindowAttribute($h, 9, [ref]$r, 16); $r }
function State { $r = Frame; "{0},{1} {2}x{3} visible={4}" -f $r.L, $r.T, ($r.R - $r.L), ($r.B - $r.T), [U]::IsWindowVisible($h) }
function Ours($x, $y) {
  $pt = New-Object U+POINT; $pt.X = $x; $pt.Y = $y
  $under = [U]::GetAncestor([U]::WindowFromPoint($pt), 2)
  ($under -eq $h) -and ([U]::GetForegroundWindow() -eq $h)
}
$mi = New-Object U+MONITORINFO; $mi.cbSize = 40
[void][U]::GetMonitorInfoW([U]::MonitorFromWindow($h, 2), [ref]$mi)
$w = $mi.rcWork; $cx = [int](($w.L + $w.R) / 2); $cy = [int](($w.T + $w.B) / 2)
"work area {0},{1} {2}x{3}" -f $w.L, $w.T, ($w.R - $w.L), ($w.B - $w.T)
function Drag($tx, $ty) {
  [void][U]::SetForegroundWindow($h); Start-Sleep -Milliseconds 300
  $r = Frame; $sx = $r.L + 300; $sy = $r.T + 12
  [void][U]::SetCursorPos($sx, $sy); Start-Sleep -Milliseconds 200
  if (-not (Ours $sx $sy)) { "not our window under the pointer or in front: stopped"; return $false }
  [U]::mouse_event(0x2, 0, 0, 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 200
  for ($i = 1; $i -le 12; $i++) { [void][U]::SetCursorPos([int]($sx + ($tx - $sx) * $i / 12), [int]($sy + ($ty - $sy) * $i / 12)); Start-Sleep -Milliseconds 60 }
  Start-Sleep -Milliseconds 600
  [U]::mouse_event(0x4, 0, 0, 0, [IntPtr]::Zero); Start-Sleep -Seconds 2
  return $true
}
foreach ($edge in $Edges.Split(" ")) {
  $ok = switch ($edge) { "left" { Drag $w.L $cy } "right" { Drag ($w.R - 1) $cy } "top" { Drag $cx $w.T } }
  if (-not $ok) { break }
  $docked = State
  switch ($edge) { "left" { [void][U]::SetCursorPos($w.R - 40, $cy) } "right" { [void][U]::SetCursorPos($w.L + 40, $cy) } "top" { [void][U]::SetCursorPos($cx, $w.B - 40) } }
  Start-Sleep -Seconds 2; $hidden = State
  # the strip: where the hidden window still is on the screen
  $f = Frame; $mx = [int](($f.L + $f.R) / 2); $my = [int](($f.T + $f.B) / 2)
  if ($my -lt $w.T) { $my = $w.T + 1 }
  switch ($edge) { "left" { [void][U]::SetCursorPos($w.L + 1, $my) } "right" { [void][U]::SetCursorPos($w.R - 2, $my) } "top" { [void][U]::SetCursorPos($mx, $w.T + 1) } }
  Start-Sleep -Milliseconds 200
  switch ($edge) { "left" { [void][U]::SetCursorPos($w.L + 2, $my + 1) } "right" { [void][U]::SetCursorPos($w.R - 3, $my + 1) } "top" { [void][U]::SetCursorPos($mx + 1, $w.T + 2) } }
  Start-Sleep -Seconds 2
  "$edge | after drag: $docked | pointer away: $hidden | edge touched: $(State)"
  # back to the middle, undocked
  [void](Drag ($cx - 200) ($cy - 150)); Start-Sleep -Seconds 1
}
[void][U]::SetCursorPos($cx, $cy)
[void]$p.CloseMainWindow(); [void]$p.WaitForExit(10000)
if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
"--- dock log"; Get-Content $log | Select-String "^dock:" | Select-Object -Last 20 | ForEach-Object { $_.Line.Substring(0, [Math]::Min(160, $_.Line.Length)) }
