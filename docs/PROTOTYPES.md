# Prototype results (Phase 0)

Environment: Windows 11 (10.0.26200), Windows Terminal 1.24.11911.0,
OpenSSH_for_Windows_9.5p2. Prototype code: `prototypes/uia-probe`
(UI Automation) and `prototypes/shim-probe` (console behavior). All tests
ran in a separate Windows Terminal window (`-w nt-proto`), leaving the
user's own window untouched, and were cleaned up afterwards.

**Test environment from 2026-09-17 on**: a separate *window* isn't
enough isolation. All windows of the Store Terminal share one process,
and a split + tab tear-out during the menu test hung that process's UI
thread for a while. The user's AI coding sessions run in that process;
one OpenConsole also kept spinning afterwards. From now on, tests run
against a **portable Terminal** at `C:\MyProjects\RustProjects\terminal-1.26.2581.0`
(`.portable`, exe file version 1.26.2609.15001), launched by absolute
path. `uia-probe` and `menu-hook` only see windows whose process image
is under that folder (`NT_PROBE_WT_DIR` overrides it; `*` means all).
Results above this note were measured on the Store build 1.24.11911.0.

## `wt` command line

| Test | Result |
|---|---|
| `wt -w <name> new-tab --title ... cmd` | Works; a named window is created on first use |
| `--sessionId 2222...` (plain GUID) | **Silently ignored**: exit code 0, no tab, no error. Windows Terminal 1.24 parses the value with `IIDFromString`, which requires braces |
| `--sessionId {2222...}` (braced) | Works; the tab's process sees `WT_SESSION=2222...` (no braces) |
| Several tabs in one call (`new-tab ... ; new-tab ...`) | Works |
| 60 tabs in one call | Works; tabs keep appearing for several seconds after `wt` returns |
| `wt.exe` started from Git Bash (MSYS) | Nothing happens; from PowerShell (i.e. `CreateProcess`) it works. NativeTerm starts it via Rust's `Command` — to be confirmed |

## Closing tabs and Ctrl+C

| Test | Result |
|---|---|
| Tab's first process exits with 0 | Tab closes automatically (default `closeOnExit`) |
| Exits with 1 | Tab stays, shows "process exited with code 1 … Ctrl+D to close, Enter to restart" and an error icon; Enter restarts the command (= "Restart connection") |
| Shim ran a child, then installed `SetConsoleCtrlHandler`; `CTRL_C_EVENT` sent to its console | Shim logged and ignored it, stayed alive, tab stayed; later exited 0 and the tab closed |
| Tab closed through its UIA close button while the shim runs | Tab closed; shim received `CTRL_CLOSE_EVENT` (time to report "tab closed") |

## UI Automation

Structure (control view): window `CASCADIA_HOSTING_WINDOW_CLASS` →
`InputSite` pane → `TabView` (Tab) → `ListView` (List) → one `ListViewItem`
(TabItem) per tab, whose children are an image, a text block, and the
close button (Button, localized name "关闭标签页"). The terminal is a
`TermControl` element (control type Text).

| Test | Result |
|---|---|
| `FindAll(Descendants)` from the window | Returns **no** tab items — it doesn't cross into the XAML island. A tree walker does |
| Tree walker, 12 tabs | All tabs, 50–60 ms |
| Tab names | Exactly the tab title, including AI tools' live status glyphs |
| 63 tabs (overflowing strip) | Walker sees only ~45: **tabs scrolled out of view are virtualized away** (140–170 ms) |
| `ItemContainerPattern.FindItemByProperty(prev, 0, empty)` loop on the list | **All 63 tabs in order, names readable without realizing, 26 ms** |
| Virtualized tab: `VirtualizedItemPattern.Realize()` + `SelectionItemPattern.Select()` | Works; the tab is scrolled into view and selected |
| `RuntimeId` across two probe processes (no scrolling) | Unchanged |
| `RuntimeId` after scrolling tabs out of view and back | **Changes** (e.g. 406 → 809): containers are recycled. Not a stable identity |
| Close button via `InvokePattern` (found by control type, not name) | Closes the tab |
| `TextPattern` on `TermControl` | Only one exists (the selected tab); ~19,500 visible characters read in 2 ms |
| `AddAutomationEventHandler(ElementSelected, window, Subtree)` + `AddFocusChangedEventHandler` | Subscribing takes ~65 ms. A tab switch by UIA select or by `wt focus-tab` is reported 10–25 ms later |
| Event content per switch | A burst of 5–7 `ElementSelected` events, **mixed between the new and the old tab**, plus focus events for the new tab's `ListViewItem` and `TermControl`. Use the burst as a trigger, debounce, then read `IsSelected` |
| Focused `TermControl` name | Usually the tab title, but right after the window opened it was the profile name ("命令提示符"). Not an identity |

## Input injection (console side)

`WriteConsoleInputW` from another process attached to the tab's console
(`AttachConsole` + `CONIN$`), one key-down record per UTF-16 unit with
virtual key 0 and scan code 0, then `'\r'`:

| Target | Result |
|---|---|
| `cmd.exe` (line input mode), text `echo 你好 😀 …` | Executed; Chinese text and emoji echoed correctly |
| `cmd.exe`, text `exit` | All 62 test tabs exited with 0 and closed |
| `ssh.exe` (raw + VT input mode) to the local Windows `sshd`, text `echo NT-INJECT 你好 😀 ok` | Executed on the server; output `NT-INJECT 你好 😀 ok` with Chinese text and emoji intact |
| Same session, text `exit` | Remote shell exited, `ssh` exited with 0, the shim exited with 0, the tab closed |

## Login signal (`LocalCommand`)

Session started by the shim as
`ssh -o PermitLocalCommand=yes -o LocalCommand="<shim-probe> authenticated" 192.168.3.2`
(password authentication, typed by the user in the tab; temporary
`known_hosts` file so the user's own wasn't touched):

| Test | Result |
|---|---|
| Before the password was entered | No signal |
| After a successful login | Helper ran once, silently, with the tab's `WT_SESSION` inherited |
| Helper in a directory with spaces, `LocalCommand="<path>" authenticated` | Fired correctly (quotes survive ssh and `cmd.exe`) |
| Wrong password with `NumberOfPasswordPrompts=1` | ssh exited **255**, no signal — distinguishable from a dropped session |
| Logged in, then the server-side session process was killed (from inside the session, via injected `taskkill`) | ssh exited **255** |

The -1 exit code (connection closed without an exit status) was not
reproduced by this test; the classification still handles it.

## Portable Windows Terminal

Stable release `v1.24.11911.0` from github.com/microsoft/terminal/releases
(same version as the installed Store build). Assets include unpackaged
ZIPs for x64 (11.1 MB), ARM64, and x86 next to the msixbundle and a
Windows 10 preinstall kit. The x64 ZIP unpacks to 32 MB; `wt.exe`,
`WindowsTerminal.exe`, and `OpenConsole.exe` carry valid Microsoft
Authenticode signatures. The ZIP contains **no** `.portable` marker; it
was created by hand.

| Test | Result |
|---|---|
| Launch `<zip>\wt.exe -w ntp new-tab …` while the Store Terminal runs | A second `WindowsTerminal.exe` process from the ZIP folder; the Store instance is unaffected |
| `<zip>\wt.exe -w 0 new-tab …` | Tab lands in the portable window; the Store window's tab count is unchanged |
| Profile from a fragment in `%LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments` | Loaded by the portable instance (`--profile` opened it); its stub was written to the portable `settings\settings.json` |
| Settings location | `settings\settings.json` and `state.json` next to the exe; the Store `settings.json` wasn't modified |
| `--sessionId '{GUID}'` + shim probe | `WT_SESSION` equals the given GUID; exit 0 closed the tab |
| UIA | Same structure and class; the window is told apart by process id / image path |
| Closing the last tab | Portable process exited |

## Own tab menu (`WH_MOUSE_LL`)

`prototypes/menu-hook` (debug build, per-monitor-v2 DPI aware):

- **The hook**: a global low-level mouse hook. A right-click on a tab
  whose title starts with a prefix is swallowed (both down and up) and
  posted to a hidden tool window, which shows a Win32 popup menu with
  `TrackPopupMenu`.
- **Tab positions**: a background thread refreshes a cache of tab
  rectangles from UIA every 300 ms.
- **The hit test**: `WindowFromPoint` → root window → cached rectangles.
- **Test driver**: `click-tab` right-clicks a tab's center with
  `SendInput` (events carry `LLMHF_INJECTED`, so they go through the same
  hook path as real clicks), then restores the cursor.

Test window: `nt-menu-A` (managed) and `own-tab` (user's own).

| Test | Result |
|---|---|
| Right-click `nt-menu-A` | Swallowed; our popup visible; **no** Terminal menu opened |
| Right-click `own-tab` | Passed through; Terminal's own tab menu opened (更改选项卡颜色, 重命名选项卡, 复制标签页, 拆分选项卡, 移动选项卡, 导出文本, 查找, 关闭, 关闭标签页); closed with Escape |
| Hook callback time (4 + 2 events) | avg 42–44 µs, max 86–88 µs, far below `LowLevelHooksTimeout` |
| Hook → menu on screen | ~100 ms (posted message, menu creation) |
| `SetForegroundWindow(owner)` from the hook-triggered handler | Succeeded; the menu had keyboard focus |
| Keyboard choice (↓ ↓ Enter) | `TrackPopupMenu` returned command 2 (克隆会话) for the right tab |
| Auto-dismiss via `EndMenu` from a timer | Works |
| UIA cache refresh (2 windows, 15 tabs) | 119–145 ms per full walk (debug); must be event-driven and incremental in the real app |
| Rectangles right after a tab opens | Changed between two refreshes (90→84, 450→456): the tab-open animation. A cache that is ~2 s stale can be a few pixels off |
| DPI | UIA rectangles and hook points matched (the injected click landed on the tab) |

**Manual test by the user (real clicks, first version with
`TrackPopupMenu`):**

- **Hook:** 36 right-button events; hook time avg 28 µs, max 104 µs. The
  cache followed a window move.
- **Style:** the Win32 menu was light and plain next to Terminal's dark
  WinUI menu.
- **Focus:** `SetForegroundWindow` failed for most real clicks. Without
  the foreground, the menu didn't close on an outside click, and a second
  `TrackPopupMenu` returned immediately while the first was still open.
  The screenshot showed our menu and Terminal's menu open at the same
  time.

**Second version: a custom-drawn, non-activating popup plus a keyboard
hook.** Injected test:

| Test | Result |
|---|---|
| Right-click `nt-menu-A` | Popup shown (357×328 at 150 %, dark); **foreground stays Windows Terminal** |
| ↓ ↓ then Esc | Hover moved; Esc closed the popup and didn't reach Terminal |
| Right-click `nt-menu-B`, ↑ Enter | Chose item 6 (发送命令…) for `nt-menu-B` |
| Right-click `nt-menu-A`, then right-click `own-tab` | Our popup closed ("outside click"), Terminal's menu opened |
| Hook time (8 events) | avg 76 µs, max 181 µs (debug build) |
| Look (screenshot) | Dark rounded flyout, Fluent icons, hover highlight, separators — visually close to Terminal's tab menu |

**Manual tests by the user (real clicks, second version):**

- **Run 1 (no theme handling yet):** 40 events, hook avg 35 µs, max
  205 µs. Menus were always replaced or closed correctly, and no two
  menus were open at the same time.
- **Terminal theme set to light while Windows stayed dark:** Terminal's
  menus turned white while ours stayed dark. This led to theme
  resolution from the owning Terminal's `settings.json` (`theme.rs`).
- **Run 2 (theme handling):** the user switched Terminal to `legacyLight`
  and right-clicked NativeTerm tabs.
  - The log shows "light (Terminal theme "legacyLight")" on every open.
  - Building the menu took 4–22 ms, including reading and parsing
    settings.
  - 25 events, hook avg 42 µs, max 406 µs.
  - With Terminal on `dark`, the popup was dark and matched (screenshot).

**Third version (portable Terminal 1.26.2609.15001):**

- **Identity:** tracked by RuntimeId, so it survives title changes.
- **Mixed tabs:** detected by counting the selected tab's `TermControl`s.
- **Terminal's menu:** a "Windows Terminal 菜单…" item replays the
  right-click, and Shift+right-click passes through.

| Test | Result |
|---|---|
| Right-click `nt-menu-B` | Our popup (357×390, dark from the portable settings' default theme), 14 ms to build |
| `wt split-pane -V cmd …` on `nt-menu-B` | Tab title became "cmd"; the cache kept it as `nt-menu-B`, marked `[mixed]` |
| Right-click the split tab | Our popup with header "nt-menu-B · 此标签还有其他窗格" and "关闭此会话（保留其他窗格）" (394×500) |
| ↑ Enter → "Windows Terminal 菜单…" | Popup closed; right-click replayed 100 ms later; **Terminal's tab menu opened** (更改选项卡颜色, 重命名选项卡, 复制标签页, 拆分选项卡, 移动选项卡, 导出文本, 查找, 重启会话, 关闭, 关闭标签页 — 1.26 adds 重启会话) |
| Shift+right-click `nt-menu-A` | Passed through; Terminal's menu opened |
| Right-click `own-tab` | Passed through; Terminal's menu opened |
| Hook time (10 events) | avg 53 µs, max 207 µs |

**Manual test by the user (real clicks, third version, 3 minutes):**

- **Clicks:** 30 right-button events, hook avg 25 µs, max 134 µs.
- **Choosing items:** items chosen with the mouse were reported
  correctly (重新连接, 关闭右侧标签, 发送命令…). "Windows Terminal 菜单…"
  was used three times, each time replaying the click.
- **Window moved to a second monitor:** the cache followed within about
  0.4 s per step (x ≈ 2050–2750), and the menu opened there correctly.
- **Tracking lost on restart:** the split `nt-menu-B` was **not**
  tracked. The prototype had been restarted while that tab's title was
  "cmd", and its tracking state lived only in memory, so a restart
  forgot it. The real app can't depend on having seen the title: it has
  to re-claim tabs from the session registry, which knows each shim's
  `WT_SESSION` and which tab it opened. Until the SSH pane is focused
  again, a tab like this is recognized only by position/order hints.

**Second manual test (hook started before any change, 3 minutes):**

- **Clicks:** 42 right-button events, hook avg 23 µs, max 117 µs.
- **Split, then renamed:** `nt-menu-B` was split (title "命令提示符",
  mixed), then renamed through "Windows Terminal 菜单… → 重命名选项卡"
  to "2222". It stayed tracked as `nt-menu-B` [mixed], and the menu kept
  working.
- **Reordered by dragging:** the tabs were dragged into a new order
  (#0/#1 swapped), and several new tabs were opened (`nt-menu-A` moved
  to #5, tabs got narrower). The cache followed, with no lost claims.
- **Window changes:** a third window appeared briefly (drag-out
  attempt), and the other test window was closed. Both claimed tabs kept
  their RuntimeIds and stayed tracked. During drags a UIA walk took up
  to 670 ms.
- **Terminal's menu:** "Windows Terminal 菜单…" was used 5 times, each
  time opening Terminal's menu.
- **Windows merged:** the user then merged the two windows, which were
  both in the same portable process (pid 16380). The tabs moved *into*
  the NativeTerm window got new RuntimeIds (339–381). The tabs already in
  the destination window kept theirs (74, 79). Moving a tab between
  windows therefore creates a new element, which confirms that the
  real app re-claims moved tabs from its session registry.

**After this test:** the "Windows Terminal 菜单…" replay item and
Shift pass-through were removed by design decision. On NativeTerm tabs,
Terminal's own menu is blocked to keep its uncontrolled paths out (see
ARCHITECTURE, "Context menus").

## Re-claiming tabs, restored tabs, named windows (portable 1.26)

Setup: a hidden test profile "NT SSH Proto" was added to the *portable*
`settings.json` (backup in the scratchpad), and
`warning.confirmOnClose` was set to `never`. The profile's command line
is `shim-probe.exe restored 900`, earlier `exit 1 900`. Windows were
closed with `WM_CLOSE`. `WindowPattern.Close` via UIA did nothing.

| Test | Result |
|---|---|
| 4 single-tab windows (`new-tab cmd /k …`; `--title`; `--suppressApplicationTitle`; `--sessionId … --title … --suppress… shim-probe`), each closed | Saved workspaces keep only profile, session GUID, and directory. Every command-line override, title, and suppression was **lost**. The shim-probe tab was saved as a synthesized "Default" profile running `cmd.exe` |
| `wt -w nt-ws4 new-tab --title nt-G …` while `nt-ws4` had a saved workspace | Workspace restored (`cmd.exe`, `WT_SESSION` = the original GUID); **nt-G silently dropped**; workspace entry consumed |
| Tabs opened with `--profile "NT SSH Proto" … shim-probe exit 1 901/902`, window closed | Saved as the **profile's** command line (`exit 1 900`), title "NT SSH Proto", suppression off, original session GUIDs |
| Relaunch that workspace with a new-tab request | Restored panes ran the profile's shim with the original `WT_SESSION` (f5, f6); the request was dropped. **Sending it again worked** |
| Split tab (NT pane + `cmd` pane, `cmd` focused) | Tab name "cmd"; NT pane `TermControl` Name = "NT SSH Proto" (profile), **HelpText = "nt-I" (the `--title`)**; foreign pane Name/HelpText "cmd" |
| Profile command line `restored` mode: the shim sets `SetConsoleTitleW("nt-restored-<guid suffix>")` | Restored tabs titled `nt-restored-00f5` / `…00f7`; the split restored tab showed "命令提示符" but, when selected, its NT pane HelpText was `nt-restored-00f6` |
| `menu-hook` started fresh, pane HelpText claiming added | First scan claimed all three, the split one via HelpText as `[mixed]`; right-clicks opened our menu for each |
| Rebuilding `shim-probe.exe` while tabs ran it | Link failed: the executable is locked (the update rule applies) |

## `native-term-platform` live tests (portable 1.26)

Setup: the "NativeTerm SSH" profile (the fragment's content, pointing at
`target\debug\nativeterm-shim.exe`) was added to the *portable*
`settings.json` (backup in the scratchpad). The test serves the real
pipe and opens tabs to `nativeterm-test.invalid`, so `ssh` fails at once
and the shims wait at their prompt. Run with
`cargo test -p native-term-platform --test portable_terminal -- --ignored --test-threads=1`.
`cargo run -p native-term-platform --example close_test_tabs` closes
tabs left behind by a failed run.

| Test | Result |
|---|---|
| 3 tabs, `Target::NewWindow`, labels `nt-it web01`, `nt-it db; prod`, `-nt-it 中文 主机` | New window found by `EnumWindows`; all claimed by rule 1 with exact titles after ≈ 0.8–1.1 s (`\;` unescaped, `--title=` with a leading `-` fine) |
| Shims' hellos | `WT_SESSION` = the assigned GUID, `--session` and alias as sent |
| Select the first tab through UIA; select with a stale name | Selected; stale → `false`, nothing touched |
| `wt -w 0 new-tab --profile "NativeTerm SSH"` (placeholder) | Shim without host said hello, closed by `close` within its grace period |
| `close` to every shim | Tabs and window gone; snapshot complete, no claims, no abandoned UIA worker |
| 30 tabs with 900-character labels (2+ `wt` calls), launched through the desktop shell (`IShellDispatch2`) | All in the new window. **Without waiting between batches, tabs 25–29 were interleaved with the first batch**; with the wait, the order matched |
| Shell path, first attempt | `GetItemObject` as `IShellFolderViewDual` → `E_NOINTERFACE`; as `IDispatch` then cast → works |
| Placeholder answered too late | Started a local shell and kept reconnecting to the pipe; now it disconnects for good when it goes local |
| Test server dropping a connection right after sending `close` | Some shims missed it: a failed write (their state replay) ended the link before reading. The link now drains what's readable before reconnecting, and waits 2 s before reconnecting |

Repeated three times in a row: all passed, no shims or Terminal left.

## First app slice (portable 1.26)

The core (`native-term-app/src/lib.rs`) was live-tested with
`tests/core_portable.rs`: two hosts with the same label in a new window
became `nt-app 测试` and `nt-app 测试 (2)`; both showed "login failed
(255)" and were located after ≈ 0.8–1.3 s. Focus selected the first tab.
Reconnect ran `ssh` again in the same shim (attempt 2), and Close ended
both tabs and the window.

The GUI was driven through UI Automation (egui exposes its widgets via
AccessKit) against a throwaway ssh directory with `.invalid` hosts:

| Step | Result |
|---|---|
| Start with `--terminal-dir <portable>` | Tree with the root config and a `config.d` folder labelled 实验室; Chinese text renders |
| Double-click 实验室 A | Tab opened in the portable Terminal; "login failed (255) · window 1 · tab 1" |
| Folder menu → Connect All in New Window | New window with both hosts; the duplicate label became "实验室 A (2)" |
| Window numbers | First version numbered by Z order, so the first tab jumped to "window 2"; now numbered by first appearance |
| Focus | The portable Terminal became the foreground window |
| Tab closed with its own close button | The shim reported closing; the row showed "closed" |
| Close buttons | Tabs ended, no shims left |
| glow renderer (eframe default) | Aborted at startup: `Vec::set_len` precondition in glutin 0.32.3 `find_configs_arb` → switched to wgpu |
| wgpu, Direct3D 12 only (eframe's wgpu lacks the feature) | `FailedToCreateSurfaceForAnyBackend` until the app enabled wgpu's `dx12` feature |

Profile setup, against a fake portable folder (empty executables, a
`settings.json` with comments and a trailing comma) and a temporary
fragments folder:

| Step | Result |
|---|---|
| Start | Banner "doesn't have the profile yet" with "Install profile" |
| Install profile | Fragment written with the debug shim path; Settings showed "installed (fragment)" |
| Remove | Fragment folder gone, banner back |
| Fragment edited to `D:\Old\nativeterm-shim.exe`, restart | First read as missing: the file had a BOM (PowerShell 5.1) and was parsed as strict JSON. With the lenient parser: rewritten, notice "Updated … moved to …" |
| Side effect of the first two clicks | The Store Terminal's `settings.json` got a new timestamp (content unchanged), because installing touches every installed Terminal. With a test fragments folder only the chosen Terminal is touched now |

NativeTerm restarts and session restore (`tests/restore_portable.rs`).
Portable settings: `firstWindowPreference: persistedLayout`,
`warning.confirmOnClose: never`. Each step is a separate NativeTerm run
(`examples/core_probe.rs`) with one `state.db`:

| Step | Result |
|---|---|
| Open two tabs in a new window; next run | Both re-attached from `state.db`: "login failed (255)", linked, located, attempt 1 (after the replay-order fix) |
| Close the window while NativeTerm runs | Shims reported closing; with a 1.5 s check the window still existed (Terminal keeps its last window while saving) and nothing was restorable. With up to 10 s: marked closed with their window |
| `wt` without arguments (Terminal restores the layout) | Saved layout: the profile's command line, the original session GUIDs, title "NativeTerm SSH". Placeholders held, replaced in the same window by `--wait` tabs, then closed: the window had only "nt-r a" and "nt-r 中文 b", both "restored, not connected" |
| First attempt of the above | Placeholders answered "local shell" at once, but the shim missed the 40 ms connection (race), started `nativeterm.exe --from-shim` and fell back 18 s later |
| Connect one restored session | Attempt 1, "login failed (255)" |
| Close the window while NativeTerm isn't running, restore | Sessions still open in `state.db`: replaced the same way |
| Close all | No sessions in the next run |
| Second GUI launch | Exited with 0 and brought the first window to the front; with `--from-shim` it exited without doing so |

Whole suite: 47 s, passed twice in a row; no shims left.

Terminal change notifications (`examples/watch_events.rs`, portable
1.26), scripted session:

| Action | Notifications |
|---|---|
| New window with one tab | 1 window event (the window's own events come before its subscription) |
| Two more tabs | ≈ 70 selection/structure events within 2 s |
| Select a tab through UIA | ≈ 25 |
| Tab printing (`ping -n 10`) for 10 s | none |
| Close a tab | 5 |
| Close the window | 1 window, 1 structure |

Idle CPU with three sessions, 58–60 s (release core):

| Version | NativeTerm core | Shim |
|---|---|---|
| 2 s scan polling, pipe polling every 20 ms | — | 47 ms / 30 s (debug) |
| Events, 15 s fallback scan, pipe polling | 281–391 ms (4 scans; mostly pipe polling) | — |
| Events, 60 s fallback, overlapped pipe, shim waits on handles | 16 ms (1 scan) | 0 ms |

Session editing in the GUI (throwaway `~/.ssh` with absolute includes,
driven through UI Automation and the clipboard):

| Step | Result |
|---|---|
| New folder "Web" | `config.d\web.conf` with the folder block |
| First try | Rejected with ssh's message: the pre-existing `lab.conf` had inherited a sandbox group's ACL ("Bad permissions … CodexSandboxUsers"); after restricting the test files, accepted |
| New host "Web One 生产", host, user, port, note | Block `web-one` with `NativeTermLabel`, `NativeTermNote`, `NativeTermId` |
| Typing with SendKeys | The Sogou input method switched to its composition mode and swallowed the keys (its candidate window showed; egui's IME support works). Fields were filled by pasting instead |
| Edit: user → root | Changed in place |
| Move to Lab | Block moved with all lines; `web.conf` back to the folder block |
| Search "生产 web1" / "zzz" | One hit, with its folder / "No host matches" |
| Esc in the search box | First version didn't clear (focus is dropped in the same frame); fixed |
| Delete (confirmed) | Block gone; backups of both files in `data\backups` |
| Tree rows | First version centered the labels (`add_sized`); now drawn left-aligned |
| "✕" button | The glyph isn't in the fonts (showed a box); "×" is |
| 2000 hosts / 50 folders (release) | Start ≈ 1.2 s, 78 MB private, idle 0 CPU with and without a focused search box; a search that is typed re-scores only when the query changes |

Idle footprint, release build, no sessions, 6–10 s samples:

| Variant | Private memory |
|---|---|
| Direct3D 12, font read into memory | 221 MB |
| Direct3D 12, no CJK font | 162 MB |
| Direct3D 12, font mapped | 165 MB |
| Vulkan, no CJK font | 82 MB |
| Vulkan, font read into memory | 136 MB |
| Vulkan, font mapped (**default**) | 74–79 MB |
| Vulkan + Direct3D 12 enabled | 106 MB |
| Vulkan drivers hidden (`VK_DRIVER_FILES`) | restarted itself on Direct3D 12: 162 MB |

Idle CPU was 0 ms in every variant. With one open session, the 2 s tab
refresh cost 78 ms per 10 s (debug build). Per GPU counters, the process
used the AMD Radeon Pro 5600M, the only GPU Windows sees under Boot Camp.

### CPU renderer (release, same machine)

Where the GPU renderer's memory went (an empty eframe window and a
probe that stops at each wgpu stage, Vulkan, AMD Radeon Pro 5600M):

| Stage | Private | GPU memory |
|---|---|---|
| Process, no graphics | 2.0 MB | — |
| wgpu instance | 21.3 MB | 8.1 MB |
| + adapter | 21.3 MB | 8.1 MB |
| + device | 46.0 MB | 26.1 MB |
| + eframe window (surface, egui) | 73.7 MB | 45.6 MB |
| NativeTerm (Vulkan) | 75 MB | 55.3 MB |
| NativeTerm, wgpu GL backend | 139 MB | 43.5 MB |
| NativeTerm, Direct3D 12 | 164 MB | 79.7 MB |

The app's own share was ~2 MB, so no GPU backend could reach "tens of
MB". With the CPU renderer (`egui_software_backend` 0.0.3, egui 0.34,
own winit runner):

| Case | Private | GPU memory |
|---|---|---|
| Probe window, 200 rows, font read into memory | 29.7 MB | 0 |
| NativeTerm at start (3 hosts) | 20 MB | 0 |
| NativeTerm after opening a session and searching | 28.8 MB | 0 |
| NativeTerm, 2000 hosts / 50 folders | 25.0 MB | 0 |

- Idle CPU 0 ms in every case; start with 2000 hosts ≈ 1.45 s.
- 50 mouse-wheel steps over the 2000-host tree (1440×960 physical):
  first 722 frames and 2.5 s CPU (no vsync, smooth scrolling repainted
  continuously); with frames capped at the refresh rate, 157 frames and
  0.97 s CPU. Per frame: UI 0.9 ms, tessellation 0.06 ms, rasterization
  4.2 ms, GDI present 0.7 ms. The GPU build, same test: 0.20–0.33 s CPU and 79–87 MB private (vsync-capped, the GPU rasterizes). So scrolling costs 3–4× the CPU of the GPU build (≈ 20 % of one core while the wheel turns), while memory drops by ~55 MB and idle stays at 0. egui 0.34 spreads every wheel notch over several frames and has no setting for that; fewer frames per notch would need a change in egui. The executable shrank from 13.9 MB to 8.5 MB.
- UIA still sees every widget (AccessKit): the GUI smoke script found
  the tree, opened a session, filled the search box with Chinese text,
  and pressed "Close" through the Invoke pattern.
- Context menu and the host dialog (egui 0.34 menus and windows) render
  and work; the dialog keeps its shadow.
- First try: the window never appeared. It is created hidden, and a
  hidden window gets no `WM_PAINT`, so the redraw request never arrived.
  The first frame is now painted directly.

## SecureCRT import (synthetic configuration)

`make_crt_fixture.py` (scratch) wrote 188 folders and 736 session files
the way SecureCRT does (BOM, CRLF, `B:` blocks, `Z:` lists): nested
Chinese and ASCII folder names, a bastion per folder used as a
`Session:` firewall, two-line descriptions, fake password values, one
local forward per folder, plus Telnet, serial, RDP, a GBK session and a
named firewall. The author's real SecureCRT files were not read.

| Step | Result |
|---|---|
| Preview | 733 to import into 184 folders; RDP, serial, Telnet skipped; "Corp Proxy" and GBK reported; 549 saved passwords reported, none read |
| Import, command line, first try | Every folder rolled back: `ssh` from Git Bash was Git's MSYS build, which didn't read the `C:/…` include. Fixed with `ssh_program()` |
| Import, command line, one folder at a time | 733 hosts, 48 s |
| Import, 8 folders at a time | 29–31 s |
| Second preview | 0 to import, 733 "already imported" |
| Import from the app, first try | 288 of 733 after 130 s: a console window per `ssh -G`. Fixed with `CREATE_NO_WINDOW` |
| Import from the app | ~40 s, tree reloaded, 34 MB private afterwards |

Aliases before pinyin: `proj-00.0-3`, `proj-00.bastion-3`; with pinyin
and the folder path as prefix: `ceshi-proj-00.kongzhijiedian0`,
`shengchan-xiangmu01.bastion`.

## Docking the window (QQ-style)

Scripted check (`dock_test.ps1`, scratch; moves the window with
`SetWindowPos` and the pointer with `SetCursorPos`, reads DWM frame
bounds and `WS_EX_TOPMOST`), 3840×2088 work area:

| Step | Result |
|---|---|
| Move to 5 px below the top | frame at y = 0, on top |
| Pointer away | frame bottom at y = 4 (strip), still on top |
| Pointer on the strip | slid back to y = 0 |
| Pointer inside for 1.2 s | stays |
| Pinned, pointer away | stays; unpinned: hides |
| Moved to the middle | undocked, not on top, doesn't hide |
| Moved to the left edge, pointer away | frame right at x = 4 |
| Restart | docked left again, hides again |

First runs found three bugs:

- The window lost "always on top" when it slid away (set with
  `SetWindowPos` behind winit's back); the strip was then covered, and
  touching it did nothing. Now set through winit.
- After a restart the position came back but not the docking.
- Dragging a docked window away while its "pointer left" check was due
  hid it first and then undocked it off screen. Hiding now waits until a
  move is settled.

Idle CPU: 0 ms over 10 s docked (pointer inside), hidden, and undocked.

## Tab menu in the app (portable 1.26)

`crates/native-term-app/tests/menu_portable.rs`, three NativeTerm tabs
(`m a`, `m b`, `m c`, failed logins) and one user tab (`cmd`) in one
window, real right-clicks with `SendInput`:

- Right-click on `m b` opens NativeTerm's menu; Esc closes it (key
  swallowed, Terminal saw nothing). The menu was open by the first check
  after the click (< 100 ms).
- Right-click on the user tab opens Terminal's own menu; NativeTerm's
  open count doesn't change.
- Right-click on `m a` right after closing Terminal's menu: first run
  failed, the click went to Terminal. Cause: Terminal's menu closing sent
  UIA structure changes, which marked the rectangles stale. With the
  senders counted (`NATIVETERM_EVENT_SENDERS`), the menu's events come
  from `MenuFlyout*`/`Popup`/`Xaml_WindowedPopupClass` elements, the tab
  strip's from `ListView`/`ListViewItem`, a tab selection's from the
  terminal control and its scroll bar. Classified accordingly
  (`classify_structure_change`); passes since.
- Down, Down moves the highlight over enabled items (Disconnect is
  disabled for a failed login and skipped). Earlier the highlight
  jumped back: the popup opens under the cursor, and mouse-move /
  mouse-leave messages reset a keyboard highlight. Now only a move over
  an item changes it, and leave clears only a mouse highlight.
- "Close Tabs to the Right" on `m a` closes `m b` and `m c`; the window
  keeps `["m a", "user tab"]`, the user's tab right of them untouched.
- The first test runs missed every tab: the test process wasn't DPI
  aware (150 % display), so `SetCursorPos` and the UIA rectangles used
  different coordinates. NativeTerm itself is per-monitor aware.
- Idle cost: with no NativeTerm tabs, no hooks are installed. The global
  `EVENT_OBJECT_LOCATIONCHANGE` hook tried first fired constantly (caret,
  cursor, other apps); it is now registered per Terminal process id.
- Two runs in a row, plus the core, restore and platform live suites:
  all pass, no shims left.
- Manual check with the GUI (`实验室 A` tab, `menu-hook click-tab`):
  after the right-click, a visible `NativeTermMenuPopup` window, no
  Terminal menu open, Terminal still in the foreground. (The popup had
  closed before a screenshot was taken.)

## Hung Terminal and UIA (portable 1.26, process suspended)

The portable `WindowsTerminal.exe` was suspended with `NtSuspendProcess`
(`scratchpad/hang-test*.ps1`, which refuses any other process) and
resumed at the end.

| Check while suspended | Result |
|---|---|
| `IsHungAppWindow` | **false** throughout (~100 s): a suspended GUI thread counts as waiting for input |
| `SendMessageTimeout(WM_NULL, SMTO_ABORTIFHUNG\|SMTO_BLOCK, 250)` | "not answered" after 250 ms, every time; answered at once after resume |
| `uia-probe list` (root `FindAll` children), default timeouts | Did not return within 60 s (killed) |
| Same with `IUIAutomation2` connection + transaction timeout 500 ms | Did not return within 30 s |
| `EnumWindows` + `ElementFromHandle`, no gate, 500 ms timeouts | Did not return within 30 s |
| `EnumWindows` + `WM_NULL` gate, unresponsive window skipped | Returned in 354 ms |
| After resume, gated | 464 ms, normal results |

## Duplicate, split, restart on a NativeTerm-profile tab (portable 1.26)

Keys were sent only while the portable Terminal was the foreground window
(`scratchpad/keys-test.ps1`). Tab `nt-K` was opened with the test profile
and the override `shim-probe exit 1 6`.

| Action | Command line run | `WT_SESSION` |
|---|---|---|
| Opened by `wt` | override (`exit 1 6`) | `…00b2` (assigned) |
| Ctrl+Shift+D (duplicate tab) | profile's (`restored`) | new (`…727e`); the tab set its own title since suppression is off |
| Alt+Shift+D (duplicate pane) | profile's | new (`…5ce3`); pane HelpText showed it |
| Enter after exit code 1 (restart connection) | **override again** (`exit 1 6`) | **new** (`6789fc65…`), not `…00b2`. The source confirms it: `_duplicateConnectionForRestart` rebuilds the settings from the profile, restores only the command line, and the connection makes a fresh GUID |

## Many tabs (portable 1.26, 64 tabs in one window)

64 `cmd` tabs were opened (`nt-many-01…64`, `--title
--suppressApplicationTitle`). Tab 5 was split with a foreign `cmd` pane,
which had focus.

| Test | Result |
|---|---|
| Control-view walk (realized tabs + panes) | First run 2.4 s right after opening; then 119–127 ms, 37 realized tabs of 64 |
| `ItemContainerPattern` names | All 64 in 40 ms (0 realized) |
| Scrolling (`select-any nt-many-60`, then `…-01`) | RuntimeIds changed (e.g. `nt-many-01`: 106 → 523) |
| menu-hook v3 (RuntimeId tracking) during scrolling | Tracked count swung 36 → 10 → 25 → 37 → 11 → 36: **claims lost for tabs scrolled away**. The split tab (unselected, foreign pane focused) was never claimed; right-click opened Terminal's menu |
| All UIA properties of a tab item (split vs. plain) | Only name, rectangle, class, and runtime id. **Nothing identifies the session** |
| menu-hook v4 (full-list alignment, see ARCHITECTURE "Claiming tabs") | Start: 63 tracked (the split tab unlocated). After `focus-tab -t 4`: 64, `nt-many-05 as "cmd" [mixed]`. Scrolled to 60 and back: **64 tracked throughout** (11–37 with rectangles during the animation). Right-click on the split tab after scrolling: NativeTerm menu for `nt-many-05` (mixed) |
| Scan time, 64 + 4 tabs, 2 windows (debug) | 195–307 ms |

## plink (PuTTY 0.84): raw and Telnet (portable 1.26)

Target: a local test server (`scratchpad/tcp-test-server.ps1`, 127.0.0.1
only). It logs received bytes as hex, echoes lines, sends fixed GBK or
UTF-8 bytes on `gbk` / `utf8`, and closes on `bye`. plink ran in a
portable Terminal tab through a small `cmd` wrapper that prints its exit
code; input was injected with `shim-probe inject <plink pid>`.

| Test | Result |
|---|---|
| `-raw`: inject `hello 你好 raw`, then `bye` | Received; CR as line end; "你好" arrived as **GBK** (`c4 e3 ba c3`) with the default code page 936 |
| `-raw`: server closes | plink stayed alive, socket in **CLOSE_WAIT**; after the next injected key it exited with **1** |
| `-telnet`: connect | plink sent option negotiation (`WILL NAWS/TSPEED/TTYPE/NEW-ENVIRON`, `DO ECHO`, `WILL/DO SGA`) |
| `-telnet`: inject lines | Arrived with Telnet newline **CR NUL** |
| `-telnet`: server closes | plink exited with **0** at once |
| Code page 65001 set before plink (`chcp 65001`): inject "你好" | Arrived as **UTF-8** (`e4 bd a0 e5 a5 bd`) |
| Display, code page 936 | Server GBK bytes → 中文测试 (U+4E2D U+6587 U+6D4B U+8BD5); UTF-8 bytes → mojibake |
| Display, code page 65001 | UTF-8 bytes → 中文测试; GBK bytes → mojibake |

Follow-up (`scratchpad/load-test.ps1`, `shim-probe plink`):

| Test | Result |
|---|---|
| Temporary saved session `HKCU\…\PuTTY\Sessions\NativeTerm-proto-1` (telnet, port, `PassiveTelnet=1`, `TelnetRet=0`, `LogType=1`, `LogFileName`), `plink -load`, key deleted 2 s after start | Loaded: **no client negotiation** was sent (passive). Deletion didn't affect the running plink. `TelnetRet=0` had no effect (still CR NUL). **No session log file** |
| `plink -telnet … -sessionlog <file>` | Option accepted, **no log file** written |
| `shim-probe plink -raw …` (plink as child, TCP table polled every 300 ms), server closes on `bye` | `CLOSE_WAIT detected` within one poll; plink ended; tab shows "disconnected". No keystroke needed |

### Serial (HHD Virtual Serial Port Tools, local bridge COM30 ↔ COM31)

- **com0com first:** com0com 3.0.0.0 (the signed build from SourceForge)
  installed but **did not load** on Windows 11:
  `CM_PROB_UNSIGNED_DRIVER`, status `0xC0000428`. It was uninstalled
  completely: pair, service, device class, driver packages
  `oem128–130`, folder.
- **Then HHD:** HHD Virtual Serial Port Tools 7.35 (winget, commercial
  trial) was installed. A local bridge pair was created through its COM
  kit via the .NET interop assembly (`createBridgePort(30/31)`, each
  side's `bridgePort` set to the other). PowerShell late binding failed
  with `TYPE_E_LIBNOTREGISTERED`.
- **The device:** `scratchpad/serial-device.ps1` simulated it on COM31.

| Test | Result |
|---|---|
| `plink -serial COM30 -sercfg 115200,8,n,1,N` (console 65001): inject Enter, `hello 你好 serial`, `gbk`, `utf8` | Device got CR, then the text as UTF-8 (`e4 bd a0 e5 a5 bd`); banner and echo shown; UTF-8 reply → 中文测试, GBK reply → mojibake |
| Second plink on COM30 | "Unable to open connection: Opening '\\.\COM30': Error 5: 拒绝访问。", exit 1 |
| Device closes its end (`quit`) | plink stays alive, and still alive after another write |
| Closing the tab | plink ended; COM30 opened fine from PowerShell right after |
| Device at 9600, plink at 115200 (`emulateBaudrate` on) | Data arrived intact: the virtual bridge doesn't corrupt bits, so a mismatch can't be tested this way |

### Title suppression and elevation (portable 1.26)

This session ran elevated, so `wt` started from it went to an
**elevated** portable instance, a separate process. There, all tab names
and pane HelpText carried "管理员: ". A non-elevated instance was then
started through `explorer.exe <script>`, with elevation checked via the
process token.

| Tab | Result (non-elevated) |
|---|---|
| `--title nt-n6 --suppressApplicationTitle cmd /k "title SET-BY-APP"` (cmd profile) | Tab renamed **SET-BY-APP**: the flag had no effect |
| `--title nt-n4 --suppressApplicationTitle cmd /k "timeout …"` | Showed "nt-n4 - timeout …" while running |
| `--profile "NT SSH Proto"` (profile has `suppressApplicationTitle: true`) `--title nt-p1 cmd /k "title SET-BY-APP"` | **Stayed nt-p1** |
| Same with the flag added (`nt-p2`) | Stayed nt-p2 |
| cmd profile by GUID + flag (`nt-p3`) | Renamed SET-BY-APP |

The elevated instance behaved the same way (`nt-e4` with the flag showed
"nt-e4 - timeout …").

Earlier results in this file that relied on the flag used commands that
never set a title. They still hold for what they measured, but
suppression itself must come from the profile.

Pitfall on the way: Windows PowerShell 5.1 reads a BOM-less UTF-8 script
as ANSI, so string literals with Chinese in the first test server were
wrong. The corrected server builds the strings from code points. Serial
wasn't tested (no COM port or virtual pair on this machine).

**Observed with the portable 1.26 at launch:**

- **Previous window restored, `new-tab` arguments lost:** launching
  `wt.exe -w nt-proto new-tab --title … cmd /k …` into a *not running*
  portable instance restored the previous session's window. Its panes
  came back as plain `cmd.exe`, except one `cmd /k echo split-pane`, and
  without our titles. The requested new tabs did not appear.
  `settings.json` has no `firstWindowPreference`. The same command into
  the running instance worked normally.
  - *Answered:* the earlier test window was named `nt-proto`, and closing
    it saved a workspace under that name. `-w nt-proto` then restored it
    and dropped the rest of the command line (see "Re-claiming tabs,
    restored tabs, named windows").
- **New in 1.26, relevant to NativeTerm:** 1.26 generates SSH profiles
  (`Windows.Terminal.SSH`, `sshFolderGenerated` in `state.json`) and
  shows them in the new-tab menu (`matchProfiles` with source
  `Windows.Terminal.SSH`). With ~800 hosts in `~/.ssh`, this generator
  matters for NativeTerm users; to evaluate.

Not covered yet:

- a claimed tab moved to another window, in the prototype;
- visual confirmation of the light palette against Terminal's light menu;
- high contrast; Windows 10 (square DWM corners);
- elevated Terminal windows;
- dragging tabs, and a scrolled tab strip;
- multiple monitors with different scaling;
- release-build timings.

## Fragment stubs left behind

The earlier memory test's fragment (`NativeTermProto`, deleted
afterwards) had left two profile stubs (`"source": "NativeTermProto"`)
in the user's Store `settings.json`. Windows Terminal writes a stub for
every fragment profile it sees and never removes it. Per the source
(`CascadiaSettingsSerialization.cpp`), a stub whose source no longer
exists is marked **orphaned** and left out of the active profile list,
so it is invisible and harmless.

## Askpass helper (saved password)

The password was stored by the user with
`cmdkey /generic:<target> /user:simon /pass` (never on a command line or
on disk). The probe then ran ssh in a new tab with
`SSH_ASKPASS=<probe>`, `SSH_ASKPASS_REQUIRE=force`, and the `LocalCommand`
login signal. The helper reads the generic credential (UTF-16LE blob) with
`CredReadW`.

| Test | Result |
|---|---|
| Known host, `simon@192.168.3.2's password: ` | Answered from Credential Manager; login without any typing; login signal fired; injected `whoami & hostname` ran |
| Login-signal helper, first version | **Bug:** `LocalCommand` inherits ssh's environment, so the helper took the askpass path and waited on the console. Fixed by also checking the call shape (one argument, not a subcommand) |
| Unknown host (`StrictHostKeyChecking=ask`, fresh `known_hosts`) | ssh passed the host-key question to the helper with **no** `SSH_ASKPASS_PROMPT=confirm`. The helper showed it in the tab; the answer `yes` (injected into the console) was returned to ssh, then the password was answered from Credential Manager and login completed |
| Session exit | `exit` → ssh 0 → tab closed |

The test credential, temporary `known_hosts`, and log were deleted
afterwards.

## `ssh-copy-id` under busybox-w32

busybox-w32 6075-g169694ebd (scoop `main/busybox`), the unmodified
`contrib/ssh-copy-id` from openssh-portable `v9.5.0.0`, run from a
Windows Terminal tab as
`busybox sh ssh-copy-id [-n] -i <tmp>\id_test -o UserKnownHostsFile=<tmp> -o StrictHostKeyChecking=accept-new root@<linux-host>`
with `HOME=<tmp>\home` and the askpass probe answering the password from
Credential Manager. Target: Ubuntu, OpenSSH 10.2p1. The key was a
throwaway ed25519 key without a passphrase.

| Test | Result |
|---|---|
| `-n` (dry run) | Remote version detected via system `ssh.exe`; key login attempt failed as expected; "Would have added" the key. No password needed |
| Install | Password answered by askpass; "Number of key(s) added: 1", exit 0; server `~/.ssh` 700, `authorized_keys` 600 |
| Login with the new key (`BatchMode=yes`) | Works |
| Run again | "All keys were skipped because they already exist", exit 0 |
| Scratch directory under `$HOME/.ssh` | Created and removed by the script; the user's `~/.ssh` untouched |
| Noise | `expr: syntax error` (busybox `expr` rejects `--`, in the `-i` argument check) and a `^` regex portability warning; neither affected the result |
| Windows target (read from the script, not run) | Install step is `exec sh -c '…'` → fails in `cmd.exe`; `-s` needs ControlMaster → unusable from Windows |
| scoop package side effect | ~200 applet shims (`grep`, `ls`, `find`, …); uninstall left the orphaned `[` / `[[` shims (scoop wildcard bug), removed by hand. Coreutils, vim, and wget shims were intact afterwards |

Cleanup: the test key line was removed from the server's
`authorized_keys` (backup compared, then deleted; key login then failed
as expected), and the credential, temporary files, and busybox were
removed.

## Capturing a Terminal window for tab thumbnails (portable 1.26, 2026-09-18)

For a tiled thumbnail tab switcher (see "Taking over Ctrl+Tab" in
ARCHITECTURE.md). Only the selected tab of a window can ever be captured,
so the question was what a capture costs and whether the window has to be
in front:

- `PrintWindow(hwnd, dc, PW_RENDERFULLCONTENT)` returned a correct image
  of the Terminal window **while it was fully covered by another window**:
  25 ms for 1752×936. So thumbnails don't need the window brought to the
  front.
- A GDI+ screen copy of a 1168×624 window plus a scale to 320 px wide
  took 21 ms per capture (20 runs); the thumbnail is ~214 KB of pixels.
  A Rust `BitBlt`/`StretchBlt` should be cheaper.
- Windows Terminal itself has no per-tab image anywhere (no
  `RenderTargetBitmap`, no DWM tab thumbnails), and a non-selected tab's
  content is unparented, so nothing can be read from it — neither an
  image nor its text.

## Clearing a tab's scrollback from inside (portable 1.26, 2026-09-18)

Question: SecureCRT's "Clear Screen and Scrollback" — Terminal's own
`clearBuffer` action can't be triggered from outside (no `wt` argument,
and fragments can't bind keys). Can the process in the tab do it?

- A PowerShell tab printed 300 lines, then wrote `ESC [3J` to its console.
  The tab's text (UIA `TextPattern` on the `TermControl`) went from
  36,722 characters, line 1 included, to 3,660: only the visible screen
  was left. So ConPTY passes the sequence on, and Terminal drops the
  scrollback while the screen stays.
- Through NativeTerm's tab menu on a tab whose login had failed, the
  shim wrote `ESC [H ESC [2J ESC [3J` and printed its key hint again:
  the tab's text was the hint alone.
- While logged in, the remote side owns the screen: the shim types
  Ctrl+L, which bash/zsh answer by clearing and redrawing the prompt
  (full-screen programs redraw). Before login Ctrl+L would end up in a
  password prompt, so it isn't typed then; checked end to end with the
  fake ssh (`send_commands`).
- **Order matters.** In Windows Terminal, `ESC [2J` doesn't discard the
  screen: it moves it into the scrollback. The first version wrote
  `ESC [3J` and then typed Ctrl+L; the remote shell's `ESC [2J` arrived
  afterwards and pushed the old screen back into the scrollback, so the
  scrollbar stayed and a second click was needed (reported by the user).
  Measured with Git bash as the "remote side" in a tab (`seq 1 300`,
  then clear): `ESC [3J` + Ctrl+L left 31 lines; `ESC [H ESC [2J ESC [3J`
  (the tab cleared here, the screen's move into the scrollback cleared
  with it) + Ctrl+L left only the prompt. Repeated through the real tab
  menu with the shim, the fake ssh running bash (`FAKE_SSH_INTERACTIVE`),
  and 600 lines: one click, prompt only, no scrollbar.

Caveat: the shim writes to the console while ssh may be writing too; a
sequence could land between two parts of the remote output's own escape
sequence. Clearing is a user action on an idle tab in practice.

## Windows Terminal settings reload

| Test | Result |
|---|---|
| Write a fragment file | Not picked up: Windows Terminal only watches `settings.json` |
| Then update `settings.json`'s modification time (content unchanged) | Settings reloaded, fragment profiles usable immediately (`wt -p "NT Proto 1000"`) |
| Delete the fragment, touch `settings.json` again | Profiles gone |

So NativeTerm must touch `settings.json`'s timestamp after changing its
fragment, instead of asking the user to restart Windows Terminal.

## Memory

60 idle `cmd` tabs opened at once, default profile (`historySize` 9001),
measured after the tabs settled:

| Process | Before | After 60 tabs | Per tab |
|---|---|---|---|
| WindowsTerminal.exe private bytes | 1160.7 MB | 1628.5 MB | **≈ 7.8 MB** |
| WindowsTerminal.exe working set | 480.8 MB | 750.7 MB | ≈ 4.5 MB |
| OpenConsole.exe (one per tab), private | — | — | ≈ 2.4 MB |
| ssh.exe (8 sessions already running on the machine), private | — | — | ≈ 3.2 MB (≈ 10 MB working set) |

After closing the 60 tabs, Windows Terminal returned to 1146.5 MB private.

Estimate for one SSH tab: ≈ 7.8 (terminal) + 2.4 (console host) + 3.2
(ssh) + ~1 (shim, to be measured) ≈ **14 MB private**, i.e. roughly
850 MB for 60 sessions.

### Scrollback size

20 tabs each printing 20,000 lines of ~100 characters (so the scrollback
is full), through two test profiles installed as a fragment:

| `historySize` | Windows Terminal private bytes, increase for 20 tabs | Per tab |
|---|---|---|
| 9001 (default) | 258.6 MB | ≈ 12.9 MB |
| 1000 | 162.7 MB | ≈ 8.1 MB |

Baselines were measured right before each set (≈ 1826–1830 MB — the
user's own AI tabs kept producing output, so only increases are
compared). The tabs closed themselves afterwards (exit 0).

Observations:

- Each tab has a floor of roughly 8 MB in Windows Terminal; a full default
  scrollback adds about 5 MB more.
- Per SSH tab, then: ≈ 8–13 MB (terminal, depending on scrollback) +
  2.4 MB (console host) + 3.2 MB (ssh) + shim (to be measured), i.e.
  **≈ 15–20 MB private**, or roughly 0.9–1.2 GB for 60 sessions.
- The user's own AI coding tabs dominate overall: Windows Terminal's
  baseline grew from 1.1 GB to 1.8 GB during the session with 12 of them.
### Comparison with Tabby (Electron)

Tabby 1.0.234 (portable) was started with a temporary configuration
(`TABBY_CONFIG_DIRECTORY` plus `--user-data-dir` pointing to a temp
folder; the user's own Tabby configuration was untouched, and the temp
folder was deleted afterwards), scrollback set to 9001 lines like Windows
Terminal's default. 20 tabs were opened with `Tabby.exe run <command>`,
each printing the same 20,000 lines.

| | Idle baseline | Increase per tab (full scrollback) |
|---|---|---|
| Windows Terminal | none extra (already running for the user's own tabs) | ≈ 13 MB (+ ≈ 2.4 MB console host, + ≈ 3.2 MB `ssh.exe` for SSH tabs) |
| Tabby | 566 MB (5 processes) | ≈ 36 MB (samples at 30 s and 50 s: 1315 / 1269 MB total) |

Estimate for 60 SSH sessions: NativeTerm's approach ≈ 1.2 GB (≈ 0.9 GB
with a 1000-line scrollback); Tabby ≈ 2.7 GB.

Notes:

- Tabby's CLI turns numeric arguments into numbers, after which the
  command fails to start silently; the probe got an argument-free mode
  (`spew-default`) for this test.
- The Tabby window was closed by the user after about a minute (normal
  shutdown in the log, no crash reports); the samples above were taken
  before that.
- The slow TypeScript tool the user remembered was probably electerm, not
  Tabby; no further comparison was considered necessary.

Conclusion: the native approach is roughly **2× lighter** than Tabby for
this workload — a real but not decisive advantage, and not "an order of
magnitude". NativeTerm's case rests mainly on multi-server session
management (which Tabby handles poorly) and on the native terminal's
rendering and behavior, with memory as a secondary benefit.
