# Roadmap

## Phase 0 — scaffolding and risk checks (current)
- [x] Workspace layout, crate boundaries, docs
- [x] Windows Terminal source review — settled: `-w 0` semantics and
      precedence, `--sessionId`/`WT_SESSION`, `wt` argument joining,
      duplicate/restart/restore behavior, tab move, `closeOnExit` and exit
      codes, UIA exposure (`Name` only, no `AutomationId`), fragment
      contents, hard-coded context menus, console input path, Ctrl+C
      delivery
- [x] Windows OpenSSH 9.5 source review — settled: raw + VT input mode
      while connected (no win32-input-mode), how key records are forwarded,
      Ctrl+C, prompts read directly from the console, `LocalCommand`
      (after auth, via `cmd.exe`, synchronous), `SSH_ASKPASS` /
      `SSH_ASKPASS_REQUIRE=force`, unusable `ControlMaster`, config/Include
      permission checks, exit codes 255 and -1, `TERM` via `SetEnv`,
      `RemoteCommand` conflict, agent pipe
- [x] **End-to-end prototype of injection** — `WriteConsoleInputW` records
      (character only, no key-up) reach a live ssh session, incl. Chinese
      text and emoji (verified against the local Windows `sshd`)
- [x] **Measure memory** — ≈ 15–20 MB private per SSH tab depending on
      scrollback, vs. ≈ 36 MB per tab + 566 MB baseline for Tabby under the
      same load (`docs/PROTOTYPES.md`); roughly 2× lighter, not an order
      of magnitude. The real shim's footprint is measured once it exists
- [x] **Prototype: UIA on Windows Terminal** — list tabs across all
      windows, select, selection-change events, `TextPattern` on the
      selected tab; `RuntimeId` stability; tabs scrolled out of view;
      query latency with 60 tabs — see `docs/PROTOTYPES.md`
      (virtualization confirmed, `ItemContainerPattern` lists all tabs in
      26 ms, `RuntimeId` not stable, selection events arrive in 10–25 ms
      but are noisy: use them as a trigger, debounce, read `IsSelected`)
- [x] **Prototype: shim closing and Ctrl+C** — exit 0 closes the tab, other
      codes keep it; the shim survives Ctrl+C and receives
      `CTRL_CLOSE_EVENT` when its tab is closed (default profile; to
      repeat with the "NativeTerm SSH" fragment profile)
- [x] **Prototype: `LocalCommand` helper** — fires once after login,
      silently, inheriting `WT_SESSION`; works from a path with spaces
      (quoted); no signal on login failure (exit 255); a killed server
      session also exits 255. Remaining for the real helper: short pipe
      timeout
- [x] **Prototype: `ssh-copy-id` under busybox-w32** — upstream script runs
      unmodified and uses the system `ssh.exe` (Linux target; Unix
      targets only, always `-i <file>`, own `HOME`, no applet shims —
      `docs/PROTOTYPES.md`)
- [x] **Prototype: askpass helper** — answers the session's own password
      prompt from Credential Manager, and prompts in the same console for
      everything else while ssh waits on it (`docs/PROTOTYPES.md`;
      askpass mode must not be detected from the environment alone)
- [x] **Prototype: own tab menu via `WH_MOUSE_LL`** — right-click on a
      NativeTerm tab shows NativeTerm's popup, other right-clicks pass
      through; callback latency (~45 µs); tab-rectangle cache from UIA;
      DPI; foreground and dismissal (`prototypes/menu-hook`,
      `docs/PROTOTYPES.md`). Still open: real clicks by a user, elevated
      windows, dragged/scrolled tabs, mixed-DPI monitors
- [x] **Prototype: re-claiming tabs** — pane HelpText keeps the session
      label (split/renamed tabs), restored panes keep `WT_SESSION` and run
      the profile's shim, named-window workspaces swallow commands until
      the window exists (`docs/PROTOTYPES.md`)
- [x] **Prototype: hung Terminal** — UIA blocks without limit even with
      `IUIAutomation2` timeouts; `WM_NULL` probe + `EnumWindows` +
      `ElementFromHandle` gate works
- [x] **Prototype: duplicate / split / restart shortcuts** — restart keeps
      the command line but gets a new `WT_SESSION` (shim carries
      `--session`)
- [x] **Prototype: 64 tabs** — full-list alignment via
      `ItemContainerPattern` keeps claims through scrolling
- [x] **Prototype: plink raw/Telnet** — input injection works, per-session
      code page (936/65001) verified both ways, Telnet exits 0 on server
      close, raw only notices a close on the next write
- [x] **Prototype: plink extras** — temporary `-load` session works;
      no session logging under plink; `CLOSE_WAIT` watch detects raw
      disconnects
- [x] **Prototype: plink serial** — virtual pair (HHD; com0com doesn't
      load on Windows 11): data, code pages, busy port, device-side
      close, port release on tab close. Baud mismatch and Break need
      real hardware
- [x] **Finding: title suppression must be in the profile** —
      `--suppressApplicationTitle` on the `wt` command line has no effect
      on 1.26; elevated NativeTerm lands in the elevated Terminal
      instance
- [ ] Re-check the command-line suppression flag on the Store build
      (1.24) and on newer releases; report upstream if still broken
- [x] `native-term-config` (first part): parse `~/.ssh/config` +
      `Include`-d files into a folder/host tree (sessions vs. shared
      settings, `Match` skipped); `NativeTerm*` keys and folder defaults;
      `ssh -G`; format-preserving edits; `IgnoreUnknown`/`Include`
      header; change detection; owner-only ACLs on new files, kept on
      replace; timestamped backups with pruning; validation with
      rollback; unique aliases; `NativeTermId` generation
- [x] `native-term-config` host operations (`ops.rs`): create folder,
      rename folder, create/edit/move/delete host; aliases stay fixed
      (renaming changes `NativeTermLabel`); every change backed up and
      validated with `ssh -G` (host name must resolve as entered), rolled
      back otherwise
- [x] Watching `~/.ssh`: reload when a config file really changed
- [ ] `native-term-config` (rest): `.nt.toml` non-SSH
      sessions; a "fix permissions" action for files ssh rejects
- [x] `native-term-shim`: `--session <id> <alias>` plus session GUID from
      `WT_SESSION`; builds the `ssh` command line itself (`LocalCommand`
      login helper, default keepalives only when not configured); reports
      connecting / authenticated / exit code over the pipe, stays alive
      after exit with R/C keys, re-runs on reconnect, exit 0 closes the
      tab, keeps retrying the pipe and replays its state; without a host:
      asks NativeTerm (placeholder closed) or starts a local shell;
      end-to-end tests with a fake ssh
- [ ] `native-term-shim` (rest): `NativeTermPreConnect`; plink sessions;
      askpass mode
- [x] `native-term-platform` (Windows): open tabs via
      `wt -w 0 new-tab --profile "NativeTerm SSH" --sessionId {…} --title=…
      --suppressApplicationTitle <shim> --session <id> <alias>` (`;`
      escaped, leading `-` safe); named window with the saved-workspace
      check; claim tabs by the three rules; Terminal windows via
      `EnumWindows` + `WM_NULL` probe + `ElementFromHandle`, UIA on a
      watchdog worker; select via realize + select; UIA close as a
      fallback; confirmation by title; `wt` launched through the desktop
      shell when elevated; install discovery (packages, portable,
      unpackaged); fragment writer. Live-tested on portable 1.26
      (`tests/portable_terminal.rs`)
- [ ] `native-term-platform` (rest): resend a missing tab once (with a
      new GUID, from the app); the elevated path tested from an elevated
      process
- [x] Connect in Tabs in New Window: `wt -w new` (unnamed) for the first
      ~100 tabs, `-w 0` for later batches, each after the previous
      batch's tabs exist and only while no other Terminal window was
      activated; the rest returned as pending
- [x] Connections paced by the queue (app): opening a folder, "Connect
      all" and automatic reconnects
- [x] Windows Terminal fragment writer: "NativeTerm SSH" profile (command
      line = shim without host, `suppressApplicationTitle: true`,
      `closeOnExit: automatic`, `historySize` 5000, fixed GUID), written
      only when changed, `settings.json` touched to reload, moved-folder
      detection
- [x] Fragment in the app: status (installed / in settings.json /
      outdated / turned off / missing), install and remove on the user's
      request, automatic rewrite after the program folder moved
- [ ] Fragment: color schemes, favorites as profiles and actions
- [x] `state.db` (SQLite, bundled, rollback journal) in the data
      directory: open-session registry (GUIDs, label, alias, position
      hints, "closed with its window"), usage counts; schema version
- [x] Restarts and session restore: sessions from `state.db` wait for
      their shims (lost after 12 s); restored placeholders held, replaced
      per window by waiting tabs (`--wait`), then closed; sessions closed
      with their window stay replaceable for 7 days; placeholders start
      NativeTerm when it isn't running; a second NativeTerm brings the
      first to the front. Live test `tests/restore_portable.rs`
- [ ] `state.db` (rest): long notes and tags keyed by `NativeTermId`;
      `notes.toml` export for sync; `NativeTermId` written on create/import;
      position hints used for unlocated split tabs
- [x] `native-term-session`: pipe server/client with user-only ACL,
      remote clients rejected, single instance, non-blocking duplex;
      versioned JSON-lines protocol; exit classification (255 and -1 are
      connection-level)
- [ ] `native-term-session` (rest): session records; verifying the
      client process path
- [x] `native-term-app` (first slice): core without UI (in-memory
      session registry, pipe server, tab refresh, open / focus /
      reconnect / disconnect / close, adoption of tabs from an earlier
      run, placeholders told to use a local shell) and an egui window
      (read-only tree with connect / connect in new window / whole folder,
      open sessions with state, attempt and stable window/tab position),
      CJK font mapped from the system; `--terminal-dir`, `--ssh-dir`.
      Live test `tests/core_portable.rs` and a GUI smoke test on portable
      1.26
- [x] Event-driven refresh: WinEvents for Terminal windows, UIA
      selection and structure events per window (own thread, watchdog),
      debounced; 60 s fallback scan. Overlapped pipe I/O and handle-based
      waits in the shim: idle CPU ≈ 0 for NativeTerm and shims
- [x] Session tree UI: search (fuzzy, all words, label/alias/host/
      user/note/folder, recent hosts first, Enter opens, Esc clears),
      recent hosts, virtualized rows (2000 hosts: 78 MB, idle 0 CPU),
      new/rename folder, new/edit/move/delete host dialogs
- [ ] `native-term-app` (rest): connection pacing; drag and drop in the
      tree; multi-select
- [x] **Measure NativeTerm's own idle CPU/memory** (release, idle: 0 ms
      CPU, 74 MB private with Vulkan) and the shim (≈ 1 MB private,
      6.6 MB working set)
- [x] Bring NativeTerm's memory down toward "tens of MB": CPU renderer
      (egui 0.34, own winit runner with AccessKit and IME,
      `egui_software_backend` + `softbuffer`): 20 MB private at start,
      25 MB with 2000 hosts, no GPU memory; frames capped at the monitor
      rate
- [x] Data directory resolution (`--data-dir`, `NATIVETERM_DATA_DIR`,
      `nativeterm.toml`, `HKCU\Software\NativeTerm\DataDir`, writable-folder
      default)
- [ ] Data directory (rest): `settings.toml` with per-machine sections, `audit\`,
      `backups\`, rotated `logs\`; atomic writes and a lock file
- [x] Pipe protocol version number from the first release; pipe name per
      user SID and logon session
- [ ] Absolute shim path everywhere; fragment rewritten when the program
      folder moves; tabs restored from an old path detected and reopened
- [x] Startup checks (part): Windows Terminal found (packages, portable,
      unpackaged; `--terminal-dir`), single instance (second start brings
      the first to the front), data folder writable (fallbacks)
- [ ] Startup checks (rest): Terminal recent enough; choosing among
      several installs in the UI; located via its package if the
      `wt.exe` alias is off; integrity-level mismatch warning

## Phase 1 — MVP
- [x] **SecureCRT importer** (first version): folders, names, host,
      port, user, descriptions, jump hosts (`Session:` firewalls), port
      forwards, own key files; found via SecureCRT's `Config Path`;
      secrets never read; report of skipped protocols, other firewalls,
      logon actions, saved passwords, character sets, duplicates;
      preview before writing (app dialog and `securecrt_import`
      example); per-folder writes checked with `ssh -G` and rolled back
      on failure; re-import skips `NativeTermSource` hosts; pinyin
      aliases. Tested on a synthetic 736-session / 188-folder
      configuration (29 s)
- [x] SecureCRT host keys → `known_hosts` (lenient `.pub` reading,
      unknown files reported, no duplicates, backup)
- [x] SecureCRT and PuTTY import of Telnet / serial / raw / rlogin /
      SUPDUP sessions as non-SSH sessions (serial line settings, charset,
      PuTTY-only options kept for the temporary saved session)
- [ ] SecureCRT importer: preview on the author's real configuration (by
      the author)
- Moved to Phase 2 (listed there): SecureCRT's button bar / Command
  Manager commands. VanDyke documents neither file and says the format
  changes between versions (`ButtonBarV#.ini`), so it is written against
  a real configuration, not guessed
- [x] `ssh` used for tabs and checks: native builds only (MSYS/Cygwin
      `ssh` on `PATH` skipped), no console window for checks
- [x] PuTTY saved-session import (read-only from the PuTTY registry key;
      names un-escaped, UTF-8 or ANSI; host, port, user incl.
      `user@host`, port forwards, agent forwarding, compression,
      keepalive; SSH proxy → `ProxyJump` to the imported session or the
      written-out host; other proxies, `.ppk` keys and non-UTF-8 code pages
      reported; Telnet / serial / raw / rlogin / SUPDUP skipped for plink;
      same preview, per-folder `ssh -G` check and `putty:` source for
      re-imports as SecureCRT; toolbar button only when PuTTY has
      sessions; `NATIVETERM_PUTTY_KEY` points tests at another key)
- [x] PuTTY host keys (`SshHostKeys`: RSA, ECDSA P-256/384/521 and Ed25519
      rebuilt from PuTTY's cached numbers; DSA / Ed448 reported) → `known_hosts`
- [x] First-run wizard (environment checks: `ssh -V`, Terminal, profile
      with an install button, ssh-agent; import from SecureCRT / PuTTY;
      key creation / "Install my key" on all hosts; where sessions and data
      are kept, with sync advice); shown once (`first_run_done`), again from
      Settings; every button opens the usual dialog or tab
- [x] First-run wizard: moving the data folder from step 4 (same copy and
      pointer as Settings)
- [x] First-run wizard: sessions on several computers (OneDrive / Dropbox
      folder found → move the session folders there; on the next
      computer, use the ones already there — only `Include` changes)
- [x] Unique tab titles (`web01 (2)`); foreign tabs excluded from batch
      operations (tab menu close sets contain NativeTerm sessions only)
- [x] Rename reconciliation (a host renamed in the tree while its tab is
      open): the tab keeps its title (Terminal can't retitle it, and the
      title is the tab's identity); the card says "now called …"; clones
      and replacement tabs after a restore use the new name
- [x] Rename (host dialog, alias kept) / Save Session (quick connect
      "Save…"), written back to config.d
- [x] Lock a session (session card and tab menu; kept in `state.db`
      across restarts and replacement tabs; left out of close
      disconnected / others / to the right and of "All" in group send;
      Close disabled until unlocked)
- [x] Session states incl. "waiting for login" and "login failed"
      (`LocalCommand` signal)
- [x] Reconnect in place / Disconnect / Close
- [x] Sessions whose shim is gone at startup (closed while NativeTerm was
      off, shutdown, sign-out) are closed at once, not looked for
- [x] Close NativeTerm's tabs when NativeTerm exits (on by default; unlocked
      sessions only, through their shims; foreign tabs and windows untouched)
- [x] Optional auto-reconnect that never retries login failures (setting
      in `state.db`; 3/10/30/60 s, at most 10 tries, spread per session;
      live test `auto_reconnect`)
- [x] "Install my key" for a host or folder (shim runs ssh once with a
      POSIX script, one tab per host; key creation when there is none)
- [x] "Install my key" on Windows hosts (recognized from how cmd.exe or
      PowerShell reject the POSIX script; PowerShell adds the key to the
      admin or user file by Windows OpenSSH's rules)
- [x] "Install my key" with one password for a whole batch (asked once in
      one tab, served to each ssh through the shim as askpass helper;
      other prompts still asked in the console)
- [x] Close Others / Close Disconnected (tab menu); "Close Tab Group"
      has no counterpart (Terminal has no tab groups)
- [x] Close Tabs to the Right (real tab order via UIA)
- [x] Clone session (same alias, base label, most recent window)
- [x] Clone: port forwards cleared (shim `--no-forwards` →
      `-o ClearAllForwardings=yes`, kept across restores)
- [x] Open a whole folder (batched `wt` calls)
- [x] Multi-select in the session tree (Ctrl+click, Shift+click): connect
      the selection (also in a new window), install the key on it
- [x] Open a whole folder (rest): rate-limited connections (more than 3
      hosts: tabs wait, the queue connects them, at most 4 logging in, 200 ms
      apart), the batch's first tab selected at the end; "Connect all" and
      automatic reconnects (resume, network change) use the same queue
- [x] Quick connect to `user@host[:port]` from the search box, "Save…"
      afterwards (new-host dialog, filled in)
- [x] "Remove this host's old key" (`ssh-keygen -R`, confirmed)
- [x] Recovery after NativeTerm restart (re-discover tabs, re-pair shims
      by session GUID / `--session`); restored placeholders replaced by
      waiting tabs in their original order (see Phase 0, restarts)
- [x] Restored sessions: "Connect all" (paced) / "Close all" in the
      sessions panel, single ones from their row
- [x] Recovery (rest): unlocated sessions show where their tab last was
      (from `state.db`), and "Locate" selects the unclaimed tabs one by
      one until every session is found, then puts the selection back
- [x] Tab menu: "Clear Screen and Scrollback" (after login the shim
      clears the tab, then types Ctrl+L so the remote side redraws on an
      empty screen; before login only the scrollback; see PROTOTYPES.md)
- [x] Own tab menu, first version (`windows_terminal::menu` +
      `tab_menu`): `WH_MOUSE_LL`/`WH_KEYBOARD_LL` installed only while
      NativeTerm has located tabs; non-activating GDI popup following the
      owning Terminal's theme / high contrast / text size; Terminal's
      menu blocked on NativeTerm tabs only; stale rectangles pass the
      click through; structure changes classified by sender (Terminal's
      own menu doesn't invalidate); location hooks per Terminal process;
      connect / disconnect / clone / close / close others / close
      disconnected / close to the right; mixed and renamed tab headers;
      live test `menu_portable`
- [x] Tab menu: the Terminal theme from `settings.json` cached until the
      file changes (reading it was the slow part of opening the menu)
- [x] Tab menu: acrylic decided against (Terminal's own menus are solid in
      both themes; GDI can't draw it anyway)
- [x] Tab menu drawn with Direct2D/DirectWrite (font fallback, color emoji;
      a DC render target in software, so no GPU needed)
- [x] Tab menu: its own rounded shape where DWM doesn't round popups
      (Windows 10, build < 22000): a layered window drawn with alpha;
      checked on Windows 11 by forcing the path (`NATIVETERM_MENU_LAYERED=1`),
      not yet seen on a real Windows 10; no shadow on that path
- Moved to Phase 2: a right-click on a NativeTerm tab whose rectangle is
  stale (right after a tab drag, until the rescan) still reaches
  Terminal's own menu. Handling it means swallowing the click, asking UIA
  outside the hook and replaying it to Terminal when it isn't ours —
  synthetic input for a window of a second; passing it through stays the
  fail-safe choice until then
- [x] Tab menu: rename (opens the host dialog in the main window) and a
      confirmation before a batch close would close a tab that holds other
      panes; the menu asks the main window, whose dialogs it uses
- [x] Non-SSH sessions via plink: `.nt.toml` storage in the same tree
      (editor operations included), plink located (bundled or installed
      PuTTY), shim runs plink as a child with the session's code page
      (UTF-8 default, GBK etc.), temporary `-load` session for options
      without a command-line flag, Telnet exit 0 = disconnected, raw
      `CLOSE_WAIT` watch, serial "port busy" check, `--ssh-dir` for tabs
- [x] Non-SSH sessions in the app: tree icons and menu, new / edit dialog
      (protocol, host, serial port picker, speed and line settings,
      charset), a serial port already open in a session is refused naming
      that tab; ssh-only actions skip them
- [x] plink checked against PuTTY 0.85's source: Ctrl+C kept as a key
      (it ended plink), socket errors (`INT_MAX`) count as a dropped
      connection, the ineffective `TelnetKey` removed, local echo / line
      editing offered, the keepalive imported as minutes plus seconds
- [x] Non-SSH sessions no longer depend on PuTTY's "Default Settings":
      plink always loads NativeTerm's temporary session, which also
      carries the tab's size at connect time (Telnet window size)
- [x] ntplink: NativeTerm's own console client over PuTTY 0.85's
      unmodified Telnet / raw / rlogin / SUPDUP / serial code (PuTTY fork
      `fivetime/putty`, additions in its `patches` folder, synced with
      upstream and released by its CI; `tools/get-ntplink.ps1` downloads
      a release, `tools/build-ntplink.cmd` builds one). No registry (options as
      `-set`), follows tab resizes (NAWS), Ctrl+C a key, raw ends on the
      server's close, echo / line editing for every protocol, Break and
      Telnet commands over a control pipe ("Break" on the card, "Send
      Break" in the tab menu), clear exit codes. The shim prefers it and
      falls back to plink with the old workarounds
- [ ] Serial Break on a real device (the virtual COM driver doesn't pass
      Break on)
- [ ] Output logging for non-SSH sessions (possible now in ntplink)
- [x] "No data since …" for serial sessions (the shim watches the console
      near the cursor; quiet after 30 s, cleared by new output; replayed
      to a restarted NativeTerm)
- [x] Session options dialog for SSH hosts (host menu; connection,
      authentication, algorithms, host key, forwarding, environment;
      algorithm pickers from `ssh -Q`, current values from `ssh -G` shown
      greyed, values written verbatim and checked by `ssh -G` with
      rollback)
- [x] Folder options (folder menu): every host in the folder file gets
      `Tag nativeterm-<file>`, a `Match tagged` block at the end holds the
      options (plus user, port, jump host, keys); a host's own values win;
      kept last and tagged on create / import / move; OpenSSH 9.4+ only
- [x] Session options: plink pages for non-SSH sessions ("PuTTY options" in
      the session dialog: connection, Telnet and SUPDUP pages; only values
      that differ from PuTTY's defaults are kept, unknown ones untouched)
- [x] Setting "Hide Windows Terminal's own SSH profiles"
      (`disabledProfileSources`, backed-up explicit edit: a textual change
      of the top-level list only, comments and layout kept, the result
      re-read before writing; unticking removes what ticking added)
- [x] Shared app-level command layer used by every UI surface
      (`actions`: one applies-rule per session command, one close-set
      function; card, tab menu and floating button use it; the floating
      button's "close disconnected" now confirms tabs with other panes)
- [x] "Change data directory" (Settings: an empty or new folder; files
      copied, `state.db` as a `VACUUM INTO` snapshot; `nativeterm.toml` in
      portable mode, `HKCU\Software\NativeTerm\DataDir` otherwise; used from
      the next start; the old folder is kept)
- [x] Session folders location (Settings → "Move session folders"): the
      `*.conf` files are copied through the safe writer, NativeTerm's
      `Include` line is rewritten (other patterns on it kept, quoted when
      the path has spaces), and ssh must accept the result and list the same
      hosts, or it is all undone; the old folder is left as it was; the
      place is a per-machine setting, watched for changes like `~/.ssh`
- [x] OneDrive placeholder detection with "Always keep on this device"
      guidance; visible sync-conflict notices (ssh folder, `config.d`, data
      folder; checked at start and on every reload; attributes only, so a
      cloud-only file is never downloaded by the check)
- [x] ssh-agent check and guidance (service state, protected keys, keys
      in the agent, hint only when needed, `ssh-add` in a tab)
- [x] Agent-forwarding notice when opening
      many `ForwardAgent` hosts (3 or more, checked with parallel `ssh -G`
      after the tabs are opened, never blocking them)

## Phase 2 — quality of life
- [x] Session search (label, alias, host, user, folder, note; fuzzy) and
      recent hosts
- [x] Pinyin search for Chinese labels and folders (initials `kzjd`
      or full `kongzhi`); favorites (`NativeTermFavorite`, a Favorites
      section at the top of the tree, toggled from the host menu);
      the recent list lives in the per-machine `state.db`
- [x] Command library (`commands.toml`), edited from the send dialog
- [x] Post-login commands (`NativeTermOnLogin`, host or folder default),
      typed after every login
- [ ] Import SecureCRT's button bar and Command Manager commands (the
      "send string" ones) into the command library
- [ ] Editable keyboard shortcuts; defaults avoid keys missing on laptop /
      Mac keyboards
- [ ] Throttle snapshots, previews, and sync on battery power
- [x] Active-session tracking (last Terminal window in front + its
      selected tab; used by the floating button)
- [x] Send commands to one or several logged-in sessions (shim injection,
      confirmation for several, audit log; card, list, tab menu, floating
      button); end-to-end test `send_commands`
- [ ] Send commands (rest): input line at the bottom of the sidebar;
      `tmux send-keys` fallback for persistent sessions; per-folder "no
      group send"
- [ ] Persistent sessions via tmux (`NativeTermPersistent`): attach-or-create,
      via `-o RemoteCommand` (hosts with their own `RemoteCommand` are
      skipped), `sh -c` wrapper with fallback when tmux is missing, optional hidden
      status bar, list/reopen/kill remote `nt-*` sessions, `screen` support
- [ ] Optional saved passwords (Windows Credential Manager + per-process
      `SSH_ASKPASS=force`, shim as helper): answers only the session's own
      password prompt and handles every other prompt in the console, one
      attempt then mark invalid, risk warning in the UI; helper checks its
      caller chain (shim → ssh → helper)
- [x] Tab switcher, first version: all tabs in all windows (the user's own
      too) with live titles, grouped by window, search, Enter / click
      switches (Ctrl+T)
- [ ] Tab switcher (rest): last-seen text (UIA `TextPattern`) or image
      snapshots; tmux text preview for persistent sessions
- [ ] Tiled thumbnail switcher on Ctrl+Tab (off by default): the existing
      low-level keyboard hook swallows Ctrl+Tab while a Terminal window
      with NativeTerm tabs is in front, a non-activating grid follows
      further Tab presses and commits when Ctrl is released; thumbnails
      from captures taken while a tab was selected (see "Taking over
      Ctrl+Tab" in ARCHITECTURE.md). Prototype done
      (`prototypes/tab-switcher`, PROTOTYPES.md): hook ~9 µs, grid in
      70–150 ms, switch 40–60 ms after Ctrl is released
- [x] NativeTerm SSH profile with a moderate scrollback size
      (`historySize` 5000)
- [x] Sidebar auto-hide/pin drawer (QQ-style): docks at the top, left or
      right edge (not towards another monitor), slides away to a 4 px
      strip, back on touch or activation, pin, always on top while
      docked, placement and edge remembered; no idle polling
- [x] Floating action button, shown only while the docked window is hidden:
      host search / quick connect, active session reconnect and clone, all
      tabs, close disconnected, show NativeTerm; draggable, position kept
- [ ] Floating action button (rest): round/transparent shape, send command
- [ ] Optional `RegisterHotKey` shortcut, off by default
- [ ] Per-host/folder terminal appearance: profile, color scheme, tab color;
      NativeTerm profiles shipped as a Windows Terminal JSON fragment
- [x] Light / dark / system theme (title bar included), Fluent/MDL2 icons,
      nested folders in the tree, status dots on hosts
- [ ] NativeTerm themes (rest): presets, Mica/Acrylic
- [x] Localization (English, Simplified Chinese) with runtime switching:
      `native-term-i18n` (Fluent, keys checked at compile time), setting
      in `state.db`, shim follows the system language / `NATIVETERM_LANG`
- [ ] Localization (rest): config library errors; more languages
- [ ] Animations (respecting the system animation setting) and toasts
- [ ] External SFTP handoff (shell out to a configured tool)
- [ ] Shared credential sets (`NativeTermCredential`)
- [ ] SOCKS/HTTP proxy helper in the shim (`NativeTermProxy` as
      `ProxyCommand`)
- [ ] Optional `trzsz ssh` per host (`NativeTermTrzsz`, user-installed)
- [ ] Per-session Backspace mapping (`^H` / `^?`) for plink sessions
- [ ] Optional `plink -ssh` for GBK SSH hosts (host/port/user/key from
      `ssh -G`, `.ppk` key, Pageant)
- [ ] Server-side session logging for persistent sessions (`tmux pipe-pane`)
- [ ] Optional cloud sync via user-installed rclone: explanation and
      consent page, rclone detection/version check, `bisync` of
      `config.d` and settings, conflict UI, `known_hosts` line merge,
      per-machine audit logs, `~`-relative paths, `crypt` recommendation;
      private-key sync left to the user's choice, with risk information

## Phase 3 — release and other platforms
- [ ] Code signing
- [ ] Update check via GitHub Releases, signature-verified packages,
      rename-then-replace for in-use shims, switch to turn the check off
- [ ] Supported platforms: Windows 11 and Windows 10 2004 (19041)+, x64
      and ARM64; Windows 10 pass of the UIA and shim prototypes (with the
      portable Windows Terminal ZIP if no Store/winget is available).
      Not run yet: no Windows 10 machine or VM was available. Already
      prepared for it: GDI presentation (no GPU requirement), Segoe MDL2
      Assets fallback for the menu icons. Still to check there: API
      availability on 19041: UIA events and tab rectangles, the
      tab menu's square corners, the fragment path, the elevated launch
      through Explorer
- [x] Windows Terminal variant detection: packaged (Store, Preview) and
      unpackaged/portable by path; absolute `wt.exe` per variant; UIA
      windows filtered by process image path
- [ ] Optional bundled portable Windows Terminal (stable ZIP, `.portable`,
      license text) as a fallback package
- [ ] "Clean up" (remove the Windows Terminal fragment) before deleting
      the folder; tell the user about the lines left in `~/.ssh`
- [ ] State "no telemetry" in README and the About page
- [ ] Third-party license texts and busybox-w32 source pointer in the release
- [ ] Integration test suite against a real Windows Terminal, including an
      install path with spaces
- [ ] Installed mode: winget/MSI packaging (updates via the installer,
      uninstaller removes the fragment), scoop manifest with `persist`,
      per-user data directories
- [ ] Packaging and its CI — the last step, once the product is
      complete: first trim what the product no longer uses (plink
      fallback and its workarounds, prototypes, probes, test hooks that
      only served experiments), then build the packages. `ntplink.exe`
      goes into `tools\` and PuTTY's licence into `licenses\`
      (`tools\get-ntplink.ps1 -Tag … -Package …` exists for it; the CI
      needs a token that can read the private PuTTY fork)
- [ ] macOS backend — Terminal.app / iTerm2 tab scripting
- [ ] Linux backend — investigate VTE

## Explicitly not planned
- Self-rendered terminal emulation of any kind
- A bundled SSH implementation (always the system's own `ssh`; auth, keys,
  and known_hosts stay where the OS keeps them)
- Moving a running tab to a new window from NativeTerm ("Send to New
  Window") — no `wt` command exists; dragging the tab out works and is
  re-claimed
- Managing the user's own tabs (local shells, AI coding sessions): they
  are listed in the tab switcher, nothing more
- Live thumbnails of background tabs (the terminal doesn't render them)
- Client-side session logging (output never passes through NativeTerm)
- Importing SecureCRT's stored passwords
- Bundling or silently installing rclone
- Syncing saved passwords
- A double-tap-Shift trigger (needs a global low-level keyboard hook;
  conflicts with Chinese IMEs and JetBrains IDEs)
- Per-session character sets for OpenSSH sessions (Windows OpenSSH works
  in UTF-8; plink sessions do get them)
- Conditional logon scripts ("wait for X, send Y"), SecureCRT-style
  logon scripts, and keyword highlighting — all require reading terminal
  output
- Windows Terminal's own tab menu on NativeTerm tabs (blocked on purpose)
- Telemetry of any kind
- Multiple sessions per tab via panes (for now; would need its own design)
