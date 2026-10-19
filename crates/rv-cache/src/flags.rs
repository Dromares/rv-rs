//! The `FileFlags` word of a node (`rvFile.cs:378-402`, cache section 3.2).

pub(crate) const SIZE: u32 = 0x0_0001;
pub(crate) const CRC: u32 = 0x0_0002;
pub(crate) const SHA1: u32 = 0x0_0004;
pub(crate) const MD5: u32 = 0x0_0008;
pub(crate) const HEADER_FILE_TYPE: u32 = 0x0_0010;
pub(crate) const ALT_SIZE: u32 = 0x0_0020;
pub(crate) const ALT_CRC: u32 = 0x0_0040;
pub(crate) const ALT_SHA1: u32 = 0x0_0080;
pub(crate) const ALT_MD5: u32 = 0x0_0100;
pub(crate) const MERGE: u32 = 0x0_0200;
pub(crate) const STATUS: u32 = 0x0_0400;
pub(crate) const ZIP_FILE_INDEX: u32 = 0x0_0800;
pub(crate) const ZIP_FILE_HEADER: u32 = 0x0_1000;
pub(crate) const CHD_VERSION: u32 = 0x0_2000;
pub(crate) const HAS_TREE: u32 = 0x0_4000;
pub(crate) const HAS_GAME: u32 = 0x0_8000;
pub(crate) const HAS_DAT: u32 = 0x1_0000;
pub(crate) const HAS_DIR_DAT: u32 = 0x2_0000;
pub(crate) const HAS_CHILDREN: u32 = 0x4_0000;
pub(crate) const TO_SORT_STATUS: u32 = 0x8_0000;
pub(crate) const MIA_STATUS: u32 = 0x10_0000;

/// Bits 21-31, which the format does not define and the C# reader ignores.
pub(crate) const UNKNOWN_MASK: u32 = !0x1F_FFFF;
