//! The 13-byte MIA salt, read at run time from the user's own ROMVault exe.
//!
//! The salt is a constant in `ROMVault38.exe`. It is **not** in this repository, in a test or in a
//! comment (plan rev 4); only where to find it is. The exe is accepted only if its SHA-256 is
//! the one `online-protocol.md` section 12 records, and the salt is the 13 bytes at the file
//! position of RVA `0x3EFEF5`, the `FieldRVA` of the `<PrivateImplementationDetails>` field the
//! IL of `MIA::Decrypt(byte[], string) @0001..@000e` initialises its array from (section 7).
//! The file position comes from the PE section table.

use std::fmt;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// Where the salt is and which exe it is in.
#[derive(Clone, Copy, Debug)]
pub struct SaltSource {
    /// Lower-case hex SHA-256 of the whole exe.
    pub exe_sha256: &'static str,
    pub rva: u32,
    pub len: usize,
}

/// `ROMVault38.exe` 3.8.0 (5,457,408 bytes).
pub const ROMVAULT_3_8_0: SaltSource = SaltSource {
    exe_sha256: "a227b30f4cb08a5472098237f99389cd4bc328d8349ffd118992d6e4f91bb8eb",
    rva: 0x003E_FEF5,
    len: 13,
};

/// The salt. It does not show in `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub struct Salt(Vec<u8>);

impl Salt {
    pub fn from_bytes(bytes: &[u8]) -> Salt {
        Salt(bytes.to_vec())
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Salt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Salt({} bytes)", self.0.len())
    }
}

fn le16(b: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as usize)
}

fn le32(b: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize)
}

/// The file offset of `rva` through the section table of the PE image `b`.
fn rva_to_offset(b: &[u8], rva: usize) -> Option<usize> {
    if b.get(..2)? != b"MZ" {
        return None;
    }
    let pe = le32(b, 0x3C)?;
    if b.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let sections = le16(b, pe + 6)?;
    let opt = le16(b, pe + 20)?;
    let table = pe + 24 + opt;
    for i in 0..sections {
        let s = table + i * 40;
        let (vsize, va, raw_size, raw_ptr) = (
            le32(b, s + 8)?,
            le32(b, s + 12)?,
            le32(b, s + 16)?,
            le32(b, s + 20)?,
        );
        if rva >= va && rva < va + vsize.max(raw_size) {
            return Some(raw_ptr + (rva - va));
        }
    }
    None
}

/// Read the salt `src` describes out of the exe at `exe`.
pub fn read_salt(exe: &Path, src: &SaltSource) -> Result<Salt> {
    let bytes = std::fs::read(exe)
        .map_err(|e| Error::Salt(format!("cannot read {}: {e}", exe.display())))?;
    let sha: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if sha != src.exe_sha256 {
        return Err(Error::Salt(format!(
            "{} is not the ROMVault 3.8.0 exe this version knows (SHA-256 {sha})",
            exe.display()
        )));
    }
    let off = rva_to_offset(&bytes, src.rva as usize)
        .ok_or_else(|| Error::Salt("the exe has no section for the salt".into()))?;
    let salt = bytes
        .get(off..off + src.len)
        .ok_or_else(|| Error::Salt("the exe ends before the salt".into()))?;
    Ok(Salt(salt.to_vec()))
}

/// A synthetic PE image for the tests: one `.text` section at RVA 0x1000, raw data at 0x200,
/// with `payload` at `rva`. Public to the crate's tests only.
#[cfg(any(test, feature = "mock"))]
pub fn synthetic_exe(rva: u32, payload: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 0x200 + 0x2000];
    b[0..2].copy_from_slice(b"MZ");
    b[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    b[0x80..0x84].copy_from_slice(b"PE\0\0");
    b[0x86..0x88].copy_from_slice(&1u16.to_le_bytes()); // one section
    b[0x94..0x96].copy_from_slice(&0xE0u16.to_le_bytes()); // optional header size
    let s = 0x80 + 24 + 0xE0;
    b[s..s + 5].copy_from_slice(b".text");
    b[s + 8..s + 12].copy_from_slice(&0x2000u32.to_le_bytes()); // virtual size
    b[s + 12..s + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // virtual address
    b[s + 16..s + 20].copy_from_slice(&0x2000u32.to_le_bytes()); // raw size
    b[s + 20..s + 24].copy_from_slice(&0x200u32.to_le_bytes()); // raw pointer
    let off = 0x200 + (rva - 0x1000) as usize;
    b[off..off + payload.len()].copy_from_slice(payload);
    b
}

/// The `SaltSource` of a synthetic exe.
#[cfg(any(test, feature = "mock"))]
pub fn synthetic_source(exe: &[u8], rva: u32, len: usize) -> SaltSource {
    let sha: String = Sha256::digest(exe)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    SaltSource {
        exe_sha256: Box::leak(sha.into_boxed_str()),
        rva,
        len,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not ROMVault's salt: any 13 bytes.
    const SYNTH: [u8; 13] = [
        0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87, 0x98, 0xA9, 0xBA, 0xCB, 0xDC,
    ];

    fn write(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rv-online-salt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn the_salt_is_found_through_the_section_table() {
        let exe = synthetic_exe(0x1ABC, &SYNTH);
        let src = synthetic_source(&exe, 0x1ABC, 13);
        let p = write("good.exe", &exe);
        assert_eq!(read_salt(&p, &src).unwrap().as_bytes(), &SYNTH);
    }

    #[test]
    fn another_file_is_refused_before_anything_is_read_from_it() {
        let exe = synthetic_exe(0x1ABC, &SYNTH);
        let src = synthetic_source(&exe, 0x1ABC, 13);
        let mut other = exe.clone();
        other[0x300] ^= 1;
        let e = read_salt(&write("other.exe", &other), &src).unwrap_err();
        assert!(matches!(e, Error::Salt(_)));
        assert!(e.to_string().contains("not the ROMVault 3.8.0 exe"), "{e}");
        let e = read_salt(Path::new("/nonexistent/rv/ROMVault38.exe"), &src).unwrap_err();
        assert!(matches!(e, Error::Salt(_)));
    }

    #[test]
    fn an_rva_outside_every_section_or_past_the_end_is_an_error() {
        let exe = synthetic_exe(0x1ABC, &SYNTH);
        let p = write("range.exe", &exe);
        assert!(read_salt(&p, &synthetic_source(&exe, 0x9000, 13)).is_err());
        assert!(read_salt(&p, &synthetic_source(&exe, 0x2FF8, 13)).is_err());
    }

    #[test]
    fn not_a_pe_file_is_an_error() {
        let junk = vec![7u8; 4096];
        let src = synthetic_source(&junk, 0x1000, 13);
        let e = read_salt(&write("junk.exe", &junk), &src).unwrap_err();
        assert!(matches!(e, Error::Salt(_)));
    }

    #[test]
    fn debug_shows_no_bytes() {
        let s = Salt::from_bytes(&SYNTH);
        let text = format!("{s:?}");
        assert!(
            !text.contains("16") && !text.contains("0x10") && !text.contains("DC"),
            "{text}"
        );
    }

    #[test]
    fn the_known_exe_is_described_by_the_spec() {
        // online-protocol.md section 12 and section 7 (Field row 5842, RVA 0x3EFEF5, 13 bytes).
        assert_eq!(ROMVAULT_3_8_0.len, 13);
        assert_eq!(ROMVAULT_3_8_0.rva, 0x3EFEF5);
        assert_eq!(ROMVAULT_3_8_0.exe_sha256.len(), 64);
    }
}
