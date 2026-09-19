//! Keys under `HKEY_CURRENT_USER`: reading subkeys and values (PuTTY's
//! saved sessions), and writing and deleting keys for tests.

use std::io;

use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::Globalization::{MultiByteToWideChar, CP_ACP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE,
    REG_SZ, REG_VALUE_TYPE,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegValue {
    Str(String),
    Dword(u32),
    Other,
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn check(status: WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status.0 as i32))
    }
}

fn open(subkey: &str) -> io::Result<Option<Key>> {
    open_in(HKEY_CURRENT_USER, subkey)
}

fn open_in(root: HKEY, subkey: &str) -> io::Result<Option<Key>> {
    let mut key = HKEY::default();
    let status = unsafe { RegOpenKeyExW(root, &HSTRING::from(subkey), None, KEY_READ, &mut key) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(status)?;
    Ok(Some(Key(key)))
}

/// Names of a key's subkeys; empty if the key doesn't exist.
pub fn user_subkeys(subkey: &str) -> io::Result<Vec<String>> {
    let Some(key) = open(subkey)? else { return Ok(Vec::new()) };
    let mut names = Vec::new();
    for index in 0.. {
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        let status =
            unsafe { RegEnumKeyExW(key.0, index, Some(PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        check(status)?;
        names.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    Ok(names)
}

/// A key's values; empty if the key doesn't exist.
pub fn user_values(subkey: &str) -> io::Result<Vec<(String, RegValue)>> {
    let Some(key) = open(subkey)? else { return Ok(Vec::new()) };
    values(&key)
}

/// The serial ports present now (`COM3`, …), sorted by number.
pub fn serial_ports() -> Vec<String> {
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    let key = match open_in(HKEY_LOCAL_MACHINE, r"HARDWARE\DEVICEMAP\SERIALCOMM") {
        Ok(Some(key)) => key,
        _ => return Vec::new(),
    };
    let mut ports: Vec<String> = values(&key)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, v)| match v {
            RegValue::Str(port) => Some(port),
            _ => None,
        })
        .collect();
    let number = |p: &String| p.trim_start_matches(|c: char| !c.is_ascii_digit()).parse::<u32>().unwrap_or(u32::MAX);
    ports.sort_by_key(|p| (number(p), p.clone()));
    ports.dedup();
    ports
}

fn values(key: &Key) -> io::Result<Vec<(String, RegValue)>> {
    let mut out = Vec::new();
    for index in 0.. {
        let mut name = vec![0u16; 16384];
        let mut name_len = name.len() as u32;
        let mut kind = 0u32;
        let mut size = 0u32;
        let status = unsafe {
            RegEnumValueW(
                key.0,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                None,
                Some(&mut size),
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        check(status)?;
        name.truncate(name_len as usize);
        let mut data = vec![0u8; size as usize];
        let status = unsafe {
            RegQueryValueExW(key.0, &HSTRING::from_wide(&name), None, None, Some(data.as_mut_ptr()), Some(&mut size))
        };
        check(status)?;
        data.truncate(size as usize);
        let value = match REG_VALUE_TYPE(kind) {
            REG_SZ | REG_EXPAND_SZ => {
                let wide: Vec<u16> = data.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
                let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
                RegValue::Str(String::from_utf16_lossy(&wide[..end]))
            }
            REG_DWORD if data.len() >= 4 => RegValue::Dword(u32::from_le_bytes([data[0], data[1], data[2], data[3]])),
            _ => RegValue::Other,
        };
        out.push((String::from_utf16_lossy(&name), value));
    }
    Ok(out)
}

/// Create a key (and its parents) and set values in it. For tests.
pub fn write_user_values(subkey: &str, values: &[(&str, RegValue)]) -> io::Result<()> {
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(subkey),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
    };
    check(status)?;
    let key = Key(key);
    for (name, value) in values {
        let (kind, bytes): (REG_VALUE_TYPE, Vec<u8>) = match value {
            RegValue::Str(s) => {
                (REG_SZ, s.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect())
            }
            RegValue::Dword(n) => (REG_DWORD, n.to_le_bytes().to_vec()),
            RegValue::Other => continue,
        };
        check(unsafe { RegSetValueExW(key.0, &HSTRING::from(*name), None, kind, Some(&bytes)) })?;
    }
    Ok(())
}

/// Delete a key and everything under it; fine if it doesn't exist. For tests.
pub fn delete_user_tree(subkey: &str) -> io::Result<()> {
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(subkey)) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check(status)
}

/// Bytes in the system's ANSI code page (e.g. GBK) as text.
pub fn from_ansi(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    let len = unsafe { MultiByteToWideChar(CP_ACP, flags, bytes, None) };
    let mut wide = vec![0u16; len.max(0) as usize];
    let len = unsafe { MultiByteToWideChar(CP_ACP, flags, bytes, Some(&mut wide)) };
    String::from_utf16_lossy(&wide[..len.max(0) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_delete() {
        let root = format!(r"Software\NativeTerm-Tests-registry-{}", std::process::id());
        let sub = format!(r"{root}\one%20two");
        write_user_values(
            &sub,
            &[("HostName", RegValue::Str("节点.example".into())), ("PortNumber", RegValue::Dword(2222))],
        )
        .unwrap();
        write_user_values(&format!(r"{root}\other"), &[]).unwrap();
        let mut keys = user_subkeys(&root).unwrap();
        keys.sort();
        assert_eq!(keys, ["one%20two", "other"]);
        let values = user_values(&sub).unwrap();
        assert!(values.contains(&("HostName".into(), RegValue::Str("节点.example".into()))), "{values:?}");
        assert!(values.contains(&("PortNumber".into(), RegValue::Dword(2222))));
        delete_user_tree(&root).unwrap();
        assert!(user_subkeys(&root).unwrap().is_empty());
        assert!(user_values(&sub).unwrap().is_empty());
        delete_user_tree(&root).unwrap();
    }

    #[test]
    fn ansi_text() {
        assert_eq!(from_ansi(b"abc"), "abc");
        assert_eq!(from_ansi(b""), "");
    }
}
