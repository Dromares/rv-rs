//! `System.Security.Cryptography.PasswordDeriveBytes` with SHA-1: PBKDF1 extended with a counter,
//! including the way `GetBytes` hands out the bytes left over from an earlier call.
//!
//! ROMVault's `MIA::Decrypt(byte[], string)` asks one `PasswordDeriveBytes(key, salt)` (SHA-1,
//! 100 iterations) for `GetBytes(32)` and then `GetBytes(16)`; the second call is where the BCL's
//! known bug sits, and the IV depends on it (`online-protocol.md` section 7).
//!
//! **Reference.** This is a port of Microsoft's published .NET Framework reference source,
//! `mscorlib/system/security/cryptography/passwordderivebytes.cs`
//! (<https://github.com/microsoft/referencesource>, MIT licence): `ComputeBaseValue`,
//! `ComputeBytes`, `HashPrefix` and `GetBytes`, including the comment there that the partial copy
//! "should really be `_extraCount` instead" but was left as shipped in v1.x.
//!
//! **Known-answer tests.** The vectors in the tests are Mono's own, from
//! `mcs/class/corlib/Test/System.Security.Cryptography/PasswordDeriveBytesTest.cs`
//! (<https://github.com/mono/mono>, MIT licence), which run the same constructor with the same
//! inputs and compare the last four bytes of the derived key. They exercise the base value, the
//! counter prefix and the block chaining. The `GetBytes(32)` then `GetBytes(16)` leftover
//! handling is not in that suite; it is tested against the reference source's logic only and has
//! **not** been compared with output from Microsoft's runtime.

use sha1::{Digest, Sha1};

const HASH_LEN: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KdfError {
    /// `GetBytes(0)`: the reference throws `IndexOutOfRangeException`.
    ZeroLength,
    /// More than 1000 blocks: `Cryptography_PasswordDerivedBytes_TooManyBytes`.
    TooManyBytes,
    /// `Buffer.InternalBlockCopy` out of range in the leftover branch.
    CopyRange,
}

impl std::fmt::Display for KdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            KdfError::ZeroLength => "cannot derive zero bytes",
            KdfError::TooManyBytes => "too many bytes asked of the key derivation",
            KdfError::CopyRange => "key derivation leftover copy is out of range",
        })
    }
}

impl std::error::Error for KdfError {}

pub struct PasswordDeriveBytes {
    password: Vec<u8>,
    salt: Vec<u8>,
    iterations: u32,
    base: Option<[u8; HASH_LEN]>,
    prefix: u32,
    extra: Option<Vec<u8>>,
    extra_count: usize,
}

impl PasswordDeriveBytes {
    /// `new PasswordDeriveBytes(byte[] password, byte[] salt, "SHA1", iterations)`. `iterations`
    /// is at least 1.
    pub fn new(password: &[u8], salt: &[u8], iterations: u32) -> PasswordDeriveBytes {
        PasswordDeriveBytes {
            password: password.to_vec(),
            salt: salt.to_vec(),
            iterations: iterations.max(1),
            base: None,
            prefix: 0,
            extra: None,
            extra_count: 0,
        }
    }

    /// The two-argument string constructor ROMVault calls: UTF-8 (no BOM) bytes of the password,
    /// SHA-1, 100 iterations.
    pub fn for_password(password: &str, salt: &[u8]) -> PasswordDeriveBytes {
        PasswordDeriveBytes::new(password.as_bytes(), salt, 100)
    }

    /// `ComputeBaseValue`: SHA1(password || salt), then `iterations - 2` more hashes.
    fn compute_base(&mut self) -> [u8; HASH_LEN] {
        let mut h = Sha1::new();
        h.update(&self.password);
        h.update(&self.salt);
        let mut base: [u8; HASH_LEN] = h.finalize().into();
        for _ in 1..self.iterations.saturating_sub(1) {
            base = Sha1::digest(base).into();
        }
        self.base = Some(base);
        base
    }

    /// `HashPrefix` + the hash of one block: SHA1(decimal counter || base), no prefix for 0.
    fn block(&mut self, base: &[u8; HASH_LEN]) -> Result<[u8; HASH_LEN], KdfError> {
        if self.prefix > 999 {
            return Err(KdfError::TooManyBytes);
        }
        let mut h = Sha1::new();
        if self.prefix > 0 {
            h.update(self.prefix.to_string().as_bytes());
        }
        self.prefix += 1;
        h.update(base);
        Ok(h.finalize().into())
    }

    /// `ComputeBytes(cb)`: whole blocks, `ceil(cb / 20)` of them.
    fn compute_bytes(&mut self, cb: usize) -> Result<Vec<u8>, KdfError> {
        if cb == 0 {
            return Err(KdfError::ZeroLength);
        }
        let base = self.base.expect("the base value is computed first");
        let mut out = Vec::with_capacity(cb.div_ceil(HASH_LEN) * HASH_LEN);
        while out.len() < cb {
            out.extend_from_slice(&self.block(&base)?);
        }
        Ok(out)
    }

    /// `GetBytes(cb)`.
    pub fn get_bytes(&mut self, cb: usize) -> Result<Vec<u8>, KdfError> {
        if cb == 0 {
            return Err(KdfError::ZeroLength);
        }
        let mut out = vec![0u8; cb];
        let mut ib = 0;
        if self.base.is_none() {
            self.compute_base();
        } else if let Some(extra) = self.extra.take() {
            ib = extra.len() - self.extra_count;
            if ib >= cb {
                out.copy_from_slice(&extra[self.extra_count..self.extra_count + cb]);
                if ib > cb {
                    self.extra_count += cb;
                    self.extra = Some(extra);
                }
                return Ok(out);
            }
            // The reference copies from offset `ib` where `_extraCount` was meant, and kept it so
            // for compatibility with v1.x. ROMVault's IV is made from these bytes.
            if extra.len() < 2 * ib {
                return Err(KdfError::CopyRange);
            }
            out[..ib].copy_from_slice(&extra[ib..2 * ib]);
        }
        let rgb = self.compute_bytes(cb - ib)?;
        out[ib..].copy_from_slice(&rgb[..cb - ib]);
        if rgb.len() + ib > cb {
            self.extra = Some(rgb);
            self.extra_count = cb - ib;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `salt` of the Mono test fixture.
    const SALT: [u8; 4] = [0xDE, 0xAD, 0xC0, 0xDE];

    fn last4(key: &[u8]) -> i32 {
        let n = key.len();
        i32::from_le_bytes([key[n - 4], key[n - 3], key[n - 2], key[n - 1]])
    }

    /// Mono's `Run(password, salt, "SHA1", iterations, getbytes, lastFourBytes)`.
    fn run(password: &str, salt: Option<&[u8]>, iterations: u32, getbytes: usize, want: i32) {
        let mut pd = PasswordDeriveBytes::new(password.as_bytes(), salt.unwrap_or(&[]), iterations);
        let key = pd.get_bytes(getbytes).unwrap();
        assert_eq!(key.len(), getbytes);
        assert_eq!(
            last4(&key),
            want,
            "password={password} salt={} iterations={iterations} get={getbytes}",
            salt.map_or(0, <[u8]>::len)
        );
    }

    #[test]
    fn mono_one_iteration() {
        run("password", Some(&SALT), 1, 20, -1_251_929_751);
        run("monomono", None, 1, 40, -1_148_594_972);
    }

    #[test]
    fn mono_sha1_known_answers() {
        // PasswordDeriveBytesTest.cs, `SHA1 ()`.
        run("password", None, 10, 10, -852_142_057);
        run("password", Some(&SALT), 20, 20, -1_096_621_819);
        run("password", None, 30, 30, 1_748_347_042);
        run("password", Some(&SALT), 40, 40, 900_690_664);
        run("password", None, 50, 50, 2_125_027_038);
        run("monomono", Some(&SALT), 60, 60, -1_167_799_882);
        run("monomono", None, 70, 70, -1_967_623_713);
        run("monomono", Some(&SALT), 80, 80, 598_766_793);
        run("monomono", None, 90, 90, -1_754_629_926);
        run("monomono", Some(&SALT), 100, 100, 1_247_157_997);
    }

    #[test]
    fn mono_old_tests() {
        // The "old tests" part of the same file.
        run("monomono", Some(&SALT), 20, 20, 622_814_236);
        run("monomono", Some(&SALT), 40, 40, 1_186_751_819);
        run("password", None, 20, 20, -385_953_596);
        run("password", None, 40, 40, -1_921_654_064);
        run("monomono", None, 60, 60, -1_988_511_363);
        run("monomono", None, 80, 80, 158_880_373);
        run("monomono", None, 100, 100, 769_250_758);
    }

    #[test]
    fn mono_too_long() {
        // `TooLong ("SHA1", 20000, -1167918035)`: 1000 blocks are fine, one byte more is not.
        let mut pd = PasswordDeriveBytes::new(b"toolong", &[], 1);
        let key = pd.get_bytes(20_000).unwrap();
        assert_eq!(last4(&key), -1_167_918_035);
        assert_eq!(pd.get_bytes(1).unwrap_err(), KdfError::TooManyBytes);
    }

    #[test]
    fn zero_bytes_is_an_error_as_in_the_reference() {
        let mut pd = PasswordDeriveBytes::new(b"password", &[], 1);
        assert_eq!(pd.get_bytes(0).unwrap_err(), KdfError::ZeroLength);
    }

    /// `B0 = SHA1(base)`, `Bk = SHA1(decimal(k) || base)`, `base = SHA1^(n-1)(password || salt)`:
    /// computed here straight from the description in `online-protocol.md` 7, without the code
    /// under test, for a synthetic password and salt.
    fn blocks(password: &str, salt: &[u8], count: usize) -> Vec<[u8; 20]> {
        let mut base: [u8; 20] = Sha1::new()
            .chain_update(password.as_bytes())
            .chain_update(salt)
            .finalize()
            .into();
        for _ in 0..98 {
            base = Sha1::digest(base).into();
        }
        (0..count)
            .map(|k| {
                let mut h = Sha1::new();
                if k > 0 {
                    h.update(k.to_string().as_bytes());
                }
                h.update(base);
                h.finalize().into()
            })
            .collect()
    }

    #[test]
    fn get_bytes_32_then_16_reproduces_the_leftover_quirk() {
        let salt = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
        let b = blocks("synthetic-key", &salt, 3);
        let mut pd = PasswordDeriveBytes::for_password("synthetic-key", &salt);
        let key = pd.get_bytes(32).unwrap();
        let iv = pd.get_bytes(16).unwrap();
        // Key = B0 || B1[..12].
        assert_eq!(&key[..20], &b[0][..]);
        assert_eq!(&key[20..], &b[1][..12]);
        // IV = B0[8..16] (the bytes the reference copies from the wrong offset) || B2[..8].
        assert_eq!(&iv[..8], &b[0][8..16]);
        assert_eq!(&iv[8..], &b[2][..8]);
        // And not what a correct implementation would hand out.
        assert_ne!(&iv[..8], &b[1][12..20]);
    }

    #[test]
    fn leftover_that_covers_the_request_is_handed_out_in_order() {
        // 30 bytes leave 10 of the 2 blocks; a request for 4 and then 6 reads them in order.
        let b = blocks("p", &[], 2);
        let mut all = Vec::new();
        all.extend_from_slice(&b[0]);
        all.extend_from_slice(&b[1]);
        let mut pd = PasswordDeriveBytes::for_password("p", &[]);
        assert_eq!(pd.get_bytes(30).unwrap(), &all[..30]);
        assert_eq!(pd.get_bytes(4).unwrap(), &all[30..34]);
        assert_eq!(pd.get_bytes(6).unwrap(), &all[34..40]);
    }
}
