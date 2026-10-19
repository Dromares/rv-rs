//! A reader that checks its own length and CRC32 against the archive headers, shared by the zip
//! and 7z entry readers.

use std::io::{self, Read};

use crate::error::ArchiveError;

/// A decoded stream that fails with [`ArchiveError::SizeMismatch`] if it ends early and with
/// [`ArchiveError::CrcMismatch`] when its last byte is read and the CRC32 is wrong.
pub struct CheckedReader<'a> {
    inner: Box<dyn Read + 'a>,
    name: String,
    expected_crc: Option<u32>,
    expected_len: u64,
    produced: u64,
    hasher: crc32fast::Hasher,
    done: bool,
}

impl<'a> CheckedReader<'a> {
    pub(crate) fn new(
        inner: Box<dyn Read + 'a>,
        name: String,
        expected_len: u64,
        expected_crc: Option<u32>,
    ) -> CheckedReader<'a> {
        CheckedReader {
            inner,
            name,
            expected_crc,
            expected_len,
            produced: 0,
            hasher: crc32fast::Hasher::new(),
            done: false,
        }
    }

    /// Keep checking the length but not the CRC32, so a caller that wants to hash the data itself
    /// still gets every byte of an entry whose CRC is wrong. ROMVault's scanner does this: it
    /// hashes the entry, compares with the header afterwards and calls the entry corrupt
    /// (`FileScan.scanAFileInAnArchive`), keeping the CRC it computed.
    pub fn without_crc_check(mut self) -> CheckedReader<'a> {
        self.expected_crc = None;
        self
    }

    fn fail(e: ArchiveError) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, e)
    }

    /// Compare the CRC32 of everything read with the header's.
    fn verify(&self) -> io::Result<()> {
        if let Some(expected) = self.expected_crc {
            let actual = self.hasher.clone().finalize();
            if actual != expected {
                return Err(Self::fail(ArchiveError::CrcMismatch {
                    entry: self.name.clone(),
                    expected,
                    actual,
                }));
            }
        }
        Ok(())
    }
}

impl Read for CheckedReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        let remaining = self.expected_len - self.produced;
        if remaining == 0 {
            // An empty entry is complete before any data.
            self.done = true;
            self.verify()?;
            return Ok(0);
        }
        let want = buf.len().min(remaining.min(usize::MAX as u64) as usize);
        let n = self
            .inner
            .read(&mut buf[..want])
            .map_err(|e| Self::fail(io_to_decode(&self.name, e)))?;
        if n == 0 {
            self.done = true;
            return Err(Self::fail(ArchiveError::SizeMismatch {
                entry: self.name.clone(),
                expected: self.expected_len,
                actual: self.produced,
            }));
        }
        self.hasher.update(&buf[..n]);
        self.produced += n as u64;
        if self.produced == self.expected_len {
            self.done = true;
            self.verify()?;
        }
        Ok(n)
    }
}

/// Turn a decoder's `io::Error` into the entry-level error.
pub(crate) fn io_to_decode(name: &str, e: io::Error) -> ArchiveError {
    if e.get_ref().is_some_and(|inner| inner.is::<ArchiveError>()) {
        return into_archive_error(e);
    }
    match e.kind() {
        io::ErrorKind::UnexpectedEof => ArchiveError::Truncated {
            what: "compressed data",
        },
        _ => ArchiveError::Decode {
            entry: name.to_string(),
            detail: e.to_string(),
        },
    }
}

/// Recover the [`ArchiveError`] a [`CheckedReader`] wrapped in an `io::Error`.
pub fn into_archive_error(e: io::Error) -> ArchiveError {
    match e.get_ref().is_some_and(|inner| inner.is::<ArchiveError>()) {
        true => match e.into_inner().map(|inner| inner.downcast::<ArchiveError>()) {
            Some(Ok(a)) => *a,
            Some(Err(inner)) => ArchiveError::Io(io::Error::other(inner)),
            None => unreachable!("checked above"),
        },
        false => ArchiveError::from(e),
    }
}
