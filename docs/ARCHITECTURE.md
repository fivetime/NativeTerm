# Architecture

## The core constraint

Two requirements that are normally satisfied by the same thing (a terminal
emulator with tabs) are deliberately split apart here:

1. **Terminal rendering must be the OS's own native terminal.** Not
   Electron/WebView, not a self-rendered emulator (even a GPU-accelerated
   one like Alacritty/WezTerm would count as "self-rendered" for this
   purpose). Reasons:
   - at 60+ concurrently open sessions across a large fleet, per-session
     memory overhead matters. Measured (see `docs/PROTOTYPES.md`): about
     15–20 MB private per SSH tab depending on scrollback, versus about
     36 MB per tab plus a 566 MB baseline for Tabby (Electron) under the
     same load — roughly half, a useful but secondary advantage;
   - with AI coding tools, terminals now carry rich output — true color,
     box drawing, CJK and emoji widths, clickable links, synchronized
     output — which legacy tools like SecureCRT don't render properly,
     while Windows Terminal does.
2. **SecureCRT-style session *management*** (a saved-session tree, clone,
   close-others, close-disconnected, close-to-the-right, send commands)
   layered on top.

No terminal emulator on any platform exposes an embeddable "give me your
tab strip" control to a third-party process:

- **Linux** is the one exception — GTK's `VTE` widget is a genuinely
  official, reusable, embeddable widget (this is how GNOME Terminal,
  Terminator, and Tilix are themselves built). Worth revisiting for the
  Linux backend.
- **Windows**: no official embeddable control. Community projects
  (`WindowsTerminal.WinUI3.Control`, `EasyWindowsTerminalControl`) wrap the
  real Windows Terminal backend into a WinUI3/WPF control, and there's the
  older `SetParent`-based conhost-reparenting trick ConEmu/Cmder use — both
  unofficial, both .NET/WinUI- or Win32-specific.
- **macOS**: no embedding story. Terminal.app exposes nothing equivalent to
  VTE.

So NativeTerm neither embeds nor renders a terminal. Instead it **drives
the OS terminal's own window and tab strip from the outside**.

## SSH sessions are tabs in the user's own terminal window

NativeTerm opens SSH sessions as tabs of the Windows Terminal window the
user is already working in, next to the user's own tabs (local shells, AI
coding tools, anything opened with `+`). The terminal's own tab strip,
rendering, fonts, hyperlink handling (Ctrl+click), and IME support are
used as-is.

- **Default target**: the most recently used Windows Terminal window
  (`wt -w 0`).
- **Connect in Tabs in New Window** (a daily need, as in SecureCRT):
  opening a folder or a selection into a **new** Terminal window,
  optionally a clone into a new window, is supported. See "Opening
  sessions in a new window".
- **Optional**: a dedicated named window (`wt -w NativeTerm`) for users who
  want SSH sessions kept apart.

### Opening sessions in a new window

Verified on portable 1.26:

- `wt -w new new-tab … ; new-tab …` opens **one** new window holding all
  the tabs of that command line.
- The window is **unnamed**, so closing it creates no workspace entry.
  A named window would be saved as a workspace and would swallow a later
  `-w <name>` command (see "Restored tabs and named windows"), so
  NativeTerm never names these windows.
- A follow-up `wt -w 0 new-tab …` lands in the new window while it is
  the most recently used one.

Batching:

- **Size:** one tab costs about 270 characters of command line (profile,
  session GUID, label, shim path, `--session`, alias). One `wt`
  invocation therefore carries about **100 tabs**, safely below the
  32,767-character limit.
- **The first batch** goes to `-w new`, and later batches to `-w 0`.
  - **`-w 0` means Terminal's most recent window, not the foreground
    window.** A new window counts as the most recent one from its
    creation (`AppHost.cpp`: "the creation of a new window marks it as
    the most recent one immediately, even before it becomes active"). So
    it doesn't matter that NativeTerm or another app has the foreground.
    Only activating *another window of the same Terminal install* moves
    the target. Before each later batch, NativeTerm checks exactly that.
  - **Each batch waits for the previous one.** Terminal builds the tabs
    of a command asynchronously. A second `wt` call sent before the
    first batch's tabs exist gets **interleaved** with them (verified:
    tabs 25–29 landed between tabs 05 and 10). So NativeTerm sends the
    next batch only once every title of the previous batch shows up in
    UIA (names only; claims are left alone). With 30 tabs in two batches
    the strip order then matched the request.
  - If the user moved to another Terminal window, or a batch doesn't show
    up within 30 s, the remaining tabs are returned as pending: they wait
    until the new window is active again, or the user chooses "open the
    rest here".
- **Connections are paced separately.** All tabs appear at once, but
  each shim waits for NativeTerm's go-ahead, so logins follow the
  connection queue and rate limit (jump hosts aren't hammered).
- **A tab that never appears is asked for once more.** `wt` returns as
  soon as it has handed the command over, so a command Terminal drops
  (busy, mid-restore, a window closing under it) is silent. After the
  confirmation wait, each label that was never claimed is sent again,
  once, under a **new terminal GUID** — the first tab may still turn up
  late, and two tabs carrying one GUID cannot be told apart. The label
  and NativeTerm's own session id stay, so it is the same session and
  the same claim. The retry goes to the window this try made (`-w 0`
  after activating it), never to a window of its own. It is skipped when
  a Terminal window could not be read (the tab may be in it, and a
  second one would be a duplicate) or when that session's shim is
  already talking to us (then the tab is there, under a name we didn't
  expect). `command::SWALLOW_ENV` drops the first `wt` command for a
  label so the whole path can be live-tested.

The other SecureCRT variants:

- **"Connect in Tabs in New Tab Group":** Windows Terminal has no tab
  groups, so this maps to a new window as well.
- **"Send to New Window"** (moving a running tab): `wt` has no command
  for it. The user drags the tab out; NativeTerm re-claims it there.

```
┌──────────────────────────┐          ┌──────────────────────────────────────┐
│      NativeTerm (GUI)     │  wt CLI  │ Windows Terminal (user's window)      │
│ - reads ~/.ssh/config     │─────────▶│ ┌────────┬────────┬───────┬───────┐  │
│ - session tree sidebar    │          │ │ claude │ pwsh   │ ssh A │ ssh B │  │
│ - tab switcher + search   │   UIA    │ └────────┴────────┴───────┴───────┘  │
│ - FAB, command input      │◀────────▶│  user's own tabs   NativeTerm's tabs │
│                           │named pipe│  (not managed)     run the shim:     │
│                           │◀────────▶│         nativeterm-shim <host-alias> │
└──────────────────────────┘          └──────────────────────────────────────┘
```

NativeTerm talks to Windows Terminal through three channels. Both Windows
Terminal (github.com/microsoft/terminal) and Windows OpenSSH
(github.com/PowerShell/openssh-portable, reviewed at tag `v9.5.0.0`, the
version shipped with the author's Windows as `OpenSSH_for_Windows_9.5p2`)
are open source; the behavior described in this document was checked
against their source unless marked otherwise.

1. **`wt` command line** — create tabs:

   ```
   wt -w 0 new-tab --sessionId {<guid>} --title "<label>" --suppressApplicationTitle
                   --profile "NativeTerm SSH" nativeterm-shim <host-alias>
   ```

   `wt` reports nothing on bad input: it exits with 0 and simply doesn't
   open the tab (e.g. Windows Terminal 1.24 requires the GUID **in
   braces**). NativeTerm therefore confirms every new tab through UIA and
   reports a tab that never appeared.

   - `-w 0`: the most recently activated window on the current virtual
     desktop (a new window if there is none). An explicit `-w` always
     overrides the user's "new instance behavior" setting, which only
     applies when `-w` is absent.
   - `--sessionId`: NativeTerm generates the GUID. Windows Terminal passes it
     to the tab's process as the `WT_SESSION` environment variable, so the
     shim learns its session identity without any command-line argument,
     and it survives session restore and moving tabs between windows —
     but **not** "Restart connection", which assigns a new GUID (see "Tab
     identity"). The shim's command line therefore also carries
     NativeTerm's own session id (`nativeterm-shim --session <id>
     <host-alias>`), which a restart keeps.
   - `--title` makes the tab title the user's own label.
   - **Title suppression lives in the profile.** The "NativeTerm SSH"
     profile sets `"suppressApplicationTitle": true`, which stops the
     remote host (via escape sequences) from overwriting the label.
     Verified on 1.26, non-elevated and elevated:
     - **The command-line flag `--suppressApplicationTitle` has no
       effect**: `cmd /k "title X"` retitled such a tab.
     - **The profile setting works:** the same command left the label in
       place, with or without the flag.
     NativeTerm still passes the flag, which is harmless, but relies on
     the profile. A welcome side effect: restore and "Restart
     connection" rebuild settings from the profile, so suppression
     survives both.
   - **Keep the command itself trivial.** `wt` re-joins command arguments
     with spaces and wraps an argument in quotes only if it contains a
     space, without escaping embedded quotes; `;` separates `wt`
     subcommands (`\;` for a literal one). So the command is only
     `nativeterm-shim <host-alias>` (aliases are sanitized: no spaces,
     quotes, or `;`), and the shim builds the full `ssh` command line
     itself — options such as `LocalCommand="..."` would otherwise be
     mangled.
   - `wt` has no subcommand to list or close tabs. Switching tabs is done
     through UIA rather than `wt focus-tab` with an index.
2. **UI Automation (UIA)** — the Windows accessibility interface screen
   readers use. Across all Windows Terminal windows, UIA exposes each tab's
   title, the tab order, which tab is selected, select and close actions,
   and selection-change events. Verified in the prototype
   (`docs/PROTOTYPES.md`):
   - Structure: `TabView` → `ListView` → one `ListViewItem` per tab; its
     `Name` is the tab title (for a tab split into panes, a generic
     "multiple panes" text); its close button is a `Button` child with a
     localized name, so it is found by control type.
   - `FindAll` doesn't cross into Windows Terminal's XAML island; the
     tree is traversed with a tree walker.
   - **Tabs scrolled out of view are virtualized** and missing from the
     tree. The list's `ItemContainerPattern` enumerates *all* tabs in
     order, names included, and is faster than walking (26 ms for 63
     tabs). A virtualized tab is selected with
     `VirtualizedItemPattern.Realize()` + `SelectionItemPattern.Select()`.
   - `RuntimeId` is **not** stable: recycled containers get new IDs after
     scrolling. It is only a short-lived handle (see "Tab identity").
   - The terminal control (`TermControl`) implements `TextPattern`; only
     the selected tab's control exists, so only its text is readable.
   - Risk: this structure is not a documented contract and may change
     between Windows Terminal versions.
3. **`nativeterm-shim`** — a tiny helper process that is the first process
   of every NativeTerm tab. It launches `ssh` as its own child in the same
   console, waits for it, and talks to NativeTerm over a named pipe.

No other public control surface exists: `wt.exe` forwards its command line
to the running instance through a private window message, and the only COM
server is for default-terminal handoff. The tab right-click menu and the
terminal-area right-click menu are hard-coded and can't be extended.

### Context menus: what the Terminal source allows

Checked in the Terminal source (1.24):

- **Terminal-area menu**: fixed items in `TermControl.xaml` (paste,
  select command/output, find; copy etc. with a selection) plus a list
  built in code in `TerminalPage::_PopulateContextMenu` (duplicate tab,
  split pane, swap/close panes, restart connection, search web, close tab).
  A comment there leaves "room for customizing this menu with actions in
  the future"; nothing reads user settings today. The menu only opens on
  right-click when `rightClickContextMenu` is on (default: right-click
  pastes).
- **Tab menu**: hard-coded as well.
- **Actions** (`AllShortcutActions.h`): none runs an external program.
  Fragment actions reach the command palette, but only with built-in
  behavior; e.g. `closeOtherTabs` would also close the user's own tabs.
- **`searchWeb` with a custom URL scheme** (`nativeterm://…`): custom
  schemes are allowed (`_IsUriSupported`), but the action only fires with
  a text selection, and schemes not listed in the global `safeUriSchemes`
  (not settable from a fragment) show a confirmation dialog on every use.
  Not usable as a menu.

Therefore NativeTerm provides its own menus:

1. **Right-click on a NativeTerm tab → NativeTerm's menu** (prototype:
   `prototypes/menu-hook`, results in `docs/PROTOTYPES.md`). A low-level
   mouse hook (`WH_MOUSE_LL`) sees right-clicks anywhere. If the point is
   over a tab item that belongs to a NativeTerm session in a Windows
   Terminal window, the click is swallowed and NativeTerm shows its own
   popup (reconnect, clone, close others / disconnected / to the right,
   lock, send commands, …). Right-clicks on the user's own tabs and in
   the terminal area pass through unchanged, so Windows Terminal's menu
   and right-click paste keep working. Rules:
   - The hook callback does no UIA work: Windows removes hooks whose
     callbacks exceed `LowLevelHooksTimeout`, and a slow callback lags
     the whole desktop's mouse. The hit test uses a cache of tab
     rectangles (UIA `BoundingRectangle`), refreshed on selection,
     structure, and window move/resize events, and a cheap
     `WindowFromPoint` check that the point is inside a Terminal window.
   - Swallow both the button-down and the matching button-up, then show
     the menu from the NativeTerm thread, not inside the callback.
   - **The popup never takes focus.** A right-click menu doesn't need
     the foreground, and a hook-triggered handler usually can't get it
     anyway: with real clicks, `SetForegroundWindow` failed most of the
     time. A Win32 `TrackPopupMenu` menu without the foreground doesn't
     close on outside clicks, and a second one fails while the first is
     open, so it stayed on screen next to Terminal's own menu. The popup
     is therefore a non-activating window (`WS_EX_NOACTIVATE`,
     `MA_NOACTIVATE`); Windows Terminal stays the active window. The
     mouse hook closes it on any button press outside it (the click
     still goes through), and on a right-click on another NativeTerm tab
     it replaces it. While it is open, the keyboard hook takes
     Up/Down/Enter/Esc (and their key-ups), and any other key closes it
     and passes through.
   - **It looks like Terminal's own menu.** A Win32 menu is light and
     plain next to Terminal's dark WinUI flyout. The popup is custom-drawn
     after the WinUI MenuFlyout:
     - metrics: 32 epx rows, 4 epx inset, a hover highlight with 4 epx
       radius, the icon at 16 epx, the text at 44 epx, 14 px Segoe UI;
     - icons: Segoe Fluent Icons glyphs (Segoe MDL2 Assets on
       Windows 10);
     - window: DWM rounded corners and border color, drop shadow;
     - colors: the flyout's solid colors for dark and light, scaled by
       the monitor's DPI;
     - theme: follows **the owning Terminal's** `theme` setting, which
       can differ from Windows (e.g. Windows dark, Terminal light). The
       settings file is found from the window's process image:
       `%LOCALAPPDATA%\Packages\<family>\LocalState` for packaged
       installs, `settings\` next to a portable one (`.portable`),
       `%LOCALAPPDATA%\Microsoft\Windows Terminal` for other unpackaged
       ones. `theme` is a name or a `{"dark": …, "light": …}` pair
       picked by the Windows app theme; the name is looked up in the
       user's `themes` (`window.applicationTheme`) or the built-ins
       (`light`/`dark`/`system` and `legacy*`, default `dark`).
       `system` and unreadable settings fall back to the Windows app
       theme (`AppsUseLightTheme`). Read on every open (~20 ms in the
       prototype, debug build); the real app caches it and reloads on
       settings-file and `WM_SETTINGCHANGE` notifications;
     - high contrast (`SPI_GETHIGHCONTRAST`): system colors
       (`COLOR_WINDOW`, `COLOR_WINDOWTEXT`, `COLOR_HIGHLIGHT`,
       `COLOR_HIGHLIGHTTEXT`) instead of the palette;
     - Windows "Text size" (`TextScaleFactor`, 100–225 %): text and row
       height grow like WinUI's;
     - acrylic: Terminal's menus looked solid in both themes here; if
       needed, the real app uses the DWM transient backdrop with
       Direct2D (GDI can't draw with alpha);
     - Windows 10 ignores the DWM corner attribute, while Terminal's
       WinUI 2 menus are still rounded there; the popup then draws its
       own rounded shape and shadow (layered window).
     The real app draws it with Direct2D/DirectWrite (color emoji, font
     fallback) instead of the prototype's GDI.
   - A stale cache (tab dragged, strip scrolled) must fail safe: on any
     doubt, let the click through.
   - **Identity survives title changes.** A tab claimed as a NativeTerm
     session stays claimed by its UIA element (RuntimeId) while its
     title changes. This covers a split tab showing the focused pane's
     title, a user rename, and AI-style live titles in a foreign pane.
     The claim ends when the element disappears (closed, or moved to
     another window, which creates a new element); the real app
     re-claims moved tabs through the session registry. When a recycled
     container changes RuntimeId (scrolled strip), the claim is renewed
     the next time the session's title shows.
   - **Mixed tabs.** When a claimed tab is selected and its view has more
     than one `TermControl`, it is marked mixed: the user split it, and
     the other panes are the user's own (local shells from the
     duplicated profile). The menu then starts with a header ("<session>
     · 此标签还有其他窗格"), scopes session actions to the SSH pane, and
     offers "关闭此会话（保留其他窗格）". Ending the shim closes only its
     pane. Batch closes that would close the whole tab ask first.
   - **Terminal's own menu is blocked on NativeTerm tabs, by design.**
     Its duplicate, split, move, and restart items create states
     NativeTerm doesn't control: foreign panes, new elements, and
     profile copies without a shim. Blocking the menu keeps that
     complexity out. NativeTerm's menu offers the equivalents that fit
     its model (clone instead of duplicate, its own rename, which applies
     at the next open, and its own close variants).
     - **A replay that was tried and removed:** the prototype once had a
       "Windows Terminal 菜单…" item that replayed the right-click with
       the hook bypassed, from a separate thread. It worked, but it
       reopened exactly the uncontrolled paths. Shift+right-click
       pass-through was removed for the same reason.
     - **The user's own tabs keep Terminal's menu unchanged.**
     - **Splits can still happen:** keyboard shortcuts (e.g.
       `alt+shift+d`) can still split a NativeTerm tab, since a fragment
       can't unbind keys. So mixed-tab handling stays.
   - Elevated Terminal windows: input to them may not reach a
     non-elevated hook (UIPI); their tabs simply keep Terminal's menu.
   - Measured: callback ~30–80 µs (max ~180 µs, debug build), menu on
     screen ~100 ms after the click. A full UIA walk takes
     120–150 ms, so the rectangle cache is updated from UIA events (and
     only for changed windows), not by polling. Rectangles move for a
     moment while a tab opens (animation); refresh again shortly after
     structure changes.
   - **Implementation (first version).** `native-term-platform`
     (`windows_terminal::menu`) owns the hooks and the popup, with no
     knowledge of sessions; `native-term-app` (`tab_menu`) supplies the
     entries and runs the chosen action through the core (a `Provider`).
     - One menu thread per process owns an invisible owner window, the
       popup (class `NativeTermMenuPopup`) and the hooks. The hook
       callbacks only read atomics and `try_lock` the rectangle list, then
       post to that thread (`WM_SHOW_MENU`, `WM_CLOSE_MENU`,
       `WM_MENU_KEY`).
     - **The hooks exist only while NativeTerm has located tabs.** Each
       core scan hands the claimed tabs (window, rectangle, label, title,
       mixed, index) to `set_tabs`; with an empty list both hooks are
       removed, so a NativeTerm without open tabs adds nothing to the
       desktop's input path.
     - **Stale rectangles pass the click through.** Any change that can
       move tabs (`Tabs`, `Windows`, `Moved`, see below) marks the list
       stale until the next scan (debounced, ~150 ms after the last
       event); a right-click in that window goes to Terminal. The hit test
       also requires `GetAncestor(WindowFromPoint(pt), GA_ROOT)` to be the
       tab's window, so a window covering the tab strip gets its click.
     - **What a UIA structure change means depends on the sender**
       (`classify_structure_change`, measured on 1.26 with
       `examples/watch_events` and `NATIVETERM_EVENT_SENDERS`): the tab
       strip reports through its `ListView`/`ListViewItem`s (and the
       Terminal window when it is created) → `Tabs`; Terminal's own tab
       menu reports through `MenuFlyout*` items, their text, `Popup` and
       `Xaml_WindowedPopupClass` → `Popup`; selecting a tab reports
       through the terminal control and its scroll bar → `Content`.
       `Popup` is ignored (no scan). Before this split, opening and
       closing Terminal's own menu marked the rectangles stale, and a
       right-click on a NativeTerm tab right after it went to Terminal.
       `Content` triggers a scan (pane changes) but doesn't invalidate
       the rectangles.
     - Window moves and resizes: `EVENT_OBJECT_LOCATIONCHANGE` is hooked
       **per Terminal process id** only (the hooks are re-synced when
       Terminal windows appear or go), since a global location hook
       fires for every caret and cursor move on the desktop. The core's
       debounce waits until the events stop (up to 20 × 150 ms), so a
       drag produces one scan at the end.
     - Painting is Direct2D with DirectWrite text (`menu_draw`): Segoe UI
       with DirectWrite's font fallback (CJK in the user's locale, color
       emoji through `ENABLE_COLOR_FONT`), icons from Segoe Fluent Icons
       or Segoe MDL2 Assets on Windows 10; a DC render target draws into
       the popup's paint DC in software (no GPU needed), in physical
       pixels; measuring uses `IDWriteTextLayout`. DWM gives the rounded
       corners, border color and shadow on Windows 11. The theme (`theme::look`) is read
       from the owning install's settings on every open; high contrast
       and text size apply as described above. The Terminal theme from
       `settings.json` is cached by the file's time and size (parsing it
       was the slow part); the Windows theme, high contrast and text size
       are read on every open (registry and `SystemParametersInfo`,
       microseconds). Acrylic was decided against: Terminal's own menus
       are solid in both themes. Where DWM doesn't round popups
       (Windows 10: build below 22000, from `CurrentBuildNumber`), the
       popup is a layered window of its own class (without
       `CS_DROPSHADOW`, whose rectangle would show at the corners): the
       second, premultiplied-alpha Direct2D target draws the rounded
       border and background with grayscale text into a 32-bit DIB for
       `UpdateLayeredWindow`, redrawn directly on every highlight change
       (a layered window gets no `WM_PAINT`). `NATIVETERM_MENU_LAYERED=1`
       forces this path; the live menu test passes on it and a capture
       on Windows 11 shows the shape. There is no shadow on that path.
     - Highlight: Up/Down move over enabled items only. A mouse move
       changes the highlight only over an item, and leaving the popup
       clears only a highlight the mouse set, so the keyboard highlight
       isn't lost when the cursor rests elsewhere.
     - Items (localized like the rest of the app): Connect/Reconnect (a waiting or ended
       session with a shim), Disconnect (connecting or connected),
       Clone Session (same alias, base label, most recent window),
       Close (for a mixed tab: "Close This Session (keeps the other
       panes)"), Close Other NativeTerm Tabs, Close Disconnected Tabs
       (all windows), Close Tabs to the Right. Every close set contains
       only NativeTerm sessions; the user's tabs between or right of them
       are never touched. A mixed tab or a renamed one gets a header
       line. Lock, send commands and the confirmation for closing a
       whole mixed tab are still open.
     - Tests: `crates/native-term-app/tests/menu_portable.rs` right-clicks
       real tabs with `SendInput` in the portable Terminal. The test
       process must be per-monitor DPI aware, like NativeTerm, or the
       cursor position and the UIA rectangles disagree on a scaled
       display. It picks items through `choose(id)` (`WM_CHOOSE_ID`)
       after checking the keyboard highlight, instead of counting
       Down presses.
2. **Keys in a disconnected tab.** After `ssh` exits, the tab's first
   process is the shim, so it can show "R reconnect · D clone · C close"
   and read the key itself — the same pattern as Terminal's own "press
   Enter to restart", without any menu support.
3. **Long term: an upstream contribution** to Windows Terminal so that
   context menus can include actions (including fragment actions),
   following the direction the source comment already names.

## Feature-by-feature mapping (SecureCRT menu → NativeTerm)

| SecureCRT feature | Possible? | How |
|---|---|---|
| Rename / Reset Name | Yes | Our own metadata; an open tab keeps its title (its identity, and suppressed application titles can't change it), its card shows the new name, and clones and replacement tabs use it |
| Reconnect | Yes | Tell the tab's shim to run `ssh` again in the same tab |
| Disconnect / Close | Yes | Tell the shim to end `ssh` / to exit with code 0, which makes Windows Terminal close the tab (see "Closing tabs") |
| Close Disconnected Tabs | Yes | Shim reports ssh exit code 255 → those shims exit with 0 |
| Close Other Tabs / Close Tab Group | Yes | Batch over NativeTerm's own tabs only |
| Close Tabs to the Right | Yes | Uses the **real tab order**, read via UIA |
| Clear Screen and Scrollback | Yes | NativeTerm's tab menu. After login the shim clears the tab (`ESC[H ESC[2J ESC[3J`: Terminal moves the screen into the scrollback on `2J`, so `3J` comes after it) and types Ctrl+L, so the remote side redraws on an empty screen without pushing anything back into the scrollback. Before login it only clears the scrollback (a password prompt may be showing); without ssh running it clears the tab. Verified through ConPTY (PROTOTYPES.md). Terminal's own `clearBuffer` can't be triggered from outside |
| Lock | Yes | Our own flag (`state.db`), excluded from batch closes and from "All" in group send; Close is disabled until unlocked. Windows Terminal's own close button can't be blocked |
| Clone Session | Yes | New tab with the same host and a unique title; port forwards cleared |
| Connect in Tabs in New Window / Clone in New Window | **Yes** | `wt -w new` (unnamed window), batches of ~100 tabs; see "Opening sessions in a new window" |
| Connect in Tabs in New Tab Group | As a new window | Windows Terminal has no tab groups |
| Send to New Window (move a running tab) | Manual | No `wt` command; the user drags the tab out and NativeTerm re-claims it |
| Save Session | Yes | Writes a `Host` block into `~/.ssh/config.d/<folder>.conf` |
| Font | Indirect | Windows Terminal profile, selected per folder/host |
| Connect SFTP / Open SecureFX | **Yes** | Built in: "Files (SFTP)…" on a host, see "File transfer (SFTP)" |
| Send Commands to Active Session | Yes | See "Sending commands" |
| Send Commands to This Group | **Yes** (source-confirmed) | Shim-based console input injection, confirmed against both the console host and Windows OpenSSH sources; an end-to-end prototype remains. `tmux send-keys` fallback for persistent sessions |
| Session logging (record all output) | **SSH: no** (client side); **other protocols: yes** | SSH output goes straight into the terminal; NativeTerm never sees it. Server-side logging for persistent sessions ("tmux, recorded on the server", see "Persistent remote sessions (tmux)"); Windows Terminal's own "Export text" saves a tab's buffer manually. Telnet / serial / raw / rlogin / SUPDUP sessions are logged by ntplink (see "ntplink: NativeTerm's own client") |
| Telnet / serial / raw / rlogin / SUPDUP | **Yes** | The shim runs PuTTY's console client `plink.exe`; NativeTerm parses no protocol. See "Other protocols via plink" |
| Local shells / AI coding sessions | Not managed | The user opens them with `+`; NativeTerm only lists them in the tab switcher |
| Per-session character set (e.g. GBK) | **Yes**, for every session type | Nothing converts along the way, so the shim sets the tab console's code pages: `NativeTermCharset` for SSH hosts (per host or folder), the session's `charset` for the others (verified both directions); see "Character sets" |
| Port forwarding | Via ssh config | `LocalForward` etc. in the host block; edited in the session options dialog |

### Session options (SecureCRT "Session Options" → NativeTerm)

NativeTerm has a session options dialog with the same categories.
- **What it edits:** the host block (SSH) or the `.nt.toml` entry
  (plink).
- **Algorithm lists:** offered from `ssh -Q cipher|mac|kex|key` on this
  machine.
- **Validation:** every save is checked with `ssh -G`.
- **Reach:** what is set there also works for command-line `ssh`, `scp`,
  and VS Code Remote.
- **Implemented (SSH):** the host context menu's "Session Options…".
  The option list lives in `native_term_config::options::SPECS`
  (keyword, category, kind: text, choice, algorithm list, or repeated
  lines). Values are kept exactly as written after the keyword, so
  quoting and ssh's `+`/`-`/`^` list prefixes pass through; only
  keywords whose values changed are rewritten (in place where the line
  exists). Empty fields show what `ssh -G` currently resolves. A list
  picker starts from the field's explicit list, or from the effective
  one when the field is empty or only adjusts the default. Options for
  a whole folder use ssh's own mechanism instead of the folder's
  defaults block (whose host name never matches): see "Folder options".

### Proxies (SOCKS5, SOCKS4, HTTP)

Windows' OpenSSH ships no `nc` or `connect`, and OpenSSH has no proxy
option of its own besides `ProxyCommand`. So the shim is the helper:
`nativeterm-shim --proxy <url> %h %p` (`native_term_config::proxy`,
shim `proxy.rs`).

- **Where it is kept:** written into the host block (or the folder's
  options block) as a real `ProxyCommand "<shim>" --proxy
  socks5://gw:1080 %h %p`. No `NativeTerm*` key: every ssh run uses it
  as it is — tabs, the files window, background checks, tmux sends, key
  installs — and so do plain `ssh`, `scp` and VS Code Remote. The
  session options' "Connection" page shows it as a type (SOCKS5, SOCKS4,
  HTTP) and an address; saving writes this install's shim path, and a
  `ProxyCommand` of any other form is shown and kept as text. With a
  jump host set as well, the page warns: ssh uses whichever comes first.
- **The helper:** connects to the proxy (20 s), asks for `host:port` —
  SOCKS5 (RFC 1928, no authentication; names sent as names so the proxy
  resolves them), SOCKS4a (IPv4 addresses as is, IPv6 refused), or HTTP
  `CONNECT` (the answer read a byte at a time so the server's first
  bytes stay for ssh) — then relays stdin to the connection and the
  connection to stdout (flushed per read) until either side closes. A
  proxy that doesn't answer in 10 s is reported as the wrong type or
  port. Errors go to stderr in the user's language, which ssh shows in
  the tab (or a background run reports).
- **Logins:** the user name is part of the URL
  (`socks5://alice@gw:1080`, `DOMAIN\user` allowed), the password is in
  Credential Manager as `NativeTerm/proxy/<url>` — never in the config,
  never on a command line. The options page has the user name and a
  password field that saves (or removes) the password right away, with
  its state: none yet, saved, or refused. SOCKS5 offers "no login" and
  "user name / password" (RFC 1929) and sends the login only if the
  proxy picks it; SOCKS4 sends the user name as its user id (it has no
  password); HTTP sends `Proxy-Authorization: Basic` (the page notes that
  Basic is encoded, not encrypted). A proxy offering only other schemes
  (NTLM, Negotiate) is reported with their names. A refused password is
  marked in its entry (like a refused ssh password) and not sent again
  until a new one is saved: tabs reconnecting and background checks
  retrying it could lock a directory account. Buffers holding the
  password are zeroed after sending.
- **Windows logins (NTLM, Negotiate):** a proxy that answers a
  `CONNECT` with `Proxy-Authenticate: NTLM` or `Negotiate` wants a
  handshake rather than a password — two or three requests on **one**
  connection, each carrying a token base64 in `Proxy-Authorization`.
  Implemented (`shim/sspi.rs`) with SSPI (`AcquireCredentialsHandleW`,
  `InitializeSecurityContextW` against `HTTP/<proxy host>`): Windows
  makes the tokens, from the credentials the person is signed in with
  when nothing is configured, or from the proxy user name and password
  when there is one. Negotiate is preferred where both are offered. The
  refusal page a proxy sends with each 407 is read and discarded, or it
  would be taken for the next answer. A proxy that goes on refusing
  after the last token, or answers without one, has refused the person:
  that is a refused login, not a broken handshake. Where Windows has no
  credentials to offer (`SEC_E_NO_CREDENTIALS` — an account signed in
  with a Microsoft account or a PIN), the tab says so and points at the
  user name and password fields instead of printing a number. No crate:
  the `sspi` crate carries its own NTLM and Kerberos and a large
  dependency tree for what is a handful of calls into `secur32.dll`.
  Tested with real tokens against a scripted proxy in unit tests, and
  end to end with the shim binary against a local fake proxy: Basic,
  then NTLM type 1, then type 3, then the relay.
- **No crate:** the `socks` crate (sync, SOCKS4/5) last released in 2022
  and pulls in the old `winapi`; the three handshakes are a few dozen
  lines each, tested against in-memory proxies, and end to end against a
  local SOCKS5 proxy with the real shim binary.
- Verified live (logins): microsocks (SOCKS5 login) and tinyproxy
  (`BasicAuth`) in a throwaway container with a random password never
  printed or stored; the test instance used `NATIVETERM_CRED_PREFIX` so
  its entries were test ones. Before a password was saved, ssh said "no
  password is saved for the proxy …"; the password typed into the
  options page and saved made plain `ssh` and the files window work
  through both. A wrong password saved for SOCKS5: the first run said
  "the proxy refused the user name or password (marked …)", the second
  "refused before and isn't sent again", and the options page showed it
  in red. End-to-end test: a local SOCKS5 proxy with a login, the real
  shim and a test Credential Manager entry (accepted, refused and
  marked, not sent a third time).
- Verified live: a host whose name (`inner.lab`) only resolves inside
  the test container, reached through an `ssh -D` SOCKS5 proxy and
  through tinyproxy (HTTP): plain `ssh` ran commands (`SSH_CONNECTION`
  from 127.0.0.1 inside the container), a NativeTerm tab connected
  (its tmux session created on the server), the files window listed
  `/root` through the HTTP proxy, the options page showed "SOCKS5
  127.0.0.1:10800" and saving rewrote it with the backslash path, which
  ssh still ran. No proxy listening, and an HTTP proxy spoken to as
  SOCKS5, gave "couldn't connect to the proxy …" and "the proxy didn't
  answer: check the type and port" (after 10 s).

### Folder options

ssh has no folders, so NativeTerm marks a folder's hosts and matches the
mark (`folder_options.rs`):

- every host block in the folder file gets `Tag nativeterm-<file stem>`
  (hosts with a `Tag` of their own are left alone and reported);
- a `Match tagged nativeterm-<file stem>` block at the **end** of the file
  holds the options. ssh reads the file in order, so the block must come
  after the hosts, whose tag is set by then; a value on the host itself
  still wins because ssh keeps the first value it sees. Folder files are
  included before the user's own `Host *` defaults, so folder options
  win over those;
- creating, importing and moving hosts keep this true: the new block is
  tagged (a moved host drops its old folder's tag) and the options block
  is moved back to the end. Clearing every option removes the block and
  the tags;
- the dialog is the session options dialog plus user, port, jump host
  and key files. Saving is checked with `ssh -G` on one of the folder's
  hosts and rolled back when ssh refuses;
- `Tag` and `Match tagged` need OpenSSH 9.4+; older versions (Windows 10
  ships 8.1) reject them, so the menu item explains that instead.
  Verified with Windows OpenSSH 9.5: options reach the folder's hosts,
  not others; a host's own `User` wins; `-P <tag>` on the command line
  would replace the tag (NativeTerm never passes it).

Legend: ✅ supported, 🟡 partly, ❌ not possible, — not applicable.

| SecureCRT | NativeTerm | |
|---|---|---|
| **Connection**: Name | `NativeTermLabel` | ✅ |
| Protocol SSH2 / Telnet / Serial / … | SSH via OpenSSH; others via plink | ✅ |
| File transfer / "SFTP session" | Built-in files window over `ssh -s sftp` (browse, up/download, drag in, edit in place, any file name encoding) | ✅ |
| Local shell command: Pre-connect | `NativeTermPreConnect` (shim runs it before `ssh`); plink: `-preconnectcommand` | ✅ |
| Description | one line in `NativeTermNote`; multi-line in `state.db` | ✅ |
| **Logon Actions**: Automate logon (Expect/Send table) | Not in general: NativeTerm never reads output. Covered cases: passwords and keyboard-interactive via askpass (SSH); commands after login via the `LocalCommand` signal (SSH) or a delay (plink); a password for `su`/`sudo` after login as delayed, hidden injection | 🟡 |
| Send initial carriage return | Injected Enter after login | ✅ |
| Logon script (VBScript/Python) | No scripting API over terminal output | ❌ |
| Remote command | `-o RemoteCommand` | ✅ |
| Display logon prompts in terminal window | Default behavior | ✅ |
| **SSH2**: Hostname, Port, Username (IPv6 too) | `HostName`, `Port`, `User` | ✅ |
| Prompt for hostname | Quick connect | ✅ |
| Firewall (jump host, SOCKS/HTTP proxy) | `ProxyJump`; a `ProxyCommand` running the shim as the helper (see "Proxies") | ✅ |
| Credentials (shared sets) | `NativeTermCredential` | ✅ |
| Authentication methods and order | `PreferredAuthentications`; GSSAPI depends on the Windows OpenSSH build | ✅/🟡 |
| Key exchange list and order | `KexAlgorithms` | ✅ |
| Minimum group exchange prime size | No OpenSSH client option | ❌ |
| **SSH2 Advanced**: Cipher, MAC lists and order | `Ciphers`, `MACs` (legacy ones re-enabled with `+`) | ✅ |
| Compression on/off | `Compression` | ✅ |
| Compression level | Removed from OpenSSH | ❌ |
| OpenSSH agent forwarding | `ForwardAgent`, with the risk notice | ✅ |
| Force session channel to close on disconnect | OpenSSH default behavior | ✅ |
| **Host Key** | `HostKeyAlgorithms`, `StrictHostKeyChecking`, `known_hosts` | ✅ |
| **Port Forwarding** / Remote / X11 | `LocalForward`, `RemoteForward`, `DynamicForward`; `ForwardX11` needs a user-installed X server | ✅/🟡 |
| **Terminal / Emulation**: type, scrollback | `SetEnv TERM=`; profile `historySize` | ✅ |
| Modes | Handled by Windows Terminal | — |
| Emacs (Alt as Meta) | Windows Terminal sends Alt as an ESC prefix | ✅ |
| Mapped Keys (per session) | Terminal key bindings are global (fragments can't bind keys); NativeTerm command buttons instead; "Backspace sends ^H / ^?" per non-SSH session (ntplink's `-nt-backspace`) | 🟡 |
| Appearance / Window (font, colors, cursor, tab color) | Per-folder/host Terminal profile, `NativeTermColorScheme`, `NativeTermTabColor` | ✅ |
| Keyword Highlighting | Not in Windows Terminal | ❌ |
| Log File | SSH: not client-side (OpenSSH), but on the server for persistent sessions (`tmux-log`: `pipe-pane`, read / copied / deleted from "Sessions on the Server"); other protocols: ntplink's session log; Terminal's "Export text" by hand | 🟡 |
| Printing | Not in Windows Terminal | ❌ |
| X/Y/Zmodem | Not in Windows Terminal; optional `NativeTermTrzsz` | 🟡 |
| **File Transfer**: FTP/SFTP | SFTP built in (see "File transfer (SFTP)"); FTP not planned | ✅/❌ |
| **PuTTY-only pages** (plink sessions) | See "Other protocols via plink" | |

## Tab identity

Two different identities are involved:

- **Session identity: the Windows Terminal session GUID.** NativeTerm
  assigns it with `--sessionId`; the shim reads it from `WT_SESSION` and
  registers under it. Windows Terminal keeps it across a tab being moved
  to another window and in saved layouts (session restore, workspaces).
  Verified on 1.26: "Restart connection" re-runs the original command
  line but with a **new** GUID and the profile's title/suppression
  settings; duplicate tab and duplicate pane run the profile's command
  line with a new GUID. The shim also receives NativeTerm's own session
  id on its command line (kept by restart, lost by restore). So the key
  used depends on how the shim started:

  | How the shim started | Command line | `WT_SESSION` | Key |
  |---|---|---|---|
  | Opened by NativeTerm | `--session <id> <alias>` | the GUID NativeTerm assigned | both |
  | Restart connection | same as opened | new | `--session` id |
  | Restored layout / workspace | profile's (no args) | original | `WT_SESSION` |
  | Duplicate tab / pane | profile's (no args) | new | none → local shell |
- **UI element: found through the session label.** UIA doesn't expose the
  session GUID. NativeTerm gives every session a unique label (cloning or
  reopening a host appends a suffix, `web01 (2)`) and opens the tab with
  `--title <label> --suppressApplicationTitle`, so the label shows up in
  UIA. `RuntimeId` is only a tracking handle: it survives renames and
  splits, but changes when a tab moves windows or a recycled container is
  scrolled.

### Claiming tabs (verified with portable Terminal 1.26)

A tab element is claimed for a session by the first rule that matches:

1. **Tab name == a session label**: the session's pane has focus.
2. **The selected tab has a pane whose `TermControl` HelpText == a
   session label**. HelpText is the pane's live terminal title, and with
   `--suppressApplicationTitle` it stays the label. This holds even when
   the user split the tab and a foreign pane has focus (the tab name is
   then the foreign pane's title), and when the user renamed the tab.
   `TermControl` Name is the pane's *profile* name ("NativeTerm SSH"),
   which marks a pane as NativeTerm's even when its label is unknown.
   Only the selected tab has `TermControl` elements.
3. **Carried claim**: once claimed, a tab stays claimed through title
   changes. `RuntimeId` can't carry it, because containers recycled by
   scrolling get new ids (106 → 523 in the 64-tab test). Instead, each
   scan reads the window's **full** tab list with `ItemContainerPattern`
   (names of all tabs, including virtualized ones, 40 ms for 64 tabs)
   and aligns it with the previous scan's list. The alignment uses:
   1. order-preserving name matches (LCS);
   2. unique names that moved (drag);
   3. in-place changes when the length is unchanged (rename, split focus
      change).
   Realized tab elements, which have rectangles for the menu hook, are
   mapped onto that list in strip order. A claim ends when its entry
   disappears from the list (closed, or moved to another window). Rules
   1–2 then re-claim it wherever it shows up. Verified: 64 tabs, one
   split with a foreign pane focused, scrolled to the end and back. All
   64 stayed claimed, and the split one still opened NativeTerm's menu.

Sessions registered by a shim but not matched to an element are
**unlocated**, e.g. a split tab that isn't selected, with a foreign pane
focused, right after NativeTerm starts. NativeTerm knows how many exist
(registered shims minus claimed tabs) and shows them as such. They are
located as soon as their tab gets selected: the selection event triggers
a rescan, and rule 2 applies. NativeTerm never cycles through tabs on its
own to find them; a "Locate" command does that on request (implemented:
the sessions panel offers it while sessions are unlocated; it selects
each unclaimed tab, rescans after 250 ms, stops as soon as every session
is found, and selects the tabs that were selected before; it says how
many tabs it looked at, or how many sessions are still missing). A tab item
exposes nothing beyond its name and rectangle (all UIA properties were
dumped), so an unselected split tab can't be identified any other way.
Two things narrow it down:

- **Position hints:** the registry remembers, per window, the order of
  session tabs and their neighbours' names. An unknown-named tab at the
  remembered position between known neighbours is the likely candidate.
  It is shown as such, but not treated as claimed.
- **Right-click on an unclaimed tab while sessions are unlocated:** the
  hook swallows the click, and a worker selects that tab and rescans its
  panes. If the tab holds an unlocated session, NativeTerm's menu opens.
  Otherwise the previous selection is restored and the right-click is
  replayed to Terminal (it is the user's own tab). This is the only
  case where a right-click changes the selected tab, and only
  momentarily.

The prototype (`prototypes/menu-hook`) was restarted while one tab was
split with a foreign pane focused. On its first scan it claimed that tab
by rule 2 and marked it mixed. The other tabs were claimed by rule 1.

### Restored tabs and named windows

Windows Terminal saves layouts in two cases:

- **Session restore** (`firstWindowPreference`).
- **Workspaces (1.25+):** closing a *named* window saves it under its
  name in `state.json` `persistedWorkspaces`.

Verified on 1.26, what a saved pane keeps and loses:

- **Kept:** the **profile** GUID, the **session GUID**, and the starting
  directory.
- **Lost:** the command-line override, `--title`, and
  `--suppressApplicationTitle`. A pane opened as `new-tab --profile X
  --title L --suppressApplicationTitle shim web01` is saved as profile
  X's own command line, with X's tab title and suppression off.
- **A tab opened without a matching profile** is saved as a synthesized
  "Default" profile running `cmd.exe`.

Consequences:

- **NativeTerm always opens tabs with `--profile "NativeTerm SSH"`, and
  that profile's command line is the shim with no host.** A restored
  pane then runs the shim with its original `WT_SESSION` (verified: the
  profile's command line ran, with the original GUIDs).
- **A shim started without a host asks NativeTerm** over the pipe, keyed
  by `WT_SESSION`:
  - *Known session:* the restored pane can't carry the label.
    - **Why:** the saved layout dropped `--title`, and the profile
      suppresses title changes, so the shim can't set it either. (Setting
      it with `SetConsoleTitleW` worked only with a non-suppressing
      profile, before suppression moved into the profile.) Every
      restored NativeTerm tab is therefore titled with the profile's name.
    - **So NativeTerm replaces restored placeholders.** For each
      registry hit it opens a proper tab: same label, a new session
      GUID, and in the "restored — reconnect?" state, since restored
      sessions never connect on their own. Then it tells the placeholder
      shim to exit with 0, which closes it. `wt` has no move-tab command,
      so replacements are appended; replacing a window's placeholders in
      their original order keeps them together.
    - **Until replaced,** placeholders count as unlocated.
  - *Restart connection* (shim crash only): the command line with
    `--session` comes back and suppression stays on (profile), so the tab
    keeps working in place under its new `WT_SESSION`.
  - *Unknown GUID:* e.g. settings copied from another machine, or the
    registry lost. The shim offers a local shell or closing the tab.
  - *No NativeTerm running:* the shim starts it, or explains, and never
    connects by itself.
- **How it is implemented** (verified with portable 1.26,
  `tests/restore_portable.rs`):
  - **Placeholders:**
    - A shim without a host waits 1 s for NativeTerm.
    - If NativeTerm isn't there, the shim starts `nativeterm.exe
      --from-shim` from its own folder and waits up to 15 s. That copy
      exits quietly if another NativeTerm already serves the pipe.
    - NativeTerm answers `hold` for a session it knows. The shim then
      shows "Reopening this restored session in a new tab…" and waits up
      to two minutes.
  - **Replacement:**
    - NativeTerm gathers placeholders for 1.5 s.
    - Per Terminal window, it activates the window: the shim reports its
      console's owner window, and `-w 0` means the most recently activated
      window.
    - It then opens the replacement tabs with `--wait`: same session id
      and label, a new GUID.
    - Once they show up, it tells the placeholders to close. If a tab
      doesn't show up, the placeholder becomes a local shell.
    - A `--wait` shim reports `waiting` and connects only on `connect` or
      the R key.
  - **Which sessions can be restored:**
    - *Open in `state.db`:* NativeTerm wasn't running when the window
      closed.
    - *Closed with their window:* the shim reported closing, and the
      window was gone within 10 s. Terminal keeps its last window for
      more than 1.5 s while saving the layout, so the first 1.5 s check
      missed it.
    - These stay replaceable for 7 days. Closing the last tab also closes
      the window and records such a session, but Terminal saves no
      layout for it, so the entry never matches and simply expires.
  - **NativeTerm restarts:**
    - Sessions open in `state.db` start as "looking for its tab".
    - Their shims reconnect within 2 s and replay the current attempt,
      then its login, then its outcome, in that order. The first version
      replayed the outcome before the login, which would have shown a
      disconnected session as connected. The shim also never reported
      the login itself, so a restarted NativeTerm couldn't know it.
    - Sessions whose shim doesn't show up within 12 s are marked gone.
    - **Tabs that can't be there any more** aren't looked for at all:
      `state.db` keeps each session's shim (process id and start time,
      schema 4). If that process no longer runs at the next start — the
      tab was closed while NativeTerm wasn't running, Windows shut down
      or signed out, Terminal crashed — the session is recorded as closed
      with its window at once: no 12 s wait, no "lost" notice, and still
      restorable if Terminal restores the pane (matched by its terminal
      GUID only). A reused process id has another start time. Sessions
      from before schema 4 (no shim recorded) are looked for as before.
      Verified in the portable Terminal (`tab_closed_while_nativeterm_was_not_running`):
      one of two tabs closed between two NativeTerm runs; the next run
      found the other at once and didn't list the closed one. With the
      check disabled the same test failed after the 12 s wait with the
      closed one shown as gone.
    - While NativeTerm runs, closing is followed as it happens: a tab's
      shim reports closing (`Closed`; with its window: restorable), and a
      shim that vanishes without a word (killed, Terminal crashed) drops
      its pipe (`Gone`); both are written to `state.db` right away.
  - **NativeTerm exits:** Settings → "Close NativeTerm's tabs when
    NativeTerm exits" (`close_tabs_on_exit` in `state.db`, **on unless
    turned off**: a tab left behind has nobody to answer its tab menu).
    When the main window closes, each open, unlocked session's shim is
    sent `Close` over its own pipe and exits with 0, which closes its tab,
    and every such session (also one whose tab was never found) is
    recorded as closed in `state.db` at that moment — the shims' own
    reports arrive after NativeTerm is gone, so without this the next
    start looked for the closed tabs (found by the user). Turned off, the
    tabs stay and the next start takes them over. Nothing
    else is touched — no window messages, no Terminal UI: tabs NativeTerm
    doesn't manage (the user's shells, other programs' tabs) stay, and so
    does their window; a window that held only NativeTerm's tabs closes
    by Terminal's own rule when its last tab does. Locked sessions stay
    open. A crash or a killed process skips this (the tabs stay, as
    without the setting). Verified in the portable Terminal: a window with
    a foreign `cmd` tab and two sessions, one locked — only the unlocked
    tab closed, the window stayed (`close_all_spares_locked_and_foreign_tabs`);
    and with the real app closed by `WM_CLOSE`, its session's window
    closed while unrelated shims and windows were untouched.
  - **A race found on the way:**
    - NativeTerm answers "local shell" and hangs up within about 40 ms.
    - The shim polled its "connected" flag every 50 ms and missed the
      whole connection, then waited 15 s for a NativeTerm it had already
      reached.
    - The link now remembers that it reached NativeTerm, and the answer
      waits in its inbox.
  - **Diagnostics:** if `%TEMP%\nativeterm-shim-debug` exists, each shim
    logs to `%TEMP%\nativeterm-shim-<pid>.log`; `NATIVETERM_DEBUG` shows
    NativeTerm's own diagnostics as notices.
- **A hung Terminal blocks UIA without limit** (verified by suspending the
  portable process). A root `FindAll`, and even `ElementFromHandle` with
  `IUIAutomation2` connection/transaction timeouts of 500 ms, did not
  return within 30–60 s. `IsHungAppWindow` stayed false the whole time.
  `SendMessageTimeout(WM_NULL, SMTO_ABORTIFHUNG, 250 ms)` detected it
  immediately. So NativeTerm:
  - enumerates Terminal windows with `EnumWindows`, never a UIA root
    search;
  - probes each window with `WM_NULL` and uses `ElementFromHandle` only
    for responsive ones (a scan with the hung window skipped: 354 ms);
  - runs all UIA work on worker threads with a watchdog. A window can
    hang between probe and call; a stuck worker is abandoned and
    replaced, never waited on, and the UI never blocks.
- **Named windows swallow commands.** When `wt -w <name> …` targets a
  name that isn't open but has a saved workspace, Terminal restores the
  workspace and **drops the rest of the command line** (verified: the
  requested tab never appeared, and the workspace entry was consumed).
  Sending the same command again, once the window exists, works.
  NativeTerm's dedicated window has a name. So before opening a tab
  there while it isn't running, NativeTerm reads `state.json` (read-only)
  for a workspace under that name. If there is one, NativeTerm launches
  the window first, waits for it to appear in UIA, and then sends the
  tab. Independently, every opened tab is confirmed through UIA and the
  request is resent once if it's missing. Session GUIDs are never reused
  for new tabs, so a restored pane and a new tab can't collide. The
  user's own window, targeted with `-w 0`, is unnamed, so this doesn't
  apply unless the user named it.
- **The shim executable is locked while tabs run it:** an update can't
  overwrite it (seen when rebuilding the prototype). See the
  rename-then-replace rule under updates.

- **Tabs moved to another window**: the terminal control and its
  connection move in-process, so the shim and `ssh` keep running and the
  session GUID stays the same, but Windows Terminal creates a new tab
  object — a new UIA element with a new `RuntimeId` (verified when the
  user merged two windows) — which is re-claimed by rules 1–2. UIA
  lookups cover all Windows Terminal windows of the chosen install.
- **The user's own tabs** (local shells, AI coding tools, anything
  NativeTerm didn't open): listed in the tab switcher with their live
  titles, and never touched by any NativeTerm operation (close others /
  close to the right / close disconnected / send commands). Their titles
  change freely — AI tools put live status in them — and NativeTerm does
  nothing to suppress that.
- **Whole window closed by the user**: every tab's shim and `ssh` end with
  it. NativeTerm notices and marks those sessions closed; persistent
  (tmux) sessions can simply be reopened.

### Windows Terminal's own tab menu on a NativeTerm tab

- **Duplicate tab / split pane (duplicate)**: Windows Terminal duplicates
  the *profile*, not the launch command, so the copy runs the profile's
  own command line — not our `nativeterm-shim <host-alias>`. The
  "NativeTerm SSH" profile's command line is `nativeterm-shim` with no
  host, which starts a local shell (configurable; `%COMSPEC%` by default).
  The copy is therefore just a local tab, i.e. one of the user's own.
- **Restart connection**: re-runs the original command (with
  `--session <id>`), with a new `WT_SESSION` and the profile's settings,
  so title suppression stays on. It only appears when the tab's first
  process exited with a non-zero code. NativeTerm's shim doesn't do
  that: after `ssh` ends, it stays and offers reconnect itself, so this
  path means the shim crashed. NativeTerm re-associates the tab by
  `--session`.
- **Rename / change color**: the user's choice; see the title fallback
  above.
- **Move tab**: see "Tabs moved to another window".
- **Close → close others / close tabs after**: Windows Terminal's versions
  close every tab, including the user's own; NativeTerm's versions only
  touch its own tabs. Both coexist; tabs closed by Windows Terminal end
  their shims, and NativeTerm marks those sessions closed. A batch close
  that would close a tab holding other panes (a split with the user's own
  shell) asks first: the menu has no dialogs of its own, so it asks the
  main window, which lists the sessions and marks the split ones.
- **Rename** (tab menu): asks the main window to open the host dialog.
- **Export text / Find**: Windows Terminal's own features, used as-is.

### Windows Terminal's own SSH profiles (1.25+)

Windows Terminal generates a profile for each host in the SSH config
(`SshHostGenerator.cpp`, source `Windows.Terminal.SSH`).
`Feature_DynamicSSHProfiles` has been `AlwaysEnabled` since v1.25
(2025-12). The Store build here (1.24) doesn't do it yet; the portable
1.26 does. It generated 8 profiles from this machine's `~/.ssh/config`.

How it works:

- **Files read:** only `%UserProfile%\.ssh\config` and
  `%ProgramData%\ssh\ssh_config`. `Include` is **not** followed.
- **Which hosts become profiles:** a `Host` line followed by a
  `HostName` line. A host that sets no `HostName` is skipped. The whole
  value of the `Host` line is used as the host name, and `Match` blocks
  are ignored.
- **The profile:** named `SSH - <host>`, with command line
  `"<ssh.exe>" <host>` (System32 OpenSSH or an MSI install) and a GUID
  derived from the name.
- **New-tab menu:** once, it adds an "SSH" folder to the new-tab menu
  (`sshFolderGenerated` in `state.json`), inlined automatically.
- **Turning it off:** `"disabledProfileSources": ["Windows.Terminal.SSH"]`
  in `settings.json`, or the `DisabledProfileSources` policy
  (`HKLM`/`HKCU\Software\Policies\Microsoft\Windows Terminal`).
  Fragments can't set this.

Findings:

- **Multi-name `Host` lines produce broken profiles.** `Host node01
  incus-node-01` became "SSH - node01 incus-node-01" with command line
  `ssh.exe node01 incus-node-01`. ssh reads the second name as a remote
  command, so the tab runs `incus-node-01` on `node01` and exits. This
  machine's config has three such hosts. The wildcard block
  `Host node01 node02 node03 incus-node-*` has no `HostName`, so it was
  skipped. A wildcard `Host` *with* `HostName` would become a profile
  too. This looks worth an upstream bug report.
- **NativeTerm's layout mostly hides hosts from it.** Folders live in
  `~/.ssh/config.d/*.conf` via `Include`, which the generator doesn't
  follow, so NativeTerm-managed hosts don't flood Terminal's menu. With
  ~800 hosts, a flat new-tab list, command palette, and profile list of
  that size would be a real cost. Hosts the user keeps in the main
  `config` still appear.
- **Tabs opened from these profiles are the user's own tabs.** They run
  plain `ssh` with no shim, so NativeTerm can't reconnect, inject, or
  track login for them. They are listed in the tab switcher like any
  other own tab. NativeTerm may show a hint ("open this host through
  NativeTerm for session features") but doesn't adopt them.
- **Positioning:** Terminal's generator is a flat quick-connect list: no
  folders, search, batch operations, command sending, persistent
  sessions, or import. NativeTerm complements it and doesn't compete
  with it.
- **NativeTerm's own parser must not repeat the bug:** a `Host` line is
  a list of patterns. Only literal names (no `*`, `?`, `!`) are
  connectable aliases; NativeTerm connects with the first one, and
  `ssh -G <alias>` resolves the effective settings. Blocks that only add
  shared settings are not sessions (see "Parsing and writing rules").
- **Settings:** NativeTerm offers "Hide Windows Terminal's SSH profiles"
  in its settings. It adds `Windows.Terminal.SSH` to
  `disabledProfileSources` as an explicit, backed-up edit of Terminal's
  `settings.json`, same as the stub cleanup, and never changes it
  silently. The default is to leave Terminal's list alone.
  Implemented (`windows_terminal::sources`): a checkbox under Settings.
  The edit is textual, because the file has comments: a small tokenizer
  finds the top-level `disabledProfileSources` array and inserts or
  removes only that entry (a list that becomes empty is removed with its
  line, so on → off restores the file byte for byte). The result must
  parse and show the requested state before anything is written; the
  previous file is copied to `<data>\backups\terminal-settings-*.json`
  and replaced through a temporary file.

## Other protocols via plink

NativeTerm parses no protocol: the shim runs a console client in the tab.
For SSH that client is OpenSSH. For Telnet, serial, raw TCP, rlogin, and
SUPDUP it is **`ntplink.exe`**, NativeTerm's own frontend over PuTTY's
unmodified protocol code (see "ntplink" below), or PuTTY's **`plink.exe`**
when ntplink isn't there:

- **License and trust:** MIT license, and official builds are signed by
  Simon Tatham.
- **Where it comes from:** NativeTerm's package ships `ntplink.exe` in
  `tools\` (decided: added when the package is built, see
  "Distribution"). plink is only a fallback for a copy without it: an
  installed PuTTY (this machine: `C:\Program Files\PuTTY\plink.exe`,
  0.84) or a `plink.exe` placed next to NativeTerm.
- **Why plink and not `putty.exe`:** `putty.exe` is a GUI terminal with
  its own window, so it can't live in a Terminal tab.

Everything protocol-independent works the same as for SSH: tabs, claiming,
the menu, closing, batch open, and input injection.

### Storage

`~/.ssh/config` only describes SSH hosts. Writing a Telnet host there
would make a plain `ssh <alias>` try SSH on port 23. So non-SSH sessions
live in a sibling file per folder, `~/.ssh/config.d/<folder>.nt.toml`.
Its extension isn't matched by `Include config.d/*.conf`, so ssh ignores
it, and it syncs, renames, and backs up together with the folder. The
sidebar shows both kinds in one tree. Each entry has a stable id.

Implemented (`native_term_config::plink`):

- **File:** `<folder>.nt.toml` beside `<folder>.conf` (the main `config`
  → `config.nt.toml`), a list of `[[session]]` tables: `name` (the alias;
  one namespace with ssh's, duplicates are reported), `label`,
  `protocol` (`telnet`, `rlogin`, `raw`, `serial`, `supdup`), `host`,
  `port`, `user`, `charset`, `serial = { line, speed, data_bits, parity,
  stop_bits, flow }` (PuTTY's defaults: 9600 8N1, XON/XOFF), `favorite`,
  `note`, `on_login`, `source`, and `[session.putty]` for options plink
  only takes from a saved session (`REG_DWORD` numbers, `REG_SZ` text).
  A file that doesn't parse is reported; the folder's ssh hosts still
  load.
- **Tree:** each session is a `HostEntry` with `plink` set (after the
  folder's ssh hosts), so search, favorites, recent hosts, multi-select
  and the queue treat both kinds alike.
- **Editing:** the editor adds, updates, favorites, moves and deletes
  them in the `.nt.toml` through the safe writer (backup, conflict
  check); every entry is validated (one-word name, host or serial line,
  a port for raw, a known charset, nothing that could pass as an
  option). ssh-only operations refuse them. "Move session folders"
  copies the `.nt.toml` files too.
- **Shim:** before running ssh, the shim looks the alias up in the
  session folders (`--ssh-dir <dir>` when NativeTerm runs on another
  folder than `~/.ssh`: the Terminal adapter adds it to every session
  tab). A non-SSH session runs ntplink instead (`NATIVETERM_NTPLINK`,
  or `ntplink.exe` next to the shim or in its `tools` folder), else plink
  (`NATIVETERM_PLINK`, next to the shim or in `tools`, on `PATH`, or in
  PuTTY's installation folder); it is read again at every reconnect, so
  edits apply. What follows describes plink; ntplink needs none of the
  workarounds (see "ntplink").
  - The console code pages are set to the charset while plink runs.
  - A serial line is opened exclusively first: "in use" (access denied)
    or "can't be opened" is reported before plink starts.
  - "Connected" is reported through the ssh login event: when one of
    plink's TCP connections is `ESTABLISHED`, or a serial plink has run
    for 1 s. Post-login commands and the card work as for ssh.
  - Exit codes 0 (a Telnet server closing) and 1 (plink's own errors),
    and a raw connection the watcher found in `CLOSE_WAIT` (it ends
    plink), are reported as connection-level (255): "disconnected" after
    a connection, "could not connect" before one.
  - PuTTY-only options: the temporary saved session
    `NativeTerm-<shim pid>-<attempt>` is written just before plink starts
    and deleted 2 s later or when plink ends, whichever is first (the
    shim may exit right after). One left by a shim that died is removed
    by the next shim (its pid no longer runs); other saved sessions are
    never touched, and an existing name is an error.
  - **Serial silence:** a serial line has no connection to lose, so the
    shim fingerprints what the console shows around the cursor (cursor
    position plus its line and the one above, read with
    `ReadConsoleOutputCharacterW`) every 300 ms. Unchanged for 30 s, it
    sends `Quiet { since }`; the next change sends `Heard`. The card
    shows "no data since HH:MM" (local time) until then; a reconnect or
    an exit clears it, and the link replays it to a restarted NativeTerm.
    Echoed typing counts as data. Verified with a virtual COM pair: data
    shown, "no data since 12:54" after 30 s of silence, gone at the next
    line; a second open of the same port refused by the app with the
    owning tab named, and a shim started outside NativeTerm on the busy
    port reporting "in use".
  - **App:** the tree shows non-SSH sessions with their own icons
    (network, serial) and protocol · target · charset on hover; their
    menu has no ssh-only items (session options, install key, forget
    host key), and "Install my key" on a folder or selection and the
    agent-forwarding notice skip them. A folder's menu has "New Telnet /
    Serial Session…": name, protocol, host / port / login name, or for
    serial the port (ports present now, from
    `HKLM\HARDWARE\DEVICEMAP\SERIALCOMM`, or typed), speed (common rates
    or typed), data bits, parity, stop bits and flow control; charset
    (common ones or any known name / code page); note and a command typed
    once connected. The alias is made from the name like an ssh host's
    (pinyin for Chinese). Editing keeps what the dialog doesn't show (id,
    favorite, source, PuTTY options it doesn't list). "PuTTY options"
    (collapsed) shows the pages of the chosen protocol: connection
    (terminal type, keepalive seconds, `TCP_NODELAY`, TCP keepalives),
    Telnet (passive negotiation, keyboard sends Telnet special commands,
    RFC 1408 environment) and SUPDUP (location, extended ASCII, MORE
    processing, scrolling). A value equal to PuTTY's default isn't
    stored, so an untouched session needs no temporary saved session;
    another protocol's page is dropped when the protocol changes; the
    importer keeps options the same way. Switches carry their text, so
    screen readers name them. Opening a serial session whose port
    an open session already uses is refused with a notice naming that
    session's tab. Verified in the app (`--ssh-dir` on a test folder):
    created from the folder menu, connected ("connected", then
    "disconnected (255)" when the test server closed), edited to GBK and
    reconnected from the card — the same server's GBK text then showed
    correctly.
  - Verified with the real plink in the portable Terminal against local
    test servers: Telnet connected, a GBK banner shown correctly, the
    server's close shown as "disconnected"; raw closed by the server
    detected with no keystroke; a refused port as "could not connect".
    The temporary session, its deletion and stale-session cleanup are
    tested with a fake plink and a test registry key (the real PuTTY key
    is never written by tests).

Import: the user's existing **PuTTY saved sessions** (e.g. switches and
serial consoles) are read, read-only, from
`HKCU\Software\SimonTatham\PuTTY\Sessions`. They can be imported like
SecureCRT sessions, or referenced by name (`plink -load <name>`).
Non-SSH sessions are imported too (both readers): Telnet, serial, raw,
rlogin and SUPDUP become `.nt.toml` sessions of the planned folder
(written before the folder's ssh hosts, and taken out again if those
fail, so a retry starts clean; a folder with only such sessions still
gets its `.conf`). Serial line settings: SecureCRT's `Baud Rate`, `Data
Bits`, `Parity` and `Stop Bits` (Windows `DCB` numbering) and flow from
`CTS Flow` / `DSR Flow` / `XON Flow` (`DTR Flow Control` / `RTS Flow
Control` are line states, not flow control); PuTTY's `SerialSpeed`,
`SerialDataBits`, `SerialParity`, `SerialStopHalfbits` and
`SerialFlowControl`. The charset comes from SecureCRT's "Output
Transformer Name" or PuTTY's `LineCodePage` (`GBK`, `CP936`,
`ISO-8859-1:1998 (Latin-1, …)`, …); one not recognized is listed as
before. PuTTY options plink only reads from a saved session (terminal
type, keepalive, TCP options, Telnet and SUPDUP pages) are kept in
`[session.putty]`. Telnet over TLS isn't something plink does and is
listed as not imported. Verified with a SecureCRT fixture, a PuTTY test
registry key, and in the app (PuTTY import of a Telnet and a serial
session next to ssh hosts).

Implemented for SSH sessions (`native_term_config::putty`): each session
becomes the same session record the SecureCRT reader produces, so one
planner and writer serve both; `NativeTermSource putty:<name>` marks
imported hosts. A PuTTY SSH proxy (method 6) becomes `ProxyJump` — to
the imported host when it names a saved session, else the host written
out. `.ppk` keys aren't usable by OpenSSH and are reported with the
PuTTYgen conversion step. PuTTY's host keys (`SshHostKeys`, values
`<type>@<port>:<host>`) store the key's numbers, not a blob: RSA as
`0x<e>,0x<n>`, ECDSA as `<curve>,0x<x>,0x<y>`, Ed25519 as the point's
`0x<x>,0x<y>`. NativeTerm rebuilds the OpenSSH blob (for Ed25519: `y`
little-endian with the sign of `x` in the top bit, RFC 8032) and adds
them through the same `known_hosts` writer as SecureCRT's; DSA and Ed448
are reported as unusable. Verified with RFC 8032's test key and with
RSA / ECDSA keys from `ssh-keygen`.

### PuTTY options and how they reach plink

**On the command line (plink 0.84):**
- protocol: `-telnet -rlogin -raw -serial -ssh -ssh-connection`;
- `-P` port, `-l` user (auto-login username, also used by telnet and
  rlogin);
- `-4`/`-6`;
- `-proxycmd` (local proxy command);
- `-preconnectcommand` ("Command to run before connection");
- `-sercfg speed,data,parity,stop,flow`: the whole Serial page (e.g.
  `115200,8,n,1,N`; flow `X` XON/XOFF, `R` RTS/CTS, `D` DSR/DTR, `N`
  none), with `-serial COMx` as the line.

**Only through a saved session (`-load`):**
- keepalive interval, `TCP_NODELAY`, `SO_KEEPALIVE`, logical host name;
- terminal-type string, terminal speeds, environment variables;
- proxy type, host, and credentials (SOCKS4/5, HTTP, Telnet);
- **Telnet page:**
  - OLD_ENVIRON handling (BSD / RFC 1408), `RFCEnviron`;
  - negotiation mode (passive / active), `PassiveTelnet`;
  - "Keyboard sends Telnet special commands", `TelnetKey`;
  - "Return key sends Telnet New Line instead of ^M", `TelnetRet`.
    This one has **no effect under plink** (verified: Enter still went
    out as CR NUL), because it belongs to `putty.exe`'s keyboard
    handling;
- **SUPDUP page:**
  - location string, `SUPDUPLocation`;
  - extended ASCII character set (None / ITS / WAITS), `SUPDUPCharset`;
  - **MORE** processing, `SUPDUPMoreProcessing`;
  - terminal scrolling, `SUPDUPScrolling`;
- session logging: **not available under plink** (verified: neither
  `LogType`/`LogFileName` in a loaded session nor `-sessionlog` on the
  command line produced a log; only `putty.exe` writes session logs).
  ntplink writes one (see "ntplink: NativeTerm's own client");
- SUPDUP.

For these, the shim writes a **temporary** saved session
`NativeTerm-<shim pid>-<attempt>` under the PuTTY key just before starting
plink (for every connection, see "What PuTTY's source says": defaults)
and deletes it once plink has read it. Verified: a session with
`PassiveTelnet=1` took effect (plink sent no option negotiation until the
server did), and deleting the key 2 s after start didn't affect the
running plink. A pre-existing key with that name is an error, never
overwritten. The user's own PuTTY sessions
are never modified. This is the only registry write in portable mode;
it is stated in the UI.

**Not applicable:** PuTTY's Terminal, Window, Appearance, Behaviour,
Selection, and Colours pages belong to `putty.exe`'s own terminal.
Windows Terminal does that job. Keyboard remapping (e.g. Backspace as
`^H` for some devices) isn't done by plink; it needs a per-session
setting in NativeTerm, sent as the right byte by the shim's input path,
which is a later item.

### Character sets (verified)

plink converts keyboard input with the console **input** code page, and
remote output is decoded with the console **output** code page. The
shim is in the same console, so it sets both (`SetConsoleCP`,
`SetConsoleOutputCP`) before starting plink:

- **Default:** UTF-8 (65001).
- **Per session:** e.g. GBK (936) for legacy network devices.
- **Every session type.** Nothing in the path converts: `ssh` passes
  bytes through, PuTTY's plink converts nothing either (see below), and
  Windows Terminal draws what the console's code page says. So the shim
  sets the code pages around the session and puts them back afterwards:
  from the session's `charset` for Telnet, serial and the rest, and from
  `NativeTermCharset` (per host, or a folder default) for SSH hosts.
  That is why NativeTerm needs no second SSH client for a GBK host.

Measured against a local test server:

| Console code page | Typed "你好" arrives as | Server's GBK bytes show as | Server's UTF-8 bytes show as |
|---|---|---|---|
| 936 (Chinese Windows default) | `c4 e3 ba c3` (GBK) | 中文测试 ✔ | mojibake |
| 65001 | `e4 bd a0 e5 a5 bd` (UTF-8) | mojibake | 中文测试 ✔ |

### What PuTTY's source says (0.85, checked against our use)

- **Data path:** `windows/plink.c` hands the session's bytes between the
  network and stdin/stdout unchanged; it converts nothing. Setting the
  console code pages (input and output) is therefore the right way to
  give a session a charset. `-legacy-charset-handling` and
  `-legacy-stdio-prompts` only concern plink's own prompts.
- **Ctrl+C:** plink puts the console in "processed input" and installs no
  handler, so a real Ctrl+C key press was a signal that ended plink
  (reproduced in the portable Terminal: plink gone, nothing sent) instead
  of ^C for the device. Now plink runs in its own process group (the
  signal is ignored) and the shim takes "processed input" off while plink
  reads key by key; with local line editing it stays (Backspace needs
  it). Verified with a real key press: plink kept running and the server
  received 0x03. (A key record written with `WriteConsoleInput` doesn't
  go through ConPTY's signal handling, so it can't reproduce this.)
- **Raw half-close:** `plink_eof` returns false on purpose ("do not
  respond to incoming EOF with outgoing"), so a raw connection closed by
  the server stays half open until stdin ends or the next write fails.
  There is no option for it; the `CLOSE_WAIT` watch stays.
- **Exit codes:** Telnet and raw return 0 for a normal close and
  `INT_MAX` after a socket error; plink returns 1 when the connection
  can't be opened or fails fatally; serial always `INT_MAX`. All of them
  now mean "connection ended" (255); `INT_MAX` was missing, so a network
  drop showed as a normal end without automatic reconnect.
- **Line discipline:** plink sends stdin straight to the backend, past
  PuTTY's `ldisc.c`, so "Keyboard sends Telnet special commands"
  (`TelnetKey`) and "Return sends Telnet New Line" (`TelnetRet`) have no
  effect (no longer offered). Local echo and local line editing
  (`LocalEcho`, `LocalEdit`: 0 on, 1 off, 2 automatic) do: plink applies
  them to the console mode; offered as "Echo and line editing". Raw
  defaults to local echo and editing, serial to neither.
- **Keepalive:** saved as `PingInterval` (minutes) plus
  `PingIntervalSecs` (the rest): 90 s is 1 and 30. Imports used only the
  seconds; both are added now, for SSH (`ServerAliveInterval`) too.
- **Defaults:** without `-load`, plink starts from PuTTY's own "Default
  Settings" in the registry (`do_defaults(NULL)`); with `-load`, missing
  values are PuTTY's built-in defaults. So a session without PuTTY
  options would inherit the user's PuTTY defaults (proxy, keepalive,
  terminal type, echo) and one with options wouldn't. Decided (by the
  user): plink always loads NativeTerm's temporary session, so a session
  behaves as its `.nt.toml` says whatever the user's PuTTY defaults are.
  It also carries the tab's size at connect time (`TermWidth`,
  `TermHeight`), which Telnet reports as the window size.
- **Window size:** plink never calls `backend_size`, so Telnet's NAWS
  reports `TermWidth` × `TermHeight` and never a change: the size the tab
  had when connecting (see above), not a later resize.
- **Not reachable through plink:** serial Break (`SS_BRK` exists in the
  serial backend, plink has no way to send specials), session logging of
  the output (`-sessionlog` exists, but session output is logged by
  PuTTY's terminal, which plink doesn't have).

### ntplink: NativeTerm's own client

Everything plink can't do (above) is in its frontend, not in PuTTY's
protocol code. So NativeTerm builds its own frontend, `ntplink.exe`, over
PuTTY 0.85's **unmodified** backends — the SSH-free set PuTTYtel uses
(`be_list(ntplink NTPlink SERIAL OTHERBACKENDS)`): Telnet, raw, rlogin,
SUPDUP, serial. A Rust rewrite was considered and rejected: years of device
quirks live in `telnet.c` and `serial` handling.

The source is a PuTTY fork, `github.com/fivetime/putty` (private for now):
upstream PuTTY plus the fork's additions, all in its `patches/` folder,
one `add_subdirectory(patches)` line, its `CHANGELOG.md` and its CI. The
CI merges upstream `main` automatically (no conflicts: upstream files
aren't edited) and publishes cross-platform binaries as a GitHub Release
tagged `0.85-YYYY-MM-DD.<commit>`; the Windows zips (x86_64, aarch64,
i686, llvm-mingw, UCRT) contain `ntplink.exe`. Getting it next to the
shim:

- `tools\get-ntplink.ps1 [-Tag …] [-Arch …]`: downloads the latest (or a
  given) release through `gh` (signed in, since the fork is private),
  checks it against the release's `SHA256SUMS`, and copies `ntplink.exe`
  into `target\debug` and `target\release`.
- `tools\build-ntplink.cmd <checkout>` (or `PUTTY_SRC`): builds it locally
  with CMake and Visual Studio's C++ tools (static C runtime, ~420 KB,
  imports only KERNEL32 and ADVAPI32) and copies it the same way.

Both were verified: the CI build (release `3edb436`) passed the same
end-to-end and raw tests in the portable Terminal as the local MSVC
build, and imports no `Reg*` functions either.

| | plink | ntplink |
|---|---|---|
| Settings | registry: "Default Settings", or NativeTerm's temporary `-load` session | PuTTY's built-in defaults plus `-set Keyword=Value` (any saved-session keyword); no registry code linked (`stubs/no-storage.c`) |
| Window size | fixed at connect | sent at connect and on every resize (NAWS, rlogin, SUPDUP) |
| Ctrl+C | ended plink (shim workaround: own process group, console mode polled) | a key: ^C for the device |
| LocalEcho / LocalEdit | only once Telnet negotiates | from the start, every protocol |
| Raw: server closes | half open (shim workaround: `CLOSE_WAIT` watch) | exits |
| Break, Telnet commands | impossible | control pipe |
| Session log | never written (PuTTY logs what its terminal shows) | `LogType` 1 (text) / 2 (every byte), `LogFileName`, appended |
| Exit codes | 0 / 1 / `INT_MAX` | 0 closed by the far end, 2 couldn't connect, 3 lost, 1 usage |

The shim passes `[session.putty]` as `-set` (no registry write at all) and
creates `\\.\pipe\nativeterm-control-<shim pid>-<attempt>` (one instance,
local clients only, served only to the ntplink process it started) before
starting ntplink with `-nt-control <pipe>`. It then tells NativeTerm which
commands the connection takes (`ShimMessage::Specials`: `brk` for serial;
Break, AYT, IP, AO, EC, EL, GA, NOP, Abort, Susp, EOR, EOF, Synch for
Telnet). The session card gets a **Break** button and the tab menu
**Send Break** for those sessions only, enabled while connected;
`AppMessage::Special { name }` becomes `special <name>` on the pipe. If
the pipe can't be made, the tab says so and the session runs without it.

**Session log.** The dialog's "Session log" section (every protocol) sets
`LogType` and `LogFileName` in `[session.putty]`; they reach ntplink as
`-set` like the other options. ntplink logs what it writes to the console
(and what it echoes locally): type 2 every byte, type 1 the text without
escape sequences (CSI, OSC and the other strings, two-byte ones) and
control characters other than CR, LF and Tab; bytes from 0x80 up are
kept, in the session's charset. The file name takes PuTTY's codes (`&H`
host or serial line, `&Y&M&D`, `&T`, `&P`); turning the log on fills in
`<data dir>\logs\&H-&Y&M&D.log` (a file per host and day). An existing
file is appended to: ntplink never asks, because PuTTY's question would
read the session's keys. PuTTY doesn't create folders, so the shim
creates the file's folder first (when it has no `&` codes). A log turned
on without a file isn't kept. plink can't log: the shim says so in the
tab and leaves the two options out of its temporary session. Limitation
from PuTTY: the codes are expanded in the ANSI code page, so a name with
characters outside it loses them (Chinese on a Chinese system is fine).
Verified: a raw test server sending SGR, OSC and charset escapes, UTF-8
text and a backspace, through the shim into a folder `logs 日志` it
created: the text log had the lines without escapes, the raw log every
byte, a second run appended, `&H` became `127.0.0.1`.

Verified in the portable Terminal against local test servers: the window
size at connect (120x30) and after resizes (59x14, 102x25); Ctrl+C sent as
0x03 without ending ntplink; raw sends no EOF on connect and ends as soon
as the server closes; `LocalEcho=0 LocalEdit=1` on raw echoes locally and
sends key by key; end to end through the shim (found next to it, a
PuTTY option passed with `-set`), Break and AYT sent from the NativeTerm
side arrived at the Telnet server as `IAC BRK` / `IAC AYT`, and the
server's close showed "disconnected". Serial Break itself couldn't be
observed: the virtual COM driver doesn't pass Break on (not even from
.NET), so it waits for a real device.

### Differences from SSH

- **No login signal:** there is no `LocalCommand`, so "logged in" isn't
  known. Commands after connecting are sent after a configurable delay.
- **No saved-password autofill:** the login prompt is ordinary terminal
  output, which NativeTerm never sees. Blind, delayed password sending
  is not offered.
- **Disconnect detection:**
  - *Telnet:* plink exits with **0** as soon as the server closes, so
    for Telnet a 0 means "disconnected" (keep the tab, offer reconnect),
    not "close the tab".
  - *Raw:* plink does **not** notice a server close. The socket stays
    in `CLOSE_WAIT` until the next keystroke, and then plink exits with
    1. The shim runs plink as its child and polls
    `GetExtendedTcpTable(TCP_TABLE_OWNER_PID_ALL)`, IPv4 and IPv6, for
    the child's connections every 300 ms. On `CLOSE_WAIT` it ends plink
    and shows "disconnected". Verified: detected within one poll after
    the server closed, with no keystroke.
- **Input injection** works as for OpenSSH (verified for raw and
  telnet). The Enter key goes out as CR NUL in Telnet (NVT), as CR in
  raw.
- **Serial** (verified with a virtual COM pair; see PROTOTYPES):
  - COM ports are enumerated for the editor.
  - A port can be held by one session only. A second plink failed with
    "Unable to open connection: Opening '\\.\COM30': Error 5" (access
    denied) and exit code 1. NativeTerm checks this before opening and
    shows "port busy", with the owning tab if it is its own.
  - There is no connection to lose: when the device side closed its end,
    plink kept running and writing. A dead device only shows as
    silence, so the tab shows "no data since …" instead of
    "disconnected".
  - Closing the tab ends plink and frees the port immediately.
  - Enter goes out as CR. The code pages work as for Telnet.
  - A baud-rate mismatch couldn't be observed on the virtual pair (data
    arrived intact); on real hardware it shows as garbage, and the
    editor offers common rates.
  - plink can't send a Break.
- **SSH through plink** (for GBK SSH hosts only) doesn't read
  `~/.ssh/config`. NativeTerm passes host, port, user, and key from
  `ssh -G`. Keys must be in PuTTY format (`.ppk`), and the agent is
  Pageant. This is an opt-in per host.

## Session config storage: `~/.ssh/config` + `Include`

No separate database. Folder grouping (the SecureCRT-style tree) maps onto
OpenSSH's own `Include` directive:

```
# ~/.ssh/config
IgnoreUnknown NativeTerm*
Include ~/.ssh/config.d/*.conf
```

Each `~/.ssh/config.d/<folder-name>.conf` file becomes one sidebar folder;
each `Host` block inside it becomes one leaf. `ssh` itself, and every other
tool that reads this file, sees exactly the same thing NativeTerm does.

### NativeTerm's own per-host settings

Settings such as tab color or persistence also live in the ssh config,
using OpenSSH's `IgnoreUnknown` directive: keywords matching
`NativeTerm*` are ignored by `ssh` but read by NativeTerm. `IgnoreUnknown`
must appear before the first such keyword, so NativeTerm adds it at the top
of `~/.ssh/config`.

```
Host web01
    HostName 10.32.16.66
    User ops
    NativeTermTabColor #C0392B
    NativeTermProfile Production
    NativeTermPersistent tmux
```

Folder-wide defaults: a `Host` pattern inside a folder file is *not* scoped
to that file (`Include` just splices text in), so defaults are declared in
a reserved pseudo-host, `Host __nativeterm_folder__`, at the top of the
folder file. NativeTerm applies its `NativeTerm*` keys to every host in
that file; `ssh` never matches that name.

Keys NativeTerm reads:

| Key | Meaning |
|---|---|
| `NativeTermId` | **Stable id** (UUID), written once when a host is created or imported. App data (recent, usage, long notes, tags, open-session records) is keyed by it, so renaming the alias or label loses nothing |
| `NativeTermLabel` / `NativeTermNote` | Display name / one-line description |
| `NativeTermSource` | Where an imported host came from (`securecrt:<folder>/<session>`); a later import skips it |
| `NativeTermFavorite` | Favorite |
| `NativeTermTabColor` / `NativeTermProfile` / `NativeTermColorScheme` | Appearance |
| `NativeTermPersistent` | tmux / screen |
| `NativeTermOnLogin` | Post-login command(s) |
| `NativeTermPreConnect` | Local command run by the shim before `ssh` (SecureCRT "Pre-connect"); runs as the user, shown in the session editor |
| `NativeTermCredential` | Name of a shared credential set (Credential Manager entry `NativeTerm/cred/<name>`) used by several hosts, like SecureCRT's "Credentials"; on a folder, its hosts' default; `none` on a host keeps the folder's set away |
| `NativeTermTrzsz` | Opt-in: run `trzsz ssh` (user-installed) instead of `ssh` for `rz`/`sz`-style transfers. It sits in the output path, which is why it is per host and off by default |

### What lives where (no database *of record*)

"No own database" means connection data is never stored twice: `~/.ssh`
is the source of truth, shared with `ssh`, `scp`, and VS Code Remote.
App state that ssh config can't or shouldn't hold lives in the data
directory. Search indexes exist only in memory.

| Data | Where | Why |
|---|---|---|
| Hosts, folders, labels, one-line notes, favorites, appearance, stable id | `~/.ssh/config.d/*.conf` (`NativeTerm*` keys) | Shared with ssh; syncs with the sessions |
| Non-SSH sessions | `~/.ssh/config.d/<folder>.nt.toml` | Same folder, not read by ssh |
| Open-session registry (session GUID ↔ `NativeTermId`, label, window, position hints) | `state.db` (SQLite) in the data directory | Must survive NativeTerm restarts (re-claiming, restored placeholders) |
| Recent sessions, usage counts, last connected | `state.db` | Per machine |
| Long notes (multi-line), tags | `state.db`, keyed by `NativeTermId` | ssh config values are single-line |
| Command library | `commands.toml` | Hand-editable, synced |
| Audit log | `audit\<machine>.log` | Append-only |
| Search index (labels, aliases, IPs, notes, pinyin initials) | Memory, rebuilt from the parse; optional cache file that can be deleted | ~800 hosts parse in milliseconds; fuzzy matching over them takes under 1 ms |

`state.db` is a single SQLite file (bundled `rusqlite`, no service), so
the data directory stays portable. Long notes and tags sync as a small
export file next to it (`notes.toml`), not as the database itself.
SQLite files and two-way sync don't mix.

Implemented (`notes.rs`, `registry::Note`): the `notes` table holds
`(nt_id, text, tags, updated_at)`, and every change is written to
`notes.toml` as well — whole file, temporary file and rename. At start
the two are merged by id: for each host the newer of the two wins, the
database takes what the file brought, and the file is rewritten with
whatever it was missing. A note someone cleared stays as an empty note
with its time, so clearing one reaches the other computers instead of
being taken for "nothing here yet". Nothing is ever merged *within* a
note: two people writing about the same host at the same time keep the
later text, and the file is plain enough to sort out by hand. A host
that has no `NativeTermId` gets one written into its block the first
time something is kept about it (`Editor::ensure_id`), since that id is
what the note belongs to. Notes and tags are searched along with the
name, alias, host, user and the one-line note, and both show in the
host's hover text.

Tables, as a first sketch:

- `sessions_open(session_guid, nt_id, label, window_hint, position_hint,
  opened_at)`;
- `recent(nt_id, machine, last_connected, count)`;
- `notes(nt_id, text, tags, updated_at)`.

### Host aliases and display names

`Host` aliases share **one namespace across all `config.d` files**, and an
alias can't contain spaces; `*`, `?`, and `!` are pattern characters. User
labels — especially ones imported from SecureCRT, such as
`10.32.16.66(osp-control1)` — may contain spaces, Chinese text, and
parentheses, and may repeat across folders. So:

- the **alias** is a generated, sanitized, globally unique identifier
  (lowercase ASCII, digits, `-`, `_`, `.`; prefixed with the folder when
  needed to stay unique), used by `ssh` and in the `wt` command line;
- the **display name** is stored separately as `NativeTermLabel`, and a
  free-text description as `NativeTermNote`;
- renaming changes only the label; the alias stays stable, so tmux session
  names, favorites, and history keep working.

```
Host ceph-cluster.osp-control1
    HostName 10.32.16.66
    NativeTermLabel 10.32.16.66(osp-control1)
    NativeTermNote OpenStack control node 1
```

### Parsing and writing rules

- **Effective settings come from `ssh -G <alias>`**, which prints the final
  configuration after `Match`, wildcards, `Include`, and defaults are
  applied. NativeTerm does not reimplement OpenSSH's precedence rules; its
  own parser only builds the tree and performs edits.
- **Sessions and shared settings.** A `Host` line is a pattern list,
  and a common hand-written layout gives one block per host plus blocks
  of settings shared by several:
  ```
  Host node01 incus-node-01
      HostName 10.32.32.130
  Host node01 node02 node03 incus-node-*
      User root
  ```
  A block is a **session** if it sets `HostName`, or if it names only
  literal hosts that no `HostName` block defines (ssh then connects to
  the name itself). Everything else is **shared settings**: blocks with
  wildcards, or blocks naming hosts defined elsewhere, possibly in
  another file. These are listed separately, and their effect shows in
  `ssh -G`. `Match` blocks are never sessions.
  - A session's first literal name is the alias NativeTerm connects
    with; further literal names are shown as alternates.
  - A duplicate-alias warning is raised only when two *session* blocks
    use the same name; ssh takes the first.
  - Verified on this machine's config: 8 sessions, 2 shared blocks,
    parsed in under 1 ms. `ssh -G node01` reported `user root` from the
    shared block.
- Writes are **format-preserving** (targeted line edits, comments and
  spacing kept), never a parse-and-reserialize round trip.
- **File permissions are enforced by ssh.** Windows OpenSSH checks
  `~/.ssh/config` *and every `Include`-d file* and aborts with "Bad owner or
  permissions" if the owner isn't the user, Administrators, SYSTEM, or
  TrustedInstaller, or if any other account can write to it. Therefore:
  - files NativeTerm creates get a protected ACL that only the user,
    Administrators, and SYSTEM can write. Existing files are replaced
    with `ReplaceFileW`, which keeps their ACL (verified; it may add the
    auto-inherited flag, the entries stay the same);
  - the check applies to the default user config and to **every
    `Include`-d file, even under `-F`** (readconf.c adds
    `SSHCONF_CHECKPERM` for includes), but not to a main file given with
    `-F`. Verified with the system ssh: a written file passed; after
    granting Everyone write access, `ssh -G` failed with a permission
    error and the writer rolled the next change back;
  - a `config.d` placed on a network share, a USB drive (FAT/exFAT has no
    ACLs), or any synced folder must be checked — NativeTerm validates the
    whole configuration with `ssh -G` after every change and at startup,
    and explains the permission problem instead of letting every
    connection fail.
- The files are watched; external edits refresh the tree. Before writing,
  NativeTerm checks the file hasn't changed since it was read and refuses
  to overwrite a newer version.
- **Backup and rollback**: every `ssh` tool on the machine reads these
  files, so a bug in NativeTerm's editor could break all SSH use. Before
  each write, the previous version is copied to `backups\` in the data
  directory (timestamped, pruned by age and count). After writing,
  `ssh -G` must parse the configuration for the affected hosts; if it
  fails, the previous version is restored automatically and the error is
  shown. Backups can also be restored manually from settings.

### Editing in the app (implemented)

- **What the dialog edits:** name (`NativeTermLabel`), host, user, port,
  jump host, keys (one `IdentityFile` per line) and a one-line note
  (`NativeTermNote`). Empty fields are removed from the block. A name
  equal to the alias isn't stored.
- **New hosts:**
  - The alias is generated from the name (`alias::unique`), and a
    `NativeTermId` is added.
  - The block is appended to the folder's file.
  - The main config's `IgnoreUnknown` and `Include` lines are ensured
    first.
- **Aliases never change:** open tabs, `ProxyJump` references and the
  user's scripts use them. Renaming only changes the label.
- **Moving a host:**
  - The block, including comments inside it, is written to the target
    file first, then removed from the source.
  - If the removal is rejected, the copy is taken out again, so the host
    never ends up defined twice.
- **Validation:**
  - After each write, ssh must accept the whole configuration, and the
    host must resolve to the host name that was entered.
  - The second check catches an earlier block for the same name, which
    ssh would use instead (first match wins; files included earlier
    come first).
  - A rejected change is rolled back, and ssh's message is shown in the
    dialog.
  - Seen in testing: a hand-made include file inheriting a sandbox
    group's permissions made *every* change fail with "Bad permissions".
    The message names the file and the account. A "fix permissions"
    action is on the roadmap.
- **Other directories** (tests, `--ssh-dir`): ssh is pointed at their
  config with `-F`. Their `Include` lines must be absolute, because
  relative and `~` includes resolve against the real `~/.ssh` even under
  `-F`.
- **Search:**
  - Fuzzy: every word must match one of name, alias, host, user, note
    or folder, with name hits weighted double.
  - Recently used hosts rank higher. Results are cached until the query
    or the tree changes.
  - Enter opens the best hit, and Esc or the × button clears the search.
    egui drops the text field's focus in the same frame Esc arrives, so
    "lost focus" counts too.
- **Tree rows:**
  - Rows are drawn by hand (full width, left-aligned) and still carry
    accessibility info.
  - Only visible rows are laid out (`show_rows`).
  - With 2000 hosts in 50 folders: 25 MB private memory (78 MB with the
    earlier GPU renderer), and no CPU when idle, even with the search box
    focused.
- **Recent hosts:** the top of the tree shows the five most recently
  opened hosts (from `state.db`).

### Ad-hoc connections

Quick connect accepts `user@host[:port]` for a machine that isn't in the
config; after the session, the user can save it into a folder.

Implemented: typing `user@host`, `host:port`, `user@host:port`, a name
with a dot, or a bare IPv6 address (optionally after `ssh `) into the
search box adds a "Connect to …" row on top; Enter still opens the best
saved match and falls back to the typed target. The shim gets `user@host`,
or `ssh://user@host:port` when there is a port (Windows OpenSSH 9.5 takes
that form, but not `ssh://[v6]:port`, so IPv6 targets can't carry a
port). The destination comes after `--`, as for aliases. A session opened
this way has "Save…" on its card, which opens the new-host dialog for
`~/.ssh/config`, filled in.

The open sessions are cards: a state dot, name and state on the first
line; tab position and buttons on the second, wrapping on narrow windows.

### Per-host terminal type

Some older network devices only understand `vt100`, while Windows OpenSSH
defaults `TERM` to `xterm-256color`. No NativeTerm-specific key is needed:
ssh takes the pty's terminal type from a `SetEnv TERM=vt100` line in the
host block (falling back to the `TERM` environment variable), which plain
`ssh` honors as well. NativeTerm offers it as a per-host / per-folder
option and writes that standard line.

### Import from SecureCRT

An importer converts SecureCRT's session files (one `.ini` per session,
organized in folders, plus a `__FolderData__.ini` per folder) into
`~/.ssh/config.d/<folder>.conf`: folder structure, session name, host,
port, and username. It finds them automatically through SecureCRT's
`Config Path` registry value (`HKCU\Software\VanDyke\SecureCRT`); on the
author's machine that folder holds 188 folders and roughly 770 sessions, so
import and the sidebar are designed for thousands of entries, not
hundreds.

What the importer brings over besides sessions:

- **Descriptions** → `NativeTermNote`; session names → `NativeTermLabel`
  with generated aliases (see "Host aliases and display names").
- **Host keys** (SecureCRT's `KnownHosts` folder) → `~/.ssh/known_hosts`,
  merged without duplicates, so ~770 hosts don't each ask for a
  fingerprint on first connect.
- **Saved commands / button bar** (SecureCRT's `Commands` folder) → the
  command library (see "Command library").
- Jump host and port-forwarding settings → `ProxyJump` / `LocalForward`
  lines where they map directly.

What it doesn't import, and reports instead of silently dropping:

- **Non-plink protocols** (RDP, TAPI, …): listed with their folder.
  Telnet, serial, raw, and rlogin sessions *are* imported, as plink
  sessions into the folder's `.nt.toml` (see "Other protocols via
  plink"), including serial line settings and a GBK character set where
  SecureCRT had one.
- **Logon actions / scripts** (expect-style "wait for X, send Y"): listed
  per session; see "Command library" for the partial replacement and
  "Known limitations".
- **Encrypted passwords**: key-based auth and `ssh-agent` are the intended
  path (see "Passwords").
- Duplicates (the same host and user in several places) are flagged for
  review.

The result is shown as a summary before anything is written.

#### Implementation (first version)

- **Where:** `native-term-config::securecrt` (`scan` reads the files,
  `plan` decides folders, aliases, and what is skipped),
  `Editor::import` writes the plan, `native-term-app::import` words the
  summary. The app has "Import from SecureCRT…" in its top bar; the
  `securecrt_import` example does the same on the command line (preview
  by default, `--write --ssh-dir <dir>` to import).
- **File format** (from public samples and VanDyke's own import scripts):
  `S:"key"=text`, `D:"key"=<hex>`, `Z:"key"=<hex count>` followed by that
  many lines (each with one leading space), `B:"key"=<hex length>`
  followed by indented hex lines. UTF-8 with BOM, CRLF. The host is
  `Hostname`, the user `Username`, the port `[SSH2] Port` for SSH2 and
  `Port` otherwise, the description `Description`. `Default.ini` at the
  top and every `__FolderData__.ini` are not sessions.
- **Secrets never enter the model.** Values of keys containing
  "password", "passphrase", "login script" or "logon script" are not
  kept; only whether they were set is recorded, for the report
  ("Saved passwords aren't imported (N sessions)"). A test checks that
  no such value survives parsing. During development only public
  samples and synthetic files were read, not the author's own session
  files.
- **Folders:** SecureCRT's nested folders become flat NativeTerm folders
  labeled with the whole path (`生产 / 控制节点`); sessions directly
  under `Sessions` go to a folder "SecureCRT". A folder whose label
  already exists is appended to. The tree doesn't show the nesting yet.
- **Aliases:** generated from the session name (or the host name when
  the name has nothing usable), unique across all files; on a clash the
  folder path is put in front (`shengchan-xiangmu01.bastion`). Han
  characters become pinyin without tones (`控制节点0` →
  `kongzhijiedian0`), for all generated aliases and folder file names,
  not just imported ones.
- **Jump hosts:** `Firewall Name` = `Session:<path>` becomes `ProxyJump`
  with the alias the jump session got (in this import or an earlier
  one). A jump session that isn't imported is reported, and the host is
  written without it. Other firewall names (global SecureCRT
  firewalls/proxies) are reported, not imported. Not checked against a
  real configuration yet; the preview shows what it found.
- **Port forwards:** `Port Forward Table V2` / `Reverse Forward Table V2`
  entries (`name|[bind,]port|different?|host|port||`) become
  `LocalForward`, `RemoteForward`, or `DynamicForward` (target
  `socks,`). A target that isn't "different from the SSH server" is
  `localhost` as seen from the server. Forwards take two arguments, so
  they are written unquoted, argument by argument.
- **Keys:** `Identity Filename V2` is used only when the session doesn't
  use the global key (`Use Global Public Key` = 0); `::…` options and a
  `.pub` suffix are dropped. The key must be in OpenSSH format (the
  report says so).
- **Descriptions:** all lines joined with " · " into `NativeTermNote`.
- **Skipped and reported:** Telnet, serial, raw and rlogin (plink
  sessions come later), RDP and other protocols, sessions without a
  host name, sessions imported before (`NativeTermSource`). Reported but
  imported: duplicates (same host, port, user), non-UTF-8 character
  sets, logon actions, unreadable forwards.
- **Writing:** one write per folder through the safe writer (backup,
  owner-only ACL), then every new alias is resolved with `ssh -G` and
  must give its host name; a failing folder is rolled back and listed,
  the others stay. Folders are written by 8 workers at once; a folder
  that failed is tried again on its own, in case another folder's
  unchecked file was the cause.
  - Measured on a synthetic configuration (188 folders, 736 sessions):
    733 hosts in 184 folders in 29 s from the command line, ~40 s from
    the app, most of it `ssh -G` (each run reads all folder files).
    Idle memory afterwards: 34 MB.
  - Two bugs found on the way:
    - **Wrong ssh.** From Git Bash, `ssh` on `PATH` is Git's MSYS
      build, which doesn't find the `C:/…` include, so every folder was
      rolled back. NativeTerm now uses `native_term_session::ssh_program()`
      everywhere (checks and tabs): `NATIVETERM_SSH`, else the first
      native `ssh.exe` on `PATH`, skipping MSYS/Cygwin builds, else
      `System32\OpenSSH`.
    - **A console per check.** From the GUI, every `ssh -G` opened a
      console window (slow, and visible). Checks now run with
      `CREATE_NO_WINDOW`; the same applied to editing hosts before.
- **Host keys** (`native-term-config::known_hosts`): SecureCRT's
  `KnownHosts` folder (next to `Sessions`) is read leniently, because its
  format isn't documented: one key per `.pub` file named
  `<name>[<address>]<port>.pub`, holding an OpenSSH line or an RFC 4716
  block (`ssh-keygen -e` output); the key type inside the blob must match.
  Anything else is counted as "not understood" in the preview, never
  guessed. The keys become `known_hosts` lines (`address,name` or
  `[address]:port,[name]:port`), appended only where the same key isn't
  already listed for that name (hashed entries can't be compared), through
  the safe writer (backup, owner-only ACL for a new file). Checked with
  `ssh-keygen -F` on the result. Not verified against a real SecureCRT
  folder yet: the preview shows how many keys were understood.
- **Not yet:** Command Manager commands (their storage isn't documented;
  button bars are imported, see "Command library").

## Cloud sync (optional, via rclone)

Lets the same sessions follow the user to another computer, on any
storage rclone supports (S3, Google Drive, OneDrive, and dozens more).
Still no storage of NativeTerm's own: the synced data is the same plain
files under `~/.ssh`, and the cloud account is the user's.

### rclone is not bundled; the user installs it knowingly

- NativeTerm never downloads or installs rclone itself.
- Enabling sync opens an explanation page first:
  - what rclone is (a third-party, MIT-licensed open-source tool) and that
    sync requires it;
  - how to install it (e.g. `scoop install rclone`, `winget install
    Rclone.Rclone`, or the official download);
  - exactly which files will be uploaded, and the risks below.
- Sync is enabled only after the user explicitly agrees. NativeTerm then
  detects `rclone` on `PATH`, checks it is recent enough for `bisync`, and
  lets the user configure the remote (rclone's own browser-based login).
- Sync can be turned off at any time; turning it off never deletes cloud
  data unless the user explicitly asks.

### What is synced

| Data | Synced? | Notes |
|---|---|---|
| `~/.ssh/config.d/*.conf` (sessions, folders) | Yes | Two-way, `rclone bisync` |
| Machine-independent settings (theme, language, FAB actions) | Yes | |
| Command library (`commands.toml`) | Yes | |
| Non-SSH sessions (`config.d/*.nt.toml`) | Yes | With the folder |
| Long notes and tags (`notes.toml`) | Yes | Exported from `state.db`; merged by `NativeTermId` |
| Config backups, recent sessions, open-session registry (`state.db`) | No | Per machine |
| `~/.ssh/config` (main file) | No | May hold machine-specific entries; NativeTerm ensures its `IgnoreUnknown` / `Include` lines on each machine |
| `~/.ssh/known_hosts` | Optional | Merged by NativeTerm as a line union, not by `bisync` — both machines append to it, so plain two-way sync would conflict constantly |
| Machine-specific settings (window/FAB positions, DPI) | No | |
| Audit log | Yes, per machine | One file per machine name, so no conflicts |
| Private keys | User's choice (off until chosen) | The user decides, with or without an rclone `crypt` remote. NativeTerm only informs: the risk of keys on cloud storage, a stronger warning when the remote is not encrypted, and the alternatives (a `crypt` remote, or a new key per machine installed with "Install my key") |
| Saved passwords (Credential Manager) | No | Bound to this Windows user and machine anyway |

### Sync behavior

- Runs on startup, after local changes (debounced), and periodically.
- Conflicts (the same folder file changed on two machines) produce rclone
  conflict copies; NativeTerm shows them and lets the user choose. One file
  per folder keeps conflicts rare.
- File permissions after sync: files rclone writes under the user profile
  are owned by the user and inherit the profile's ACL, which normally
  satisfies Windows OpenSSH's permission check (see "Parsing and writing
  rules"); NativeTerm still validates with `ssh -G` after each sync.
- Paths are written as `~/.ssh/...`, never `C:\Users\<name>\...`, so the
  files work on machines with a different user name — and later on
  macOS/Linux.

### Security

- A server inventory (host names, IPs, users) is sensitive; many companies
  forbid it on public cloud storage. Sync is opt-in, and an rclone `crypt`
  remote (client-side encryption of names and contents) is recommended.
- rclone's own config file holds cloud access tokens, in plain form unless
  the user sets an rclone config password. It stays in the user profile
  (not the portable NativeTerm folder, which would carry the cloud account
  along when copied), and NativeTerm suggests setting that password.

### Setting up a new computer

Install NativeTerm → install rclone → connect the cloud account → pull →
the session tree is back → generate a new key and install it on the
frequently used hosts.

For OneDrive-only users, a zero-tool alternative: keep `config.d` inside
the OneDrive folder and point `Include` at it.

## Session lifecycle (the shim)

Because `wt` launches the tab's process, NativeTerm itself never owns the
`ssh` process. The shim closes that gap:

- It reads its session GUID from `WT_SESSION` and registers it on
  NativeTerm's named pipe.
- It builds the `ssh` command line itself (host alias from its argument;
  `LocalCommand`, keepalive, and other options from NativeTerm's settings
  and `ssh -G`), so no complex quoting ever passes through `wt`.
- It runs `ssh` as a child in its own console (the tab), with no stdio
  redirection, so `ssh` keeps a real interactive console.
- Before every attempt it runs the host's `NativeTermPreConnect`, if it
  has one (its own or its folder's, `none` to keep the folder's away):
  `cmd /c <command>` in the tab, with the session waiting for it. That
  is what brings the VPN up, opens the tunnel or mounts the drive — and
  it runs again before a reconnect, since after sleep those are down
  too. A command that fails is reported and the connection goes ahead,
  because many useful ones "fail" (a VPN that was already up); `!` in
  front of the command makes a failure stop the connection instead.
  `.nt.toml` sessions have the same through `pre_connect`.
- It waits for `ssh` to exit and reports the exit code.
- It stays alive after `ssh` exits, so a disconnected tab remains visible
  (like SecureCRT's red disconnected tabs) until the user closes it, and
  can re-run `ssh` in place when asked to reconnect. Optional automatic
  reconnect with backoff. Windows Terminal keeps a tab open as long as any
  process is attached to its console, so this works regardless of the
  user's `closeOnExit` setting.
- **Ctrl+C**: while connected, Windows OpenSSH turns off processed input
  (raw mode), so Ctrl+C is just the character `0x03` sent to the server
  and no signal reaches the shim. `ssh` restores the console mode when it
  exits; from then on Ctrl+C is a `CTRL_C_EVENT` delivered to every
  attached process — the shim ignores it (`SetConsoleCtrlHandler`) so the
  tab stays. (Ctrl+C at a password prompt makes `ssh` itself exit.)
- **Tab closed by the user**: Windows sends `CTRL_CLOSE_EVENT` and
  force-terminates the process about five seconds later; the shim can't
  survive this and uses that window only to report "tab closed".
- If the pipe is gone (NativeTerm not running, restarting, or crashed), the
  shim keeps `ssh` working normally and keeps retrying the connection in
  the background.
- **Without a host argument** (the profile's command line: a pane
  restored by Windows Terminal, "Duplicate tab", a new pane from the
  profile), the shim asks NativeTerm, which closes placeholders it
  replaces with a real session. With no answer within a few seconds (or
  no NativeTerm), it starts a local shell.
- After `ssh` exits the tab shows the outcome ("Login failed or
  cancelled", "Disconnected", "Session ended", each with the exit code)
  and "Press R to reconnect, C to close this tab"; Enter also
  reconnects. NativeTerm's Connect/Close commands do the same.

### Shim ↔ NativeTerm protocol (implemented)

- One JSON object per line over the per-user pipe (see Security),
  `"type"` tagged. Shim → NativeTerm: `hello` (protocol version, role,
  pid, `WT_SESSION`, `--session`, alias), `connecting`, `authenticated`,
  `exited {code}`, `closing`. NativeTerm → shim: `welcome`, `connect`,
  `disconnect`, `close`, `send_text {text, enter}`, `local_shell`.
- The pipe handles are synchronous, so a blocked read would also block
  writes on the same handle: readers peek (`PeekNamedPipe`) and poll
  every 20 ms instead. The server always keeps one unconnected instance
  ready, and `FILE_FLAG_FIRST_PIPE_INSTANCE` makes a second NativeTerm
  fail to bind.
- The shim's link runs in a background thread: it retries every 2 s and,
  after every (re)connect, sends `hello` plus the latest lifecycle state
  (`connecting`/`exited`, and `authenticated` if seen), so a restarted
  NativeTerm learns each tab's state at once.
- Login detection has two paths: the `LocalCommand` helper
  (`nativeterm-shim --authenticated <shim pid>`) sets the named event
  `Local\NativeTerm-auth-<shim pid>`, which the shim reads when `ssh`
  exits (login failure vs. disconnect works without NativeTerm), and also
  connects to the pipe with role `auth_signal`. The helper writes and
  exits at once; the server accepts such already-closed clients
  (`ERROR_NO_DATA`) and still reads their messages.
- `LocalCommand` is skipped when the shim's path contains `%` (token
  expansion) or `"`; keepalive defaults (`ServerAliveInterval=15`,
  `ServerAliveCountMax=3`) are added only when `ssh -G` shows none.

### Closing tabs

Windows Terminal reports the exit code of a tab's **first** process — the
shim — and with the default `closeOnExit` (`automatic`) closes the tab
when that code is 0, but leaves it open with a "process exited" notice for
any other code. So:

- To close a NativeTerm tab, NativeTerm tells its shim to end `ssh` and
  exit with code 0. No UIA needed; UIA's close action is only a fallback.
- The "NativeTerm SSH" profile sets `closeOnExit` explicitly, so a user
  default of `never` doesn't break this.
- The shim never exits non-zero on its own, to avoid leaving tabs that
  show only "process exited".

Exit classification: **255** means a connection-level failure (refused,
dropped, auth failure, keepalive timeout). Windows OpenSSH adds a quirk:
when the connection closes without the server sending an exit status, its
exit code is **-1** (`0xFFFFFFFF`), which is treated the same as 255. Any
other code is the remote session's own exit status → "closed normally".
Whether a connection-level exit means "disconnected" or "login failed"
depends on whether the session ever authenticated (see "Login and
authentication").

### Keepalive

After sleep or a network change, `ssh` without keepalives often hangs
instead of exiting, so no exit code ever arrives and disconnect detection
and auto-reconnect silently stop working. NativeTerm therefore adds
`-o ServerAliveInterval=15 -o ServerAliveCountMax=3` (a dead connection is
noticed in about 45 seconds) — but only when `ssh -G` shows the user hasn't
configured these, because command-line options override the config file.

### Clones and port forwarding

A host with `LocalForward`/`RemoteForward`/`DynamicForward` can't be opened
twice: the second session fails to bind the same local port. Clones are
started with `-o ClearAllForwardings=yes`.

Implemented: "Clone Session" in the tab menu opens the clone with the
shim flag `--no-forwards`, which adds that option to every connect of the
tab. The flag is kept in `state.db` (`sessions.no_forwards`, schema 2),
so a clone replaced after a Terminal restore stays without forwards.

## Login and authentication

Host-key confirmation, password, key passphrase, and 2FA prompts appear
in the tab exactly as with plain `ssh` — the user answers them there. The
remote command (e.g. tmux) only runs after authentication succeeds.

### Knowing when a session has logged in

The shim starts every session with (built by the shim, not passed through
`wt`):

```
ssh -o PermitLocalCommand=yes -o LocalCommand="nativeterm-shim --authenticated" ...
```

How Windows OpenSSH runs it, and what that requires of the helper:

- **Timing**: after authentication, after the session channel is
  requested, and before the pty and remote shell are set up — exactly the
  "logged in" moment.
- **Execution**: through the C runtime's `system()`, i.e. `cmd.exe /c`,
  synchronously, in the same console, with the environment inherited. So:
  - the helper inherits `WT_SESSION` and needs no session argument;
  - `ssh` waits for it, so it must return quickly — it reports
    "authenticated" over the pipe with a short connect timeout and gives up
    silently if NativeTerm isn't running;
  - anything it prints appears in the tab, so it prints nothing;
  - the command string must avoid `%` (expanded by both ssh's token
    expansion and `cmd.exe`), and a helper path containing spaces (e.g.
    under `Program Files`) needs `cmd.exe`-correct quoting — covered by a
    test;
  - a `cmd.exe` AutoRun entry in the user's registry would also run here;
    NativeTerm warns if one prints output.
- **`ProxyJump`**: the jump connection is a separate `ssh` process that
  doesn't receive our options, so the signal only fires for the final
  host.

Session states: *connecting / waiting for login* → *connected* →
*disconnected* or *login failed* or *could not connect* or *closed*.

With `ProxyJump` and password authentication, one session asks for two
passwords (jump host, then target); the "authenticated" signal only
arrives after the final target, so the state stays "waiting for login"
through both prompts.

- **Waiting for login** is shown in the sidebar and tab switcher; opening a
  whole folder of password-auth hosts leaves many tabs waiting, and the
  user needs to see which.
- **Sending commands** is refused for sessions that haven't logged in.
  Windows OpenSSH reads password, passphrase, and host-key prompts
  directly from the console; it discards input that was pending when the
  prompt appears, but text injected *while* a prompt waits would be taken
  as the password or the yes/no answer. Group-send confirmation lists the
  excluded sessions.
- **Connection-level exit (255 / -1) before authentication** → "login
  failed / cancelled"; **after authentication** → "disconnected".
- **Could not connect**: when ssh connects directly (`ssh -G` shows no
  `ProxyCommand` / `ProxyJump`), the shim polls ssh's own TCP connections
  (`GetExtendedTcpTable`, every 250 ms until one is established). A 255
  before login with none ever established is "could not connect to the
  server" (`ShimMessage::Unreachable` before `Exited`; state
  `Unreachable`). Through a proxy the connection may be another process's
  (or not TCP at all), so it stays "login failed / cancelled". Verified on
  Windows 10 (OpenSSH 8.1): an unreachable address showed "could not
  connect (255)", a normal login still "connected".
- **Auto-reconnect never retries a login failure**, to avoid repeated bad
  attempts triggering server-side bans (e.g. fail2ban). It does go on
  after a reconnect that never reached the server (*could not connect*:
  no login was tried, e.g. the network is still down after sleep); before,
  the first such failure ended the retries, which is exactly when they
  are needed.
  - Implemented as a setting (off by default, kept in `state.db`'s
    `settings` table): "Reconnect dropped sessions automatically".
  - Only a drop after login (`Disconnected`) is retried; the core sends
    Connect after 3 s, 10 s, 30 s, then every 60 s, at most 10 times,
    plus up to 2 s derived from the session id, so sessions that dropped
    together (sleep, network change) don't all reconnect at once.
  - Nothing is sent if the session changed meanwhile (the user
    reconnected, closed it, or the setting was turned off).
  - The count starts over only after a connection stayed up for 60 s; a
    host that accepts the login and closes at once isn't retried forever.
    A failed reconnect always counts on (the last login's age says
    nothing then), and giving up resets the count.
  - The session row shows "auto-reconnect n" while it is retrying.
  - Live test: `tests/auto_reconnect.rs` (shims started directly, fake
    ssh): the dropped session reconnected twice in 17.5 s, the failed
    login stayed at attempt 1, nothing happened after the setting was
    turned off; a host that goes away after the first login
    (`FAKE_SSH_LOGIN_ONCE`, seen as direct and never reached) was retried
    on after the failed reconnects, shown as "could not connect" with
    "auto-reconnect 2".
- **Restored sessions** wait for Connect. When there are any, the
  sessions panel offers "Connect all" (200 ms apart, so jump hosts
  aren't hit at once) and "Close all"; single sessions connect from
  their row.

### Passwords

NativeTerm has no password storage or encryption of its own. The intended
path is key-based authentication plus `ssh-agent` (see "Installing public
keys"), so a key passphrase is typed at most once per login session.

#### Optional: saved passwords via Windows Credential Manager

For hosts that can't move to keys yet, a clearly-labeled opt-in feature
keeps passwords in Windows Credential Manager (encrypted by Windows in the
user's profile, not by NativeTerm) and supplies them through ssh's
`SSH_ASKPASS` mechanism.

No conflict with plain `ssh` usage (e.g. typing `ssh` in PowerShell):

- `ssh` never writes passwords anywhere. `known_hosts`, `~/.ssh/config`,
  and the Windows `ssh-agent` key store (`HKCU\Software\OpenSSH\Agent`) are
  shared by both and stay consistent — a host key accepted in either place
  is trusted in both.
- Entries are named `NativeTerm:<user>@<host>:<port>` and are visible in
  Control Panel → Credential Manager; nothing is written under `~/.ssh`.

Rules:

- **Per-process environment only.** The shim sets `SSH_ASKPASS` and
  `SSH_ASKPASS_REQUIRE=force` only on the `ssh` it launches. They must never
  be set as user or system environment variables, or manual `ssh`, Git over
  SSH, and VS Code Remote would all start calling NativeTerm's helper.
- **How Windows OpenSSH uses it** (supported, source-confirmed):
  `SSH_ASKPASS_REQUIRE=force` routes **every** prompt through the program —
  host-key confirmation, password and password change, key passphrase,
  PIN, keyboard-interactive (2FA). (`prefer` would additionally need
  `DISPLAY` set.) The program is started with the prompt text as its only
  argument, `SSH_ASKPASS_PROMPT=confirm` marks yes/no confirmations, and
  ssh reads the first line of its output and requires exit code 0.
- **The helper is the shim itself**, in askpass mode. The mode is detected
  from an environment variable **and** the call shape (exactly one
  argument that isn't a shim subcommand): `LocalCommand` and
  `ProxyCommand` children inherit ssh's environment, so the variable alone
  made the login-signal helper act as askpass and wait for console input
  (found in the prototype). The *host* comes from the session (`WT_SESSION`),
  never from the prompt text. The *kind* of prompt is decided as follows:
  only OpenSSH's own password prompt format (`<user>@<host>'s password: `)
  for the session's own user and host is answered from Credential
  Manager. Every other prompt — host keys, passphrases, 2FA, custom
  keyboard-interactive text — is shown by the helper itself in the same
  console tab (it is attached to it), so the user answers it exactly as
  without askpass. Echo can't be decided from `SSH_ASKPASS_PROMPT` alone:
  the host-key question arrives **without** `confirm` (verified), so the
  helper turns echo on for prompts ending in a `(yes/no…)?` question and
  off otherwise.
- **Stale passwords**: sessions run with `-o NumberOfPasswordPrompts=1`.
  One failure marks the saved password invalid and asks the user to update
  it, instead of retrying into a server-side ban (e.g. fail2ban).
- **Security trade-off, stated in the UI**: any program running as the
  user can read these credentials without a prompt. Malware on the
  workstation would get plaintext passwords for every saved host, which
  are often reused across machines. Keys plus `ssh-agent` remain the
  recommended path.
- Implemented (`native_term_win::credentials`, `native_term_config::password`,
  shim `saved.rs`):
  - **Entry**: `NativeTerm:<user>@<host>:<port>` from `ssh -G` (so all
    aliases of one account share it), a generic credential kept on this
    machine (`CRED_PERSIST_LOCAL_MACHINE`, not roaming), the password as
    UTF-8. The host dialog (editing a saved host) has "Saved password
    (optional)": the state (none / saved / refused), a password field
    with "Save Password" and "Remove" (written to or removed from
    Credential Manager at once, never kept in the dialog), and the
    warning above.
  - **Shim**: an account with a usable entry runs ssh with
    `SSH_ASKPASS=<shim>`, `SSH_ASKPASS_REQUIRE=force`,
    `NATIVETERM_ASKPASS=<pipe>` (per process) and `-o
    NumberOfPasswordPrompts=1`. The pipe (one per shim, user and SYSTEM
    only) answers only when the helper's parent is the ssh this shim
    started (`GetNamedPipeClientProcessId`, then the parent from a
    Toolhelp snapshot) and only ssh's own prompt for this account
    (`<user>@<host>'s password:`, `(<user>@<host>) Password:`): not a
    jump host's, not a bare `Password:`, not a passphrase or a code. The
    password is read from Credential Manager when asked, never kept.
    Every other prompt the helper asks in the console.
  - **Refused**: when the password was given and the login failed (255
    before login, the server reached), the entry's comment becomes
    "refused by the server"; it is kept but no longer used, the tab says
    so, and NativeTerm shows a notice (`ShimMessage::PasswordRefused`).
    Saving a new password clears the mark.
  - **Input method**: egui's password field let an IME in Chinese mode
    turn the typed letters into candidates (Sogou pinyin: "abc1" became
    "节能"), so a password saved that way was wrong. While a password
    field has the focus its frame asks for no IME, so eframe turns the
    IME off, as Windows' own password boxes do.
  - Verified: shim test with the fake ssh (the right password logs in,
    `NumberOfPasswordPrompts=1` passed, the password never printed; a
    changed one refused once, marked, the next attempt not given it);
    Credential Manager round trip under a test name; live, locally,
    against a temporary container with password login: saved through
    the dialog (with the Chinese IME on), connected with no prompt; the
    server's password changed: refused, marked, notice for both tabs,
    the next reconnect asked in the tab. Test entries were removed.

#### Shared credential sets (`NativeTermCredential`)

Many servers share one account's password (a batch of machines in one
rack, every device behind a jump host). A credential set is one saved
password that several hosts use, like SecureCRT's "Credentials":

- **Entry**: `NativeTerm/cred/<name>` in Credential Manager, next to the
  per-account entries; a set holds a password only. The user is the ssh
  config's, as always (a folder sets it for its hosts with `User` in the
  folder options), so there is no second source for the account.
- **Which hosts**: `NativeTermCredential <name>` on a host, or on a
  folder for its hosts; `none` on a host keeps its folder's set from it.
  A set wins over the account's own saved password. A name is 1–64
  characters without spaces, quotes, `/ \ : * ?` or control characters
  (it is written unquoted in the ssh config, is part of the entry name,
  and `*` would match other entries when they are listed), and not
  `none`.
- **The same rules as the account's own entry**: only this session's
  own user@host password prompt is answered (`Target::answers`), never a
  jump host's, a passphrase or a code; one try; a refusal marks the set
  (`refused by the server`), so every host that uses it stops giving it
  until a new password is saved, and the tab names the set. The files
  window uses the set the same way.
- **UI**: the host dialog's "Credential set" (as the folder / none / a
  set / New…) and its password section follow the choice at once: with a
  set it edits the set's password and says it is shared (no Remove
  there: that belongs where all its hosts are seen). The folder menu's
  "Credential Set" sets the folder's default. "Credential Sets…" (in the
  settings and the folder menu) lists the sets (names only are read to
  list them: `CredEnumerateW` with the prefix), how many hosts use each,
  a refused mark, a new password field, Remove (asked twice, with the
  count), a new set, and sets the ssh config names that have no entry.
- Verified: unit tests (entry names, which set a host uses: its own,
  its folder's, `none`; name rules; `ops` writes and refuses bad names;
  listing by prefix); a shim test with the fake ssh (a folder's set is
  given instead of a wrong per-account entry; a refusal marks the set,
  names it, leaves the account's entry alone). Live, against a container
  with password login: a set made in the dialog (IME off in the password
  field), put on the "Lab" folder from its menu; `daas-pw` (root) and
  `daas-pw2` (another account, `ops`, same password) both logged in with
  no prompt, and the files window connected with it; after the server's
  root password changed, a reconnect was refused once, the tab named the
  set, the dialog showed it refused, and `daas-pw2` then asked in its tab
  instead of giving the refused password.

### Installing public keys (`ssh-copy-id`)

Moving 400+ hosts from passwords to keys should cost one final password
per host. Windows OpenSSH has no `ssh-copy-id`, so the release bundles:

- **busybox-w32** — a single-exe set of Unix tools providing `sh`;
- **OpenSSH's own `ssh-copy-id` script**, run by that `sh`.

The upstream script skips keys the host already accepts, and it calls the
system `ssh.exe`, so ssh config aliases, users, `ProxyJump`, agent, and
`known_hosts` all apply. Verified in the prototype (`docs/PROTOTYPES.md`):
the script runs unmodified under busybox-w32 against a Linux host, and
the password can come from the askpass helper (saved password).

Constraints found while verifying:

- **Unix targets only.** The install step runs `exec sh -c '…'` on the
  remote; a Windows `sshd` runs it in `cmd.exe`, where it fails. The
  `-s` (sftp) mode doesn't help from a Windows client: it opens a
  ControlMaster connection (`ssh -M -S`), which Windows OpenSSH can't do.
  (Windows hosts got their own path later: see "Windows targets" below.)
- **Always pass `-i <file>`.** busybox `expr` rejects `--`, so the
  script's check for a bare `-i` followed by another option doesn't work.
  With an explicit file the result is correct; the harmless
  `expr: syntax error` / `expr: warning` lines are filtered from the
  summary.
- **Set `HOME` to a NativeTerm scratch directory.** The script creates
  its temporary directory under `$HOME/.ssh` (and removes it). With
  `HOME` pointing elsewhere, the user's `~/.ssh` isn't touched;
  `ssh.exe` itself reads the profile directory, not `HOME`, so config and
  keys still apply.
- **Implemented differently, without busybox.** The first version needs
  no bundled tools: the shim (`--install-key <pub> <alias>`) runs the
  system `ssh` once with a short POSIX script as the remote command. The
  script creates `~/.ssh/authorized_keys` with `umask 077`, adds the key
  unless its blob is already there (and a missing final newline first),
  and prints a marker the shim reads from stdout; the password prompt
  still comes from the console. Each host gets a tab ("Install My Key…"
  on a host, "Install My Key on All…" on a folder, 300 ms apart), so the
  user types each password where the host asks. The key comment is
  reduced to harmless characters; the key itself must be a plain OpenSSH
  public key. Without any public key, the
  dialog offers "Create a key…": a tab running `ssh-keygen -t ed25519`
  (`--create-key`), where the passphrase is chosen. Checked with a Git
  `sh` standing in for the host: added once, reported as present the
  second time, no duplicate.
- **One password for a batch** (implemented): when several hosts share a
  password, the dialog's "Same password on all of them" (on by default
  for more than one host) opens one tab running
  `--install-key-batch <pub> <alias…>` instead of a tab per host. The
  shim asks for the password once (console, no echo; a line of stdin when
  stdin is a pipe, for scripts and tests) and keeps it in memory only —
  never in an argument, the environment, a file or the log. It serves it
  on a pipe with a random name that only the user and SYSTEM may open,
  and runs each host's ssh with `SSH_ASKPASS=<the shim>`,
  `SSH_ASKPASS_REQUIRE=force` (Windows OpenSSH 9.5 honours it: verified),
  `NATIVETERM_ASKPASS=<pipe>` and `-o NumberOfPasswordPrompts=1`, so a
  wrong password fails that host once instead of being retried into a
  ban. The shim in askpass mode (that variable **and** exactly one
  argument, the prompt) asks the pipe; only password prompts are answered
  (`user@host's password:`, keyboard-interactive `(user@host) Password:`,
  `Password for …:`; not passphrases, new passwords or codes). Anything
  else — a new host key, a key passphrase, a one-time code — the helper
  asks in the tab's console, echoing only `(yes/no…)` questions. A
  Windows host's second ssh run (PowerShell) takes the same password
  without asking. The tab ends with a summary (added / had it / failed)
  and lists the failed hosts; ssh's own `Permission denied (…)` is
  reported as a refused login ("a wrong password?"). The batch is split
  into several tabs only if the aliases would overflow a command line
  (24 000 characters). Verified: with the fake ssh (three hosts, one with
  another password: 1 added, 1 present, 1 failed; the password never
  printed), and against the local Windows `sshd` in a portable Terminal
  tab with a user that doesn't exist: the host-key question came from
  the helper in the console, the password went through the pipe (ssh:
  `read_passphrase: requested to askpass`), the second host asked
  nothing, both were reported as refused logins.
- **Windows targets** (implemented): a Windows `sshd` runs the command in
  `cmd.exe` (or PowerShell), which rejects the POSIX script — and quotes
  it back: cmd stops at the `-qF` of `if grep -qF` ("-qF was unexpected
  at this time", "此时不应有 -qF"), PowerShell's parse error repeats the
  `umask` line. No POSIX shell prints either, so that is how the shim
  knows; ssh then runs once more with
  `powershell -NoProfile -NonInteractive -EncodedCommand <script>` (the
  password is asked again). Following Windows OpenSSH's rules, members of
  Administrators (`S-1-5-32-544` in `whoami /groups`) get the key in
  `%ProgramData%\ssh\administrators_authorized_keys`, which is then
  limited to Administrators and SYSTEM; everyone else in
  `%USERPROFILE%\.ssh\authorized_keys`. Because PowerShell echoes script
  source in its errors, both scripts assemble their success markers when
  printing them, so a quoted line can't pass for success. Output is read
  as bytes (a Windows shell answers in its code page, e.g. GBK). Tested
  end to end with the fake ssh running the command through `cmd.exe`, and
  the PowerShell script locally with the profile and ProgramData in a
  scratch folder (admin file created and locked; second run: present).
- **Ship `busybox.exe` alone, without applet shims or `PATH` changes.**
  The scoop package creates ~200 shims (`grep`, `ls`, `find`, `sort`, …)
  that compete with the user's own tools. NativeTerm calls
  `busybox.exe sh <script>` by absolute path.

Alternatives considered and rejected:

- Existing Windows ports: one ships binaries only (no source to audit — not
  acceptable for a tool handling logins to the whole fleet); one uses its
  own Go SSH library and therefore ignores `~/.ssh/config`.
- Git for Windows' `ssh-copy-id`: uses Git's bundled ssh, which can't reach
  the Windows `ssh-agent`.

Usage: "Install my key" on a host or a folder opens one tab that works
through the hosts sequentially (the password typed once there, or each
host asking when left empty) and ends with a per-host summary. Network devices (routers, VyOS) manage keys
through their own configuration commands — a manually edited
`authorized_keys` doesn't survive — so they're excluded.

## Persistent remote sessions (tmux)

Opt-in per host or folder (`NativeTermPersistent tmux`). Instead of a
plain login shell, the tab runs:

```
ssh -o RequestTTY=yes -o RemoteCommand="sh -c '… exec tmux new-session -A -s nt-<alias>-<id8> …'" <host>
```

`-A` attaches to the tmux session if it already exists, otherwise creates
it. The user never sees this command: ssh doesn't echo it, and the tab
title is the fixed label.

- **Passed as `-o RemoteCommand`, not as a trailing command**: Windows
  OpenSSH aborts if a host has `RemoteCommand` in its config *and* a
  command is given on the command line, while a `-o RemoteCommand` simply
  takes precedence. Hosts whose config already sets its own
  `RemoteCommand` are not wrapped (persistence is disabled for them, with
  a notice), since overriding it would silently change what the user
  configured.
- **One string, quoted for the remote shell**: ssh joins command words with
  spaces and the remote shell interprets the result, so the shim builds
  the full string itself. It is wrapped in `sh -c '...'` (independent of
  the user's login shell — bash, fish, csh):
  - if tmux exists, `exec` it; otherwise `exec` a normal login shell and
    report the fallback to NativeTerm;
  - optionally append `set-option status off` so tmux's status bar (which
    shows the `nt-<id>` name and window list) is hidden.

The shell and whatever runs in it live on the server, independent of the
SSH connection, so they survive:

- network drops, laptop sleep, Wi-Fi changes;
- the terminal window or tab being closed;
- NativeTerm or Windows Terminal crashing or restarting.

Reconnecting (manual or automatic) re-runs the same command and lands back
in the same shell, with running commands and screen contents intact.

What persistence enables beyond reconnecting:

- **Recovery after restart**: NativeTerm lists `nt-*` tmux sessions per
  host (`ssh <host> tmux ls`) and offers to reopen them as tabs.
- **Group send fallback**: `ssh <host> tmux send-keys -t nt-<id> '<cmd>' Enter`
  sends to a session without touching the terminal at all, at the cost of
  one extra SSH connection per target. Windows OpenSSH's `ControlMaster`
  connection sharing is unusable (its build can't pass file descriptors
  between processes), so each is a full handshake; run in parallel.
- **Text preview in the tab switcher**: `tmux capture-pane -p` returns the
  current screen text of a background session.
- **Looks like a plain session** (implemented): tmux's green status
  bar is turned off, for NativeTerm's own session only (`set-option -t
  =<name>: status off` on every attach, so sessions made earlier lose it
  too; nothing server-wide); the session card says "kept on the server
  (tmux)" instead, with what that means. A new session keeps 50,000
  lines of history (tmux's default is 2,000): `history-limit` applies
  to windows made after it is set, so the session is created, the
  option set, a new window opened and the first one (`^`, whatever the
  user's `base-index`) closed. `set-option` needs its target as
  `=<name>:`: tmux 3.4 answers "no such session" to `=<name>`. Not done:
  tmux's mouse mode would let the wheel scroll the history (tried: it
  does), but it takes the mouse from Windows Terminal (right click opens
  tmux's menu instead of pasting, dragging selects in tmux; Shift gives
  it back), so it stays off: Windows Terminal's own selecting, copying
  and right-click paste come first (the user's decision). Live: a new session had one
  window, history 50,000, the log pipe on and no status bar; after a
  disconnect and reconnect the tab showed the same shell and its output,
  still without the bar.
- **Server-side session logging** (implemented): `NativeTermPersistent
  tmux-log` ("tmux, recorded on the server" in the host dialog and the
  folder menu). The remote command creates the session detached with
  `pipe-pane "cat >> $HOME/.nativeterm/logs/<name>.log"` only when it
  doesn't exist yet (`tmux has-session -t =<name> || tmux new-session -d
  -s <name> ";" pipe-pane …; exec tmux attach-session -t =<name>`): on
  attaching, `pipe-pane -o` toggled the open pipe *off* (seen with tmux
  3.4: the second connection's output was missing, `#{pane_pipe}` 0) and
  a plain `pipe-pane` would replace it. `";"` rather than `\;`, since a
  backslash doesn't survive ssh's option parsing. The log holds
  everything the session's first pane shows, while no tab is attached
  too; it grows until deleted. A session made before `tmux-log` was
  chosen isn't recorded (the pipe is only added on creation). In
  "Sessions on the Server", a session with a log has "Log", and the logs
  of ended sessions stay listed ("ended, log kept"): the viewer shows the
  last 256 KB (`tail -c`) as text (`persistent::log_text`: escape
  sequences and control characters out, CR LF as LF, a lone CR ends a
  line, backspace by columns, so readline's `\b\b  \b\b` for a CJK
  character erases it), "Save a Copy" writes the whole file as it is to
  `<data dir>\logs\server\<alias>\<name>.log` and selects it in
  Explorer, "Delete on the Server…" asks once more. Verified against a
  daas Ubuntu container (tmux 3.4) through the shim in the portable
  Terminal: first connection, disconnect, reconnect, and a line sent to
  the detached session all in the log, one `cat` running; listing,
  reading, copying and deleting through the same functions the dialog
  calls. The dialog's own clicks weren't tried here.

Costs and caveats — why it's opt-in, not default:

- **Scrolling and selection change**: tmux manages its own screen, so the
  terminal's native scrollback no longer shows history; scrolling goes
  through tmux (mouse mode or copy mode), and with tmux mouse mode on,
  native text selection needs Shift+drag. This is a real step away from
  the "native terminal experience" this project is built around.
- **Key binding**: tmux's prefix (Ctrl+B) is intercepted.
- **Requirements**: tmux must be installed on the host (present on typical
  Ubuntu servers; absent on network devices like routers and VyOS). If
  it's missing, fall back to a plain shell and show it in the UI.
- **Nesting**: if the host's shell profile already starts tmux, don't wrap
  it again.
- **Naming**: `nt-<alias>-<first 8 hex digits of the session id>`
  (`nt-ceph-cluster_osd1-0f3a9c21`): readable in `tmux ls`, stable for the
  tab's lifetime (reconnects, NativeTerm restarts), new for a new tab.
  tmux names can't contain `.` or `:`, and ssh expands `%` in
  `RemoteCommand`, so only letters, digits, `-` and `_` are kept.
- **Leftovers**: detached sessions accumulate on servers; NativeTerm
  provides "list / kill remote NativeTerm sessions" per host and folder.
- **Security**: a detached shell stays logged in on the server until
  killed.

`screen` is supported the same way (`screen -D -R -S <name>`) for hosts
without tmux; `dtach`/`abduco` keep native scrollback but aren't installed
by default and don't restore screen contents.

Implemented (`native_term_config::persistent`, shim `persistent.rs`):

- **Setting**: `NativeTermPersistent tmux|screen|off` on a host, or
  `tmux|screen` in the folder's `Host __nativeterm_folder__` block as the
  default for its hosts; a host's own value wins, so `off` exempts one
  host from a persistent folder. The host dialog has "Keep on the
  server" (as the folder / tmux / screen / off); a folder's menu has
  "Keep sessions on the server" (off / tmux / screen); the host's tooltip
  shows what applies.
- **Shim**: read again at every attempt (an edit applies at the next
  reconnect). Needs the tab's session id (`--session`); a host whose
  config has its own `RemoteCommand` is left alone with a notice. The
  tab shows "Attaching to tmux session … on the server" before ssh starts.
- **Command**: `sh -c 'if command -v tmux >/dev/null 2>&1; then exec tmux
  new-session -A -s <name>; fi; echo "<missing>" >&2; exec
  "${SHELL:-/bin/sh}" -l'`, where `<missing>` is the localized "tmux is
  not installed on this host: a plain shell, not kept after a
  disconnect" (quotes, `%` and backslashes removed). Checked that
  Windows OpenSSH 9.5 and 8.1 pass `-o RemoteCommand=` through verbatim
  (`ssh -G`).
- **Windows servers**: their sshd runs `RemoteCommand` with its default
  shell (`cmd.exe`), where `sh -c` doesn't exist, so a persistent folder
  holding Windows hosts needs `off` on those hosts.
- **Verified** against a temporary Ubuntu 24.04 container (tmux 3.4,
  screen 4.09, bash): from the portable Terminal on Windows 11 and from
  Terminal 1.24 on Windows 10 (OpenSSH 8.1), a marker exported in the
  shell survived a disconnect (`tmux ls`: the session stayed, detached)
  and a killed `ssh.exe`; after the reconnect `echo $NT_MARK` printed it,
  with the screen intact. The same with screen (`Attached` → `Detached`
  → same shell). With tmux moved away: the fallback line, a plain shell,
  nothing kept. Also found there: editing a host of a config NativeTerm
  hadn't set up failed ("Bad configuration option: nativetermpersistent",
  rolled back), because only "new host" added `IgnoreUnknown
  NativeTerm*`; every write of a `NativeTerm*` key now does
  (`header::ensure_ignore`, without touching the user's `Include` lines).
- **Sessions on the server** (a host's menu): NativeTerm asks the host
  in the background with `ssh -o BatchMode=yes -o RequestTTY=no -o
  ClearAllForwardings=yes -o PermitLocalCommand=no -o RemoteCommand=…`
  (keys or the agent only; a host needing a password gets an explanation;
  the command goes in `RemoteCommand` so a host's own one can't clash),
  running `tmux ls -F …` and `screen -ls`. The dialog lists the `nt-*`
  sessions with program, creation time (tmux) and state: "in tab …"
  when an open tab of this NativeTerm has it (its session id gives the
  same name) with "Show tab", else attached elsewhere / detached with
  "Open". "Open" starts a tab whose session id begins with the name's 8
  hex digits, so the shim attaches to that session, and keeps doing so on
  reconnects. "End…" asks once more, then runs `tmux kill-session -t
  =<name>` / `screen -S <name> -X quit` and lists again. Verified
  locally (portable Terminal 1.26, daas container): the open tab's
  session shown "in tab", the tab closed from NativeTerm left it
  "detached", "Open" brought a tab back into the same shell (its history
  and variables there) and the list then showed it "in tab" again; "End…"
  → "End Now" on it ended the session on the server (`tmux ls`: no
  server), the list said there were none, and the tab showed "ended (0)"
  (the remote shell ended, ssh exited normally).
- **Per folder**: "Sessions on the Server…" in a folder's menu (only
  when a host under it is kept on the server) lists the sessions of all
  those hosts in one window, grouped by host, each with the same actions.
  The hosts are asked at the same time, six at most (one `ssh -o
  BatchMode=yes` each); a host that can't be asked shows its own error
  and the rest are still listed. After ending a session or deleting a
  log only that host is asked again. Two aliases of one server both list
  its sessions: each session shows under the host it belongs to only.
  "Open All (n)" opens every session of these hosts that is attached
  nowhere and in no tab, in new tabs (after a restart, say).
  Verified: two hosts on one container with a detached session each and
  an ended session's log; the folder's list showed each session under
  its own host and the log under its host; "Open All (2)" opened two
  tabs attached to them (`tmux ls`: both attached, no new session; the
  text typed before was still on screen) and the list then showed "in
  tab …" with "Show Tab".
- **Not yet**: previews, logging for screen sessions (`-L -Logfile` needs
  screen 4.6).

## File transfer (SFTP)

**Files (SFTP)…** in a host's menu or in a terminal tab's menu (the tab
menu's "Files (SFTP)") opens the session in **one files window for all
sessions**, laid out like SecureFX (the user's reference): local files on
the left, the server's on the right, each side with a tab per session;
the two tab rows move together (a click on a local tab selects the
session on both sides, and the other way round). A host already open is
shown instead of opened twice. Browse both sides, transfer between them,
create folders, rename, delete, and edit a server's file in place. Nothing is installed
on the server: the SFTP subsystem comes with OpenSSH's server (a user
without sudo, or one who won't install lrzsz/trzsz, can still transfer
files). Decided by the user: file transfer is built in, not only handed
to an external tool.

**Transport: `ssh -s <alias> sftp`, Windows' own OpenSSH.** The host's
config, keys, ssh-agent, jump hosts and `known_hosts` work exactly as in
its terminal tab; NativeTerm doesn't carry a second SSH implementation.
Options added: `RequestTTY=no`, `ClearAllForwardings=yes`,
`PermitLocalCommand=no`, `RemoteCommand=none` (a host's own
RemoteCommand next to `-s` makes ssh refuse), `ForwardAgent=no`,
`ForwardX11=no`, `ConnectTimeout=15`, `ServerAliveInterval=15`, no console
window. Windows OpenSSH can't share the terminal's connection
(`ControlMaster` needs descriptor passing), so the files window logs in
once more. ssh's stderr is decoded for messages (`\ooo` escapes, then
UTF-8 or the ANSI code page: "不知道这样的主机。" instead of
`\262\273…`, `native_term_win::ssh_message`).

**Authentication.** ssh's prompts go to the shim as askpass helper
(`SSH_ASKPASS` forced, `NATIVETERM_ASKPASS` = a private pipe of the files
window, `NATIVETERM_ASKPASS_NO_CONSOLE=1`). Only the helper whose parent is
this window's ssh is answered: the account's own password prompt
(`Target::answers`) from Credential Manager when saved and not marked
refused (then `NumberOfPasswordPrompts=1`; a refusal marks it, as in a
tab); any other prompt — a password not saved, a passphrase, a new host
key, a code — is asked in the window (password fields without IME).
Cancelling one cancels the rest of that connection's prompts (ssh would
ask again), and with no console the helper never waits on a hidden one.
The helper waits up to 300 s for an answer typed in the window.
Credential Manager secrets written by other tools (`cmdkey`, Windows'
dialogs: UTF-16LE) are read too (`credentials::blob_text`).

**Protocol: `native_term_sftp`, SFTP v3 of our own (~750 lines).** The
libraries checked (2026-09) — `russh-sftp` 3.0 and `openssh-sftp-client`
0.15 — both run over `ssh.exe`'s pipes on Windows at the same speed, but
both take file names as UTF-8 (`String` / `Path`): a GBK-named file on
the test server showed as `����.txt` and couldn't be opened
(`russh-sftp`), or the whole directory failed to list
(`openssh-sftp-client`); both also need tokio. Ours:
- a `Session` usable from several threads: requests written under a
  lock, a reader thread hands each reply to its request by id, so a
  folder is listed while a transfer runs;
- transfers keep 64 × 32 KB requests in flight (as OpenSSH's sftp),
  writing each chunk where it belongs (replies in any order, short reads
  asked again), progress per chunk; each can start at a byte offset
  (`download_from` / `upload_from`: the part before it is kept). A
  stopped transfer doesn't wait for the replies still in flight (2 MB of
  reads would take ~20 s at 100 KB/s): the reader drops them, the handle
  is closed without waiting, and as the server handles requests in
  order, whatever comes next sees the file as they left it;
- file names are bytes end to end. `Names` decides only how they're shown
  and how new ones are written: Auto (UTF-8 where valid, else the system's
  ANSI code page) or any `encoding_rs` encoding (GBK, GB18030, Big5,
  Shift_JIS, EUC-JP, EUC-KR, Windows-125x, ISO-8859-x, KOI8, …) chosen in
  the window or as `NativeTermFileEncoding` (host or folder). Bytes valid
  in neither show as `\xNN`; new names are encoded strictly (a name that
  can't be written in the encoding is refused, never a look-alike);
- `transfer`: folders both ways (planned first for the total, existing
  folders reused, files replaced; a link to a folder isn't followed, so
  no loops), recursive delete; server names become valid Windows names
  (`< > : " / \ | ? *`, trailing dots, `CON`…). Files keep their
  modification time both ways (as SecureFX and WinSCP do by default), so
  a copied file compares the same afterwards.
- `sync`: compares a local folder with a server's folder and everything
  below them. Names are matched as they would be written on Windows;
  files by size and modification time (2 s slack, as FAT keeps); a folder
  on one side only is one entry, copied or deleted whole; folders on both
  sides are gone into; `.ntpart` files are left out; a link to a folder
  on the server isn't gone into. What each entry needs, per direction:
  *both ways* (what a side lacks is copied to it; of two different files
  the newer wins; the same time with different sizes can't be decided),
  *local is the source*, *server is the source*; the one-way ones can
  delete what the target has in excess. A file on one side and a folder
  of that name on the other, or a name the server's encoding can't
  write, is left alone and said so.
- *Pause and resume:* a file is copied as `<name>.ntpart` (locally for a
  download, on the server for an upload) and renamed over the real name
  when complete (an upload keeps the old file's permissions; a server
  without `posix-rename` gets the old file removed first), so a partial
  file never passes for the real one. Pause stops a transfer and keeps
  its plan, the index of the first file not copied and the partial file;
  Resume goes on from that file, at the partial file's length. Cancel
  (running or paused) removes the partial file. A partial file is also
  continued by any later transfer of the same file (after a lost
  connection, a closed window, a restart) if it is no longer than the
  file and not older than the file's last change; otherwise the copy
  starts over. Closing the window or a tab stops transfers as Pause does.
  Speed counts only what the current run copied.

**The window** (`files_window.rs`; `window::open`: the runner opens
windows while running, each with its own egui context; sessions asked
for reach the open window through a queue it drains each frame; closing
the window, or a session's tab, with transfers or edits running asks
first; a session's tab closes its connection).
- *Local side:* starts where it was last for this host (else Downloads,
  see "Last folders"); folders, files, and above the top of
  a drive the drives ("This PC"); Open (a folder, or a file with its
  program), New Folder, rename, Delete (to the Recycle Bin, asked);
  Upload sends the selection to the server's folder shown.
- *Server's side:* up, refresh, path, Download (the selection to the local
  folder shown), Edit, Delete (asked, "folders with everything in them"),
  New Folder, rename, name encoding; `ls -l` permissions; links to
  folders open like folders; below it the session's log (connecting,
  connected and where, each transfer's end, errors).
- *Folder trees* beside both lists, as in SecureFX: the local one starts
  with Desktop, Documents, Downloads and the drives, the server's with
  `/`. A folder's subfolders are read when it is opened (a chevron, or a
  double click); the folders above the one shown open by themselves and
  it is highlighted; a click shows a folder in the list. A narrow list
  drops columns (permissions, then the date, then the size) before the
  name gets short.
- *Across:* the buttons, dragging the selection from one side's list to
  the other's list or onto a folder in the other side's tree (a green
  frame shows where it would go), or files dropped from Explorer onto the
  server's side.
- Ctrl/Shift selection, right-click menus, F5, Backspace, Enter, F2,
  Delete on the side clicked last. Rows, tabs and icon buttons are named
  for screen readers and UI automation (AccessKit).
- *Transfer queue* at the bottom, always shown (empty, it says how to
  start a transfer), every session's: its header counts what runs (with
  their total speed), what is done and what failed; each transfer shows
  host, progress, percent, bytes, speed and the time left while it runs
  (size and speed once done); Pause / Resume / Cancel on each transfer,
  Resume also on a failed one; Pause All, Resume All, Clear Finished
  (done, cancelled, failed), and ✕ to remove one finished row. The files
  being edited are listed there too, with their state.
- *Status lines* under both sides, on the same row: what the folder holds
  (folders, files) and what is selected (count and size); the server's
  starts with its connection (connected, connecting, not connected).
- *From a terminal tab kept in tmux* (`NativeTermPersistent tmux` /
  `tmux-log`) the server's side starts in the tab's current folder:
  `tmux display-message -p -t =<session>: "#{pane_current_path}"` over
  `ssh -o BatchMode=yes` (a host that needs a password falls back to the
  home folder); asking again for a host already open moves it there.
  Windows OpenSSH can't reuse the tab's connection, so "the same
  session" is the same host, config and login, on a connection of its
  own.

**Editing in place.** Edit (or Enter, double-click) downloads the file
to `%TEMP%\NativeTerm-edit\<host>\<random>\`, opens it with its program
(ShellExecute; none registered: Windows' "How do you want to open"), and
watches it: when it changes and has stopped changing, it is uploaded to
`.<name>.nt-<random>` next to the original, given the original's
permissions and renamed over it (`posix-rename@openssh.com`), so the file
on the server is never half written; where that isn't allowed (no write
access to the folder) it is written in place. If the server's copy
changed since it was opened, nothing is uploaded until "Overwrite".

**Verified** (Windows 11, daas Ubuntu container over the network, and
Windows' own `sftp-server.exe` over pipes in unit tests): listing a
3000-entry folder in 0.34 s; 32 MB up 10–11 MB/s, down at the link's
1.2 MB/s (`sftp.exe`: the same); a connection killed mid-upload fails
at once, later calls too; missing file, unwritable local path, directory
as file, rename without and with replace, nothing left open; a GBK-named
file listed as 中文.txt, downloaded, renamed and back; a link to /tmp
listed as a link, opened as a folder; folders up and down with an empty
folder and CJK names, progress adding up; an edited file uploaded on save
by replacing (no temporary file left), a change on the server holding
the upload until Overwrite. In the real window, driven through UI
Automation in a test instance (`NATIVETERM_OPEN_FILES=<alias>[,…]`,
`NATIVETERM_OPEN_FILES_SESSION`, `NATIVETERM_LOCAL_START`, test folders):
the first single-pane version listed, downloaded the GBK file, uploaded
two files through the picker (SHA256 equal), deleted with confirmation,
logged in with a saved password (test credential) and asked for one
without it (Cancel ended the connection at once with ssh's message). The
two-sided window: opened as from a tab whose tmux session sat in
`/tmp/终端目录`, the server's side started there; Upload of a local file
and Download of a server's file with the buttons (both lists refreshed,
the queue and the log showed them); two sessions: a click on the first
local tab selected the first server tab, a click on the second server
tab the second local tab; both sides' rows line up. The trees: opened
from a tab sitting in `/srv/数据`, the server's tree showed `/`, `srv`
and `数据` open with `数据` highlighted and its subfolder read; the local
tree opened down to the local folder. With the real mouse (SendInput,
each point checked to be the test window, the window kept on top while
testing): a local file dragged onto the server's list was uploaded to
the folder shown, a server's file dragged onto the local list was
downloaded, a local file dropped on `incoming` in the server's tree went
to `/srv/incoming`, and a 3 MB file dragged from an Explorer window onto
the server's list was uploaded. With the real keyboard (the IME composes
typed letters, so names went in through the clipboard): New folder took
`新目录 乙` and Enter; F2 renamed `改名前.txt` to `改名后.txt` in place;
the password field (no IME) took the letters as typed, three wrong
passwords ended the connection and Reconnect with the right one
connected. Found and fixed on the way: Enter didn't submit the dialogs
(the field took the focus back every frame, so it never reported losing
it), and after a dialog the list's keys (F2, F5, Delete) stayed dead
until a click, as the closed field kept the keyboard. The tab menu: a
right click on a tmux tab of the portable Terminal and "文件（SFTP）"
opened the window on that host in the tab's folder (`/root`); after
`cd /etc/ssh` in the tab, the same item brought the same window and tab
to `/etc/ssh` without a second connection.

Pause and resume, live against a daas container (a 40 MB random file,
~1.5 MB/s down, ~8 MB/s up): a download paused at 21% left
`大文件.bin.ntpart` at 8,585,216 bytes, not growing; Resume went on from
21% and the file's SHA-256 matched the server's. A download with the
program killed at ~11 MB, then the same file downloaded again after a
restart, started at that point and matched too. An upload paused at 60%
left `大文件.bin.ntpart` on the server beside the untouched old file;
Resume finished it and the server's SHA-256 matched. Pause All / Resume
All; Pause shown as paused 0.19 s after the click (it had waited ~2 s
for the reads in flight); Cancel on a paused upload and a paused download removed the partial
file (and the list no longer showed it); ✕ and Clear Finished emptied the
queue. Tests (with Windows' `sftp-server.exe`): a paused download and a
paused upload go on to the same bytes, a new plan continues an old
partial upload, when a partial file is continued.

**Last folders.** Each host's last folder on both sides is kept in
`state.db`'s per-machine settings (`files.remote:<alias>`, the server's
path as hex since it is bytes; `files.local:<alias>`), written in the
background whenever a listing shows another folder. A new tab's server
side starts in the terminal tab's tmux folder if it has one, else in
the last folder if it is still a folder there (checked on the connect
thread), else home; the local side in the last folder if it still
exists, else Downloads (`NATIVETERM_LOCAL_START` before both, for tests).
The trees bring the folder shown into view, sideways too (a row is as
wide as its indent and name), once each time it changes. Live: after
going to `/srv/深/层` and `dl\子目录` and restarting, both sides opened
there ("connected; folder /srv/深/层") with the local tree scrolled to
`子目录`; with `/srv/深/层` deleted on the server, the next start went to
`/root`.

**Synchronize** (`files_sync.rs`; the server's toolbar, with the local
side at a folder): the local folder shown and the server's folder shown
are compared on a worker (the count so far shown, Cancel), then listed:
a tick, what is done (upload, download, delete local, delete on the
server, same, kept, can't decide), the path, and both sides' size and
time; only what something is done with unless "Show files that are the
same". The direction and "Delete extra files on the target" change the
list at once, without comparing again. The summary counts and weighs
the ticked copies (folders copied whole are counted, not weighed) and
deletions (in red). Start runs the copies as ordinary transfers (one
upload job, one download job: pausable, resumable) and the deletions
as deletes (local ones to the Recycle Bin); with deletions it asks once
more first. Live: `sync-l` against `/srv/同步` (a file the same on both
sides, one newer locally, one only local, a folder only on the server,
a common subfolder with the same file) listed download 只在服务器
(folder), upload 改.txt and 本地新.txt, nothing else; after Start both
sides held the same files with the same times, and comparing again said
they are alike. With an extra file and folder put on the server, "local
is the source" and "Delete extra files" listed two deletions, asked
once more, and deleted both on the server. Tests: what each mode does
with each case, and two trees against Windows' `sftp-server.exe`.

**Not yet:** transfers between two servers.

## Active session tracking

The active session is the selected tab of the foreground Windows Terminal
window, when that tab is one of NativeTerm's SSH sessions:

- `SetWinEventHook` with `EVENT_SYSTEM_FOREGROUND` reports, event-driven,
  when a terminal window becomes the foreground window.
- UIA `ElementSelected` events (subscribed on each terminal window,
  subtree scope) report tab switches within ~10–25 ms, whether the switch
  came from a click, a key binding, `wt focus-tab`, or NativeTerm's own UIA
  select. The events are only a **trigger**: one switch produces a burst
  of 5–7 events, and some of them name the previously selected tab. On
  each burst NativeTerm waits ~50 ms (debounce), then reads which tab has
  `IsSelected` set. It doesn't take the tab from the event sender.
- UIA focus-changed events, filtered to the terminal's process, arrive in
  the same burst and are a second trigger. The focused `TermControl`'s
  name isn't reliable as a tab title: right after a window opens, it can
  still be the profile name.

This is **sticky state**: when the user clicks into NativeTerm's own UI
(sidebar, FAB), the foreground window becomes NativeTerm, and the
active-session pointer is not cleared. Selecting one of the user's own tabs
(e.g. an AI coding session) doesn't make it a command target either; the
pointer keeps the last NativeTerm session. The sidebar and FAB show the active
session's name.

## Sending commands

In order of preference:

1. **Through the shim (preferred).** NativeTerm sends the text over the
   pipe; the shim, attached to the same console as `ssh`, writes it with
   `WriteConsoleInputW`. No tab switch, no focus change; group send is the
   same operation per shim.

   What the sources show:
   - **Console host**: injected records go into the same input buffer the
     user's typing goes into. With VT input mode, records are converted to
     VT text at write time; key-down records with virtual key 0 pass their
     character through unchanged, and surrogate pairs are combined.
   - **Windows OpenSSH**: while connected it enables VT input mode (never
     win32-input-mode) and reads with `ReadConsoleInputW`. It forwards a
     key-down record's character as UTF-8, ignoring virtual key codes,
     modifier state, and repeat counts; key-up records are ignored;
     surrogate pairs are combined. Chinese text and emoji arrive intact.

   Injection rules that follow:
   - one key-down record per UTF-16 unit, with only the character set
     (virtual key 0, scan code 0), no key-up records;
   - Enter is `'\r'`;
   - never a record whose character *and* scan code are both 0 — ssh sends
     it as a NUL byte;
   - only after the session reported "authenticated" (see "Login and
     authentication").

   Confirmed end to end (`tests/send_commands.rs`): a shim with its own
   console, a stand-in ssh that reads key records with
   `ReadConsoleInputW` like Windows OpenSSH; `uptime` and
   `echo 你好 😀` arrived as two lines.

   Implemented (`Core::send_text`):
   - text is sent line by line (`\r` after each; after the last only if
     "Press Enter after the last line" is on); CRLF is folded;
   - only to sessions in the "connected" state with a shim: others are
     listed as not sent (in the test, the session still at its login
     prompt got nothing);
   - every send is appended to `<data dir>\audit\commands-YYYY-MM.log`
     (UTC time, the sessions, the text with line breaks shown as ⏎);
   - UI: "Send…" on each logged-in session card, "Send to several…"
     above the list (all logged-in sessions ticked; more than one target
     asks first), "Send Command…" in the tab menu, and a line on the
     floating button for the active session.
2. **`tmux send-keys`** for persistent sessions (see above).

   Implemented (`native_term_app::tmux_send`, the send dialog): a session
   of a host kept in tmux (`NativeTermPersistent tmux` / `tmux-log`)
   that isn't logged in (disconnected, reconnecting, ended, restored and
   not yet connected, or its tab closed) can be ticked, marked "through
   tmux on the server"; the card's "Send…" and "Send to several…" are on
   for such sessions too. Logged-in sessions still get the text in their
   tab; the others each get one `ssh <alias> <command>` on a thread
   (BatchMode: key or agent login; `RequestTTY=no`,
   `ClearAllForwardings`, `PermitLocalCommand=no`, `RemoteCommand=none`,
   no console window). The command checks the session exists (`tmux
   has-session -t =<name>`, else a marker and exit 3), then one tmux
   invocation types every line literally (`send-keys -t =<name>: -l --
   '<line>'`, POSIX single quotes, so key names, `$`, quotes and
   backticks stay text) with `Enter` where asked. Results come back as
   they arrive: sent, "its tmux session isn't on the server", or ssh's
   last message; sends through tmux are in the audit log as
   `<label> (tmux)`. Tests: the command for several lines (quotes, a
   leading `-`, an empty line), and the quoting through Git's `sh`. Live:
   with the tab disconnected (then after a restart, the session "ended"),
   `echo "it's 经 tmux: $((6*7))" > /tmp/nt-proof` reached the tmux pane
   as typed and the file held `it's 经 tmux: 42`; with the tmux session
   killed on the server, the dialog said it isn't there.
3. **Fallback: focus + synthetic keystrokes.** Select the tab via UIA,
   `SetForegroundWindow`, then `SendInput`. Single target only.

Commands are only ever sent to NativeTerm's own SSH sessions, never to the
user's own tabs.

Windows Terminal also has a built-in `toggleBroadcastInput` action, which
mirrors typing to every pane of one tab. It only works across panes within
a tab, so it doesn't replace group send across tabs, but it's a native
option for a small group opened side by side as panes (see "Panes").


### Forgetting a host key

"Forget Host Key…" on a host removes what `known_hosts` has for its host
name and its alias (as `[name]:port` when the port isn't 22) with
`ssh-keygen -R`, which also finds hashed entries and keeps
`known_hosts.old`. The dialog lists the names first and explains when to
do it (a reinstalled host).

### Changes made outside NativeTerm

`~/.ssh` is watched with `FindFirstChangeNotificationW` (a thread that
sleeps until something changes, no polling). On a change, the config
files' modification times and sizes are compared with the ones the tree
was loaded from; only a real change reloads the tree (ssh writes
`known_hosts` in the same folder on every new host). Checked: a host
appended to a folder file by another program appeared within two
seconds.

### Safeguards (this can touch hundreds of production hosts)

- **Confirmation for group send**: show the target list and the command
  before sending; locked sessions are excluded.
- **Per-folder "no group send"** setting, e.g. for production:
  implemented as `NativeTermNoGroupSend yes` in the folder's `Host
  __nativeterm_folder__` block, a checkbox in the folder's menu. Its
  sessions are left out of "all" on the send line and aren't ticked at
  first in "Send to several…" (they can be ticked by hand, like locked
  ones); a single session can always be sent to.
- **Audit log**: every sent command is appended locally — timestamp in
  ISO 8601 with UTC offset, machine name, session GUIDs and aliases of the
  targets, and the text — so logs from several machines line up.
- **Auth-prompt guard**: no injection into a session that hasn't reported
  "authenticated" (see "Login and authentication"), so text can never be
  typed into a password, host-key, or 2FA prompt.

### Where the command is typed

Never in the terminal — the user types into NativeTerm's own UI:

- **Sidebar**: a single-line input pinned to the bottom of the sidebar.
  Implemented (`send_line.rs`): "To" picks the active session (the
  NativeTerm tab last in front, see "Active session tracking") or "all
  logged-in (N)", which leaves out locked sessions and folders marked
  "No group send" ("N left out", with the reason on hover). Enter sends;
  to more than one it first shows "Send to N sessions? Enter again, or
  Esc" with the names on hover, and nothing is sent until then. The
  result stays under the line ("sent to 2", not logged in / failed by
  name); with nobody to send to, the text stays for another target.
  ↑ / ↓ go through this run's earlier lines (100). Same path as
  "Send…": `Core::send_text`, logged-in sessions only, the audit log.
  Verified locally against two tabs on a container: the active one only
  got the first line, both got the confirmed one (two audit entries), a
  cancelled confirmation sent nothing and wrote nothing, and with the
  folder marked "all" was 0 ("2 left out", "no logged-in session to send
  to", nothing arrived). The line is an ordinary text field: an IME in
  Chinese mode takes the letters, as in the terminal itself.
- **Floating action button**: the entry point while the sidebar is hidden.
- **Optional keyboard shortcut**, off by default: a modifier+key combo
  registered via `RegisterHotKey`. Pick a combo not already bound in
  Windows Terminal, since the terminal is usually the foreground window.

Rejected: a "press Shift twice" trigger. Keyboard input goes only to the
foreground window's process, so while the terminal is focused NativeTerm
never sees those keystrokes. `RegisterHotKey` is the OS's exception, but it
needs a modifier plus a regular key; double-tap Shift would need a
system-wide low-level keyboard hook (`WH_KEYBOARD_LL`), which:

- collides with Chinese IMEs (Microsoft Pinyin, Sogou), where tapping Shift
  toggles Chinese/English input and double taps are common;
- collides with JetBrains IDEs, where double Shift is "Search Everywhere";
- looks like keylogger behavior to security software such as 360.

## Security

- **Named pipe access**: anyone who can talk to the pipe can type into
  every open server session. The pipe is created with an ACL granting only
  the current user, and NativeTerm verifies the connecting process
  (`GetNamedPipeClientProcessId`) is a `nativeterm-shim` from its own
  install directory.
  Implemented (`Core::our_shim`): the check runs before the first message
  is read. The peer's image path must be the shim NativeTerm opens its
  tabs with — compared without case, and, when that differs, through
  both paths' real names, so a short (8.3) name or a link still matches.
  On Unix the channel is a socket at `$XDG_RUNTIME_DIR/nativeterm/app.sock`
  (else `$TMPDIR/nativeterm-<uid>/`, `0700`, the socket `0600`), one
  instance found by connecting first, and `accept` reads the peer's
  credentials (`SO_PEERCRED`; `getpeereid` and `LOCAL_PEERPID` on macOS)
  and drops any other user's connection before the same image check.
  Anything else has its connection closed at once; the program is named
  in the log every time and in a notice the first time, so a loop cannot
  fill the window. Live-tested by having the test program itself say
  hello (`core_portable.rs`).
- **Pipe name per user and logon session**:
  `\\.\pipe\nativeterm-<user SID>-<logon session id>`. With several
  Windows users signed in (or fast user switching), each shim reaches only
  its own user's NativeTerm, and two instances never collide on one name.
- **Askpass helper callers**: any process could start
  `nativeterm-shim` in askpass mode with a forged `WT_SESSION`. A process
  running as the user could read Credential Manager directly anyway, but
  the helper shouldn't make that easier: it answers from Credential
  Manager only when its parent is an `ssh.exe` whose parent is the shim
  registered for that session.
- **Group-send safeguards, audit log, auth-prompt guard**: see above.
- **Keys and agent**: passphrase-protected keys would prompt once per
  session (60 prompts for 60 tabs). NativeTerm checks whether the Windows
  "OpenSSH Authentication Agent" service is running and explains how to
  enable it if not. Windows OpenSSH talks to `\\.\pipe\openssh-ssh-agent`
  unless `SSH_AUTH_SOCK` is set; the shim never changes `SSH_AUTH_SOCK`,
  and the check honors it if the user set one (e.g. for another agent). Key installation: see "Installing public keys".
  - Implemented: Settings → "SSH keys and ssh-agent" shows the service
    state (read through the service manager; NativeTerm never changes
    it), each key pair in `~/.ssh` with whether it has a passphrase
    (`ssh-keygen -y -P ""` succeeds only without one; NativeTerm doesn't
    read private keys itself), and whether the agent holds keys
    (`ssh-add -l`). Only when a key has a passphrase and no agent is
    available does a hint appear at the top (dismissable); it explains
    the administrator command to enable the service (with a copy button).
    "Add my keys to the agent…" opens a tab running `ssh-add`, where the
    passphrases are typed.
- **Agent forwarding**: with `ForwardAgent yes`, every host the user logs
  into can use the user's keys to reach other hosts while the session is
  open. When opening many such hosts at once, NativeTerm points this out:
  after opening three or more hosts, a background check (`ssh -G`, eight
  at a time) counts those whose effective `ForwardAgent` isn't `no`; from
  three on, a notice names them and where to turn it off.
- **Integrity levels**: if Windows Terminal runs elevated and NativeTerm
  does not, Windows blocks UIA control and synthetic input across that
  boundary. NativeTerm detects this and tells the user instead of failing
  silently.

## Robustness

- **Restart/crash recovery**: on startup NativeTerm re-discovers existing
  tabs via UIA, re-pairs them with shims as those reconnect to the pipe,
  and (for persistent hosts) offers to reopen detached tmux sessions.
- **Windows Terminal's "restore previous session" setting**: restored
  panes keep only the profile and the session GUID (verified in 1.26, see
  "Restored tabs and named windows"), so they start the profile's command
  line — the shim without a host — with their original `WT_SESSION`.
  Nothing reconnects on its own (dozens of simultaneous logins at startup
  are rarely wanted): NativeTerm recognizes the GUIDs from its registry,
  offers to reopen those sessions (all, some, or none) and closes the
  placeholders it replaces. Without NativeTerm the placeholder becomes a
  local shell.
- **`wt` argument handling**: see channel 1 above — the only command passed
  through `wt` is `nativeterm-shim <host-alias>`; `;` in titles is
  written `\;`. Command construction is one function with
  dedicated tests.
- **Opening many tabs at once** (a whole folder): each `wt` invocation
  costs a few hundred ms, so several tabs are chained into one invocation
  with `;`. Connections are rate-limited, since dozens of simultaneous
  logins through one jump host can hit sshd's `MaxStartups`. Each new tab
  becomes the selected one, so a batch keeps switching the visible tab;
  when the batch finishes, NativeTerm selects the tab the user chose (by
  default the first of the batch).
- **UIA cost**: enumerating 63 tabs through `ItemContainerPattern` took
  26 ms, walking the tree 140–170 ms, and a lookup while 60 tabs were
  still opening took 1.5 s. NativeTerm keeps a cached tab list updated by
  events and refreshes it on demand, never in a tight polling loop.
- **Single instance**: the pipe is bound before the window opens. If
  another NativeTerm owns it, the new copy brings that one's window to
  the front and exits (quietly when started by a restored tab).
- **Dependency check**: on startup, verify Windows Terminal is installed and
  new enough for `-w`, `--sessionId`, and `--suppressApplicationTitle`,
  and record the `ssh -V` version (behavior here was reviewed for Windows
  OpenSSH 9.5; newer versions were spot-checked, other clients such as a
  Git-bundled `ssh` earlier on `PATH` are flagged);
  report clearly otherwise. `wt.exe` is an app execution alias that users
  can turn off in Windows settings, so Windows Terminal is located through
  its installed package when the alias is missing, and unpackaged or
  portable copies (which have no alias) are found by path — scoop's
  `windows-terminal`, a user-chosen folder, or the bundled fallback.
- **Absolute shim path, and a moved program folder**: the `wt` command
  line and the fragment's profile contain the shim's full path. On
  startup NativeTerm compares the fragment's path with its own location
  and rewrites the fragment if the folder moved. Terminal's saved layouts
  store only the profile GUID, not our command line (verified on 1.26),
  so restored tabs pick up the rewritten path. The exception is a layout
  restored before NativeTerm has run and fixed the fragment: those tabs
  fail to launch and are reopened from the registry.
- **Multi-monitor / DPI**: per-monitor DPI awareness (PerMonitorV2 in the
  manifest); sidebar docking and FAB position computed per monitor.
- **Sessions whose tabs are gone.** At start, a session from `state.db`
  whose helper process is no longer running had its tab taken with it:
  NativeTerm marks the record closed-with-window (Terminal may restore
  the pane) and used to say nothing else. It now offers them back, as a
  line above the session list with "Open them again" and "Leave them".
  The same offer covers sessions whose helpers never connect within the
  grace period. When the fragment had to be put right at start, the line
  says why: the program folder moved, so the tabs Terminal restored ran
  a helper that is not there any more. Opening them again marks the old
  records as no longer restorable, so a pane Terminal brings back later
  becomes a local shell rather than a second copy of the session.
- **Resume from sleep**: many sessions reconnect at once; reconnects go
  through the same queue and rate limit as opening a folder, to avoid
  hammering jump hosts.
  - Implemented (`connect_queue.rs`): one worker takes session ids in
    order. A session is connected once its shim is linked and it can
    connect (sessions that close, vanish or already connect are dropped;
    a shim that doesn't turn up within 30 s is skipped). At most four
    sessions are logging in at a time (a slow login frees its slot after
    20 s), and starts are 200 ms apart. Opening more than three hosts
    starts the tabs in `--wait` mode and queues them; "Connect all" and
    automatic reconnects queue too. When a batch of tabs has been opened,
    its first tab is selected. Live test: six hosts through the queue
    (1.5 s), then "Connect all" on them.
  - **A `Connect` can be lost.** A shim whose pipe connection breaks
    reconnects and replays the state it last reported, so a session that
    was told to connect can come back saying "waiting" — the message went
    down with the link. Two things make sure it is told again: the queue
    puts a session back in its place when the link is gone or the send
    fails between choosing it and telling it, and a "waiting" that
    arrives while NativeTerm believes the session is connecting puts it
    back in the queue. Without this, one tab in six hung in "waiting"
    about half the time when six were opened at once.
- **Elevation decides which Terminal instance a tab joins** (verified,
  2026-09-17):
  - `wt` started from an elevated process joins (or starts) the
    *elevated* instance of that install, a separate process with its own
    windows, not the user's normal window.
  - In that instance, tab names and pane HelpText carry the console's
    "管理员: " (Administrator: ) prefix.
  So NativeTerm runs non-elevated. If it is started elevated anyway, it
  launches `wt` with the user's normal token: through the desktop shell's
  `IShellDispatch2::ShellExecute` (Shell windows → desktop →
  `SID_STopLevelBrowser` → active shell view → `GetItemObject` as
  `IDispatch` → `IShellFolderViewDual::Application`). Explorer then
  starts `wt`, unelevated. Asking `GetItemObject` for
  `IShellFolderViewDual` directly fails with `E_NOINTERFACE`. It first
  calls `AllowSetForegroundWindow(ASFW_ANY)`, since Explorer isn't the
  foreground app. Verified from a non-elevated process with the path
  forced (30 tabs, two batches); the prototype had used
  `explorer.exe <script>`. Label matching strips
  a localized administrator prefix. The same applies to its own test
  tooling.
- **Multiple Windows Terminal installs** (Store, Preview, unpackaged or
  portable ZIP): which one `wt.exe` launches is not guaranteed. NativeTerm
  detects installed variants and lets the user pick one; the choice is kept
  in settings. Per the Terminal source (`WindowEmperor.cpp`), each variant
  is a separate single-instance application: the handoff mutex / message
  window name contains the branding, " Admin" when elevated, the user's
  SID hash, and — for unpackaged builds — a hash of the full
  `WindowsTerminal.exe` path. Therefore:
  - NativeTerm always calls the chosen variant's own `wt.exe` by absolute
    path; `-w 0` then only reaches that variant's windows.
  - All variants' top-level windows share the class
    `CASCADIA_HOSTING_WINDOW_CLASS`, so UIA window lists are filtered by
    the owning process's image path.
  - Moving a portable Terminal folder makes it a new instance (new hash).
- **Start checks on the chosen Terminal** (`install.rs`): its version is
  read from the package full name
  (`Microsoft.WindowsTerminal_1.24.11911.0_x64__8wekyb3d8bbwe`) or, for an
  unpackaged or portable folder, from `WindowsTerminal.exe`'s version
  resource (`GetFileVersionInfoW`). NativeTerm needs **1.21 or newer**:
  that is where `--sessionId` arrived (microsoft/terminal#16598, "Implement
  buffer restore", first released in 1.21), and without it a new tab cannot
  be told from any other. An older one is named at start and in the
  wizard rather than silently misbehaving; a version that cannot be read
  is given the benefit of the doubt.
  A packaged install is normally started through its `wt.exe` app
  execution alias, which the user can turn off (Settings → Apps → Advanced
  app settings → App execution aliases) and which some "debloat" scripts
  remove. The alias only points at the `wt.exe` **inside the package**,
  and that `wt.exe` is a shim that runs the `WindowsTerminal.exe` next to
  it (`terminal/src/cascadia/wt/shim.cpp`), so the copy the package info
  leads to is used instead; `wt.exe` on the PATH is the last resort. The
  launcher is asked for at each launch, never cached, since the alias can
  go at any time.
- **Different permission levels** (`WindowsTerminal::mismatch`): Windows
  keeps NativeTerm and Terminal apart when they don't run at the same
  level, and neither side can work around it, so it is said plainly.
  NativeTerm elevated and Terminal not: a normal tab's shim cannot write
  to a pipe an elevated process created (mandatory integrity, no-write-up),
  so it never connects, and nothing can be dropped on NativeTerm's window
  (UIPI). Terminal elevated and NativeTerm not: its windows are listed but
  can be neither read through UIA nor sent anything. A Terminal process of
  this user whose token cannot even be opened stands above this one, which
  counts as the same mismatch.
- **Portable Windows Terminal** (unpackaged, `.portable` marker file next
  to `WindowsTerminal.exe`): settings live in its `settings\` folder, but
  fragments are still read from `%LOCALAPPDATA%\Microsoft\Windows
  Terminal\Fragments` (not redirected in portable mode), so NativeTerm's
  fragment works for it too — it just isn't carried along with the
  portable folder. There's no app execution alias, and portable mode
  can't be the Windows default terminal.
- **Localized `ssh` errors look garbled**: Windows OpenSSH escapes
  non-ASCII bytes of system messages, so on a Chinese system "could not
  resolve hostname" is followed by `\262\273\326…` (the GBK text in
  octal). This is `ssh`'s own output; NativeTerm's lines are unaffected.
- **Changed host keys**: after a server is rebuilt, `ssh` refuses to
  connect. NativeTerm can't read the terminal output to detect this, so it
  offers "Remove this host's old key" (`ssh-keygen -R <host>`, confirmed by
  the user) on sessions that failed to connect.
- **Panes**: Windows Terminal can split a tab into panes, but NativeTerm's
  model is one session per tab. Panes the user creates are treated like
  the user's own tabs. Verified (1.24): a split tab's UIA name is the
  **focused pane's** title. When the user split `nt-menu-B`
  (`--title … --suppressApplicationTitle`) and focus moved to a new
  pane, the tab showed "命令提示符". All three panes' `TermControl`
  elements (present only for the selected tab) were also named
  "命令提示符", the NativeTerm pane included, because `--title` sets the
  tab title, not the pane's terminal title. So while a foreign pane has
  focus, neither the tab nor the pane can be recognized by title; this
  is one reason tab identity doesn't rely on titles alone. Side-by-side session views (e.g. a small group opened as panes
  with `wt split-pane`, combined with Windows Terminal's built-in
  `toggleBroadcastInput`) would need their own design.
- **A hung Windows Terminal** (observed with 1.24.11911.0 on 2026-09-17:
  after splitting a tab and tearing a tab out into a new window, the UI
  thread spun at 100 % CPU and every window of that process stopped
  responding; no NativeTerm prototype was running). All windows share
  one process, so the user's AI sessions hang with it, and "Close
  program" ends all of them. NativeTerm must:
  - never block on it: a `WM_NULL` probe gates every UIA call, and UIA
    runs on watchdog-guarded workers. UIA timeouts and `IsHungAppWindow`
    turned out not to be enough (see "Restored tabs and named windows");
  - disable the menu hook's interception for hung windows;
  - after the user restarts Terminal, offer to reopen the sessions that
    were in it;
  - never offer to kill Terminal itself.
- **Large lists**: the sidebar renders only visible rows
  (`egui::ScrollArea::show_rows`), not every host every frame — sized for
  thousands of sessions.

## Tab switcher

With many tabs, the tab strip is too cramped to find anything: once the
window is narrow, each tab shrinks to its icon plus a letter or two. The
tab switcher is a scrollable list of **all** tabs in all Windows Terminal
windows — NativeTerm's SSH sessions and the user's own tabs (local
shells, AI coding sessions) alike — with their full, live titles. Clicking
an entry only switches to that tab; for the user's own tabs that is all
NativeTerm ever does.

- Each entry shows the title (for SSH sessions, the label and session
  state) and, optionally, a preview:
  - a **last-seen text preview** with its capture time: the terminal
    control implements UIA `TextPattern`, so the visible text of the
    selected tab can be read once when it is switched away from (and when
    the switcher opens) — cheaper than an image and searchable.
    Implemented (`uia::screen_text`, kept in `previews` beside the
    picture): the visible ranges only, so the screen without the
    scrollback, the last 30 lines, read on the same events as the
    picture. It is what a tab NativeTerm doesn't run can be searched by;
    its own tabs answer with their console text instead (`Core::screen`),
    which is also what a tmux session shows. A hit in the text scores a
    quarter of a hit in the name, so names still come first; or
  - a **last-seen snapshot** image (`PrintWindow` with
    `PW_RENDERFULLCONTENT` while that tab is selected; kept in memory only,
    a few hundred KB each); or
  - for persistent sessions, **current screen text** via
    `tmux capture-pane`.
  No live image thumbnails: Windows Terminal only renders the selected tab.
  Nor are they needed: previews are snapshots, taken only on events (a
  tab switched away from, the switcher opening), never on a timer — so
  there is nothing to throttle on battery (decided 2026-09-19).
- A search box filters by title; clicking an entry selects that tab via
  UIA and brings its window to the front.
- Opened from the sidebar, the FAB, or the optional shortcut.

Windows Terminal's own tab search (`tabSearch` action) also lists tabs by
title and benefits from the same stable labels.

### Pictures (implemented)

"All Tabs" shows the tabs as a list or as pictures (a switch next to its
search box, kept in `state.db` as `tabs.view`). In the list, hovering a
tab shows its picture. The pictures view shows each window's tabs as
cards: the picture, the name with the session's state, and "as of HH:MM".
A tab never seen selected shows a note instead ("select it once").

- **Taking them** (`previews.rs`, `windows_terminal::capture`): after a
  scan that followed one of Terminal's notifications (a tab or window
  switched, opened, closed, moved to the front) or the tab list opening,
  the selected tab of every Terminal window of the chosen install is
  pictured: `PrintWindow` with `PW_RENDERFULLCONTENT` (works while the
  window is covered, not minimized), the visible frame only
  (`DWMWA_EXTENDED_FRAME_BOUNDS`) below the tab strip (the tab items'
  bottom), scaled with `HALFTONE` to 360 px wide, read with `GetDIBits`.
  A tab pictured under the same title in the last 3 s isn't pictured
  again. The 60 s fallback scan and the list's title rescan (every 5 s,
  only while NativeTerm has the focus) take no pictures: no timer ever
  does.
- **Kept** in memory only, by window and tab position, with the title
  then and the time; forgotten when the window or tab is gone, or when
  the window's tab count changed and the title at that position differs
  (the tabs moved). About 360×200×4 bytes a tab.
- The list no longer repaints every 5 s while NativeTerm is in the
  background: Terminal's notifications repaint it.
- Verified: two tmux tabs with different output in the portable Terminal;
  the pictures view showed each tab's own screen, the selected one
  marked "current tab"; selecting the other tab from its card updated
  its picture to its newer output; the view choice survived a restart.
  Idle with the pictures view open and Terminal in front: 47 ms CPU in
  130 s (the two fallback scans, no pictures).

### Taking over Ctrl+Tab (source review, Terminal 1.26, 2026-09-18)

Terminal's Ctrl+Tab list is the command palette in "tab switch" mode
(`defaults.json`: `ctrl+tab` → `Terminal.NextTab`, `nextTab`/`prevTab`
with the global `tabSwitcherMode` of `inOrder` / `mru` / `disabled`;
`CommandPalette.cpp`). It shows title, icon and status glyphs only, its
"preview" is a real tab switch, and it commits when no modifier is held
any more (`_anchorKeyUpHandler`, polled on every key-up). What the source
rules out:

- **No extension point.** No action can call another program: the only
  ways out of the process are launching a pane's command line
  (`newTab`/`splitPane`), `ShellExecute` through `searchWeb`'s
  `queryUrl`, writing a file (`exportBuffer`), or `sendInput` into the
  pane's own connection — which for NativeTerm means ssh types it to the
  remote host, so it is useless here.
- **Fragments can't bind keys** (`CascadiaSettingsSerialization.cpp`:
  "fragments shouldn't be allowed to bind actions to keys directly"), so
  a rebind would mean editing the user's own `settings.json`.
- **Unbinding Ctrl+Tab doesn't help either.** It would then reach the
  process in the tab (ConPTY forces win32-input-mode, so a real
  `VK_TAB` + Ctrl record arrives), but while a session is connected ssh
  owns the console input, not the shim — the key would go to the remote
  host.

So the way to take it over is NativeTerm's existing low-level keyboard
hook (installed only while it has located tabs, already used for the tab
menu): swallow Ctrl+Tab while the foreground window is a Terminal window
with NativeTerm tabs and the setting is on, show NativeTerm's own
switcher without taking the focus, follow further Tab presses, and commit
when Ctrl is released — the same semantics Terminal uses. Terminal's own
list still appears when NativeTerm isn't running or the setting is off.
Committing selects the tab through UIA (`wt -w <id> focus-tab -t <n>`
would also work).

Implemented in `windows_terminal/switcher.rs`, with the menu's own
machinery (`menu.rs`):

- The setting (`tabs.switcher`, off by default) is pushed to the menu
  thread as a flag, because the hook may not read settings, take a lock
  or do anything else slow. On a Tab key-down it tests that flag, that
  Ctrl is down and Alt and the Windows key are not, and that the
  foreground window is one of at most eight window handles kept in
  atomics as the tabs are scanned. That is all: one `GetForegroundWindow`
  and a few comparisons.
- The grid itself is a `WS_EX_NOACTIVATE | WS_EX_TOPMOST` popup of the
  same class as the tab menu, drawn with Direct2D over the middle of the
  Terminal window (kept on its monitor, and no bigger than 5 × 4 tiles).
  Terminal keeps the focus the whole time and never learns about any of
  it.
- While it is up the hook swallows Tab, the arrows, Enter, Space and Esc
  and hands them to the menu thread; Shift passes through (it belongs to
  Ctrl+Shift+Tab); Ctrl's release passes through as well — Terminal saw
  it go down — and also commits. Any other key, or a click outside,
  leaves the tabs as they are.
- Tiles come from what NativeTerm already keeps: the picture of a tab
  from when it was last seen selected (`previews`), or else the text of
  its own console (its sessions) or the screen read when it was last
  pictured. A tab neither pictured nor read shows its name only, and is
  asked for its screen there and then, so the next grid has it.
- The one thing NativeTerm does to Terminal is select the tab at the end,
  by index and title, through UIA — the same call the tab list uses.

Thumbnails can only come from captures taken while a tab was selected:
a non-selected tab's content is unparented from the XAML tree, its UIA
peer is unreachable and its UIA renderer is disabled, so neither its text
nor an image is obtainable from outside (Terminal itself has no tab
thumbnails anywhere — no `RenderTargetBitmap`, no DWM tab thumbnails, and
its tab tooltip is text only). Measured here: `PrintWindow` with
`PW_RENDERFULLCONTENT` captures a Terminal window even while it is fully
covered by another window (25 ms for 1752×936), and a capture scaled to
320 px costs ~21 ms with GDI+ and ~214 KB per thumbnail. So the switcher
would capture each window's current tab when it opens, keep the snapshot
of a tab when it is switched away, and show title and state for tabs it
has never seen.

### Controlling Terminal from outside (source review, 1.26)

The Monarch/Peasant COM architecture was removed in late 2024 ("Remove
Monarch/Peasant & Make UI single-threaded"); `src/cascadia/Remoting` is
empty. Terminal is now one process, and a second `wt.exe` hands its
command line to the running instance with `WM_COPYDATA` after finding its
message window by a class name that hashes the user's SID (and the
install path when unpackaged). Consequences:

- **No COM and no IPC lists or reads tabs.** The only tab operation from
  outside is the write-only `wt -w <id> focus-tab -t <n>`. Reading tab
  titles and the selection stays UIA's job, as NativeTerm already does.
- **Terminal's `WM_USER` messages are in-process only**; `WM_GET_WINDOW_LIST`
  passes a raw pointer through `LPARAM`, so sending it from another
  process would write into Terminal's address space. Never do that.
- `ITerminalHandoff3` is the one COM class still registered, and it only
  accepts an inbound ConPTY session (defterm), so it is no use here.
- Spawning `wt.exe` stays the right way to open tabs.

Implemented (first version, no previews yet): "All tabs" next to "Open
sessions" (or Ctrl+T, which also focuses its search box).

- Every tab of the chosen Terminal install, grouped by window (the same
  window numbers as in the session list), in strip order; the user's own
  tabs with a page icon, NativeTerm's with the host icon, a state dot, the
  session label (and the live title when it differs) and the state. The
  selected tab of each window is marked, and highlighted when its window
  is in front.
- Typing filters and ranks by title and label (the same fuzzy match as
  the tree); Enter switches to the best match.
- Switching selects the tab by index and title (or by title in the same
  window, if it moved) and brings its window to the front.
- While the list is shown, the core scans every tab even when NativeTerm
  has no sessions of its own, and rescans every 5 s while NativeTerm has
  the focus (titles change without a notification). Hidden, nothing extra
  runs.

## Finding sessions: search, favorites, recent

With ~800 sessions, browsing the tree is the slow path.

- **Search** matches label, alias, `HostName`/IP, user, folder, and note.
  Chinese labels and folder names also match by pinyin, initials or
  full (`kzjd` or `kongzhi` for `控制节点`), using the `pinyin` crate;
  the pinyin forms are computed once per tree generation.
- **Favorites**: `NativeTermFavorite yes` in the host block (so they sync
  with the sessions); shown in a Favorites section at the top of the
  sidebar (with a star after the label), toggled from the host's context
  menu, and later optionally exported as Windows Terminal profiles (see "Appearance").
- **Recent**: the most recently opened sessions, kept per machine in the
  data directory.

## Command library and post-login commands

- **Command library**: named commands (imported from SecureCRT's saved
  commands / button bar, or created in NativeTerm), stored in
  `commands.toml` in the data directory and synced with the settings.
  Each can be sent to the active session or a group, with the usual
  safeguards (confirmation, audit log, never before login).
- Implemented: `commands.toml` (`[[command]]` with `name`, `text`,
  optional `enter = false` and `group`), edited from the send dialog
  (pick, save as, delete). A file that can't be parsed is reported and
  never overwritten.
- **SecureCRT's button bars** (implemented, `native_term_config::button_bar`,
  app `commands_import.rs`): "Import from SecureCRT" also reads
  `ButtonBarV5.ini` (or `ButtonBarV4.ini`) next to `Sessions`. Its format
  is the one VanDyke's tip "How to Import a Single Button Bar" shows: a
  bar as `Z:"<name>"=<count in hex>`, then a line per button with a
  leading space — function, argument, label and four more fields (read
  from both ends, so a command may hold commas), backslashes doubled.
  Only `SEND` buttons become commands, named after the label and grouped
  by the bar; the send string's `\r` / `\n` end lines (a trailing one:
  Enter after the last line, else the last line is left to finish), `\\`,
  `\t` and octal codes are kept. Buttons using what SecureCRT fills in or
  does itself (`\p` pause, `\u` user name, `\v` clipboard, local
  addresses, …) and other functions (menu functions, scripts, programs)
  are listed in the preview with the reason, not imported: an enable
  password behind a pause isn't something to copy blindly. The merge
  leaves out a command the library has (same name and text) and gives a
  different one with a taken name "<label> (<bar>)". Verified with
  VanDyke's own example and a made-up config; not yet against a real
  SecureCRT folder. `NATIVETERM_SECURECRT_CONFIG` points the dialog at a
  test folder. The Command Manager's storage isn't documented; it isn't
  read.
- **Post-login commands**: `NativeTermOnLogin <command>` on a host or
  folder is sent once, via shim injection, right after the
  "authenticated" signal. The text waits in the input buffer until `ssh`
  starts reading, and the server's pty holds it until the remote shell
  reads it — like typing ahead. This replaces the simple kind of
  SecureCRT logon action (e.g. `sudo -i`, `cd /srv/app`). Conditional
  scripts are out of reach (see "Known limitations").
  - Implemented: "After login" in the host dialog (one line, stored as
    `NativeTermOnLogin`; a folder's `Host __nativeterm_folder__` default
    applies too). The core types it when a session changes to
    "connected" — after every login, reconnects included — through the
    same path as sent commands (audit log). Clones keep it. Tested end to
    end: typed after the first login and again after a reconnect.

## First run

A short wizard, each step skippable:

1. Import from SecureCRT (found automatically if installed), with the
   summary described in "Import from SecureCRT".
2. Check the ssh client (`ssh -V`), Windows Terminal, and the
   `ssh-agent` service.
3. Offer to create a key and install it on selected hosts or folders
   ("Install my key").
4. Choose the data directory and, optionally, cloud sync.

Implemented (`wizard.rs`): step 4 shows where sessions and NativeTerm's
data are, with "Open" buttons, and a field to move the data folder (the
same copy and pointer as Settings; not offered when `--data-dir` or the
environment chose it). "Sessions on several computers" is the zero-tool
sync from "Cloud sync": the OneDrive (personal and work, from the
variables its client sets) and Dropbox folders found are offered as the
place for the session folders (`<root>\NativeTerm\ssh-folders`), moved
there like "Move session folders"; on the next computer, where that
folder already holds folder files, the button uses them instead
(`Editor::adopt_folders`: only the `Include` line changes, nothing is
copied, ssh must accept it; this computer's own folders stay where they
were). Settings → "Move session folders" does the same for any folder
that already holds session folders. Folders already inside a sync root
are shown as synced. Private keys are never synced; rclone sync for
other storage is a later item. The move and adopt paths are tested in
scratch folders (the real OneDrive is never written by tests). The wizard opens once, until finished or skipped
(`first_run_done` in `state.db`), and again from Settings. Its buttons
only open the existing dialogs (import, install my key) or tabs (key
creation); while such a dialog is open the wizard waits behind it.

## Keyboard shortcuts

- Every command has an optional shortcut; defaults are few and editable.
- Defaults avoid keys missing on some keyboards (e.g. Insert, Pause,
  Scroll Lock, right-hand modifiers) — the author uses a MacBook keyboard
  under Boot Camp.
- Global shortcuts (`RegisterHotKey`) are off by default and must not
  collide with Windows Terminal's own key bindings.

Implemented (`native_term_app::shortcuts`, `native_term_win::hotkey`,
`shortcut_ui.rs`):

- **Commands**: show NativeTerm and search hosts, search hosts, all
  tabs, send a command to the active session, send to several sessions,
  files (SFTP) of the active session. Each may have a shortcut in
  NativeTerm's window and a global one. Defaults: Ctrl+F and Ctrl+T in
  the window, nothing global. The files window's keys (F5, F2, Delete,
  Backspace, Enter) and text fields' (Enter, Esc) stay as they are.
- **Combinations** are written as Windows Terminal writes them
  (`ctrl+alt+f`, `ctrl+shift+comma`), so they compare with its bindings
  as they are; kept in `state.db` (`shortcuts`) as the changes from the
  defaults only, so a new default reaches whoever never changed that one.
- **Checks**: the same combination for two commands; a global one
  without Ctrl or Alt, or one Windows keeps (Alt+F4, Alt+Tab, Alt+Esc,
  Alt+Space, Ctrl+Esc), isn't registered; one in the window without Ctrl
  or Alt (F1–F24 aside) isn't caught, as it would fire while typing in a
  field; a global one Windows Terminal uses (its default bindings, and
  every `keys` in its `settings.json`'s `actions` and `keybindings`, both
  formats) is registered with a warning, since it takes the key from the
  Terminal; one another program holds is reported with Windows' reason.
- **Global shortcuts**: `RegisterHotKey` with no window on a thread of
  its own (`WM_HOTKEY` in its queue; no keyboard hook), `MOD_NOREPEAT`;
  the whole set is replaced on each change. A press runs the command:
  NativeTerm's window comes out where it shows something; "active
  session" is the selected tab of the Terminal window last in front.
- **Settings**: "Keyboard Shortcuts" lists each command with its two
  shortcuts; a click records the next combination pressed (Esc cancels),
  ✕ clears, "Restore Defaults"; problems are shown under each.
- Verified: unit tests (combinations both ways, egui keys, the saved
  text, Windows Terminal's bindings from a settings.json, the checks) and
  a registration test (registered, refused while another holds it, free
  after). Live: Ctrl+Alt+F recorded as the global shortcut of "Files of
  the active session"; pressed while the portable Terminal was in front
  with a tmux tab in `/srv/热键`, it opened the files window, in front,
  at `/srv/热键`. Ctrl+Shift+D recorded as another global shortcut was
  shown as Windows Terminal's `duplicateTab`. Ctrl+T in the window still
  opened "All tabs" with its search focused.

## Resource budget

NativeTerm and its shims must stay cheap enough to leave running all day
on a laptop:

- **Idle NativeTerm**: no repaint unless something changes (egui's
  on-demand repaint), event-driven UIA and foreground tracking, no polling
  loops; target idle CPU ≈ 0 and memory in the tens of MB.
  - **Measured (release build):** idle CPU 0 ms over 10 s, and the
    window isn't repainted. Private memory is **20 MB** at start, ~28 MB
    after use (glyph caches), and no GPU memory. With the earlier GPU
    renderer it was 74 MB, almost all of it the graphics driver (see
    "Renderer").
  - **With open sessions** (implemented, measured in release with three
    sessions, idle for a minute): NativeTerm used 16 ms of CPU and the
    shim 0 ms.
    - **What NativeTerm listens to.** Win32 WinEvents report Terminal
      windows being shown, hidden or destroyed. These are out of context,
      so a hung Terminal can't block them. UIA events on each responsive
      Terminal window report a tab selected or the tree changed (tabs
      opened, closed, moved). The UIA registrations run on their own
      thread, which is replaced if a registration hangs for more than
      5 s.
    - **Bursts.** One opened tab produced dozens of events, so NativeTerm
      waits 150 ms after the first and then scans once. A selection
      behind NativeTerm's back was reflected after ≈ 200 ms. Ten seconds
      of `ping` output in a tab produced no events at all.
    - **Not watched: title changes.** AI tools and shells retitle all the
      time. A rename is picked up by a fallback scan every 60 s, which
      costs ≈ 70 ms (release, one window).
    - **No polling anywhere else either.** The pipe uses overlapped I/O:
      a reader waits on an event, and writes from other threads aren't
      blocked by it. Before, readers polled `PeekNamedPipe` every 20 ms.
      That cost 266 ms of CPU per minute with three sessions, and 47 ms
      per 30 s in each shim.
    - **What the shim waits on.** The ssh process, a "message arrived"
      event, the login event, and console input, with
      `WaitForMultipleObjects`. Its link reads in a blocking thread, and
      closing the link cancels that read.
- **Shim**: a small native Rust binary with no runtime: ≈ 1 MB private
  memory, 6.6 MB working set per tab (release, waiting at its prompt), on
  top of `ssh.exe` and the console host.
- Nothing runs on a timer while idle: measured 0 ms CPU over 60 s with
  the window open and 60 s minimized (debug build, no sessions). Features
  that could (previews, sync) work from events instead: snapshots are
  taken when a tab is switched away from or the switcher opens, and sync
  runs when asked. A future feature that needs a timer is off by default
  and pauses on battery power and while minimized.

## Sidebar auto-hide / pin (QQ-style drawer)

The session-tree sidebar can behave like the classic Windows QQ panel or
the taskbar's auto-hide mode: docked to a screen edge, collapsed to a thin
sliver, sliding out on hover and retracting when the pointer leaves, with a
pin toggle to keep it permanently visible.

- **Collapsed state**: a visible sliver of our own window flush against the
  screen edge, so hover detection is the normal window-local pointer-enter
  event — no global hooks.
- **Slide animation**: moving the window via `ViewportCommand` (usually
  smoother than resizing it every frame; to be verified).
- **Auto-retract**: once the pointer leaves, a short delay before
  collapsing absorbs accidental brush-past hovers.
- **Pin**: one boolean; pinned skips auto-collapse.
- **Stay-on-top while expanded**, so the drawer can overlay the maximized
  terminal window.

Implemented (first version), on NativeTerm's main window:

- **Docking:** a move that ends with the window's visible frame within
  12 px of the work area's top, left or right edge docks it there (top
  wins in a corner). The frame is aligned to the edge using DWM's frame
  bounds, so the invisible resize borders don't leave a gap. An edge
  with another monitor behind it is not used (the hidden window would
  show there). A move is "ended" once no mouse button is held (checked
  every 120 ms after the last move). Dragging the window away undocks
  it. Maximized windows don't dock.
- **Hiding:** 450 ms after the pointer leaves, the window slides (160 ms,
  eased) until only a 4 px strip is on screen. It stays out while the
  pointer is over its frame, a mouse button is held, a move is being
  settled, or it has keyboard focus with a text field active. Touching
  the strip, or activating the window (Alt+Tab), slides it back.
- **On top:** docked windows are "always on top" (set through winit, which
  otherwise resets the level itself), so the strip isn't covered.
- **Pin:** a "Pin" toggle appears in the top bar while docked; pinned, the
  window never hides. Kept in `state.db` (`settings.dock_pinned`).
- **Remembered:** the window's position, size and edge
  (`settings.window`); after a restart it docks again. A saved position
  that is no longer on a monitor is ignored.
- **No polling while idle:** leaving the client area is reported by the
  window; only a pointer over the title bar or borders is checked
  (every 450 ms). Measured: 0 ms CPU over 10 s docked with the pointer
  inside, docked and hidden, and undocked.
- **Where:** `src/dock.rs` (geometry, slides, unit tests),
  `src/window.rs` (events and timers), `native_term_win::dock` (work
  area, frame bounds, cursor, buttons).
- **Not yet:** the floating action button; hiding into a strip on the
  bottom edge (the taskbar is usually there).

## Floating action button (FAB)

While the sidebar is hidden, frequently used actions are reachable from a
small draggable floating button, bottom-right by default.

- **Visibility**: shown only when the sidebar is not visible.
- **Speed-dial actions**: send command to the active session (primary),
  quick connect (type to search hosts), tab switcher, reconnect / clone the
  active session, close disconnected sessions, open the sidebar.
- **Target display**: actions on the active session show its name.
- **Drag vs. click**: a press that moves more than a few pixels is a drag.
- **Position**: remembered; clamped to the visible work area on startup.
- **Implementation**: a second window of NativeTerm's own runner
  (`src/window.rs`, one window today) — borderless, always-on-top —
  dragged via `ViewportCommand::StartDrag`. Transparency needs a layered
  window with the CPU renderer (per-pixel alpha from the rendered
  buffer).

Implemented (first version):

- **Windows:** the runner (`src/window.rs`) now drives two windows, each
  with its own egui context, renderer and repaint timer: the main window
  and the button (borderless, always on top, no taskbar entry, rounded
  corners through DWM, 52 × 52 points, the accent color with an icon).
- **Visibility:** shown exactly while the docked main window is hidden
  (and while its panel is open). It is created hidden at start and
  painted once, so it appears without a flash.
- **Position:** bottom right of the work area at first; dragged anywhere
  (a press that egui sees as a drag hands the move to Windows); the
  collapsed position is kept in `settings.fab`.
- **Panel:** a click opens a panel that grows up and to the left from the
  button (kept on the screen) and fits its height to the content:
  - a field for a host (fuzzy search over the saved hosts, which the main
    window publishes on every reload) or `user@host[:port]`; Enter opens
    the best host, else the typed target;
  - the active session — NativeTerm's session in the selected tab of the
    Terminal window that was last in front (the core records it on
    foreground changes) — with Reconnect and Clone;
  - All tabs (brings the main window out with the tab list), Close
    disconnected tabs, Show NativeTerm.
  - Esc, a click elsewhere, or any action closes the panel.
- **Bringing the main window out** from the button slides it back and
  keeps it out until the pointer has been over it or it loses the focus
  (otherwise it would hide at once: the pointer is still at the button).
- **Its own shape** (`native_term_win::layered`): the button's window is
  a layered one (`WS_EX_LAYERED`), and its frame is handed to Windows
  whole with `UpdateLayeredWindow` instead of being blitted through the
  window's device context. The software renderer already paints
  premultiplied colours with an alpha channel, which is exactly what that
  call wants, so nothing about the drawing changes — only the way the
  pixels reach the screen. The button is therefore a round disc with a
  soft shadow, a little see-through at rest and solid under the pointer,
  and its corners are not part of the window at all: a click there lands
  on whatever is behind it. The panel paints its own rounded face the
  same way (DWM's rounding is for rectangles).
  - winit rewrites the window's styles when it is shown, moved or put on
    top, and a style bit it doesn't know about doesn't survive that, so
    the layered bit is put back before every frame (one call).
  - If the frame ever can't be handed over that way, the window falls
    back to the ordinary path (a square, opaque button) instead of
    disappearing.
- **The command line** in the panel is the sidebar's own `SendLine`, so
  the button has the same history (↑ / ↓), the same choice between the
  active session and every connected one (with its second-Enter
  confirmation), the same hosts left out by "No group send" — published
  with the host list — and the same audit trail.
- Measured: idle 0 ms CPU with the button shown; 22.7 MB private for
  both windows. Scripted check `fab_test.ps1` (scratch): shown only while
  docked-hidden, the panel opens from the button's corner and fits its
  height to the content, host search, "Show NativeTerm" brings the window
  out and hides the button, the button returns to its place.

All UI surfaces invoke one shared app-level command layer
(`native_term_app::actions`), implemented:

- `SessionCommand` (connect / reconnect, disconnect, clone, close, lock /
  unlock, clear screen, switch to, send) with one `applies(&SessionView)`
  rule each; the session card, the tab menu and the floating button grey
  their buttons out by it and run them with `Core::run`, which checks the
  rule again. Before, each surface had its own version of the rules
  (the card also required an open session, the tab menu didn't).
- `CloseSet` (others in this window, to the right, ended in all windows,
  restored and still waiting) computed by one function over the
  sessions: locked sessions never, the user's own tabs never (they
  aren't sessions). `Core::close_sessions` closes a set unless it takes
  a tab that also holds other panes; then it returns the ids for the
  confirmation dialog (`Core::close_ids` after it). The floating
  button's "Close disconnected tabs" used to close such tabs without
  asking; it now asks like the tab menu.
- Surface-specific parts stay with the surface: send opens its dialog or
  line, rename opens the host dialog. Keyboard shortcuts and the tab
  switcher will call the same commands.
- Live test `menu_portable` passes on it; its input is sent only while a
  window of the portable test Terminal is in front (checked before every
  `SendInput`).

## Appearance

### NativeTerm's own UI
- `egui` light/dark visuals, following the system by default. A "skin" is
  a named `Style`/`Visuals` preset (colors, rounding, spacing, text sizes).
  Community theme crates (e.g. `catppuccin-egui`) can be offered as presets.
- **CJK fonts**: egui's bundled fonts have no Chinese glyphs. A system font
  (Microsoft YaHei, else SimSun) is added as a fallback font. It is
  **memory-mapped**, not read: egui clones owned font data while
  parsing, so reading the 20 MB `msyh.ttc` cost ≈ 55 MB of private
  memory; a mapped file's pages are shared with the file cache.
- **Renderer: the CPU, not the GPU.** NativeTerm's window is small,
  mostly static, and repainted only on input, so it doesn't need a GPU.
  - **Why.** Any GPU backend costs more than the whole rest of the app.
    An empty eframe window on wgpu/Vulkan took 74 MB private memory:
    Vulkan instance +19 MB, device +25 MB, window surface +28 MB, all in
    the driver (the app itself added ~2 MB). Direct3D 12 took 162–165 MB,
    wgpu's GL backend 139 MB. glow (eframe's default) crashed on
    glutin 0.32.3, whose WGL pixel-format query sets a vector's length
    beyond the buffer (undefined behavior, caught by Rust's debug
    checks).
  - **How.** `src/window.rs` runs egui on winit with `egui-winit`
    (input, IME, clipboard, AccessKit) and paints with
    `egui_software_backend`'s rasterizer (SSE4.1/AVX2 picked at run time,
    tile cache: only changed tiles are redrawn) into a `softbuffer`
    surface, which presents with GDI. There is no eframe: its runner is
    tied to a GPU painter, and the software backend's own runner has no
    AccessKit and ignores IME requests.
  - **Result.** 20 MB private at start, no GPU memory; a frame at
    1440×960 takes ~1.5 ms of UI and tessellation and 2–4 ms of
    rasterization. Idle stays at 0 CPU.
  - **Frame cap.** Without vsync, egui's smooth scrolling repainted as
    fast as the CPU allowed (720 frames for 50 wheel steps). Frames are
    now spaced by the monitor's refresh rate.
  - **Trade-off.** Scrolling costs 3–4× the CPU of the GPU build
    (≈ 20 % of one core while the wheel turns: egui spreads each notch
    over several frames, and each frame redraws the scrolled panel).
    Idle, typing and clicking stay cheap, and memory is ~55 MB lower.
  - **Debug builds.** Unoptimized, the rasterizer took 131 ms for the
    tree's part of a 480×900 window at 200 % (3.8 ms optimized), so a
    debug build scrolled a tree of 735 hosts at a few frames per second.
    `[profile.dev.package."*"] opt-level = 3` in the workspace
    `Cargo.toml` optimizes dependencies in debug builds too (our crates
    stay debuggable): ~5.5 ms per frame. The tree also builds a host's
    tooltip only while it shows, and the recent list (`state.db`) is read
    at most every 2 s instead of on every frame.
  - **Rules in the runner.** The window starts hidden and is shown
    after the first frame (AccessKit must be set up before it is
    visible; a hidden window gets no redraw, so the first frame is
    painted directly). A minimized window runs no UI pass, so texture
    updates are never produced without being painted. Repaint requests
    from background threads (the core) arrive as user events and are
    dropped if a newer frame already ran. `NATIVETERM_FRAME_LOG=<file>`
    writes per-frame timings.
  - **Side effects.** Works the same in VMs, over Remote Desktop and on
    old drivers, with no fallback or restart logic; no discrete GPU is
    ever woken on hybrid laptops.
- **Implemented so far:**
  - **Theme:** Settings → Appearance: system default, light or dark
    (`settings.theme`); the window's title bar follows.
  - **Look** (`looks.rs`, Settings → Look, `settings.theme.preset`): on
    top of light and dark — standard (egui's own), the Windows accent
    colour for what is selected (read from DWM, lightened on a dark
    theme so a dark accent still shows, with black or white on it by
    brightness), soft (less contrast, quieter lines, no page-white
    window) and compact (the same colours, less room per row: more hosts
    on screen). It is only `egui` style, so switching costs nothing; the
    floating button has its own egui context and follows the choice.
  - **Icons:** named by what they mean (`native_term_platform::Icon`),
    drawn with the platform's icon font: Segoe Fluent Icons (Windows 11)
    or Segoe MDL2 Assets (Windows 10), memory-mapped like the CJK font
    and added as the last fallback font, so a glyph (private use area)
    can sit in any label; elsewhere Phosphor (regular), bundled through
    `egui-phosphor`, with `icons::phosphor` naming the glyph that means
    the same. The Windows tab menu draws `Icon::segoe` itself.
  - **Moving hosts:** a host dragged onto a folder moves into that
    folder's file (several at once if the dragged host is part of a
    selection); the folder under the pointer is marked, a grouping node
    with no file of its own is not a target, and a host dropped on the
    folder it is already in changes nothing. It is the "Move to" menu
    item's shortcut, and that item says so.
  - **Dropping files on NativeTerm's own window:** a session in the list
    and a tab (row or picture) in the tab list take dropped files like
    the session's Terminal tab does, and ask the same question (upload
    over SFTP, or type the names). On the tab itself the drop has to be
    told from a paste (the mouse hook's drag-release), but here Windows
    reports the drop, so nothing is guessed.
  - **Session tree:** folder labels are nested at " / " (an imported
    SecureCRT tree `生产 / 控制节点` shows as folders in folders; a group
    without its own file is a folder too), sorted by name, with folder
    and host icons and chevrons. With up to 20 folders everything is
    open; with more, only the top level (up to 60) or nothing. A group's
    "Connect All" opens every host below it. A host with open sessions
    has a dot: green connected, amber connecting or waiting, red failed
    or dropped (the best state of its sessions). Several hosts can be
    selected like in Explorer: Ctrl+click toggles one, Shift+click takes
    the rows from the last click (across folders and search results;
    Ctrl+Shift adds them); right-clicking a selected host then offers
    "Connect These N" (through the connection queue), in a new window,
    and "Install My Key on These N"; right-clicking outside the
    selection selects that host alone. A click opens or closes a folder;
    the second click of a double click is ignored (it closed the folder
    that had just been opened, which looked like the tree collapsing by
    itself).
- **Windows 11 materials**: tried, and they don't fit this renderer
  (measured on Windows 11 26200, 2026-09-22).
  - DWM draws Mica or Acrylic *behind* a window, so they only show where
    the window itself is see-through. NativeTerm paints on the CPU and
    presents with GDI (`softbuffer`), which writes opaque pixels: there
    is nothing for a material to show through.
  - Handing the frame over with `UpdateLayeredWindow` instead (what the
    floating button does) gives real per-pixel alpha, but the material is
    still not drawn: with `DWMWA_SYSTEMBACKDROP_TYPE` accepted
    (`DwmSetWindowAttribute` returned success) the see-through parts of
    the window showed the desktop behind it plainly, not blurred or
    tinted. `SetWindowCompositionAttribute` with an acrylic accent policy
    was accepted as well, with the same result.
  - A window with a title bar cannot be presented that way at all:
    `UpdateLayeredWindow` also sets the window's size, so passing the
    client size shrinks the window by its frame on every frame (the frame
    log showed 1440x960 -> 1418x904 -> 1396x848 ...). It works for the
    floating button because that window is borderless.
  - So a material would mean presenting through DirectComposition (a
    swap chain with alpha) — a change of renderer, not a setting. Not
    done; the CPU renderer's own advantages (20 MB, no GPU memory, works
    over Remote Desktop) were the reason for choosing it.
  - Rounded corners for borderless windows are separate and are done
    (`DWMWA_WINDOW_CORNER_PREFERENCE` for the drawer's menus; the
    floating button draws its own round shape).

### Terminal tabs
Rendering belongs to Windows Terminal, but each tab can be opened with:
- `--profile <name>` — a Windows Terminal profile (font, colors, opacity,
  background);
- `--colorScheme <name>` — a color scheme;
- `--tabColor #RRGGBB` — the tab's color.

Set per host or folder (`NativeTermProfile`, `NativeTermColorScheme`,
`NativeTermTabColor`), e.g. production tabs red, test tabs green, which
also reduces mistakes on production machines.

Implemented (`native_term_config::appearance`, shim `look.rs`):

- **Tab color**: `NativeTermTabColor` on a host or in the folder's `Host
  __nativeterm_folder__` block (a preset name red / orange / yellow /
  green / blue / purple, `#RRGGBB` or `#RGB`, or `none` to have none in a
  colored folder), passed as `wt new-tab --tabColor #RRGGBB`. The app
  keeps each saved host's look (`Core::set_host_looks`, refreshed with
  the tree), so new tabs, clones and replaced placeholders all get it;
  the tree shows the color as a bar before the host's icon.
- **Color scheme**: `NativeTermColorScheme` (one of the 16 schemes of
  Windows Terminal's `defaults.json`, or `none`). `wt new-tab
  --colorScheme` had no effect in Terminal 1.26 — not on a plain `cmd`
  tab either — so the shim applies the scheme itself before every
  connect, with OSC 4 (the 16 colors), 10, 11 and 12; back to `none`, it
  sends OSC 104 / 110 / 111 / 112 at the next connect. The colors come
  from `defaults.json` (1.26), taken over unchanged. A remote `reset`
  (RIS) puts the Terminal's colors back until the next connect. The
  user's own schemes aren't offered: NativeTerm has no colors for them.
- **Profile**: not per host. Every NativeTerm tab uses the "NativeTerm
  SSH" profile, which keeps the title fixed; another profile might not,
  and the title is how NativeTerm finds its tabs.
- **UI**: the host dialog has "Tab color" (as the folder / none / the
  presets with a swatch / custom `#RRGGBB`) and "Color scheme" (as the
  folder / Terminal's default / the 16); a folder's menu has "Tab Color"
  and "Color Scheme" submenus; the host tooltip shows what applies.
- Verified in the portable Terminal 1.26: a tab from NativeTerm red,
  still found as window 1 · tab 1; the shim's OSC gave a tab One Half
  Light (light background, dark text) where `--colorScheme` did nothing.

The NativeTerm-provided SSH profile also sets a moderate scrollback size
(`historySize`) to keep per-tab memory in check. These settings apply only
to tabs NativeTerm opens; the user's own tabs keep the user's own default
profile (AI coding sessions typically want a large scrollback).

Profiles and color schemes that NativeTerm provides are installed through
Windows Terminal's official **JSON fragment extensions**, never by editing
the user's `settings.json`:

- **Location**: `%LOCALAPPDATA%\Microsoft\Windows Terminal\Fragments\NativeTerm\`
  (per user; portable mode) or `%PROGRAMDATA%\...` (machine-wide; an
  installer may use it). The folder name `NativeTerm` is the source name
  shown on Windows Terminal's "Extensions" settings page, where the user
  can disable it.
- **Applying changes**: Windows Terminal only watches `settings.json`, not
  fragment files. After writing or removing its fragment, NativeTerm
  updates `settings.json`'s modification time (content untouched), which
  makes Windows Terminal reload settings including fragments — verified in
  the prototype; no restart needed.
- **What a fragment may contain**: profiles (new ones, or updates to
  existing ones), color schemes, and actions **without key bindings**. It
  cannot change the new-tab menu or other global settings.
- **The "NativeTerm SSH" profile**: command line `nativeterm-shim` (no
  host), explicit `closeOnExit` (see "Closing tabs"), and a moderate
  `historySize` (Windows Terminal's default is 9001 lines).
- **Status in the app** (implemented). At startup, and on "Check again",
  NativeTerm reads the chosen Terminal's `settings.json` read-only and
  sorts the profile into one of these states:
  - *installed*: the fragment points at this shim;
  - *defined in settings.json*: a hand-made profile without `source`.
    The hidden stub Terminal keeps for a removed fragment doesn't count;
  - *outdated*: the fragment points at another shim;
  - *turned off*: `NativeTerm` is listed in `disabledProfileSources`;
  - *missing*.

  How the states are handled:
  - `settings.json` is JSON with comments and trailing commas. It is read
    with a lenient parser and never written back.
  - Fragments are read the same way, so a BOM from a hand edit is fine.
  - **Installing is the user's action:** a banner button, or Settings →
    "Install / update" and "Remove". Every Terminal of the user reads
    the fragment, including the Store build, so NativeTerm doesn't write
    it on its own.
  - The one automatic write is an *outdated* fragment NativeTerm
    installed itself, rewritten when the program folder moved.
  - A turned-off profile gets a pointer to Terminal's Extensions page.
  - After writing, the `settings.json` of the chosen Terminal and of all
    installed packages is touched, so each one reloads.
  - `NATIVETERM_FRAGMENTS_DIR` redirects the fragment for tests. It also
    limits the touch to the chosen Terminal: a first test run touched the
    Store build's `settings.json` (timestamp only).
- **Favorites**: frequently used hosts can be added as profiles; fragment
  profiles appear automatically in the new-tab dropdown (the default menu
  lists "remaining profiles"). They launch `nativeterm-shim <host-alias>`,
  so they work with or without NativeTerm running. Only favorites — not
  hundreds of hosts.
  Implemented, behind "Favorites in Windows Terminal's own menus" in the
  settings (off by default, and only offered while the fragment is
  installed, since it is the fragment that carries them). Each favorite
  becomes a profile whose GUID **is** the host's `NativeTermId`, so a
  saved layout keeps pointing at the same host however it is renamed;
  its name is the host's label, its command line the shim with the
  alias, and it carries the host's tab color and color scheme (the shim
  sets the colors too, per connect; in the profile they are right from
  the first pixel). A host without an id is left out. The fragment is
  rewritten whenever the favorites change, and Terminal picks that up
  from the touched `settings.json`.
  The fragment carries **no color schemes**: the schemes NativeTerm
  offers are Terminal's own built-ins (`appearance::SCHEMES`, copied
  from its `defaults.json`), so they are already there under those
  names. A fragment may carry schemes, and would, if NativeTerm ever
  offered one of its own.
- **Command palette**: fragment actions (e.g. `newTab` with a favorite's
  command line) show up in Windows Terminal's command palette. No action
  can run an arbitrary program without opening a tab, so NativeTerm's own
  commands (tab switcher, group send) are not exposed there.
  Implemented with the favorites: each one also gets an action
  (`newTab` on its profile GUID, named "NativeTerm: <label>", with an
  `id` so a newer Terminal can bind it). Verified in the portable
  Terminal 1.26: the profile in the new-tab dropdown, the entry as the
  first hit for "NativeTerm" in the command palette.

## Localization

- `i18n-embed` + `i18n-embed-fl` (Project Fluent): message keys are checked
  at compile time; translation files are embedded in the binary, so the
  portable single-folder distribution is unaffected. Fluent handles
  plurals ("1 session" / "3 sessions").
- Default language from the OS (`sys-locale`); switchable at runtime —
  egui redraws every frame, so no restart is needed.
- Missing translations fall back to English.
- Layout: text lengths differ a lot between Chinese and English; fixed-width
  areas wrap or truncate with an ellipsis.
- The shim prints status lines into the terminal, so translations live in
  a shared crate used by both binaries.
- Not translated: host names, user labels, remote output.

Implemented (English, 简体中文, 繁體中文, 日本語 — 998 messages each):

- **Where:** `native-term-i18n` embeds `i18n/<language>/<crate>.ftl`
  (`native_term_app.ftl`, `native_term_shim.ftl`,
  `native_term_config.ftl`) and builds loaders. Each crate that shows
  text has an `i18n.toml` pointing there (with a `domain` override, since
  `fl!` would use the package name), so `i18n_embed_fl::fl!` checks every
  key at compile time. All three crates wrap it as `t!`.
- **The configuration library** says what it refuses to write in the same
  language as the rest (`native-term-config` picks the key, the message
  file holds the wording, and the values it quotes back — names, paths,
  ssh's own words — are not translated). It has its own loader, which the
  program switches together with its own (`i18n::set_language`); in the
  shim both follow `NATIVETERM_LANG` or the system.
- **Kept honest by tests**, because Fluent fails quietly: every language
  has exactly the English message ids; no translation uses a
  `{ $variable }` the English text doesn't have (an invented one shows an
  error at run time, in that language only); and every language is loaded
  and asked to format something, since a file Fluent can't parse simply
  has no messages. There is no tooling for this in the ecosystem —
  `cargo-i18n`'s checks for Fluent are "not yet implemented" and its
  extraction is gettext-only — so the tests are the check.
- **Choosing:** Settings → Language: "System default" (the Windows
  display languages, via `sys-locale`) or a fixed language, kept in
  `state.db` (`settings.language`) and applied before the window opens.
  Switching applies at the next frame. `NATIVETERM_LANG` overrides the
  system language for both binaries (tests use `en`).
- **The shim** follows the system language or `NATIVETERM_LANG`; it runs
  in Terminal's environment and doesn't read NativeTerm's setting.
- **Details:** Fluent's Unicode isolation marks around arguments are
  turned off (plain labels, no stray characters in UIA names); this has
  to be repeated after every language switch, because it applies to the
  bundles loaded at the time. A test checks that every language has
  exactly the English message ids. Messages with counts use Fluent
  plurals in English; Chinese has none.
- **Not translated yet:** error texts from the config library (host
  validation, write errors) and OS error messages (those come in the
  Windows display language anyway).

## Effects and animation

- `egui`'s built-in helpers (`animate_bool`, `animate_value_with_time`,
  easing functions in `emath::easing`); toast notifications via
  `egui-notify` or `egui-toast`.
- Used for: drawer slide, FAB speed-dial expand, tab switcher fade-in,
  hover highlights, status changes, result toasts ("Closed 5 disconnected
  sessions").

Implemented:

- **Toasts** (`toast.rs`): short messages in the bottom right corner of
  the main window for what an action just did, when nothing else on
  screen says it — closing several tabs at once, clearing finished
  sessions, connecting a group. They fade in, wait four seconds, fade
  out; a click takes one away; at most four are on screen and the newest
  win. Anything, on any thread, can ask for one (`toast::done`,
  `toast::info`), which wakes the window. Problems are deliberately *not*
  toasts: they stay in the notice line at the top until they are cleared,
  because a message that disappears by itself is the wrong place for
  something that went wrong.
- **The system setting**: `desktop::animations` reads
  `SPI_GETCLIENTAREAANIMATION` (Settings → Accessibility → Visual effects
  → Animation effects, asked at most once a second). With it off the
  toasts simply appear and disappear, and the docked window arrives
  without sliding (the slide is started already finished, so the same
  code path still sets everything).
- **Battery**: egui repaints only when something changes, but continuously
  while an animation runs. Animations are short; persistent states use
  static indicators, never endless pulsing.
- **System setting**: when Windows' "Animation effects" is off
  (`SystemParametersInfo`), NativeTerm's animations are off too.
- Terminal-side effects (acrylic, background images, retro effects) are
  Windows Terminal profile settings, selected via `--profile`.

## Distribution

- **Code signing**: UIA control, `SendInput`, and named pipes resemble
  automation tools and malware; unsigned, the binaries are likely to be
  flagged by 360 and SmartScreen. Signing is planned early, not as an
  afterthought.
- **Bundled third-party tools**: busybox-w32 (GPLv2) and OpenSSH's
  `ssh-copy-id` script ship as separate programs next to NativeTerm (MIT).
  The release includes their license texts and a pointer to the exact
  busybox-w32 source used, as GPLv2 requires.
- **ntplink**: packaging puts `ntplink.exe` in `tools\` and PuTTY's
  licence (MIT) in `licenses\PuTTY.txt`, taken from a pinned release of
  the PuTTY fork: `tools\get-ntplink.ps1 -Tag <tag> -Package <folder>`
  (checked against the release's `SHA256SUMS`; `-Arch` per package
  architecture). The fork is private, so a CI job that packages needs a
  token with read access to it for `gh`. The package's notes name the
  fork release it carries.
- **Updates**: NativeTerm checks GitHub Releases and offers updates;
  downloaded packages are verified against their signature before being
  applied. Portable copies replace files inside their own folder;
  installed copies defer to their installer or package manager.
- **Replacing files that are in use**: open tabs keep
  `nativeterm-shim.exe` running, and Windows won't overwrite a running
  executable — but it allows renaming one. A portable update renames the
  old files (`*.old`), writes the new ones, and deletes the renamed files
  on the next start once no old shim is running. Tabs opened before the
  update keep their old shim (see the pipe protocol version under
  "Installed mode specifics").
- **Network use, stated and switchable**: NativeTerm itself connects to
  the internet only for the update check (GitHub) and, if the user enabled
  it, cloud sync through rclone. The update check can be turned off, e.g.
  in corporate or offline environments; everything else works without
  internet access.
- **Supported platforms** (proposed): Windows 11 and Windows 10 2004
  (build 19041) or later — Windows Terminal's own minimum — with a current
  Windows Terminal; x64 and ARM64 builds. Windows 10 has no inbox Windows
  Terminal; the user installs it (Store, `winget install --id
  Microsoft.WindowsTerminal -e`, or the release's unpackaged ZIP). The tab
  strip, UIA tree, and ConPTY live in Windows Terminal itself, not in the
  OS, so the prototypes are expected to hold on Windows 10 with the same
  Terminal version; a Windows 10 pass is still run before release.
- **Fallback: a bundled portable Windows Terminal** (MIT-licensed) for
  machines where the user can't or won't install it — e.g. no Store, no
  winget, locked-down Windows 10. It is optional and never replaces an
  installed Terminal the user already works in, because NativeTerm's point
  is sharing the user's own window. Rules:
  - Stable release builds only (the unpackaged ZIP from the GitHub
    release, e.g. `Microsoft.WindowsTerminal_1.24.11911.0_x64.zip`, ~11 MB
    packed / 32 MB unpacked, Microsoft-signed), never Canary — Canary is a
    nightly and the least stable channel. Verified with 1.24.11911.0
    (`docs/PROTOTYPES.md`): runs next to the Store build as a separate
    instance, loads NativeTerm's fragment, sets `WT_SESSION` from
    `--sessionId`.
  - Placed in `tools\WindowsTerminal\`. The ZIP has no `.portable`
    marker; NativeTerm's package adds it, so the Terminal's settings stay
    inside the NativeTerm folder and the whole thing remains portable. Its
    license text (and `NOTICE.html`) ships alongside.
  - No auto-update: NativeTerm's own update replaces it, and the About
    page shows its version.
  - Being unpackaged, it can't be the default terminal and shows its own
    taskbar identity; stated in the UI when the user picks it.
- **Diagnostics, no telemetry**: NativeTerm writes rotated local logs to
  `logs\` in the data directory for troubleshooting and collects no telemetry of any kind.
  Implemented (`diag.rs`): one file a day, `nativeterm-YYYY-MM-DD.log`,
  holding the starts and stops and every notice the user was shown, with
  a UTC timestamp per line. Files older than a fortnight go at start, and
  a day's file stops growing at 8 MB. Notices are written for people to
  read, so nothing secret goes in; the text of sent commands has its own
  log (`audit\`), which the user can delete. The file is opened, written
  and closed per line, so a data directory on a synced or removable drive
  is never held open. For a
  tool with access to a whole server fleet, this is stated plainly.
- **Uninstall**: delete the folder, and let NativeTerm remove its Windows
  Terminal fragment first ("Clean up" in settings). The `IgnoreUnknown` /
  `Include` lines and `NativeTerm*` keys in `~/.ssh` are harmless to `ssh`
  and are left in place; the user is told they exist.
  Implemented (`cleanup.rs`, `traces.rs`). The dialog has two halves.
  What only NativeTerm can take back: the fragment profile, with the
  button that removes it. And what stays, each with its place: the data
  folder (with a button that opens it), the registry value that points
  at the data folder, the saved passwords in Credential Manager, and
  every line NativeTerm wrote in the ssh configuration — the
  `IgnoreUnknown` and `Include` lines of the main config (an `Include`
  is recognised however it is written: `~`, relative, or the folder
  itself), the `NativeTerm*` keys, and the `ProxyCommand` lines that run
  the shim — each as `file:line text`, with the user's own lines left
  out. The ssh lines are never removed: they are what makes the sessions
  work for `ssh` itself. The passwords and the registry value are
  removed only on a second click of the same button, and a profile
  someone wrote into Terminal's own `settings.json` is pointed out but
  left alone. The whole list can be copied as text.
  Windows Terminal also keeps a stub (`"source": "NativeTerm"`, the GUID,
  the name, and any user edits to that profile) in its own
  `settings.json` for every fragment profile it has seen. After the
  fragment is gone, the stubs are marked orphaned and hidden (verified in
  the source and on this machine), so NativeTerm leaves them alone by
  default. "Clean up" offers to remove them as an explicit, backed-up
  edit of Windows Terminal's settings. Fragment profile GUIDs stay fixed
  across versions, so a reinstall picks up the user's edits again.
- **Testing**: config parsing and `wt` command construction are unit-tested
  with real-world samples; UIA and shim behavior need integration tests
  that launch a real Windows Terminal.

## Settings and data directory

Portable mode is the primary mode; installed mode uses the same code. The
difference is only where the **data directory** is by default — and, like
SecureCRT's configuration folder, it can be placed anywhere (OneDrive, a
network share, a USB drive).

```
NativeTerm\                  program folder
├── nativeterm.exe
├── nativeterm-shim.exe
├── nativeterm.toml          optional pointer: data_dir = "..."
├── tools\                   ntplink.exe, busybox-w32, ssh-copy-id,
│                            optional WindowsTerminal\ (portable fallback)
├── licenses\                third-party license texts
└── data\                    default data directory in portable mode
    ├── settings.toml        shared settings + a section per machine name
    ├── commands.toml        command library
    ├── state.db             SQLite: open-session registry, recent/usage, long notes
    ├── notes.toml           synced export of long notes and tags
    ├── audit\               <machine>.log
    ├── backups\             previous versions of edited ssh config files
    └── logs\                diagnostic logs (rotated)
```

### Locating the data directory

First match wins:

1. `--data-dir <path>` on the command line (also allows several separate
   setups, e.g. work and personal);
2. the `NATIVETERM_DATA_DIR` environment variable;
3. `nativeterm.toml` next to the executable (portable mode; relative paths
   resolve against the program folder);
4. `HKCU\Software\NativeTerm\DataDir` (installed mode — the same approach
   SecureCRT uses with its `Config Path` value);
5. default: `data\` next to the executable if that folder is writable,
   otherwise `%APPDATA%\NativeTerm` (read-only locations such as
   `Program Files`, network shares, write-protected drives).

"Change data directory" in settings copies the current data to the new
location and updates the pointer used in that mode.
Implemented: the new folder must be new or empty and not inside the old
one (or around it). `state.db` is open, so it is copied with SQLite's
`VACUUM INTO` (a consistent snapshot); everything else is copied as is.
The pointer is `nativeterm.toml` when the data came from the program
folder or that file, and the registry value otherwise; with
`--data-dir` or `NATIVETERM_DATA_DIR` the setting only explains where to
change it. The switch happens at the next start (the running app keeps
its open database and paths), and the old folder is left for the user to
delete.

The session tree is separate from the data directory by design — it stays
in ssh's own config — but `config.d` can also live anywhere: the main
`~/.ssh/config`'s `Include` line accepts an absolute path, and NativeTerm
maintains that line. Putting sessions on OneDrive is the same kind of
setting.
Implemented: Settings → "Move session folders" copies the folder files
(`*.conf`) to the new place through the safe writer (owner-only access),
points NativeTerm's `Include` at it (the line's other patterns are kept;
a path with spaces is quoted), and checks that `ssh -G` accepts the
config and that the same hosts are listed — otherwise the copies and the
edit are undone. The old folder stays as it was (ssh no longer reads it).
The place is a per-machine `state.db` setting; every editor, the change
detection, the storage check and a second folder watcher follow it.

### What stays outside the data directory, by design

Session data in `~/.ssh` (the single source of truth), saved passwords in
Windows Credential Manager, the rclone config in the user profile, and the
Windows Terminal fragment file (required at that location by Windows
Terminal).

### Machine-specific settings

Window/FAB positions and the chosen Windows Terminal variant are stored
under the current machine's name inside `settings.toml`, so a data
directory used from several machines doesn't apply one machine's layout to
another.

Implemented (`settings.rs`). `settings.toml` is the home of what the user
chose; `state.db` keeps what NativeTerm recorded. The split, by key:

- shared, at the top of the file: language, theme, auto-reconnect, close
  tabs on exit, hover cards, the drag-and-drop answer, files at once,
  shortcuts, the tab switcher's view;
- per machine, under `[machine."<COMPUTERNAME>"]` (`MACHINE_KEYS`):
  `window`, `fab`, `dock_pinned`, `terminal.install`, `folders_dir`,
  `first_run_done`, `agent_hint_dismissed` — where something was on the
  screen, which Terminal to drive, where the session folder is on this
  computer, and whether this computer's start checks have been through;
- neither: the open-session registry, usage counts, long notes and each
  host's last folders in the files window. Those are records, not
  choices, they change constantly, and they stay in `state.db`.

Every machine's section is kept when one machine writes, so two computers
sharing a directory don't erase each other's layout. A data directory
written by an earlier version has its settings in `state.db`; they are
taken over into the file once, at the first start that finds no
`settings.toml` (the per-host `files.*` rows stay in the database). The
file is written whole through a temporary file and a rename; a file that
cannot be parsed is **never** overwritten, and NativeTerm says so rather
than throwing away what someone typed. A byte order mark from a Windows
editor is accepted. Comments are lost on a rewrite, like any file a
program owns.

### Data directory on shared or synced storage

- **Two machines using it at once**: writes go to a temporary file then
  rename (never a half-written file); audit logs are already one file per
  machine; a lock file records which machine has the directory open, and
  another machine opening it gets a notice.
  Implemented (`data_lock.rs`): `nativeterm.lock` is held open for
  writing (`FILE_SHARE_READ`) for as long as NativeTerm runs, which
  Windows refuses to a second writer on a local disk and over SMB alike;
  the file's text (machine, user, pid, since) only names the holder, so a
  file left behind by a machine that lost power blocks nothing. The lock
  is taken before anything is written and released when NativeTerm
  closes. The notice is a warning, not a refusal: it is the user's data
  directory.
- **Permissions**: the audit log contains every command sent; on a network
  share, check who can read it.
- **OneDrive "Files On-Demand"**: files may be cloud-only placeholders that
  download on first read, so reads can stall and change notifications can
  be unreliable. NativeTerm detects placeholder files and asks the user to
  mark the folder "Always keep on this device".
- **Sync conflicts happen in practice**: the user's own SecureCRT
  configuration on OneDrive contains numbered duplicates
  (`Global-Simon-2.ini`, `Global-Simon-3.ini`) that look like sync conflict
  copies. Conflicts must be surfaced, never silently ignored.
- Implemented (`storage.rs`, `native_term_win::cloud`): the ssh folder's
  `config*` / `known_hosts*`, every file in `config.d`, and the data
  folder are checked at start and after every reload. A file's cloud
  state comes from its attributes and reparse tag only (`FindFirstFileW`;
  recall-on-access / offline → cloud-only, a cloud reparse tag without
  the pinned attribute → may be freed), so the check never downloads
  anything. Conflict copies are recognized by name: OneDrive
  (`name-<COMPUTERNAME>[-n].ext`), Dropbox ("conflicted copy"),
  Syncthing (`.sync-conflict-`), rclone (`.conflictN`) and numbered
  copies (`name (1).ext`). A conflict copy in `config.d` is flagged as
  serious: it still matches `*.conf`, so ssh reads it and its hosts are
  defined twice. The warning names the files, opens their folder, and
  can be dismissed until something changes.

### Installed mode specifics

- **Per user**: with one installation and several Windows users, each user
  has their own data directory (default under their own `%APPDATA%`).
- **Updates**: files under `Program Files` can't be replaced without
  elevation, so installed copies are updated by their installer or
  package manager (winget, MSI); NativeTerm only announces new versions.
  Portable copies update themselves (see "Distribution").
- **Paths with spaces**: `C:\Program Files\NativeTerm\nativeterm-shim.exe`
  passes through `wt` and Windows command-line quoting; this case gets
  dedicated tests, since a portable copy in a space-free path would never
  exercise it.
- **scoop**: scoop installs each version into
  `~\scoop\apps\nativeterm\<version>`; without `persist`, `data\` would be
  lost on every update. The scoop manifest persists `data` and
  `nativeterm.toml`.
- **Old shims after an update**: tabs opened before an update keep running
  the previous shim, whose folder may be gone or replaced. The pipe
  protocol carries a version number; a newer NativeTerm talks to older
  shims where possible and otherwise marks those tabs "reopen required".
  Windows Terminal's session restore can also try to relaunch a shim path
  that no longer exists (see "Robustness").
- **Uninstall**: the uninstaller removes the Windows Terminal fragment
  itself; it can't rely on the user pressing "Clean up" first.

## Known limitations

- **Non-UTF-8 servers over OpenSSH** (e.g. legacy GBK hosts): Windows
  OpenSSH works in UTF-8 and doesn't transcode, so such hosts show
  garbled text. Telnet/serial/raw sessions run through plink and *do*
  get a per-session character set (see "Other protocols via plink").
  For GBK hosts over SSH there are two options:
  - the host can be opened with `plink -ssh` instead (its own key and
    agent handling, see there);
  - a server-side workaround (`luit`, or a UTF-8 locale).
- **Client-side session logging**: see the feature table.
- **Logon scripts with conditions** ("wait for this prompt, then send
  that"): SecureCRT implements them by reading terminal output, which
  NativeTerm never sees. Only fixed post-login commands are possible (see
  "Command library"); persistent sessions can additionally script through
  tmux on the server.
- **Keyword highlighting** (e.g. coloring `error` red): Windows Terminal
  has no such feature, and NativeTerm doesn't render output.

## Crate layout

- `native-term-config` — `~/.ssh/config` + `config.d` read/format-preserving
  write, `NativeTerm*` keys, SecureCRT importer
- `native-term-session` — session records, shim pipe protocol, exit
  classification
- `native-term-platform` — the `TerminalBackend` contract (below), the
  overlay-menu contract, tab claiming (pure logic), and the Windows
  Terminal backend: install discovery, `wt` command lines and batches,
  unelevated launch, window enumeration with the `WM_NULL` probe, UIA
  reads and actions on a watchdog worker, the fragment profile
- `native-term-shim` — the per-tab helper binary
- `native-term-win` — small Windows helpers shared by the crates above
  (current user SID, logon session id, SDDL security descriptors, file
  ACLs)
- `native-term-os` — the operating system as the app needs it: local
  time, process identity, the shell and its folders, the desktop, saved
  passwords, cloud-synced files, folder watching, global shortcuts,
  services, font files. On Windows each name is the `native-term-win`
  helper it always was; elsewhere the same name over libc and the
  desktop's tools, or an honest "not here" (`Unsupported`, an empty
  list, `None`). What only Windows has (registry, docking, layered
  windows, the picker) is reachable through it under `cfg(windows)` only
- `native-term-app` — the `egui` GUI. Off Windows it builds against
  `native_term_platform::stub::NoTerminal` (no terminal driven yet) and
  a stand-in Terminal profile (`terminal_profile_stub.rs`); everything
  Windows-only in it is behind `cfg(windows)`, and the whole binary
  passes clippy on the Linux and macOS targets
- `native-term-i18n` (planned) — translations shared by app and shim

## Platform sequencing

Windows first (the primary daily-driver OS). Other platforms get their own
terminal backend without touching the rest: `Core` holds an
`Arc<dyn TerminalBackend>` and never names Windows Terminal (the one
exception is `Core::windows_terminal()`, a `cfg(windows)` downcast for a
test that reads the install's launcher path).

### The `TerminalBackend` contract

`native_term_platform::TerminalBackend` (`platform/src/backend.rs`) is the
whole of what NativeTerm asks of a terminal program. Every method may
take seconds and is called from background threads, often several at
once; nothing in it runs on the GUI thread.

- `capabilities()` — what the backend can do beyond the basics
  (`capture`, `screen_text`, `overlay_menu`, `tab_rects`,
  `profile_install`, `named_windows`). The program hides what a backend
  can't do; a backend never pretends.
- `shim_path()` — the helper every tab runs, and what a client on the
  pipe must be (`our_shim` compares the peer's image against it).
- `window_ids()`, `foreground()`, `activate(window)` — the terminal's
  windows (responsive or not), the one in front if it is the terminal's,
  and bringing one forward. A window is a `WindowId(u64)`: the `HWND` on
  Windows (`from_hwnd`/`hwnd` exist only there), iTerm2's or WezTerm's
  window id elsewhere; the program only compares it and hands it back.
  The shim reports it as a signed integer (`Hello.terminal_window`,
  `from_wire`/`to_wire`), so the wire format did not change. `activate` also decides what the next
  `Target::Recent` means (the `-w 0` dance on Windows: activate, then
  open into the most recent window).
- `snapshot(labels)` — every window and tab, claimed for the open
  sessions' labels through `claim::Claimer` (pure logic, shared by every
  backend). Claims are kept only for labels in `labels`; a window that
  can't be read comes back `unresponsive` with `complete == false` and
  keeps its earlier claims for the next scan.
- `open(target, tabs)` — tabs running the shim. `Target::NewWindow` must
  report the new window in `OpenReport::window` (the resend path depends
  on it); `Target::Named` is a window found again by name where the
  terminal supports that, a new window where it doesn't. Confirmed
  afterwards with `wait_for` (a default method: poll `snapshot` until
  every expected label is claimed or the timeout passes).
- `open_tool(title, shim_args)` — a tab running the shim for a tool
  (installing a key, an SFTP session).
- `select(window, tab)`, `close(window, tab)` — `Ok(false)` when the tab
  moved since the snapshot (index or name no longer match).
- `subscribe(notify)` — change notifications (`Change::{Windows,
  Foreground, Tabs, Content, Popup, Moved}`); dropping the `Subscription`
  stops them. A backend without events of its own polls and reports what
  it saw change; `counts()` is for diagnostics and tests.
- `screen_text`, `capture`, `start_overlay_menu` — optional, with defaults
  of "none": the selected tab's screen text, a picture of the window
  below the tab strip, and NativeTerm's own menu over the tab strip.
- `as_any()` — for what only one terminal has (Windows Terminal's install
  and profile fragment).

The overlay menu has its own contract (`platform/src/overlay.rs`):
`MenuProvider` is what the program supplies (entries for a tab, what was
chosen, the hover card, the Ctrl+Tab tiles, a switch); `OverlayMenu` is
the menu once it is up (tabs and their rectangles, invalidation, the
Ctrl+Tab flag, what the switcher picked, and how long ago a drag from
another window ended over the terminal, which tells a drop from a paste).
`MenuTab`, `Entry`, `HoverCard`, `SwitcherTab` are neutral data. Windows
Terminal's `TabMenu` implements `OverlayMenu`; a backend that can't draw
one returns `Ok(None)` from `start_overlay_menu` and the menu accessors on
`Core` quietly report "no menu".

`tools\check-portable.ps1` runs clippy (tests and examples included,
warnings as errors) for every portable crate on the
`x86_64-unknown-linux-gnu` and `aarch64-apple-darwin` targets from a
Windows machine; nothing is linked, so no cross toolchain is needed, and
SQLite's build script is told not to look for the library. Code behind a
`cfg` that has no Unix side, or a Unix side that stopped compiling, shows
up there before a Linux or macOS machine ever sees it.

Two things test the contract. `native_term_platform::contract::exercise`
drives any backend through it against a live terminal: a new window with
two tabs, claimed, the second selected, both closed, the window gone, a
tool tab with its title (each backend crate runs it as an `#[ignore]`
test on a machine with that terminal). `native_term_platform::FakeBackend`
is a terminal in memory: tabs titled with their labels, claimed through
the real `Claimer` with no rectangles, and a test's hand on the rest (the
foreground window, the user's own tabs, tabs that never appear, change
notifications). `tests/core_fake.rs` runs `Core` against it — opening,
locating, the batch's first tab selected, focus, close, the resend of a
lost tab into the window the first try made, rescans on notifications,
the active session following the foreground, sessions of the last run
offered back — on any platform, beside a running NativeTerm
(`Core::start_with_pipe` serves a pipe of the test's own).

What stays outside the contract, by design: finding the terminal
(`Install`, `Version`, `Kind`), the elevation mismatch check, the fragment
profile and `disabledProfileSources`, `wt` command lines, UIA, the
watchdog worker, the hooks and popup behind the menu. Each of those is
Windows Terminal's business.

### Where the other platforms are going

The plan (2026-09-22) is staged so Windows behaves the same after every
step: dependencies gated so the portable crates build on Linux and macOS
targets (done); the trait above with `Core` on dynamic dispatch (done);
a `WindowId` newtype in place of the raw `HWND` (done); a `FakeBackend` so
`Core` is tested without a terminal on any platform (done); a `native-term-os`
facade for the one-line OS helpers (local time, process identity, the
shell, credentials, folder watching; done) with `cfg` splits in the binaries
(done for the app);
the session pipe and the shim on Unix (`AF_UNIX`, `SO_PEERCRED` /
`LOCAL_PEERPID`, termios; the first shim runs ssh on the inherited tty
and leaves typing and screen reads to the backend); then the backends:

- **Linux**: WezTerm first (`wezterm cli spawn / list / activate-tab /
  set-tab-title / get-text / send-text / kill-pane`, `WEZTERM_PANE` as
  the per-tab id; no events, so a 1 s poll). It also runs on Windows, so
  the whole chain is verified here before a Linux machine is at hand.
  VTE-based terminals, GNOME Terminal and Konsole have no usable API for
  reading tabs or text; Ghostty can't be read either.
- **macOS**: iTerm2 (JXA through `osascript`: `createTabWithDefaultProfile
  ({command})`, `session.uniqueId / name / contents / write`,
  `ITERM_SESSION_ID` as the per-tab id; 1 s poll). Terminal.app can only
  open tabs by simulated keystrokes. First use asks for Automation
  permission, which macOS remembers only for an `.app` bundle — a
  packaging constraint.
