//! Enums and flag sets that the cache stores as raw integers.
//!
//! Each is a newtype over the wire integer with named constants, not a Rust `enum`, so
//! a value this code does not know about is carried through unchanged.

macro_rules! raw_newtype {
    ($(#[$meta:meta])* $name:ident($ty:ty) { $($(#[$cmeta:meta])* $cname:ident = $val:expr),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub $ty);

        impl $name {
            $($(#[$cmeta])* pub const $cname: $name = $name($val);)*

            /// True when every bit of `other` is set in `self`.
            pub const fn contains(self, other: $name) -> bool {
                self.0 & other.0 == other.0
            }

            /// `self` with the bits of `other` set.
            pub const fn with(self, other: $name) -> $name {
                $name(self.0 | other.0)
            }

            /// `self` with the bits of `other` cleared.
            pub const fn without(self, other: $name) -> $name {
                $name(self.0 & !other.0)
            }
        }
    };
}

raw_newtype! {
    /// `FileType`, stored as `u8` (`SharedEnum/FileType.cs:1-12`).
    FileType(u8) {
        UNSET = 0,
        DIR = 1,
        ZIP = 2,
        SEVEN_ZIP = 3,
        FILE = 4,
        FILE_ZIP = 5,
        FILE_SEVEN_ZIP = 6,
        FILE_ONLY = 100,
    }
}

impl FileType {
    /// Zip and SevenZip nodes carry the two zip-structure bytes (`DBTypeGet.isCompressedDir`).
    pub const fn is_compressed_dir(self) -> bool {
        self.0 == Self::ZIP.0 || self.0 == Self::SEVEN_ZIP.0
    }
}

raw_newtype! {
    /// `DatStatus`, stored as `u8` (`SharedEnum/DatStatus.cs:1-9`). Value 5 is the retired `InDatMIA`.
    DatStatus(u8) {
        IN_DAT_COLLECT = 0,
        IN_DAT_MERGED = 1,
        IN_DAT_NO_DUMP = 2,
        NOT_IN_DAT = 3,
        IN_TO_SORT = 4,
        RETIRED_IN_DAT_MIA = 5,
    }
}

raw_newtype! {
    /// `GotStatus`, stored as `u8` (`SharedEnum/GotStatus.cs:1-7`).
    GotStatus(u8) {
        NOT_GOT = 0,
        GOT = 1,
        CORRUPT = 2,
        FILE_LOCKED = 3,
    }
}

raw_newtype! {
    /// `ZipStructure`, stored as `u8` (`Compress/StructuredZip/StructuredArchive.cs:3-16`).
    ZipStructure(u8) {
        NONE = 0,
        ZIP_TRRNT = 1,
        ZIP_TDC = 2,
        ZIP_DTD = 3,
        SEVEN_ZIP_TRRNT = 4,
        ZIP_ZSTD = 5,
        ZIP_DTZ = 6,
        SEVEN_ZIP_SLZMA = 8,
        SEVEN_ZIP_NLZMA = 9,
        SEVEN_ZIP_SZSTD = 10,
        SEVEN_ZIP_NZSTD = 11,
    }
}

raw_newtype! {
    /// `RvTreeRow.TreeSelect`, stored as `u8` (`RvTreeRow.cs:14-19`).
    TreeSelect(u8) {
        UN_SELECTED = 0,
        SELECTED = 1,
        LOCKED = 2,
    }
}

raw_newtype! {
    /// `HeaderFileType`, raw byte including the `Required` bit (`SharedEnum/HeaderFileType.cs:1-25`).
    HeaderFileType(u8) {
        NOTHING = 0,
        ZIP = 1,
        GZ = 2,
        SEVEN_ZIP = 3,
        RAR = 4,
        CHD = 5,
        A7800 = 6,
        LYNX = 7,
        NES = 8,
        FDS = 9,
        PCE = 10,
        PSID = 11,
        SNES = 12,
        SPC = 13,
        HEADER_MASK = 0x1f,
        REQUIRED = 0x80,
    }
}

raw_newtype! {
    /// `MIAStatus` flags, stored as `u8` (`SharedEnum/MIAStatus.cs:1-12`).
    MiaStatus(u8) {
        NONE = 0,
        NEW = 1,
        SEEN = 2,
        MIA = 4,
        MIA_FROM_DAT = 8,
        MIA_USER = 16,
    }
}

raw_newtype! {
    /// `ToSortDirType` flags, stored as `u8` (`rvFile.cs:81-90`).
    ToSortDirType(u8) {
        NONE = 0,
        PRIMARY = 1,
        CACHE = 2,
        FILE_ONLY = 4,
    }
}

raw_newtype! {
    /// `DatFlags` in a `Dat`, stored as `u8` (`RvDat.cs:21-30`).
    DatFlags(u8) {
        AUTO_ADDED_DIRECTORY = 1,
        MULTI_DAT_OVERRIDE = 2,
        MULTI_DATS_IN_DIRECTORY = 4,
        USE_DESCRIPTION_AS_DIR_NAME = 8,
        SINGLE_ARCHIVE = 16,
        USE_ID_FOR_NAME = 32,
    }
}

raw_newtype! {
    /// `FileStatus` flags, always stored as `u32` (`SharedEnum/FileStatus.cs:4-49`).
    FileStatus(u32) {
        HEADER_FILE_TYPE_FROM_DAT = 1 << 0,
        SIZE_FROM_DAT = 1 << 1,
        CRC_FROM_DAT = 1 << 2,
        SHA1_FROM_DAT = 1 << 3,
        MD5_FROM_DAT = 1 << 4,
        ALT_SIZE_FROM_DAT = 1 << 5,
        ALT_CRC_FROM_DAT = 1 << 6,
        ALT_SHA1_FROM_DAT = 1 << 7,
        ALT_MD5_FROM_DAT = 1 << 8,
        HEADER_FILE_TYPE_FROM_HEADER = 1 << 9,
        SIZE_FROM_HEADER = 1 << 10,
        CRC_FROM_HEADER = 1 << 11,
        SHA1_FROM_HEADER = 1 << 12,
        MD5_FROM_HEADER = 1 << 13,
        ALT_SIZE_FROM_HEADER = 1 << 14,
        ALT_CRC_FROM_HEADER = 1 << 15,
        ALT_SHA1_FROM_HEADER = 1 << 16,
        ALT_MD5_FROM_HEADER = 1 << 17,
        SIZE_VERIFIED = 1 << 18,
        CRC_VERIFIED = 1 << 19,
        SHA1_VERIFIED = 1 << 20,
        MD5_VERIFIED = 1 << 21,
        ALT_SIZE_VERIFIED = 1 << 22,
        ALT_CRC_VERIFIED = 1 << 23,
        ALT_SHA1_VERIFIED = 1 << 24,
        ALT_MD5_VERIFIED = 1 << 25,
        SHA256_VERIFIED = 1 << 26,
        ALT_SHA256_VERIFIED = 1 << 27,
        DATE_FROM_DAT = 1 << 28,
        DEEP_SCANNED = 1 << 29,
        /// Retired: the old `ToSortPrimary` marker (`UpdateFixToSortStatus`, `DB.cs:193-208`).
        LEGACY_TO_SORT_PRIMARY = 1 << 30,
        /// Retired: the old `ToSortCache` marker.
        LEGACY_TO_SORT_CACHE = 1 << 31,
    }
}

/// `RvDat.DatData` ids (`RvDat.cs:35-57`).
pub mod dat_data {
    pub const ID: u8 = 0;
    pub const DAT_NAME: u8 = 1;
    pub const DAT_ROOT_FULL_NAME: u8 = 2;
    pub const ROOT_DIR: u8 = 3;
    pub const DESCRIPTION: u8 = 4;
    pub const CATEGORY: u8 = 5;
    pub const VERSION: u8 = 6;
    pub const DATE: u8 = 7;
    pub const AUTHOR: u8 = 8;
    pub const EMAIL: u8 = 9;
    pub const HOME_PAGE: u8 = 10;
    pub const URL: u8 = 11;
    pub const FILE_TYPE: u8 = 12;
    pub const MERGE_TYPE: u8 = 13;
    pub const SUPER_DAT: u8 = 14;
    pub const DIR_SETUP: u8 = 15;
    pub const HEADER: u8 = 16;
    pub const SUB_DIR_TYPE: u8 = 17;
    pub const COMPRESSION: u8 = 18;
}

/// `RvGame.GameData` ids (`RvGame.cs:15-47`). Numeric order is not declaration order.
pub mod game_data {
    pub const DESCRIPTION: u8 = 1;
    pub const ROM_OF: u8 = 2;
    pub const IS_BIOS: u8 = 3;
    pub const SOURCE_FILE: u8 = 4;
    pub const CLONE_OF: u8 = 5;
    pub const SAMPLE_OF: u8 = 6;
    pub const BOARD: u8 = 7;
    pub const YEAR: u8 = 8;
    pub const MANUFACTURER: u8 = 9;
    pub const EMU_ARC: u8 = 10;
    pub const ID: u8 = 11;
    pub const PUBLISHER: u8 = 12;
    pub const DEVELOPER: u8 = 13;
    pub const GENRE: u8 = 14;
    pub const SUB_GENRE: u8 = 15;
    pub const RATINGS: u8 = 16;
    pub const SCORE: u8 = 17;
    pub const PLAYERS: u8 = 18;
    pub const ENABLED: u8 = 19;
    pub const CRC: u8 = 20;
    pub const RELATED_TO: u8 = 21;
    pub const SOURCE: u8 = 22;
    pub const CATEGORY: u8 = 23;
    pub const CLONE_OF_ID: u8 = 24;
    pub const SERIAL: u8 = 25;
    pub const VERSION: u8 = 26;
    pub const GAME_ID: u8 = 27;
}
