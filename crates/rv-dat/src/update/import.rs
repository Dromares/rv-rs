//! The DATs found under `DatRoot` (`RomVaultCore/ReadDat/Storage/DatImportDir.cs`,
//! `DatImportDat.cs`).

use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use rv_core::sorters::{compare_dat_name, directory_name_compare_case};
use rv_core::{DatFlags, HeaderType, RemoveSubType, Settings};

use crate::DatHeader;

/// Ticks between 0001-01-01 and 1970-01-01 (`DateTime.UnixEpoch.Ticks`).
const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;

/// A file's last-write time as .NET UTC ticks (`FileInfo.LastWriteTimeUtc.Ticks`), 0 when it
/// cannot be read (`RVIO.cs:74`).
pub fn ticks_of(meta: &fs::Metadata) -> i64 {
    let Ok(modified) = meta.modified() else {
        return 0;
    };
    match modified.duration_since(UNIX_EPOCH) {
        Ok(d) => {
            UNIX_EPOCH_TICKS + (d.as_secs() as i64) * 10_000_000 + i64::from(d.subsec_nanos() / 100)
        }
        Err(e) => {
            let d = e.duration();
            UNIX_EPOCH_TICKS - (d.as_secs() as i64) * 10_000_000 - i64::from(d.subsec_nanos() / 100)
        }
    }
}

/// `DatImportDat`: one DAT file on disk and what the rule of its directory says about it.
#[derive(Clone, Debug)]
pub struct ImportDat {
    /// `DatRoot/dir/name.dat`, with `/`.
    pub full_name: String,
    /// UTC ticks of the file's last write.
    pub time_stamp: i64,
    pub flags: DatFlags,
    pub sub_dir_type: RemoveSubType,
    /// Set by reading the DAT: the rule's header type.
    pub header_type: HeaderType,
    /// Set by reading the DAT.
    pub header: Option<DatHeader>,
}

impl ImportDat {
    pub fn flag(&self, f: DatFlags) -> bool {
        self.flags.contains(f)
    }

    pub fn set_flag(&mut self, f: DatFlags, on: bool) {
        self.flags = if on {
            self.flags.with(f)
        } else {
            self.flags.without(f)
        };
    }
}

/// `DatImportDir`: a directory of `DatRoot` that holds DATs, or has subdirectories that do.
#[derive(Clone, Debug)]
pub struct ImportDir {
    pub name: String,
    /// Sorted by [`compare_dat_name`] of the full name.
    pub dats: Vec<ImportDat>,
    /// Sorted by `DirectoryNameCompareCase`.
    pub dirs: Vec<ImportDir>,
}

/// A problem that stops the walk of `DatRoot`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// `Path: ... Not Found.`
    PathNotFound(String),
    /// No rule in the settings covers this DAT (`FindDatRule` returned null).
    NoRule(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::PathNotFound(p) => write!(f, "Path: {p} Not Found."),
            ImportError::NoRule(d) => write!(f, "no DAT rule covers {d}"),
        }
    }
}

impl std::error::Error for ImportError {}

fn is_dat_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".dat") || lower.ends_with(".xml") || lower.ends_with(".datz")
}

/// `DatImportDir.RecursiveDatTree` (`DatImportDir.cs:102-177`): walk `DatRoot`. Returns the
/// tree (named `RomVault`) and the number of DATs.
///
/// The extensions `.dat`, `.xml` and `.datz` are matched in any case; .NET's glob is
/// case-sensitive on Linux, so ROMVault would skip `FOO.DAT` there but take it on Windows.
pub fn recursive_dat_tree(settings: &Settings) -> Result<(ImportDir, usize), ImportError> {
    let mut root = ImportDir {
        name: "RomVault".to_owned(),
        dats: Vec::new(),
        dirs: Vec::new(),
    };
    let count = walk(settings, &mut root, "DatRoot")?;
    Ok((root, count))
}

fn walk(settings: &Settings, dir: &mut ImportDir, dat_tree: &str) -> Result<usize, ImportError> {
    let physical = settings.dat_physical_path(dat_tree);
    let path = Path::new(&physical);
    if !path.is_dir() {
        return Err(ImportError::PathNotFound(physical));
    }
    let mut files: Vec<(String, i64)> = Vec::new();
    let mut subdirs: Vec<String> = Vec::new();
    // `RVIO.DirectoryInfo.GetFiles` swallows errors and returns what it has.
    if let Ok(rd) = fs::read_dir(path) {
        for entry in rd.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(meta) = fs::metadata(entry.path()) else {
                continue;
            };
            if meta.is_dir() {
                subdirs.push(name);
            } else if meta.is_file() && is_dat_name(&name) {
                files.push((name, ticks_of(&meta)));
            }
        }
    }

    let mut count = files.len();
    for (name, time_stamp) in files {
        let full_name = format!("{dat_tree}/{name}");
        let rule = settings
            .find_dat_rule(&full_name)
            .ok_or_else(|| ImportError::NoRule(full_name.clone()))?;
        let mut dat = ImportDat {
            full_name,
            time_stamp,
            flags: DatFlags::default(),
            sub_dir_type: rule.sub_dir_type,
            header_type: rule.header_type,
            header: None,
        };
        dat.set_flag(DatFlags::MULTI_DAT_OVERRIDE, rule.multi_dat_dir_override);
        dat.set_flag(
            DatFlags::USE_DESCRIPTION_AS_DIR_NAME,
            rule.use_description_as_dir_name,
        );
        dat.set_flag(DatFlags::SINGLE_ARCHIVE, rule.single_archive);
        dat.set_flag(DatFlags::USE_ID_FOR_NAME, rule.use_id_for_name);
        // Insert in order, before equal names (`BinarySearch.ListSearch`).
        let (_, at) = rv_core::sorters::list_search(&dir.dats, |d| {
            compare_dat_name(&dat.full_name, &d.full_name)
        });
        dir.dats.insert(at, dat);
    }
    if dir.dats.len() > 1 {
        for d in &mut dir.dats {
            d.set_flag(DatFlags::MULTI_DATS_IN_DIRECTORY, true);
        }
    }

    for name in subdirs {
        let mut child = ImportDir {
            name: name.clone(),
            dats: Vec::new(),
            dirs: Vec::new(),
        };
        let child_count = walk(settings, &mut child, &format!("{dat_tree}/{name}"))?;
        count += child_count;
        if child_count > 0 {
            let (_, at) = rv_core::sorters::list_search(&dir.dirs, |d| {
                directory_name_compare_case(&child.name, &d.name)
            });
            dir.dirs.insert(at, child);
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_of_the_unix_epoch_and_a_known_time() {
        assert_eq!(UNIX_EPOCH_TICKS, 621_355_968_000_000_000);
        // 2024-09-01 03:52:00 UTC, the NES DAT of the board's collection: ticks as the cache
        // stores them (`DatRoot/Console/NES/Roms/...` has 638607595200000000).
        let secs = 1_725_162_720i64;
        assert_eq!(
            UNIX_EPOCH_TICKS + secs * 10_000_000,
            638_607_595_200_000_000
        );
    }

    #[test]
    fn dat_names() {
        assert!(is_dat_name("a.DAT"));
        assert!(is_dat_name("a.xml"));
        assert!(is_dat_name("a.DatZ"));
        assert!(!is_dat_name("a.zip"));
        assert!(!is_dat_name("dat"));
    }
}
