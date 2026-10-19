//! One pass over a stream: main hashes of the whole stream, alt hashes of the stream
//! without its copier header (`FileScan.CheckSumRead`, `FileScan.cs:228-531`).

use std::io::{self, Read};

use rv_core::HeaderFileType;

use crate::hash::{Digests, HashKinds, HashSet};
use crate::header::detect;
use crate::MAX_HEADER_PROBE;

const BUFFER_SIZE: usize = 4096 * 1024; // `Buffersize`, `FileScan.cs:214-226`

/// What to compute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanOptions {
    /// `fullScan` of `CheckSumRead` (Level 2 and 3): hash everything. When false and no header
    /// is found nothing is hashed; when false and a header is found only the alt CRC is
    /// (`FileScan.cs:261-269,295-304`). Level 1 is the only caller that passes false.
    pub full_scan: bool,
    /// Also compute SHA256 (`scanSHA256`). 3.8.0 never does, the code is there
    /// (`FileScan.cs:275,303`).
    pub sha256: bool,
}

impl Default for ScanOptions {
    /// Level-2 behaviour with SHA256 on.
    fn default() -> Self {
        ScanOptions {
            full_scan: true,
            sha256: true,
        }
    }
}

/// Hashes of the stream after the copier header (`AltCRC`, `AltMD5`, ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AltDigests {
    /// Stream length minus header length (`FileScan.cs:291`).
    pub size: u64,
    pub digests: Digests,
}

/// The result of [`scan`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanResult {
    /// Bytes read. With `full_scan` off and no header this is only the probe.
    pub size: u64,
    pub header_type: HeaderFileType,
    /// Hashes of the whole stream; all `None` when nothing was hashed.
    pub digests: Digests,
    /// Present when a copier header with a non-zero offset was found.
    pub alt: Option<AltDigests>,
}

/// Read until `buf` is full or the stream ends. `CheckSumRead` assumes `Stream.Read` returns
/// the requested count (`FileScan.cs:256,366,387`); a short read there corrupts the hashes.
fn read_full<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match r.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(got)
}

/// Hash `r` in one pass.
pub fn scan<R: Read>(r: &mut R, opts: ScanOptions) -> io::Result<ScanResult> {
    let mut probe = [0u8; MAX_HEADER_PROBE];
    let probe_len = read_full(r, &mut probe)?;
    let probe = &probe[..probe_len];

    let (header_type, header_len) = detect(probe);
    let kinds = HashKinds {
        crc: true,
        md5: true,
        sha1: true,
        sha256: opts.sha256,
    };

    if header_type == HeaderFileType::NOTHING || header_len == 0 {
        // `FileScan.cs:261-286`: no alt hashes. Level 1 stops here.
        if !opts.full_scan {
            return Ok(ScanResult {
                size: probe_len as u64,
                header_type,
                digests: Digests::default(),
                alt: None,
            });
        }
        let mut main = HashSet::new(kinds);
        main.update(probe);
        let size = feed(r, probe_len as u64, &mut [&mut main])?;
        return Ok(ScanResult {
            size,
            header_type,
            digests: main.finish(),
            alt: None,
        });
    }

    // A header was found. Main hashers (full scan only) see header and body, alt hashers see
    // the body only; the alt CRC exists even for a quick scan (`FileScan.cs:295-304`).
    let alt_kinds = if opts.full_scan {
        kinds
    } else {
        HashKinds {
            crc: true,
            md5: false,
            sha1: false,
            sha256: false,
        }
    };
    let mut main = opts.full_scan.then(|| HashSet::new(kinds));
    let mut alt = HashSet::new(alt_kinds);
    // `detect` only matches when the probe is longer than the header.
    if let Some(m) = &mut main {
        m.update(probe);
    }
    alt.update(&probe[header_len..]);
    let mut hashers: Vec<&mut HashSet> = main.iter_mut().chain(std::iter::once(&mut alt)).collect();
    let size = feed(r, probe_len as u64, &mut hashers)?;
    Ok(ScanResult {
        size,
        header_type,
        digests: main.map(HashSet::finish).unwrap_or_default(),
        alt: Some(AltDigests {
            size: size - header_len as u64,
            digests: alt.finish(),
        }),
    })
}

/// Stream the rest of `r` into every hasher; return the total length given that `already`
/// bytes were consumed earlier.
fn feed<R: Read>(r: &mut R, already: u64, hashers: &mut [&mut HashSet]) -> io::Result<u64> {
    let mut buf = vec![0u8; BUFFER_SIZE];
    let mut total = already;
    loop {
        let n = read_full(r, &mut buf)?;
        if n == 0 {
            return Ok(total);
        }
        for h in hashers.iter_mut() {
            h.update(&buf[..n]);
        }
        total += n as u64;
    }
}
