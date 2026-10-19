//! Hunk decoders (`CHD/CHDSharpLib/CHDReaders.cs`): zlib, zstd, lzma, huff, flac and the CD
//! variants cdzl, cdzs, cdlz and cdfl. `avhu` (LaserDisc audio/video) is not supported.

use std::io::Read;

use super::cdrom::ecc_generate;
use super::flac;
use super::header::{codec, codec_name};
use super::huffman::{BitStream, HuffmanDecoder};
use crate::error::{ChdError, Result};

const CD_MAX_SECTOR_DATA: usize = 2352;
const CD_MAX_SUBCODE_DATA: usize = 96;
const CD_FRAME_SIZE: usize = CD_MAX_SECTOR_DATA + CD_MAX_SUBCODE_DATA;
/// `s_cd_sync_header` (`CHDReaders.cs:158`).
const CD_SYNC_HEADER: [u8; 12] = [
    0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,
];

fn fail(what: impl Into<String>) -> crate::error::ArchiveError {
    ChdError::Decompression(what.into()).into()
}

/// Read exactly `out.len()` bytes; a stream that ends earlier is `CHDERR_INVALID_DATA`.
fn read_full(mut r: impl Read, out: &mut [u8], what: &str) -> Result<()> {
    let mut done = 0;
    while done < out.len() {
        match r.read(&mut out[done..]) {
            Ok(0) => {
                return Err(ChdError::InvalidData(format!("{what}: the stream ends early")).into())
            }
            Ok(n) => done += n,
            Err(e) => return Err(fail(format!("{what}: {e}"))),
        }
    }
    Ok(())
}

fn zlib(input: &[u8], out: &mut [u8]) -> Result<()> {
    read_full(flate2::read::DeflateDecoder::new(input), out, "zlib")
}

fn zstd(input: &[u8], out: &mut [u8]) -> Result<()> {
    let d = ruzstd::decoding::StreamingDecoder::new(input)
        .map_err(|e| fail(format!("zstd frame header: {e}")))?;
    read_full(d, out, "zstd")
}

/// CHD's LZMA streams have no header; the decoder is told lc=3 lp=0 pb=2 and a dictionary the
/// size of the output (`CHDReaders.cs:65-81`, `docs/codecs.md` section 12.3).
fn lzma(input: &[u8], out: &mut [u8]) -> Result<()> {
    let dict = u32::try_from(out.len()).map_err(|_| fail("lzma: the hunk is too large"))?;
    let mut props = [0u8; 5];
    props[0] = (2 * 5) * 9 + 3; // (pb * 5 + lp) * 9 + lc
    props[1..].copy_from_slice(&dict.to_le_bytes());
    let r = crate::zip::raw_lzma1(input, &props, "CHD hunk")?;
    read_full(r, out, "lzma")
}

fn huffman(input: &[u8], out: &mut [u8]) -> Result<()> {
    let mut bs = BitStream::new(input);
    let mut d = HuffmanDecoder::new(256, 16);
    d.import_tree_huffman(&mut bs)
        .map_err(|e| ChdError::InvalidData(format!("huff tree: {e:?}")))?;
    for b in out.iter_mut() {
        *b = d.decode_one(&mut bs) as u8;
    }
    if bs.overflow() {
        return Err(ChdError::InvalidData("huff: the data ends early".to_string()).into());
    }
    Ok(())
}

/// Decode FLAC frames from `input` until `out` is full, writing the 16-bit samples big-endian
/// (`swap`) or little-endian. Returns how many input bytes the frames used.
fn flac_frames(input: &[u8], out: &mut [u8], big_endian: bool) -> Result<usize> {
    let mut pos = 0;
    let mut dst = 0;
    while dst < out.len() {
        let f = flac::decode_frame(&input[pos..])?;
        let bytes = f.samples.len() * 2;
        if dst + bytes > out.len() {
            return Err(fail("FLAC: a frame runs past the end of the hunk"));
        }
        for (i, s) in f.samples.iter().enumerate() {
            let b = if big_endian {
                s.to_be_bytes()
            } else {
                s.to_le_bytes()
            };
            out[dst + 2 * i..dst + 2 * i + 2].copy_from_slice(&b);
        }
        dst += bytes;
        pos += f.bytes_used;
    }
    Ok(pos)
}

/// The `flac` codec: a leading `L` or `B` byte, then frames (`CHDReaders.cs:120-159`). The
/// samples are decoded little-endian and byte-swapped when the byte is `B`.
fn flac_codec(input: &[u8], out: &mut [u8]) -> Result<()> {
    let (&tag, rest) = input
        .split_first()
        .ok_or_else(|| ChdError::InvalidData("flac: empty hunk".to_string()))?;
    flac_frames(rest, out, tag == b'B')?;
    Ok(())
}

/// How a CD hunk's sector and subcode areas are compressed.
#[derive(Clone, Copy)]
enum CdKind {
    Zlib,
    Zstd,
    Lzma,
    Flac,
}

/// The CD codecs (`CHDReaders.cdzlib`, `cdzstd`, `cdlzma`, `cdflac`, `:166-320`). A hunk is a
/// number of 2448-byte frames (2352 sector bytes then 96 subcode bytes). The input is, for the
/// non-FLAC kinds, an ECC bitmap, the length of the sector stream, the sector stream, and the
/// subcode stream; for `cdfl`, FLAC frames for all sectors followed by the subcode stream.
fn cd(kind: CdKind, input: &[u8], out: &mut [u8]) -> Result<()> {
    if out.is_empty() || !out.len().is_multiple_of(CD_FRAME_SIZE) {
        return Err(
            ChdError::InvalidData("CD hunk size is not a multiple of 2448".to_string()).into(),
        );
    }
    let frames = out.len() / CD_FRAME_SIZE;
    let mut sector = vec![0u8; frames * CD_MAX_SECTOR_DATA];
    let mut subcode = vec![0u8; frames * CD_MAX_SUBCODE_DATA];
    let mut ecc_bitmap: &[u8] = &[];

    if let CdKind::Flac = kind {
        let used = flac_frames(input, &mut sector, true)?;
        zlib(&input[used..], &mut subcode)?;
    } else {
        let complen_bytes = if out.len() < 65536 { 2 } else { 3 };
        let ecc_bytes = frames.div_ceil(8);
        let header = ecc_bytes + complen_bytes;
        if input.len() < header {
            return Err(ChdError::InvalidData("CD hunk header is cut short".to_string()).into());
        }
        let mut complen_base = ((input[ecc_bytes] as usize) << 8) | input[ecc_bytes + 1] as usize;
        if complen_bytes > 2 {
            complen_base = (complen_base << 8) | input[ecc_bytes + 2] as usize;
        }
        if header + complen_base > input.len() {
            return Err(ChdError::InvalidData(
                "CD hunk sector stream runs past the hunk".to_string(),
            )
            .into());
        }
        let (base, sub) = input[header..].split_at(complen_base);
        match kind {
            CdKind::Zlib => {
                zlib(base, &mut sector)?;
                zlib(sub, &mut subcode)?;
            }
            CdKind::Zstd => {
                zstd(base, &mut sector)?;
                zstd(sub, &mut subcode)?;
            }
            CdKind::Lzma => {
                lzma(base, &mut sector)?;
                zlib(sub, &mut subcode)?;
            }
            CdKind::Flac => unreachable!("handled above"),
        }
        ecc_bitmap = &input[..ecc_bytes];
    }

    for f in 0..frames {
        let start = f * CD_FRAME_SIZE;
        out[start..start + CD_MAX_SECTOR_DATA]
            .copy_from_slice(&sector[f * CD_MAX_SECTOR_DATA..(f + 1) * CD_MAX_SECTOR_DATA]);
        out[start + CD_MAX_SECTOR_DATA..start + CD_FRAME_SIZE]
            .copy_from_slice(&subcode[f * CD_MAX_SUBCODE_DATA..(f + 1) * CD_MAX_SUBCODE_DATA]);
        // The sync header and the P/Q parity were left out of the stream and are regenerated.
        if !ecc_bitmap.is_empty() && ecc_bitmap[f / 8] & (1 << (f % 8)) != 0 {
            out[start..start + CD_SYNC_HEADER.len()].copy_from_slice(&CD_SYNC_HEADER);
            ecc_generate(&mut out[start..start + CD_MAX_SECTOR_DATA]);
        }
    }
    Ok(())
}

/// Decode one stored hunk with codec `c` (`CHDBlockRead.GetReaderFromCodec`).
pub(super) fn decode(c: u32, input: &[u8], out: &mut [u8]) -> Result<()> {
    match c {
        codec::ZLIB => zlib(input, out),
        codec::ZSTD => zstd(input, out),
        codec::LZMA => lzma(input, out),
        codec::HUFF => huffman(input, out),
        codec::FLAC => flac_codec(input, out),
        codec::CD_ZLIB => cd(CdKind::Zlib, input, out),
        codec::CD_ZSTD => cd(CdKind::Zstd, input, out),
        codec::CD_LZMA => cd(CdKind::Lzma, input, out),
        codec::CD_FLAC => cd(CdKind::Flac, input, out),
        other => Err(ChdError::UnsupportedCodec(codec_name(other)).into()),
    }
}

/// True for the codecs [`decode`] implements.
pub fn is_supported(c: u32) -> bool {
    matches!(
        c,
        codec::ZLIB
            | codec::ZSTD
            | codec::LZMA
            | codec::HUFF
            | codec::FLAC
            | codec::CD_ZLIB
            | codec::CD_ZSTD
            | codec::CD_LZMA
            | codec::CD_FLAC
    )
}
