//! What a server is, as far as it said so itself.
//!
//! The first thing an SSH server sends is its identification string
//! (RFC 4253, 4.2): `SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.19`: the
//! protocol, the software, and after a space whatever who built it wanted
//! said. NativeTerm's ssh hands it on when the login is done
//! (`NATIVETERM_SERVER_VERSION`, see the shim's `--authenticated`), and
//! it is kept per host (`state.db`, `servers`). Nothing is asked of the
//! server and nothing is run on it for this: a program run there would
//! be in its logs, and would look like something done to it.
//!
//! What it says of the operating system is what the package's builder
//! put there, so it is the family a system is of, not always the system:
//!
//! - Debian's package says the vendor it was built for and the package's
//!   revision (`debian/rules`: `SSH_EXTRAVERSION :=
//!   $(DEB_VENDOR)-<revision>`): `Debian-2+deb12u10`,
//!   `Ubuntu-3ubuntu13.19`, and so `Raspbian-…`, `Kali-…`. A system that
//!   takes the package as it is says what the package says: Zorin OS and
//!   elementary OS say Ubuntu, Lingmo says Debian (measured 2026-09-28).
//! - Deepin's says `Deepin` (measured, Deepin 25).
//! - FreeBSD's says `FreeBSD-20260709` (`SSH_VERSION_FREEBSD` in its
//!   `crypto/openssh/version.h`).
//! - Windows' is `OpenSSH_for_Windows_9.5`: the software's own name.
//! - Fedora, Arch (EndeavourOS) and macOS say the software only
//!   (`OpenSSH_10.2`; measured): they are not known, and have the
//!   picture any host has.

use std::collections::HashMap;

/// The systems a server is known to be of, and has a picture for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Os {
    Ubuntu,
    Debian,
    Deepin,
    Raspbian,
    Kali,
    FreeBsd,
    Windows,
}

/// As long as an identification string may be (RFC 4253: 255 with its
/// line's end).
pub const LONGEST: usize = 253;

/// An identification string as it is kept: what can be printed of it;
/// `None` for what is none.
#[must_use]
pub fn cleaned(said: &str) -> Option<String> {
    let said: String = said.trim().chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(LONGEST).collect();
    software(&said).is_some().then_some(said)
}

/// The software a server is, and what is said after it.
#[must_use]
pub fn software(said: &str) -> Option<(&str, &str)> {
    let (_protocol, rest) = said.strip_prefix("SSH-")?.split_once('-')?;
    let (software, comments) = rest.split_once(' ').unwrap_or((rest, ""));
    (!software.is_empty()).then_some((software, comments.trim()))
}

impl Os {
    pub const ALL: [Os; 7] = [Os::Ubuntu, Os::Debian, Os::Deepin, Os::Raspbian, Os::Kali, Os::FreeBsd, Os::Windows];

    /// The system a server that says `said` is of, where it says.
    #[must_use]
    pub fn of(said: &str) -> Option<Os> {
        let (software, comments) = software(said)?;
        if software.starts_with("OpenSSH_for_Windows") {
            return Some(Os::Windows);
        }
        // (the vendor: the letters what is said begins with)
        let vendor: String = comments.chars().take_while(char::is_ascii_alphabetic).collect();
        Os::ALL.into_iter().find(|os| os.vendor().eq_ignore_ascii_case(&vendor))
    }

    fn vendor(self) -> &'static str {
        match self {
            Os::Windows => "Windows",
            Os::FreeBsd => "FreeBSD",
            _ => self.name(),
        }
    }

    /// Its name, as it calls itself.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Os::Ubuntu => "Ubuntu",
            Os::Debian => "Debian",
            Os::Deepin => "Deepin",
            Os::Raspbian => "Raspbian",
            Os::Kali => "Kali",
            Os::FreeBsd => "FreeBSD",
            Os::Windows => "Windows",
        }
    }
}

/// What is known of a server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Known {
    /// What it said it is.
    pub said: String,
    pub os: Option<Os>,
}

impl Known {
    #[must_use]
    pub fn of(said: String) -> Known {
        Known { os: Os::of(&said), said }
    }

    /// The software and what it said after it, without the protocol.
    #[must_use]
    pub fn software(&self) -> String {
        match software(&self.said) {
            Some((software, "")) => software.to_string(),
            Some((software, comments)) => format!("{software} {comments}"),
            None => self.said.clone(),
        }
    }
}

/// The servers known, by their hosts' aliases.
pub type Servers = HashMap<String, Known>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_is_of_the_system_it_says() {
        // as the test machines said, 2026-09-28
        let measured = [
            ("SSH-2.0-OpenSSH_10.2p1 Ubuntu-2ubuntu3.6", Some(Os::Ubuntu)),
            ("SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.19", Some(Os::Ubuntu)),
            ("SSH-2.0-OpenSSH_9.2p1 Debian-2+deb12u10", Some(Os::Debian)),
            ("SSH-2.0-OpenSSH_10.0p2 Deepin", Some(Os::Deepin)),
            ("SSH-2.0-OpenSSH_10.2", None),
            ("SSH-2.0-OpenSSH_10.5", None),
            ("SSH-2.0-OpenSSH_9.9", None),
        ];
        for (said, os) in measured {
            assert_eq!(Os::of(said), os, "{said}");
        }
        // as their sources say
        assert_eq!(Os::of("SSH-2.0-OpenSSH_10.4 FreeBSD-20260709"), Some(Os::FreeBsd));
        assert_eq!(Os::of("SSH-2.0-OpenSSH_for_Windows_9.5"), Some(Os::Windows));
        assert_eq!(Os::of("SSH-2.0-OpenSSH_for_Windows_10.2 Win32-OpenSSH-GitHub"), Some(Os::Windows));
        assert_eq!(Os::of("SSH-2.0-OpenSSH_9.2p1 Raspbian-2+deb12u3"), Some(Os::Raspbian));
        assert_eq!(Os::of("SSH-2.0-OpenSSH_9.9p1 Kali-3"), Some(Os::Kali));
        // what is no system's name is none
        for said in
            ["SSH-2.0-dropbear_2022.83", "SSH-2.0-OpenSSH_9.6 Ubuntuish", "SSH-1.99-Cisco-1.25", "", "HTTP/1.1 400"]
        {
            assert_eq!(Os::of(said), None, "{said}");
        }
    }

    #[test]
    fn what_a_server_said_is_kept_as_it_can_be_shown() {
        assert_eq!(
            cleaned(" SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.19\r\n").unwrap(),
            "SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.19"
        );
        assert_eq!(
            cleaned("SSH-2.0-Open\u{1b}[31mSSH\u{7}_9").unwrap(),
            "SSH-2.0-Open[31mSSH_9",
            "nothing the terminal would act on"
        );
        assert_eq!(cleaned(&format!("SSH-2.0-{}", "x".repeat(400))).unwrap().len(), LONGEST);
        assert_eq!(cleaned("hello"), None);
        assert_eq!(cleaned("SSH-2.0-"), None);
        let known = Known::of("SSH-2.0-OpenSSH_10.2p1 Ubuntu-2ubuntu3.6".into());
        assert_eq!((known.os, known.software().as_str()), (Some(Os::Ubuntu), "OpenSSH_10.2p1 Ubuntu-2ubuntu3.6"));
        assert_eq!(Known::of("SSH-2.0-OpenSSH_10.2".into()).software(), "OpenSSH_10.2");
    }
}
