# Session logs

Built on 2026-09-29: what a session's terminal is sent, written to a file
on this computer, for every kind of session (SSH, Telnet, serial, raw,
rlogin, SUPDUP), with SecureCRT's options (Session Options → Terminal →
Log File).

## Why it can be done now

NativeTerm draws no terminal, and for a long time an SSH session's output
went from `ssh` straight into Windows Terminal: nothing of NativeTerm's saw
it, so client-side logging was on the "not planned" list. Every SSH tab
now runs NativeTerm's own fork of OpenSSH (see `RZSZ.md`), which already
reads the output (ZMODEM) and the keyboard (dropped files); a non-SSH tab
runs ntplink, NativeTerm's own frontend over PuTTY's backends. Both copy
what they write to the terminal to the shim that started them, and the
shim writes the log. One implementation for every session type and every
platform, and a log that can start and stop while the session runs.

## The pipe: client → shim

The shim makes one anonymous pipe per tab (`std::io::pipe`), makes its
write end inheritable and names it in `NATIVETERM_LOG` for every client
it starts in the tab (a `HANDLE` value on Windows, a descriptor number
elsewhere). The client takes the variable out of its environment and
keeps the end from its own children (the rz / sz helper, `LocalCommand`).
Records: one byte of kind, four of length (little-endian), the bytes.

| Kind | What |
|---|---|
| `O` | what the client wrote to the terminal: the session's output, the server's stderr (SSH) |
| `T` | a message of the client's own (the trace, below) |

A write that fails (the shim gone) ends the feed; the session goes on.
Without `NATIVETERM_LOG` nothing changes in either client.

- **NativeTerm's ssh** (`nativeterm/nt_log.c` in the fork): `channels.c`
  calls a hook after each successful write of a channel's output and
  stderr to its local end (`nt_channel_written`, NULL outside ssh: the
  file is in `libssh`, which sshd links too); ssh sets it for the
  session channel. Data going to the rz / sz helper during a transfer is
  not copied. On Unix the descriptor is moved to 3 before `closefrom()`,
  which then keeps it.
- **ntplink** (`patches/ntplink.c` in the PuTTY fork): `terminal_output`
  copies what goes to the console, after the ZMODEM watch.
- **PuTTY's plink** (the fallback when ntplink is missing) can't: no
  log, and the tab says so when a log would start.

## The trace

"Trace level" in SecureCRT puts the client's own debug messages into the
log. Here, with a level above 0 and a log on (or starting on connect):

- **ssh**: the shim adds `-o LogLevel=VERBOSE` (1), `DEBUG1` (2), `DEBUG2`
  (3) or `DEBUG3` (4 and up), and `NATIVETERM_LOG_TRACE=<the level the
  host's config has, INFO by default>`. The fork then sends every message
  as a `T` record from its first `log_init` on (reading the config
  already says a lot) and writes to the console only what it showed
  before: the tab doesn't fill with debug lines.
- **ntplink**: `NATIVETERM_LOG_TRACE=1`; its Event Log ("Connecting to
  …", "Connected to …") goes to the feed as well as where it went.

A level set while a session runs applies from its next connection.

## The shim's log (`native-term-shim/src/session_log.rs`)

- **Settings**, read before every connection
  (`native_term_config::session_log`): a host's `NativeTermLog*` keys, or
  its folder's (`Host __nativeterm_folder__`); a non-SSH session's `log`
  table in its `.nt.toml`. A session with any log key of its own uses its
  own set, whole (every key is written), else its folder's.
- **Starting**: before the client starts, when "Start log upon connect"
  is on and no log is on yet, so the connection's own messages (the
  trace) and what the server says before the login are in it; or from
  the session's menu ("Start Session Log"). "Prompt for filename": the
  shim asks NativeTerm (`ShimMessage::AskLogFile`, a `Request`
  connection), which shows the desktop's save dialog; the output waits
  meanwhile (up to 8 MB), so nothing is lost; cancelled, no log; without
  NativeTerm, the settings' file.
- **Staying on**: across reconnects in the tab (the file stays open; the
  connect and disconnect texts are written each time), until stopped from
  the menu or the tab closes. NativeTerm hears the file
  (`ShimMessage::Logging`, replayed to a NativeTerm that starts later) and
  offers "Stop Session Log" and "Open Log File".
- **The file**: SecureCRT's substitutions (`%H` host, `%S` session, `%P`
  port, `%F` folder, `%Y` `%y` `%M` `%D` `%h` `%m` `%s` `%t`, `%%`,
  `%NAME%` an environment variable); host, session and folder made safe
  in a file name. Empty: `<data folder>/logs/%H-%Y%M%D.log`; a relative
  name goes into `<data folder>/logs`, `~` is the home folder. The data
  folder comes in NativeTerm's welcome (`AppMessage::Welcome`), taken on
  the link's own thread as it arrives; a log starting before it (on
  connecting, right after the tab opened) waits for it up to 1.5 s; with
  no NativeTerm, the usual folder (`~/.local/share/NativeTerm`,
  `%APPDATA%\NativeTerm`). Folders are made. "Overwrite" replaces the file when
  a log starts, "Append" adds to it. "Start new log at midnight" opens
  the next day's file (added to) where the name has `%D`.
- **Text** (not raw): escape sequences left out (`vte`'s parser),
  Backspace takes back the character before it, carriage returns
  dropped, a line written at its line feed (the one not finished yet when
  the log stops or the session ends). A session in a legacy code page
  (`NativeTermCharset gbk`, a non-SSH session's charset) is read as what
  it is and written as UTF-8. "Timestamp each line": `[YYYY-MM-DD
  hh:mm:ss] ` before each line, at the time it began; "On each line"
  after it. **Raw**: every byte as it came; no line texts.
- **Custom data**: "Upon connect" when the client starts, "Upon
  disconnect" when it ends, "On each line" before each line, with the
  substitutions. "Log only custom data": the session's own output is left
  out (in text mode each line still gets its "On each line" text).
- **Line ends**: CRLF on Windows, LF elsewhere.

## Settings

The session options dialog has a "Log File" page (a host's, or a
folder's default), laid out as SecureCRT's; the non-SSH session dialog has
the same page. The SecureCRT importer takes a session's page over when
the session logs at all (starts on connect, prompts, raw, midnight, trace,
only custom, connect or disconnect texts); the defaults alone every
session file carries are not written. SecureCRT's "Log Mode" 0 is
Overwrite (its default on the page), 1 Append. The PuTTY importer takes
the Logging page (`LogType` 1 or 2) as a log started on connect;
`LogFileClash` 0 overwrites, anything else appends. Non-SSH sessions
that ntplink logged by PuTTY's settings (`LogType`, `LogFileName`) keep
them as their own set, and the next save writes them as NativeTerm's.

## Verified (2026-09-29, Windows)

- The fork alone (`tee_test.py`): a command on the deepin box, output and
  the server's stderr as `O` records; with a trace level, 1 894 `T`
  records and only the server's stderr on the console; without
  `NATIVETERM_LOG`, nothing different.
- The shim with the fork, in a hidden console, `NativeTermLogStart yes`:
  the connect and disconnect texts, timestamps, escape sequences left
  out, UTF-8 Chinese; raw kept every byte; trace level 2 wrote the
  connection's debug lines from the config onwards.
- ntplink: a raw session to port 22, the server's identification string
  logged, the folder's settings used (`%F-%S-%P.log`); with a trace level
  its Event Log.
- Linux (deepin, X11, WezTerm), through NativeTerm: the "Log File" page
  saved the host's set; connecting logged into the data folder (at first
  into the usual folder: the welcome came after the log had started, now
  waited for); WezTerm's tab menu had "Stop Session Log" and "Open Log
  File", stopping wrote the unfinished prompt line and turned the item
  into "Start Session Log"; a typed command's coloured output was plain
  text in the log.
- The shim test `the_session_log_starts_and_stops_from_nativeterm`:
  started from NativeTerm in the data folder its welcome names, fed,
  stopped, nothing written after.
