//! The scan of a directory tree against the database: `FileScanning.CheckADir`,
//! `CheckAnArchive`, `MatchFound`, `NewFileFound`, `DBFileNotFound` and `Populate.FromADir`
//! (`RomVaultCore/Scanner/FileScanning.cs`, `Populate.cs`).

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rv_core::progress::{CancelFlag, ProgressEvent, ProgressSink};
use rv_core::settings::{dat_tree_path, path_combine};
use rv_core::sorters::compare_name;
use rv_core::time::ticks_of;
use rv_core::{
    Cx, EFile, FileStatus, FileType, GotStatus, RvFile, ScanLevel, Settings, ToSortDirType,
};

use super::merge::{compare_with_alt, file_add, file_merge_in, merge_in_archive, phase1};
use super::populate::{from_a_file, scan_7z, scan_zip, OpenFailure};
use super::scanned::ScannedFile;
use super::ScanReport;

/// What the directories above a node say about whether its files are stored as plain files
/// (`IsFileOnly`, `RomVaultCore/Utils/IsFileOnly.cs`).
#[derive(Clone, Debug, Default)]
pub(crate) struct Ancestor {
    /// The node has a DAT; this is the DAT's `forcepacking` text.
    pub dat_compression: Option<String>,
    /// The node holds DATs; is one of them `fileonly`?
    pub dat_list_file_only: Option<bool>,
}

impl Ancestor {
    pub fn of(node: &RvFile, compression: &HashMap<u32, String>) -> Ancestor {
        Ancestor {
            dat_compression: (node.dat_uid != 0)
                .then(|| compression.get(&node.dat_uid).cloned().unwrap_or_default()),
            dat_list_file_only: (!node.dir_dats.is_empty()).then(|| {
                node.dir_dats.iter().any(|d| {
                    d.get(rv_core::dat_data::COMPRESSION)
                        .is_some_and(|c| c == "fileonly")
                })
            }),
        }
    }
}

pub(crate) struct Scan<'a> {
    pub settings: &'a Settings,
    pub level: ScanLevel,
    pub progress: &'a dyn ProgressSink,
    pub cancel: &'a CancelFlag,
    pub abort: bool,
    pub report: ScanReport,
    /// `uid -> DatData.Compression` of every DAT in the tree.
    pub compression: HashMap<u32, String>,
    /// The ancestors of the directory being scanned, outermost first.
    pub chain: Vec<Ancestor>,
    /// The ToSort root above the scan root is a "files only" one.
    pub to_sort_file_only: bool,
    pub depth: usize,
}

/// Directory depth at which the scan stops descending, so a symbolic link cycle ends.
const MAX_DEPTH: usize = 128;

impl Scan<'_> {
    fn text2(&self, s: &str) {
        self.progress.report(ProgressEvent::Item(s.to_owned()));
    }

    fn error(&mut self, file: &str, message: String) {
        self.progress.report(ProgressEvent::Error {
            file: file.to_owned(),
            message: message.clone(),
        });
        self.report.errors.push((file.to_owned(), message));
    }

    /// `IsFileOnly.isFileOnly(dbDir)` (`IsFileOnly.cs:8-71`) for the directory at `tree`. The
    /// rule is looked up with `/`; ROMVault appends `\` and so finds only the root rule on Linux
    /// (`docs/linux-port.md` section 3.3).
    fn is_file_only(&self, tree: &str) -> bool {
        if self.settings.files_only {
            return true;
        }
        let dat_path = format!("{}/", dat_tree_path(tree));
        let Some(rule) = self.settings.find_dat_rule(&dat_path) else {
            return self.to_sort_file_only;
        };
        let mut dat_header_type: Option<&str> = None;
        for a in self.chain.iter().rev() {
            if let Some(c) = &a.dat_compression {
                dat_header_type = Some(c);
                break;
            }
            if let Some(file_only) = a.dat_list_file_only {
                if file_only {
                    dat_header_type = Some("fileonly");
                }
                break;
            }
        }
        if rule.compression_override_dat {
            return rule.compression == FileType::FILE_ONLY;
        }
        if dat_header_type.is_some_and(|t| t.eq_ignore_ascii_case("fileonly")) {
            return true;
        }
        rule.compression == FileType::FILE_ONLY
    }

    /// `Populate.FromADir` (`Populate.cs:69-160`): list one directory on disk.
    fn list_dir(&mut self, tree: &str, tree_case: &str) -> ScannedFile {
        let physical = self.settings.physical_path(tree_case);
        let mut dir = ScannedFile::new(FileType::DIR, "");
        self.progress
            .report(ProgressEvent::Text(format!("Scanning Dir : {physical}")));

        let mut files: Vec<(String, fs::Metadata)> = Vec::new();
        // `RVIO.DirectoryInfo` swallows listing errors and returns what it has.
        if let Ok(rd) = fs::read_dir(Path::new(&physical)) {
            for entry in rd.flatten() {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                let Ok(meta) = fs::metadata(entry.path()) else {
                    continue;
                };
                if meta.is_dir() {
                    let mut d = ScannedFile::new(FileType::DIR, &name);
                    d.file_mod_time_stamp = ticks_of(&meta);
                    d.got_status = GotStatus::GOT;
                    dir.children.push(d);
                } else if meta.is_file() {
                    files.push((name, meta));
                }
            }
        }

        let rule = self
            .settings
            .find_dat_rule(&format!("{}/", dat_tree_path(tree)));
        let ignore = match rule {
            Some(r) if !r.ignore_scan_regex().is_empty() => r.ignore_scan_regex(),
            _ => self.settings.ignore_scan_regex(),
        };
        let is_file_only = self.is_file_only(tree);

        for (name, meta) in files {
            if name.starts_with("__RomVault.") && name.ends_with(".tmp") {
                // ROMVault deletes these; the scanner here only reads.
                let message = "Stale temporary file ignored.".to_owned();
                self.error(&path_combine(&physical, &name), message);
                continue;
            }
            if ignore.iter().any(|re| re.is_match(&name)) {
                continue;
            }
            let ext = name.rfind('.').map_or("", |i| &name[i..]).to_lowercase();
            let mut ft = match ext.as_str() {
                ".7z" => FileType::SEVEN_ZIP,
                ".zip" => FileType::ZIP,
                _ => FileType::FILE,
            };
            if self.settings.files_only || self.to_sort_file_only || is_file_only {
                ft = FileType::FILE;
            }
            let mut f = ScannedFile::new(ft, &name);
            f.size = Some(meta.len());
            f.file_mod_time_stamp = ticks_of(&meta);
            f.got_status = GotStatus::GOT;
            f.file_status_set(FileStatus::SIZE_VERIFIED);
            if self.level == ScanLevel::Level3 && ft == FileType::FILE {
                self.text2(&name);
                self.hash_loose(&mut f, tree_case);
            }
            dir.children.push(f);
        }
        dir
    }

    /// `Populate.FromAFile` for a file in the directory at `tree_case`.
    fn hash_loose(&mut self, file: &mut ScannedFile, tree_case: &str) {
        let physical = self.settings.physical_path(tree_case);
        let path = Path::new(&physical).join(&file.name);
        let mut errors = Vec::new();
        if from_a_file(file, &path, self.level, &mut errors) {
            self.abort = true;
        }
        self.report.files_hashed += 1;
        for e in errors {
            self.error(&path.to_string_lossy(), e);
        }
    }

    /// `Populate.FromAZipFileArchive` (`Populate.cs:28-66`): read the archive `db` stands for. On
    /// failure the archive's own status is set and `None` returned.
    fn open_archive(
        &mut self,
        db: &mut RvFile,
        tree_case: &str,
        cx: &Cx<'_>,
    ) -> Option<ScannedFile> {
        let physical = self.settings.physical_path(tree_case);
        let path = Path::new(&physical);
        let deep = self.level != ScanLevel::Level1;
        let result = match db.file_type {
            FileType::SEVEN_ZIP => scan_7z(path, db.file_mod_time_stamp, deep),
            _ => scan_zip(path, db.file_mod_time_stamp, deep),
        };
        self.report.archives_scanned += 1;
        match result {
            Ok(ar) => {
                db.set_zip_struct(ar.zip_struct);
                Some(ar)
            }
            Err(OpenFailure::Unreadable(why)) => {
                self.error(&physical, format!("Zip Error Opening File: {why}"));
                db.file_mod_time_stamp = i64::MIN;
                db.set_got_status(GotStatus::FILE_LOCKED, cx);
                None
            }
            Err(OpenFailure::Modified) => {
                self.error(&physical, "Zip Error File Modified".to_owned());
                db.file_mod_time_stamp = i64::MIN;
                db.set_got_status(GotStatus::FILE_LOCKED, cx);
                None
            }
            Err(OpenFailure::Corrupt(why)) => {
                self.error(&physical, why);
                db.set_got_status(GotStatus::CORRUPT, cx);
                None
            }
        }
    }

    /// `FileScanning.CheckAnArchive` (`FileScanning.cs:107-142`). `tree` is the archive's own tree
    /// name, `cx` its context (the parent's tree name).
    pub fn check_an_archive(&mut self, db: &mut RvFile, tree: &str, tree_case: &str, cx: &Cx<'_>) {
        self.text2(tree);
        let Some(mut ar) = self.open_archive(db, tree_case, cx) else {
            db.mark_as_missing(cx);
            return;
        };
        self.progress.report(ProgressEvent::Sub {
            done: 0,
            total: ar.children.len() as u64,
        });
        merge_in_archive(db, &mut ar, cx);
    }

    /// `FileScanning.CheckADir` (`FileScanning.cs:152-390`): list the directory `db` stands for
    /// and bring its children in line with it. `tree` and `tree_case` are `db`'s tree name with
    /// the names it has in the DB and the names it has on disk.
    pub fn check_a_dir(&mut self, db: &mut RvFile, tree: &str, tree_case: &str) {
        self.text2(tree);
        if self.depth >= MAX_DEPTH {
            self.error(
                tree,
                "Directories are nested too deeply; not looking further.".to_owned(),
            );
            return;
        }
        self.report.dirs_scanned += 1;

        let mut file_dir = self.list_dir(tree, tree_case);
        self.progress.report(ProgressEvent::Sub {
            done: 0,
            total: file_dir.children.len() as u64,
        });
        if self.cancel.is_cancelled() {
            self.report.cancelled = true;
            return;
        }
        file_dir.sort();

        let settings = self.settings;
        let child_cx = Cx::new(settings, tree);
        let dir_got = db.got_status;

        let (mut dbi, mut fi) = (0usize, 0usize);
        loop {
            let res = if dbi < db.children.len() && fi < file_dir.children.len() {
                let (d, f) = (&db.children[dbi], &file_dir.children[fi]);
                compare_name(d.file_type, &d.name, f.file_type, &f.name)
            } else if fi < file_dir.children.len() {
                1
            } else if dbi < db.children.len() {
                -1
            } else {
                break;
            };

            match res {
                0 => {
                    // The same name and type: pair them up, exact case first when there is
                    // more than one candidate.
                    let mut dbs = 1;
                    while dbi + dbs < db.children.len() {
                        let (a, b) = (&db.children[dbi], &db.children[dbi + dbs]);
                        if compare_name(a.file_type, &a.name, b.file_type, &b.name) != 0 {
                            break;
                        }
                        dbs += 1;
                    }
                    let mut files = 1;
                    while fi + files < file_dir.children.len() {
                        let (a, b) = (&file_dir.children[fi], &file_dir.children[fi + files]);
                        if compare_name(a.file_type, &a.name, b.file_type, &b.name) != 0 {
                            break;
                        }
                        files += 1;
                    }

                    let case_test = files > 1;
                    for index_case in (if case_test { 0u8 } else { 1u8 })..2 {
                        for f in 0..files {
                            if file_dir.children[fi + f].search_found {
                                continue;
                            }
                            // Phase 1: no more hashing.
                            let mut paired = false;
                            for d in 0..dbs {
                                if db.children[dbi + d].search_found {
                                    continue;
                                }
                                let Some(alt) = phase1(
                                    settings,
                                    &db.children[dbi + d],
                                    &file_dir.children[fi + f],
                                    self.level,
                                    index_case,
                                ) else {
                                    continue;
                                };
                                let mut sf = file_dir.children[fi + f].clone();
                                self.match_found(
                                    &mut db.children[dbi + d],
                                    &mut sf,
                                    alt,
                                    tree,
                                    tree_case,
                                    &child_cx,
                                );
                                db.children[dbi + d].search_found = true;
                                file_dir.children[fi + f].search_found = true;
                                paired = true;
                                break;
                            }
                            if paired || self.abort {
                                continue;
                            }
                            // Phase 2: hash the file and try again.
                            for d in 0..dbs {
                                if db.children[dbi + d].search_found {
                                    continue;
                                }
                                let Some(alt) = self.phase2(
                                    &db.children[dbi + d],
                                    &mut file_dir.children[fi + f],
                                    index_case,
                                    tree_case,
                                ) else {
                                    continue;
                                };
                                let mut sf = file_dir.children[fi + f].clone();
                                self.match_found(
                                    &mut db.children[dbi + d],
                                    &mut sf,
                                    alt,
                                    tree,
                                    tree_case,
                                    &child_cx,
                                );
                                db.children[dbi + d].search_found = true;
                                file_dir.children[fi + f].search_found = true;
                                break;
                            }
                        }
                    }

                    // What is left on either side.
                    for _ in 0..dbs {
                        if db.children[dbi].search_found {
                            dbi += 1;
                        } else {
                            self.db_file_not_found(db, &mut dbi, tree, dir_got, &child_cx);
                        }
                    }
                    for f in 0..files {
                        if file_dir.children[fi + f].search_found {
                            continue;
                        }
                        let mut sf = file_dir.children[fi + f].clone();
                        self.new_file_found(&mut sf, db, dbi, tree, tree_case, &child_cx);
                        dbi += 1;
                    }
                    fi += files;
                }
                1 => {
                    let mut sf = file_dir.children[fi].clone();
                    self.new_file_found(&mut sf, db, dbi, tree, tree_case, &child_cx);
                    dbi += 1;
                    fi += 1;
                }
                _ => self.db_file_not_found(db, &mut dbi, tree, dir_got, &child_cx),
            }

            if self.abort {
                return;
            }
            if self.cancel.is_cancelled() {
                self.report.cancelled = true;
                return;
            }
        }
    }

    /// `FileCompare.Phase2Test` (`Compare.cs:109-140`): hash the file now, then compare.
    fn phase2(
        &mut self,
        db: &RvFile,
        test: &mut ScannedFile,
        index_case: u8,
        tree_case: &str,
    ) -> Option<bool> {
        let same_name = if index_case == 0 {
            db.name == test.name
        } else {
            compare_name(db.file_type, &db.name, test.file_type, &test.name) == 0
        };
        if !same_name || db.file_type != FileType::FILE || test.file_type != FileType::FILE {
            return None;
        }
        self.text2(&test.name);
        self.hash_loose(test, tree_case);
        if self.abort {
            return None;
        }
        // A locked file cannot be compared: the item takes over its state.
        if test.got_status == GotStatus::FILE_LOCKED {
            return Some(false);
        }
        compare_with_alt(db, test)
    }

    /// `FileScanning.MatchFound` (`FileScanning.cs:392-434`). Returns whether it recursed.
    fn match_found(
        &mut self,
        db: &mut RvFile,
        file: &mut ScannedFile,
        alt: bool,
        dir_tree: &str,
        dir_tree_case: &str,
        cx: &Cx<'_>,
    ) -> bool {
        let tree = path_combine(dir_tree, &db.name);
        let tree_case = path_combine(dir_tree_case, &file.name);
        match db.file_type {
            FileType::ZIP | FileType::SEVEN_ZIP => {
                let settings = self.settings;
                // Only look inside an archive that changed, or when asked to look at everything.
                if db.file_mod_time_stamp != file.file_mod_time_stamp
                    || self.level == ScanLevel::Level3
                    || (self.level == ScanLevel::Level2 && !db.is_deep_scanned(settings))
                {
                    db.mark_as_missing(cx);
                    file_merge_in(db, file, false, cx);
                    self.chain.push(Ancestor::of(db, &self.compression));
                    self.check_an_archive(db, &tree, &tree_case, cx);
                    self.chain.pop();
                } else {
                    // The case of the name may have changed, nothing else.
                    db.file_check_name(&file.name);
                }
                false
            }
            FileType::DIR => {
                let mut recursed = false;
                // Directories that are rows of the tree are scanned from the top level.
                if db.tree.is_none() {
                    self.chain.push(Ancestor::of(db, &self.compression));
                    self.depth += 1;
                    self.check_a_dir(db, &tree, &tree_case);
                    self.depth -= 1;
                    self.chain.pop();
                    recursed = true;
                }
                if self.abort {
                    return true;
                }
                file_merge_in(db, file, false, cx);
                recursed
            }
            _ => {
                file_merge_in(db, file, alt, cx);
                false
            }
        }
    }

    /// `FileScanning.NewFileFound` (`FileScanning.cs:436-481`).
    fn new_file_found(
        &mut self,
        file: &mut ScannedFile,
        db_dir: &mut RvFile,
        index: usize,
        dir_tree: &str,
        dir_tree_case: &str,
        cx: &Cx<'_>,
    ) {
        let tree = path_combine(dir_tree, &file.name);
        let tree_case = path_combine(dir_tree_case, &file.name);
        match file.file_type {
            FileType::ZIP | FileType::SEVEN_ZIP => {
                file_add(db_dir, file, index, cx);
                let new = &mut db_dir.children[index];
                self.chain.push(Ancestor::of(new, &self.compression));
                self.check_an_archive(new, &tree, &tree_case, cx);
                self.chain.pop();
            }
            FileType::DIR => {
                file_add(db_dir, file, index, cx);
                let new = &mut db_dir.children[index];
                self.chain.push(Ancestor::of(new, &self.compression));
                self.depth += 1;
                self.check_a_dir(new, &tree, &tree_case);
                self.depth -= 1;
                self.chain.pop();
            }
            _ => {
                // A new loose file is always hashed, whatever the level.
                if file.got_status != GotStatus::FILE_LOCKED && !file.deep_scanned {
                    self.text2(&file.name);
                    self.hash_loose(file, dir_tree_case);
                }
                file_add(db_dir, file, index, cx);
            }
        }
    }

    /// `FileScanning.DBFileNotFound` (`FileScanning.cs:483-512`).
    fn db_file_not_found(
        &mut self,
        db_dir: &mut RvFile,
        dbi: &mut usize,
        _dir_tree: &str,
        dir_got: GotStatus,
        cx: &Cx<'_>,
    ) {
        let child = &mut db_dir.children[*dbi];
        if child.file_remove(cx, dir_got) == EFile::Delete {
            db_dir.children.remove(*dbi);
            return;
        }
        match child.file_type {
            FileType::ZIP | FileType::SEVEN_ZIP => child.mark_as_missing(cx),
            FileType::DIR if child.tree.is_none() => child.mark_as_missing(cx),
            _ => {}
        }
        *dbi += 1;
    }
}

/// `true` when `to_sort_type` marks the ToSort directory as files only.
pub(crate) fn to_sort_file_only(t: Option<ToSortDirType>) -> bool {
    t.is_some_and(|t| t.contains(ToSortDirType::FILE_ONLY))
}
