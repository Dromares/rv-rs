//! Archives (`RomVaultCore/FixFile/FixAZip.cs`, `docs/fixing.md` section 5).
//!
//! A zip is rebuilt by writing a complete replacement beside it, checking that replacement by
//! reading it back, and renaming it over the original; the original is never deleted first. What
//! leaves the archive for ToSort is written and put in place before the archive is replaced.
//!
//! What is not done (the item is left as it is and listed in [`crate::FixReport::skipped`]):
//!
//! * 7z archives are read but not written, and a zip whose structure is not TorrentZip, TDC or
//!   DTD is not rewritten (`rv-archive` has no raw-copy writer for them). They can be moved
//!   whole to ToSort or the corrupt folder, and fixed *from*.
//! * A zip with a corrupt member that must stay in it, or whose corrupt member must leave it, is
//!   rebuilt only when the original can be kept whole: the corrupt bytes cannot be re-compressed,
//!   so the whole old archive is copied into `ToSort/Corrupt` first.
//! * 3.8.0's whole-archive "ZipMove" shortcut (rename a matching archive over the target) is not
//!   ported; the archive is rebuilt instead, which gives the same result.

use std::cell::RefCell;
use std::fs;

use rv_archive::zip::{TorrentZipWriter, ZipStructure as ZipKind};
use rv_archive::AtomicFile;
use rv_core::{DatStatus, FileType, GotStatus, RepStatus, RvFile, ZipStructure};
use rv_scan::db::item::{self, ScannedFile};

use crate::dbhelper::is_zero_length;
use crate::error::{FixError, Res};
use crate::fixer::{action, io_err, Fixer};
use crate::fsx::{copy_hashing, move_new};
use crate::nodepath::NodePath;
use crate::source::{copy_failure, scanned_from_node, with_source, HashingReader};

enum Act {
    /// Not in the new archive.
    Drop,
    /// Copied from the old archive as it is.
    Keep,
    /// Made from one of the sources (or empty).
    Fix { sources: Vec<NodePath>, zero: bool },
    /// Copied to a ToSort archive and dropped.
    ToSort,
    /// Dropped; the whole old archive is kept in ToSort/Corrupt.
    Corrupt,
}

struct Mem {
    path: NodePath,
    node: RvFile,
    act: Act,
}

fn kind_of(raw: ZipStructure) -> Option<ZipKind> {
    match raw.0 {
        1 => Some(ZipKind::ZipTrrnt),
        2 => Some(ZipKind::ZipTDC),
        3 => Some(ZipKind::ZipDTD),
        _ => None,
    }
}

fn mod_time(n: &RvFile) -> Option<i64> {
    (n.file_mod_time_stamp > 0).then_some(n.file_mod_time_stamp)
}

impl Fixer<'_> {
    /// `FixAZip.FixZip` (`FixAZip.cs:27-95`).
    pub(crate) fn fix_zip(&mut self, p: &NodePath) -> Res<()> {
        let show_new_mia = self.settings.show_new_mia;
        let Some(node) = self.node(p) else {
            return Ok(());
        };
        let ds = node.dir_status(show_new_mia);
        if ds.has_unknown() {
            return Err(FixError::FindFixesInvalidStatus(format!(
                "{p} has files with no fix status."
            )));
        }
        let (dat_struct, fix_flag) = node.zip_dat_struct_value();
        let needs_trrntzipped = node.got_status == GotStatus::GOT
            && node.dat_status == DatStatus::IN_DAT_COLLECT
            && fix_flag
            && node.zip_struct.unwrap_or(ZipStructure::NONE) != dat_struct;
        let (got, dat, case_differs) =
            (node.got_status, node.dat_status, !node.file_name.is_empty());

        if got == GotStatus::CORRUPT
            && dat != DatStatus::IN_TO_SORT
            && !ds.fix_check_files_can_be_fixed()
        {
            return self.move_zip_to_corrupt(p);
        }

        if !ds.fix_check_files_can_be_fixed() {
            if ds.fix_check_has_needed_for_fix() {
                // Rebuilt once everything that is needed from it has been used.
                return Ok(());
            }
            if !ds.fix_check_has_files_to_be_removed() && !needs_trrntzipped {
                if case_differs {
                    self.rename_archive_case(p)?;
                }
                return Ok(());
            }
        }
        if needs_trrntzipped
            && !ds.fix_check_files_can_be_fixed()
            && !ds.fix_check_has_files_to_be_removed()
        {
            self.row(action::RETORRENTZIP, p, kind_name(dat_struct));
        }

        self.check_create_directories(&p.parent())?;
        if got == GotStatus::NOT_GOT && self.phys(p).exists() {
            return Err(FixError::RescanNeeded(format!(
                "{} is on disk but the database says it is missing.",
                self.phys(p).display()
            )));
        }
        if self.try_move_archive_to_sort(p)? {
            return Ok(());
        }
        self.rebuild_archive(p, dat_struct)
    }

    fn rename_archive_case(&mut self, p: &NodePath) -> Res<()> {
        let from = self.phys(p);
        let to = from.with_file_name(&p.last().expect("an archive has a name").name);
        move_new(&from, &to, &self.opts.failpoints)?;
        self.row(action::RENAME, p, format!("-> {}", to.display()));
        let ticks = fs::metadata(&to)
            .map(|m| rv_core::time::ticks_of(&m))
            .unwrap_or(0);
        if let Some(n) = self.node_mut(p) {
            n.file_name.clear();
            n.file_mod_time_stamp = ticks;
        }
        Ok(())
    }

    /// `FixAZipMove.CheckFileMoveToSort` (`FixAZipMove.cs:226-288`): an archive in which nothing
    /// is wanted goes to ToSort as it is, not re-compressed.
    fn try_move_archive_to_sort(&mut self, p: &NodePath) -> Res<bool> {
        let Some(node) = self.node(p) else {
            return Ok(false);
        };
        let mut any = false;
        for c in &node.children {
            match c.rep_status {
                RepStatus::MOVE_TO_SORT => any = true,
                RepStatus::MISSING | RepStatus::NOT_COLLECTED => {}
                _ => return Ok(false),
            }
        }
        if !any {
            return Ok(false);
        }
        self.check_unchanged(p)?;
        let full = node.clone();
        let (dir, filename) = self.create_to_sort_dirs(p)?;
        let from = self.phys(p);
        let to = self.phys(&dir).join(&filename);
        self.row(action::ARCHIVE_TO_SORT, p, format!("-> {}", to.display()));
        move_new(&from, &to, &self.opts.failpoints)?;
        self.fail("after_rename")?;
        let mut sf = scanned_from_node(&full, &filename);
        sf.children
            .retain(|c| c.got_status == GotStatus::GOT || c.got_status == GotStatus::CORRUPT);
        self.add_archive_record(&dir, &mut sf);
        self.cache.clear();
        self.with_cx(p, |n, cx| n.mark_as_missing(cx));
        self.check_delete_file(p);
        Ok(true)
    }

    /// Put an archive that was just written (or moved) into `dir` of the tree, and its entries
    /// into the groups.
    fn add_archive_record(&mut self, dir: &NodePath, sf: &mut ScannedFile) {
        let tree = dir.tree_name();
        let settings = self.settings;
        let cx = rv_core::Cx::new(settings, &tree);
        if let Some(parent) = self.node_mut(dir) {
            item::add_archive(parent, sf, &cx);
        }
        let ap = dir.child(sf.file_type, &sf.name);
        let members: Vec<(NodePath, RvFile)> = self
            .node(&ap)
            .map(|a| {
                a.children
                    .iter()
                    .map(|c| (ap.child(c.file_type, &c.name), c.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for (mp, m) in members {
            self.groups.add_new(&m, &mp);
        }
    }

    /// `FixAZipFunctions.MoveZipToCorrupt` (`FixAZipFunctions.cs:97-163`).
    fn move_zip_to_corrupt(&mut self, p: &NodePath) -> Res<()> {
        self.check_unchanged(p)?;
        let node = self.leaf(p)?;
        let dir = self.corrupt_dir()?;
        let name = self.free_name(&dir, node.name_case());
        let from = self.phys(p);
        let to = self.phys(&dir).join(&name);
        self.row(
            action::ARCHIVE_TO_CORRUPT,
            p,
            format!("-> {}", to.display()),
        );
        move_new(&from, &to, &self.opts.failpoints)?;
        self.fail("after_rename")?;
        self.add_corrupt_archive_record(&dir, &name, &node, &to);
        self.cache.clear();
        self.with_cx(p, |n, cx| n.mark_as_missing(cx));
        self.check_delete_file(p);
        Ok(())
    }

    /// An archive that is in ToSort/Corrupt: read as far as it reads, else a corrupt file.
    fn add_corrupt_archive_record(
        &mut self,
        dir: &NodePath,
        name: &str,
        like: &RvFile,
        at: &std::path::Path,
    ) {
        match item::read_archive(at, name, like.file_type) {
            Ok(mut sf) => self.add_archive_record(dir, &mut sf),
            Err(_) => {
                let mut sf = ScannedFile::new(like.file_type, name);
                sf.got_status = GotStatus::CORRUPT;
                sf.file_mod_time_stamp = fs::metadata(at)
                    .map(|m| rv_core::time::ticks_of(&m))
                    .unwrap_or(0);
                sf.size = fs::metadata(at).ok().map(|m| m.len());
                let tree = dir.tree_name();
                let settings = self.settings;
                let cx = rv_core::Cx::new(settings, &tree);
                if let Some(parent) = self.node_mut(dir) {
                    item::add_file(parent, &sf, &cx);
                }
            }
        }
    }

    /// The general case of `FixAZip.FixZip` (`FixAZip.cs:158-533`).
    fn rebuild_archive(&mut self, p: &NodePath, dat_struct: ZipStructure) -> Res<()> {
        let archive = self
            .node(p)
            .ok_or_else(|| FixError::Logic(format!("{p} is not in the tree")))?
            .clone();
        if archive.file_type != FileType::ZIP {
            self.skip(p, "7z archives are not rewritten");
            return Ok(());
        }
        let own = if matches!(
            archive.dat_status,
            DatStatus::NOT_IN_DAT | DatStatus::IN_TO_SORT
        ) {
            archive.zip_struct.unwrap_or(ZipStructure::NONE)
        } else {
            dat_struct
        };
        let Some(structure) = kind_of(own) else {
            self.skip(
                p,
                format!("archives of structure {} are not rewritten", kind_name(own)),
            );
            return Ok(());
        };

        // ---- plan: what happens to each entry. Nothing is written yet. ----
        let mut mems: Vec<Mem> = Vec::new();
        let mut used: Vec<NodePath> = Vec::new();
        for child in &archive.children {
            let cp = p.child(child.file_type, &child.name);
            let mut leaf = child.clone();
            leaf.children.clear();
            let act = match child.rep_status {
                RepStatus::MISSING
                | RepStatus::NOT_COLLECTED
                | RepStatus::RENAME
                | RepStatus::INCOMPLETE => Act::Drop,
                RepStatus::DELETE => {
                    self.double_check_delete(&cp)?;
                    Act::Drop
                }
                RepStatus::CORRECT
                | RepStatus::IN_TO_SORT
                | RepStatus::NEEDED_FOR_FIX
                | RepStatus::CORRUPT => {
                    if child.got_status != GotStatus::GOT {
                        self.skip(
                            p,
                            format!(
                                "{} is corrupt and cannot be copied into a new archive",
                                child.name
                            ),
                        );
                        return Ok(());
                    }
                    Act::Keep
                }
                RepStatus::CAN_BE_FIXED | RepStatus::CORRUPT_CAN_BE_FIXED => {
                    if is_zero_length(child) {
                        Act::Fix {
                            sources: Vec::new(),
                            zero: true,
                        }
                    } else {
                        let sources = self.fix_file_list(&cp);
                        if sources.is_empty() {
                            // It looked fixable, and the file that would have fixed it is gone.
                            Act::Drop
                        } else {
                            used.extend(sources.iter().cloned());
                            Act::Fix {
                                sources,
                                zero: false,
                            }
                        }
                    }
                }
                RepStatus::MOVE_TO_SORT => {
                    used.extend(self.used_by(&cp));
                    Act::ToSort
                }
                RepStatus::MOVE_TO_CORRUPT => Act::Corrupt,
                other => {
                    return Err(FixError::Logic(format!(
                        "{cp} has status {}, which Fix does not know how to fix.",
                        other.name()
                    )))
                }
            };
            mems.push(Mem {
                path: cp,
                node: leaf,
                act,
            });
        }
        let preserve_original = archive.got_status == GotStatus::CORRUPT
            || mems.iter().any(|m| {
                m.node.got_status == GotStatus::CORRUPT
                    && matches!(m.act, Act::Fix { .. } | Act::Corrupt)
            });

        let keeps_any = mems
            .iter()
            .any(|m| matches!(m.act, Act::Keep | Act::Fix { .. }));
        let sorts_any = mems.iter().any(|m| matches!(m.act, Act::ToSort));
        let name = p.last().expect("an archive has a name").name.clone();
        let orig = self.phys(p);
        let final_path = orig.with_file_name(&name);
        if archive.got_status != GotStatus::NOT_GOT {
            self.check_unchanged(p)?;
        }

        // ---- write the replacement and the ToSort archive to temp files ----
        let mut main: Option<TorrentZipWriter<AtomicFile>> = if keeps_any {
            let at = AtomicFile::create(&final_path).map_err(|e| io_err(e, &final_path))?;
            Some(
                TorrentZipWriter::new(at, structure)
                    .map_err(|e| FixError::FileSystem(e.to_string()))?,
            )
        } else {
            None
        };
        let mut sort: Option<(NodePath, String, TorrentZipWriter<AtomicFile>)> = None;
        if sorts_any {
            let (dir, filename) = self.create_to_sort_dirs(p)?;
            let to = self.phys(&dir).join(&filename);
            let at = AtomicFile::create(&to).map_err(|e| io_err(e, &to))?;
            let kind = kind_of(archive.zip_struct.unwrap_or(ZipStructure::NONE))
                .unwrap_or(ZipKind::ZipTrrnt);
            let w =
                TorrentZipWriter::new(at, kind).map_err(|e| FixError::FileSystem(e.to_string()))?;
            sort = Some((dir, filename, w));
        }
        let mut fixed_here = 0u64;
        for m in &mems {
            if self.cancel.is_cancelled() {
                return Err(FixError::Cancelled);
            }
            match &m.act {
                Act::Drop | Act::Corrupt => {}
                Act::Keep => {
                    let w = main.as_mut().expect("a keeper needs the new archive");
                    self.add_member(w, &m.node, &m.node.name, &m.path)?;
                }
                Act::Fix { sources, zero } => {
                    let w = main.as_mut().expect("a fix needs the new archive");
                    if *zero {
                        w.add_bytes(&m.node.name, &[], mod_time(&m.node))
                            .map_err(|e| FixError::FileSystem(e.to_string()))?;
                        self.row(action::EMPTY, &m.path, "");
                    } else {
                        let src = self
                            .best_source(sources)
                            .ok_or_else(|| FixError::Logic(format!("{} has no source", m.path)))?;
                        self.row(action::COPY, &m.path, format!("<-- {src}"));
                        self.add_member(w, &m.node, &m.node.name, &src)?;
                    }
                    fixed_here += 1;
                }
                Act::ToSort => {
                    let (_, _, w) = sort.as_mut().expect("opened above");
                    self.row(action::TO_SORT, &m.path, "");
                    self.add_member(w, &m.node, &m.node.name, &m.path)?;
                }
            }
        }

        // ---- finish, sync, read back and check ----
        let mut main_at = match main {
            Some(w) => Some(
                w.finish()
                    .map_err(|e| FixError::FileSystem(e.to_string()))?,
            ),
            None => None,
        };
        let mut sort_at = match sort {
            Some((dir, filename, w)) => Some((
                dir,
                filename,
                w.finish()
                    .map_err(|e| FixError::FileSystem(e.to_string()))?,
            )),
            None => None,
        };
        self.fail("after_temp_write")?;
        if let Some(at) = &mut main_at {
            at.sync().map_err(|e| io_err(e, at.tmp_path()))?;
        }
        if let Some((_, _, at)) = &mut sort_at {
            at.sync().map_err(|e| io_err(e, at.tmp_path()))?;
        }
        self.fail("after_fsync")?;

        let mut main_sf: Option<ScannedFile> = None;
        if let Some(at) = &main_at {
            let expected: Vec<&RvFile> = mems
                .iter()
                .filter(|m| matches!(m.act, Act::Keep | Act::Fix { .. }))
                .map(|m| &m.node)
                .collect();
            let sf = self.read_back_archive(at.tmp_path(), &name, p, &expected, Some(structure))?;
            main_sf = Some(sf);
        }
        let mut sort_sf: Option<ScannedFile> = None;
        if let Some((_, filename, at)) = &sort_at {
            let expected: Vec<&RvFile> = mems
                .iter()
                .filter(|m| matches!(m.act, Act::ToSort))
                .map(|m| &m.node)
                .collect();
            let sf = self.read_back_archive(at.tmp_path(), filename, p, &expected, None)?;
            sort_sf = Some(sf);
        }
        // The old archive, whole, for a corrupt entry's sake.
        let mut keep_at: Option<(NodePath, String, AtomicFile)> = None;
        if preserve_original {
            let dir = self.corrupt_dir()?;
            let cname = self.free_name(&dir, archive.name_case());
            let to = self.phys(&dir).join(&cname);
            let mut at = AtomicFile::create(&to).map_err(|e| io_err(e, &to))?;
            let mut input = fs::File::open(&orig).map_err(|e| io_err(e, &orig))?;
            let mut h = rv_scan::HashSet::new(rv_scan::HashKinds::ROMVAULT);
            copy_hashing(&mut input, &mut at, &mut h).map_err(|e| io_err(e, &to))?;
            at.sync().map_err(|e| io_err(e, &to))?;
            keep_at = Some((dir, cname, at));
        }

        // ---- put things in place: what leaves first, then the replacement ----
        // The mutation test: 3.8.0 deletes the old archive here, before the new one is renamed in
        // (`FixAZip.cs:375-445`, then `:453-481`).
        #[cfg(feature = "mutation-3-8-0-order")]
        if main_at.is_some() {
            fs::remove_file(&orig).map_err(|e| io_err(e, &orig))?;
        }
        self.fail("before_rename")?;
        if let (Some((dir, filename, at)), Some(mut sf)) = (sort_at.take(), sort_sf.take()) {
            let to = self.phys(&dir).join(&filename);
            at.commit_new().map_err(|e| io_err(e, &to))?;
            self.fail("after_rename")?;
            self.add_archive_record(&dir, &mut sf);
        }
        if let Some((dir, cname, at)) = keep_at.take() {
            let to = self.phys(&dir).join(&cname);
            at.commit_new().map_err(|e| io_err(e, &to))?;
            self.row(
                action::ARCHIVE_TO_CORRUPT,
                p,
                format!("copy -> {}", to.display()),
            );
            self.add_corrupt_archive_record(&dir, &cname, &archive, &to);
        }

        let kept: Vec<(String, RepStatus)> = mems
            .iter()
            .filter(|m| {
                matches!(m.act, Act::Keep) && m.node.rep_status == RepStatus::NEEDED_FOR_FIX
            })
            .map(|m| (m.node.name.clone(), m.node.rep_status))
            .collect();
        let old_names: Vec<String> = mems.iter().map(|m| m.node.name.clone()).collect();

        match (main_at.take(), main_sf.take()) {
            (Some(at), Some(mut sf)) => {
                at.commit().map_err(|e| io_err(e, &final_path))?;
                self.cache.clear();
                self.fail("after_rename")?;
                if orig != final_path && orig.exists() && !same_file(&orig, &final_path) {
                    // The old name differed only in case: the new archive has the DAT's.
                    fs::remove_file(&orig).map_err(|e| io_err(e, &orig))?;
                    rv_archive::sync_dir(&orig).map_err(|e| io_err(e, &orig))?;
                }
                self.with_cx(p, |n, cx| item::merge_archive(n, &mut sf, cx));
                self.after_rebuild(p, &kept, &old_names);
            }
            _ => {
                // Everything in the old archive has gone somewhere else or is a proven duplicate.
                self.fail("before_source_delete")?;
                fs::remove_file(&orig).map_err(|e| io_err(e, &orig))?;
                rv_archive::sync_dir(&orig).map_err(|e| io_err(e, &orig))?;
                self.cache.clear();
                self.with_cx(p, |n, cx| n.mark_as_missing(cx));
                self.check_delete_file(p);
            }
        }

        let detail = format!(
            "{} fixed, {} to ToSort",
            fixed_here,
            mems.iter().filter(|m| matches!(m.act, Act::ToSort)).count()
        );
        self.row(action::REBUILD, p, detail);
        self.report.fixed += fixed_here;
        self.check_files_used_for_fix(&used, false);
        Ok(())
    }

    /// After the new archive is in place and merged: statuses a merge resets, and the groups.
    fn after_rebuild(&mut self, p: &NodePath, kept: &[(String, RepStatus)], old_names: &[String]) {
        for (name, st) in kept {
            let cp = p.child(FileType::FILE_ZIP, name);
            if self.node(&cp).is_some() {
                self.set_status(&cp, *st);
            }
        }
        let members: Vec<(NodePath, RvFile)> = self
            .node(p)
            .map(|a| {
                a.children
                    .iter()
                    .map(|c| (p.child(c.file_type, &c.name), c.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for name in old_names {
            let cp = p.child(FileType::FILE_ZIP, name);
            if self.node(&cp).is_none() {
                self.groups.forget(&cp);
            }
        }
        for (mp, m) in members {
            if self.groups.group_of(&mp).is_none() && m.got_status == GotStatus::GOT {
                self.groups.add_new(&m, &mp);
            }
        }
    }

    /// Copy one entry from `src` into the writer, hashing it on the way and checking it against
    /// what the database knows about the source.
    fn add_member(
        &mut self,
        w: &mut TorrentZipWriter<AtomicFile>,
        want: &RvFile,
        name: &str,
        src: &NodePath,
    ) -> Res<()> {
        let src_node = self.leaf(src)?;
        let failure = RefCell::new(None);
        let res = with_source(
            &self.db.root,
            self.settings,
            &mut self.cache,
            src,
            |r, size| {
                let mut hr = HashingReader::new(r, self.cancel, &failure);
                match w.add_file(name, size, &mut hr, mod_time(want), None) {
                    Ok(_) => Ok((hr.hash.finish(), size)),
                    Err(e) => Err(copy_failure(&failure, &src.tree_name(), e.to_string())),
                }
            },
        );
        let (digests, size) = match res {
            Ok(v) => v,
            Err(e) => {
                if matches!(e, FixError::SourceDataStreamCorrupt(_)) {
                    self.mark_source_corrupt(src);
                }
                return Err(e);
            }
        };
        let bad = src_node.size.is_some_and(|s| s != size)
            || src_node
                .crc
                .as_deref()
                .zip(digests.crc.as_ref())
                .is_some_and(|(a, b)| a != &b[..]);
        if bad {
            self.mark_source_corrupt(src);
            return Err(FixError::SourceDataStreamCorrupt(format!(
                "{src} did not read back as the database says."
            )));
        }
        Ok(())
    }

    /// Read a finished temp archive back and check that it holds exactly `expected`, each entry
    /// with the values the tree wants for it, in the structure asked for.
    fn read_back_archive(
        &self,
        tmp: &std::path::Path,
        name: &str,
        p: &NodePath,
        expected: &[&RvFile],
        structure: Option<ZipKind>,
    ) -> Res<ScannedFile> {
        let sf = item::read_archive(tmp, name, FileType::ZIP)
            .map_err(|e| FixError::DestinationCheckSumMismatch(format!("{p}: {e}")))?;
        if let Some(s) = structure {
            if sf.zip_struct.0 != s as u8 {
                return Err(FixError::DestinationCheckSumMismatch(format!(
                    "{p}: the new archive is not a {}.",
                    s.name()
                )));
            }
        }
        if sf.children.len() != expected.len() {
            return Err(FixError::DestinationCheckSumMismatch(format!(
                "{p}: the new archive has {} entries, {} were expected.",
                sf.children.len(),
                expected.len()
            )));
        }
        for want in expected {
            let found = sf.children.iter().find(|c| c.name == want.name);
            let ok = found.is_some_and(|c| {
                c.got_status == GotStatus::GOT && item::matches(want, c).is_some()
            });
            if !ok {
                return Err(FixError::DestinationCheckSumMismatch(format!(
                    "{p}: {} in the new archive is not what the DAT lists.",
                    want.name
                )));
            }
        }
        Ok(sf)
    }
}

/// Are the two paths the same file (a case-insensitive file system gives one file two names)?
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

fn kind_name(s: ZipStructure) -> &'static str {
    kind_of(s).map_or("Unstructured", |k| k.name())
}
