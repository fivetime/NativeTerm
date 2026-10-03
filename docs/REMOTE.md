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

### Typing into such programs

What a model's client does with input (chat templates, system prompts,
tool formats) happens inside it and is none of the mirror's business: a
device delivers keys as a keyboard does. What matters is the program's
own **input box**: these tools do not use the shell's line editing but
an editor of their own, with ways of their own.

- **Enter sends, a new line is another key** (Shift+Enter, Ctrl+J,
  Alt+Enter, a backslash before Enter: each program its own). Text with
  line breaks sent as keys would send each line. So several lines go as a
  bracketed paste (mode 2004) when the program asked for it; otherwise a
  line break is sent as the program's new-line key from its key set.
  Whether Shift+Enter differs from Enter depends on the program enabling
  kitty's keyboard protocol; WezTerm encodes for the mode the program set.
- **Long pastes folded** by the program ("[Pasted text +40 lines]"): the
  text is there; the mirror shows the fold.
- **Enter right after a paste**: a program without bracketed paste tells a
  paste by its burst of characters, and an Enter inside the burst becomes
  part of it. Enter is sent on its own, once the program has drawn the
  paste.
- **A completion menu takes Enter**: `/commands` and `@files` open menus,
  where Enter picks an entry instead of sending, and changes the text.
  Hence "insert" (the text into the box, no Enter; the person sees the box
  in the mirror) and "send" (insert, then Enter) as two actions.
- **IME**: typing Chinese into a full-screen program in a desktop terminal
  often goes wrong (the candidate window misplaced, the uncommitted text
  lost). From a phone it does not: its own IME and voice input in its own
  text field, and only the committed text sent.
- **Images**: such tools paste images from the desktop's clipboard
  (Ctrl+V). Putting a phone's image there would replace what the person
  has on their clipboard; instead it is saved to a temporary folder on the
  desktop and its path inserted (most tools take a path or `@path`); the
  clipboard only when asked for.
- **Interrupting is costly by mistake**: Esc often interrupts the model,
  Ctrl+C twice often quits the program. The key bar keeps them apart from
  the other keys, and a key set can mark a key "ask first".
- **Predicted echo guesses wrong** where a program draws its own input box:
  off in the alternate screen and wherever the program handles input
  itself.
- **Two people typing in one box** interleave: control decides who types.

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
- **Typing**: a native text field, not a key-by-key keyboard: written with
  the phone's IME or voice, several lines, then **insert** or **send**
  (by the rules in "Typing into such programs"), images and files added
  as paths. Beside it a key bar for keys at once (Esc, Tab, Ctrl, arrows,
  y / n, the person's own keys, key sets per program, interrupting keys
  apart); NativeTerm's saved commands; predicted echo on slow networks
  (as mosh, not in programs drawing their own input box); a full terminal
  on an iPad with a keyboard.
- **Overview**: every open session as a live thumbnail with its state
  (running, idle, waiting, failed).
- **Search, select, copy, open links** on the phone; pinch to zoom.
- **Thrift**: nothing redrawn while the app is in the background; data used
  shown.

Targets, measured in each phase: a key echoed within 100 ms on a LAN and
200 ms through a relay; the current screen within 1 s of opening the app;
back within 3 s after a change of network.

## AI clients through MCP

Outside AI clients (Claude Code, Codex, any MCP client) may use NativeTerm
through MCP, a standard protocol, as chrome-devtools MCP lets them use
Chrome. NativeTerm contains no AI for it: it detects, summarizes and
interprets nothing; it offers what only it has, and shows what a client
sends.

Reading the screen is not the point: an AI running in a terminal already
knows what it printed, and runs commands through its own shell. The
question is what a client **cannot do without NativeTerm**:

- know the person's hosts as they organized them (the session tree,
  folders, tags, notes, jump hosts, the names of credential sets);
- reach sessions the person has logged into (passwords, second factors,
  jump hosts, serial lines, network devices over Telnet), which the
  client has no credentials for;
- the person's attention: tabs they see, the desktop's notifications, and
  through remote control their phone, to tell them something or have them
  decide.

### Who it is for

Little for the leading clients: Claude Code already does phones and
approvals well, and Codex and Gemini will likely follow. A great deal for
the many people and companies running **open models themselves** (Qwen,
DeepSeek, Llama and others, served by Ollama or vLLM, driven by open
agents such as OpenCode, Goose, Cline, Continue): those have no app of
their own, no push, no approvals away from the desk. NativeTerm can be
their phone and their approval channel.

It fits the rest of the design: people who run their own models tend to
want nothing to leave their network, and a self-hosted model, the
self-hosted server, the LAN or their own network, and NativeTerm make a
whole that stays inside a company.

What follows from it:

- **Tools made for weaker models**: few tools, each doing one thing, with
  simple parameters; hosts and sessions named by name, with a list of
  candidates back when a name is ambiguous (no ids to remember); short
  JSON of a fixed shape for small contexts; errors that say what was
  wrong and what to send instead; every call safe to repeat (an id per
  request: sent twice, done once).
- **The CLI matters as much as MCP**: many self-hosted setups call
  functions the OpenAI-compatible way, without MCP; anything that can run
  a command can use `nativeterm` with JSON.
- **Conservative defaults**: weaker models err and are steered by injected
  text more often; level 2's confirmation of every action is not to be
  relaxed for convenience.
- **Measured with them**: tool-call success rates with open models of
  several sizes (about 7B, 32B, 70B), through Ollama and vLLM, with two or
  three open agents; the smallest one using the first set reliably is the
  bar.

### Levels

| Level | What | Offered |
|---|---|---|
| 0 — the inventory, no secrets | find hosts by name, tag, note, folder; the open sessions, their states, which tab is which host | first set |
| 1 — the interface, nothing run on servers | open a tab for a host (logged in by NativeTerm, visible, the person can take over); name and colour tabs; messages, progress and decisions for the person (below) | first set |
| 2 — acting in other sessions | type or run in another logged-in tab; read other tabs' screens and history; SFTP transfers; adding or changing hosts | later, per tab, each action confirmed by the person |
| 3 — never | any secret (passwords, keys, credential sets' content); security settings (opening remote control, unlocking sessions, approving devices, accepting host keys); sending to all sessions; turning logs or audit off | never |

Reading another tab is level 2, not 0: its screen may show a key or a
token.

### Messages for the person

The biggest gain on a phone: what a full-screen program shows is hard to
read there, but a client can send its result formatted for it.

| Tool | For | On the phone |
|---|---|---|
| `post_message` | a summary, a report, a result | a card in the session's "messages" page (beside "terminal"), optionally a notification |
| `post_progress` | a long task's steps and percentage, updated in place | the lock screen (Live Activity, Dynamic Island), a card |
| `ask` | a decision: approve or refuse, or a choice among options, or a short answer | a notification with buttons; a card; the desktop asks too |

Markdown (CommonMark with GitHub's tables, task lists and fenced code):
headings, lists, tables scrolled sideways, code highlighted by language,
diffs in colour, long parts folded. Images only as files the client
attaches (end-to-end encrypted, size-limited); nothing fetched from the
web. Rendered as Markdown only: no HTML, no scripts. The desktop shows the
same cards. They travel in the session protocol (post, update, answer),
end-to-end encrypted; a notification carries the title only, the content
is fetched from the desktop when opened.

### Decisions (`ask`)

1. The client asks: a question, options, and the facts it is about as
   fields (the host, the exact command). Its tool call waits.
2. NativeTerm sends it to the paired devices and asks on the desktop: a
   notification with buttons (no need to open the app), a card, a dialog.
3. The first answer, from any of them, counts; the others show where it
   was answered. The answer goes back to the client.
4. No answer within the timeout (10 minutes by default) is "no answer",
   never a yes; so are a lost connection and a closed app.

- The facts are shown as facts, apart from the client's own words
  (marked "the AI's explanation"), so wording cannot pass for what is
  done.
- Hosts in production (by the person's tag or folder) need Face ID /
  fingerprint to approve.
- Each request has an id and an expiry; the device signs its answer with
  its key; a late or repeated answer is refused.
- Rate-limited per session: no flood of approvals until one is clicked
  without reading.

Some clients can hand their own permission prompts to an MCP tool (Claude
Code has such an option for its non-interactive mode, to be checked);
where they can, their prompts become the same buttons on the phone.
Prompts a client shows inside its terminal stay there, reached through
remote control's "waiting for you" and the key bar.

### Shape

1. **The CLI first**: `nativeterm` subcommands with JSON output, as `gh`
   has them (`hosts`, `sessions`, `open`, `tab`, `post`, `progress`,
   `ask`); people and any AI tool can use them.
2. **The MCP server** over the same operations, on stdio only (started by
   the client; no port another local program could use), each client
   paired once on the desktop; tools annotated (read-only, destructive)
   so the client's own permission prompts apply too.
3. **A skill** (`SKILL.md`): post a summary when done, tables for
   comparisons, `ask` instead of waiting for someone to type y, progress
   updated in place, short cards with details folded.
4. **The remote-control protocol and security model**: the MCP server is
   a local paired device; nothing of a second kind to secure.

### There by default, connected in one click

A client uses these tools when the person has configured them in it: it
asks the MCP server for its tools, and their descriptions tell the model
when to call which. The model need not have heard of NativeTerm; what
counts is whether people set it up, so setting it up must cost nothing.
(Fame matters only to a model using a command it was never told about,
as it knows `gh` from its training; `nativeterm skill` tells it instead.)

**On by default, within NativeTerm itself** (it touches nothing else):

- the `nativeterm` CLI installed with NativeTerm and on the PATH;
  `nativeterm mcp` starts the MCP server, `nativeterm skill` prints the
  whole skill;
- every NativeTerm tab carries `NATIVETERM=1` (and the session's id), so a
  client running in it can tell where it is.

That alone is not enough: clients rarely look at the environment, weaker
models least of all. Hence:

**One click, asked once** (it writes into other programs' settings):

- a step of the first-run guide lists the AI tools found on the computer
  (OpenCode, Goose, Cline, Continue, Claude Code, …) and asks: "Let these
  use NativeTerm (messages, decisions on your phone, …)?"; one click
  writes the MCP entry and the skill into each one's own settings, each
  file backed up first;
- a tool installed later is noticed and offered once, the same way;
- settings list what was written, and remove it (restoring the backups)
  in one action;
- a text the person can paste into a project's instructions (AGENTS.md
  and the like): "In this terminal, ask for decisions with `nativeterm
  ask`."

Never written silently: a program gaining abilities its owner did not
agree to is what NativeTerm never does (nothing silent, nothing opened by
itself), and a change people can see and undo is one they trust.

Default or not, the limits stay: levels 0 and 1 only; level 2 opened per
tab and confirmed per action; level 3 never; the phone only once the
person paired one (the desktop otherwise); rates limited per client.

### Getting known

- Listed where people look for MCP servers: the official MCP registry and
  the common directories.
- Integration notes contributed to open agents' documentation (OpenCode,
  Goose and others), guides for Ollama's and vLLM's users.
- The decision protocol (`ask`: the request, the answer, its signing)
  published as an open specification, so any agent can speak it, and
  NativeTerm is the phone it already has.
- MCP needs people using NativeTerm first, but works the other way too:
  "decisions from any open agent on your phone, one tap" is a reason of
  its own to try NativeTerm, for people and companies running their own
  models.

### Risks

- **Getting round the client's own permissions**: a client told not to
  `ssh prod` could type into a tab already logged into prod. Level 2 is
  therefore confirmed by NativeTerm itself, per action, and its tools are
  annotated as destructive.
- **Prompt injection**: server output can steer a client; level 2 and
  `ask` show the exact command and host, never only the client's words.
- **The wrong target**: sessions are named by their unique id; the person
  sees host, folder and tag colour when confirming; production always
  confirmed.
- **Secrets**: level 3; and a client may put a secret it saw into a
  message: messages go only to paired devices, end-to-end encrypted, but
  what they say is the client's doing (said in the skill).
- **Another local program posing as a client**: stdio only, each client
  paired on the desktop.
- **Several clients in one tab**: the control of remote control.
- **Phishing through messages**: a card says which client and session it
  came from; links show their full address and ask before opening; no
  remote images.
- **Floods**: rates and sizes limited; progress updates one card;
  sessions mutable.
- **Audit**: every call logged on the desktop: which client, when, which
  session, what.

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
- The native text field with insert and send (bracketed paste, the
  program's new-line key, Enter on its own after the paste), images as
  paths; phone-sized while away; the key bar with key sets per program; mouse
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

### RA — AI clients through MCP

- First set, after R1: the `nativeterm` CLI (JSON) and the MCP server on
  stdio with pairing; `NATIVETERM=1` in tabs, `nativeterm skill`; the
  first-run guide's "connect your AI tools" (backups, undo); levels 0 and 1: hosts, sessions, opening tabs,
  naming and colouring them, `post_message`, `post_progress`, `ask` (on
  the desktop; on the phone through the web client, then R5's
  notifications with buttons); the skill.
- Level 2 after R2 (input) and only per tab, each action confirmed.
- Level 3 never.
- Tried first with open models of several sizes and open agents (the
  people it is for), then with Claude Code and Codex; whether a client's
  own permission prompts can go through `ask`, checked.

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
- Which level 2 actions come at all (typing in a logged-in tab, changing
  hosts, transfers).
- Each AI tool's settings format and where it keeps MCP entries and skills
  (found per tool when connecting it is built; some may need their own
  extension instead).
- Whether the self-hosted server is free for everyone, or free for people
  and paid for companies (their features: OIDC / LDAP, audit export,
  recording).
