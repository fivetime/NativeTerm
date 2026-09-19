//! `nativeterm-shim --proxy <url> <host> <port>`: ssh's `ProxyCommand`
//! helper (see `native_term_config::proxy`). It connects to the proxy,
//! asks it for a connection to `host:port` (SOCKS5, SOCKS4a or HTTP
//! `CONNECT`; the proxy resolves the name), then passes bytes between
//! ssh (stdin / stdout) and that connection until either side closes.
//! Messages go to stderr, which ssh shows (or a background run reports).
//!
//! No crate: the `socks` crate's last release is from 2022 and brings the
//! old `winapi`; the three handshakes are a few dozen lines each.

use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, TcpStream, ToSocketAddrs};
use std::time::Duration;

use native_term_config::proxy::{Kind, Proxy};

use crate::t;

/// How long connecting to the proxy may take.
const TIMEOUT: Duration = Duration::from_secs(20);
/// How long the proxy may take to answer (one of another type waits for
/// more of its own protocol and never does).
const ANSWER: Duration = Duration::from_secs(10);

pub fn run(url: &str, host: &str, port: &str) -> i32 {
    let result = Proxy::parse(url).and_then(|proxy| {
        let port = port.parse::<u16>().map_err(|_| format!("invalid port {port:?}"))?;
        let stream = connect(&proxy, host, port)?;
        Ok((proxy, stream))
    });
    match result {
        Ok((_, stream)) => relay(stream),
        Err(e) => {
            eprintln!("[NativeTerm] {}", t!("proxy-failed", url = url, host = host, port = port, error = e));
            1
        }
    }
}

/// A connection to `host:port` through the proxy.
fn connect(proxy: &Proxy, host: &str, port: u16) -> Result<TcpStream, String> {
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
    match proxy.kind {
        Kind::Socks5 => socks5(&mut stream, host, port)?,
        Kind::Socks4 => socks4(&mut stream, host, port)?,
        Kind::Http => http(&mut stream, host, port)?,
    }
    let _ = stream.set_read_timeout(None);
    Ok(stream)
}

fn io_error(e: io::Error) -> String {
    match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => t!("proxy-no-answer"),
        _ => t!("proxy-broken", error = e.to_string()),
    }
}

/// SOCKS5 (RFC 1928), no authentication.
pub(crate) fn socks5(stream: &mut (impl Read + Write), host: &str, port: u16) -> Result<(), String> {
    stream.write_all(&[5, 1, 0]).map_err(io_error)?;
    let mut choice = [0u8; 2];
    stream.read_exact(&mut choice).map_err(io_error)?;
    match choice {
        [5, 0] => {}
        [5, 0xff] | [5, 2] => return Err(t!("proxy-needs-login")),
        _ => return Err(t!("proxy-not-socks", kind = "SOCKS5")),
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
        return Err(t!("proxy-not-socks", kind = "SOCKS5"));
    }
    if head[1] != 0 {
        return Err(t!("proxy-refused", reason = socks5_reason(head[1])));
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
        _ => return Err(t!("proxy-not-socks", kind = "SOCKS5")),
    };
    let mut skip = vec![0u8; rest];
    stream.read_exact(&mut skip).map_err(io_error)
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

/// SOCKS4a: the proxy resolves the name (an IPv4 address is sent as is).
pub(crate) fn socks4(stream: &mut (impl Read + Write), host: &str, port: u16) -> Result<(), String> {
    let mut request = vec![4, 1];
    request.extend(port.to_be_bytes());
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            request.extend(ip.octets());
            request.push(0);
        }
        Ok(IpAddr::V6(_)) => return Err(t!("proxy-socks4-v6")),
        Err(_) => {
            request.extend([0, 0, 0, 1, 0]);
            request.extend(host.as_bytes());
            request.push(0);
        }
    }
    stream.write_all(&request).map_err(io_error)?;
    let mut reply = [0u8; 8];
    stream.read_exact(&mut reply).map_err(io_error)?;
    match reply[1] {
        0x5a => Ok(()),
        0x5b => Err(t!("proxy-refused", reason = "rejected or failed")),
        0x5c | 0x5d => Err(t!("proxy-needs-login")),
        _ => Err(t!("proxy-not-socks", kind = "SOCKS4")),
    }
}

/// HTTP `CONNECT`. The answer is read a byte at a time so nothing after
/// its headers (the server's first bytes) is taken from ssh.
pub(crate) fn http(stream: &mut (impl Read + Write), host: &str, port: u16) -> Result<(), String> {
    let target = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    let request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n");
    stream.write_all(request.as_bytes()).map_err(io_error)?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16 * 1024 {
            return Err(t!("proxy-not-http"));
        }
        match stream.read(&mut byte).map_err(io_error)? {
            0 => break,
            _ => head.push(byte[0]),
        }
    }
    let text = String::from_utf8_lossy(&head);
    let status = text.lines().next().unwrap_or_default().trim().to_string();
    let code = status.split_whitespace().nth(1).and_then(|c| c.parse::<u16>().ok());
    match code {
        _ if !status.starts_with("HTTP/") => Err(t!("proxy-not-http")),
        Some(200..=299) => Ok(()),
        Some(407) => Err(t!("proxy-needs-login")),
        _ => Err(t!("proxy-refused", reason = status)),
    }
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
        got: Vec<u8>,
    }

    impl Fake {
        fn new(answer: &[u8]) -> Fake {
            Fake { answer: io::Cursor::new(answer.to_vec()), got: Vec::new() }
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
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn socks5_by_name() {
        let mut fake = Fake::new(&[5, 0, 5, 0, 0, 1, 10, 0, 0, 1, 0x1f, 0x90, b'S', b'S', b'H']);
        socks5(&mut fake, "db.lan", 22).unwrap();
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
        socks5(&mut fake, "10.1.2.3", 2222).unwrap();
        assert_eq!(&fake.got[3..], [5, 1, 0, 1, 10, 1, 2, 3, 0x08, 0xae]);
        let mut fake = Fake::new(&[5, 0xff]);
        assert_eq!(socks5(&mut fake, "db", 22).unwrap_err(), t!("proxy-needs-login"));
        let mut fake = Fake::new(&[5, 0, 5, 5, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert!(socks5(&mut fake, "db", 22).unwrap_err().contains("connection refused"));
        let mut fake = Fake::new(b"HTTP/1.1 400 Bad Request\r\n\r\n");
        assert_eq!(socks5(&mut fake, "db", 22).unwrap_err(), t!("proxy-not-socks", kind = "SOCKS5"));
    }

    #[test]
    fn socks4a() {
        let mut fake = Fake::new(&[0, 0x5a, 0, 0, 0, 0, 0, 0]);
        socks4(&mut fake, "db.lan", 22).unwrap();
        let mut want = vec![4, 1, 0, 22, 0, 0, 0, 1, 0];
        want.extend(b"db.lan\0");
        assert_eq!(fake.got, want);
        let mut fake = Fake::new(&[0, 0x5a, 0, 0, 0, 0, 0, 0]);
        socks4(&mut fake, "192.168.1.9", 22).unwrap();
        assert_eq!(fake.got, [4, 1, 0, 22, 192, 168, 1, 9, 0]);
        let mut fake = Fake::new(&[0, 0x5b, 0, 0, 0, 0, 0, 0]);
        assert!(socks4(&mut fake, "db", 22).is_err());
        assert!(socks4(&mut Fake::new(&[]), "::1", 22).is_err());
    }

    #[test]
    fn http_connect() {
        let mut fake = Fake::new(b"HTTP/1.1 200 Connection established\r\nProxy-Agent: x\r\n\r\nSSH-2.0");
        http(&mut fake, "db.lan", 22).unwrap();
        assert_eq!(fake.got, b"CONNECT db.lan:22 HTTP/1.1\r\nHost: db.lan:22\r\n\r\n");
        let mut left = Vec::new();
        fake.read_to_end(&mut left).unwrap();
        assert_eq!(left, b"SSH-2.0");
        let mut fake = Fake::new(b"HTTP/1.0 200 OK\r\n\r\n");
        http(&mut fake, "fe80::1", 22).unwrap();
        assert!(String::from_utf8_lossy(&fake.got).starts_with("CONNECT [fe80::1]:22 "));
        let mut fake = Fake::new(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n");
        assert_eq!(http(&mut fake, "db", 22).unwrap_err(), t!("proxy-needs-login"));
        let mut fake = Fake::new(b"HTTP/1.1 403 Forbidden\r\n\r\n");
        assert!(http(&mut fake, "db", 22).unwrap_err().contains("403 Forbidden"));
        let mut fake = Fake::new(&[5, 0]);
        assert_eq!(http(&mut fake, "db", 22).unwrap_err(), t!("proxy-not-http"));
    }
}
