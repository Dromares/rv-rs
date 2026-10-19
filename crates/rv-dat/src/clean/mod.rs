//! ROMVault's DAT clean-up pipeline: what `DatReader.ReadInDatFile` does to a parsed DAT before
//! it is merged into the database (`RomVaultCore/ReadDat/DatReader.cs:65-293`,
//! `DAT/DATReader/DatClean/*.cs`, `docs/dat-loading.md` section 7).
//!
//! [`run`] is steps 1 to 18 in ROMVault's order, [`finish`] is what follows them (the
//! auto-directory, the header type, file extensions, descriptions). Each step is also a public
//! function so the predicates can be tested alone.
//!
//! Differences from 3.8.0, on purpose:
//!
//! * The steps that `(DatDir)` cast every child of a directory fail with a [`CleanError`] when
//!   a loose file sits where a game should be (an XML DAT with a top-level `<rom>`); ROMVault
//!   throws and the whole update stops. The update here skips that one DAT and goes on.
//! * `ChildNameSearch` looks at the names as they are now. ROMVault's name index goes stale
//!   when `AddCategory` renames games in place, so a parent set can be missed there.
//! * Games are sorted for merging with an ordinal natural-order compare. ROMVault's
//!   `string.CompareTo` is culture-sensitive; the order only decides the order in which clone
//!   ROMs reach their parent, which the sort in step 11 undoes.
//! * `NonMerged` copies the device ROMs into each game; ROMVault shares one `DatFile` object
//!   between the games, so a later rename (the `.chd` of `SetExt`) is applied to it twice.
//! * Disks directly in the root of the DAT (an XML `<disk>` outside a game) are left where
//!   they are by `MoveUpCHDs`.

mod basic;
mod layout;
mod merge;

use std::fmt;

use rv_core::{FileType, HeaderFileType, HeaderType, MergeType, RemoveSubType, ZipStructure};

use crate::{DatDir, DatHeader, DatNode};

pub use basic::{
    add_category, check_de_duped, clean_filenames, find_category, remove_chd, remove_no_dumps,
    remove_non_chd, set_add_id_numbers, set_status,
};
pub use layout::{
    clean_file_names_full, clear_description, directory_expand, directory_flatten, fix_dupes,
    make_dat_single_level, move_up_chds, remove_all_date_time, remove_empty_directories,
    remove_un_needed_directories, set_ext, set_type,
};
pub use merge::{
    has_rom_of, make_merge_set, make_non_merge_set, make_split_set, remove_device_ref,
    remove_dupes, remove_empty_sets, set_match_ids,
};

/// A DAT that the clean-up cannot process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanError(pub String);

impl fmt::Display for CleanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CleanError {}

pub(crate) type CR<T> = Result<T, CleanError>;

/// The `(DatDir)tDat[g]` cast: a loose file where a game or directory should be.
pub(crate) fn dir_at(dir: &mut DatDir, index: usize) -> CR<&mut DatDir> {
    match &mut dir.children[index] {
        DatNode::Dir(d) => Ok(d),
        DatNode::File(f) => Err(CleanError(format!(
            "{:?} is a file where a game was expected",
            f.base.name
        ))),
    }
}

/// The rule settings the pipeline reads, so a test can build them without a `Settings`.
#[derive(Clone, Debug)]
pub struct CleanRule {
    pub filter: rv_core::FilterType,
    pub use_id_for_name: bool,
    pub add_category_sub_dirs: bool,
    pub category_order: Vec<String>,
    pub merge: MergeType,
    pub merge_override_dat: bool,
    pub compression: FileType,
    pub compression_override_dat: bool,
    pub compression_sub: ZipStructure,
    pub convert_while_fixing: bool,
    pub single_archive: bool,
    pub sub_dir_type: RemoveSubType,
    pub use_description_as_dir_name: bool,
    pub header_type: HeaderType,
}

impl From<&rv_core::DatRule> for CleanRule {
    fn from(r: &rv_core::DatRule) -> CleanRule {
        CleanRule {
            filter: r.filter,
            use_id_for_name: r.use_id_for_name,
            add_category_sub_dirs: r.add_category_sub_dirs,
            category_order: r.category_order.clone(),
            merge: r.merge,
            merge_override_dat: r.merge_override_dat,
            compression: r.compression,
            compression_override_dat: r.compression_override_dat,
            compression_sub: r.compression_sub,
            convert_while_fixing: r.convert_while_fixing,
            single_archive: r.single_archive,
            sub_dir_type: r.sub_dir_type,
            use_description_as_dir_name: r.use_description_as_dir_name,
            header_type: r.header_type,
        }
    }
}

/// `DatReader.GetCompressionMethod` (`DatReader.cs:343-375`, step 9): the archive type and
/// structure the DAT's games get. The DAT's own `forcepacking` wins unless the rule overrides
/// it.
pub fn compression_method(rule: &CleanRule, header: &DatHeader) -> (FileType, ZipStructure) {
    let mut ft = if rule.compression == FileType::FILE_ONLY {
        FileType::FILE
    } else {
        rule.compression
    };
    let mut zs = rule.compression_sub;
    if !rule.compression_override_dat && rule.compression != FileType::FILE_ONLY {
        match header.compression.to_lowercase().as_str() {
            "tdc" => {
                ft = FileType::ZIP;
                zs = ZipStructure::ZIP_TDC;
            }
            "unzip" | "file" | "fileonly" => {
                ft = FileType::DIR;
                zs = ZipStructure::NONE;
            }
            "7zip" | "7z" => {
                ft = FileType::SEVEN_ZIP;
                zs = ZipStructure::SEVEN_ZIP_SLZMA;
            }
            "zip" => {
                ft = FileType::ZIP;
                zs = ZipStructure::ZIP_TRRNT;
            }
            _ => {}
        }
    }
    (ft, zs)
}

/// `DatReader.SetMergeType` (`DatReader.cs:295-340`, step 6).
pub fn set_merge_type(rule: &CleanRule, header: &mut DatHeader) -> CR<()> {
    if !has_rom_of(&header.base_dir) {
        return Ok(());
    }
    let mut mt = rule.merge;
    if !rule.merge_override_dat {
        match header.merge_type.to_lowercase().as_str() {
            "full" => mt = MergeType::Merge,
            "nonmerge" => mt = MergeType::NonMerged,
            "split" => mt = MergeType::Split,
            _ => {}
        }
    }
    set_match_ids(&mut header.base_dir)?;
    match mt {
        MergeType::Merge => make_merge_set(&mut header.base_dir, header.mame_xml)?,
        MergeType::NonMerged => make_non_merge_set(&mut header.base_dir)?,
        MergeType::Split => make_split_set(&mut header.base_dir)?,
        MergeType::None | MergeType::ChdsMerge => {}
    }
    remove_device_ref(&mut header.base_dir);
    remove_dupes(
        &mut header.base_dir,
        !header.mame_xml,
        mt != MergeType::NonMerged,
    )?;
    remove_empty_sets(&mut header.base_dir);
    Ok(())
}

/// Steps 1 to 18 of `ReadInDatFile` (`DatReader.cs:112-242`).
pub fn run(rule: &CleanRule, header: &mut DatHeader) -> CR<()> {
    // 1
    clean_filenames(&mut header.base_dir);
    // 2
    match rule.filter {
        rv_core::FilterType::ChdsOnly => remove_non_chd(&mut header.base_dir)?,
        rv_core::FilterType::RomsOnly => remove_chd(&mut header.base_dir)?,
        rv_core::FilterType::KeepAll => {}
    }
    // 3
    remove_no_dumps(&mut header.base_dir);
    // 4
    if rule.use_id_for_name {
        set_add_id_numbers(&mut header.base_dir, "");
    }
    // 5
    if rule.add_category_sub_dirs {
        add_category(&mut header.base_dir, &rule.category_order);
    }
    // 6
    set_merge_type(rule, header)?;
    // 7
    set_status(&mut header.base_dir);
    // 8
    if rule.merge != MergeType::NonMerged {
        check_de_duped(&mut header.base_dir);
    }
    // 9
    let (mut ft, mut zs) = compression_method(rule, header);
    if rule.compression == FileType::FILE_ONLY {
        ft = FileType::DIR;
        zs = ZipStructure::NONE;
    }
    // 10
    if rule.single_archive {
        directory_flatten(&mut header.base_dir);
        make_dat_single_level(
            header,
            rule.use_description_as_dir_name,
            rule.sub_dir_type,
            ft == FileType::DIR,
            rule.add_category_sub_dirs,
            &rule.category_order,
        );
    }
    // 11
    set_type(&mut header.base_dir, ft, zs, rule.convert_while_fixing)?;
    // 12
    remove_un_needed_directories(&mut header.base_dir)?;
    // 13
    remove_all_date_time(&mut header.base_dir);
    // 14
    move_up_chds(&mut header.base_dir);
    // 15
    directory_expand(&mut header.base_dir);
    // 16
    clean_file_names_full(&mut header.base_dir);
    // 17
    fix_dupes(&mut header.base_dir);
    // 18
    remove_empty_directories(&mut header.base_dir);
    Ok(())
}

/// What follows step 18 (`DatReader.cs:249-285`): wrap the DAT in its auto-created directory
/// when there is one, work out the copier-header type from the DAT's `header=` and the rule,
/// add the file extensions and blank the descriptions that only repeat the name. Returns the
/// header file type that every non-disk file of the DAT now carries.
pub fn finish(
    rule: &CleanRule,
    header: &mut DatHeader,
    extra_dir_name: Option<&str>,
) -> HeaderFileType {
    if let Some(extra) = extra_dir_name.filter(|n| !n.trim().is_empty()) {
        header.base_dir.base.name = crate::varfix::clean_file_name(extra);
        let inner = std::mem::replace(&mut header.base_dir, DatDir::new("", FileType::DIR));
        header.base_dir.child_add(DatNode::Dir(inner));
    }

    let mut header_file_type = rv_core::header::file_type_from_header(&header.header);
    if header_file_type != HeaderFileType::NOTHING {
        match rule.header_type {
            HeaderType::Optional => {}
            HeaderType::Headerless => header_file_type = HeaderFileType::NOTHING,
            HeaderType::Headered => {
                header_file_type = HeaderFileType(header_file_type.0 | HeaderFileType::REQUIRED.0)
            }
        }
    }

    set_ext(&mut header.base_dir, header_file_type);
    clear_description(&mut header.base_dir);
    header_file_type
}
