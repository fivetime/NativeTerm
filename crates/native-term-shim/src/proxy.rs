//! `nativeterm-shim --proxy <url> <host> <port>`: ssh's `ProxyCommand`
//! helper (see `native_term_config::proxy`). It connects to the proxy,
//! asks it for a connection to `host:port` (SOCKS5, SOCKS4a or HTTP
//! `CONNECT`; the proxy resolves the name), then passes bytes between
//! ssh (stdin / stdout) and that connection until either side closes.
//! Messages go to stderr, which ssh shows (or a background run reports).
//!
//! A login: the user name is in the URL, the password in Credential
//! Manager (`Proxy::password_entry`). SOCKS5 sends both (RFC 1929), HTTP
//! as `Basic`, SOCKS4 the user id only. A password the proxy refuses is
//! marked and not used again until a new one is saved: background runs
//! and reconnects retrying it could lock a directory account.
//!
//! No crate: the `socks` crate's last release is from 2022 and brings the
//! old `winapi`; the three handshakes are a few dozen lines each.

use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, TcpStream, ToSocketAddrs};
use std::time::Duration;

use native_term_config::password::REFUSED;
use native_term_config::proxy::{Kind, Proxy};
use native_term_os::credentials;

use crate::t;

/// How long connecting to the proxy may take.
const TIMEOUT: Duration = Duration::from_secs(20);
/// How long the proxy may take to answer (one of another type waits for
/// more of its own protocol and never does).
const ANSWER: Duration = Duration::from_secs(10);

/// Why the proxy gave no connection.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Fail {
    /// It refused the user name or password.
    Login,
    Other(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fail::Login => f.write_str(&t!("proxy-login-refused")),
            Fail::Other(text) => f.write_str(text),
        }
    }
}

impl From<String> for Fail {
    fn from(text: String) -> Fail {
        Fail::Other(text)
    }
}

/// A user name and password.
pub(crate) type Login<'a> = Option<(&'a str, &'a str)>;

pub fn run(url: &str, host: &str, port: &str) -> i32 {
    let result = Proxy::parse(url).map_err(Fail::Other).and_then(|proxy| {
        let port = port.parse::<u16>().map_err(|_| Fail::Other(format!("invalid port {port:?}")))?;
        connect(&proxy, host, port)
    });
    match result {
        Ok(stream) => relay(stream),
        Err(e) => {
            let error = e.to_string();
            eprintln!("[NativeTerm] {}", t!("proxy-failed", url = url, host = host, port = port, error = error));
            1
        }
    }
}

/// The login's password from Credential Manager.
fn password(proxy: &Proxy) -> Result<Option<String>, Fail> {
    let Some(entry) = proxy.password_entry() else { return Ok(None) };
    match credentials::read(&entry) {
        Ok(Some(saved)) if saved.comment == REFUSED => Err(Fail::Other(t!("proxy-password-refused"))),
        Ok(Some(saved)) => Ok(Some(saved.secret)),
        Ok(None) => Err(Fail::Other(t!("proxy-no-password", proxy = proxy.url()))),
        Err(e) => Err(Fail::Other(e.to_string())),
    }
}

/// The password was refused: it isn't sent again until a new one is saved.
fn mark_refused(proxy: &Proxy) {
    let Some(entry) = proxy.password_entry() else { return };
    if let Ok(Some(mut saved)) = credentials::read(&entry) {
        saved.comment = REFUSED.to_string();
        let _ = credentials::write(&entry, &saved);
    }
}

/// A connection to `host:port` through the proxy.
fn connect(proxy: &Proxy, host: &str, port: u16) -> Result<TcpStream, Fail> {
    let secret = password(proxy)?;
    let addresses: Vec<_> = (proxy.host.as_str(), proxy.port)
        .to_socket_addrs()
        .map_err(|e| t!("proxy-unknown", proxy = proxy.address(), error = e.to_string()))?
        .collect();
    let mut last = None;
    let mut stream = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, TIMEOUT) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let mut stream = stream.ok_or_else(|| {
        let error = last.map(|e| e.to_string()).unwrap_or_default();
        t!("proxy-unreachable", proxy = proxy.address(), error = error)
    })?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(ANSWER));
    let user = proxy.user.as_deref();
    let login = user.zip(secret.as_deref());
    let done = match proxy.kind {
        Kind::Socks5 => socks5(&mut stream, host, port, login),
        Kind::Socks4 => socks4(&mut stream, host, port, user),
        Kind::Http => http(&mut stream, &proxy.host, host, port, login),
    };
    drop(secret);
    if done == Err(Fail::Login) && proxy.takes_password() {
        mark_refused(proxy);
    }
    done?;
    let _ = stream.set_read_timeout(None);
    Ok(stream)
}

fn io_error(e: io::Error) -> String {
    match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => t!("proxy-no-answer"),
        _ => t!("proxy-broken", error = e.to_string()),
    }
}

/// SOCKS5 (RFC 1928), with a user name and password (RFC 1929) if given.
pub(crate) fn socks5(stream: &mut (impl Read + Write), host: &str, port: u16, login: Login) -> Result<(), Fail> {
    let offer: &[u8] = if login.is_some() { &[5, 2, 0, 2] } else { &[5, 1, 0] };
    stream.write_all(offer).map_err(io_error)?;
    let mut choice = [0u8; 2];
    stream.read_exact(&mut choice).map_err(io_error)?;
    match (choice, login) {
        ([5, 0], _) => {}
        ([5, 2], Some((user, password))) => {
            let (user, password) = (user.as_bytes(), password.as_bytes());
            let too_long = || Fail::Other(t!("proxy-login-too-long"));
            let mut request = vec![1, u8::try_from(user.len()).map_err(|_| too_long())?];
            request.extend(user);
            request.push(u8::try_from(password.len()).map_err(|_| too_long())?);
            request.extend(password);
            let written = stream.write_all(&request);
            request.fill(0);
            written.map_err(io_error)?;
            let mut status = [0u8; 2];
            stream.read_exact(&mut status).map_err(io_error)?;
            if status[1] != 0 {
                return Err(Fail::Login);
            }
        }
        ([5, 0xff], _) | ([5, 2], None) => return Err(Fail::Other(t!("proxy-needs-login"))),
        _ => return Err(Fail::Other(t!("proxy-not-socks", kind = "SOCKS5"))),
    }
    let mut request = vec![5, 1, 0];
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            request.push(1);
            request.extend(ip.octets());
        }
        Ok(IpAddr::V6(ip)) => {
            request.push(4);
            request.extend(ip.octets());
        }
        Err(_) => {
            let name = host.as_bytes();
            let len = u8::try_from(name.len()).map_err(|_| format!("host name too long: {host}"))?;
            request.push(3);
            request.push(len);
            request.extend(name);
        }
    }
    request.extend(port.to_be_bytes());
    stream.write_all(&request).map_err(io_error)?;
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).map_err(io_error)?;
    if head[0] != 5 {
        return Err(Fail::Other(t!("proxy-not-socks", kind = "SOCKS5")));
    }
    if head[1] != 0 {
        return Err(Fail::Other(t!("proxy-refused", reason = socks5_reason(head[1]))));
    }
    // the address the proxy bound, then its port
    let rest = match head[3] {
        1 => 4 + 2,
        4 => 16 + 2,
        3 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).map_err(io_error)?;
            len[0] as usize + 2
        }
        _ => return Err(Fail::Other(t!("proxy-not-socks", kind = "SOCKS5"))),
    };
    let mut skip = vec![0u8; rest];
    stream.read_exact(&mut skip).map_err(io_error)?;
    Ok(())
}

fn socks5_reason(code: u8) -> String {
    match code {
        2 => "not allowed by the proxy's rules".into(),
        3 => "network unreachable".into(),
        4 => "host unreachable".into(),
        5 => "connection refused".into(),
        6 => "timed out".into(),
        7 => "command not supported".into(),
        8 => "address type not supported".into(),
        n => format!("failure {n}"),
    }
}

/// SOCKS4a: the proxy resolves the name (an IPv4 address is sent as is);
/// `user` is the user id.
pub(crate) fn socks4(stream: &mut (impl Read + Write), host: &str, port: u16, user: Option<&str>) -> Result<(), Fail> {
    let mut request = vec![4, 1];
    request.extend(port.to_be_bytes());
    let user = user.unwrap_or("").as_bytes();
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            request.extend(ip.octets());
            request.extend(user);
            request.push(0);
        }
        Ok(IpAddr::V6(_)) => return Err(Fail::Other(t!("proxy-socks4-v6"))),
        Err(_) => {
            request.extend([0, 0, 0, 1]);
            request.extend(user);
            request.push(0);
            request.extend(host.as_bytes());
            request.push(0);
        }
    }
    stream.write_all(&request).map_err(io_error)?;
    let mut reply = [0u8; 8];
    stream.read_exact(&mut reply).map_err(io_error)?;
    match reply[1] {
        0x5a => Ok(()),
        0x5b => Err(Fail::Other(t!("proxy-refused", reason = "rejected or failed"))),
        0x5c | 0x5d => Err(Fail::Login),
        _ => Err(Fail::Other(t!("proxy-not-socks", kind = "SOCKS4"))),
    }
}

/// At most this many requests on one connection: a Windows login takes
/// three (the empty one, the challenge, the answer).
const ROUNDS: usize = 4;

/// What the proxy said to a `CONNECT`.
#[cfg_attr(not(windows), allow(dead_code))]
struct Answer {
    /// The first line, as it stands.
    status: String,
    code: Option<u16>,
    /// Whether it looks like HTTP at all.
    http: bool,
    /// The logins it offers, in its own order.
    schemes: Vec<String>,
    /// The token of `scheme`, if it sent one.
    token: Option<(String, Vec<u8>)>,
    /// `Content-Length` of the page it sent with a refusal.
    body: usize,
    chunked: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Answer {
    /// Whether `Basic` is on offer (a proxy that names nothing takes it).
    fn takes_basic(&self) -> bool {
        self.schemes.is_empty() || self.schemes.iter().any(|s| s.eq_ignore_ascii_case("basic"))
    }

    /// Its token for `scheme`, if that is the one it answered with.
    fn token_for(&self, scheme: &str) -> Option<&[u8]> {
        let (named, token) = self.token.as_ref()?;
        named.eq_ignore_ascii_case(scheme).then_some(token.as_slice())
    }
}

/// Reads the head of one answer a byte at a time, so nothing after it
/// (the server's first bytes) is taken from ssh, then the page it sent
/// with a refusal, which would otherwise be read as the next answer.
fn answer(stream: &mut (impl Read + Write)) -> Result<Answer, Fail> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16 * 1024 {
            return Err(Fail::Other(t!("proxy-not-http")));
        }
        match stream.read(&mut byte).map_err(io_error)? {
            0 => break,
            _ => head.push(byte[0]),
        }
    }
    let text = String::from_utf8_lossy(&head);
    let status = text.lines().next().unwrap_or_default().trim().to_string();
    let header = |name: &str| -> Vec<String> {
        text.lines()
            .filter_map(|l| l.split_once(':'))
            .filter(|(key, _)| key.trim().eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim().to_string())
            .collect()
    };
    let offers = header("proxy-authenticate");
    let mut answer = Answer {
        code: status.split_whitespace().nth(1).and_then(|c| c.parse().ok()),
        http: status.starts_with("HTTP/"),
        status,
        schemes: offers.iter().filter_map(|o| o.split_whitespace().next().map(str::to_string)).collect(),
        token: offers.iter().find_map(|offer| {
            let (scheme, token) = offer.split_once(' ')?;
            Some((scheme.to_string(), unbase64(token.trim())?))
        }),
        body: header("content-length").first().and_then(|v| v.trim().parse().ok()).unwrap_or(0),
        chunked: header("transfer-encoding").iter().any(|v| v.to_lowercase().contains("chunked")),
    };
    if answer.code != Some(200) && answer.body > 0 && answer.body < 1024 * 1024 {
        let mut page = vec![0u8; answer.body];
        if stream.read_exact(&mut page).is_err() {
            answer.body = 0;
        }
    }
    Ok(answer)
}

/// HTTP `CONNECT`, with a login if the proxy wants one: `Basic` when a
/// user name and password are configured, or a Windows login (NTLM,
/// Negotiate) through SSPI, which needs nothing configured — Windows
/// answers with the credentials the person is signed in with. Both are
/// kept to one connection, as NTLM requires.
// without the Windows login round the loop runs once and returns
#[cfg_attr(not(windows), allow(clippy::never_loop))]
pub(crate) fn http(
    stream: &mut (impl Read + Write),
    proxy: &str,
    host: &str,
    port: u16,
    login: Login,
) -> Result<(), Fail> {
    let target = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    let mut authorization = login.map(|(user, password)| {
        let mut pair = format!("{user}:{password}");
        let header = format!("Basic {}", base64(pair.as_bytes()));
        // SAFETY: zeros are valid UTF-8
        unsafe { pair.as_bytes_mut().fill(0) };
        header
    });
    #[cfg(not(windows))]
    let _ = proxy;
    #[cfg(windows)]
    let mut windows: Option<crate::sspi::Handshake> = None;
    #[cfg(windows)]
    let mut sent_token = false;
    for _ in 0..ROUNDS {
        let mut request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
        if let Some(header) = authorization.take() {
            request.push_str(&format!("Proxy-Authorization: {header}\r\n"));
        }
        request.push_str("\r\n");
        let written = stream.write_all(request.as_bytes());
        // SAFETY: zeros are valid UTF-8
        unsafe { request.as_bytes_mut().fill(0) };
        written.map_err(io_error)?;

        let answer = answer(stream)?;
        match answer.code {
            _ if !answer.http => return Err(Fail::Other(t!("proxy-not-http"))),
            Some(200..=299) => return Ok(()),
            Some(407) => {}
            _ => return Err(Fail::Other(t!("proxy-refused", reason = answer.status))),
        }
        // a login Windows can answer itself, with or without a user name
        #[cfg(windows)]
        let () = {
            if let Some(scheme) = crate::sspi::supported(&answer.schemes) {
                if answer.chunked {
                    return Err(Fail::Other(t!("proxy-schemes", schemes = answer.schemes.join(", "))));
                }
                if windows.is_none() {
                    windows = Some(
                        crate::sspi::Handshake::start(scheme, proxy, login)
                            .map_err(|e| Fail::Other(t!("proxy-windows-login", error = e)))?,
                    );
                }
                let handshake = windows.as_mut().expect("just made");
                let token = answer.token_for(scheme);
                // the login was made, or we sent a token and it came back
                // with nothing to carry on with: it has refused the person
                if handshake.finished() || (sent_token && token.is_none()) {
                    return Err(Fail::Login);
                }
                match handshake.next(token) {
                    // nothing more to send and still refused: it said no
                    Ok(None) => return Err(Fail::Login),
                    Ok(Some(token)) => {
                        authorization = Some(format!("{scheme} {}", base64(&token)));
                        sent_token = true;
                        continue;
                    }
                    Err(e) => return Err(Fail::Other(t!("proxy-windows-login", error = e))),
                }
            }
        };
        return match (answer.takes_basic(), login.is_some()) {
            (false, _) => Err(Fail::Other(t!("proxy-schemes", schemes = answer.schemes.join(", ")))),
            (true, true) => Err(Fail::Login),
            (true, false) => Err(Fail::Other(t!("proxy-needs-login"))),
        };
    }
    Err(Fail::Login)
}

/// base64 back to bytes; `None` if it isn't base64.
fn unbase64(text: &str) -> Option<Vec<u8>> {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let text = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut bits = 0u32;
    let mut have = 0u32;
    for c in text.bytes() {
        let value = ABC.iter().position(|a| *a == c)? as u32;
        bits = (bits << 6) | value;
        have += 6;
        if have >= 8 {
            have -= 8;
            out.push((bits >> have) as u8);
        }
    }
    Some(out)
}

fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// ssh's stdin to the connection, the connection to ssh's stdout.
fn relay(stream: TcpStream) -> i32 {
    let mut up = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[NativeTerm] {}", io_error(e));
            return 1;
        }
    };
    std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut up);
        // ssh is done sending; the server may still answer
        let _ = up.shutdown(Shutdown::Write);
    });
    let mut down = stream;
    let mut out = io::stdout().lock();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match down.read(&mut buf) {
            Ok(0) => return 0,
            Ok(n) => {
                if out.write_all(&buf[..n]).and_then(|()| out.flush()).is_err() {
                    // ssh went away
                    return 0;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A proxy's side of a handshake: what it answers, and what it got.
    struct Fake {
        answer: io::Cursor<Vec<u8>>,
        /// What it answers to each further request, in order.
        more: std::collections::VecDeque<Vec<u8>>,
        got: Vec<u8>,
        /// Requests seen (a request ends with a blank line).
        requests: usize,
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    impl Fake {
        fn new(answer: &[u8]) -> Fake {
            Fake {
                answer: io::Cursor::new(answer.to_vec()),
                more: std::collections::VecDeque::new(),
                got: Vec::new(),
                requests: 0,
            }
        }

        /// A proxy that answers the first request with the first of
        /// `answers`, the second with the second, and so on.
        fn talking(answers: &[&[u8]]) -> Fake {
            let mut fake = Fake::new(b"");
            fake.more = answers.iter().map(|a| a.to_vec()).collect();
            fake
        }

        /// The requests it was sent, as text.
        fn sent(&self) -> Vec<String> {
            String::from_utf8_lossy(&self.got)
                .split("\r\n\r\n")
                .filter(|r| !r.trim().is_empty())
                .map(str::to_string)
                .collect()
        }
    }

    impl Read for Fake {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.answer.read(buf)
        }
    }

    impl Write for Fake {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.got.extend_from_slice(buf);
            if self.got.ends_with(b"\r\n\r\n") {
                self.requests += 1;
                if let Some(next) = self.more.pop_front() {
                    self.answer = io::Cursor::new(next);
                }
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn other(text: String) -> Result<(), Fail> {
        Err(Fail::Other(text))
    }

    #[test]
    fn socks5_by_name() {
        let mut fake = Fake::new(&[5, 0, 5, 0, 0, 1, 10, 0, 0, 1, 0x1f, 0x90, b'S', b'S', b'H']);
        socks5(&mut fake, "db.lan", 22, None).unwrap();
        let mut want = vec![5, 1, 0, 5, 1, 0, 3, 6];
        want.extend(b"db.lan");
        want.extend([0, 22]);
        assert_eq!(fake.got, want);
        // the server's first bytes are left for ssh
        let mut left = Vec::new();
        fake.read_to_end(&mut left).unwrap();
        assert_eq!(left, b"SSH");
    }

    #[test]
    fn socks5_by_address_and_refusals() {
        let mut fake = Fake::new(&[5, 0, 5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        socks5(&mut fake, "10.1.2.3", 2222, None).unwrap();
        assert_eq!(&fake.got[3..], [5, 1, 0, 1, 10, 1, 2, 3, 0x08, 0xae]);
        let mut fake = Fake::new(&[5, 0xff]);
        assert_eq!(socks5(&mut fake, "db", 22, None), other(t!("proxy-needs-login")));
        let mut fake = Fake::new(&[5, 0, 5, 5, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert!(socks5(&mut fake, "db", 22, None).unwrap_err().to_string().contains("connection refused"));
        let mut fake = Fake::new(b"HTTP/1.1 400 Bad Request\r\n\r\n");
        assert_eq!(socks5(&mut fake, "db", 22, None), other(t!("proxy-not-socks", kind = "SOCKS5")));
    }

    #[test]
    fn socks5_with_a_login() {
        let mut fake = Fake::new(&[5, 2, 1, 0, 5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        socks5(&mut fake, "db", 22, Some(("alice", "s3cret"))).unwrap();
        let mut want = vec![5, 2, 0, 2, 1, 5];
        want.extend(b"alice");
        want.push(6);
        want.extend(b"s3cret");
        want.extend([5, 1, 0, 3, 2, b'd', b'b', 0, 22]);
        assert_eq!(fake.got, want);
        // a proxy that doesn't need it
        let mut fake = Fake::new(&[5, 0, 5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        socks5(&mut fake, "db", 22, Some(("alice", "s3cret"))).unwrap();
        assert!(!fake.got.windows(6).any(|w| w == b"s3cret"));
        // refused
        let mut fake = Fake::new(&[5, 2, 1, 1]);
        assert_eq!(socks5(&mut fake, "db", 22, Some(("alice", "wrong"))), Err(Fail::Login));
        // wants one, none saved
        let mut fake = Fake::new(&[5, 2]);
        assert_eq!(socks5(&mut fake, "db", 22, None), other(t!("proxy-needs-login")));
    }

    #[test]
    fn socks4a() {
        let mut fake = Fake::new(&[0, 0x5a, 0, 0, 0, 0, 0, 0]);
        socks4(&mut fake, "db.lan", 22, None).unwrap();
        let mut want = vec![4, 1, 0, 22, 0, 0, 0, 1, 0];
        want.extend(b"db.lan\0");
        assert_eq!(fake.got, want);
        let mut fake = Fake::new(&[0, 0x5a, 0, 0, 0, 0, 0, 0]);
        socks4(&mut fake, "192.168.1.9", 22, Some("alice")).unwrap();
        assert_eq!(fake.got, [&[4, 1, 0, 22, 192, 168, 1, 9][..], b"alice\0"].concat());
        let mut fake = Fake::new(&[0, 0x5b, 0, 0, 0, 0, 0, 0]);
        assert!(socks4(&mut fake, "db", 22, None).is_err());
        let mut fake = Fake::new(&[0, 0x5d, 0, 0, 0, 0, 0, 0]);
        assert_eq!(socks4(&mut fake, "db", 22, Some("bob")), Err(Fail::Login));
        assert!(socks4(&mut Fake::new(&[]), "::1", 22, None).is_err());
    }

    #[test]
    fn http_connect() {
        let mut fake = Fake::new(b"HTTP/1.1 200 Connection established\r\nProxy-Agent: x\r\n\r\nSSH-2.0");
        http(&mut fake, "gw", "db.lan", 22, None).unwrap();
        assert_eq!(fake.got, b"CONNECT db.lan:22 HTTP/1.1\r\nHost: db.lan:22\r\n\r\n");
        let mut left = Vec::new();
        fake.read_to_end(&mut left).unwrap();
        assert_eq!(left, b"SSH-2.0");
        let mut fake = Fake::new(b"HTTP/1.0 200 OK\r\n\r\n");
        http(&mut fake, "gw", "fe80::1", 22, None).unwrap();
        assert!(String::from_utf8_lossy(&fake.got).starts_with("CONNECT [fe80::1]:22 "));
        let mut fake = Fake::new(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n");
        assert_eq!(http(&mut fake, "gw", "db", 22, None), other(t!("proxy-needs-login")));
        let mut fake = Fake::new(b"HTTP/1.1 403 Forbidden\r\n\r\n");
        assert!(http(&mut fake, "gw", "db", 22, None).unwrap_err().to_string().contains("403 Forbidden"));
        let mut fake = Fake::new(&[5, 0]);
        assert_eq!(http(&mut fake, "gw", "db", 22, None), other(t!("proxy-not-http")));
    }

    #[test]
    fn http_with_a_login() {
        let mut fake = Fake::new(b"HTTP/1.1 200 OK\r\n\r\n");
        http(&mut fake, "gw", "db", 22, Some(("Aladdin", "open sesame"))).unwrap();
        let sent = String::from_utf8(fake.got).unwrap();
        assert!(sent.contains("\r\nProxy-Authorization: Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==\r\n\r\n"), "{sent}");
        let refused = b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"x\"\r\n\r\n";
        assert_eq!(http(&mut Fake::new(refused), "gw", "db", 22, Some(("a", "b"))), Err(Fail::Login));
        // a proxy that takes neither Basic nor anything Windows answers
        let digest = b"HTTP/1.1 407 Denied\r\nProxy-Authenticate: Digest realm=\"x\"\r\n\r\n";
        let error = http(&mut Fake::new(digest), "gw", "db", 22, Some(("a", "b"))).unwrap_err().to_string();
        assert_eq!(error, t!("proxy-schemes", schemes = "Digest"));
    }

    /// A `Proxy-Authenticate: NTLM` challenge as a proxy sends it: a
    /// type 2 message with a target name, the challenge and a target
    /// info block (NTLMv2 needs one).
    #[cfg(windows)]
    fn ntlm_challenge() -> Vec<u8> {
        let target: Vec<u8> = "GW".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let info = [0u8; 4]; // MsvAvEOL
        let target_at = 56u32;
        let mut message = Vec::new();
        message.extend(b"NTLMSSP\0");
        message.extend(2u32.to_le_bytes()); // type 2
        message.extend((target.len() as u16).to_le_bytes());
        message.extend((target.len() as u16).to_le_bytes());
        message.extend(target_at.to_le_bytes());
        // unicode | request target | NTLM | always sign | target type
        // server | extended session security | target info
        message.extend(0x0088_8205u32.to_le_bytes());
        message.extend([1, 2, 3, 4, 5, 6, 7, 8]); // the challenge
        message.extend([0u8; 8]); // reserved
        message.extend((info.len() as u16).to_le_bytes());
        message.extend((info.len() as u16).to_le_bytes());
        message.extend((target_at + target.len() as u32).to_le_bytes());
        message.extend([6, 1, 0, 0, 0, 0, 0, 15]); // version
        message.extend(target);
        message.extend(info);
        message
    }

    /// NTLM: three requests on one connection, the tokens made by
    /// Windows. A user name and password are given here because the
    /// machine this runs on may have no credentials of its own to offer
    /// (`sspi::tests`); the path through the proxy is the same either
    /// way.
    #[cfg(windows)]
    #[test]
    fn a_windows_login_is_answered_by_windows() {
        let challenge = format!(
            "HTTP/1.1 407 Denied\r\nProxy-Authenticate: NTLM {}\r\nContent-Length: 5\r\n\r\nhello",
            base64(&ntlm_challenge())
        );
        let mut fake = Fake::talking(&[
            b"HTTP/1.1 407 Denied\r\nProxy-Authenticate: NTLM\r\nProxy-Authenticate: Basic realm=\"x\"\r\nContent-Length: 3\r\n\r\nno!",
            challenge.as_bytes(),
            b"HTTP/1.1 200 Connection established\r\n\r\nSSH-2.0",
        ]);
        http(&mut fake, "gw.corp", "db.lan", 22, Some((r"CORP\alice", "secret"))).expect("the proxy let us through");
        let sent = fake.sent();
        assert_eq!(sent.len(), 3, "{sent:?}");
        assert!(sent[0].contains("Proxy-Authorization: Basic "), "what was configured, first: {}", sent[0]);
        let first = sent[1].lines().find_map(|l| l.strip_prefix("Proxy-Authorization: NTLM ")).expect(&sent[1]);
        assert!(unbase64(first).unwrap().starts_with(b"NTLMSSP\0"), "a real token");
        let second = sent[2].lines().find_map(|l| l.strip_prefix("Proxy-Authorization: NTLM ")).expect(&sent[2]);
        let answer = unbase64(second).unwrap();
        assert!(answer.starts_with(b"NTLMSSP\0"), "a real token");
        assert_eq!(answer[8..12], 3u32.to_le_bytes(), "the answer to the challenge (type 3)");
        // the page the proxy sent with each refusal was read, not left
        // in the stream
        let mut left = Vec::new();
        fake.read_to_end(&mut left).unwrap();
        assert_eq!(left, b"SSH-2.0");
    }

    /// The same, but the proxy says no at the end.
    #[cfg(windows)]
    #[test]
    fn a_windows_login_the_proxy_refuses_is_a_refusal() {
        let challenge =
            format!("HTTP/1.1 407 Denied\r\nProxy-Authenticate: NTLM {}\r\n\r\n", base64(&ntlm_challenge()));
        let mut fake = Fake::talking(&[
            b"HTTP/1.1 407 Denied\r\nProxy-Authenticate: NTLM\r\n\r\n",
            challenge.as_bytes(),
            b"HTTP/1.1 407 Denied\r\nProxy-Authenticate: NTLM\r\n\r\n",
            b"HTTP/1.1 407 Denied\r\nProxy-Authenticate: NTLM\r\n\r\n",
        ]);
        let login = Some((r"CORP\alice", "secret"));
        assert_eq!(http(&mut fake, "gw.corp", "db.lan", 22, login), Err(Fail::Login));
    }

    #[test]
    fn base64_decodes_what_it_encodes() {
        for text in [b"".as_slice(), b"f", b"fo", b"foo", b"foob", "用户:密".as_bytes()] {
            assert_eq!(unbase64(&base64(text)).as_deref(), Some(text), "{text:?}");
        }
        assert_eq!(unbase64("TlRMTVNTUAABAAAA").unwrap(), b"NTLMSSP\0\x01\0\0\0");
        assert_eq!(unbase64("not base64!"), None);
    }

    #[test]
    fn base64_encodes() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64("用户:密".as_bytes()), "55So5oi3OuWvhg==");
    }
}
