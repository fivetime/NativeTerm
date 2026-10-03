# Remote control: a terminal tab on the phone and in a browser

Status: design (2026-10-03), not started. It comes after the product is
finished (see `ROADMAP.md`, "Remote control"); each phase below is a step
of its own, with a commit, tests and measurements, as everything else.

## What it is

A terminal tab on the desktop can be **opened to remote control**, each
tab on its own (several can be open), as Claude Code's `/rc` opens a
session. While it is open, a
phone or a browser the person paired shows **exactly what the desktop
shows** in that tab (the text, its colours, the cursor, the scrollback),
and what is typed there is **sent to the desktop and runs there**, as if
typed on the desktop's keyboard.

- Any tab (but a locked one, see Security): a local shell, an SSH
  session, a serial line; whatever runs in it (a build, a deploy, a command-line tool waiting for a yes). Nothing
  of it is about SSH: the mirror is of the terminal, not of a connection.
- The desktop is the only place anything runs. The phone holds no shell,
  no SSH key, no password, no host list, and connects to no server of the
  person's: a lost phone loses a pairing that can be revoked, nothing else.
- A tab not opened to remote control is invisible from outside.
- The desktop decides: it opens and closes remote control, sees who is
  connected and who types, takes the keyboard back, ends everything at
  once. Opening it is never silent or automatic.

The typical case: something long or interactive runs on the desktop, the
person walks away, and from the phone sees how it goes, is told when it
waits or ends, and answers.

### What it is not

- Not a VPN and not remote desktop: no other port, program or window of
  the desktop is reachable; only the sessions opened, only their
  terminals (the one addition, R7: a file sent from a device into a
  session, carried by NativeTerm on the desktop and confirmed there).
- Not an AI feature. NativeTerm has none and gets none here. Command-line
  AI tools are among the programs that gain from it, as any program in a
  terminal does. Outside AI clients may operate NativeTerm through a
  standard interface (MCP, a CLI; see "AI clients through MCP"), as
  chrome-devtools MCP lets them operate Chrome: the AI is theirs, not
  NativeTerm's.
- Not a way to run commands on servers by itself: NativeTerm never probes
  servers, and nothing here changes that.
- Not a remote for NativeTerm itself: a device types into the tabs opened
  to it, nothing else. Opening a host, a new tab, switching tabs stay on
  the desktop (reopening a closed session from a device asks the desktop,
  see Lifecycle).

### Why it is worth doing

- Among command-line AI tools, only Claude Code has a good experience on
  several devices; the others have none or a poor one. Done in the
  terminal, it serves every program in it at once (Codex, Gemini CLI,
  Aider, OpenCode, a build, a deploy), with nothing to adapt in them.
- What exists: Termius with tmux on the server, tmate, upterm (sharing a
  terminal through a server-side program), Teleport (sharing and auditing
  for companies). What sets this apart: nothing installed or run on any
  server; local shells, serial lines and Telnet as well as SSH; the
  desktop's exact screen; a terminal client people already use.
- What can be sold: the hosted service (reaching the desktop from
  anywhere, notifications, read-only links for helping someone), and for
  teams the remote input log's export and the recording of remote
  sessions (made on the desktop, see Security, whichever server carries
  them). Use on the LAN or through the person's own network stays free;
  whether the self-hosted server is free too is an open question.

## Platforms

The mirror is built into NativeTerm's WezTerm fork (macOS, Linux, and
Windows with WezTerm), where every pane lives in WezTerm's multiplexer:

- `Mux::subscribe` reports `PaneOutput(pane)` when a pane changes;
- a pane gives its lines with their attributes (`get_lines`), its size
  (`get_dimensions`), its cursor, and a writer for input (`writer`);
- WezTerm's own remote mux protocol already has `GetPaneRenderChanges`,
  `WriteToPane`, `SendKeyDown`, `SendPaste`: the model to follow (not the
  protocol to expose: it is internal and tied to WezTerm's version).

Windows Terminal (the default on Windows) has no interface to read a tab's
screen or to type into it: remote control needs WezTerm there. Mirroring
NativeTerm's own tabs in Windows Terminal through a second ConPTY in the
shim is possible (it would not cover the person's own tabs; the shim would
then also keep a screen of its own) and stays in "possible, undecided".
Forking Windows Terminal (C++, MIT) would cover everything but is large
and a lasting merge burden; not planned.

## Architecture

```
 phone app / browser (PWA)                 desktop
 ┌─────────────────────┐                   ┌──────────────────────────────┐
 │ renderer (rows)     │                   │ WezTerm fork                 │
 │ key bar, input      │◀── session ──────▶│  mirror service              │
 │ command list, push  │    protocol       │   Mux subscription           │
 └─────────────────────┘   (end-to-end     │   snapshot + row changes     │
            │               encrypted)     │   input → pane (key encoding)│
            │                              │  tabs: "remote" mark         │
            └──── transport ───────────────┤ NativeTerm: pairing, devices,│
   LAN direct │ WebRTC P2P │ TURN │ WS 443 │  indicator, end all          │
                                           └──────────────────────────────┘
              ▲ signaling, STUN, TURN, push: the person's own server
              │ (self-hosted) or the hosted service (opt-in, paid)
```

Three layers, each replaceable without touching the others:

1. **Session protocol** between the desktop and a device: what a screen
   is, what input is. End-to-end encrypted. Knows nothing of WezTerm and
   nothing of the network.
2. **Transport**: how the protocol's frames get there. LAN direct first;
   later WebRTC peer-to-peer, TURN, and a WebSocket relay on port 443;
   last (optional) the same WebSocket relay behind Cloudflare.
3. **Sources** on the desktop: produce screen state and take input. The
   first is the WezTerm fork (in the GUI process for R1-R2; from R3 in the
   mux server, where detached panes live); others (Windows Terminal
   through the shim's ConPTY) could come later.

## Session model

A **session** is one tab opened to remote control. Its id is made when it
is opened; its name is the tab's title.

- **Opening**: the tab's menu ("Open to remote control"), or
  `nativeterm rc` typed in the tab (WezTerm's `WEZTERM_PANE` says which
  tab). NativeTerm shows a window with the QR code and the link for
  pairing a new device; devices already paired see the session at once.
- **While open**: the tab carries a mark in the tab strip ("● remote");
  its hover card says who is watching and who types.
- **Ending**: the menu again, or `nativeterm rc off`; closing the tab;
  NativeTerm quitting; the main window's "end all remote control"; the
  desktop being away longer than the grace period. Devices are told why.
- A tab split into panes: the active pane first; the whole layout later.

### Lifecycle

| What happens | The session | The device shows |
|---|---|---|
| Network gone briefly (either side, under the grace period, 60 s by default) | kept; reconnects by itself | "Reconnecting…", then what came meanwhile |
| Desktop asleep, off, offline beyond the grace period, NativeTerm quit | ended (a detached session too: its shell may live on in the mux server, its remote control does not) | "Ended: the desktop is offline"; what it had received stays readable |
| Tab closed, or remote control turned off | ended at once | "Ended: the tab was closed" / "…turned off" |

The **terminal** is not the session: a command keeps running on the
desktop when the remote session ends.

While any session is open the desktop is kept awake (and told so; on
battery the person is warned), since its sleeping ends every session.

### Closing, detaching, resuming

| The tab's kind | Closed: what resume gives | Detached: resumed whole? |
|---|---|---|
| Local shell in the WezTerm mux server (phase R3) | a new shell; the old output to read | yes: processes, directory, scrollback |
| Local shell in the GUI process (today) | a new shell; the old output to read | no |
| SSH with the host kept on the server (tmux, an existing option) | reconnects and attaches the same tmux session | yes (server side) |
| Plain SSH, Telnet | a new login; the old programs have ended | no |
| Serial | the port reopened; what came meanwhile is lost | partly |

Resume always says which of these it is. With a device connected:

- closing a tab that can be detached asks "detach (the device stays) or
  close (it is disconnected)?", detach chosen; one that cannot be detached
  warns that the device is disconnected and its programs end;
- a detached session stays usable from the device while NativeTerm runs;
  the desktop lists detached sessions, to bring one back as a tab or end
  it. A detached pane has no tab in the GUI, so from R3 the mirror runs
  where the panes live (the mux server), not in the GUI process;
- from a device, resuming a **detached** session is allowed; reopening a
  **closed** one asks the desktop (it starts a connection);
- after detaching, remote control stays on; a closed tab reopened starts
  with it off.

When the desktop comes back (awake again, NativeTerm started again), the
devices stay paired, nothing to scan again; but no session is reopened by
itself: remote control is opened on the desktop again, so a desktop never
becomes reachable just by starting.

## Session protocol

Versioned from the first frame: the phone app, the web client and the
desktop are updated separately, and each side says what it supports.

- **Frames**: length-prefixed binary messages from a written schema, so
  that Rust, Swift, Kotlin and TypeScript decode them alike (protobuf or
  CBOR: decided in R0 by measuring size and decoding cost).
- **Encryption**: end-to-end, independent of the transport (Noise, with
  each device's static key exchanged when paired through the QR code). A
  server in between, ours, the person's or a CDN, sees ciphertext and a
  routing header (which device), nothing else. TLS is never relied on for
  this: a CDN ends TLS at its edge.
- **Hello**: versions, capabilities, the device's identity.
- **Sessions**: the list of open sessions; subscribe to one.
- **Screen**: a snapshot (rows, cursor, size, title) then changed rows with
  a sequence number; a row is text with styled runs (colours, bold,
  italic, underline, inverse, hyperlinks). The device asks for scrollback
  when it scrolls; the desktop keeps where each device last looked ("new
  since you left").
- **Input**: text (what the phone's IME committed, never its composition),
  paste, and abstract keys (a name and modifiers). The desktop encodes a
  key with WezTerm's own encoding for the pane's current modes, so a key
  from the phone is the same bytes as from the desktop's keyboard.
- **Size**: the desktop's by default: a device zooms and pans, and never
  shrinks the tab by connecting (tmux's "the smallest client wins" would
  squeeze the desktop's 200 columns to a phone's 50). On request, the tab
  takes the phone's columns and rows while the person is away
  (full-screen programs lay themselves out again), and goes back.
- **Control**: who types. By default whoever typed last holds it, the
  desktop's keyboard can always take it back, a session can be read-only.
- **Pointer**: taps and scrolling sent as mouse events (click, wheel) when
  the program in the pane asked for mouse reporting; otherwise scrolling is
  the device's own, through the scrollback.
- **Events**: bell, terminal notifications (OSC 9 / OSC 777 / OSC 99),
  command marks (OSC 133: start, end, exit code), the session ending and
  why.
- **Pictures**: images in the terminal (sixel, kitty's and iTerm2's image
  protocols) as cells that stand for an image, the image sent once and
  referred to; until then (R1, R2) a placeholder where one is.
- **Reliability**: heartbeats well inside any proxy's idle timeout (30 s;
  Cloudflare closes idle WebSockets after about 100 s); every message
  sequenced, a reconnect resumes from the last one seen; snapshots split
  into small frames; no connection is assumed to last. **Input typed while
  disconnected is not queued**: replayed later it would land on a screen
  that has changed.

The screen is synchronized as state (as mosh does), not as the terminal's
byte stream: the device shows what WezTerm computed, so the two never
disagree on a sequence one of them handles differently, and a device that
joins late or comes back has the current screen at once.

### What the program in the tab sees

Programs find out what their terminal can do by its environment (`TERM`,
`TERM_PROGRAM`, `COLORTERM`, `WEZTERM_PANE`, `WT_SESSION`), by terminfo,
and by asking it and waiting for the answer (DA1 / DA2, XTVERSION
`CSI > q`, DECRQM for a mode: 2004 bracketed paste, 1049 the alternate
screen, 2026 synchronized output; `CSI ? u` for kitty's keyboard protocol;
XTGETTCAP; OSC 10 / 11 for the colours); many sequences (OSC 8, 9, 133)
they send anyway, since a terminal that does not know one ignores it.

All of it is asked of the desktop's WezTerm and answered by it: a device
watching changes nothing a program sees, and a device never has to
understand an escape sequence, only to draw what WezTerm made of them (its
underline styles, hyperlinks, colours, images). Nothing is negotiated with
the device about the terminal; the protocol's own capabilities (Hello) are
about the protocol only.

### Programs with their own interface (full-screen and inline TUIs)

Command-line tools that draw their own interface (AI coding assistants
among them) fold what they show themselves ("+40 lines", a key to
expand). There is no adopted standard for a terminal to fold a part of a
program's output (OSC 133 folds a whole command's output, nothing inside a
program): the folded text is in the program, not in the terminal. The
mirror shows the fold as the desktop does; what it means on the device:

- **Expanding is the program's**: the device sends the program's key, the
  program draws the rest, the mirror follows. Hence **key sets per
  program** in the key bar (expand, history, interrupt, approve, a new
  line without sending): the person's own, plain keys, nothing
  program-specific in NativeTerm's code.
- **No terminal scrollback in the alternate screen**: a full-screen program
  keeps its history itself. In that screen with mouse reporting on, a
  swipe is sent as wheel events and the program scrolls; "new since you
  left" and the command list apply to shells, not inside such programs.
- **Taps** reach the program as clicks when it asked for the mouse (some
  expand a fold on a click).
- **Pasting several lines** is sent as a bracketed paste (mode 2004) when
  the program asked for it, so the lines are one input, not one command
  each.
- **Phone-sized**: such programs often draw their whole conversation again
  when the size changes: a burst of changed rows, carried as any other.
  Tables and diffs are wrapped by the program at the phone's width.
- **Redrawing all the time** (spinners, timers): changes are gathered and
  sent at most 20-30 times a second, none while the app is in the
  background (events only). With synchronized output (mode 2026) a frame
  is sent when the program says it is complete: no half-drawn interface,
  fewer updates.
- **No command marks inside them**: notifications come from the bell and
  OSC 9 / 777 / 99 when the program sends them, else from the person's
  keywords.
- **Copying** what is folded needs it expanded first.

## Security model

- Pairing by QR code on the desktop, confirmed there; one key pair per
  device; devices listed and revocable on the desktop.
- Remote control off by default, per tab, never started by itself; the
  desktop always shows that a tab is open and who is connected; one action
  ends everything.
- Read-only per session; input can be refused per device.
- The phone app asks Face ID / fingerprint when opened and before input.
- Passwords: the ones NativeTerm asks are asked in its own windows, which
  are not terminals and never mirrored. A password typed into a terminal
  (sudo, a plain `ssh` in a local shell) is not echoed, so it is not on
  any screen to mirror; what must not keep it is the remote input log,
  which writes "(hidden)" instead of the text while the terminal is in a
  password state (canonical input with echo off, as `getpass` sets it).
  Echo off alone is not that state: full-screen programs run with it off
  all the time, and their screens are mirrored as any other.
- Locked sessions (an existing NativeTerm flag) cannot be opened.
- A log on the desktop of remote input: which device, when, what.
- Recording of remote sessions (teams) is made on the desktop, which has
  the screens anyway, and stored where the company says; never by a
  relay, which only ever sees ciphertext.
- Servers hold no terminal content, ever; the hosted service keeps
  accounts, devices' public keys and subscriptions only.
- Read-only links (R7) are a temporary device: a key in the link's
  fragment (never sent to a server), read-only, an expiry, and the desktop
  asked when it is first used.

## The experience on the phone

- **Phone-sized while away**: the tab takes the phone's size (see Size),
  back to the desktop's with one action.
- **Commands, not pages**: with OSC 133 marks, a list of the commands with
  their exit status and time; a tap goes to the output; long output folds.
  Local shells get the marks from NativeTerm's shell integration; remote
  shells only if the person sets that up on the server (nothing is run
  there for them).
- **Told when it matters**: "waiting for you" (bell, OSC 9 / 777 / 99) and
  "finished" (OSC 133, with exit code and duration); keywords the person
  sets ("Allow?", "ERROR"); per-session mute. Replies from the
  notification itself; long tasks on the lock screen (Live Activities,
  widgets; an ongoing notification on Android).
- **Typing**: a key bar (Esc, Tab, Ctrl, arrows, y / n, the person's own
  keys, key sets per program); NativeTerm's saved commands; voice and IME
  text; predicted echo on slow networks (as mosh); a full terminal on an
  iPad with a keyboard.
- **Overview**: every open session as a live thumbnail with its state
  (running, idle, waiting, failed).
- **Search, select, copy, open links** on the phone; pinch to zoom.
- **Thrift**: nothing redrawn while the app is in the background; data used
  shown.

Targets, measured in each phase: a key echoed within 100 ms on a LAN and
200 ms through a relay; the current screen within 1 s of opening the app;
back within 3 s after a change of network.

## AI clients through MCP

Outside AI clients (Claude Code, Codex, any MCP client) may operate
NativeTerm's terminals, as chrome-devtools MCP lets them operate Chrome.
MCP is a standard protocol; NativeTerm contains no AI for it.

What exists already: `wezterm cli` (`list`, `get-text`, `send-text`,
`spawn`) lets a tool read and type into WezTerm's panes today. What
NativeTerm adds:

- its own notions: the session tree's hosts, opening one by name, the
  sessions and their states, SFTP transfers;
- operations that fit how such clients work: **run a command and wait for
  it to end** (OSC 133: its output, exit code and duration, instead of
  reading the screen again and again to guess), **wait for a text** to
  appear, read the screen or a command's output as text;
- the control that makes it safe (below).

### Shape

1. **The CLI first**: `nativeterm` subcommands with JSON output, as `gh`
   has them: `sessions`, `read`, `run`, `wait`, `send`, `open <host>`,
   `files`. People use them, and so can any AI tool through a shell.
2. **The MCP server** is a thin layer over the same operations (stdio,
   started by the client), with a skill (a `SKILL.md`) saying how to use
   them well: run-and-wait over send-then-read, read before typing, never
   type into a session not opened to it.
3. **The same protocol and security model as remote control**: the MCP
   server is a local paired device speaking the session protocol (session
   list, snapshot, input, events). Nothing of a second kind to secure.

### Safety

Terminal output comes from servers and is not to be trusted: an AI that
reads a screen can be steered by what is written on it (prompt
injection) into typing a dangerous command on a production server. So:

- **Opened per tab, as remote control**: "open to AI clients", off by
  default; a tab not opened is invisible to them.
- **Read-only by default**; typing needs the tab's own permission.
- Optionally every command confirmed on the desktop first, or only
  commands matching the person's allowlist.
- Locked sessions, and folders marked "no group send", are never open to
  them.
- **Never a secret**: NativeTerm gives no password, key or credential to a
  client; its password windows are not reachable; the password state of a
  terminal hides input from the log as for devices.
- Every input written to the remote input log, marked as from an AI
  client; the desktop shows that one is typing and takes the keyboard
  back with one action.
- Nothing run on servers by NativeTerm for it: the client types into the
  terminal as a person would; never probing stays true.

### Tools (first set)

| Tool | Does | Needs |
|---|---|---|
| `list_sessions` | sessions open to clients: name, host, state, size | read |
| `read_screen` | the screen (and scrollback on request) as text | read |
| `read_command` | a command's output, exit code, duration (OSC 133) | read |
| `wait_for` | until a text appears, a command ends, or a timeout | read |
| `run_command` | type a command, wait for it to end, return its result | input |
| `send_keys` | text and keys (as the protocol's abstract keys) | input |
| `open_host` | open a host of the session tree in a new tab (asks the desktop) | desktop |
| `transfer` | upload or download through NativeTerm's SFTP (asks the desktop) | desktop |

## Servers

Needed only to reach the desktop from outside its network and to notify a
closed app. The LAN needs none. Every server is available **self-hosted**
(one program or a compose file) as well as hosted; the hosted service is
opt-in and paid, and the free use needs no account (NativeTerm's promise:
no telemetry, no account, nothing sent anywhere, stays true unless the
person turns the hosted service on).

Without any server of ours: the LAN, or the person's own network across
places (Tailscale, ZeroTier, WireGuard): NativeTerm treats it as a LAN, no
account needed. Forwarding a port on a router to the desktop is not
offered and is advised against: it puts the entry on the public internet.

For companies (with the self-hosted server, or the hosted one): sign-in
through their own identity provider (OIDC / LDAP), the remote input log
exported for their audits, recording of remote sessions where they ask
for it (made on the desktop, see Security).

| Part | Does | Notes |
|---|---|---|
| Accounts and devices | sign-in, devices and their public keys, pairing, revocation | the least data; passkeys; phone numbers where needed |
| Signaling | presence (the desktop online), exchange of connection data | WebSocket, light, routed by device id |
| STUN / TURN | hole punching; relaying ciphertext where it fails | the bandwidth cost; several regions; TURN over TLS 443; short-lived credentials (coturn) |
| WebSocket relay | the last fallback: ciphertext over HTTPS 443 | for networks that let only HTTPS through |
| Push | APNs, FCM, Web Push; quick replies routed back to the desktop | the least content; vendors' channels in mainland China (FCM is unavailable) |
| Web client | the PWA's files | versioned with the protocol |
| Billing | plans, payments, metered relay traffic | iOS in-app purchase rules |

Built in Rust (axum / tokio), sharing the protocol crate with the desktop;
PostgreSQL for accounts; coturn for TURN; containers in at least two
regions. Monitoring of connection success, direct versus relayed share,
latency and push delivery; logs of metadata only; rate limits keyed on the
real client address; abuse limits on relays; DDoS protection; regular
penetration tests; a written response plan for a compromised key.

## Phases

Each phase ends usable and measured; none needs the ones after it.
**Cloudflare comes last and nothing depends on it.**

### R0 — protocol and session model

The protocol's schema and versioning, the encryption handshake, the
session lifecycle, written and tested (encode / decode on Rust and in a
browser; a desktop and a device simulated in tests: snapshot, changes,
reconnect from a sequence number, input not queued while disconnected).
The frame format chosen by measuring.

### R1 — LAN, read-only, one tab

- The mirror service in the WezTerm fork: Mux subscription, snapshot and
  row changes, served on the LAN only.
- Opening from the tab's menu and `nativeterm rc`; the "remote" mark; the
  pairing window with the QR code; the device list; end all.
- The web client (PWA, renders rows; xterm.js or a thin cell renderer:
  measured) in a phone's browser; images as placeholders.
- Measured: latency, data used, the snapshot's time.

### R2 — input, control, the phone basics

- Input: text, paste, abstract keys encoded by WezTerm for the pane's
  modes; control (whoever typed last, the desktop takes it back,
  read-only); the remote input log on the desktop.
- Phone-sized while away; the key bar with key sets per program; mouse
  events for programs that asked for them (taps, wheel); bracketed paste;
  updates gathered (20-30 a second, synchronized output); scrollback and
  "new since you left"; the command list (OSC 133); reconnection within
  the grace period; the desktop kept awake.
- Several devices, revocation, the lifecycle table above, end to end.

### R3 — detach and resume

- Local panes in WezTerm's mux server (`wezterm-mux-server`, a unix
  domain), so closing a tab can detach it; measured first: start time,
  input latency, Windows. The mirror moves there with them.
- SSH resume through the existing server-side tmux option.
- The questions when closing a tab with devices connected; the list of
  detached sessions; resume from a device (detached only) and from the
  desktop.

### R4 — the private channel, self-hosted

- WebRTC between desktop and device (data channel; ICE with STUN; TURN
  when direct fails), the session protocol's encryption on top.
- The WebSocket relay on 443 as the last fallback (no CDN).
- The self-hosted server: signaling, STUN / TURN, the WebSocket relay; one
  program or a compose file, documented.
- Tested on a set of networks: home NAT, mobile carrier NAT, a company
  firewall that passes only HTTPS, IPv6 only.

### R5 — the apps, notifications, the hosted service

- Images in the terminal shown on the device.
- iOS and Android apps (the web client stays): Face ID / fingerprint,
  notifications (waiting, finished, keywords), replies from notifications,
  Live Activities and widgets, the overview, voice and IME input, predicted
  echo.
- Push through the server (APNs, FCM, Web Push).
- The hosted service, opt-in: accounts, devices, billing, metering, two
  regions, monitoring, status page.

### R6 — mainland China (if it is a market)

Vendors' push channels (Huawei, Xiaomi, OPPO, vivo, Honor) or the unified
push alliance; ICP filing for domains and servers there; a separate region
for data; relays measured between the mainland and abroad.

### R7 — sharing and working together

Read-only links with an expiry for helping someone; several people
watching with who holds control shown; sending a file from the phone into
a session (through NativeTerm's SFTP).

### RA — AI clients through MCP (after R2)

The `nativeterm` CLI subcommands with JSON output; the MCP server over
them as a local paired device; the skill; "open to AI clients" per tab,
read-only first, input with its own permission, optional confirmation or
allowlist, the AI marked in the input log. Read-only tools could come as
early as R1 (screen, scrollback); typing needs R2's input. Tried with
Claude Code and Codex.

### R8 — Cloudflare (last, optional)

Only what Cloudflare can carry, and only as an addition:

- the web client served from its CDN;
- signaling and the WebSocket relay behind its proxy (WebSocket works on
  every plan): the heartbeat, resume and small frames from R0 are what
  make this work, since it closes idle connections (about 100 s) and resets
  connections when its edges are updated;
- rate limits on `CF-Connecting-IP`;
- optionally its managed TURN service.

What it cannot do: UDP is not proxied, so WebRTC direct traffic and
STUN / TURN never go through it (TURN only through its paid products);
large non-HTML transfers are against its self-serve terms; TLS ends at
its edge (harmless: the protocol is end-to-end encrypted). Whether it
speeds up a desktop abroad with a phone in mainland China is measured,
not assumed: without its China network (enterprise, ICP), mainland users
reach edges in Hong Kong, Japan or the US, sometimes slower than direct.

Cloudflare Tunnel (cloudflared on the desktop, a public hostname behind
Cloudflare Access) works too but puts the entry on the public internet;
it is documented as the person's own choice, not offered by default.

## Alternatives considered

- **Pixels (VNC, as a VM's tty1 through noVNC)**: exact, but heavy on
  mobile networks and batteries, a 200-column terminal unreadable at a
  phone's width, no text to select, awkward IME input (keysyms), screen
  recording permissions (macOS) and restrictions (Wayland). Not used.
- **The byte stream (as OpenStack Zun's wsproxy does to a container's
  attach stream, xterm.js in the browser)**: light, but a device that
  joins has no current screen until something is drawn again, and two
  different parsers (WezTerm, xterm.js) can disagree. Its topology is kept
  (a WebSocket service, one-time tokens, a browser client); the payload is
  WezTerm's screen state instead. "Snapshot plus byte stream" may serve
  for a first prototype, measured against the rows.
- **A system VPN of NativeTerm's own**: needs drivers and administrator
  rights (and Apple's approval on iOS), takes the phone's only VPN slot,
  reaches far more than the terminal, and cannot run in a browser. The
  private channel is per application instead (WebRTC, in browsers too).
- **Exposing the desktop**: a forwarded port, or Cloudflare Tunnel behind
  Cloudflare Access: works, but on the public internet; documented as the
  person's own choice at most.
- **Caching the last screen on a server for an offline desktop**: no. The
  desktop gone, the session has ended; servers hold no terminal content.

## Effort and costs (rough, not measured)

| Phase | Estimate |
|---|---|
| R0-R1 | 1-2 weeks after the protocol is written |
| R2 | 4-6 weeks |
| R3 | to be measured first (the mux server) |
| R4 | 4-6 weeks, plus about 2 for the self-hosted package |
| R5 | the apps several weeks each; the hosted service 6-8 weeks, plus 1-2 months for billing, regions, monitoring |
| R6 | 1-2 months, plus the ICP filing's own time |

The hardest part of R4 is not writing it but making it hold on every kind
of network (company firewalls, carrier NAT, HTTPS-only), which only tests
on those networks settle. Lasting costs of a hosted service: servers and
bandwidth (TURN above all; the more connections go direct, the less),
developer accounts and push certificates for each platform, store fees on
in-app payments, and someone on call for it, security first.

## Open questions

- Windows Terminal: no remote control there, or the shim's second ConPTY
  for NativeTerm's own tabs (R1 finding decides).
- Local panes in the mux server (R3): its cost, measured before deciding.
- Protobuf or CBOR (R0).
- Where the hosted service is paid for: in the apps (store rules and fees)
  or on the website / the desktop.
- Whether mainland China is served (R6).
- MCP's place: after R2 as proposed, or its read-only tools right after
  R1.
- Whether the self-hosted server is free for everyone, or free for people
  and paid for companies (their features: OIDC / LDAP, audit export,
  recording).
