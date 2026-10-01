//! SecureCRT's "Password V2" encryption, read (decryption only): what its
//! saved credentials and sessions keep as `S:"Password V2"=02:<hex>` or
//! `=03:<hex>`, so that an import can put the password where NativeTerm
//! keeps passwords (the system's password store) instead of asking for
//! it again. The algorithm as public decoders read it (HyperSine's
//! `securecrt_cipher.py`, Metasploit's `securecrt.rb`), checked against a
//! real SecureCRT 9 credential:
//!
//! - `02:`: AES-256-CBC, the key SHA-256 of the configuration passphrase,
//!   a zero IV.
//! - `03:`: the first 16 bytes are a salt; key and IV (32 + 16 bytes) are
//!   OpenSSH's bcrypt_pbkdf of the passphrase and salt, 16 rounds.
//! - Either way the plaintext is a 32-bit little-endian length, the UTF-8
//!   text, its SHA-256 and padding: a wrong passphrase or a damaged value
//!   fails the check instead of giving a wrong password.
//!
//! The configuration passphrase is empty unless the person set one in
//! SecureCRT; OpenSSH's bcrypt_pbkdf refuses an empty passphrase (so does
//! the `bcrypt-pbkdf` crate), SecureCRT's does not: hence the few lines of
//! it here, on the `blowfish` crate's bcrypt primitives, as that crate
//! writes them.

use aes::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
use blowfish::Blowfish;
use sha2::{Digest, Sha256, Sha512};

/// Why a value could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CryptError {
    /// Not `02:` or `03:` followed by hex.
    Format,
    /// The length or checksum inside did not hold: another configuration
    /// passphrase, or a damaged value.
    Check,
}

/// The text of a `Password V2` value (`02:…` or `03:…`), with the
/// configuration passphrase (empty unless one was set).
pub fn decrypt(value: &str, passphrase: &str) -> Result<String, CryptError> {
    let (prefix, hex) = value.trim().split_once(':').ok_or(CryptError::Format)?;
    let mut bytes = from_hex(hex).ok_or(CryptError::Format)?;
    let (key, iv, body) = match prefix {
        "02" => (Sha256::digest(passphrase.as_bytes()).into(), [0u8; 16], 0),
        "03" => {
            if bytes.len() < 16 {
                return Err(CryptError::Format);
            }
            let mut derived = [0u8; 48];
            bcrypt_pbkdf(passphrase.as_bytes(), &bytes[..16], 16, &mut derived);
            let key: [u8; 32] = derived[..32].try_into().map_err(|_| CryptError::Format)?;
            let iv: [u8; 16] = derived[32..].try_into().map_err(|_| CryptError::Format)?;
            (key, iv, 16)
        }
        _ => return Err(CryptError::Format),
    };
    let data = &mut bytes[body..];
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return Err(CryptError::Format);
    }
    let decrypted = cbc::Decryptor::<aes::Aes256>::new(&key.into(), &iv.into())
        .decrypt_padded_mut::<NoPadding>(data)
        .map(|plain| plain.len())
        .map_err(|_| CryptError::Format);
    // (the plaintext was decrypted in place: the buffer is wiped either way)
    let text = decrypted.and_then(|n| checked(&bytes[body..body + n]));
    bytes.fill(0);
    text
}

/// The text inside: its length, then it, then its SHA-256.
fn checked(plain: &[u8]) -> Result<String, CryptError> {
    let head: [u8; 4] = plain.get(..4).ok_or(CryptError::Check)?.try_into().map_err(|_| CryptError::Check)?;
    let len = u32::from_le_bytes(head) as usize;
    let text = plain.get(4..4 + len).ok_or(CryptError::Check)?;
    let sum = plain.get(4 + len..4 + len + 32).ok_or(CryptError::Check)?;
    if Sha256::digest(text).as_slice() != sum {
        return Err(CryptError::Check);
    }
    String::from_utf8(text.to_vec()).map_err(|_| CryptError::Check)
}

fn from_hex(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.trim();
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()
}

const BHASH_WORDS: usize = 8;
const BHASH_SIZE: usize = BHASH_WORDS * 4;
const BHASH_SEED: &[u8; BHASH_SIZE] = b"OxychromaticBlowfishSwatDynamite";

/// OpenSSH's bcrypt hash of SHA-512 digests (the `bcrypt-pbkdf` crate's
/// `bhash`).
fn bhash(sha2_pass: &[u8], sha2_salt: &[u8]) -> [u8; BHASH_SIZE] {
    let mut blowfish = Blowfish::bc_init_state();
    blowfish.salted_expand_key(sha2_salt, sha2_pass);
    for _ in 0..64 {
        blowfish.bc_expand_key(sha2_salt);
        blowfish.bc_expand_key(sha2_pass);
    }
    let mut cdata = [0u32; BHASH_WORDS];
    for (i, word) in cdata.iter_mut().enumerate() {
        *word = u32::from_be_bytes([
            BHASH_SEED[i * 4],
            BHASH_SEED[i * 4 + 1],
            BHASH_SEED[i * 4 + 2],
            BHASH_SEED[i * 4 + 3],
        ]);
    }
    for _ in 0..64 {
        for i in (0..BHASH_WORDS).step_by(2) {
            let [l, r] = blowfish.bc_encrypt([cdata[i], cdata[i + 1]]);
            cdata[i] = l;
            cdata[i + 1] = r;
        }
    }
    let mut out = [0u8; BHASH_SIZE];
    for (i, word) in cdata.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

/// OpenSSH's bcrypt_pbkdf: PBKDF2 with `bhash` as the PRF, the output
/// spread over the blocks. Unlike OpenSSH's, an empty passphrase is taken.
fn bcrypt_pbkdf(passphrase: &[u8], salt: &[u8], rounds: u32, output: &mut [u8]) {
    let stride = output.len().div_ceil(BHASH_SIZE);
    let sha2_pass = Sha512::digest(passphrase);
    let mut generated = vec![0u8; stride * BHASH_SIZE];
    for (block, chunk) in generated.chunks_mut(BHASH_SIZE).enumerate() {
        let mut salted = Sha512::new();
        salted.update(salt);
        salted.update(((block + 1) as u32).to_be_bytes());
        let mut u = bhash(&sha2_pass, &salted.finalize());
        chunk.copy_from_slice(&u);
        for _ in 1..rounds {
            u = bhash(&sha2_pass, &Sha512::digest(u));
            for (out, byte) in chunk.iter_mut().zip(u) {
                *out ^= byte;
            }
        }
    }
    for (i, out) in output.iter_mut().enumerate() {
        *out = generated[(i % stride) * BHASH_SIZE + i / stride];
    }
    generated.fill(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Made with HyperSine's securecrt_cipher.py (pycryptodome), empty
    /// configuration passphrase unless said: `enc --prefix 03 …`.
    #[test]
    fn values_made_by_the_public_decoder() {
        for (value, passphrase, text) in VECTORS {
            assert_eq!(decrypt(value, passphrase).as_deref(), Ok(*text), "{value}");
        }
    }

    #[test]
    fn a_wrong_passphrase_or_value_fails_the_check() {
        let (value, _, _) = VECTORS[0];
        assert_eq!(decrypt(value, "not it"), Err(CryptError::Check));
        assert_eq!(decrypt("04:00", ""), Err(CryptError::Format));
        assert_eq!(decrypt("03:zz", ""), Err(CryptError::Format));
        assert_eq!(decrypt("03:0011", ""), Err(CryptError::Format), "shorter than the salt");
        assert_eq!(decrypt("hunter2", ""), Err(CryptError::Format));
    }

    #[test]
    fn the_bcrypt_hash_is_openssh_s() {
        // the bcrypt-pbkdf crate's own test value (all-zero digests)
        let out = bhash(&[0; 64], &[0; 64]);
        assert_eq!(
            out,
            [
                0x46, 0x02, 0x86, 0xe9, 0x72, 0xfa, 0x83, 0x3f, 0x8b, 0x12, 0x83, 0xad, 0x8f, 0xa9, 0x19, 0xfa, 0x29,
                0xbd, 0xe2, 0x0e, 0x23, 0x32, 0x9e, 0x77, 0x4d, 0x84, 0x22, 0xba, 0xc0, 0xa7, 0x92, 0x6c
            ]
        );
    }

    const VECTORS: &[(&str, &str, &str)] = &[
        ("03:ee37ed18fd80d738b04abd10e247b4a90a83c120f1c2bce15411fec48149add6596d7f7a7a7e94a29d1bee9e229c11744d879c4becb1e7c1b9aa1c3140b08e03e3a08a9246d892ea30affc8a9f600b1f", "", "s3cret"),
        ("03:5a41e3edca5df63a756a581870d50a0c1039ed37dbe5630e389a042c1f3b26e1489acfc8f126a682a5cf6364afdbe9a51310647a26068ed9802aae72fa081a01b753c2cb2f1cc094083c8a4d4c2a0d73", "", "密码 with spaces"),
        ("02:e8ab25dc07655559e6fa70cdfecb5af11ea8d0783d534be792c25f3e9fb14268864e5f3669179eb4f9ebd6d5da73abe474af4878656fd97175a96f7de42b3d72", "", "root-pw"),
        ("03:bd21d4265e9773fac476f70853dce80f30773516b3cdb15ed8437300a1aec0ce53367824c893a285bb2701ed736d463bad85930e72faaf7405d7c88bf735b08b", "my config pass", "x"),
        ("02:474fb9895168da41789a1b98cfa1f05460f863f6769ce691157e99e7eb5f6b025e375d0164ab7d06ac873826ca9e0e0cffdc959ccc75e2ab5ec6d4aa3a7b396b6205fc3fb0e1b34cc3a6e22b3d2b2940fe411617d89c2f85e2a532d147f8792f", "pass2", "longer password 1234567890 1234567890"),
    ];
}
