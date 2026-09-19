# rz / sz (ZMODEM) in every session

Decisions and findings so far (2026-09-19). Nothing of this is built yet.

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

## Build (Windows)

Builds on this machine (VS 2026 Enterprise with the v143 build tools and
Spectre libraries for v143 and v145 added): `nativetermuild.ps1` in the
fork (branch `nativeterm`) → `bind\Release\ssh.exe`,
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
