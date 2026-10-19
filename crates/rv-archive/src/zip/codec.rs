//! Decompressor dispatch for zip entries (`ZipFileData.OpenStream`, `ZipFileData.cs:458-545`).
//!
//! Methods ROMVault decodes and this crate does too: Stored, Deflate, Deflate64, BZip2, LZMA,
//! Zstandard (93 and the deprecated 20) and PPMd. Shrink, Reduce and Implode (methods 1-6) are
//! known to ROMVault but no permitted crate decodes them, so they return
//! [`ArchiveError::UnsupportedCompression`] when an entry is opened. See `docs/codecs.md`
//! sections 7-9 of the decompile notes for the algorithms if someone ports them.

use std::io::{self, BufReader, Read};

use super::method;
use crate::error::{ArchiveError, Result};

/// A method name for error messages.
pub(crate) fn method_name(m: u16) -> String {
    let name = match m {
        method::STORED => "Stored",
        method::SHRUNK => "Shrink",
        method::REDUCED1..=method::REDUCED4 => "Reduce",
        method::IMPLODED => "Implode",
        method::DEFLATED => "Deflate",
        method::DEFLATE64 => "Deflate64",
        method::BZIP2 => "BZip2",
        method::LZMA => "LZMA",
        method::ZSTANDARD_DEPRECATED => "Zstandard (deprecated 20)",
        method::ZSTANDARD => "Zstandard",
        method::PPMD => "PPMd",
        10 => "PKWARE DCL Implode",
        99 => "AES encryption",
        _ => "",
    };
    if name.is_empty() {
        format!("method {m}")
    } else {
        format!("{name} (method {m})")
    }
}

/// The methods `Zip.ValidateFileHeaders` lets through (`ZipOpenRead.cs:235-255`). Others make the
/// whole archive fail to open in ROMVault, and here.
pub(crate) fn romvault_reads_method(m: u16) -> bool {
    matches!(
        m,
        method::STORED
            | method::SHRUNK
            | method::REDUCED1
            | method::REDUCED2
            | method::REDUCED3
            | method::REDUCED4
            | method::IMPLODED
            | method::DEFLATED
            | method::DEFLATE64
            | method::BZIP2
            | method::LZMA
            | method::ZSTANDARD_DEPRECATED
            | method::ZSTANDARD
            | method::PPMD
    )
}

fn decode_err(name: &str, what: &str, e: impl std::fmt::Display) -> ArchiveError {
    ArchiveError::Decode {
        entry: name.to_string(),
        detail: format!("{what}: {e}"),
    }
}

/// Wrap `raw` (the entry's compressed bytes, already limited to the compressed size) in the
/// decoder for `method`.
pub(crate) fn open_decoder<'a, R: Read + 'a>(
    method: u16,
    mut raw: R,
    name: &str,
) -> Result<Box<dyn Read + 'a>> {
    Ok(match method {
        method::STORED => Box::new(raw),
        method::DEFLATED => Box::new(flate2::read::DeflateDecoder::new(raw)),
        method::DEFLATE64 => Box::new(deflate64::Deflate64Decoder::new(raw)),
        method::BZIP2 => Box::new(bzip2_rs::DecoderReader::new(raw)),
        method::LZMA => {
            // Two version bytes, a little-endian properties size, the properties, then the raw
            // stream (`ZipFileData.cs:515-525`).
            let mut head = [0u8; 4];
            raw.read_exact(&mut head)
                .map_err(|e| decode_err(name, "LZMA header", e))?;
            let props_len = u16::from_le_bytes([head[2], head[3]]) as usize;
            let mut props = vec![0u8; props_len];
            raw.read_exact(&mut props)
                .map_err(|e| decode_err(name, "LZMA properties", e))?;
            raw_lzma1(raw, &props, name)?
        }
        method::ZSTANDARD | method::ZSTANDARD_DEPRECATED => {
            let d = ruzstd::decoding::StreamingDecoder::new(raw)
                .map_err(|e| decode_err(name, "zstd frame header", e))?;
            Box::new(d)
        }
        method::PPMD => {
            // Two property bytes: order, allocator size in MiB and restore method
            // (`ZipFileData.cs:533-541`, `PpmdProperties.cs:42-45`).
            let mut p = [0u8; 2];
            raw.read_exact(&mut p)
                .map_err(|e| decode_err(name, "PPMd header", e))?;
            let props = u16::from_le_bytes(p);
            let order = (props & 0x0f) as u32 + 1;
            let mem = ((((props >> 4) & 0xff) as u32) + 1) << 20;
            let restore = ppmd_rust::RestoreMethod::from(props >> 12);
            let d = ppmd_rust::Ppmd8Decoder::new(raw, order, mem, restore)
                .map_err(|e| decode_err(name, "PPMd parameters", e))?;
            Box::new(d)
        }
        other => {
            return Err(ArchiveError::UnsupportedCompression {
                method: method_name(other),
            })
        }
    })
}

/// A raw LZMA1 stream with the 5 property bytes given separately.
pub(crate) fn raw_lzma1<'a, R: Read + 'a>(
    raw: R,
    props: &[u8],
    name: &str,
) -> Result<Box<dyn Read + 'a>> {
    let mut filters = liblzma::stream::Filters::new();
    filters
        .lzma1_properties(props)
        .map_err(|e| decode_err(name, "LZMA properties", e))?;
    let stream = liblzma::stream::Stream::new_raw_decoder(&filters)
        .map_err(|e| decode_err(name, "LZMA decoder", e))?;
    Ok(Box::new(liblzma::read::XzDecoder::new_stream(
        BufReader::new(raw),
        stream,
    )))
}

/// Turn a decoder's `io::Error` into the entry-level error.
pub(crate) fn io_to_decode(name: &str, e: io::Error) -> ArchiveError {
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
