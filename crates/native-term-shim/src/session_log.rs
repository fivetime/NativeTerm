//! The session log, written here for every kind of session (see
//! `native_term_config::session_log` for the settings and
//! `docs/SESSION-LOG.md`).
//!
//! The client (NativeTerm's ssh, or ntplink) copies what it writes to the
//! terminal into a pipe of this shim's, named in `NATIVETERM_LOG` (a
//! handle on Windows, a descriptor elsewhere, inherited): records of one
//! byte of kind (`O` output, `T` a message of the client's own), four of
//! length (little-endian), then the bytes. A thread here reads them and
//! hands them to the log, which writes them when a log is on and drops
//! them when it is off; so a log can start and stop while the client runs.
//! The pipe is made once per tab and given to every client started in it.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use native_term_config::session_log::{self, LogSettings, Names, Stamp};

/// Where the client finds the pipe.
pub const ENV: &str = "NATIVETERM_LOG";
/// Asks NativeTerm's ssh to send its own messages as `T` records; the
/// value is the `LogLevel` the console showed them up to before.
pub const TRACE_ENV: &str = "NATIVETERM_LOG_TRACE";

const NEWLINE: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// Output held while the file is being asked for, at most this much.
const HOLD_LIMIT: usize = 8 << 20;

/// The time now, as the substitutions show it.
pub fn now() -> Stamp {
    use chrono::{Datelike, Timelike};
    let t = chrono::Local::now();
    Stamp {
        year: t.year(),
        month: t.month(),
        day: t.day(),
        hour: t.hour(),
        minute: t.minute(),
        second: t.second(),
        milli: t.timestamp_subsec_millis().min(999),
    }
}

/// ssh's `LogLevel` for a trace level (0: none asked).
pub fn ssh_log_level(trace: u8) -> Option<&'static str> {
    match trace {
        0 => None,
        1 => Some("VERBOSE"),
        2 => Some("DEBUG1"),
        3 => Some("DEBUG2"),
        _ => Some("DEBUG3"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Record {
    Output(Vec<u8>),
    Trace(String),
}

/// The text of the output, a line at a time: escape sequences left out,
/// Backspace taking back the character before it, carriage returns
/// dropped; lines end at a line feed.
struct Lines {
    text: String,
    started: Option<Stamp>,
    /// The time of the output being read.
    at: Stamp,
    done: Vec<(Stamp, String)>,
}

impl vte::Perform for Lines {
    fn print(&mut self, c: char) {
        self.started.get_or_insert(self.at);
        self.text.push(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                let started = self.started.take().unwrap_or(self.at);
                self.done.push((started, std::mem::take(&mut self.text)));
            }
            0x08 => {
                self.text.pop();
            }
            b'\t' => {
                self.started.get_or_insert(self.at);
                self.text.push('\t');
            }
            _ => {}
        }
    }
}

struct Text {
    parser: vte::Parser,
    lines: Lines,
    /// A session in a legacy code page: read as what it is.
    decoder: Option<encoding_rs::Decoder>,
}

impl Text {
    fn new(charset: Option<&str>) -> Text {
        Text {
            parser: vte::Parser::new(),
            lines: Lines { text: String::new(), started: None, at: Stamp::default(), done: Vec::new() },
            decoder: charset.and_then(encoding).map(|e| e.new_decoder_without_bom_handling()),
        }
    }

    /// The lines `bytes` finished.
    fn feed(&mut self, bytes: &[u8], at: Stamp) -> Vec<(Stamp, String)> {
        self.lines.at = at;
        match &mut self.decoder {
            Some(decoder) => {
                let mut text = String::with_capacity(bytes.len() * 2);
                let mut rest = bytes;
                loop {
                    let need = decoder.max_utf8_buffer_length(rest.len()).unwrap_or(rest.len() * 3 + 16);
                    text.reserve(need);
                    let (result, read, _) = decoder.decode_to_string(rest, &mut text, false);
                    rest = &rest[read..];
                    if result == encoding_rs::CoderResult::InputEmpty {
                        break;
                    }
                }
                self.parser.advance(&mut self.lines, text.as_bytes());
            }
            None => self.parser.advance(&mut self.lines, bytes),
        }
        std::mem::take(&mut self.lines.done)
    }

    /// The line not finished yet, if any (written when the log stops).
    fn partial(&mut self) -> Option<(Stamp, String)> {
        let started = self.lines.started.take()?;
        Some((started, std::mem::take(&mut self.lines.text)))
    }
}

/// The encoding a session's charset names (`gbk`, `big5`, a code page
/// number); `None` for UTF-8 and anything unknown.
fn encoding(charset: &str) -> Option<&'static encoding_rs::Encoding> {
    let charset = charset.trim();
    let found = match charset.parse::<u16>() {
        Ok(page) => codepage::to_encoding(page),
        Err(_) => encoding_rs::Encoding::for_label(charset.as_bytes()),
    };
    found.filter(|e| *e != encoding_rs::UTF_8)
}

struct Open {
    path: PathBuf,
    writer: BufWriter<File>,
    /// The day it was opened for (a new file after midnight).
    day: (i32, u32, u32),
    /// The settings it was started with that decide the format.
    raw: bool,
    text: Text,
}

/// A tab's session log.
pub struct Log {
    settings: LogSettings,
    names: Names,
    charset: Option<String>,
    data_dir: PathBuf,
    home: Option<PathBuf>,
    open: Option<Open>,
    /// Output kept while the file is asked for.
    held: Option<Vec<(Stamp, Record)>>,
    held_bytes: usize,
}

impl Log {
    pub fn new(data_dir: PathBuf, home: Option<PathBuf>) -> Log {
        Log {
            settings: LogSettings::default(),
            names: Names::default(),
            charset: None,
            data_dir,
            home,
            open: None,
            held: None,
            held_bytes: 0,
        }
    }

    /// The session's settings, read again before every connection (an
    /// edit applies at the next one; a log that is on goes on as started).
    pub fn configure(&mut self, settings: LogSettings, names: Names, charset: Option<String>) {
        self.settings = settings;
        self.names = names;
        self.charset = charset;
    }

    pub fn set_data_dir(&mut self, dir: PathBuf) {
        self.data_dir = dir;
    }

    pub fn settings(&self) -> &LogSettings {
        &self.settings
    }

    /// The file being written, if a log is on.
    pub fn file(&self) -> Option<&Path> {
        self.open.as_ref().map(|o| o.path.as_path())
    }

    /// Where a log started now goes (the settings' file, substituted).
    pub fn planned_file(&self, at: &Stamp) -> PathBuf {
        let name = session_log::expand(&self.settings.file, &self.names, at, |n| std::env::var(n).ok(), true);
        let name = if self.settings.file.trim().is_empty() {
            session_log::expand(session_log::DEFAULT_NAME, &self.names, at, |_| None, true)
        } else {
            name
        };
        session_log::resolve(&name, &self.data_dir, self.home.as_deref())
    }

    /// Start writing to `path` (made, with its folders; replaced unless
    /// the settings append).
    pub fn start(&mut self, path: PathBuf, at: Stamp) -> io::Result<()> {
        self.stop(at);
        if let Some(folder) = path.parent().filter(|f| !f.as_os_str().is_empty()) {
            std::fs::create_dir_all(folder)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(self.settings.append)
            .truncate(!self.settings.append)
            .open(&path)?;
        self.open = Some(Open {
            path,
            writer: BufWriter::new(file),
            day: (at.year, at.month, at.day),
            raw: self.settings.raw,
            text: Text::new(self.charset.as_deref()),
        });
        Ok(())
    }

    /// Stop: the line not finished yet is written, the file closed.
    pub fn stop(&mut self, at: Stamp) {
        self.held = None;
        self.held_bytes = 0;
        if self.open.is_some() {
            self.write_partial(at);
            if let Some(mut open) = self.open.take() {
                let _ = open.writer.flush();
            }
        }
    }

    /// Keep what arrives until `release` (the file is being asked for).
    pub fn hold(&mut self) {
        self.held = Some(Vec::new());
        self.held_bytes = 0;
    }

    pub fn holding(&self) -> bool {
        self.held.is_some()
    }

    /// The file was chosen (or not: `None`): what was held goes into it.
    pub fn release(&mut self, path: Option<PathBuf>, at: Stamp) -> io::Result<()> {
        let held = self.held.take().unwrap_or_default();
        self.held_bytes = 0;
        let Some(path) = path else { return Ok(()) };
        self.start(path, at)?;
        for (at, record) in held {
            self.record(record, at);
        }
        Ok(())
    }

    /// The session connected: its text, if a log is on.
    pub fn connected(&mut self, at: Stamp) {
        let text = self.settings.upon_connect.clone();
        self.custom(&text, at);
        self.flush();
    }

    /// The session ended: the line not finished yet, then its text. The
    /// log stays on for the next connection in this tab.
    pub fn disconnected(&mut self, at: Stamp) {
        self.write_partial(at);
        let text = self.settings.upon_disconnect.clone();
        self.custom(&text, at);
        self.flush();
    }

    pub fn output(&mut self, bytes: &[u8], at: Stamp) {
        self.record(Record::Output(bytes.to_vec()), at);
    }

    pub fn trace(&mut self, line: &str, at: Stamp) {
        self.record(Record::Trace(line.to_string()), at);
    }

    fn record(&mut self, record: Record, at: Stamp) {
        if let Some(held) = &mut self.held {
            let size = match &record {
                Record::Output(b) => b.len(),
                Record::Trace(t) => t.len(),
            };
            if self.held_bytes + size <= HOLD_LIMIT {
                self.held_bytes += size;
                held.push((at, record));
            }
            return;
        }
        if self.open.is_none() {
            return;
        }
        self.next_day(at);
        match record {
            Record::Output(bytes) => self.write_output(&bytes, at),
            Record::Trace(line) => {
                self.write_partial(at);
                let prefix = self.stamp_prefix(&at);
                self.write_line(&format!("{prefix}{line}"));
            }
        }
        self.flush();
    }

    fn write_output(&mut self, bytes: &[u8], at: Stamp) {
        let only_custom = self.settings.only_custom;
        let Some(open) = &mut self.open else { return };
        if open.raw {
            if !only_custom {
                let _ = open.writer.write_all(bytes);
            }
            return;
        }
        let lines = open.text.feed(bytes, at);
        for (started, text) in lines {
            self.write_text_line(started, &text);
        }
    }

    /// A line of the session's text, with what goes before each line.
    fn write_text_line(&mut self, started: Stamp, text: &str) {
        let mut line = self.stamp_prefix(&started);
        let each = self.settings.each_line.clone();
        if !each.is_empty() {
            line.push_str(&session_log::expand(&each, &self.names, &started, |n| std::env::var(n).ok(), false));
        }
        if self.settings.only_custom {
            if each.is_empty() {
                return;
            }
        } else {
            line.push_str(text);
        }
        self.write_line(&line);
    }

    fn write_partial(&mut self, _at: Stamp) {
        let partial = match &mut self.open {
            Some(open) if !open.raw => open.text.partial(),
            _ => None,
        };
        if let Some((started, text)) = partial {
            self.write_text_line(started, &text);
        }
    }

    fn stamp_prefix(&self, at: &Stamp) -> String {
        match self.settings.timestamp && !self.open.as_ref().is_some_and(|o| o.raw) {
            true => format!(
                "[{:04}-{:02}-{:02} {:02}:{:02}:{:02}] ",
                at.year, at.month, at.day, at.hour, at.minute, at.second
            ),
            false => String::new(),
        }
    }

    fn custom(&mut self, text: &str, at: Stamp) {
        if self.open.is_none() || text.trim().is_empty() {
            return;
        }
        let text = session_log::expand(text, &self.names, &at, |n| std::env::var(n).ok(), false);
        self.write_line(&text);
    }

    fn write_line(&mut self, line: &str) {
        if let Some(open) = &mut self.open {
            let _ = open.writer.write_all(line.as_bytes());
            let _ = open.writer.write_all(NEWLINE.as_bytes());
        }
    }

    fn flush(&mut self) {
        if let Some(open) = &mut self.open {
            let _ = open.writer.flush();
        }
    }

    /// After midnight, a new file where the name has the day in it.
    fn next_day(&mut self, at: Stamp) {
        let due = self.open.as_ref().is_some_and(|o| o.day != (at.year, at.month, at.day))
            && self.settings.midnight
            && session_log::names_the_day(if self.settings.file.trim().is_empty() {
                session_log::DEFAULT_NAME
            } else {
                &self.settings.file
            });
        if !due {
            return;
        }
        let path = self.planned_file(&at);
        let append = self.settings.append;
        // a new day's file is added to, not replaced, when it exists
        self.settings.append = true;
        if self.start(path, at).is_err() {
            self.open = None;
        }
        self.settings.append = append;
    }
}

/// The log, shared by the tab's thread and the pipe's.
pub type Shared = Arc<Mutex<Log>>;

pub fn lock(log: &Shared) -> MutexGuard<'_, Log> {
    log.lock().unwrap_or_else(|e| e.into_inner())
}

/// The pipe clients copy their output into (see the module's comment).
pub struct Feed {
    writer: io::PipeWriter,
}

impl Feed {
    /// Made once per tab; its reader hands what comes to `log`.
    pub fn open(log: Shared) -> io::Result<Feed> {
        let (reader, writer) = io::pipe()?;
        inheritable(&writer)?;
        std::thread::Builder::new().name("session-log".into()).spawn(move || read_records(reader, &log))?;
        Ok(Feed { writer })
    }

    /// The value of `NATIVETERM_LOG` for a client started now.
    pub fn value(&self) -> String {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            (self.writer.as_raw_handle() as usize).to_string()
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            self.writer.as_raw_fd().to_string()
        }
    }
}

#[cfg(windows)]
fn inheritable(writer: &io::PipeWriter) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT};
    // SAFETY: the handle is the pipe's own, open for as long as `writer`.
    unsafe { SetHandleInformation(HANDLE(writer.as_raw_handle()), HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT) }
        .map_err(io::Error::other)
}

#[cfg(unix)]
fn inheritable(writer: &io::PipeWriter) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: the descriptor is the pipe's own, open for as long as
    // `writer`; clearing FD_CLOEXEC only lets a child keep it.
    match unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFD, 0) } {
        -1 => Err(io::Error::last_os_error()),
        _ => Ok(()),
    }
}

fn read_records(mut reader: io::PipeReader, log: &Shared) {
    let mut head = [0u8; 5];
    loop {
        if reader.read_exact(&mut head).is_err() {
            return;
        }
        let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        let mut body = vec![0u8; len];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let at = now();
        let mut log = lock(log);
        match head[0] {
            b'O' => log.output(&body, at),
            b'T' => log.trace(String::from_utf8_lossy(&body).trim_end(), at),
            _ => {}
        }
    }
}

/// A tab's log, as the shim runs it: the settings read before every
/// connection, the pipe given to every client, a log started on
/// connecting or from NativeTerm, NativeTerm told what is written.
pub struct TabLog {
    log: Shared,
    feed: Option<Feed>,
    link: Option<crate::link::LinkSender>,
}

impl TabLog {
    pub fn new(link: Option<&crate::link::Link>) -> TabLog {
        // NativeTerm says where its data folder is when it welcomes the
        // shim; until then (or without it), the usual place
        let data_dir = welcomed_dir().unwrap_or_else(|| {
            native_term_os::home::app_data().map(|d| d.join("NativeTerm")).unwrap_or_else(|| PathBuf::from("."))
        });
        let log: Shared = Arc::new(Mutex::new(Log::new(data_dir, native_term_os::home::home_dir())));
        let feed = match Feed::open(Arc::clone(&log)) {
            Ok(feed) => Some(feed),
            Err(e) => {
                crate::debug::log(format!("session log: no pipe: {e}"));
                None
            }
        };
        TabLog { log, feed, link: link.map(crate::link::Link::sender) }
    }

    /// The session's settings and names, before a connection.
    pub fn configure(&self, settings: LogSettings, names: Names, charset: Option<String>) {
        lock(&self.log).configure(settings, names, charset);
    }

    /// NativeTerm's data folder (from its welcome).
    pub fn set_data_dir(&self, dir: &str) {
        if !dir.is_empty() {
            lock(&self.log).set_data_dir(PathBuf::from(dir));
        }
    }

    /// The client gets the pipe; `trace`: the trace level asked for, when
    /// a log is on or starts on connecting (0 otherwise).
    pub fn prepare(&self, command: &mut std::process::Command) -> u8 {
        let Some(feed) = &self.feed else { return 0 };
        command.env(ENV, feed.value());
        let log = lock(&self.log);
        match log.file().is_some() || log.settings().start {
            true => log.settings().trace,
            false => 0,
        }
    }

    /// The client is about to connect: a log that starts on connecting
    /// starts now (unless one is on), so that it has the connection's own
    /// messages (the trace) and what the server says before the login;
    /// then the text for connecting.
    pub fn connecting(&self) {
        let start = {
            let log = lock(&self.log);
            log.settings().start && log.file().is_none() && !log.holding()
        };
        if start {
            self.start();
        }
        lock(&self.log).connected(now());
    }

    pub fn disconnected(&self) {
        lock(&self.log).disconnected(now());
    }

    /// Start or stop, from the session's menu.
    pub fn set(&self, on: bool) {
        match on {
            true => {
                let off = {
                    let log = lock(&self.log);
                    log.file().is_none() && !log.holding()
                };
                if off {
                    self.start();
                }
            }
            false => {
                lock(&self.log).stop(now());
                self.report();
            }
        }
    }

    /// Start now: in the settings' file, or the one the person names
    /// (asked in NativeTerm's window, while the output waits).
    fn start(&self) {
        self.await_data_dir();
        let (prompt, planned) = {
            let log = lock(&self.log);
            (log.settings().prompt, log.planned_file(&now()))
        };
        if !prompt {
            self.open(planned);
            return;
        }
        lock(&self.log).hold();
        let log = Arc::clone(&self.log);
        let link = self.link.clone();
        std::thread::spawn(move || {
            // NativeTerm not there: the settings' file
            let chosen = match ask_file(&planned) {
                Some(answer) => answer,
                None => Some(planned),
            };
            let result = lock(&log).release(chosen.clone(), now());
            if let Err(e) = result {
                println!("\r\n{}", crate::t!("log-not-started", error = e.to_string()));
            }
            if let Some(link) = link {
                let file = lock(&log).file().map(|f| f.display().to_string());
                link.send(ShimMessage::Logging { file });
            }
        });
    }

    fn open(&self, path: PathBuf) {
        if let Err(e) = lock(&self.log).start(path.clone(), now()) {
            let file = path.display().to_string();
            println!("\r\n{}", crate::t!("log-not-started-file", file = file, error = e.to_string()));
        }
        self.report();
    }

    /// NativeTerm's data folder, where it will say it: its welcome comes
    /// on the link's own thread, maybe just after the client starts (a log
    /// that starts on connecting starts right then), so it is waited for a
    /// moment; without it, the usual place.
    fn await_data_dir(&self) {
        if self.link.is_none() {
            return;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        loop {
            if let Some(dir) = welcomed_dir() {
                lock(&self.log).set_data_dir(dir);
                return;
            }
            if std::time::Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    fn report(&self) {
        if let Some(link) = &self.link {
            let file = lock(&self.log).file().map(|f| f.display().to_string());
            link.send(ShimMessage::Logging { file });
        }
    }
}

use native_term_session::protocol::{AppMessage, Role, ShimMessage};

static TAB: std::sync::OnceLock<TabLog> = std::sync::OnceLock::new();

/// NativeTerm's data folder, as its welcome said (set on the link's
/// thread as soon as the welcome arrives).
static WELCOMED: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The link heard NativeTerm's welcome, with its data folder.
pub fn welcomed(dir: &str) {
    if dir.is_empty() {
        return;
    }
    *WELCOMED.lock().unwrap_or_else(|e| e.into_inner()) = Some(PathBuf::from(dir));
    if let Some(tab) = tab() {
        tab.set_data_dir(dir);
    }
}

fn welcomed_dir() -> Option<PathBuf> {
    WELCOMED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The tab's log, made on first use (one tab per shim).
pub fn init(link: Option<&crate::link::Link>) -> &'static TabLog {
    TAB.get_or_init(|| TabLog::new(link))
}

pub fn tab() -> Option<&'static TabLog> {
    TAB.get()
}

/// A session's log settings and the names its substitutions use, from
/// the session tree (`effective`: what ssh uses, for its host and port).
pub fn for_alias(alias: &str, effective: &[(String, String)]) -> (LogSettings, Names) {
    let tree = native_term_config::tree::SessionTree::load(&crate::plink::ssh_dir());
    let Some((folder, host)) = tree.find(alias) else {
        return (
            LogSettings::default(),
            Names { host: alias.to_string(), session: alias.to_string(), ..Names::default() },
        );
    };
    let of = |key: &str| effective.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    // (a non-SSH session's host name is its host, or its serial line)
    let names = Names {
        host: of("hostname").or_else(|| host.hostname.clone()).unwrap_or_else(|| alias.to_string()),
        session: host.label().to_string(),
        port: of("port").and_then(|p| p.parse().ok()).or(host.port),
        folder: folder.label().to_string(),
    };
    (LogSettings::for_host(folder, host), names)
}

/// NativeTerm's messages for the log (its welcome, the menu's start and
/// stop); whether `message` was one.
pub fn handle(message: &AppMessage) -> bool {
    let Some(tab) = tab() else { return false };
    match message {
        AppMessage::Welcome { data_dir: Some(dir), .. } => {
            tab.set_data_dir(dir);
            true
        }
        AppMessage::Log { on } => {
            tab.set(*on);
            true
        }
        _ => false,
    }
}

/// The tab is closing: the line not finished yet is written, the file
/// closed.
pub fn finish() {
    if let Some(tab) = tab() {
        lock(&tab.log).stop(now());
    }
}

/// The file the person names in NativeTerm's window: `Some(None)` they
/// cancelled, `None` NativeTerm couldn't be asked.
fn ask_file(suggested: &Path) -> Option<Option<PathBuf>> {
    let name = crate::pipe_name()?;
    let conn = native_term_session::pipe::connect(&name, std::time::Duration::from_millis(500)).ok()?;
    let hello = ShimMessage::Hello {
        protocol: native_term_session::PROTOCOL_VERSION,
        role: Role::Request,
        pid: std::process::id(),
        wt_session: crate::wt_session(),
        session: None,
        alias: None,
        terminal_window: None,
    };
    conn.send(&hello).ok()?;
    conn.send(&ShimMessage::AskLogFile { suggested: suggested.display().to_string() }).ok()?;
    loop {
        match conn.recv::<AppMessage>(std::time::Duration::from_secs(600)) {
            Ok(Some(AppMessage::LogFile { path })) => return Some(path.map(PathBuf::from)),
            Ok(Some(_)) => continue,
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: u32, day: u32) -> Stamp {
        Stamp { year: 2026, month: 9, day, hour, minute: 30, second: 5, milli: 0 }
    }

    fn log(dir: &Path, settings: LogSettings) -> Log {
        let mut log = Log::new(dir.to_path_buf(), None);
        let names = Names { host: "10.0.0.1".into(), session: "sw1".into(), port: Some(22), folder: "Lab".into() };
        log.configure(settings, names, None);
        log
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap().replace("\r\n", "\n")
    }

    #[test]
    fn text_without_escape_sequences_a_line_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = log(dir.path(), LogSettings::default());
        let path = log.planned_file(&at(9, 3));
        assert_eq!(path, dir.path().join("logs").join("10.0.0.1-20260903.log"));
        log.start(path.clone(), at(9, 3)).unwrap();
        log.output(b"\x1b[1;32muser@sw1\x1b[0m:~$ ls\r\n", at(9, 3));
        log.output(b"a.txt\x1b]0;title\x07  b.txt\r\nabcd\x08\x08XY\r\n", at(9, 3));
        log.output(b"prompt$ ", at(9, 3));
        log.stop(at(9, 3));
        assert_eq!(read(&path), "user@sw1:~$ ls\na.txt  b.txt\nabXY\nprompt$ \n");
    }

    #[test]
    fn timestamps_custom_data_and_append() {
        let dir = tempfile::tempdir().unwrap();
        let settings = LogSettings {
            file: "%S.log".into(),
            append: true,
            timestamp: true,
            upon_connect: "== %S connected %h:%m ==".into(),
            upon_disconnect: "== gone ==".into(),
            each_line: "%H> ".into(),
            ..LogSettings::default()
        };
        let mut log = log(dir.path(), settings);
        let path = log.planned_file(&at(9, 3));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "before\n").unwrap();
        log.start(path.clone(), at(9, 3)).unwrap();
        log.connected(at(9, 3));
        log.output(b"hello\r\n", at(10, 3));
        log.trace("debug1: channel 0: free", at(10, 3));
        log.disconnected(at(11, 3));
        log.stop(at(11, 3));
        assert_eq!(
            read(&path),
            "before\n== sw1 connected 09:30 ==\n[2026-09-03 10:30:05] 10.0.0.1> hello\n\
             [2026-09-03 10:30:05] debug1: channel 0: free\n== gone ==\n"
        );
    }

    #[test]
    fn raw_keeps_every_byte_and_overwrite_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("raw.log");
        std::fs::write(&path, "old contents").unwrap();
        let mut log = log(dir.path(), LogSettings { raw: true, timestamp: true, ..LogSettings::default() });
        log.start(path.clone(), at(9, 3)).unwrap();
        log.output(b"\x1b[31mred\x1b[0m\r\n", at(9, 3));
        log.stop(at(9, 3));
        assert_eq!(std::fs::read(&path).unwrap(), b"\x1b[31mred\x1b[0m\r\n");
    }

    #[test]
    fn only_custom_data_and_nothing_without_a_log() {
        let dir = tempfile::tempdir().unwrap();
        let settings = LogSettings {
            only_custom: true,
            upon_connect: "up".into(),
            each_line: "*".into(),
            ..LogSettings::default()
        };
        let mut log = log(dir.path(), settings);
        // nothing is on: nothing is written anywhere
        log.output(b"lost\n", at(9, 3));
        let path = dir.path().join("c.log");
        log.start(path.clone(), at(9, 3)).unwrap();
        log.connected(at(9, 3));
        log.output(b"secret\r\nmore\r\n", at(9, 3));
        log.stop(at(9, 3));
        assert_eq!(read(&path), "up\n*\n*\n");
    }

    #[test]
    fn a_new_file_after_midnight_where_the_name_has_the_day() {
        let dir = tempfile::tempdir().unwrap();
        let settings = LogSettings { file: "%Y%M%D.log".into(), midnight: true, ..LogSettings::default() };
        let mut log = log(dir.path(), settings);
        let first = log.planned_file(&at(23, 3));
        log.start(first.clone(), at(23, 3)).unwrap();
        log.output(b"late\r\n", at(23, 3));
        log.output(b"early\r\n", at(0, 4));
        log.stop(at(0, 4));
        assert_eq!(read(&first), "late\n");
        assert_eq!(read(&dir.path().join("logs").join("20260904.log")), "early\n");
    }

    #[test]
    fn output_is_held_while_the_file_is_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = log(dir.path(), LogSettings::default());
        log.hold();
        log.output(b"first\r\n", at(9, 3));
        let path = dir.path().join("asked.log");
        log.release(Some(path.clone()), at(9, 3)).unwrap();
        log.output(b"second\r\n", at(9, 3));
        log.stop(at(9, 3));
        assert_eq!(read(&path), "first\nsecond\n");
        // not chosen: nothing is kept
        log.hold();
        log.output(b"third\r\n", at(9, 3));
        log.release(None, at(9, 3)).unwrap();
        assert!(log.file().is_none());
    }

    #[test]
    fn a_legacy_code_page_is_read_as_what_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = Log::new(dir.path().to_path_buf(), None);
        log.configure(LogSettings::default(), Names::default(), Some("gbk".into()));
        let path = dir.path().join("gbk.log");
        log.start(path.clone(), at(9, 3)).unwrap();
        // "中文" in GBK, split in the middle of a character
        log.output(&[0xD6, 0xD0, 0xCE], at(9, 3));
        log.output(&[0xC4, b'\r', b'\n'], at(9, 3));
        log.stop(at(9, 3));
        assert_eq!(read(&path), "中文\n");
    }

    #[test]
    fn records_from_the_pipe() {
        let dir = tempfile::tempdir().unwrap();
        let shared: Shared = Arc::new(Mutex::new(log(dir.path(), LogSettings::default())));
        let path = dir.path().join("pipe.log");
        lock(&shared).start(path.clone(), at(9, 3)).unwrap();
        let mut feed = Feed::open(Arc::clone(&shared)).unwrap();
        let mut send = |kind: u8, body: &[u8]| {
            let mut record = vec![kind];
            record.extend_from_slice(&(body.len() as u32).to_le_bytes());
            record.extend_from_slice(body);
            feed.writer.write_all(&record).unwrap();
        };
        send(b'O', b"from ssh\r\n");
        send(b'T', b"debug1: hello");
        for _ in 0..100 {
            if std::fs::read_to_string(&path).is_ok_and(|t| t.contains("hello")) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        lock(&shared).stop(at(9, 3));
        assert_eq!(read(&path), "from ssh\ndebug1: hello\n");
        assert!(feed.value().parse::<u64>().is_ok());
    }
}
