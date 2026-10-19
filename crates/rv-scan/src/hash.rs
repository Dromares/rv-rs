//! The four hashes, fed from one pass over the data.

use md5::{Digest, Md5};
use sha1::Sha1;
use sha2::Sha256;

/// Which hashes a [`HashSet`] computes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HashKinds {
    pub crc: bool,
    pub md5: bool,
    pub sha1: bool,
    pub sha256: bool,
}

impl HashKinds {
    /// CRC32, MD5 and SHA1, which is all that ROMVault 3.8.0 ever enables
    /// (`Scanner/Populate.cs:35,169` leave `scanSHA256` false).
    pub const ROMVAULT: HashKinds = HashKinds {
        crc: true,
        md5: true,
        sha1: true,
        sha256: false,
    };
    /// All four.
    pub const ALL: HashKinds = HashKinds {
        crc: true,
        md5: true,
        sha1: true,
        sha256: true,
    };
}

/// Results of a finished [`HashSet`]. CRC is 4 bytes big endian, MD5 16, SHA1 20, SHA256 32,
/// so `bytes[0]` is the most significant byte, as in `Crc32ResultB` (`CRC.cs:97-105`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Digests {
    pub crc: Option<[u8; 4]>,
    pub md5: Option<[u8; 16]>,
    pub sha1: Option<[u8; 20]>,
    pub sha256: Option<[u8; 32]>,
}

/// Running hashes over one stream of bytes.
#[derive(Clone)]
pub struct HashSet {
    crc: Option<crc32fast::Hasher>,
    md5: Option<Md5>,
    sha1: Option<Sha1>,
    sha256: Option<Sha256>,
}

impl HashSet {
    pub fn new(kinds: HashKinds) -> HashSet {
        HashSet {
            crc: kinds.crc.then(crc32fast::Hasher::new),
            md5: kinds.md5.then(Md5::new),
            sha1: kinds.sha1.then(Sha1::new),
            sha256: kinds.sha256.then(Sha256::new),
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        if let Some(h) = &mut self.crc {
            h.update(data);
        }
        if let Some(h) = &mut self.md5 {
            h.update(data);
        }
        if let Some(h) = &mut self.sha1 {
            h.update(data);
        }
        if let Some(h) = &mut self.sha256 {
            h.update(data);
        }
    }

    pub fn finish(self) -> Digests {
        Digests {
            crc: self.crc.map(|h| h.finalize().to_be_bytes()),
            md5: self.md5.map(|h| h.finalize().into()),
            sha1: self.sha1.map(|h| h.finalize().into()),
            sha256: self.sha256.map(|h| h.finalize().into()),
        }
    }
}

/// Lower-case hex of `bytes`.
pub fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
