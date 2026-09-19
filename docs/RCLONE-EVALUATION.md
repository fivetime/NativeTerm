# rclone as a file back end: evaluation (2026-09-19)

Whether NativeTerm's files window could use [rclone](https://rclone.org)
for its transfers instead of, or next to, its own SFTP client. Written so
the question needn't be researched again; each finding says how it was
established: **[source]** read in rclone's code, **[live]** tried against a
test server, **[data]** from rclone's own backend data.

**Decision.** Hosts reached over SSH keep NativeTerm's own SFTP client:
one experience with terminal tabs (the same ssh config, askpass, saved
passwords), pause and resume at the byte, non-UTF-8 names, kept
modification times, one connection. rclone is the candidate if files over
protocols *without* SSH are ever wanted (FTP/FTPS, WebDAV, S3, cloud
drives), behind the same window, queue and Synchronize dialog, after the
fork patches listed below. A configured external client (WinSCP and the
like) was dropped: they don't read the ssh config the hosts are defined in.

## What was looked at

- The fork `fivetime/rclone`, cloned to `C:\MyProjects\RustProjects\rclone`,
  at `c8d60a67f` (2026-09-16), version `v1.76.0-DEV`. MIT licence. Go
  module `github.com/rclone/rclone`, needs Go ≥ 1.26 (built with 1.27.1).
- Driven the way NativeTerm would: `rclone rcd` (the remote-control
  server, JSON over HTTP on 127.0.0.1), the SFTP back end through the
  system `ssh.exe` and NativeTerm's test ssh config, against an Ubuntu
  24.04 container with OpenSSH.

## Findings

### What fits

- **Progress for the queue [source, live].** `core/stats` gives, per file
  being transferred (`transferring`): `name`, `bytes`, `size`,
  `percentage`, `speed`, `speedAvg`, `eta`; and totals (`bytes`,
  `totalBytes`, `speed`, `eta`, `transfers`, `errors`, `lastError`,
  `checks`, `deletes`, `elapsedTime`, …). Live: a 40 MB download reported
  bytes, percent and ETA every second. Stats can be grouped
  (`_group`, `core/group-list`, `core/stats-delete`).
- **Jobs [source, live].** Any call can run asynchronously (`_async=true`
  → `jobid`); `job/status` (finished, success, error, duration),
  `job/list`, `job/stop`, `job/stopgroup`, `job/batch`.
- **Correctness and speed [live].** The downloaded file's SHA-256 matched
  the server's; speed was that of NativeTerm's own client on the same
  link.
- **The system ssh [source, live].** The SFTP back end's `ssh` option
  (`--sftp-ssh`, "path and arguments to external ssh binary") runs e.g.
  `ssh.exe -F <config> <alias>` plus `-s sftp`, so ProxyJump, keys, Match,
  Include and `NativeTerm*`-aware config all apply. rclone starts it with
  its own environment (no `Env` set), so an askpass helper set for rclone
  reaches ssh.
- **Breadth [data].** 68 back ends (see below), plus `serve` (SFTP, FTP,
  WebDAV, HTTP, S3, NFS, DLNA, restic, docker), `mount`, `bisync`.

### What doesn't

1. **No pause [source].** Nothing in `fs/accounting`, `fs/rc` or
   `fs/operations` pauses a transfer; a job can only be stopped. The
   nearest thing, `core/bwlimit` set very low, keeps connections open
   until they time out.
2. **No resume [source, live].** Transfers go to a temporary name
   (`--partial-suffix`, default `.partial`) and are renamed when complete
   (`--inplace` writes the target directly instead). Stopped at 41 %, rclone
   deleted the partial file (`big.bin.74aaaabe.partial: Removing partially
   written file on error: context canceled`); the next copy started at 0 %.
   No "resume" anywhere in `fs/`. (NativeTerm's client keeps `.ntpart` and
   continues at its length, also after a restart.)
3. **Non-UTF-8 names are destroyed by the JSON API [live].** A server file
   named with the GBK bytes `D6 D0 CE C4` (中文) plus `.txt`: `rclone lsf`
   on the command line printed the bytes unchanged, but `operations/list`
   returned `"Name": "����.txt"` (Go's JSON encoding replaces invalid UTF-8
   with U+FFFD). The bytes are gone, so the file can't be named again in a
   download or delete. The SFTP back end's name encoding (`encoder.Display`
   = `Standard`) only escapes a few characters; there is no charset
   conversion [source]. The FTP back end is the same, and old FTP servers
   in China often use GBK.
4. **External ssh: ~40 s hang per remote [live].** On first use the SFTP
   back end probes the server's hash commands, running `md5sum` (then
   `md5 -r`, …) with stdin left open; through an external ssh on Windows
   each probe waits until it times out (exit 143 after ~39 s). With
   `md5sum_command=none,sha1sum_command=none` (and `shell_type=unix` to skip
   the shell probe) a listing took under a second.
5. **External ssh: no reuse [source].** `sshClientExternal.CanReuse()` is
   false: each SFTP session is a new ssh process, so a password host
   prompts once per connection (NativeTerm's client: one connection).
6. **Host keys [live].** With the internal ssh, `known_hosts_file` unset
   means *no host key validation* (rclone says so); with an external ssh,
   ssh checks them as usual.
7. **Size [live].** Built with `-trimpath -ldflags "-s -w"`: 80 MB with
   everything; 30 MB with only local, SFTP, FTP, WebDAV, S3 and `rcd`
   (the AWS SDK for S3 is much of it).

### Worth knowing

- A remote needn't be configured: a connection string carries the back end
  and its options, e.g.
  `:sftp,ssh='C:/Windows/System32/OpenSSH/ssh.exe -F C:/path/config alias',md5sum_command=none,sha1sum_command=none,shell_type=unix,known_hosts_file=none:/srv/data`.
  With `rcd`, the path belongs in `fs` (`fs=":sftp,…:/srv/r"`,
  `remote=""`); `remote="/srv/r"` against `fs=":sftp,…:"` gave
  "directory not found" (404 — rclone answers errors with HTTP codes).
- `--config NUL` keeps it from reading or writing a config file.
- `--rc-addr 127.0.0.1:<port> --rc-no-auth` for a private local server;
  something would still have to stop other local users from reaching it
  (`--rc-user`/`--rc-pass`, or a random port plus a token).
- `rclone lsf` etc. print raw bytes; only the JSON API loses them.
- Environment: this machine's `GOROOT` points at a missing
  `C:\Developer\go-1.24.4`; building needs `GOROOT=C:\Developer\go-1.27.1`
  (and `GOTOOLCHAIN=local`).

## What the fork would need

| Patch | For | Size |
|---|---|---|
| Names' bytes in the API (e.g. a base64 field beside `Name`/`Path`, and accepted in requests) | non-UTF-8 names (GBK and others) | small |
| Skip hash probing (or close stdin) with an external ssh | the ~40 s hang | very small |
| A trimmed `main` (only the back ends offered, `rcd`) | size | small |
| Keep the partial file on stop; continue at its length | pause / resume | medium, per back end: local, SFTP, FTP, SMB can write at an offset; S3 and cloud drives have their own multipart / resumable-upload schemes |

## Commands of `rclone rcd` (full build)

From `rc/list`:

- **operations:** about check cleanup copyfile copyurl delete deletefile
  fsinfo getfile hashsum hashsumfile list mkdir movefile publiclink purge
  rmdir rmdirs settier settierfile size stat uploadfile
- **sync:** bisync copy move sync
- **job:** batch list status stop stopgroup
- **core:** bwlimit command disks du gc group-list memstats obscure pid
  quit stats stats-delete stats-reset transferred version
- **config:** create delete dump get listremotes oauthstatus oauthstop
  password paths providers setpath unlock unset update
- **options:** blocks get info local set
- **mount:** listmounts mount types unmount unmountall
- **serve:** list start stop stopall types
- **vfs:** forget list poll-interval queue queue-set-expiry refresh stats
- **fscache:** clear entries; **backend:** command; **debug:** (profiling);
  **rc:** error fatal list noop noopauth panic

## Back ends

By rclone's own tier (`docs/data/backends/*.yaml`; Tier 1 is best
supported):

- **Tier 1** (35): Alias (virtual), Azure Blob, B2, Box, Combine
  (virtual), Crypt (virtual), Drime, Drive, Dropbox, Filelu, Filen,
  Filescom, FTP, Gofile, Google Cloud Storage, Hidrive, Imagekit, Local,
  Mailru, Memory, Netstorage, Onedrive, Opendrive, Oracle Object Storage,
  Pcloud, Pikpak, Pixeldrain, S3, SFTP, Shade, Storj, Swift, Union
  (virtual), WebDAV, Yandex
- **Tier 2** (9): Azure Files, Doi, HDFS, Internxt, Jottacloud, Koofr,
  Mega, Putio, SMB
- **Tier 3** (13): Archive (virtual), Cloudinary, Fichier, HTTP,
  Huaweidrive, Internet Archive, Premiumizeme, Qingstor, Quatrix,
  Seafile, Sugarsync, Ulozto, Zoho
- **Tier 4** (7): Chunker (virtual), Compress (virtual), Filefabric,
  Hasher (virtual), Iclouddrive, Proton Drive, Sia
- **Tier 5** (4): Cache (virtual), Google Photos, Linkbox, Sharefile

The ones a file window would most likely offer, with the features that
matter to it (`PartialUploads`: a file can be seen incomplete while it is
written; `OpenChunkWriter`: multipart uploads; `PutStream`: uploads of
unknown size; `Move`/`DirMove`/`Copy`: done on the server):

| Backend | Tier | Hashes | ModTime precision | PartialUploads | OpenChunkWriter | PutStream | Move | DirMove | Copy | Empty folders |
|---|---|---|---|---|---|---|---|---|---|---|
| Local (`local`) | 1 | md5, sha1, sha256, … | 1 ns | ✓ |  | ✓ | ✓ | ✓ |  | ✓ |
| SFTP (`sftp`) | 1 | md5, sha1 | 1 s | ✓ |  | ✓ | ✓ | ✓ | ✓ | ✓ |
| FTP (`ftp`) | 1 | - | 1 s | ✓ |  | ✓ | ✓ | ✓ |  | ✓ |
| SMB (`smb`) | 2 | - | 1 ms | ✓ |  | ✓ | ✓ | ✓ |  | ✓ |
| WebDAV (`webdav`) | 1 | sha1 | 1 s |  |  |  | ✓ | ✓ | ✓ | ✓ |
| HTTP (`http`, read only) | 3 | - | 1 s |  |  | ✓ |  |  |  | ✓ |
| S3 (`s3`) | 1 | md5 | 1 ns |  | ✓ | ✓ |  |  | ✓ |  |
| Azure Blob (`azureblob`) | 1 | md5 | 1 ns |  | ✓ | ✓ |  |  | ✓ |  |
| B2 (`b2`) | 1 | sha1 | 1 ms |  | ✓ | ✓ |  |  | ✓ |  |
| Swift (`swift`) | 1 | md5 | 1 ns |  |  | ✓ |  |  | ✓ |  |
| OneDrive (`onedrive`) | 1 | quickxor | 1 s |  |  |  | ✓ | ✓ | ✓ | ✓ |
| Google Drive (`drive`) | 1 | md5, sha1, sha256 | 1 ms |  |  | ✓ | ✓ | ✓ | ✓ | ✓ |
| Dropbox (`dropbox`) | 1 | dropbox | 1 s |  |  | ✓ | ✓ | ✓ | ✓ | ✓ |
| Box (`box`) | 1 | sha1 | 1 s |  |  | ✓ | ✓ | ✓ | ✓ | ✓ |
| pCloud (`pcloud`) | 1 | sha1, sha256 | 1 s | ✓ |  |  | ✓ | ✓ | ✓ | ✓ |
| Mega (`mega`) | 2 | - | none |  |  |  | ✓ | ✓ |  | ✓ |

## Repeating it

A trimmed build (a module of its own, pointing at the clone, so the fork
isn't touched):

```go
// main.go — go.mod: module rclone-trim; go 1.26.0;
// require github.com/rclone/rclone v0.0.0;
// replace github.com/rclone/rclone => C:/MyProjects/RustProjects/rclone
package main

import (
	_ "github.com/rclone/rclone/backend/ftp"
	_ "github.com/rclone/rclone/backend/local"
	_ "github.com/rclone/rclone/backend/s3"
	_ "github.com/rclone/rclone/backend/sftp"
	_ "github.com/rclone/rclone/backend/webdav"
	"github.com/rclone/rclone/cmd"
	_ "github.com/rclone/rclone/cmd/rcd"
	_ "github.com/rclone/rclone/cmd/version"
	_ "github.com/rclone/rclone/fs/operations"
	_ "github.com/rclone/rclone/fs/sync"
)

func main() { cmd.Main() }
```

```sh
export GOROOT=C:/Developer/go-1.27.1 PATH=/c/Developer/go-1.27.1/bin:$PATH GOTOOLCHAIN=local GOFLAGS=-mod=mod
cp <clone>/go.sum . && go mod tidy && go build -trimpath -ldflags "-s -w" -o rclone-trim.exe .
rclone-trim.exe rcd --rc-no-auth --rc-addr 127.0.0.1:5599 --config NUL -v
```

Then, as JSON POSTs to `http://127.0.0.1:5599/`: `operations/list`
(`fs`, `remote`), `operations/copyfile` (`srcFs`, `srcRemote`, `dstFs`,
`dstRemote`, `_async: true`), `core/stats`, `job/status` / `job/stop`
(`jobid`), `core/stats-reset`, `core/quit`.
