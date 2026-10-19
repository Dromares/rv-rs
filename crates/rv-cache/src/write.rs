//! Serialization: `RvFile.Write` (`rvFile.cs:405-509`). The flag word is derived from
//! which optional fields are present, exactly as the C# writer derives it from the
//! in-memory object.

use rv_core::{MetaEntry, RvDat, RvFile};

use crate::error::CacheError;
use crate::flags;
use crate::{CacheFile, END_MARKER, MAX_DEPTH};

fn invalid<T>(why: impl Into<String>) -> Result<T, CacheError> {
    Err(CacheError::InvalidModel(why.into()))
}

struct Writer {
    out: Vec<u8>,
}

impl Writer {
    fn u8(&mut self, v: u8) {
        self.out.push(v);
    }

    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn i32(&mut self, v: i32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn u64(&mut self, v: u64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn i64(&mut self, v: i64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    /// `BinaryWriter.Write(string)`: shortest 7-bit varint byte length, then UTF-8.
    fn string(&mut self, s: &str) -> Result<(), CacheError> {
        let Ok(mut len) = u32::try_from(s.len()) else {
            return invalid("string is longer than 4 GiB");
        };
        if len > i32::MAX as u32 {
            return invalid("string is longer than 2 GiB");
        }
        while len >= 0x80 {
            self.u8((len & 0x7f) as u8 | 0x80);
            len >>= 7;
        }
        self.u8(len as u8);
        self.out.extend_from_slice(s.as_bytes());
        Ok(())
    }

    fn byte_array(&mut self, what: &str, bytes: &[u8]) -> Result<(), CacheError> {
        let Ok(len) = u8::try_from(bytes.len()) else {
            return invalid(format!(
                "{what} is {} bytes, the format allows 255",
                bytes.len()
            ));
        };
        self.u8(len);
        self.out.extend_from_slice(bytes);
        Ok(())
    }

    fn meta_entries(&mut self, what: &str, entries: &[MetaEntry]) -> Result<(), CacheError> {
        let Ok(count) = u8::try_from(entries.len()) else {
            return invalid(format!(
                "{what} has {} metadata entries, the format allows 255",
                entries.len()
            ));
        };
        self.u8(count);
        for entry in entries {
            self.u8(entry.id);
            self.string(&entry.value)?;
        }
        Ok(())
    }

    fn count(&mut self, what: &str, n: usize) -> Result<(), CacheError> {
        match i32::try_from(n) {
            Ok(n) => {
                self.i32(n);
                Ok(())
            }
            Err(_) => invalid(format!("{what} count {n} does not fit an Int32")),
        }
    }

    fn dat(&mut self, dat: &RvDat) -> Result<(), CacheError> {
        self.i64(dat.time_stamp); // RvDat.cs:147
        self.u8(dat.dat_flags.0); // :148
        self.meta_entries("a Dat", &dat.entries) // :150-154
    }

    fn node(
        &mut self,
        node: &RvFile,
        version: i32,
        is_root: bool,
        depth: usize,
    ) -> Result<(), CacheError> {
        if depth > MAX_DEPTH {
            return Err(CacheError::TooDeep);
        }
        let compressed = node.file_type.is_compressed_dir();
        if !compressed && (node.zip_struct.is_some() || node.zip_dat_struct.is_some()) {
            return invalid(format!(
                "node {:?} is not a Zip/SevenZip but has zip-structure bytes",
                node.name
            ));
        }
        if compressed {
            if node.zip_struct.is_none() {
                return invalid(format!("archive node {:?} has no zip_struct", node.name));
            }
            if (version >= 3) != node.zip_dat_struct.is_some() {
                return invalid(format!(
                    "archive node {:?}: zip_dat_struct must be present exactly in version 3 files",
                    node.name
                ));
            }
        }
        if is_root && node.file_type != rv_core::FileType::DIR {
            return invalid("the root node must be a Dir");
        }
        if node.unknown_flag_bits & !flags::UNKNOWN_MASK != 0 {
            return invalid("unknown_flag_bits overlaps defined flag bits");
        }

        let mut fl = node.unknown_flag_bits;
        let set = |fl: &mut u32, present: bool, bit: u32| {
            if present {
                *fl |= bit;
            }
        };
        set(&mut fl, node.dat_index.is_some(), flags::HAS_DAT);
        set(&mut fl, node.tree.is_some(), flags::HAS_TREE);
        set(&mut fl, node.game.is_some(), flags::HAS_GAME);
        set(&mut fl, !node.dir_dats.is_empty(), flags::HAS_DIR_DAT);
        set(&mut fl, !node.children.is_empty(), flags::HAS_CHILDREN);
        set(&mut fl, node.size.is_some(), flags::SIZE);
        set(&mut fl, node.crc.is_some(), flags::CRC);
        set(&mut fl, node.sha1.is_some(), flags::SHA1);
        set(&mut fl, node.md5.is_some(), flags::MD5);
        set(
            &mut fl,
            node.header_file_type.is_some(),
            flags::HEADER_FILE_TYPE,
        );
        set(&mut fl, node.alt_size.is_some(), flags::ALT_SIZE);
        set(&mut fl, node.alt_crc.is_some(), flags::ALT_CRC);
        set(&mut fl, node.alt_sha1.is_some(), flags::ALT_SHA1);
        set(&mut fl, node.alt_md5.is_some(), flags::ALT_MD5);
        set(&mut fl, node.merge.is_some(), flags::MERGE);
        set(&mut fl, node.status.is_some(), flags::STATUS);
        set(
            &mut fl,
            node.zip_file_index.is_some(),
            flags::ZIP_FILE_INDEX,
        );
        set(
            &mut fl,
            node.zip_file_header_position.is_some(),
            flags::ZIP_FILE_HEADER,
        );
        set(&mut fl, node.chd_version.is_some(), flags::CHD_VERSION);
        set(&mut fl, node.to_sort_type.is_some(), flags::TO_SORT_STATUS);
        set(&mut fl, node.mia_status.is_some(), flags::MIA_STATUS);

        if !is_root {
            self.u8(node.file_type.0); // the parent writes it, rvFile.cs:480
        }
        self.u32(fl); // :441
        self.string(&node.name)?; // :446
        self.string(&node.file_name)?; // :447
        self.i64(node.file_mod_time_stamp); // :449
        if let Some(index) = node.dat_index {
            self.i32(index); // :451
        }
        self.u8(node.dat_status.0); // :452
        self.u8(node.got_status.0); // :453
        if compressed {
            if let Some(raw) = node.zip_dat_struct {
                self.u8(raw); // :455-457
            }
            if let Some(zip_struct) = node.zip_struct {
                self.u8(zip_struct.0); // :458
            }
        }
        if let Some(tree) = &node.tree {
            self.u8(u8::from(tree.expanded)); // RvTreeRow.cs:71
            self.u8(tree.checked.0); // :72
        }
        if let Some(game) = &node.game {
            self.meta_entries("a Game", &game.entries)?; // :463
        }
        if !node.dir_dats.is_empty() {
            self.count("dat", node.dir_dats.len())?; // :465-467
            for dat in &node.dir_dats {
                self.dat(dat)?; // :468-472
            }
        }
        if !node.children.is_empty() {
            self.count("child", node.children.len())?; // :475-477
            for child in &node.children {
                self.node(child, version, false, depth + 1)?; // :478-482
            }
        }
        if let Some(v) = node.size {
            self.u64(v); // :487
        }
        if let Some(v) = &node.crc {
            self.byte_array("CRC", v)?;
        }
        if let Some(v) = &node.sha1 {
            self.byte_array("SHA1", v)?;
        }
        if let Some(v) = &node.md5 {
            self.byte_array("MD5", v)?;
        }
        if let Some(v) = node.header_file_type {
            self.u8(v.0); // :492
        }
        if let Some(v) = node.alt_size {
            self.u64(v);
        }
        if let Some(v) = &node.alt_crc {
            self.byte_array("AltCRC", v)?;
        }
        if let Some(v) = &node.alt_sha1 {
            self.byte_array("AltSHA1", v)?;
        }
        if let Some(v) = &node.alt_md5 {
            self.byte_array("AltMD5", v)?;
        }
        if let Some(v) = &node.merge {
            self.string(v)?; // :498
        }
        if let Some(v) = &node.status {
            self.string(v)?; // :499
        }
        if let Some(v) = node.zip_file_index {
            self.i32(v); // :500
        }
        if let Some(v) = node.zip_file_header_position {
            self.u64(v); // :501
        }
        if let Some(v) = node.chd_version {
            self.u32(v); // :502
        }
        if let Some(v) = node.to_sort_type {
            self.u8(v.0); // :504
        }
        if let Some(v) = node.mia_status {
            self.u8(v.0); // :506
        }
        self.u32(node.file_status.0); // :508
        Ok(())
    }
}

/// Serialize a whole cache file: version, root node, end marker (`DB.cs:77-79`).
pub(crate) fn serialize(cache: &CacheFile) -> Result<Vec<u8>, CacheError> {
    if cache.version != 2 && cache.version != 3 {
        return Err(CacheError::UnsupportedVersion(cache.version));
    }
    let mut w = Writer { out: Vec::new() };
    w.i32(cache.version);
    w.node(&cache.root, cache.version, true, 0)?;
    w.u64(END_MARKER);
    Ok(w.out)
}
