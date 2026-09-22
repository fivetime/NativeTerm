//! `nativeterm-shim --zmodem download|upload`: rz / sz in a tab. NativeTerm's
//! ssh (its Win32-OpenSSH fork) spots a ZMODEM start in what the server
//! sends and runs this with the session's data on stdin / stdout: the
//! server's `sz` means a download here, its `rz` an upload. The console
//! (stderr, and its input for Esc / Ctrl+C) stays the tab's, for progress
//! and cancelling. The protocol is the `zmodem2` crate's.
//!
//! Downloads go to `<name>.ntpart` first and get their name when complete;
//! a `.ntpart` left by a broken transfer is continued where it stopped.
//! Where files go, and which files are sent, is asked with a picker (or
//! `NATIVETERM_ZMODEM_DIR` / `NATIVETERM_ZMODEM_FILES`, for tests and for
//! a fixed download folder).

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use zmodem2::{Action, Event, FileInfo, Position};

use crate::t;

/// Partial downloads.
const PART: &str = ".ntpart";
/// Nothing from the other side this long: remind it (zmodem2's timeout).
const NUDGE: Duration = Duration::from_secs(10);
/// Nothing at all this long: give up.
const GIVE_UP: Duration = Duration::from_secs(60);
/// Where the last folders are kept (HKCU).
#[cfg(windows)]
const REG_KEY: &str = r"Software\NativeTerm\Zmodem";
/// Written last on stdout: ssh gives the session back to the terminal.
/// Raw XOFF never appears in ZMODEM data (it is always escaped there);
/// the fork's `nativeterm/nt_zmodem.h` has the same bytes.
pub const END: &[u8] = b"\x13\x13\x13\x13";

/// The session's data both ways: chunks from the other side (a closed
/// channel is its end) and our bytes to it.
pub struct Wire<W: Write> {
    pub input: mpsc::Receiver<Vec<u8>>,
    pub output: W,
}

/// What a transfer tells the person watching.
pub trait Watch {
    fn file(&mut self, name: &str, size: Option<u64>, from: u64);
    fn progress(&mut self, done: u64);
    fn file_done(&mut self, name: &str, path: Option<&Path>);
    fn note(&mut self, text: &str);
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Done { files: usize, bytes: u64 },
    Cancelled,
    Failed(String),
}

/// Reads wire input for a little while; `false` once the other side is gone.
fn wait_input(input: &mpsc::Receiver<Vec<u8>>, into: &mut Vec<u8>) -> bool {
    match input.recv_timeout(Duration::from_millis(50)) {
        Ok(bytes) => {
            into.extend_from_slice(&bytes);
            while let Ok(more) = input.try_recv() {
                into.extend_from_slice(&more);
            }
            true
        }
        Err(mpsc::RecvTimeoutError::Timeout) => true,
        Err(mpsc::RecvTimeoutError::Disconnected) => false,
    }
}

/// A received name as a file name here: its last component, `_` for what
/// Windows doesn't allow, never empty, `.` or `..`.
pub fn local_name(raw: &[u8]) -> String {
    let name = String::from_utf8_lossy(raw);
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = last
        .chars()
        .map(|c| if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') { '_' } else { c })
        .collect();
    let clean = clean.trim_end_matches(['.', ' ']).trim().to_string();
    if clean.is_empty() || clean == "." || clean == ".." {
        "file".to_string()
    } else {
        clean
    }
}

/// `name`, or `name (2).ext`, ... : the first that isn't taken in `dir`.
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    if !path.exists() {
        return path;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).expect("a free name")
}

/// Receive what the server's `sz` sends into `dir`; `escape`: ask the
/// sender to escape every control character (ESCCTL), which a link that
/// is not transparent needs (Telnet without binary mode changes CR, NUL
/// and 0xFF).
pub fn receive<W: Write>(
    mut wire: Wire<W>,
    dir: &Path,
    watch: &mut dyn Watch,
    cancel: &AtomicBool,
    escape: bool,
) -> Outcome {
    let Ok(mut receiver) = zmodem2::Receiver::new() else { return Outcome::Failed("zmodem".into()) };
    // where each file starts is ours to say: after its partial file
    receiver.set_manual_file_accept(true);
    if escape && receiver.set_escape_control(true).is_err() {
        return Outcome::Failed("zmodem".into());
    }
    let mut input = Vec::new();
    let mut quiet = Instant::now();
    let mut open: Option<(File, PathBuf, String)> = None;
    // what the server says the file was last changed (ZFILE), kept on the
    // file here
    let mut modified: Option<std::time::SystemTime> = None;
    let (mut files, mut bytes) = (0usize, 0u64);
    let mut done = 0u64;
    let mut gone = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return abort_receiver(&mut receiver, &mut wire);
        }
        match receiver.poll() {
            Action::WriteWire(data) => {
                let n = data.len();
                if wire.output.write_all(data).and_then(|()| wire.output.flush()).is_err() {
                    return Outcome::Failed(t!("zmodem-gone"));
                }
                receiver.wire_written(n);
            }
            Action::WriteFile(data) => {
                let n = data.len();
                let Some((file, _, _)) = open.as_mut() else { return Outcome::Failed("no file".into()) };
                if let Err(e) = file.write_all(data) {
                    watch.note(&e.to_string());
                    return abort_receiver(&mut receiver, &mut wire);
                }
                done += n as u64;
                bytes += n as u64;
                watch.progress(done);
                if receiver.file_written(n).is_err() {
                    return Outcome::Failed("zmodem".into());
                }
            }
            Action::Event(Event::FileStarted(info)) => {
                let name = local_name(info.name);
                let size = info.size.map(|s| u64::from(s.get()));
                modified = info.modified.map(|secs| std::time::UNIX_EPOCH + Duration::from_secs(u64::from(secs)));
                let part = dir.join(format!("{name}{PART}"));
                let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
                let resume = size.is_some_and(|s| have > 0 && have < s);
                let start = if resume { have } else { 0 };
                let file = OpenOptions::new().create(true).write(true).append(resume).truncate(!resume).open(&part);
                let file = match file {
                    Ok(f) => f,
                    Err(e) => {
                        watch.note(&format!("{}: {e}", part.display()));
                        return abort_receiver(&mut receiver, &mut wire);
                    }
                };
                watch.file(&name, size, start);
                done = start;
                if receiver.accept_file_at(start as u32).is_err() {
                    return Outcome::Failed("zmodem".into());
                }
                open = Some((file, part, name));
            }
            Action::Event(Event::FileCompleted) => {
                if let Some((file, part, name)) = open.take() {
                    if let Some(modified) = modified.take() {
                        let _ = file.set_modified(modified);
                    }
                    drop(file);
                    let path = free_name(dir, &name);
                    let placed = std::fs::rename(&part, &path).map(|()| path);
                    watch.file_done(&name, placed.as_deref().ok());
                    files += 1;
                }
            }
            Action::Event(Event::SessionCompleted) => {
                // the closing ZFIN may still be queued: the sender waits for it
                while let Action::WriteWire(data) = receiver.poll() {
                    let n = data.len();
                    if wire.output.write_all(data).is_err() {
                        break;
                    }
                    receiver.wire_written(n);
                }
                // (its closing "OO" is hidden by ssh: waiting for it here could
                // take the shell's prompt that comes right after)
                let _ = wire.output.flush();
                return Outcome::Done { files, bytes };
            }
            Action::Event(Event::Aborted) => return Outcome::Failed(t!("zmodem-aborted")),
            Action::Event(_) => {}
            Action::Idle => {
                if !input.is_empty() {
                    match receiver.submit_wire(&input) {
                        Ok(used) => {
                            input.drain(..used);
                            if used > 0 {
                                continue;
                            }
                        }
                        Err(e) => return Outcome::Failed(format!("{e:?}")),
                    }
                }
                if gone {
                    return Outcome::Failed(t!("zmodem-gone"));
                }
                let before = input.len();
                gone = !wait_input(&wire.input, &mut input);
                if input.len() > before {
                    quiet = Instant::now();
                } else if quiet.elapsed() > GIVE_UP {
                    return abort_receiver(&mut receiver, &mut wire);
                } else if quiet.elapsed() > NUDGE && receiver.timeout().is_err() {
                    return Outcome::Failed(t!("zmodem-timeout"));
                }
            }
            _ => {}
        }
    }
}

fn abort_receiver<W: Write>(receiver: &mut zmodem2::Receiver, wire: &mut Wire<W>) -> Outcome {
    let _ = receiver.abort();
    // the cancel abort() queued (ten CAN, ten backspaces): out it goes,
    // and it is what makes the server's sz give up
    while let Action::WriteWire(data) = receiver.poll() {
        let n = data.len();
        if wire.output.write_all(data).and_then(|()| wire.output.flush()).is_err() {
            break;
        }
        receiver.wire_written(n);
    }
    drain_other_side(wire);
    Outcome::Cancelled
}

fn abort_sender<W: Write>(sender: &mut zmodem2::Sender, wire: &mut Wire<W>) -> Outcome {
    sender.abort();
    while let Action::WriteWire(data) = sender.poll() {
        let n = data.len();
        if wire.output.write_all(data).and_then(|()| wire.output.flush()).is_err() {
            break;
        }
        sender.wire_written(n);
    }
    drain_other_side(wire);
    Outcome::Cancelled
}

/// Silence this long ends the draining after a cancel.
const QUIET: Duration = Duration::from_millis(500);
/// A sender that doesn't stop: at most this long.
const DRAIN_AT_MOST: Duration = Duration::from_secs(15);

/// Set when a cancel had to take in data the other side had sent ahead: the
/// shell's prompt after it may have gone with that data.
static DRAINED: AtomicBool = AtomicBool::new(false);

/// Takes in what the other side still sends after a cancel, until it is
/// quiet (as lrzsz does): a sender streams ahead of what it knows, and
/// that data, left for the terminal, would show as garbage (and its escape
/// sequences would make the terminal answer into the shell).
fn drain_other_side<W: Write>(wire: &mut Wire<W>) {
    let until = Instant::now() + DRAIN_AT_MOST;
    while Instant::now() < until {
        match wire.input.recv_timeout(QUIET) {
            Ok(_) => DRAINED.store(true, Ordering::Relaxed),
            Err(_) => return,
        }
    }
}

/// Send `files` to the server's `rz`. Files over 4 GB (ZMODEM's limit) are
/// left out with a note.
/// Send `files` to the server's `rz`; `escape`: escape every control
/// character, which a link that is not transparent needs (the server's
/// `rz` asks for it only when started with `-e`).
pub fn send<W: Write>(
    mut wire: Wire<W>,
    files: &[PathBuf],
    watch: &mut dyn Watch,
    cancel: &AtomicBool,
    escape: bool,
) -> Outcome {
    let Ok(mut sender) = zmodem2::Sender::new() else { return Outcome::Failed("zmodem".into()) };
    // a reliable transport: no pause for acknowledgements
    sender.set_streaming_window(usize::MAX);
    sender.set_escape_control(escape);
    let mut queue: Vec<&PathBuf> = Vec::new();
    for path in files {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX);
        if size > u64::from(u32::MAX) {
            watch.note(&t!("zmodem-too-big", name = path.display().to_string()));
        } else {
            queue.push(path);
        }
    }
    let mut queue = queue.into_iter();
    let mut current = match start_next(&mut sender, &mut queue, watch) {
        Ok(next) => next,
        Err(e) => return Outcome::Failed(e),
    };
    let mut input = Vec::new();
    let mut quiet = Instant::now();
    let mut buf = vec![0u8; 8192];
    let (mut sent, mut bytes) = (0usize, 0u64);
    let mut gone = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return abort_sender(&mut sender, &mut wire);
        }
        match sender.poll() {
            Action::WriteWire(data) => {
                let n = data.len();
                if wire.output.write_all(data).is_err() {
                    return Outcome::Failed(t!("zmodem-gone"));
                }
                sender.wire_written(n);
                // with no window the sender only idles at the end: what the
                // receiver says meanwhile (ZRPOS after an error, a cancel)
                // is taken as it comes
                while let Ok(more) = wire.input.try_recv() {
                    input.extend_from_slice(&more);
                }
                if !input.is_empty() {
                    match sender.submit_wire(&input) {
                        Ok(used) => {
                            input.drain(..used);
                        }
                        Err(e) => return Outcome::Failed(format!("{e:?}")),
                    }
                }
            }
            Action::ReadFile { offset, max_len } => {
                let Some((file, _)) = current.as_mut() else { return Outcome::Failed("no file".into()) };
                let want = max_len.min(buf.len());
                let read =
                    file.seek(SeekFrom::Start(u64::from(offset.get()))).and_then(|_| file.read(&mut buf[..want]));
                match read {
                    Ok(n) if n > 0 => {
                        if sender.submit_file(&buf[..n]).is_err() {
                            return Outcome::Failed("zmodem".into());
                        }
                        let end = u64::from(offset.get()) + n as u64;
                        watch.progress(end);
                        bytes += n as u64;
                    }
                    Ok(_) => return Outcome::Failed(t!("zmodem-short")),
                    Err(e) => {
                        watch.note(&e.to_string());
                        return abort_sender(&mut sender, &mut wire);
                    }
                }
            }
            Action::Event(Event::FileCompleted) => {
                if let Some((_, name)) = current.take() {
                    watch.file_done(&name, None);
                    sent += 1;
                }
                let _ = wire.output.flush();
                match start_next(&mut sender, &mut queue, watch) {
                    Ok(next) => current = next,
                    Err(e) => return Outcome::Failed(e),
                }
            }
            Action::Event(Event::SessionCompleted) => {
                // the closing "OO" may still be queued
                while let Action::WriteWire(data) = sender.poll() {
                    let n = data.len();
                    if wire.output.write_all(data).is_err() {
                        break;
                    }
                    sender.wire_written(n);
                }
                let _ = wire.output.flush();
                return Outcome::Done { files: sent, bytes };
            }
            Action::Event(Event::Aborted) => return Outcome::Failed(t!("zmodem-aborted")),
            Action::Event(_) => {}
            Action::Idle => {
                if wire.output.flush().is_err() {
                    return Outcome::Failed(t!("zmodem-gone"));
                }
                if !input.is_empty() {
                    match sender.submit_wire(&input) {
                        Ok(used) => {
                            input.drain(..used);
                            if used > 0 {
                                continue;
                            }
                        }
                        Err(e) => return Outcome::Failed(format!("{e:?}")),
                    }
                }
                if gone {
                    return Outcome::Failed(t!("zmodem-gone"));
                }
                let before = input.len();
                gone = !wait_input(&wire.input, &mut input);
                if input.len() > before {
                    quiet = Instant::now();
                } else if quiet.elapsed() > GIVE_UP {
                    return abort_sender(&mut sender, &mut wire);
                } else if quiet.elapsed() > NUDGE && sender.timeout().is_err() {
                    return Outcome::Failed(t!("zmodem-timeout"));
                }
            }
            _ => {}
        }
    }
}

/// Offers the next file of `queue` that opens (or ends the session when
/// there is none): the file and its name.
fn start_next<'a>(
    sender: &mut zmodem2::Sender,
    queue: &mut impl Iterator<Item = &'a PathBuf>,
    watch: &mut dyn Watch,
) -> Result<Option<(File, String)>, String> {
    for path in queue.by_ref() {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match File::open(path).and_then(|f| f.metadata().map(|m| (f, m))) {
            Ok((file, meta)) => {
                let size = meta.len();
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .and_then(|d| u32::try_from(d.as_secs()).ok());
                // 0o100_644: a regular file, read by all, written by its
                // owner (Windows has no Unix mode of its own)
                let mut info = FileInfo::new(name.as_bytes(), Some(Position::new(size as u32))).with_mode(0o100_644);
                if let Some(modified) = modified {
                    info = info.with_modified(modified);
                }
                sender.start_file(info).map_err(|e| format!("{e:?}"))?;
                watch.file(&name, Some(size), 0);
                return Ok(Some((file, name)));
            }
            Err(e) => watch.note(&format!("{}: {e}", path.display())),
        }
    }
    sender.finish().map_err(|e| format!("{e:?}"))?;
    Ok(None)
}

/// Progress on the tab's console, a line per file, updated a few times a
/// second.
struct Console {
    name: String,
    size: Option<u64>,
    started: Instant,
    from: u64,
    last: Instant,
    done: u64,
}

impl Console {
    fn new() -> Console {
        let now = Instant::now();
        Console { name: String::new(), size: None, started: now, from: 0, last: now, done: 0 }
    }

    fn line(&self) -> String {
        let secs = self.started.elapsed().as_secs_f64().max(0.001);
        let speed = (self.done.saturating_sub(self.from)) as f64 / secs;
        let size = self.size.map(mb).unwrap_or_else(|| "?".into());
        t!("zmodem-progress", name = self.name.as_str(), done = mb(self.done), size = size, speed = mb(speed as u64))
    }
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

impl Watch for Console {
    fn file(&mut self, name: &str, size: Option<u64>, from: u64) {
        let now = Instant::now();
        (self.name, self.size, self.started, self.from, self.done) = (name.to_string(), size, now, from, from);
        if from > 0 {
            eprint!("\r\n[NativeTerm] {}", t!("zmodem-resume", name = name, from = mb(from)));
        }
        eprint!("\r\n{}", self.line());
    }

    fn progress(&mut self, done: u64) {
        self.done = done;
        if self.last.elapsed() >= Duration::from_millis(250) {
            self.last = Instant::now();
            eprint!("\r\x1b[K{}", self.line());
        }
    }

    fn file_done(&mut self, _name: &str, path: Option<&Path>) {
        eprint!("\r\x1b[K{}", self.line());
        if let Some(path) = path {
            eprint!("  → {}", path.display());
        }
    }

    fn note(&mut self, text: &str) {
        eprint!("\r\n[NativeTerm] {text}");
    }
}

/// `NATIVETERM_ZMODEM_DEBUG=<file>`: the first 64 KB each way of the wire,
/// in hex, to `<file>.wire` (for diagnosing a transport that changes data).
#[derive(Clone)]
struct Tap(Option<Arc<std::sync::Mutex<(File, usize)>>>);

impl Tap {
    fn open() -> Tap {
        let file = std::env::var_os("NATIVETERM_ZMODEM_DEBUG").and_then(|p| {
            let mut p = std::path::PathBuf::from(p).into_os_string();
            p.push(".wire");
            OpenOptions::new().create(true).append(true).open(p).ok()
        });
        Tap(file.map(|f| Arc::new(std::sync::Mutex::new((f, 0)))))
    }

    fn log(&self, way: &str, bytes: &[u8]) {
        let Some(tap) = &self.0 else { return };
        let mut tap = tap.lock().unwrap_or_else(|e| e.into_inner());
        if tap.1 > 128 * 1024 {
            return;
        }
        tap.1 += bytes.len();
        let hex: String = bytes.iter().take(4096).map(|b| format!("{b:02x}")).collect();
        let _ = writeln!(tap.0, "{way} {}: {hex}", bytes.len());
    }
}

/// A writer that logs what it writes (see `Tap`).
struct Tapped<W: Write> {
    inner: W,
    tap: Tap,
}

impl<W: Write> Write for Tapped<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.tap.log("out", &buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// ntplink reads the keyboard itself (PuTTY's reader thread takes every
/// key): on Esc or Ctrl+C it sets this event, named after our pid.
#[cfg(windows)]
fn watch_cancel_event(cancel: Arc<AtomicBool>) {
    use windows::core::HSTRING;
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
    let name = HSTRING::from(format!("Local\\NativeTermZmodemCancel-{}", std::process::id()));
    // SAFETY: the name is an HSTRING alive through the call; the event is
    // ours, waited on by the thread it is moved to, and never closed (the
    // process ends with the transfer).
    let Ok(event) = (unsafe { CreateEventW(None, true, false, &name) }) else { return };
    let event = event.0 as isize;
    std::thread::spawn(move || {
        // SAFETY: a valid event handle of this process (see above).
        unsafe {
            WaitForSingleObject(windows::Win32::Foundation::HANDLE(event as *mut _), INFINITE);
        }
        cancel.store(true, Ordering::Relaxed);
    });
}

/// Esc or Ctrl+C in the tab cancels (ssh doesn't read the keyboard while
/// the transfer has the session).
#[cfg(unix)]
fn watch_keys(cancel: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let Ok(keys) = crate::console::KeyReader::open() else { return };
        loop {
            match keys.read_key(Duration::from_secs(1)) {
                Ok(Some('\u{1b}' | '\u{3}')) => {
                    cancel.store(true, Ordering::Relaxed);
                    return;
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    });
}

/// Esc or Ctrl+C in the tab cancels (ssh doesn't read the keyboard while
/// the transfer has the session).
#[cfg(windows)]
fn watch_keys(cancel: Arc<AtomicBool>) {
    watch_cancel_event(Arc::clone(&cancel));
    use windows::core::w;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{ReadConsoleInputW, INPUT_RECORD, KEY_EVENT};
    std::thread::spawn(move || {
        // SAFETY: the console input handle is opened and used on this
        // thread only; `records` is a local buffer ReadConsoleInputW fills
        // up to its length, and only key events' union members are read.
        unsafe {
            let Ok(handle) = CreateFileW(
                w!("CONIN$"),
                (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                Default::default(),
                None,
            ) else {
                return;
            };
            // NATIVETERM_ZMODEM_DEBUG=<file>: the console's input events, for
            // diagnosing cancel keys
            let mut log = std::env::var_os("NATIVETERM_ZMODEM_DEBUG")
                .and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok());
            let mut records = [INPUT_RECORD::default(); 16];
            loop {
                let mut read = 0u32;
                if ReadConsoleInputW(handle, &mut records, &mut read).is_err() {
                    return;
                }
                for r in &records[..read as usize] {
                    if u32::from(r.EventType) != KEY_EVENT {
                        continue;
                    }
                    let key = r.Event.KeyEvent;
                    let ch = key.uChar.UnicodeChar;
                    if let Some(log) = log.as_mut() {
                        let down = key.bKeyDown.as_bool();
                        let _ = writeln!(log, "key down={down} vk={:#x} ch={ch:#x}", key.wVirtualKeyCode);
                    }
                    // through ConPTY a key may come as its character only
                    let esc = key.wVirtualKeyCode == 0x1B || ch == 0x1B;
                    if key.bKeyDown.as_bool() && (esc || ch == 0x03) {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
            }
        }
    });
}

/// The folder last used for `name` (kept in the registry; nowhere yet
/// elsewhere).
fn remembered(name: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        native_term_os::registry::user_values(REG_KEY).ok()?.into_iter().find_map(|(n, v)| match v {
            native_term_os::registry::RegValue::Str(s) if n == name => Some(PathBuf::from(s)),
            _ => None,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        None
    }
}

fn remember(name: &str, folder: &Path) {
    #[cfg(windows)]
    {
        let value = native_term_os::registry::RegValue::Str(folder.display().to_string());
        let _ = native_term_os::registry::write_user_values(REG_KEY, &[(name, value)]);
    }
    #[cfg(not(windows))]
    {
        let _ = (name, folder);
    }
}

/// Where to put received files: a folder dialog on Windows; elsewhere the
/// Downloads folder (or the one remembered) without asking.
fn pick_folder(start: Option<&Path>) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        native_term_os::picker::pick_folder(&t!("zmodem-folder-title"), start)
    }
    #[cfg(not(windows))]
    {
        start.map(Path::to_path_buf)
    }
}

/// Which files to send: a file dialog on Windows; elsewhere none, so the
/// transfer is declined unless `NATIVETERM_ZMODEM_FILES` names them.
fn pick_files(start: Option<&Path>) -> Option<Vec<PathBuf>> {
    #[cfg(windows)]
    {
        native_term_os::picker::pick_files(&t!("zmodem-files-title"), start)
    }
    #[cfg(not(windows))]
    {
        let _ = start;
        None
    }
}

/// Asks NativeTerm (its pipe) to open the files window of this tab's
/// session; whether it could be asked.
fn ask_for_files() -> bool {
    use native_term_session::pipe;
    use native_term_session::protocol::{Role, ShimMessage};
    let name = std::env::var("NATIVETERM_PIPE").ok().or_else(|| pipe::pipe_name().ok());
    let Some(conn) = name.and_then(|n| pipe::connect(&n, Duration::from_millis(500)).ok()) else { return false };
    let hello = ShimMessage::Hello {
        protocol: native_term_session::PROTOCOL_VERSION,
        role: Role::Request,
        pid: std::process::id(),
        wt_session: std::env::var("WT_SESSION").ok().filter(|s| !s.is_empty()),
        session: None,
        alias: None,
        terminal_window: None,
    };
    conn.send(&hello).is_ok() && conn.send(&ShimMessage::OpenFiles).is_ok()
}

/// The helper: `mode` is `download` (the server ran `sz`), `upload` (it
/// ran `rz`), or `tmux` (one of them in tmux, which changes the data both
/// ways: it is stopped, and the files window offered instead). Exit code 0
/// when done, 1 when cancelled or failed.
pub fn run(mode: &str, escape: bool, files: bool) -> i32 {
    let (tx, input) = mpsc::channel();
    let tap = Tap::open();
    let tap_in = tap.clone();
    std::thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    tap_in.log("in", &buf[..n]);
                    if tx.send(buf[..n].to_vec()).is_err() {
                        return;
                    }
                }
            }
        }
    });
    let output = Tapped { inner: io::BufWriter::with_capacity(64 * 1024, io::stdout().lock()), tap };
    let wire = Wire { input, output };
    let cancel = Arc::new(AtomicBool::new(false));
    let mut console = Console::new();
    if mode == "tmux" {
        let mut wire = wire;
        if let Ok(mut receiver) = zmodem2::Receiver::new() {
            abort_receiver(&mut receiver, &mut wire);
        }
        let text = if !files {
            t!("zmodem-tmux-stopped")
        } else if ask_for_files() {
            t!("zmodem-tmux-files")
        } else {
            t!("zmodem-tmux")
        };
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(END).and_then(|()| stdout.flush());
        eprint!("\r\n[NativeTerm] {text}\r\n");
        return 1;
    }
    let outcome = match mode {
        "download" => {
            let dir = std::env::var_os("NATIVETERM_ZMODEM_DIR").map(PathBuf::from).or_else(|| {
                eprint!("\r\n[NativeTerm] {}", t!("zmodem-pick-folder"));
                let start = remembered("DownloadFolder").or_else(native_term_os::shell::downloads_folder);
                let dir = pick_folder(start.as_deref());
                if let Some(dir) = &dir {
                    remember("DownloadFolder", dir);
                }
                dir
            });
            match dir {
                Some(dir) => {
                    eprint!("\r\n[NativeTerm] {}", t!("zmodem-receiving", folder = dir.display().to_string()));
                    watch_keys(Arc::clone(&cancel));
                    receive(wire, &dir, &mut console, &cancel, escape)
                }
                None => {
                    let mut wire = wire;
                    let mut receiver = zmodem2::Receiver::new().expect("zmodem");
                    abort_receiver(&mut receiver, &mut wire)
                }
            }
        }
        "upload" => {
            let files: Option<Vec<PathBuf>> = std::env::var("NATIVETERM_ZMODEM_FILES")
                .ok()
                .map(|v| v.split('|').filter(|s| !s.is_empty()).map(PathBuf::from).collect())
                .or_else(|| {
                    eprint!("\r\n[NativeTerm] {}", t!("zmodem-pick-files"));
                    let start = remembered("UploadFolder");
                    let files = pick_files(start.as_deref());
                    if let Some(folder) = files.as_ref().and_then(|f| f.first()).and_then(|f| f.parent()) {
                        remember("UploadFolder", folder);
                    }
                    files
                });
            match files {
                Some(files) => {
                    eprint!("\r\n[NativeTerm] {}", t!("zmodem-sending", count = files.len()));
                    watch_keys(Arc::clone(&cancel));
                    send(wire, &files, &mut console, &cancel, escape)
                }
                None => {
                    let mut wire = wire;
                    let mut sender = zmodem2::Sender::new().expect("zmodem");
                    abort_sender(&mut sender, &mut wire)
                }
            }
        }
        other => Outcome::Failed(format!("unknown mode {other:?}")),
    };
    // the session goes back to the terminal (the wire's writer was
    // flushed when it was dropped)
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(END).and_then(|()| stdout.flush());
    let (text, code) = match outcome {
        Outcome::Done { files, bytes } => (t!("zmodem-done", files = files, size = mb(bytes)), 0),
        Outcome::Cancelled if DRAINED.load(Ordering::Relaxed) => (t!("zmodem-cancelled-enter"), 1),
        Outcome::Cancelled => (t!("zmodem-cancelled"), 1),
        Outcome::Failed(e) => (t!("zmodem-failed", error = e), 1),
    };
    eprint!("\r\n[NativeTerm] {text}\r\n");
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Quiet;
    impl Watch for Quiet {
        fn file(&mut self, _: &str, _: Option<u64>, _: u64) {}
        fn progress(&mut self, _: u64) {}
        fn file_done(&mut self, _: &str, _: Option<&Path>) {}
        fn note(&mut self, _: &str) {}
    }

    /// Writes to a channel: one side's output is the other's input. Like
    /// ssh, which keeps reading, it takes bytes after the other side is
    /// done (the sender's closing "OO").
    struct ToChannel(mpsc::Sender<Vec<u8>>);
    impl Write for ToChannel {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let _ = self.0.send(buf.to_vec());
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn pair() -> (Wire<ToChannel>, Wire<ToChannel>) {
        let (a_tx, a_rx) = mpsc::channel();
        let (b_tx, b_rx) = mpsc::channel();
        (Wire { input: a_rx, output: ToChannel(b_tx) }, Wire { input: b_rx, output: ToChannel(a_tx) })
    }

    fn pattern(len: usize, seed: u8) -> Vec<u8> {
        (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
    }

    #[test]
    fn upload_escaped_for_telnet() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let file = src.path().join("all.bin");
        // every byte value, often
        let content: Vec<u8> = (0..200_000u32).map(|i| (i.wrapping_mul(7919) % 256) as u8).collect();
        std::fs::write(&file, &content).unwrap();
        let (ours, theirs) = pair();
        let files = vec![file];
        let sender = std::thread::spawn(move || send(theirs, &files, &mut Quiet, &AtomicBool::new(false), true));
        let got = receive(ours, dst.path(), &mut Quiet, &AtomicBool::new(false), false);
        assert!(matches!(sender.join().unwrap(), Outcome::Done { files: 1, .. }));
        assert_eq!(got, Outcome::Done { files: 1, bytes: 200_000 });
        assert_eq!(std::fs::read(dst.path().join("all.bin")).unwrap(), content);
    }

    #[test]
    fn names_from_the_server() {
        assert_eq!(local_name(b"/tmp/a/report.txt"), "report.txt");
        assert_eq!(local_name("中文 文件.log".as_bytes()), "中文 文件.log");
        assert_eq!(local_name(b"a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(local_name(b"../.."), "file");
        assert_eq!(local_name(b"dir\\evil. "), "evil");
        assert_eq!(local_name(b""), "file");
    }

    #[test]
    fn both_ways_with_resume_and_names() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let big = src.path().join("big.bin");
        std::fs::write(&big, pattern(300_000, 7)).unwrap();
        let cn = src.path().join("中文.txt");
        std::fs::write(&cn, "中文内容").unwrap();
        let empty = src.path().join("empty.dat");
        std::fs::write(&empty, b"").unwrap();
        // a broken earlier download of big.bin, and a file of that name
        std::fs::write(dst.path().join("big.bin.ntpart"), &pattern(300_000, 7)[..120_000]).unwrap();
        std::fs::write(dst.path().join("中文.txt"), "older").unwrap();

        let (ours, theirs) = pair();
        let files = vec![big.clone(), cn.clone(), empty.clone()];
        let never = AtomicBool::new(false);
        let sender = std::thread::spawn(move || send(theirs, &files, &mut Quiet, &AtomicBool::new(false), false));
        let got = receive(ours, dst.path(), &mut Quiet, &never, false);
        let sent = sender.join().unwrap();
        assert_eq!(got, Outcome::Done { files: 3, bytes: 300_000 - 120_000 + 12 });
        assert!(matches!(sent, Outcome::Done { files: 3, .. }), "{sent:?}");
        assert_eq!(std::fs::read(dst.path().join("big.bin")).unwrap(), pattern(300_000, 7));
        assert_eq!(std::fs::read_to_string(dst.path().join("中文 (2).txt")).unwrap(), "中文内容");
        assert_eq!(std::fs::read_to_string(dst.path().join("中文.txt")).unwrap(), "older", "kept");
        assert_eq!(std::fs::read(dst.path().join("empty.dat")).unwrap(), b"");
        assert!(!dst.path().join("big.bin.ntpart").exists());
    }

    #[test]
    fn cancelling_stops_both_sides() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let big = src.path().join("big.bin");
        std::fs::write(&big, pattern(2_000_000, 3)).unwrap();
        let (ours, theirs) = pair();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancel);
        // the sender cancels once it is under way
        struct CancelSoon(Arc<AtomicBool>);
        impl Watch for CancelSoon {
            fn file(&mut self, _: &str, _: Option<u64>, _: u64) {}
            fn progress(&mut self, done: u64) {
                if done > 100_000 {
                    self.0.store(true, Ordering::Relaxed);
                }
            }
            fn file_done(&mut self, _: &str, _: Option<&Path>) {}
            fn note(&mut self, _: &str) {}
        }
        let files = vec![big];
        let sender = std::thread::spawn(move || send(theirs, &files, &mut CancelSoon(stop), &cancel, true));
        let got = receive(ours, dst.path(), &mut Quiet, &AtomicBool::new(false), true);
        assert_eq!(sender.join().unwrap(), Outcome::Cancelled);
        assert!(!matches!(got, Outcome::Done { .. }), "{got:?}");
        // the partial file stays for a later resume
        assert!(dst.path().join("big.bin.ntpart").exists());
        assert!(!dst.path().join("big.bin").exists());
    }
}
