//! Unit tests for the zip64 paths, which need offsets and sizes above 4 GiB that no synthetic
//! file can really hold: a sparse reader supplies the zeros.

use std::io::{Cursor, Read, Seek, SeekFrom, Write};

use super::read::ZipArchive;
use super::write::{central_record_for_test, expand_file, zip64_fields};

/// A `Read + Seek` over a huge virtual file: listed regions hold bytes, the rest is zeros.
struct Sparse {
    len: u64,
    pos: u64,
    regions: Vec<(u64, Vec<u8>)>,
}

impl Read for Sparse {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(self.len.saturating_sub(self.pos) as usize);
        for (i, b) in buf[..n].iter_mut().enumerate() {
            let at = self.pos + i as u64;
            *b = self
                .regions
                .iter()
                .find(|(s, d)| at >= *s && at < *s + d.len() as u64)
                .map_or(0, |(s, d)| d[(at - *s) as usize]);
        }
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Sparse {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.pos = match p {
            SeekFrom::Start(n) => n,
            SeekFrom::End(d) => (self.len as i64 + d) as u64,
            SeekFrom::Current(d) => (self.pos as i64 + d) as u64,
        };
        Ok(self.pos)
    }
}

#[test]
fn zip64_field_layout_matches_the_c_sharp_writer() {
    // Local header: both sizes, 16 bytes, when either needs it (`ZipExtraFieldWrite.cs:23-37`).
    let z = zip64_fields(5_000_000_000, 0, 0, false);
    assert!(z.is_zip64);
    assert_eq!(
        (z.header_uncompressed, z.header_compressed),
        (0xffff_ffff, 0xffff_ffff)
    );
    let mut want = vec![1, 0, 16, 0];
    want.extend(5_000_000_000u64.to_le_bytes());
    want.extend(0u64.to_le_bytes());
    assert_eq!(z.extra, want);
    // Central directory: only the fields that overflow, in the order size, csize, offset.
    let z = zip64_fields(10, 5_000_000_000, 6_000_000_000, true);
    assert_eq!(
        (z.header_uncompressed, z.header_compressed, z.header_offset),
        (10, 0xffff_ffff, 0xffff_ffff)
    );
    let mut want = vec![1, 0, 16, 0];
    want.extend(5_000_000_000u64.to_le_bytes());
    want.extend(6_000_000_000u64.to_le_bytes());
    assert_eq!(z.extra, want);
    // Nothing overflows: no extra field.
    assert!(!zip64_fields(1, 2, 3, true).is_zip64);
}

#[test]
fn reads_a_zip_whose_entry_and_central_directory_sit_past_4_gib() {
    let local_at: u64 = 4_900_000_000;
    let name = b"big.bin";
    // Local header with the zip64 extra (`LocalFileHeaderWrite`).
    let l = zip64_fields(5_000_000_000, 4_950_000_000, local_at, false);
    let mut local = Vec::new();
    local.extend(0x0403_4b50u32.to_le_bytes());
    local.extend(45u16.to_le_bytes());
    local.extend(2u16.to_le_bytes());
    local.extend(8u16.to_le_bytes());
    local.extend(0xBC00u16.to_le_bytes());
    local.extend(0x2198u16.to_le_bytes());
    local.extend(0xdead_beefu32.to_le_bytes());
    local.extend(l.header_compressed.to_le_bytes());
    local.extend(l.header_uncompressed.to_le_bytes());
    local.extend((name.len() as u16).to_le_bytes());
    local.extend((l.extra.len() as u16).to_le_bytes());
    local.extend(name);
    local.extend(&l.extra);
    // Central directory entry, zip64 EOCD, locator, EOCD.
    let cd_start = local_at + local.len() as u64 + 4_950_000_000;
    let mut cd = Vec::new();
    central_record_for_test(
        name,
        0xdead_beef,
        4_950_000_000,
        5_000_000_000,
        local_at,
        &mut cd,
    );
    let mut tail = cd.clone();
    let rec_pos = cd_start + cd.len() as u64;
    tail.extend(0x0606_4b50u32.to_le_bytes());
    tail.extend(44u64.to_le_bytes());
    tail.extend(45u16.to_le_bytes());
    tail.extend(45u16.to_le_bytes());
    tail.extend([0u8; 8]);
    tail.extend(1u64.to_le_bytes());
    tail.extend(1u64.to_le_bytes());
    tail.extend((cd.len() as u64).to_le_bytes());
    tail.extend(cd_start.to_le_bytes());
    tail.extend(0x0706_4b50u32.to_le_bytes());
    tail.extend(0u32.to_le_bytes());
    tail.extend(rec_pos.to_le_bytes());
    tail.extend(1u32.to_le_bytes());
    tail.extend(0x0605_4b50u32.to_le_bytes());
    tail.extend([0, 0, 0, 0]);
    tail.extend(1u16.to_le_bytes());
    tail.extend(1u16.to_le_bytes());
    tail.extend(0xffff_ffffu32.to_le_bytes());
    tail.extend(0xffff_ffffu32.to_le_bytes());
    tail.extend(0u16.to_le_bytes());
    let len = cd_start + tail.len() as u64;
    let r = Sparse {
        len,
        pos: 0,
        regions: vec![(local_at, local.clone()), (cd_start, tail)],
    };
    let zip = ZipArchive::open(r).unwrap();
    assert!(zip.is_zip64());
    let e = &zip.entries()[0];
    assert_eq!(e.name, "big.bin");
    assert_eq!(e.uncompressed_size, 5_000_000_000);
    assert_eq!(e.compressed_size, 4_950_000_000);
    assert_eq!(e.local_header_offset, local_at);
    assert_eq!(e.data_offset, local_at + local.len() as u64);
    assert!(e.is_zip64);
}

#[test]
fn zip64_is_required_when_the_eocd_says_so() {
    let mut z = Vec::new();
    z.extend(0x0605_4b50u32.to_le_bytes());
    z.extend([0, 0, 0, 0]);
    z.extend(1u16.to_le_bytes());
    z.extend(1u16.to_le_bytes());
    z.extend(0xffff_ffffu32.to_le_bytes());
    z.extend(0xffff_ffffu32.to_le_bytes());
    z.extend(0u16.to_le_bytes());
    assert!(matches!(
        ZipArchive::open(Cursor::new(z)),
        Err(crate::ArchiveError::Zip64Required)
    ));
}

#[test]
fn expand_file_moves_a_range_up_in_place() {
    let data: Vec<u8> = (0..200u8).collect();
    let mut c = Cursor::new(data.clone());
    expand_file(&mut c, 10, 150, 20).unwrap();
    let out = c.into_inner();
    assert_eq!(&out[..10], &data[..10]);
    assert_eq!(&out[30..170], &data[10..150]);
    // And across more than one 40 KiB buffer.
    let big: Vec<u8> = (0..130_000u32).map(|i| (i % 251) as u8).collect();
    let mut c = Cursor::new(big.clone());
    expand_file(&mut c, 5, 129_000, 20).unwrap();
    let out = c.into_inner();
    assert_eq!(&out[25..129_020], &big[5..129_000]);
    let _ = Write::flush(&mut Cursor::new(Vec::new()));
}
