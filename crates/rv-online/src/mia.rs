//! MIA ("missing in action"): the list of files that are known to be gone, fetched from the
//! service, kept in a cache, and applied to the database tree (`MIA::LoadmiaCRC`, `ApplyMIA`,
//! `online-protocol.md` section 5.3).
//!
//! The list is AES-256-CBC (PKCS#7) encrypted with a key and an IV from
//! [`PasswordDeriveBytes`](crate::kdf::PasswordDeriveBytes) over the user's DatVault key and the
//! salt of the user's own ROMVault exe ([`crate::salt`]). The cache file is the encrypted blob as
//! the server sent it, like ROMVault's `MIA.Cache`.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use aes::Aes256;
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, KeyIvInit};
use rv_core::{MiaStatus, RvFile};

use crate::client::Client;
use crate::kdf::PasswordDeriveBytes;
use crate::salt::Salt;

/// `MIACacheStatus` (`online-protocol.md` section 5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MiaCacheStatus {
    None = 0,
    Expired = 1,
    ExpiredNoChange = 2,
    NoChange = 3,
    NewFetched = 4,
    Failed = 5,
    DatVaultKeyNotFound = 6,
}

impl MiaCacheStatus {
    /// What the menu says for it (`MIA::UpdateMIA`).
    pub fn describe(self) -> &'static str {
        match self {
            MiaCacheStatus::None | MiaCacheStatus::NewFetched => "MIA list fetched",
            MiaCacheStatus::Expired => "Your DatVault membership is invalid or has expired",
            MiaCacheStatus::ExpiredNoChange => {
                "Your DatVault membership has expired; nothing changed"
            }
            MiaCacheStatus::NoChange => "The MIA list has not changed",
            MiaCacheStatus::Failed => "MIA fetch failed",
            MiaCacheStatus::DatVaultKeyNotFound => "A DatVault key is required for MIA updates",
        }
    }
}

/// Why an MIA blob could not be used. A wrong key shows up as `Decrypt` (bad padding) or as
/// `BadMagic`, never as a crash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MiaError {
    Kdf(String),
    Decrypt,
    BadMagic,
    Truncated,
    /// `miaver` is not the version the server announced.
    VersionMismatch,
}

impl fmt::Display for MiaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MiaError::Kdf(m) => write!(f, "key derivation failed: {m}"),
            MiaError::Decrypt => f.write_str("the MIA list does not decrypt (wrong key?)"),
            MiaError::BadMagic => f.write_str("the MIA list does not start with MIA"),
            MiaError::Truncated => f.write_str("the MIA list is cut short"),
            MiaError::VersionMismatch => f.write_str("the MIA list is not the version announced"),
        }
    }
}

impl std::error::Error for MiaError {}

/// `MIA::Decrypt(byte[], string)`: the key (32 bytes) is the first `GetBytes`, the IV (16 bytes)
/// the second, from the same derivation object. See [`crate::kdf`] for the leftover quirk.
pub fn derive_key_iv(
    key: &str,
    salt: &Salt,
) -> std::result::Result<([u8; 32], [u8; 16]), MiaError> {
    let mut pd = PasswordDeriveBytes::for_password(key, salt.as_bytes());
    let k = pd.get_bytes(32).map_err(|e| MiaError::Kdf(e.to_string()))?;
    let iv = pd.get_bytes(16).map_err(|e| MiaError::Kdf(e.to_string()))?;
    Ok((
        k.try_into().expect("32 bytes"),
        iv.try_into().expect("16 bytes"),
    ))
}

/// AES-256-CBC, PKCS#7.
pub fn decrypt(blob: &[u8], key: &str, salt: &Salt) -> std::result::Result<Vec<u8>, MiaError> {
    let (k, iv) = derive_key_iv(key, salt)?;
    cbc::Decryptor::<Aes256>::new(&k.into(), &iv.into())
        .decrypt_padded_vec_mut::<Pkcs7>(blob)
        .map_err(|_| MiaError::Decrypt)
}

/// One entry of the list (`miaFile`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MiaRecord {
    pub size: Option<u64>,
    pub crc: Option<[u8; 4]>,
    pub sha1: Option<[u8; 20]>,
    pub md5: Option<[u8; 16]>,
    /// .NET ticks of the day it went missing.
    pub date_ticks: Option<u64>,
}

/// The decrypted list with an index per hash.
#[derive(Debug, Default)]
pub struct MiaList {
    /// The release, as .NET ticks.
    pub miaver: u64,
    records: Vec<MiaRecord>,
    by_crc: HashMap<[u8; 4], Vec<u32>>,
    by_sha1: HashMap<[u8; 20], Vec<u32>>,
    by_md5: HashMap<[u8; 16], Vec<u32>>,
}

impl MiaList {
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn records(&self) -> &[MiaRecord] {
        &self.records
    }

    fn push(&mut self, r: MiaRecord) {
        let i = self.records.len() as u32;
        if let Some(c) = r.crc {
            self.by_crc.entry(c).or_default().push(i);
        }
        if let Some(s) = r.sha1 {
            self.by_sha1.entry(s).or_default().push(i);
        }
        if let Some(m) = r.md5 {
            self.by_md5.entry(m).or_default().push(i);
        }
        self.records.push(r);
    }
}

fn take<'a>(b: &'a [u8], at: &mut usize, n: usize) -> std::result::Result<&'a [u8], MiaError> {
    let s = b.get(*at..*at + n).ok_or(MiaError::Truncated)?;
    *at += n;
    Ok(s)
}

/// `LoadmiaCRC(byte[], long)` after the decryption: the file format of section 5.3.
/// `check_version > 0` demands that the list's own version equals it.
pub fn parse(plain: &[u8], check_version: i64) -> std::result::Result<MiaList, MiaError> {
    if plain.get(..3) != Some(b"MIA") {
        return Err(MiaError::BadMagic);
    }
    let mut at = 3;
    take(plain, &mut at, 1)?; // a byte the exe reads and ignores
    let miaver = u64::from_le_bytes(take(plain, &mut at, 8)?.try_into().expect("8 bytes"));
    if check_version > 0 && i128::from(miaver) != i128::from(check_version) {
        return Err(MiaError::VersionMismatch);
    }
    let mut list = MiaList {
        miaver,
        ..MiaList::default()
    };
    while at < plain.len() {
        let t = take(plain, &mut at, 1)?[0];
        let mut r = MiaRecord::default();
        if t & 1 != 0 {
            r.size = Some(u64::from_le_bytes(
                take(plain, &mut at, 8)?.try_into().expect("8"),
            ));
        }
        if t & 2 != 0 {
            r.crc = Some(take(plain, &mut at, 4)?.try_into().expect("4"));
        }
        if t & 4 != 0 {
            r.sha1 = Some(take(plain, &mut at, 20)?.try_into().expect("20"));
        }
        if t & 8 != 0 {
            r.md5 = Some(take(plain, &mut at, 16)?.try_into().expect("16"));
        }
        if t & 16 != 0 {
            r.date_ticks = Some(u64::from_le_bytes(
                take(plain, &mut at, 8)?.try_into().expect("8"),
            ));
        }
        list.push(r);
    }
    Ok(list)
}

/// Decrypt and parse.
pub fn load_list(
    blob: &[u8],
    key: &str,
    salt: &Salt,
    check_version: i64,
) -> std::result::Result<MiaList, MiaError> {
    parse(&decrypt(blob, key, salt)?, check_version)
}

// ---- the cache and the fetch ---------------------------------------------------------------

/// What [`fetch`] found out.
#[derive(Debug)]
pub struct Fetched {
    pub status: MiaCacheStatus,
    pub list: Option<MiaList>,
}

/// `MIA::LoadmiaCRC()`: the version check, the cache, the fetch.
///
/// 1. No key: `DatVaultKeyNotFound`.
/// 2. Ask the server for its version; any failure counts as `-1`. With `-1` the server is out of
///    reach or the membership is gone (`Expired`): no cache means `Expired`; a cache that
///    decrypts is used (`NoChange`), one that does not is `Failed`.
/// 3. Otherwise a cache whose own version is the server's is used (`NoChange`); if not, the list
///    is fetched, and when it decrypts it replaces the cache (`NewFetched`). A failed fetch, or a
///    list that does not decrypt, falls back to the cache as it is (`NoChange`), or `Failed`.
///
/// A wrong key never panics: it is `Failed`.
pub fn fetch(client: &Client, key: &str, salt: &Salt, cache: &Path) -> Fetched {
    let done = |status, list| Fetched { status, list };
    if key.trim().is_empty() {
        return done(MiaCacheStatus::DatVaultKeyNotFound, None);
    }
    let server_ver = client.get_auto_mia_version(key).unwrap_or(-1);
    let cached = std::fs::read(cache).ok();
    if server_ver == -1 {
        return match cached {
            None => done(MiaCacheStatus::Expired, None),
            Some(b) => match load_list(&b, key, salt, -1) {
                Ok(l) => done(MiaCacheStatus::NoChange, Some(l)),
                Err(_) => done(MiaCacheStatus::Failed, None),
            },
        };
    }
    if let Some(b) = &cached {
        if let Ok(l) = load_list(b, key, salt, server_ver) {
            return done(MiaCacheStatus::NoChange, Some(l));
        }
    }
    if let Ok(Some(blob)) = client.get_auto_mia(key) {
        if let Ok(l) = load_list(&blob, key, salt, -1) {
            // A cache that cannot be written is still a list that was fetched.
            return match crate::fsio::write_atomic(cache, &blob) {
                Ok(()) => done(MiaCacheStatus::NewFetched, Some(l)),
                Err(_) => done(MiaCacheStatus::Failed, Some(l)),
            };
        }
    }
    match cached.map(|b| load_list(&b, key, salt, -1)) {
        Some(Ok(l)) => done(MiaCacheStatus::NoChange, Some(l)),
        _ => done(MiaCacheStatus::Failed, None),
    }
}

// ---- applying the list ---------------------------------------------------------------------

/// How an update was asked for (`MIA.updateType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateType {
    /// The menu item: when nothing changed, nothing is applied.
    Regular,
    /// After a settings change: applied even if the list has not changed.
    DoUpdate,
    /// Shift on the menu item: applied, and the New/MIA flags are reset for every file first.
    ForceUpdate,
}

#[derive(Clone, Copy, Debug)]
pub struct ApplyOptions {
    /// `Settings.MIADays`: younger than this is "New", older is "MIA".
    pub mia_days: i64,
    pub force_update: bool,
    /// .NET UTC ticks of now.
    pub now_ticks: i64,
}

const TICKS_PER_DAY: f64 = 864_000_000_000.0;
const NEW_AND_MIA: MiaStatus = MiaStatus::NEW.with(MiaStatus::MIA);

fn same<const N: usize>(a: &Option<Vec<u8>>, b: &Option<[u8; N]>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.as_slice() == b.as_slice(),
        _ => true,
    }
}

/// The hashes of one side of a file: the main ones, or the header-stripped ("alt") ones.
struct Probe<'a> {
    size: Option<u64>,
    crc: &'a Option<Vec<u8>>,
    sha1: &'a Option<Vec<u8>>,
    md5: &'a Option<Vec<u8>>,
}

impl MiaList {
    /// `isExactMatch`: sizes equal if both are known, and every hash both have equal.
    fn exact(&self, i: u32, p: &Probe<'_>) -> bool {
        let r = &self.records[i as usize];
        if let (Some(a), Some(b)) = (p.size, r.size) {
            if a != b {
                return false;
            }
        }
        same(p.crc, &r.crc) && same(p.sha1, &r.sha1) && same(p.md5, &r.md5)
    }

    /// The SHA-1 list, then the CRC list, then the MD5 list.
    fn lookup(&self, p: &Probe<'_>) -> Option<&MiaRecord> {
        let key = |v: &Option<Vec<u8>>| v.clone();
        let cand = |ids: Option<&Vec<u32>>| -> Option<u32> {
            ids?.iter().copied().find(|&i| self.exact(i, p))
        };
        let hit = key(p.sha1)
            .and_then(|k| <[u8; 20]>::try_from(k).ok())
            .and_then(|k| cand(self.by_sha1.get(&k)))
            .or_else(|| {
                key(p.crc)
                    .and_then(|k| <[u8; 4]>::try_from(k).ok())
                    .and_then(|k| cand(self.by_crc.get(&k)))
            })
            .or_else(|| {
                key(p.md5)
                    .and_then(|k| <[u8; 16]>::try_from(k).ok())
                    .and_then(|k| cand(self.by_md5.get(&k)))
            });
        hit.map(|i| &self.records[i as usize])
    }

    /// `miaFound(file)`: the main hashes first, then the alt hashes if the file has any.
    pub fn find_file(&self, f: &RvFile) -> Option<&MiaRecord> {
        let main = Probe {
            size: f.size,
            crc: &f.crc,
            sha1: &f.sha1,
            md5: &f.md5,
        };
        if let Some(r) = self.lookup(&main) {
            return Some(r);
        }
        if f.alt_size.is_some()
            || f.alt_crc.is_some()
            || f.alt_sha1.is_some()
            || f.alt_md5.is_some()
        {
            let alt = Probe {
                size: f.alt_size,
                crc: &f.alt_crc,
                sha1: &f.alt_sha1,
                md5: &f.alt_md5,
            };
            return self.lookup(&alt);
        }
        None
    }
}

/// What applying a list did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub files: usize,
    pub found: usize,
}

fn apply_node(node: &mut RvFile, list: &MiaList, o: &ApplyOptions, out: &mut Applied) {
    if node.is_file() {
        out.files += 1;
        match list.find_file(node).cloned() {
            None => node.mia_clear(NEW_AND_MIA),
            Some(r) => {
                out.found += 1;
                let reset = if o.force_update {
                    NEW_AND_MIA
                } else {
                    MiaStatus::NEW
                };
                let young = r.date_ticks.is_some_and(|d| {
                    ((o.now_ticks as f64) - (d as f64)) / TICKS_PER_DAY < o.mia_days as f64
                });
                if young {
                    node.mia_clear(if o.force_update {
                        NEW_AND_MIA
                    } else {
                        MiaStatus::MIA
                    });
                    node.mia_set(MiaStatus::NEW);
                } else {
                    node.mia_clear(reset);
                    node.mia_set(MiaStatus::MIA);
                }
            }
        }
    }
    for c in &mut node.children {
        apply_node(c, list, o, out);
    }
}

/// `ApplyMIA(DB.DirRoot)`: the MIA flags of every file under the first child of `root` (the
/// `RomVault` tree; ToSort is left alone).
pub fn apply_mia(root: &mut RvFile, list: &MiaList, o: &ApplyOptions) -> Applied {
    let mut out = Applied::default();
    if let Some(tree) = root.children.first_mut() {
        apply_node(tree, list, o, &mut out);
    }
    out
}

/// `MIA::ClearOut`: take the New and MIA flags off every file.
pub fn clear_mia(root: &mut RvFile) {
    fn walk(n: &mut RvFile) {
        if n.is_file() {
            n.mia_clear(NEW_AND_MIA);
        }
        n.children.iter_mut().for_each(walk);
    }
    if let Some(tree) = root.children.first_mut() {
        walk(tree);
    }
}

/// Now, as .NET ticks.
pub fn now_ticks() -> i64 {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    rv_core::time::UNIX_EPOCH_TICKS + (d.as_secs() as i64) * 10_000_000
}

/// The calendar date (UTC, `YYYY-MM-DD`) of .NET ticks, for "MIA Release {date}".
pub fn ticks_to_date(ticks: u64) -> String {
    let days = (ticks / 864_000_000_000) as i64 - 719_162; // days since 1970-01-01
                                                           // Civil from days (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Build a list and encrypt it the way the server does. For the tests (and the mock).
#[cfg(any(test, feature = "mock"))]
pub mod testing {
    use super::*;
    use cbc::cipher::BlockEncryptMut;

    /// The plain bytes of a list: the file format of section 5.3 written out.
    pub fn serialize(miaver: u64, records: &[MiaRecord]) -> Vec<u8> {
        let mut b = b"MIA\x01".to_vec();
        b.extend_from_slice(&miaver.to_le_bytes());
        for r in records {
            let t = u8::from(r.size.is_some())
                | u8::from(r.crc.is_some()) << 1
                | u8::from(r.sha1.is_some()) << 2
                | u8::from(r.md5.is_some()) << 3
                | u8::from(r.date_ticks.is_some()) << 4;
            b.push(t);
            if let Some(s) = r.size {
                b.extend_from_slice(&s.to_le_bytes());
            }
            if let Some(c) = r.crc {
                b.extend_from_slice(&c);
            }
            if let Some(s) = r.sha1 {
                b.extend_from_slice(&s);
            }
            if let Some(m) = r.md5 {
                b.extend_from_slice(&m);
            }
            if let Some(d) = r.date_ticks {
                b.extend_from_slice(&d.to_le_bytes());
            }
        }
        b
    }

    /// AES-256-CBC, PKCS#7, with the key and IV the client derives.
    pub fn encrypt(plain: &[u8], key: &str, salt: &Salt) -> Vec<u8> {
        let (k, iv) = derive_key_iv(key, salt).expect("derive");
        cbc::Encryptor::<Aes256>::new(&k.into(), &iv.into()).encrypt_padded_vec_mut::<Pkcs7>(plain)
    }

    pub fn blob(miaver: u64, records: &[MiaRecord], key: &str, salt: &Salt) -> Vec<u8> {
        encrypt(&serialize(miaver, records), key, salt)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{blob, serialize};
    use super::*;
    use crate::mock::{self, loopback_client};
    use rv_core::FileType;

    const KEY: &str = "KEY-1234-secret";
    /// Synthetic: any 13 bytes, not ROMVault's.
    fn salt() -> Salt {
        Salt::from_bytes(&[3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5, 8, 9])
    }

    fn sha(n: u8) -> [u8; 20] {
        [n; 20]
    }

    fn rec(n: u8) -> MiaRecord {
        MiaRecord {
            size: Some(100 + u64::from(n)),
            crc: Some([n, 0, 0, 1]),
            sha1: Some(sha(n)),
            md5: Some([n; 16]),
            date_ticks: Some(638_000_000_000_000_000),
        }
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rv-online-mia-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // ---- crypto and format -----------------------------------------------------------------

    #[test]
    fn a_blob_round_trips_through_encrypt_and_decrypt() {
        let plain = serialize(7, &[rec(1), rec(2)]);
        let b = testing::encrypt(&plain, KEY, &salt());
        assert_eq!(b.len() % 16, 0);
        assert_ne!(
            &b[..plain.len().min(b.len())],
            &plain[..b.len().min(plain.len())]
        );
        assert_eq!(decrypt(&b, KEY, &salt()).unwrap(), plain);
    }

    #[test]
    fn a_wrong_key_or_salt_is_an_error_never_a_panic() {
        let b = blob(7, &[rec(1)], KEY, &salt());
        for wrong in ["", "x", "KEY-1234-secreT", "other key entirely"] {
            assert!(load_list(&b, wrong, &salt(), -1).is_err(), "{wrong:?}");
        }
        assert!(load_list(&b, KEY, &Salt::from_bytes(&[0; 13]), -1).is_err());
        // Not a multiple of the block size, and empty.
        assert_eq!(
            decrypt(&b[..b.len() - 1], KEY, &salt()),
            Err(MiaError::Decrypt)
        );
        assert_eq!(decrypt(&[], KEY, &salt()), Err(MiaError::Decrypt));
    }

    #[test]
    fn the_key_and_iv_follow_the_leftover_quirk() {
        // The IV is not the bytes a correct GetBytes(16) would give (see kdf.rs).
        let (k, iv) = derive_key_iv(KEY, &salt()).unwrap();
        let mut pd = PasswordDeriveBytes::for_password(KEY, salt().as_bytes());
        let k2 = pd.get_bytes(32).unwrap();
        let iv2 = pd.get_bytes(16).unwrap();
        assert_eq!(k.as_slice(), &k2[..]);
        assert_eq!(iv.as_slice(), &iv2[..]);
    }

    #[test]
    fn every_flag_combination_parses() {
        let mut records = Vec::new();
        for t in 0u8..32 {
            records.push(MiaRecord {
                size: (t & 1 != 0).then_some(u64::from(t) + 1000),
                crc: (t & 2 != 0).then_some([t, 1, 2, 3]),
                sha1: (t & 4 != 0).then_some([t; 20]),
                md5: (t & 8 != 0).then_some([t; 16]),
                date_ticks: (t & 16 != 0).then_some(u64::from(t) + 5),
            });
        }
        let list = parse(&serialize(42, &records), -1).unwrap();
        assert_eq!(list.miaver, 42);
        assert_eq!(list.records(), &records[..]);
        assert_eq!(list.len(), 32);
    }

    #[test]
    fn bad_files_are_errors() {
        assert_eq!(parse(b"MI", -1).unwrap_err(), MiaError::BadMagic);
        assert_eq!(
            parse(b"XIA\x01\0\0\0\0\0\0\0\0", -1).unwrap_err(),
            MiaError::BadMagic
        );
        assert_eq!(parse(b"MIA\x01\0\0", -1).unwrap_err(), MiaError::Truncated);
        let mut cut = serialize(1, &[rec(1)]);
        cut.pop();
        assert_eq!(parse(&cut, -1).unwrap_err(), MiaError::Truncated);
        // The version byte after the magic is ignored.
        let mut other = serialize(1, &[]);
        other[3] = 99;
        assert!(parse(&other, -1).is_ok());
        // An empty list is a list.
        assert!(parse(&serialize(1, &[]), -1).unwrap().is_empty());
    }

    #[test]
    fn the_version_check_applies_only_when_positive() {
        let p = serialize(500, &[]);
        assert!(parse(&p, 500).is_ok());
        assert!(parse(&p, -1).is_ok());
        assert!(parse(&p, 0).is_ok());
        assert_eq!(parse(&p, 501).unwrap_err(), MiaError::VersionMismatch);
    }

    // ---- the fetch, one test per status ----------------------------------------------------

    fn served(
        version: Option<i64>,
        blob: Option<Vec<u8>>,
    ) -> (mock::SharedState, mock::MockServer, Client) {
        let st = mock::state(KEY);
        {
            let mut s = st.lock().unwrap();
            s.mia_version = version;
            s.mia_blob = blob;
        }
        let srv = mock::start_service(&st);
        let c = loopback_client(&srv);
        (st, srv, c)
    }

    #[test]
    fn no_key_is_status_6_and_asks_nobody() {
        let (_st, srv, c) = served(Some(1), None);
        let dir = tmp("nokey");
        for k in ["", "   "] {
            let r = fetch(&c, k, &salt(), &dir.join("MIA.Cache"));
            assert_eq!(r.status, MiaCacheStatus::DatVaultKeyNotFound);
            assert!(r.list.is_none());
        }
        assert!(srv.requests().is_empty());
        assert_eq!(MiaCacheStatus::DatVaultKeyNotFound as u8, 6);
    }

    #[test]
    fn a_first_fetch_is_status_4_and_writes_the_encrypted_blob() {
        let b = blob(700, &[rec(1), rec(2)], KEY, &salt());
        let (_st, srv, c) = served(Some(700), Some(b.clone()));
        let cache = tmp("new").join("sub").join("MIA.Cache");
        let r = fetch(&c, KEY, &salt(), &cache);
        assert_eq!(r.status, MiaCacheStatus::NewFetched);
        assert_eq!(r.list.unwrap().len(), 2);
        assert_eq!(std::fs::read(&cache).unwrap(), b);
        assert_eq!(srv.count("GetAutoMIAVersion"), 1);
        assert_eq!(srv.count("GetAutoMIA"), 1);
    }

    #[test]
    fn a_current_cache_is_status_3_without_a_download() {
        let b = blob(700, &[rec(1)], KEY, &salt());
        let (_st, srv, c) = served(Some(700), Some(vec![0; 16]));
        let cache = tmp("cur").join("MIA.Cache");
        std::fs::write(&cache, &b).unwrap();
        let r = fetch(&c, KEY, &salt(), &cache);
        assert_eq!(r.status, MiaCacheStatus::NoChange);
        assert_eq!(r.list.unwrap().miaver, 700);
        assert_eq!(srv.count("GetAutoMIA"), 0);
        assert_eq!(std::fs::read(&cache).unwrap(), b);
    }

    #[test]
    fn an_old_cache_is_replaced_by_a_newer_list() {
        let old = blob(600, &[rec(1)], KEY, &salt());
        let new = blob(700, &[rec(1), rec(2), rec(3)], KEY, &salt());
        let (_st, srv, c) = served(Some(700), Some(new.clone()));
        let cache = tmp("old").join("MIA.Cache");
        std::fs::write(&cache, &old).unwrap();
        let r = fetch(&c, KEY, &salt(), &cache);
        assert_eq!(r.status, MiaCacheStatus::NewFetched);
        assert_eq!(r.list.unwrap().len(), 3);
        assert_eq!(std::fs::read(&cache).unwrap(), new);
        assert_eq!(srv.count("GetAutoMIA"), 1);
    }

    #[test]
    fn no_version_and_no_cache_is_status_1() {
        // The service answers -1 for a key it does not know (or a lapsed membership).
        let (_st, srv, c) = served(Some(700), Some(vec![]));
        let cache = tmp("exp").join("MIA.Cache");
        let r = fetch(&c, "someone else's key", &salt(), &cache);
        assert_eq!(r.status, MiaCacheStatus::Expired);
        assert!(r.list.is_none());
        assert_eq!(srv.count("GetAutoMIA"), 0);
        assert!(!cache.exists());
        // The same when the version call faults.
        let (_st, _srv, c) = served(None, None);
        assert_eq!(
            fetch(&c, KEY, &salt(), &cache).status,
            MiaCacheStatus::Expired
        );
    }

    #[test]
    fn no_version_but_a_cache_uses_the_stale_cache() {
        let old = blob(600, &[rec(1)], KEY, &salt());
        let (_st, _srv, c) = served(None, None);
        let cache = tmp("stale").join("MIA.Cache");
        std::fs::write(&cache, &old).unwrap();
        let r = fetch(&c, KEY, &salt(), &cache);
        assert_eq!(r.status, MiaCacheStatus::NoChange);
        assert_eq!(r.list.unwrap().miaver, 600);
        // A stale cache that does not decrypt is a failure.
        std::fs::write(&cache, b"not a cache at all, 32 bytes!!!!!").unwrap();
        let r = fetch(&c, KEY, &salt(), &cache);
        assert_eq!(r.status, MiaCacheStatus::Failed);
        assert!(r.list.is_none());
    }

    #[test]
    fn a_list_made_with_another_key_is_status_5() {
        // The service accepts KEY but ships a blob that KEY does not open.
        let wrong = blob(700, &[rec(1)], "another key", &salt());
        let (_st, _srv, c) = served(Some(700), Some(wrong));
        let cache = tmp("wrongkey").join("MIA.Cache");
        let r = fetch(&c, KEY, &salt(), &cache);
        assert_eq!(r.status, MiaCacheStatus::Failed);
        assert!(r.list.is_none());
        assert!(!cache.exists(), "a blob that does not open is not cached");
        // The same with the right key but another salt: the exe the user named is not 3.8.0's.
        let b = blob(700, &[rec(1)], KEY, &Salt::from_bytes(&[9; 13]));
        let (_st, _srv, c) = served(Some(700), Some(b));
        assert_eq!(
            fetch(&c, KEY, &salt(), &cache).status,
            MiaCacheStatus::Failed
        );
    }

    #[test]
    fn a_failed_download_falls_back_to_the_cache() {
        let old = blob(600, &[rec(1)], KEY, &salt());
        let (_st, srv, c) = served(Some(700), None); // GetAutoMIA faults
        let cache = tmp("fb").join("MIA.Cache");
        std::fs::write(&cache, &old).unwrap();
        let r = fetch(&c, KEY, &salt(), &cache);
        assert_eq!(srv.count("GetAutoMIA"), 1);
        assert_eq!(r.status, MiaCacheStatus::NoChange);
        assert_eq!(r.list.unwrap().miaver, 600);
        // Without a cache, the same failure is a failure.
        std::fs::remove_file(&cache).unwrap();
        assert_eq!(
            fetch(&c, KEY, &salt(), &cache).status,
            MiaCacheStatus::Failed
        );
    }

    #[test]
    fn a_network_error_is_status_1_or_the_cache() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let ep = crate::Endpoint::new(&format!("http://127.0.0.1:{port}/x"), true).unwrap();
        let c = Client::with_transport(
            ep,
            Box::new(crate::mock::LoopbackOnly(crate::transport::UreqTransport)),
        );
        let cache = tmp("net").join("MIA.Cache");
        assert_eq!(
            fetch(&c, KEY, &salt(), &cache).status,
            MiaCacheStatus::Expired
        );
        std::fs::write(&cache, blob(1, &[rec(1)], KEY, &salt())).unwrap();
        assert_eq!(
            fetch(&c, KEY, &salt(), &cache).status,
            MiaCacheStatus::NoChange
        );
    }

    #[test]
    fn statuses_two_and_zero_are_never_returned_as_in_the_exe() {
        // `MIA::LoadmiaCRC()` has no path that returns ExpiredNoChange (2), and None (0) is the
        // "apply" case of `UpdateMIA`'s switch only; both exist for the numbering.
        assert_eq!(MiaCacheStatus::None as u8, 0);
        assert_eq!(MiaCacheStatus::ExpiredNoChange as u8, 2);
        for s in [
            MiaCacheStatus::None,
            MiaCacheStatus::Expired,
            MiaCacheStatus::ExpiredNoChange,
            MiaCacheStatus::NoChange,
            MiaCacheStatus::NewFetched,
            MiaCacheStatus::Failed,
            MiaCacheStatus::DatVaultKeyNotFound,
        ] {
            assert!(!s.describe().is_empty());
        }
    }

    // ---- applying --------------------------------------------------------------------------

    fn file(name: &str, size: u64, n: u8) -> RvFile {
        let mut f = RvFile::new(FileType::FILE, name);
        f.size = Some(size);
        f.crc = Some(vec![n, 0, 0, 1]);
        f.sha1 = Some(vec![n; 20]);
        f.md5 = Some(vec![n; 16]);
        f
    }

    fn tree(files: Vec<RvFile>, tosort: Vec<RvFile>) -> RvFile {
        let mut rv = RvFile::new(FileType::DIR, "RomVault");
        let mut game = RvFile::new(FileType::ZIP, "game.zip");
        game.children = files
            .into_iter()
            .map(|mut f| {
                f.file_type = FileType::FILE_ZIP;
                f
            })
            .collect();
        rv.children.push(game);
        let mut ts = RvFile::new(FileType::DIR, "ToSort");
        ts.children = tosort;
        let mut root = RvFile::new(FileType::DIR, "DirRoot");
        root.children = vec![rv, ts];
        root
    }

    fn opts(force: bool) -> ApplyOptions {
        ApplyOptions {
            mia_days: 14,
            force_update: force,
            // 10 days after rec()'s date.
            now_ticks: 638_000_000_000_000_000 + 10 * 864_000_000_000,
        }
    }

    fn mia_of(root: &RvFile, i: usize) -> MiaStatus {
        root.children[0].children[0].children[i].mia()
    }

    #[test]
    fn matching_by_each_hash_and_the_exact_match_rules() {
        let list = parse(&serialize(1, &[rec(1)]), -1).unwrap();
        // 0: everything equal. 1: size differs. 2: sha1 equal but crc differs. 3: nothing equal.
        // 4: only a crc, equal. 5: only a size.
        let mut f4 = RvFile::new(FileType::FILE_ZIP, "crc-only");
        f4.crc = Some(vec![1, 0, 0, 1]);
        let mut f5 = RvFile::new(FileType::FILE_ZIP, "size-only");
        f5.size = Some(101);
        let mut f2 = file("crc-differs", 101, 1);
        f2.crc = Some(vec![9, 9, 9, 9]);
        let mut root = tree(
            vec![
                file("equal", 101, 1),
                file("size-differs", 999, 1),
                f2,
                file("other", 101, 2),
                f4,
                f5,
            ],
            vec![],
        );
        let a = apply_mia(&mut root, &list, &opts(false));
        assert_eq!(a, Applied { files: 6, found: 2 });
        let got: Vec<bool> = (0..6)
            .map(|i| {
                mia_of(&root, i).contains(MiaStatus::NEW)
                    || mia_of(&root, i).contains(MiaStatus::MIA)
            })
            .collect();
        assert_eq!(got, [true, false, false, false, true, false]);
    }

    #[test]
    fn md5_and_the_alt_hashes_find_a_record_too() {
        let md5_only = MiaRecord {
            md5: Some([5; 16]),
            ..MiaRecord::default()
        };
        let alt_rec = MiaRecord {
            sha1: Some([6; 20]),
            size: Some(50),
            ..MiaRecord::default()
        };
        let list = parse(&serialize(1, &[md5_only, alt_rec]), -1).unwrap();
        let mut by_md5 = RvFile::new(FileType::FILE_ZIP, "md5");
        by_md5.md5 = Some(vec![5; 16]);
        let mut by_alt = file("alt", 1000, 0);
        by_alt.alt_size = Some(50);
        by_alt.alt_sha1 = Some(vec![6; 20]);
        let none = file("none", 1000, 0);
        let mut root = tree(vec![by_md5, by_alt, none], vec![]);
        let a = apply_mia(&mut root, &list, &opts(false));
        assert_eq!(a.found, 2);
        assert!(mia_of(&root, 0).contains(MiaStatus::MIA));
        assert!(mia_of(&root, 1).contains(MiaStatus::MIA));
        assert_eq!(mia_of(&root, 2), MiaStatus::NONE);
    }

    #[test]
    fn young_records_are_new_and_old_ones_are_mia() {
        let young = MiaRecord {
            date_ticks: Some(638_000_000_000_000_000 + 5 * 864_000_000_000),
            ..rec(1)
        };
        let old = MiaRecord {
            date_ticks: Some(638_000_000_000_000_000 - 40 * 864_000_000_000),
            ..rec(2)
        };
        let undated = MiaRecord {
            date_ticks: None,
            ..rec(3)
        };
        let list = parse(&serialize(1, &[young, old, undated]), -1).unwrap();
        let mut root = tree(
            vec![file("y", 101, 1), file("o", 102, 2), file("u", 103, 3)],
            vec![],
        );
        apply_mia(&mut root, &list, &opts(false));
        assert_eq!(mia_of(&root, 0), MiaStatus::NEW);
        assert_eq!(mia_of(&root, 1), MiaStatus::MIA);
        assert_eq!(mia_of(&root, 2), MiaStatus::MIA);
    }

    #[test]
    fn a_file_no_longer_listed_loses_both_flags_and_force_resets_them() {
        let list = parse(&serialize(1, &[rec(1)]), -1).unwrap();
        let mut stale = file("stale", 555, 7);
        stale.mia_set(
            MiaStatus::NEW
                .with(MiaStatus::MIA)
                .with(MiaStatus::MIA_USER),
        );
        let mut listed = file("listed", 101, 1);
        listed.mia_set(MiaStatus::NEW); // old (rec is 10 days old < 14 -> young): keeps NEW
        let mut root = tree(vec![stale, listed], vec![]);
        apply_mia(&mut root, &list, &opts(false));
        // The user's own flag stays; New and MIA go.
        assert_eq!(mia_of(&root, 0), MiaStatus::MIA_USER);
        assert_eq!(mia_of(&root, 1), MiaStatus::NEW);
        // Force: a young file gets both flags cleared first, then New set; an old one MIA only.
        let mut f = file("listed", 101, 1);
        f.mia_set(MiaStatus::MIA);
        let mut root = tree(vec![f], vec![]);
        apply_mia(&mut root, &list, &opts(true));
        assert_eq!(mia_of(&root, 0), MiaStatus::NEW);
    }

    #[test]
    fn to_sort_is_not_touched_and_clear_removes_the_flags() {
        let list = parse(&serialize(1, &[rec(1)]), -1).unwrap();
        let mut root = tree(vec![file("a", 101, 1)], vec![file("in-tosort", 101, 1)]);
        apply_mia(&mut root, &list, &opts(false));
        assert_ne!(mia_of(&root, 0), MiaStatus::NONE);
        assert_eq!(root.children[1].children[0].mia(), MiaStatus::NONE);
        clear_mia(&mut root);
        assert_eq!(mia_of(&root, 0), MiaStatus::NONE);
        // No tree at all is fine.
        let mut empty = RvFile::new(FileType::DIR, "DirRoot");
        assert_eq!(
            apply_mia(&mut empty, &list, &opts(false)),
            Applied::default()
        );
    }

    #[test]
    fn ticks_to_date_known_days() {
        // The cache stores 638607595200000000 for 2024-09-01 03:52 UTC (rv-core's time tests).
        assert_eq!(ticks_to_date(638_607_595_200_000_000), "2024-09-01");
        assert_eq!(ticks_to_date(621_355_968_000_000_000), "1970-01-01");
        assert_eq!(
            ticks_to_date(621_355_968_000_000_000 + 11_016 * 864_000_000_000 + 5),
            "2000-02-29"
        );
        assert_eq!(ticks_to_date(0), "0001-01-01");
    }

    #[test]
    fn now_is_after_2024() {
        assert!(now_ticks() > 638_000_000_000_000_000);
    }
}
