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

/// The systems there are pictures of: those a server says it is of, and
/// those the person says a host runs (`NativeTermSystem`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Os {
    AlmaLinux,
    Alpine,
    Arch,
    CentOs,
    Debian,
    Deepin,
    Elementary,
    EndeavourOs,
    Fedora,
    FreeBsd,
    Kali,
    Mint,
    MacOs,
    Manjaro,
    OpenBsd,
    Raspbian,
    Rhel,
    Rocky,
    Suse,
    Ubuntu,
    Windows,
    Zorin,
    /// A Linux that is none of them.
    Linux,
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
    /// All of them, as they are chosen among: by their names, a Linux
    /// that is none of the others last.
    pub const ALL: [Os; 23] = [
        Os::AlmaLinux,
        Os::Alpine,
        Os::Arch,
        Os::CentOs,
        Os::Debian,
        Os::Deepin,
        Os::Elementary,
        Os::EndeavourOs,
        Os::Fedora,
        Os::FreeBsd,
        Os::Kali,
        Os::Mint,
        Os::MacOs,
        Os::Manjaro,
        Os::OpenBsd,
        Os::Raspbian,
        Os::Rhel,
        Os::Rocky,
        Os::Suse,
        Os::Ubuntu,
        Os::Windows,
        Os::Zorin,
        Os::Linux,
    ];

    /// The system a server that says `said` is of, where it says.
    #[must_use]
    pub fn of(said: &str) -> Option<Os> {
        let (software, comments) = software(said)?;
        if software.starts_with("OpenSSH_for_Windows") {
            return Some(Os::Windows);
        }
        // (the vendor: the letters what is said begins with)
        let vendor: String = comments.chars().take_while(char::is_ascii_alphabetic).collect();
        Os::ALL.into_iter().find(|os| os.vendor().is_some_and(|name| name.eq_ignore_ascii_case(&vendor)))
    }

    /// What a server of it says after its software, where its servers
    /// say something (see above for where each is known from).
    fn vendor(self) -> Option<&'static str> {
        match self {
            Os::Ubuntu => Some("Ubuntu"),
            Os::Debian => Some("Debian"),
            Os::Deepin => Some("Deepin"),
            Os::Raspbian => Some("Raspbian"),
            Os::Kali => Some("Kali"),
            Os::FreeBsd => Some("FreeBSD"),
            _ => None,
        }
    }

    /// The system the person says a host runs (`NativeTermSystem`),
    /// where it is one of these.
    #[must_use]
    pub fn named(id: &str) -> Option<Os> {
        Os::ALL.into_iter().find(|os| os.id().eq_ignore_ascii_case(id.trim()))
    }

    /// What it is written as (`NativeTermSystem`): as it calls itself in
    /// its `/etc/os-release` (`ID=`), where it has one.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Os::AlmaLinux => "almalinux",
            Os::Alpine => "alpine",
            Os::Arch => "arch",
            Os::CentOs => "centos",
            Os::Debian => "debian",
            Os::Deepin => "deepin",
            Os::Elementary => "elementary",
            Os::EndeavourOs => "endeavouros",
            Os::Fedora => "fedora",
            Os::FreeBsd => "freebsd",
            Os::Kali => "kali",
            Os::Mint => "linuxmint",
            Os::MacOs => "macos",
            Os::Manjaro => "manjaro",
            Os::OpenBsd => "openbsd",
            Os::Raspbian => "raspbian",
            Os::Rhel => "rhel",
            Os::Rocky => "rocky",
            Os::Suse => "suse",
            Os::Ubuntu => "ubuntu",
            Os::Windows => "windows",
            Os::Zorin => "zorin",
            Os::Linux => "linux",
        }
    }

    /// Its name, as it calls itself.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Os::AlmaLinux => "AlmaLinux",
            Os::Alpine => "Alpine Linux",
            Os::Arch => "Arch Linux",
            Os::CentOs => "CentOS",
            Os::Debian => "Debian",
            Os::Deepin => "Deepin",
            Os::Elementary => "elementary OS",
            Os::EndeavourOs => "EndeavourOS",
            Os::Fedora => "Fedora",
            Os::FreeBsd => "FreeBSD",
            Os::Kali => "Kali Linux",
            Os::Mint => "Linux Mint",
            Os::MacOs => "macOS",
            Os::Manjaro => "Manjaro",
            Os::OpenBsd => "OpenBSD",
            Os::Raspbian => "Raspberry Pi OS",
            Os::Rhel => "Red Hat Enterprise Linux",
            Os::Rocky => "Rocky Linux",
            Os::Suse => "SUSE / openSUSE",
            Os::Ubuntu => "Ubuntu",
            Os::Windows => "Windows",
            Os::Zorin => "Zorin OS",
            Os::Linux => "Linux",
        }
    }
}

/// How a host's system is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum By {
    /// The person said it.
    Chosen,
    /// Its server said it.
    Said,
}

/// The system a host runs: what the person says (`chosen`), before what
/// its server said.
#[must_use]
pub fn system(chosen: Option<&str>, known: Option<&Known>) -> Option<(Os, By)> {
    let chosen = chosen.and_then(Os::named).map(|os| (os, By::Chosen));
    chosen.or_else(|| Some((known?.os?, By::Said)))
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
        // what the person says is taken before it, where it is a system there is
        let known = Known::of("SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.19".into());
        let silent = Known::of("SSH-2.0-OpenSSH_10.2".into());
        assert_eq!(system(None, Some(&known)), Some((Os::Ubuntu, By::Said)));
        assert_eq!(system(Some("zorin"), Some(&known)), Some((Os::Zorin, By::Chosen)));
        assert_eq!(system(Some(" Fedora "), Some(&silent)), Some((Os::Fedora, By::Chosen)));
        assert_eq!(system(Some("macos"), None), Some((Os::MacOs, By::Chosen)));
        assert_eq!(system(Some("plan9"), Some(&known)), Some((Os::Ubuntu, By::Said)), "one there is none of");
        assert_eq!(system(None, Some(&silent)), None);
        assert_eq!(system(None, None), None);
        for os in Os::ALL {
            assert_eq!(Os::named(os.id()), Some(os));
            assert!(native_term_config::system::valid(os.id()), "{os:?}");
        }
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
