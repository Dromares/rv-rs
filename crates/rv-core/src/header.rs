//! Copier-header detection (`FileScanner/FileHeaders.cs`).
//!
//! A detector matches when the probe is longer than its `header_length` and its magic bytes
//! are at `magic_offset`. The first match in table order wins. `file_offset` is the number of
//! header bytes that the "alt" (header-stripped) hashes skip. The archive and CHD entries have
//! `file_offset` 0: they set the header type but produce no alt hashes (`FileScan.cs:261`).

use crate::enums::HeaderFileType;

/// One row of the detector table (`FileHeaders.cs:151-168`).
#[derive(Clone, Copy, Debug)]
pub struct Detector {
    pub file_type: HeaderFileType,
    /// The probe must be strictly longer than this (`FileHeaders.cs:119`).
    pub header_length: usize,
    /// Header bytes skipped by the alt hashes.
    pub file_offset: usize,
    /// Name that a DAT's `<clrmamepro header="...">` is matched against, lower case; empty for
    /// archive types.
    pub header_id: &'static str,
    pub magic_offset: usize,
    pub magic: &'static [u8],
}

const fn d(
    file_type: HeaderFileType,
    header_length: usize,
    file_offset: usize,
    header_id: &'static str,
    magic_offset: usize,
    magic: &'static [u8],
) -> Detector {
    Detector {
        file_type,
        header_length,
        file_offset,
        header_id,
        magic_offset,
        magic,
    }
}

const A7800_MAGIC1: &[u8] = b"ATARI7800";
const A7800_MAGIC2: &[u8] = b"ACTUAL CART DATA STARTS HERE";
const NES_MAGIC: &[u8] = &[0x4E, 0x45, 0x53, 0x1A];
const LYNX_MAGIC: &[u8] = b"LYNX";
const LYNX_MAGIC2: &[u8] = b"BS9";

/// `FileHeaderReader.Detectors` in source order (`FileHeaders.cs:12-50`).
pub static DETECTORS: &[Detector] = &[
    // Standard archive types (`FileHeaders.cs:15-18`)
    d(HeaderFileType::ZIP, 22, 0, "", 0, &[0x50, 0x4b, 0x03, 0x04]),
    d(HeaderFileType::GZ, 18, 0, "", 0, &[0x1f, 0x8b, 0x08]),
    d(
        HeaderFileType::SEVEN_ZIP,
        6,
        0,
        "",
        0,
        &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C],
    ),
    d(
        HeaderFileType::RAR,
        6,
        0,
        "",
        0,
        &[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07],
    ),
    // CHD (`FileHeaders.cs:21`)
    d(HeaderFileType::CHD, 76, 0, "", 0, b"MComprHD"),
    // Headered files (`FileHeaders.cs:24-49`)
    d(
        HeaderFileType::A7800,
        128,
        128,
        "a7800.xml",
        1,
        A7800_MAGIC1,
    ),
    d(
        HeaderFileType::A7800,
        128,
        128,
        "a7800.xml",
        100,
        A7800_MAGIC2,
    ),
    d(
        HeaderFileType::A7800,
        128,
        128,
        "no-intro_a7800.xml",
        1,
        A7800_MAGIC1,
    ),
    d(
        HeaderFileType::A7800,
        128,
        128,
        "no-intro_a7800.xml",
        100,
        A7800_MAGIC2,
    ),
    d(
        HeaderFileType::FDS,
        16,
        16,
        "fds.xml",
        0,
        &[
            0x46, 0x44, 0x53, 0x1A, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    ),
    d(
        HeaderFileType::FDS,
        16,
        16,
        "fds.xml",
        0,
        &[
            0x46, 0x44, 0x53, 0x1A, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    ),
    d(
        HeaderFileType::FDS,
        16,
        16,
        "fds.xml",
        0,
        &[
            0x46, 0x44, 0x53, 0x1A, 0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    ),
    d(
        HeaderFileType::FDS,
        16,
        16,
        "fds.xml",
        0,
        &[
            0x46, 0x44, 0x53, 0x1A, 0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    ),
    d(
        HeaderFileType::FDS,
        16,
        16,
        "no-intro_fds.xml",
        0,
        &[0x46, 0x44, 0x53],
    ),
    d(HeaderFileType::LYNX, 64, 64, "lynx.xml", 0, LYNX_MAGIC),
    d(HeaderFileType::LYNX, 64, 64, "lynx.xml", 6, LYNX_MAGIC2),
    d(
        HeaderFileType::LYNX,
        64,
        64,
        "no-intro_lnx.xml",
        0,
        LYNX_MAGIC,
    ),
    d(
        HeaderFileType::LYNX,
        64,
        64,
        "no-intro_lnx.xml",
        6,
        LYNX_MAGIC2,
    ),
    d(
        HeaderFileType::NES,
        16,
        16,
        "no-intro_nes.xml",
        0,
        NES_MAGIC,
    ),
    d(HeaderFileType::NES, 16, 16, "nongoodnes.xml", 0, NES_MAGIC),
    d(HeaderFileType::NES, 16, 16, "nes.xml", 0, NES_MAGIC),
    d(HeaderFileType::NES, 16, 16, "nes0.xml", 0, NES_MAGIC),
    d(
        HeaderFileType::PCE,
        512,
        512,
        "pce.xml",
        0,
        &[0x40, 0, 0, 0, 0, 0, 0, 0, 0xAA, 0xBB, 0x02],
    ),
    d(
        HeaderFileType::PSID,
        118,
        118,
        "psid.xml",
        0,
        &[0x50, 0x53, 0x49, 0x44, 0x00, 0x01, 0x00, 0x76],
    ),
    d(
        HeaderFileType::PSID,
        118,
        118,
        "psid.xml",
        0,
        &[0x50, 0x53, 0x49, 0x44, 0x00, 0x03, 0x00, 0x7c],
    ),
    d(
        HeaderFileType::PSID,
        124,
        124,
        "psid.xml",
        0,
        &[0x50, 0x53, 0x49, 0x44, 0x00, 0x02, 0x00, 0x7c],
    ),
    d(
        HeaderFileType::PSID,
        124,
        124,
        "psid.xml",
        0,
        &[0x50, 0x53, 0x49, 0x44, 0x00, 0x01, 0x00, 0x7c],
    ),
    d(
        HeaderFileType::PSID,
        124,
        124,
        "psid.xml",
        0,
        &[0x52, 0x53, 0x49, 0x44, 0x00, 0x02, 0x00, 0x7c],
    ),
    d(
        HeaderFileType::SNES,
        512,
        512,
        "snes.xml",
        22,
        &[0xAA, 0xBB, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00],
    ),
    d(
        HeaderFileType::SNES,
        512,
        512,
        "snes.xml",
        22,
        &[0x53, 0x55, 0x50, 0x45, 0x52, 0x55, 0x46, 0x4F],
    ),
    d(HeaderFileType::SPC, 256, 256, "spc.xml", 0, b"SNES-SPC"),
];

/// `FileHeaderReader.GetType(buffer, headerSize, out offset)` (`FileHeaders.cs:115-133`).
///
/// `probe` holds the first bytes of the file. Returns the type and the number of header bytes
/// the alt hashes skip (0 when nothing was found or for archive types).
pub fn detect(probe: &[u8]) -> (HeaderFileType, usize) {
    for detector in DETECTORS {
        if probe.len() <= detector.header_length {
            continue;
        }
        let end = detector.magic_offset + detector.magic.len();
        if probe.len() >= end && probe[detector.magic_offset..end] == *detector.magic {
            return (detector.file_type, detector.file_offset);
        }
    }
    (HeaderFileType::NOTHING, 0)
}

/// `FileHeaderReader.GetFileHeaderLength` (`FileHeaders.cs:53-62`): the first table row of
/// `file_type`, 0 if there is none.
pub fn file_header_length(file_type: HeaderFileType) -> usize {
    DETECTORS
        .iter()
        .find(|d| d.file_type == file_type)
        .map_or(0, |d| d.file_offset)
}

/// `FileHeaderReader.GetFileTypeFromHeader` (`FileHeaders.cs:64-83`): the type for the
/// `header=` name of a DAT, matched case-insensitively.
pub fn file_type_from_header(header: &str) -> HeaderFileType {
    if header.trim().is_empty() {
        return HeaderFileType::NOTHING;
    }
    let wanted = header.to_lowercase();
    DETECTORS
        .iter()
        .filter(|d| !d.header_id.is_empty())
        .find(|d| d.header_id == wanted)
        .map_or(HeaderFileType::NOTHING, |d| d.file_type)
}

/// `FileHeaderReader.AltHeaderFile` (`FileHeaders.cs:85-98`): types whose files get alt hashes.
pub fn alt_header_file(file_type: HeaderFileType) -> bool {
    let t = HeaderFileType(file_type.0 & HeaderFileType::HEADER_MASK.0);
    [
        HeaderFileType::A7800,
        HeaderFileType::FDS,
        HeaderFileType::LYNX,
        HeaderFileType::NES,
        HeaderFileType::PCE,
        HeaderFileType::PSID,
        HeaderFileType::SNES,
        HeaderFileType::SPC,
        HeaderFileType::CHD,
    ]
    .contains(&t)
}
