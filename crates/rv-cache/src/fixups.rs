//! The post-load fix-ups that ROMVault applies while it reads a cache (cache section 6).
//!
//! They are separate from parsing on purpose: [`crate::CacheFile::from_bytes`] returns
//! exactly what is on disk, so a file writes back unchanged. Call [`apply_load_fixups`]
//! when the in-memory state ROMVault would have after loading is wanted, for example
//! before migrating a version 2 file. Writing afterwards stores the repaired values.
//!
//! Not covered here: `RepStatusReset` (step 9). It needs the DAT rules and settings, so it lives
//! in `rv-core` (`Database::report_status_reset`) and [`crate::load_database`] runs it.

use rv_core::{DatStatus, FileStatus, FileType, MiaStatus, RvFile, ToSortDirType, ZipStructure};

use crate::CacheFile;

/// Apply the fix-ups of `rvFile.cs:513-689` and `DB.cs:174` in ROMVault's order.
///
/// A version 2 file gets its `zip_dat_struct` bytes synthesized and is then marked
/// version 3, because that is what ROMVault writes next (`DB.cs:163-169`, `:77`).
pub fn apply_load_fixups(cache: &mut CacheFile) {
    let version = cache.version;
    // The root keeps its own stored status; children after the first read it as ToSort
    // (`rvFile.cs:647-656`, 2024/08/03).
    let root_status = DatStatus::IN_DAT_COLLECT;
    for (i, child) in cache.root.children.iter_mut().enumerate() {
        let parent_status = if i > 0 {
            DatStatus::IN_TO_SORT
        } else {
            root_status
        };
        fix_node(child, parent_status, version);
    }
    header_repair(&mut cache.root);
    update_fix_to_sort_status(&mut cache.root);
    if version < 3 {
        cache.version = 3;
    }
}

fn fix_node(node: &mut RvFile, parent_status: DatStatus, version: i32) {
    // 2024/08/03: anything under a ToSort item is InToSort (`rvFile.cs:551-554`).
    if parent_status == DatStatus::IN_TO_SORT {
        node.dat_status = DatStatus::IN_TO_SORT;
    }
    // 2025/06/20: the retired `InDatMIA` (`rvFile.cs:555-560`). A stored MIA status
    // byte wins, as in `:678-679`.
    if node.dat_status == DatStatus::RETIRED_IN_DAT_MIA {
        node.dat_status = DatStatus::IN_DAT_COLLECT;
        if node.mia_status.is_none() {
            node.mia_status = Some(MiaStatus::MIA_FROM_DAT);
        }
    }

    if node.file_type.is_compressed_dir() {
        // Version 2 files have no zipDatStruct byte (`rvFile.cs:565-582`).
        if version < 3 {
            node.zip_dat_struct = Some(0);
            if node.dat_status == DatStatus::IN_DAT_COLLECT {
                let structure = if node.file_type == FileType::SEVEN_ZIP {
                    ZipStructure::SEVEN_ZIP_SLZMA
                } else {
                    ZipStructure::ZIP_TRRNT
                };
                node.zip_dat_struct = Some(0x80 | structure.0);
            }
        }
        // 2024-01-08: the ZipStructure enum changed (`rvFile.cs:590-592`).
        if node.file_type == FileType::SEVEN_ZIP && node.zip_struct == Some(ZipStructure::ZIP_TRRNT)
        {
            node.zip_struct = Some(ZipStructure::SEVEN_ZIP_SLZMA);
        }
        // 2023-02-06 (`rvFile.cs:594-596`).
        if node.file_type == FileType::SEVEN_ZIP
            && node.dat_status == DatStatus::IN_DAT_COLLECT
            && node.zip_dat_struct.map(|raw| raw & 0x7f) == Some(ZipStructure::ZIP_TRRNT.0)
        {
            node.zip_dat_struct = Some(0x80 | ZipStructure::SEVEN_ZIP_SLZMA.0);
        }
    }

    let status = node.dat_status;
    for child in &mut node.children {
        fix_node(child, status, version);
    }
    header_repair(node);
}

/// "fixing missing flag" (`rvFile.cs:683-685`).
fn header_repair(node: &mut RvFile) {
    let has_header = node
        .header_file_type
        .is_some_and(|raw| raw.0 & rv_core::HeaderFileType::HEADER_MASK.0 != 0);
    let has_alt = node.alt_size.is_some()
        || node.alt_crc.is_some()
        || node.alt_sha1.is_some()
        || node.alt_md5.is_some();
    if has_header && has_alt {
        node.file_status = node
            .file_status
            .with(FileStatus::HEADER_FILE_TYPE_FROM_HEADER);
    }
}

/// `UpdateFixToSortStatus` (`DB.cs:193-208`): move the retired `FileStatus` bits 30 and
/// 31 of the root's children into the ToSort type.
fn update_fix_to_sort_status(root: &mut RvFile) {
    let both = ToSortDirType::PRIMARY.with(ToSortDirType::CACHE);
    if root
        .children
        .iter()
        .any(|c| c.to_sort_type.is_some_and(|t| t.contains(both)))
    {
        return;
    }
    for child in &mut root.children {
        let mut sort = child.to_sort_type.unwrap_or(ToSortDirType::NONE);
        if child
            .file_status
            .contains(FileStatus::LEGACY_TO_SORT_PRIMARY)
        {
            sort = sort.with(ToSortDirType::PRIMARY);
        }
        if child.file_status.contains(FileStatus::LEGACY_TO_SORT_CACHE) {
            sort = sort.with(ToSortDirType::CACHE);
        }
        child.to_sort_type = (sort != ToSortDirType::NONE).then_some(sort);
        child.file_status = child
            .file_status
            .without(FileStatus::LEGACY_TO_SORT_PRIMARY)
            .without(FileStatus::LEGACY_TO_SORT_CACHE);
    }
}
