//! Logon actions, done here (the table and its keys are
//! `native_term_config::logon`): what the session writes to the terminal
//! reaches this shim through the session log's pipe (`session_log`), is
//! read as text the way the log reads it (escape sequences left out, the
//! session's charset), and each row waits for its Expect in what came
//! after the row before; then its Send is typed, as the person would type
//! it (the console on Windows, WezTerm's `send-text` elsewhere).
//!
//! - An SSH session starts at the server's first output: that is after
//!   the login (the password prompts are ssh's own and never pass through
//!   the pipe), so nothing is typed into a prompt of ssh's. A Telnet or
//!   serial session starts with the client: its login prompt is the
//!   server's, and the table is what answers it.
//! - "Send initial carriage return" and rows without an Expect go at the
//!   start (and after the row before them).
//! - A single thread types, row after row, so a `\p` pause holds up
//!   neither the log nor the rows' order; a connection that ends stops
//!   whatever was still to be typed for it.
//! - Passwords (`\w`) and hidden Sends are read from the system's password
//!   store when typed and never written anywhere: not to the log (what is
//!   typed is not output), not to the debug log.
//! - What cannot be typed (a hidden Send not stored on this computer, no
//!   password saved) is left out and NativeTerm is told
//!   (`ShimMessage::LogonNote`); the rows after it go on.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use native_term_config::logon::{self, LogonActions, Piece, Step};
use native_term_config::password;
use native_term_session::protocol::ShimMessage;

/// How much of the recent text is kept to look for an Expect in.
const TAIL: usize = 16 * 1024;

/// What the rows need to know of the connection.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// The session's user (`\s` without credentials).
    pub user: Option<String>,
    /// The password store entry of the session's own saved password
    /// (`\w` without credentials).
    pub password_entry: Option<String>,
    /// SSH (starts at the first output) or not (starts at once).
    pub ssh: bool,
    /// The session's charset, as the log reads it.
    pub charset: Option<String>,
}

/// The connection the rows being typed belong to: a new one (or none)
/// makes the rest of the old one's go.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);
static WORKER: OnceLock<Mutex<Sender<Work>>> = OnceLock::new();
static LINK: OnceLock<crate::link::LinkSender> = OnceLock::new();

/// A session's logon actions (its own, else its folder's), read again
/// before every connection.
pub fn for_alias(alias: &str) -> LogonActions {
    let tree = native_term_config::tree::SessionTree::load(&crate::plink::ssh_dir());
    tree.find(alias).map(|(folder, host)| LogonActions::for_host(folder, host)).unwrap_or_default()
}

/// What an SSH connection's rows need: the user and saved password entry
/// from `ssh -G`.
pub fn ssh_context(effective: &[(String, String)], charset: Option<String>) -> Context {
    let user = effective.iter().find(|(k, _)| k == "user").map(|(_, v)| v.clone()).filter(|u| !u.is_empty());
    Context { user, password_entry: password::target(effective, None).map(|t| t.name), ssh: true, charset }
}

/// Where the notes go (NativeTerm), once per tab.
pub fn init(link: Option<&crate::link::Link>) {
    if let Some(link) = link {
        let _ = LINK.set(link.sender());
    }
}

/// A connection starts (the client was just started): its table, if it
/// has one to work through.
pub fn start(actions: &LogonActions, context: Context) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let mut engine = lock();
    *engine = None;
    if !actions.active() {
        return;
    }
    crate::debug::log(format!(
        "logon: {} row(s), {}",
        actions.steps.len(),
        if context.ssh { "ssh" } else { "at once" }
    ));
    let decoder =
        context.charset.as_deref().and_then(crate::session_log::encoding).map(|e| e.new_decoder_without_bom_handling());
    let mut new = Engine {
        steps: actions.steps.clone(),
        initial_cr: actions.initial_cr,
        begun: false,
        next: 0,
        context,
        generation,
        parser: vte::Parser::new(),
        screen: Screen::default(),
        decoder,
    };
    if !new.context.ssh {
        new.begin();
    }
    *engine = Some(new);
}

/// The connection ended: nothing more is typed for it.
pub fn stop() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    *lock() = None;
}

/// Output of the session (from the log's pipe).
pub fn output(bytes: &[u8]) {
    if let Some(engine) = lock().as_mut() {
        engine.feed(bytes);
    }
}

fn lock() -> std::sync::MutexGuard<'static, Option<Engine>> {
    ENGINE.lock().unwrap_or_else(|e| e.into_inner())
}

struct Engine {
    steps: Vec<Step>,
    initial_cr: bool,
    begun: bool,
    /// The row waited for.
    next: usize,
    context: Context,
    generation: u64,
    parser: vte::Parser,
    screen: Screen,
    decoder: Option<encoding_rs::Decoder>,
}

impl Engine {
    /// The initial carriage return and the rows without an Expect before
    /// the first that has one.
    fn begin(&mut self) {
        self.begun = true;
        if self.initial_cr {
            self.send(None, Job::Enter);
        }
        self.advance();
    }

    fn feed(&mut self, bytes: &[u8]) {
        if !self.begun {
            self.begin();
        }
        if self.next >= self.steps.len() {
            return;
        }
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
                self.parser.advance(&mut self.screen, text.as_bytes());
            }
            None => self.parser.advance(&mut self.screen, bytes),
        }
        self.advance();
    }

    /// The rows whose Expect has come (or that have none), in order.
    fn advance(&mut self) {
        while let Some(step) = self.steps.get(self.next) {
            if !step.expect.is_empty() {
                match self.screen.text.find(&step.expect) {
                    // what came after it is what the next row looks in
                    Some(at) => {
                        self.screen.text.drain(..at + step.expect.len());
                    }
                    None => return,
                }
            }
            let row = self.next + 1;
            self.send(Some(row), Job::Step(step.clone()));
            self.next += 1;
        }
    }

    fn send(&self, row: Option<usize>, job: Job) {
        let worker = WORKER.get_or_init(|| Mutex::new(spawn_worker()));
        let sent = worker.lock().unwrap_or_else(|e| e.into_inner()).send(Work {
            generation: self.generation,
            row,
            job,
            context: self.context.clone(),
        });
        if sent.is_err() {
            crate::debug::log("logon: the typing thread is gone");
        }
    }
}

/// The text of the output, escape sequences left out, Backspace taking
/// back the character before it, carriage returns dropped: what the
/// person sees, as the log reads it.
#[derive(Default)]
struct Screen {
    text: String,
}

impl Screen {
    fn keep_tail(&mut self) {
        if self.text.len() > TAIL {
            let mut cut = self.text.len() - TAIL / 2;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            self.text.drain(..cut);
        }
    }
}

impl vte::Perform for Screen {
    fn print(&mut self, c: char) {
        self.text.push(c);
        self.keep_tail();
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.text.push('\n'),
            b'\t' => self.text.push('\t'),
            0x08 => {
                self.text.pop();
            }
            _ => {}
        }
    }
}

enum Job {
    /// The initial carriage return.
    Enter,
    Step(Step),
}

struct Work {
    generation: u64,
    /// The row, from 1 (none: the initial carriage return).
    row: Option<usize>,
    job: Job,
    context: Context,
}

fn spawn_worker() -> Sender<Work> {
    let (sender, receiver) = mpsc::channel::<Work>();
    let started = std::thread::Builder::new().name("logon".into()).spawn(move || {
        for work in receiver {
            if work.generation == GENERATION.load(Ordering::SeqCst) {
                run(&work);
            }
        }
    });
    if let Err(e) = started {
        crate::debug::log(format!("logon: no typing thread: {e}"));
    }
    sender
}

/// Types one job, piece by piece; stops where the connection ended.
fn run(work: &Work) {
    let current = || work.generation == GENERATION.load(Ordering::SeqCst);
    let step = match &work.job {
        Job::Enter => {
            type_text("\r");
            return;
        }
        Job::Step(step) => step,
    };
    let row = work.row.unwrap_or(0);
    let send = if step.hide {
        match logon::secret_id(&step.send) {
            Some(id) => match stored(&logon::secret_entry(id)) {
                Some(saved) => saved.secret,
                None => {
                    note(crate::t!("logon-hidden-missing", row = row));
                    return;
                }
            },
            // (a hidden Send written by hand into the configuration)
            None => step.send.clone(),
        }
    } else {
        step.send.clone()
    };
    let credentials = || -> (Option<String>, Option<String>) {
        match &step.credential {
            Some(set) => {
                let saved = stored(&password::set_entry(set));
                let user = saved
                    .as_ref()
                    .map(|s| s.user.clone())
                    .filter(|u| !u.is_empty())
                    .or_else(|| work.context.user.clone());
                (user, saved.map(|s| s.secret))
            }
            None => {
                (work.context.user.clone(), work.context.password_entry.as_deref().and_then(stored).map(|s| s.secret))
            }
        }
    };
    crate::debug::log(format!("logon: row {row}"));
    for piece in logon::pieces(&send) {
        if !current() {
            return;
        }
        match piece {
            Piece::Text(text) => type_text(&text),
            Piece::Pause => std::thread::sleep(Duration::from_secs(1)),
            Piece::User => match credentials().0 {
                Some(user) => type_text(&user),
                None => note(crate::t!("logon-no-user", row = row)),
            },
            Piece::Password => match credentials().1 {
                Some(mut secret) => {
                    type_text(&secret);
                    // (the copy made for typing goes at once)
                    secret.clear();
                }
                None => note(crate::t!("logon-no-password", row = row)),
            },
            Piece::Clipboard => match clipboard() {
                Some(text) => type_text(&text),
                None => note(crate::t!("logon-no-clipboard", row = row)),
            },
        }
    }
    if step.enter && current() {
        type_text("\r");
    }
}

fn stored(entry: &str) -> Option<native_term_os::credentials::Saved> {
    native_term_os::credentials::read(entry).ok().flatten()
}

fn type_text(text: &str) {
    if let Err(e) = crate::console::inject(text, false) {
        crate::debug::log(format!("logon: typing failed: {e}"));
    }
}

fn clipboard() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok().filter(|t| !t.is_empty())
}

fn note(text: String) {
    crate::debug::log(format!("logon: {text}"));
    if let Some(link) = LINK.get() {
        link.send(ShimMessage::LogonNote { text });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(chunks: &[&[u8]]) -> String {
        let mut parser = vte::Parser::new();
        let mut screen = Screen::default();
        for chunk in chunks {
            parser.advance(&mut screen, chunk);
        }
        screen.text
    }

    #[test]
    fn the_text_as_seen() {
        assert_eq!(screen(&[b"\x1b[1;32muser@web\x1b[0m:~$ "]), "user@web:~$ ");
        assert_eq!(screen(&[b"Pass", b"word: "]), "Password: ", "split across reads");
        assert_eq!(screen(&[b"logn\x08in:\r\n"]), "login:\n");
        assert_eq!(screen(&["登录：".as_bytes()]), "登录：");
    }

    fn engine(steps: &[(&str, &str)]) -> Engine {
        Engine {
            steps: steps
                .iter()
                .map(|(e, s)| Step { expect: e.to_string(), send: s.to_string(), enter: true, ..Step::default() })
                .collect(),
            initial_cr: false,
            begun: true,
            next: 0,
            context: Context::default(),
            // (not the current connection: nothing is typed)
            generation: 0,
            parser: vte::Parser::new(),
            screen: Screen::default(),
            decoder: None,
        }
    }

    #[test]
    fn rows_wait_for_their_expect_in_order() {
        let mut e = engine(&[("login:", "root"), ("Password:", "x"), ("", "id"), ("#", "exit")]);
        e.feed(b"Welcome\r\nPassword: ");
        assert_eq!(e.next, 0, "the first row's Expect first");
        e.feed(b"\r\nlogin: ");
        assert_eq!(e.next, 1);
        e.feed(b"root\r\n");
        assert_eq!(e.next, 1, "the old Password: was before login:");
        e.feed(b"Pass");
        e.feed(b"word: ");
        assert_eq!(e.next, 3, "and the row without an Expect with it");
        e.feed(b"\r\nlast login\r\nroot@sw1 #");
        assert_eq!(e.next, 4);
        e.feed(b"#");
        assert_eq!(e.next, 4, "done");
    }

    #[test]
    fn a_charset_is_read_as_what_it_is() {
        let mut e = engine(&[("用户名:", "admin")]);
        e.decoder = crate::session_log::encoding("gbk").map(|e| e.new_decoder_without_bom_handling());
        let (gbk, _, _) = encoding_rs::GBK.encode("请输入用户名:");
        e.feed(&gbk);
        assert_eq!(e.next, 1);
    }

    #[test]
    fn the_tail_is_kept_short() {
        let mut e = engine(&[("never", "x")]);
        for _ in 0..100 {
            e.feed(&[b'a'; 1000]);
        }
        assert!(e.screen.text.len() <= TAIL);
    }
}
