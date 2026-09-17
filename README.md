# NativeTerm

A SecureCRT-style session manager for people who manage hundreds of
servers, built on the terminal their operating system already has.

NativeTerm is **not** a terminal emulator. It never renders a single
character of terminal output itself. Instead it:

- reads your existing `~/.ssh/config` (and `~/.ssh/config.d/*.conf` via
  `Include`) as the single source of truth for saved sessions — no separate
  database, no re-entering credentials you already have configured
- also manages Telnet, serial, and raw sessions (e.g. switch consoles) by
  running PuTTY's console client `plink` in the tab, with a per-session
  character set for legacy GBK devices
- opens every session as a tab in the Windows Terminal window you're
  already using (or, on request, a new window), next to your own tabs (later: Terminal.app / Linux
  terminals), so rendering, fonts, Ctrl+click links, and input methods are
  the OS terminal's own — including the rich output of AI coding tools
  that legacy SSH clients can't display
- drives that window's tabs from the outside (`wt` command line + UI
  Automation) and runs a tiny per-tab helper (`nativeterm-shim`) around
  `ssh`, which gives SecureCRT-style operations: close others, close
  disconnected, close to the right, reconnect in place, clone, send
  commands, and a searchable tab switcher

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for how and why, and
[`docs/ROADMAP.md`](docs/ROADMAP.md) for what's built vs. planned.

## Why

Existing multi-server SSH managers are either:

- native and single-platform (SecureCRT), whose rendering can't keep up
  with the rich output of today's terminal tools, or
- cross-platform and Electron/web-rendered. Good ones exist (e.g. Tabby),
  but they aren't built for managing hundreds of hosts (~800 sessions in
  ~200 folders here), and they cost more memory: in our measurement about
  twice as much per open session as the native approach
  (see [`docs/PROTOTYPES.md`](docs/PROTOTYPES.md))

## Portable, no telemetry

NativeTerm is primarily a portable app: by default its settings and logs
live next to the executable, and the data directory can be moved anywhere
(OneDrive, a network share, a USB drive). An installed mode uses the same
code with a per-user data directory. It collects no telemetry of any kind.

## Status

Early development. Phase 0 prototypes are done (`prototypes/`, results in
[`docs/PROTOTYPES.md`](docs/PROTOTYPES.md)). A first working slice
exists: it reads the session tree from `~/.ssh` (search, create, edit,
move, delete hosts and folders), opens hosts as tabs of Windows Terminal
(in the current or a new window), tracks their state and position from
Terminal's events, and focuses, reconnects, and closes them. Sessions are
recorded in `state.db`; tabs restored by Terminal are replaced by proper
sessions. Right-clicking a NativeTerm tab opens NativeTerm's own menu.

```
cargo build --release -p native-term-app -p native-term-shim
target\release\nativeterm.exe [--terminal-dir <portable Terminal>] [--ssh-dir <dir>]
```

The chosen Terminal needs the "NativeTerm SSH" profile; the app offers to
install it (a Terminal fragment) under Settings.

## License

MIT
