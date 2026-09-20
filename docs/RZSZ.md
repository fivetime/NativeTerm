# rz / sz (ZMODEM) in every session

Built and verified live on 2026-09-20 (Windows; the fork's `nativeterm` branch plus the shim).

## What is wanted

The user's habit: in whatever SSH session they are in, `apt install lrzsz`
and use `rz` / `sz` right away. So rz/sz only counts if it works in every
interactive tab without preparation: no per-host switch, no reconnect, no
config change. (A per-host opt-in engine — `tssh`, or ntplink with PuTTY's
SSH — was proposed and rejected for that reason.)

## Why it needs our own ssh

ZMODEM travels inside the terminal's byte stream; the terminal program has
to spot it and take over. Windows Terminal doesn't implement it, and with
Windows' own `ssh.exe` NativeTerm is not in that stream (Terminal ↔ ssh
directly; the shim only starts ssh). Putting something in between:

- a ConPTY in the middle (what `trzsz` does on Windows) re-renders the
  output, which mangles ZMODEM's binary data; trzsz uses its own text
  protocol (`trz`/`tsz`) for that reason;
- ssh with pipes instead of a console loses window resizing.

So every interactive tab's SSH client must be one NativeTerm controls.

## Chosen: our fork of Win32-OpenSSH, plus a Rust helper

- **Fork:** `fivetime/openssh-portable` (a fork of
  `PowerShell/openssh-portable`), local clone
  `C:\MyProjects\RustProjects\openssh-portable`, remotes `origin` (fork),
  `upstream-pwsh` (PowerShell, branch `latestw_all`), `upstream`
  (openssh/openssh-portable). Work branch `nativeterm`, from
  `latestw_all`.
- **Base version:** PowerShell's `latestw_all` (10.2p1 as of 2026-09-15).
  PowerShell lags upstream by about 10–11 months (10.1p1 and 10.2p1 of
  Oct 2025 merged Aug/Sep 2026; upstream is at 10.5p1 of 2026-08-11).
  Decided to stay on PowerShell's base: the client-side fixes in
  10.3–10.5 found in the log are a use-after-free on a `cipher_init()`
  failure path, one with multiplexing sockets (not on Windows), an X11
  NULL crash, and leaks — none pressing. Upstream security releases get
  cherry-picked when they matter; a weekly workflow can flag them.
  (Catching up to 10.5 was tried in a scratch clone: 52 conflicting files,
  most generated or packaging; 26 real hunks in 13 sources; new crypto
  files for the Visual Studio projects. Not needed now.)
- `latestw` is PowerShell's old Windows branch, stuck at 7.9 (2018) and
  fully contained in `latestw_all`; other branches are stale too.
- **The patch** stays small and apart, like the PuTTY fork's `patches/`:
  an output filter on the session channel (`channel_register_filter`,
  `channels.c`; `clientloop.c` registers the `~` escape filter the same
  way) spots a ZMODEM start, runs `nativeterm-shim --zmodem` and pumps
  bytes between the server and it until it ends. New code in its own
  files, a few hook lines elsewhere.
- **The helper** (shim, Rust) does the protocol with the `zmodem2`
  crate, the file / folder dialogs, progress and cancel. ntplink (serial,
  Telnet) can hand over to the same helper.
- Same patch builds for macOS / Linux later (the client code is shared).

## zmodem2 (crates.io) — evaluated

`zmodem2` 0.7.2 (MIT OR Apache-2.0, Jarkko Sakkinen, active: 0.7.2 of
2026-08-05): sans-IO (`poll()` returns actions: write wire, read / write
file, events), `no_std`, heapless, deps `bitflags`, `hex`, `thiserror`.
Sender and receiver, CRC-32, ZRPOS resume, skip, batches, lrzsz interop
tests. Files up to 4 GB (ZMODEM's own 32-bit offsets). Others: `rzsz`
(Apache-2.0) is a server-side lrzsz replacement for Unix; `lrzsz2` is
GPL-3.

Tested against real lrzsz 0.12.21rc (Ubuntu 24.04 container) over
`ssh -T host "sz …" / "rz …"` (a prototype, no terminal in between):

- download: 50 MB random + a Chinese file name + an empty file, all
  SHA-256 equal; 1.2 MB/s on a link where `ssh cat` gives 1.3 MB/s;
- upload: 30 MB + a Chinese name + empty, all equal; 0.8 MB/s with the
  default window (an ack wait every 10 KB) → 8.8 MB/s with
  `set_streaming_window(usize::MAX)` (the link: 8.4 MB/s);
- resume: a download stopped at 12 MB and run again was accepted at
  12,000,256 (`set_manual_file_accept` + `accept_file_at`) and finished
  with the right SHA-256;
- to watch: after aborting, ssh and the remote `sz` took ~30 s to exit.

## How it works (built)

**ssh (fork, `nativeterm/nt_zmodem.c`, registered in `clientloop.c`)** —
only when `NATIVETERM_ZMODEM` names the helper (the shim sets it for the
ssh it starts; any other ssh ignores it):

- the session channel's output filter looks for a ZMODEM hex header,
  ZDLE `B` `00` (ZRQINIT: the server's `sz`, a download) or `01` (ZRINIT:
  its `rz`, an upload); text before it goes to the terminal; a header cut
  off at the end of a write is held back for the rest;
- there it starts `nativeterm-shim --zmodem download|upload` (posix_spawn,
  its stdin / stdout pipes) and swaps the channel's rfd / wfd to them: the
  server's data goes to the helper, the helper's to the server, and the
  keyboard isn't read meanwhile; the helper keeps the console (stderr for
  progress, CONIN$ for Esc / Ctrl+C);
- the helper ends by writing raw XOFF (`\x13\x13\x13\x13`), which never
  appears raw in ZMODEM data (XOFF is always ZDLE-escaped): the input
  filter forwards what comes before it and gives the terminal back. The
  write end of the helper's stdout stays open in ssh, so a helper that
  dies without the marker never reads as the keyboard's end (which would
  close the session's input); the next server data finds it gone;
- the `~` escape filter is kept (wrapped, with its own context) and
  bypassed during a transfer (binary data could contain `\r~.`);
- cosmetic: sz's `rz\r` before the header is not shown (held back at the
  end of a write, blanked when it is a write of its own); the sender's
  closing `OO` within a second after a download is blanked (NUL, which
  terminals ignore; the write length can't change);
- Windows: ssh's console reader thread is already waiting for a key when
  the helper starts, so it would take the first key (the user's Esc). A
  stand-in key (`WriteConsoleInputW`) satisfies it; keyboard data read in
  the 0.3 s after the terminal gets the session back is dropped (the
  stand-in, keys pressed during the transfer).

**The helper (shim `zmodem.rs`, crate `zmodem2`)**:

- download: a folder picker (last folder remembered in
  `HKCU\Software\NativeTerm\Zmodem`, first time Downloads), or
  `NATIVETERM_ZMODEM_DIR`; each file goes to `<name>.ntpart` and gets its
  name when complete (`name (2).ext` when taken); a `.ntpart` left by a
  cancelled or broken transfer is continued (`accept_file_at`); names:
  the last path component, characters Windows refuses as `_`;
- upload: a multi-file picker, or `NATIVETERM_ZMODEM_FILES` (`|`-separated);
  files over 4 GB are left out with a note (use Files (SFTP));
  `set_streaming_window(usize::MAX)`;
- the pickers (`native_term_win::picker`) are owned by a hidden topmost
  window of the helper's own process: never a Terminal window (a picker
  owned by one disables it, and a killed picker left it disabled — why an
  earlier picker was removed), and it comes up in front;
- progress on the console, a line per file (`12.3 / 50.0 MB  1.3 MB/s`);
- Esc or Ctrl+C cancels: through ConPTY the key arrives as its character
  only (`vk=0, ch=0x1b`), so both are checked; `NATIVETERM_ZMODEM_DEBUG=<file>`
  logs the console's key events;
- workarounds for zmodem2 0.7.2 (to report upstream):
  - `abort()` sends nothing to the other side: the helper sends the
    cancel itself (ten CAN, ten backspaces, as lrzsz), then takes in what
    the sender still streams until it is quiet for 0.5 s (at most 15 s),
    so it isn't shown as garbage (whose escape sequences made the terminal
    answer into the shell); the shell's prompt can go with it, so the
    note says to press Enter;
  - the ZFILE subpacket carries only "name\0size\0": rz then sets a
    garbage time and mode 0600 (a file dated 2486). The helper puts
    "\0size mtime(octal) 100644" into the name it hands zmodem2
    (`zfile_name`), which lrzsz reads; the name alone when it wouldn't fit
    zmodem2's 256 bytes;
  - on SessionCompleted the closing ZFIN may still be queued: flushed
    before returning (else sz waits and times out).

`native_term_session::ssh_program()` prefers `openssh\ssh.exe` next to the
program (NativeTerm's own build, to ship with it) after `NATIVETERM_SSH`.

**Verified live** (portable Terminal, a NativeTerm tab to an Ubuntu 24.04
container through the fork's ssh; lrzsz installed from the tab with
`apt-get install lrzsz`, then used right away):

- `sz big.bin *.txt` (20 MB random + a Chinese name): both arrived with the
  server's SHA-256, 1.3 MB/s (the link's rate), the terminal given back at
  the prompt, no `rz` line, no `OO`;
- `rz` with two files (30 MB + a Chinese name): SHA-256 equal on the
  server, `-rw-r--r--` and the local modification time;
- `rz` with the picker: it came up in front of the Terminal (owned by the
  shim); closing it cancelled rz on the server (`rz` exit 128) and gave
  the prompt back; the Terminal stayed enabled;
- `sz`, Esc once after a few seconds: cancelled at 15.9 of 20 MB, nothing
  shown as garbage; `sz` again: "going on from 16.0 MB", completed, the
  right SHA-256.
- Unit tests: both ways in-process (resume from a partial file, a name
  collision, a Chinese name, an empty file), cancelling, the ZFILE fields,
  received-name cleaning.

**In tmux** (NativeTerm's persistent sessions, or any): tmux is a terminal
emulator of its own; it drops the ZDLE from the header (the tab showed
`**B00000000000000`) and would change the binary data both ways (and take
Ctrl+B as its prefix), so ZMODEM can't work through it, and changing the
server's tmux is not ours to do. ssh spots the header as tmux leaves it
(`**B00` / `**B01` and ten more hex digits, no ZDLE) and runs the helper
in `tmux` mode: it cancels rz / sz (the cancel passes through tmux as
Ctrl+X keys) and takes in what follows, then asks NativeTerm (pipe, role
`Request`, message `OpenFiles`, found by `WT_SESSION`) to open the files
window of the tab's session, which starts at the tmux pane's folder, and
says so in the tab. Verified live: `sz small.bin` and `rz` in a tmux tab
both stopped on the server (no rz / sz left running) and opened the files
window at `/srv/zt` with the file listed.

**ntplink (Telnet, serial, raw)**: the same handover in PuTTY's frontend
(`patches/ntplink.c` in the PuTTY fork; the shim sets `NATIVETERM_ZMODEM`
for it). The device's output is watched for the hex header; from there the
helper (`--zmodem download|upload --escape-control`) runs with pipes, the
device's output goes to it and its output to the device until its end
marker; the `rz` line and the closing `OO` are hidden the same way. Things
that differ from ssh:

- Flow control both ways: the backend is unthrottled when the helper's
  pipe drains (PuTTY's Telnet stops reading past 4 KB of backlog, which
  first stalled the transfer at ~5 KB), and the helper's output is read
  only while the connection keeps up.
- Telnet without binary mode changes CR, NUL and 0xFF, and busybox's
  telnetd leaves a NUL in when a CR NUL is split between two reads (rz
  reported "Bad CRC" at ~350 KB). So every control character is escaped:
  on download the receiver's ZRINIT gets ESCCTL (the header rewritten, CRC
  recomputed); on upload zmodem2 can't escape everything, so its output
  gets C0 / C1, DEL and 0xFF as ZDLE plus a printable byte (hex headers
  left alone).
- The sender takes the receiver's input while streaming (it only idled at
  the end before), so a ZRPOS after an error or a cancel is acted on at
  once; for ssh too.
- ntplink reads the keyboard itself (PuTTY's reader thread): during a
  transfer keys aren't sent; Esc (alone) or Ctrl+C sets the helper's named
  event `Local\NativeTermZmodemCancel-<pid>`.
- tmux in a Telnet / serial session: ntplink spots the header tmux leaves
  (as ssh does) and runs the helper as `--zmodem tmux --escape-control
  --no-files`: rz / sz are stopped on the server and the tab says so; no
  files window (these sessions have no SFTP). Verified live: `sz` and `rz`
  in tmux over Telnet both stopped (none left running), the shell usable
  after; `sz` outside tmux still transferred (SHA-256 equal).
- The console is now read with VT input: before, arrows, Home, F-keys sent
  nothing, and through ConPTY Esc was lost too (so vim, shell history and
  device CLIs over Telnet / serial missed them). A console without VT
  input falls back to the old mode.

Verified live (busybox telnetd, lrzsz): `sz` 3 MB and 30 MB with the
server's SHA-256 (~1.1 MB/s); `rz` 20 MB (SHA-256, mode 644, no retries in
`rz -vv`); Esc during `sz` (30 MB) and during `rz` (150 MB): both stopped
on the server, `rz` removed its partial file; Up recalled the shell's last
command. `rz` 150 MB over ssh again after the sender change: SHA-256 equal.

**Offered upstream** (codeberg.org/jarkko/zmodem2, one branch each from
`fivetime/zmodem2`, every one with tests against lrzsz): #8 `abort()`
sends the cancel sequence (ten CAN, ten backspaces), #9 ZFILE carries the
modification time and mode (`FileInfo::with_modified` / `with_mode`), #10
ESCCTL (`Receiver::set_escape_control`, and the sender escaping every
control character when asked). The workarounds here stay until those are
released; then `zfile_name`, `with_escctl`, `EscapeAll` and the CANCEL
sequence can go.

Not yet: shipping the fork's ssh with NativeTerm (packaging) and
macOS / Linux builds.

## Build (Windows)

Builds on this machine (VS 2026 Enterprise with the v143 build tools and
Spectre libraries for v143 and v145 added): `nativeterm\build.ps1` in the
fork (branch `nativeterm`) → `bin\x64\Release\ssh.exe`,
`OpenSSH_for_Windows_10.2p1 Win32-OpenSSH-GitHub, LibreSSL 4.2.0`, 0 errors;
it logged in to the test container and `ssh -G` resolves our configs.
What the repo's own `Start-OpenSSHBuild` trips over here, and what the
script does instead:

- VS 2026's default MSVC (14.51) has no Spectre libraries (MSB8040 while
  vcpkg builds LibreSSL): the script's triplet pins vcpkg to v143 (14.44),
  the toolset the OpenSSH projects use anyway;
- the projects expect `vcpkg_installed\x64-custom\x64-custom\…`: vcpkg runs
  with `--x-install-root=vcpkg_installed\x64-custom`;
- `Start-OpenSSHBuild` recognises VS 2015–2022 only (by "2022" in MSBuild's
  path) and falls to the VS 2015 branch: the script calls MSBuild itself;
- `paths.targets` pins Windows SDK 10.0.22621; only 10.0.26100 is here:
  passed as `/p:WindowsSDKVersion`;
- `openbsd_compat` finds OpenSSL only through Visual Studio's vcpkg
  integration: vcpkg's `vcpkg.props` / `vcpkg.targets` are imported for this
  build only (`ForceImportBeforeCppProps` / `ForceImportAfterCppTargets`,
  `VcpkgManifestInstall=false`), not `vcpkg integrate install` (which would
  apply to every C++ project of the user).

Warnings: 7 × C4819 (source characters outside code page 936) and 2 ×
C4047 in `clientloop.c` / `serverloop.c`, both upstream as is.
