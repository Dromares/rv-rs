//! Building the decoder for one 7z folder (`SevenZipReadStream.cs:25-291`, `Coder.cs:88-125`).
//!
//! Supported: folders that are a linear chain with one packed stream. The first coder (in decode
//! order) is Copy, LZMA, LZMA2, PPMd (var.H), Deflate, BZip2 or Zstandard; BCJ x86/ARM/ARMT/PPC/
//! SPARC/IA64 and Delta filters may follow it (`filters.rs`). Not supported and returned as
//! [`ArchiveError::UnsupportedCompression`]: BCJ2 (four input streams), ARM64 (ROMVault leaves it
//! out too, `SevenZipReadStream.cs:233`), and AES.

use std::io::{self, Read};

use super::filters::{self, FilterReader};
use super::header::{Coder, Folder};
use crate::error::{ArchiveError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Copy,
    Lzma,
    Lzma2,
    Ppmd7,
    Deflate,
    BZip2,
    Zstd,
    Bcj,
    Ppc,
    Ia64,
    Arm,
    ArmThumb,
    Sparc,
    Delta,
    Bcj2,
    Aes,
    Unknown,
}

/// `Coder.Read`'s method-id table (`Coder.cs:88-125`), plus AES (`06 F1 07 01`) so it is not
/// reported as unknown.
fn kind_of(method: &[u8]) -> Kind {
    match method {
        [0x00] => Kind::Copy,
        [0x03, 0x01, 0x01] => Kind::Lzma,
        [0x21] => Kind::Lzma2,
        [0x03, 0x04, 0x01] => Kind::Ppmd7,
        [0x04, 0x01, 0x08] => Kind::Deflate,
        [0x04, 0x02, 0x02] => Kind::BZip2,
        [0x04, 0xf7, 0x11, 0x01] => Kind::Zstd,
        [0x03, 0x03, 0x01, 0x03] => Kind::Bcj,
        [0x03, 0x03, 0x01, 0x1b] => Kind::Bcj2,
        [0x03, 0x03, 0x07, 0x01] => Kind::ArmThumb,
        [0x03, 0x03, 0x05, 0x01] => Kind::Arm,
        [0x03, 0x03, 0x02, 0x05] => Kind::Ppc,
        [0x03, 0x03, 0x08, 0x05] => Kind::Sparc,
        [0x03, 0x03, 0x04, 0x01] => Kind::Ia64,
        [0x03] => Kind::Delta,
        [0x06, 0xf1, 0x07, 0x01] => Kind::Aes,
        _ => Kind::Unknown,
    }
}

fn kind_name(k: Kind, method: &[u8]) -> String {
    let name = match k {
        Kind::Copy => "Copy",
        Kind::Lzma => "LZMA",
        Kind::Lzma2 => "LZMA2",
        Kind::Ppmd7 => "PPMd",
        Kind::Deflate => "Deflate",
        Kind::BZip2 => "BZip2",
        Kind::Zstd => "ZSTD",
        Kind::Bcj => "BCJ",
        Kind::Ppc => "PPC",
        Kind::Ia64 => "IA64",
        Kind::Arm => "ARM",
        Kind::ArmThumb => "ARMT",
        Kind::Sparc => "SPARC",
        Kind::Delta => "Delta",
        Kind::Bcj2 => "BCJ2",
        Kind::Aes => "AES",
        Kind::Unknown => {
            let hex: String = method.iter().map(|b| format!("{b:02X}")).collect();
            return format!("7z method {hex}");
        }
    };
    name.to_string()
}

/// The method names of a folder's coders, in header order.
pub(super) fn coder_names(folder: &Folder) -> Vec<String> {
    folder
        .coders
        .iter()
        .map(|c| kind_name(kind_of(&c.method), &c.method))
        .collect()
}

fn unsupported(c: &Coder) -> ArchiveError {
    ArchiveError::UnsupportedCompression {
        method: kind_name(kind_of(&c.method), &c.method),
    }
}

fn decode_err(what: &str, e: impl std::fmt::Display) -> ArchiveError {
    ArchiveError::Decode {
        entry: "7z folder".to_string(),
        detail: format!("{what}: {e}"),
    }
}

/// The coders in decode order (from the packed stream towards the result), or an error when the
/// folder is not a linear single-input chain.
fn decode_order(folder: &Folder) -> Result<Vec<(usize, &Coder)>> {
    if folder.packed_streams.len() != 1 {
        return Err(ArchiveError::UnsupportedCompression {
            method: "a 7z folder with several packed streams".to_string(),
        });
    }
    for c in &folder.coders {
        if c.num_in_streams != 1 || c.num_out_streams != 1 {
            return Err(unsupported(c));
        }
    }
    // Coder i has in stream i and out stream i. Walk from the result back to the packed stream.
    let mut order = Vec::new();
    let mut out = folder
        .main_out_stream()
        .ok_or_else(|| ArchiveError::corrupt("7z folder", "no unbound output stream"))?;
    loop {
        order.push((out as usize, &folder.coders[out as usize]));
        // The coder's input is bound to another coder's output, or is the packed stream.
        match folder.bind_pairs.iter().find(|b| b.in_index == out) {
            Some(b) => out = b.out_index,
            None => break,
        }
        if order.len() > folder.coders.len() {
            return Err(ArchiveError::corrupt("7z folder", "the bind pairs loop"));
        }
    }
    if order.len() != folder.coders.len() {
        return Err(ArchiveError::corrupt("7z folder", "unused coders"));
    }
    // `order` runs from the result to the packed stream; decode order is the reverse.
    order.reverse();
    Ok(order)
}

/// Wrap `packed` (limited to the folder's packed size) in the decoders of `folder` and return a
/// reader of the folder's result, at most `unpack_size` bytes long.
pub(super) fn open_folder<'a, R: Read + 'a>(
    folder: &Folder,
    packed: R,
) -> Result<Box<dyn Read + 'a>> {
    let order = decode_order(folder)?;
    let kinds: Vec<Kind> = order.iter().map(|(_, c)| kind_of(&c.method)).collect();
    let first = order[0].1;
    // Every coder's output is cut at its stated size: LZMA1 here has no end marker, so the
    // decoder would otherwise fail when a filter reads on to the end.
    let size_of = |idx: usize| folder.unpack_sizes.get(idx).copied().unwrap_or(0);
    let mut reader: Box<dyn Read + 'a> = match kinds[0] {
        Kind::Lzma => {
            let mut filters = liblzma::stream::Filters::new();
            filters
                .lzma1_properties(&first.properties)
                .map_err(|e| decode_err("LZMA properties", e))?;
            lzma_reader(packed, &filters)?
        }
        Kind::Lzma2 => {
            let mut filters = liblzma::stream::Filters::new();
            filters
                .lzma2_properties(&first.properties)
                .map_err(|e| decode_err("LZMA2 properties", e))?;
            lzma_reader(packed, &filters)?
        }
        Kind::Copy => Box::new(packed),
        Kind::Ppmd7 => {
            let p = &first.properties;
            if p.len() != 5 {
                return Err(decode_err("PPMd properties", "expected 5 bytes"));
            }
            let mem = u32::from_le_bytes([p[1], p[2], p[3], p[4]]);
            Box::new(
                ppmd_rust::Ppmd7Decoder::new(packed, p[0] as u32, mem)
                    .map_err(|e| decode_err("PPMd parameters", e))?,
            )
        }
        Kind::Deflate => Box::new(flate2::read::DeflateDecoder::new(packed)),
        Kind::BZip2 => Box::new(bzip2_rs::DecoderReader::new(packed)),
        Kind::Zstd => Box::new(
            ruzstd::decoding::StreamingDecoder::new(packed)
                .map_err(|e| decode_err("zstd frame header", e))?,
        ),
        _ => return Err(unsupported(first)),
    };
    // Filters run after the entropy coder, in decode order.
    reader = Box::new(reader.take(size_of(order[0].0)));
    for (&(idx, c), &k) in order.iter().zip(&kinds).skip(1) {
        reader = match k {
            Kind::Bcj => {
                let start = match c.properties.as_slice() {
                    [] => 0,
                    [a, b, c2, d] => u32::from_le_bytes([*a, *b, *c2, *d]),
                    _ => return Err(decode_err("BCJ properties", "expected 0 or 4 bytes")),
                };
                Box::new(FilterReader::new(reader, filters::X86::new(start)))
            }
            Kind::Arm => Box::new(FilterReader::new(reader, filters::Arm::new())),
            Kind::ArmThumb => Box::new(FilterReader::new(reader, filters::ArmThumb::new())),
            Kind::Ppc => Box::new(FilterReader::new(reader, filters::Ppc::new())),
            Kind::Sparc => Box::new(FilterReader::new(reader, filters::Sparc::new())),
            Kind::Ia64 => Box::new(FilterReader::new(reader, filters::Ia64::new())),
            Kind::Delta => {
                let p = c.properties.first().copied().unwrap_or(0);
                Box::new(FilterReader::new(reader, filters::Delta::new(p)))
            }
            _ => return Err(unsupported(c)),
        };
        reader = Box::new(reader.take(size_of(idx)));
    }
    Ok(reader)
}

/// LZMA or LZMA2 through liblzma's raw decoder (`SevenZipReadStream.cs:205-210`).
fn lzma_reader<'a, R: Read + 'a>(
    packed: R,
    filters: &liblzma::stream::Filters,
) -> Result<Box<dyn Read + 'a>> {
    let stream = liblzma::stream::Stream::new_raw_decoder(filters)
        .map_err(|e| decode_err("LZMA decoder", e))?;
    Ok(Box::new(liblzma::read::XzDecoder::new_stream(
        io::BufReader::new(packed),
        stream,
    )))
}
