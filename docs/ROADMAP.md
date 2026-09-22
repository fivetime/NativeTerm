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
- [x] Re-check the command-line suppression flag on newer releases:
      still broken on 1.26.2609.15001 (2026-09-22). Two tabs opened side
      by side, `wt new-tab --title FLAG-ON --suppressApplicationTitle
      cmd /c "title SET-BY-PROGRAM & timeout 30"` and the same without
      the flag: both tabs ended up called SET-BY-PROGRAM, so the flag on
      the command line does nothing and the title has to come from the
      profile (which is what NativeTerm does). Upstream already has it —
      microsoft/terminal #15732 (the same `wt --suppressApplicationTitle
      --title …` case) and #19493 (October 2025, still open) — so there
      is nothing to report that isn't there. The Store build was not
      tested: the only Store Terminal on this machine is the one the
      author works in
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
- [x] `native-term-config` (rest): `.nt.toml` non-SSH sessions; and a
      "fix permissions" action for files ssh rejects — `acl::open_to_others`
      reads a file's DACL and reports who else may write it (allow entries
      only, write rights only, the SDDL abbreviations and hexadecimal
      masks), the storage check runs it over the config, the folder files
      and the private keys, and the banner offers to put the permissions
      back (owner, Administrators, SYSTEM; inherited entries off), which is
      what NativeTerm writes itself
- [x] `native-term-shim`: `--session <id> <alias>` plus session GUID from
      `WT_SESSION`; builds the `ssh` command line itself (`LocalCommand`
      login helper, default keepalives only when not configured); reports
      connecting / authenticated / exit code over the pipe, stays alive
      after exit with R/C keys, re-runs on reconnect, exit 0 closes the
      tab, keeps retrying the pipe and replays its state; without a host:
      asks NativeTerm (placeholder closed) or starts a local shell;
      end-to-end tests with a fake ssh
- [x] `native-term-shim` (rest): `NativeTermPreConnect` — a command run
      on this computer before every attempt (a VPN, a tunnel, mounting a
      drive), per host or as a folder default, `none` on a host to keep
      the folder's away. It runs in the tab through `cmd /c`, the
      session waits for it, and what it prints is what the person sees.
      A command that fails is reported and the connection goes ahead
      (plenty of them "fail" harmlessly); one written with `!` in front
      stops the connection instead. `.nt.toml` sessions have the same
      through `pre_connect`. plink sessions and askpass mode were
      already done (`shim/preconnect.rs`,
      `native-term-config/src/preconnect.rs`)
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
- [x] `native-term-platform` (rest): a tab that never appears is asked
      for once more, under a new terminal GUID and in the window the
      first try used — the label and the session stay, so it is the same
      session in the end. Only once, only when every Terminal window
      could be read (or a second tab might be a duplicate) and only while
      no shim of that session has spoken. A test hook drops the first
      `wt` command for a label (`command::SWALLOW_ENV`), which is how it
      is live-tested (`core_portable.rs`)
- [x] The elevated path tested from a process that really is elevated
      (`tests/elevated_launch.rs`, run from an elevated prompt): the
      shell detour puts the tab in the user's own Terminal (its process
      is not elevated), while a direct launch from there makes Terminal's
      elevated instance — which is what the detour avoids
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
- [x] Fragment: favorites as profiles and actions. With "Favorites in
      Windows Terminal's own menus" on (off by default, and only while
      the fragment is installed), every favorite host gets a profile of
      its own — its `NativeTermId` as the GUID, so saved layouts keep
      pointing at it; its label as the name; the shim with the alias as
      the command line; its tab color and color scheme — and a command
      palette entry that opens it. So a favorite can be opened straight
      from Windows Terminal, with or without NativeTerm running. The
      fragment carries no color schemes: the ones NativeTerm offers are
      Terminal's own built-ins, which it already has (it could carry
      them if that ever changes)
- [x] `state.db` (SQLite, bundled, rollback journal) in the data
      directory: open-session registry (GUIDs, label, alias, position
      hints, "closed with its window"), usage counts; schema version
- [x] Restarts and session restore: sessions from `state.db` wait for
      their shims (lost after 12 s); restored placeholders held, replaced
      per window by waiting tabs (`--wait`), then closed; sessions closed
      with their window stay replaceable for 7 days; placeholders start
      NativeTerm when it isn't running; a second NativeTerm brings the
      first to the front. Live test `tests/restore_portable.rs`
- [x] Notes and tags, kept by `NativeTermId` in `state.db` and written
      to `notes.toml` so they sync: as many lines as someone likes, plus
      tags, in the host dialog and the non-SSH session dialog. They are
      searched along with the name, alias, host and one-line note, and
      shown when the mouse rests on a host. The file and the database are
      merged at start, newer note wins per host, and a cleared note is an
      empty note rather than a missing one so clearing reaches the other
      computers too. A host without an id gets one written into its block
      the first time something is kept about it (`notes.rs`,
      `Editor::ensure_id`)
- [x] `state.db` (rest): position hints used for unlocated split tabs —
      the locating sweep selects tabs one by one, which the person sees, so
      the tabs a session was last seen at (window number and tab index from
      `state.db`) are tried first and it usually ends on the first pick
- [x] `native-term-session`: pipe server/client with user-only ACL,
      remote clients rejected, single instance, non-blocking duplex;
      versioned JSON-lines protocol; exit classification (255 and -1 are
      connection-level)
- [x] `native-term-session` (rest): the connecting process is checked
      before a word is exchanged — it must be the very
      `nativeterm-shim.exe` NativeTerm starts its tabs with
      (`GetNamedPipeClientProcessId`, then the process image, compared
      without case and through both paths' real names). Anything else is
      closed at once and said once, so a program of this user cannot
      claim a session's GUID and be handed what was meant for that tab.
      Session records live in `native-term-app::registry` instead of this
      crate: they are rows of `state.db` and follow the app's session
      model, while this crate stays the protocol and the pipe
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
- [x] Connection pacing (see the queue above), including a `Connect`
      that goes down with a link breaking: the shim replays "waiting"
      when it comes back, and a session that was told to connect is told
      again rather than waiting forever. It used to make one tab in six
      hang in "waiting" about half the time
      (`connect_queue.rs`, `core_portable.rs`)
- [x] `native-term-app` (rest): drag and drop in the tree — a host
      dragged onto a folder moves there (the whole selection if it is part
      of one), the folder under the pointer is marked, and a grouping node
      without a file of its own takes nothing. The same thing the "Move
      to" menu does, which now says so
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
- [x] Data directory (rest): `settings.toml` holds what the user chose,
      shared between machines except the few keys that only make sense on
      one (`window`, `fab`, `terminal.install`, the session folder, the
      wizard), which go under `[machine."<name>"]`; the settings an older
      data directory kept in `state.db` are taken over once, while the
      records stay there (open sessions, usage, each host's last folders).
      The file is written whole through a temporary file and a rename,
      and one that cannot be parsed is never overwritten — NativeTerm
      says so instead. `audit\`, `backups\` and `logs\` are made at
      start; `logs\` now holds NativeTerm's own daily log (every notice,
      pruned after a fortnight, nothing secret and nothing sent
      anywhere). A `nativeterm.lock` held open for writing says which
      machine has the directory, so a second one is told rather than
      quietly overwriting it (`settings.rs`, `diag.rs`, `data_lock.rs`)
- [x] Pipe protocol version number from the first release; pipe name per
      user SID and logon session
- [x] The program folder moved: the tab profile is rewritten at start
      (quietly when the old program is gone, with a word when it is still
      there — a second copy), and the `ProxyCommand` lines NativeTerm
      wrote are pointed at this copy. A line whose helper **is** on this
      computer is left alone, so a config synced between computers keeps
      working on both (`native-term-config/src/repair.rs`)
- [x] The rest of it: sessions of the last run whose tabs are not there
      any more are offered back. NativeTerm used to drop them quietly
      when their helper was gone; now a line says how many there are,
      with "Open them again" and "Leave them". After a move it says why:
      the tabs Terminal restored started a helper that is no longer
      there. Opening them again also marks the old records as not worth
      restoring, so a pane Terminal brings back later is not a duplicate
      (`Core::lost_at_start`, `App::lost_banner`)
- [x] Startup checks (part): Windows Terminal found (packages, portable,
      unpackaged; `--terminal-dir`), single instance (second start brings
      the first to the front), data folder writable (fallbacks)
- [x] Startup checks (rest): the Terminal's version is read (a package's
      full name, or `WindowsTerminal.exe`'s version resource) and a
      version older than 1.21 is named at start and in the wizard — that
      is where `--sessionId` comes from, and without it NativeTerm cannot
      tell its tabs apart; the install is chosen in the settings when
      there is more than one (`terminal.install`, read at the next start,
      a portable folder included); a packaged Terminal whose `wt.exe` app
      execution alias was turned off is started from the package folder
      instead (the alias only points there), with the PATH as a last
      resort; and NativeTerm says plainly when it and Terminal run at
      different permission levels, which Windows keeps apart either way
      (`install.rs`, `main.rs::install_notices`, `WindowsTerminal::mismatch`)

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
- [x] Output logging for non-SSH sessions: ntplink writes PuTTY's
      `LogType` 1 (text without escape sequences) / 2 (every byte) to
      `LogFileName` (PuTTY's `&H` `&Y&M&D` `&T` codes, appended); "Session
      log" in the session dialog, default `<data dir>\logs\&H-&Y&M&D.log`
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
- [x] Import SecureCRT's button bars (send-string buttons) into the command
      library, grouped by bar; other buttons listed with the reason
- [ ] Import SecureCRT's Command Manager commands (storage format needed:
      one command file from the user); preview on a real button bar file
- [x] Editable keyboard shortcuts; defaults avoid keys missing on laptop /
      Mac keyboards
- [x] Nothing to throttle on battery: idle CPU is 0 ms (window open or
      minimized); previews are snapshots taken on events (tab switched
      away, switcher opened), never on a timer; a future timer-driven
      feature is off by default and pauses on battery / minimized
- [x] Active-session tracking (last Terminal window in front + its
      selected tab; used by the floating button)
- [x] Send commands to one or several logged-in sessions (shim injection,
      confirmation for several, audit log; card, list, tab menu, floating
      button); end-to-end test `send_commands`
- [x] Send line at the bottom of the sidebar (active session or all
      logged-in ones, confirmation for several, history) and per-folder
      "No group send"
- [x] Send commands to persistent sessions that aren't logged in (tab
      disconnected or closed) through `tmux send-keys` over their own ssh
      connection (key/agent login), results as they arrive, audit log
- [x] Persistent sessions via tmux or screen (`NativeTermPersistent`, per
      host or folder default, `off` per host): attach-or-create via
      `-o RemoteCommand` (hosts with their own `RemoteCommand` are
      skipped), `sh -c` wrapper with a plain-shell fallback when the
      program is missing; host dialog, folder menu, tooltip. Verified
      against a temporary Ubuntu container from Windows 11 and Windows 10
      (tmux and screen: disconnect / killed ssh, reconnect, same shell;
      fallback without tmux)
- [x] Sessions on the server (a host's menu): list NativeTerm's tmux /
      screen sessions there, show the tab that has one, reopen a closed
      tab's session in a new tab (same shell), end one (asked twice)
- [x] Persistent tmux sessions look plain: status bar off for
      NativeTerm's sessions (the card says "kept on the server"), 50,000
      lines of history
- [x] Persistent sessions (rest): the list per folder, "Open All" for
      the detached ones. (tmux's mouse
      mode stays off: Windows Terminal's own selecting, copying and
      right-click paste come first; decided 2026-09-19.)
- [x] Optional saved passwords (Windows Credential Manager + per-process
      `SSH_ASKPASS=force`, shim as helper): answers only the session's own
      password prompt and handles every other prompt in the console, one
      attempt then mark refused, risk warning in the UI; helper checks its
      caller chain (shim → ssh → helper). Verified live against a password
      container; password fields turn the IME off
- [x] Tab switcher, first version: all tabs in all windows (the user's own
      too) with live titles, grouped by window, search, Enter / click
      switches (Ctrl+T)
- [x] Tab switcher pictures: "All Tabs" as a list (picture on hover) or
      as pictures, each tab as it looked when last seen selected, taken
      after Terminal's notifications only (no live thumbnails, no timer)
- [x] A tab nobody has looked at still shows what is on it: Terminal
      renders only the tab it shows, so its picture waits for that, but
      the tab's own console keeps its screen either way. The shim reads it
      (`ReadConsoleOutputCharacterW` on `CONOUT$`, `AppMessage::Screen` →
      `ShimMessage::Screen`) and the card draws the text until a picture
      of it exists. Asked for only while the pictures are shown and the
      window is focused, at most every 5 s, a few KB an answer; a
      disconnected session shows its "connection lost" screen, and a tab
      without our shim keeps the old note
- [x] Tab switcher (rest): the text of a tab NativeTerm doesn't run is
      read with UIA `TextPattern` at the moment it is pictured (~2 ms for
      a screenful), kept with the picture, and searched — so a local
      shell or an AI session is found by what is on it, not only by its
      title. A hit in the text ranks below one in the name. NativeTerm's
      own tabs already answer with their console text, which also covers
      the tmux preview the item asked for
- [x] A card when the mouse rests on a tab (like a browser's): the tab's
      picture, or the text its console holds, with the session's name and
      state, under the tab after half a second. It uses what the tab menu
      already has — the low-level mouse hook and the same non-activating
      Direct2D popup — and the hook only tests the point against the tab
      strip's box (four comparisons; a low-level hook that takes too long
      is dropped by Windows, which is what a fuller test in it caused).
      Terminal's own title tooltip still shows, above the tab: it is set
      in its code (`Tab.cpp`, `ToolTipService::SetToolTip`) with no
      setting to turn it off, and Terminal has no API for any of this
      (Monarch/Peasant was removed; only UIA is left to read from outside)
- [x] Tiled thumbnail switcher on Ctrl+Tab (off by default; "Ctrl+Tab
      shows a grid of tab pictures" in the options): the low-level
      keyboard hook the tab menu already installs swallows Ctrl+Tab while
      a Terminal window with NativeTerm tabs is in front, a non-activating
      grid (`windows_terminal/switcher.rs`, drawn with the same Direct2D
      popup as the menu and the hover card) shows every tab of that window
      with the picture it was last seen with, further Tab / Shift+Tab /
      arrow presses move the choice, Ctrl released switches through UIA,
      Esc (or a click elsewhere, or any other key) leaves the tabs as they
      are. Tiles with no picture show the text the tab last had. The
      mouse picks a tile too. Turned off, Terminal's own Ctrl+Tab is
      untouched
- [x] NativeTerm SSH profile with a moderate scrollback size
      (`historySize` 5000)
- [x] Sidebar auto-hide/pin drawer (QQ-style): docks at the top, left or
      right edge (not towards another monitor), slides away to a 4 px
      strip, back on touch or activation, pin, always on top while
      docked, placement and edge remembered; no idle polling
- [x] Floating action button, shown only while the docked window is hidden:
      host search / quick connect, active session reconnect and clone, all
      tabs, close disconnected, show NativeTerm; draggable, position kept
- [x] Floating action button (rest): the window carries its own
      transparency (`native_term_win::layered`, `UpdateLayeredWindow` from
      the frame the software renderer already paints), so the button is a
      round, slightly see-through disc with a soft shadow and the corners
      are not part of the window — a click there reaches what is behind
      it. The panel draws its own rounded face the same way. The command
      line for the active session is the sidebar's own `SendLine`, so it
      has the same history (↑ / ↓), the same "all connected" target with
      its confirmation, the same hosts left out by "No group send", and
      the same audit trail
- [x] Optional global shortcuts (`RegisterHotKey`), off by default, checked
      against Windows Terminal's bindings
- [x] Per-host/folder tab color (`--tabColor`) and color scheme (applied by
      the shim with OSC, since `--colorScheme` had no effect in 1.26);
      tree shows the color; no per-host profile (the NativeTerm profile
      keeps the title fixed)
- [x] Light / dark / system theme (title bar included), Fluent/MDL2 icons,
      nested folders in the tree, status dots on hosts
- [x] NativeTerm themes (rest): looks on top of light and dark
      (`looks.rs`, Settings -> Look): standard, the Windows accent colour
      for what is selected, soft (less contrast, for a dark room) and
      compact (more hosts on screen). Kept in `settings.toml`, applied to
      every NativeTerm window.
      Mica and Acrylic were tried and don't fit this renderer, which is
      the point of it: the window is painted on the CPU and presented
      with GDI, which has no alpha, and DWM only draws a material behind
      a window that is see-through. Handing the frame over with
      `UpdateLayeredWindow` (as the floating button does) gives alpha but
      no material either: measured on Windows 11 26200, the backdrop is
      simply not drawn behind a layered window, and a window with a title
      bar shrinks by its frame every frame, because that call also sets
      the window's size. A material would mean presenting through
      DirectComposition — a renderer change, not a setting; see
      "Windows 11 materials" in ARCHITECTURE.md
- [x] Localization (English, Simplified Chinese) with runtime switching:
      `native-term-i18n` (Fluent, keys checked at compile time), setting
      in `state.db`, shim follows the system language / `NATIVETERM_LANG`
- [x] Localization (rest): the configuration library says what it
      refuses to write in the person's own language too (its own message
      domain, `native_term_config.ftl`, switched with the program's
      choice), and two more languages: 日本語 and 繁體中文（台灣）.
      998 messages each. Three tests hold them together: every language
      has exactly the English message ids, no translation invents a
      `{ $variable }` the English text doesn't have, and every language
      actually loads and formats (a broken .ftl otherwise fails quietly,
      at run time, in that language only)
- [x] Animations (respecting the system animation setting) and toasts:
      short messages in the corner of the window for what an action did
      when nothing else says it ("closed 5 tabs", "cleared 3 finished
      sessions", "connecting 4 sessions"), fading in and out, a click
      takes one away; problems still stay in the notice line until they
      are cleared. Everything that moves asks Windows first
      (`SPI_GETCLIENTAREAANIMATION`, `desktop::animations`): with
      animation effects off the messages simply appear, and the docked
      window arrives without sliding
- [x] Built-in file transfer over SFTP (`native_term_sftp` over `ssh -s
      sftp`): one SecureFX-style window, local and server sides with a tab
      per session moving together; from a host's or a terminal tab's menu
      (a tmux tab opens at its current folder); transfers by buttons, drag
      between the sides or from Explorer; folders, rename, delete (local:
      Recycle Bin), edit in place with upload on save, any file name
      encoding, saved password or asked in the window
- [x] SFTP folder trees beside both lists (local: Desktop, Documents,
      Downloads, drives; server: `/`), read when opened, opened down to the
      folder shown, drop targets
- [x] SFTP transfers: pause / resume / cancel each or all, resume at the
      byte (`.ntpart` partial files, also after a lost connection or a
      restart), clear finished
- [x] SFTP: last folder per host on both sides (`state.db`), trees scroll
      to the folder shown
- [x] SFTP Synchronize: compare a local and a server's folder recursively
      (size, modification time; times kept by transfers), both ways /
      local is the source / server is the source, optional deletion of
      extras, as ordinary pausable transfers
- [ ] Files over protocols without SSH (FTP/FTPS, WebDAV, S3, cloud
      drives), later and only if wanted. Hosts reached over SSH keep our
      own SFTP client (one experience: ssh config, askpass, pause/resume
      at the byte, non-UTF-8 names). Evaluated 2026-09-19: rclone
      (fork fivetime/rclone) as a back end behind the same window, queue
      and Synchronize dialog, driven through `rclone rcd`. Its progress
      (`core/stats`: bytes, percent, speed, ETA per file) fits the queue,
      but upstream has no pause and no resume (a stopped copy deletes its
      `.partial` file; the next starts at 0), its JSON API turns non-UTF-8
      names into U+FFFD for good (a GBK name can't be named again), the
      SFTP back end with an external ssh hangs ~40 s per remote probing
      `md5sum` (stdin left open), and it is 80 MB (30 MB trimmed to
      local/SFTP/FTP/WebDAV/S3 and `rcd`). Before using it: patch the fork
      to send names' bytes, skip hash probing with an external ssh, trim
      the build, and (if large resumable transfers are needed there) keep
      and continue partial files; download it on demand, not bundled.
      A configured external client (WinSCP etc.) was dropped: they don't
      read the ssh config the hosts are defined in. Details, rclone's
      back ends and API, and how to repeat it: `docs/RCLONE-EVALUATION.md`.
- [x] Shared credential sets (`NativeTermCredential` on hosts and folders,
      `NativeTerm/cred/<name>` in Credential Manager): host dialog, folder
      menu, "Credential Sets…" dialog; refusals mark the set for all hosts
- [x] SOCKS5 / SOCKS4 / HTTP proxies: the shim as the `ProxyCommand`
      helper (`--proxy <url> %h %p`), written as a real `ProxyCommand` so
      every ssh run uses it; set as a type and address in session /
      folder options
- [x] Proxy logins: user name in the URL, password in Credential Manager
      (`NativeTerm/proxy/<url>`); SOCKS5 RFC 1929, HTTP Basic, SOCKS4 user
      id; a refused password is marked and not retried
- [x] NTLM / Negotiate proxy logins (`shim/sspi.rs`): a proxy that
      asks for a Windows login gets one, over the three rounds NTLM needs
      on one connection. Windows makes the tokens, from the credentials
      the person is signed in with when nothing is configured (no
      password to type or store) or from the proxy user name and password
      when there is one. Negotiate is preferred over NTLM where both are
      offered. Where Windows has no credentials to offer — an account
      signed in with a Microsoft account or a PIN — the tab says so and
      what to do instead, rather than a number
- [x] rz / sz (ZMODEM) in every SSH tab: our fork of Win32-OpenSSH
      (`fivetime/openssh-portable`, branch `nativeterm`: a session channel
      filter) hands transfers to `nativeterm-shim --zmodem` (crate
      `zmodem2`): pickers, progress, Esc cancels, `.ntpart` resume.
      Verified live against lrzsz installed on the spot (see
      `docs/RZSZ.md`)
- [x] rz / sz in ntplink sessions (Telnet, serial, raw): the same helper,
      with flow control both ways and every control character escaped
      (Telnet changes CR / NUL / 0xFF); Esc / Ctrl+C cancel through a
      named event; in tmux the transfer is stopped with a note. ntplink also reads the keyboard with VT input now
      (arrows, Esc and F-keys were lost before)
- [x] Files dropped into a tab (`docs/DROP.md`): Terminal owns the drop
      and answers it by pasting the names, so our ssh spots a paste that
      is nothing but paths of this machine, holds it back and asks what to
      do — upload them (the files window, at the tab's folder), or type
      the names after all. The answer can be kept; it is then used only
      when the mouse says a drag ended over the tab, so a pasted path is
      never uploaded behind one's back. Not for ntplink or plink sessions
      (no SFTP there)
- [x] Several files at once in a transfer (`files.at_once`, three by
      default, up to sixteen; the rest queue), shared by every transfer of
      a connection. 120 small files: 11.0 s at one, 5.4 s at three, 2.3 s
      at eight
- [x] Dropping onto NativeTerm's own windows (a session in the list, a
      tab row or picture) as a second way in: the same question as on the
      tab itself (upload over SFTP, or type the names), and no guessing
      about whether it was a drop — Windows tells us directly here
- [x] zmodem2 fixes offered upstream (codeberg.org/jarkko/zmodem2, from
      `fivetime/zmodem2`), one branch each, with tests against lrzsz:
      #8 `abort()` sends the cancel sequence, #9 ZFILE carries the
      modification time and mode, #10 ESCCTL. Used through
      `[patch.crates-io]` (the fork, pinned) until they are released,
      with the shim's workarounds dropped; received files now keep the
      server's modification time
- [ ] rz / sz (rest): ship the fork's ssh with NativeTerm (packaging),
      macOS / Linux builds
- [x] Per-session Backspace mapping (`^H` / `^?`) for non-SSH sessions:
      the session dialog's "Backspace sends", written as `backspace` in
      the `.nt.toml` file and passed to NativeTerm's own client as
      `-nt-backspace`. The console gives 0x08 for Backspace and 0x7F for
      Ctrl+Backspace; `^?` swaps the two on the way out (as PuTTY's
      option does), so both codes stay reachable, and the local echo
      still shows what was typed. plink has no say over the keys, so the
      setting needs ntplink (`patches/ntplink.c`)
- [x] A character set for SSH hosts (`NativeTermCharset`, per host or
      as a folder default), which is what the "optional `plink -ssh` for
      GBK hosts" was for: nothing in the path converts — `ssh` passes
      bytes through and so does PuTTY's plink — so what decides is the
      console's code page, and the shim now sets it for an SSH session
      the same way it already did for Telnet and serial ones. No second
      SSH client, no `.ppk` keys, no Pageant (`charset.rs`)
- [x] Server-side session logging for persistent sessions (`tmux-log`:
      `pipe-pane` started with the session; "Sessions on the Server" shows
      the log as text, saves a copy, deletes it, ended sessions' logs too)
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
      Windows 10 pass (22H2, 19045, English, Terminal 1.24 portable,
      OpenSSH 8.1; see PROTOTYPES "Windows 10 pass"): passed — app,
      wizard, fragment, SSH login signal, UIA tab location, tab menu
      (self-drawn rounded layered popup, MDL2 icons), clone, ntplink
      Telnet with NAWS and Break, GBK on code page 437, re-linking after
      an app restart. Fixed on the way: `--ssh-dir` never reached ssh
      (`-F`), OpenSSH 8.1 leaving the console without processed output,
      a key with bad permissions reported as having a passphrase. Still
      open: the elevated launch through Explorer (the VM's desktop user
      is the built-in Administrator, whose Explorer is elevated too; to
      do: sign in as an ordinary administrator such as `root`, or set
      `FilterAdministratorToken=1` (Admin Approval Mode for the built-in
      Administrator) and reboot, then start NativeTerm elevated and check
      the Terminal comes up unelevated with the tab menu working);
      ARM64; the VC++ runtime (the binaries use `VCRUNTIME140.dll`; the
      one Windows 10 ships, 14.00.24215, was enough here)
- [x] Windows Terminal variant detection: packaged (Store, Preview) and
      unpackaged/portable by path; absolute `wt.exe` per variant; UIA
      windows filtered by process image path
- [ ] Optional bundled portable Windows Terminal (stable ZIP, `.portable`,
      license text) as a fallback package
- [x] "Clean up" (Settings → Clean up…): the Windows Terminal profile,
      which only NativeTerm can take back, is removed from there; and
      everything that stays is listed with its place — the data folder,
      the registry value that points at it, the saved passwords in
      Credential Manager, and every line NativeTerm wrote in the ssh
      configuration, each with its file and line number. The ssh lines
      are never removed (they are what makes the sessions work for
      `ssh` itself) and the passwords and the registry value only on a
      second click. The list can be copied
      (`cleanup.rs`, `native-term-config/src/traces.rs`)
- [x] State "no telemetry" in README and in the settings ("Sends nothing
      anywhere: no telemetry, no update pings, no accounts"), with the
      version beside it
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
- [ ] Session restore on Terminal 1.26: `tests/restore_portable.rs`
      (`restart_and_session_restore`) fails against the portable
      1.26.2581.0, and did before the backend refactor (checked at
      `5cf5256` on a clean state, 2026-09-22). Two things changed in
      Terminal: the persisted layout carries fresh `sessionId`s instead
      of the ones `wt --sessionId` was given (NativeTerm assigned
      `832a920a…`/`fd0f0569…`, `state.json` holds `09eb56fe…`/`0e56713a…`),
      so a restored pane's `WT_SESSION` matches no session and the
      placeholder is told to be a local shell; and closing the window
      with NativeTerm running no longer records "closed with window"
      (`restorable` stays 0 in `state.db`). Both need a look: match
      restored panes some other way (title, position), and find out what
      1.26 does with `WM_CLOSE`. The other two tests in the file pass
- [ ] Cross-platform (see "Platform sequencing" in ARCHITECTURE.md),
      staged so Windows behaves the same after every step:
  - [x] P0 — dependencies gated: the portable crates (`i18n`, `config`,
        `session`, `sftp`, `platform`) `cargo check` on the Linux and
        macOS targets (2026-09-22)
  - [x] P1a — `TerminalBackend` and `OverlayMenu` contracts in
        `native-term-platform`; `Core` holds `Arc<dyn TerminalBackend>`
        and no longer names Windows Terminal (2026-09-22)
  - [x] P1b — `WindowId` newtype in place of the raw `HWND` (2026-09-22)
  - [x] P1c — `FakeBackend` and `core_fake` tests: `Core` tested without
        a terminal, on any platform; `contract::exercise` for every
        backend (2026-09-22)
  - [x] P2a — `native-term-os` facade for the OS helpers: the app no
        longer names `native-term-win`; Unix sides over libc (`localtime_r`,
        `/proc`, `proc_pidinfo`, `mmap`), `notify` for folder watching,
        `xdg-open`/`open`, and `Unsupported` for passwords, the
        wastebasket and global shortcuts (2026-09-22)
  - [ ] P2b — `cfg` splits in the binaries, `NoTerminal` stub, `Icon`
        enum (Segoe on Windows, Phosphor elsewhere): the app `cargo check`s
        on the Linux and macOS targets
  - [ ] P2c — portable-check script in the checklist
  - [x] P3a — the session channel on Unix (`native-term-session::pipe`
        split into `windows`/`unix` behind one API): a socket under
        `$XDG_RUNTIME_DIR`, one instance by connecting first, peer
        credentials checked in `accept`, `close` wakes a reader, a
        write-and-exit client is still read; its tests run on the
        Linux/macOS machines when they come (2026-09-22). Done before P2b
        because the app has no `Core` without a channel
  - [ ] P3b — the shim on Unix (ssh on the inherited tty; typing and
        screen reads through the backend)
  - [ ] P4 — Linux backend: WezTerm (`wezterm cli`; also runs on Windows
        for end-to-end checks before a Linux machine is at hand)
  - [ ] P5 — macOS backend: iTerm2 (JXA through `osascript`)

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
