//! SFTP version 3 on the wire (draft-ietf-secsh-filexfer-02, the version
//! OpenSSH speaks): packets are a length, a type, and for requests an id;
//! strings are a length and bytes. File names are bytes (UTF-8 in practice).

use std::io::{self, Read};

pub const INIT: u8 = 1;
pub const VERSION: u8 = 2;
pub const OPEN: u8 = 3;
pub const CLOSE: u8 = 4;
pub const READ: u8 = 5;
pub const WRITE: u8 = 6;
pub const LSTAT: u8 = 7;
pub const SETSTAT: u8 = 9;
pub const OPENDIR: u8 = 11;
pub const READDIR: u8 = 12;
pub const REMOVE: u8 = 13;
pub const MKDIR: u8 = 14;
pub const RMDIR: u8 = 15;
pub const REALPATH: u8 = 16;
pub const STAT: u8 = 17;
pub const RENAME: u8 = 18;
pub const READLINK: u8 = 19;
pub const EXTENDED: u8 = 200;

pub const STATUS: u8 = 101;
pub const HANDLE: u8 = 102;
pub const DATA: u8 = 103;
pub const NAME: u8 = 104;
pub const ATTRS: u8 = 105;
pub const EXTENDED_REPLY: u8 = 201;

pub const FX_OK: u32 = 0;
pub const FX_EOF: u32 = 1;
pub const FX_NO_SUCH_FILE: u32 = 2;
pub const FX_PERMISSION_DENIED: u32 = 3;

pub const OPEN_READ: u32 = 0x01;
pub const OPEN_WRITE: u32 = 0x02;
pub const OPEN_CREAT: u32 = 0x08;
pub const OPEN_TRUNC: u32 = 0x10;
pub const OPEN_EXCL: u32 = 0x20;

const ATTR_SIZE: u32 = 0x01;
const ATTR_UIDGID: u32 = 0x02;
const ATTR_PERMISSIONS: u32 = 0x04;
const ATTR_ACMODTIME: u32 = 0x08;
const ATTR_EXTENDED: u32 = 0x8000_0000;

/// A packet larger than this is taken for a broken stream (OpenSSH's
/// limit is 256 KB).
const MAX_PACKET: u32 = 1024 * 1024;

/// A file's attributes; what the server didn't send is `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attrs {
    pub size: Option<u64>,
    pub uid_gid: Option<(u32, u32)>,
    pub permissions: Option<u32>,
    /// Access and modification time, seconds since the Unix epoch.
    pub atime_mtime: Option<(u32, u32)>,
}

impl Attrs {
    const TYPE_MASK: u32 = 0o170000;

    pub fn is_dir(&self) -> bool {
        self.permissions.is_some_and(|p| p & Self::TYPE_MASK == 0o040000)
    }

    pub fn is_symlink(&self) -> bool {
        self.permissions.is_some_and(|p| p & Self::TYPE_MASK == 0o120000)
    }

    pub fn mtime(&self) -> Option<u32> {
        self.atime_mtime.map(|(_, m)| m)
    }
}

/// A request body under construction.
#[derive(Default)]
pub struct Body(pub Vec<u8>);

impl Body {
    pub fn u32(mut self, v: u32) -> Body {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn u64(mut self, v: u64) -> Body {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn string(mut self, s: &[u8]) -> Body {
        self = self.u32(s.len() as u32);
        self.0.extend_from_slice(s);
        self
    }

    pub fn attrs(mut self, a: &Attrs) -> Body {
        let mut flags = 0;
        if a.size.is_some() {
            flags |= ATTR_SIZE;
        }
        if a.uid_gid.is_some() {
            flags |= ATTR_UIDGID;
        }
        if a.permissions.is_some() {
            flags |= ATTR_PERMISSIONS;
        }
        if a.atime_mtime.is_some() {
            flags |= ATTR_ACMODTIME;
        }
        self = self.u32(flags);
        if let Some(size) = a.size {
            self = self.u64(size);
        }
        if let Some((uid, gid)) = a.uid_gid {
            self = self.u32(uid).u32(gid);
        }
        if let Some(p) = a.permissions {
            self = self.u32(p);
        }
        if let Some((at, mt)) = a.atime_mtime {
            self = self.u32(at).u32(mt);
        }
        self
    }
}

/// A whole packet: length, type, then `body` (which starts with the id
/// for requests).
pub fn packet(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 5);
    out.extend_from_slice(&(body.len() as u32 + 1).to_be_bytes());
    out.push(kind);
    out.extend_from_slice(body);
    out
}

/// Reads one packet: its type and the rest.
pub fn read_packet(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len);
    if len == 0 || len > MAX_PACKET {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("an SFTP packet of {len} bytes")));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    let kind = buf.remove(0);
    Ok((kind, buf))
}

/// Takes fields off a packet's body.
pub struct Fields<'a> {
    buf: &'a [u8],
}

fn short() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "a short SFTP packet")
}

impl<'a> Fields<'a> {
    pub fn new(buf: &'a [u8]) -> Fields<'a> {
        Fields { buf }
    }

    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.buf.len() < n {
            return Err(short());
        }
        let (head, rest) = self.buf.split_at(n);
        self.buf = rest;
        Ok(head)
    }

    pub fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().map_err(|_| short())?))
    }

    pub fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().map_err(|_| short())?))
    }

    pub fn string(&mut self) -> io::Result<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }

    pub fn attrs(&mut self) -> io::Result<Attrs> {
        let flags = self.u32()?;
        let mut a = Attrs::default();
        if flags & ATTR_SIZE != 0 {
            a.size = Some(self.u64()?);
        }
        if flags & ATTR_UIDGID != 0 {
            a.uid_gid = Some((self.u32()?, self.u32()?));
        }
        if flags & ATTR_PERMISSIONS != 0 {
            a.permissions = Some(self.u32()?);
        }
        if flags & ATTR_ACMODTIME != 0 {
            a.atime_mtime = Some((self.u32()?, self.u32()?));
        }
        if flags & ATTR_EXTENDED != 0 {
            for _ in 0..self.u32()? {
                self.string()?;
                self.string()?;
            }
        }
        Ok(a)
    }

    pub fn rest(&self) -> &'a [u8] {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes_round_trip() {
        let a = Attrs {
            size: Some(5_000_000_000),
            uid_gid: Some((0, 10)),
            permissions: Some(0o100644),
            atime_mtime: Some((1, 2)),
        };
        let body = Body::default().attrs(&a).string("名字".as_bytes());
        let mut f = Fields::new(&body.0);
        assert_eq!(f.attrs().unwrap(), a);
        assert_eq!(f.string().unwrap(), "名字".as_bytes());
        assert!(f.rest().is_empty());
        assert!(Fields::new(&[0, 0, 0, 9, 1]).string().is_err(), "short");

        let p = packet(READ, &[1, 2, 3]);
        let (kind, body) = read_packet(&mut &p[..]).unwrap();
        assert_eq!((kind, body), (READ, vec![1, 2, 3]));
        assert!(Attrs { permissions: Some(0o040755), ..Default::default() }.is_dir());
    }
}
