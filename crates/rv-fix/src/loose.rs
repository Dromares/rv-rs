//! Loose files (`RomVaultCore/FixFile/FixAFile.cs`, `docs/fixing.md` section 4).

use std::cell::RefCell;
use std::fs;
use std::io::{self, Write};

use rv_archive::AtomicFile;
use rv_core::{FileType, GotStatus, RepStatus};
use rv_scan::db::item;

use crate::dbhelper::is_zero_length;
use crate::error::{FixError, Res};
use crate::fixer::{action, io_err, Fixer, READ_BACK};
use crate::fsx::move_new;
use crate::nodepath::NodePath;
use crate::source::{copy_failure, scanned_from_node, with_source, HashingReader};

impl Fixer<'_> {
    /// `FixAFile.FixFile` (`FixAFile.cs:17-86`).
    pub(crate) fn fix_file(&mut self, p: &NodePath) -> Res<()> {
        match self.status(p) {
            RepStatus::UNKNOWN => Err(FixError::FindFixesInvalidStatus(format!(
                "{p} has no fix status."
            ))),
            RepStatus::UN_SCANNED
            | RepStatus::MISSING
            | RepStatus::INCOMPLETE
            | RepStatus::NOT_COLLECTED
            | RepStatus::IGNORE
            | RepStatus::IN_TO_SORT
            | RepStatus::NEEDED_FOR_FIX
            | RepStatus::CORRUPT
            | RepStatus::RENAME => Ok(()),
            RepStatus::CORRECT => self.fix_file_check_name(p),
            RepStatus::DELETE => self.fix_file_delete(p),
            RepStatus::MOVE_TO_SORT => self.fix_file_move_to_sort(p),
            RepStatus::MOVE_TO_CORRUPT => self.fix_file_move_to_corrupt(p),
            RepStatus::CAN_BE_FIXED | RepStatus::CORRUPT_CAN_BE_FIXED => {
                self.fix_file_can_be_fixed(p)
            }
            other => Err(FixError::Logic(format!(
                "{p} has status {}, which Fix does not know how to fix.",
                other.name()
            ))),
        }
    }

    /// A correct file whose name differs in case from the DAT's takes the DAT's.
    fn fix_file_check_name(&mut self, p: &NodePath) -> Res<()> {
        let case_differs = self.node(p).is_some_and(|n| !n.file_name.is_empty());
        if !case_differs {
            return Ok(());
        }
        let from = self.phys(p);
        let to = from.with_file_name(&p.last().expect("a file has a name").name);
        move_new(&from, &to, &self.opts.failpoints)?;
        self.row(
            action::RENAME,
            p,
            format!("{} -> {}", from.display(), to.display()),
        );
        if let Some(n) = self.node_mut(p) {
            n.file_name.clear();
        }
        Ok(())
    }

    /// `FixFileDelete` (`FixAFile.cs:115-150`).
    pub(crate) fn fix_file_delete(&mut self, p: &NodePath) -> Res<()> {
        self.double_check_delete(p)?;
        let phys = self.phys(p);
        if phys.exists() {
            self.row(action::DELETE, p, "");
            self.fail("before_source_delete")?;
            let mut perms = fs::metadata(&phys)
                .map_err(|e| io_err(e, &phys))?
                .permissions();
            if perms.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                let _ = fs::set_permissions(&phys, perms);
            }
            fs::remove_file(&phys).map_err(|e| io_err(e, &phys))?;
            rv_archive::sync_dir(&phys).map_err(|e| io_err(e, &phys))?;
        }
        self.check_delete_file(p);
        Ok(())
    }

    /// `FixFileMoveToSort` (`FixAFile.cs:152-193`).
    fn fix_file_move_to_sort(&mut self, p: &NodePath) -> Res<()> {
        self.check_unchanged(p)?;
        let src_node = self.leaf(p)?;
        let (dir, filename) = self.create_to_sort_dirs(p)?;
        let from = self.phys(p);
        let to = self.phys(&dir).join(&filename);
        self.row(action::TO_SORT, p, format!("-> {}", to.display()));
        move_new(&from, &to, &self.opts.failpoints)?;
        self.fail("after_rename")?;
        self.add_loose_copy(&dir, &filename, &src_node, GotStatus::GOT);
        self.check_delete_file(p);
        Ok(())
    }

    /// `FixFileMoveToCorrupt` (`FixAFile.cs:195-251`).
    pub(crate) fn fix_file_move_to_corrupt(&mut self, p: &NodePath) -> Res<()> {
        self.check_unchanged(p)?;
        let src_node = self.leaf(p)?;
        let dir = self.corrupt_dir()?;
        let name = self.free_name(&dir, src_node.name_case());
        let from = self.phys(p);
        let to = self.phys(&dir).join(&name);
        self.row(action::TO_CORRUPT, p, format!("-> {}", to.display()));
        move_new(&from, &to, &self.opts.failpoints)?;
        self.fail("after_rename")?;
        self.add_loose_copy(&dir, &name, &src_node, GotStatus::CORRUPT);
        self.check_delete_file(p);
        Ok(())
    }

    /// `FixFileCanBeFixed` (`FixAFile.cs:385-499`): make a missing (or corrupt) file from one of
    /// the files that have its data.
    fn fix_file_can_be_fixed(&mut self, p: &NodePath) -> Res<()> {
        self.check_create_directories(&p.parent())?;
        let target = self.leaf(p)?;
        let dest = self.phys(p);

        // A corrupt file already there is kept, whole, in ToSort/Corrupt: fixing replaces it.
        if dest.exists() {
            if target.got_status != GotStatus::CORRUPT {
                return Err(FixError::RescanNeeded(format!(
                    "{} is on disk but the database says it is missing.",
                    dest.display()
                )));
            }
            self.fix_file_move_to_corrupt_in_place(p)?;
        }

        if is_zero_length(&target) {
            self.write_empty_file(p)?;
            let used = self.used_by(p);
            self.check_files_used_for_fix(&used, true);
            self.report.fixed += 1;
            return Ok(());
        }

        let list = self.fix_file_list(p);
        let Some(src) = self.best_source(&list) else {
            return Err(FixError::RescanNeeded(format!(
                "{p} has no source any more."
            )));
        };
        if self.can_move_source(&src) {
            self.row(action::MOVE, p, format!("<-- {src}"));
            self.move_into_place(&src, p)?;
        } else {
            self.row(action::COPY, p, format!("<-- {src}"));
            self.copy_into_place(&src, p)?;
        }
        let mut used = list;
        used.push(src);
        self.check_files_used_for_fix(&used, true);
        self.report.fixed += 1;
        Ok(())
    }

    /// Every file of the group of `p` (they all count as used when `p` is made).
    pub(crate) fn used_by(&self, p: &NodePath) -> Vec<NodePath> {
        self.groups
            .group_of(p)
            .map(|g| {
                self.groups
                    .group(g)
                    .files
                    .iter()
                    .filter(|f| *f != p)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The corrupt file at `p`'s own place goes to ToSort/Corrupt and `p` becomes a missing file
    /// that can be fixed.
    fn fix_file_move_to_corrupt_in_place(&mut self, p: &NodePath) -> Res<()> {
        self.check_unchanged(p)?;
        let src_node = self.leaf(p)?;
        let dir = self.corrupt_dir()?;
        let name = self.free_name(&dir, src_node.name_case());
        let from = self.phys(p);
        let to = self.phys(&dir).join(&name);
        self.row(action::TO_CORRUPT, p, format!("-> {}", to.display()));
        move_new(&from, &to, &self.opts.failpoints)?;
        self.add_loose_copy(&dir, &name, &src_node, GotStatus::CORRUPT);
        // The wanted file is missing again.
        let parent_got = self
            .node(&p.parent())
            .map_or(GotStatus::NOT_GOT, |n| n.got_status);
        self.with_cx(p, |n, cx| {
            n.file_remove(cx, parent_got);
        });
        self.set_status(p, RepStatus::CAN_BE_FIXED);
        Ok(())
    }

    /// An empty file, which needs no source.
    fn write_empty_file(&mut self, p: &NodePath) -> Res<()> {
        self.row(action::EMPTY, p, "");
        let dest = self.phys(p);
        let target = self.leaf(p)?;
        let mut at = AtomicFile::create(&dest).map_err(|e| io_err(e, &dest))?;
        at.flush().map_err(|e| io_err(e, &dest))?;
        self.finish_loose(&mut at, &target, p)
            .and_then(|(sf, alt)| self.commit_loose(at, p, sf, alt))
    }

    /// Copy the data of `src` into a temp file next to `p`, check it, and put it in place.
    fn copy_into_place(&mut self, src: &NodePath, p: &NodePath) -> Res<()> {
        let dest = self.phys(p);
        let target = self.leaf(p)?;
        let mut at = AtomicFile::create(&dest).map_err(|e| io_err(e, &dest))?;
        let failure = RefCell::new(None);
        let src_node = self.leaf(src)?;
        let copied = with_source(
            &self.db.root,
            self.settings,
            &mut self.cache,
            src,
            |r, _size| {
                let mut hr = HashingReader::new(r, self.cancel, &failure);
                let res = io::copy(&mut hr, &mut at);
                match res {
                    Ok(n) => Ok((hr.hash.finish(), n)),
                    Err(e) => Err(copy_failure(&failure, &src.tree_name(), e.to_string())),
                }
            },
        );
        let (digests, written) = match copied {
            Ok(v) => v,
            Err(e) => {
                if matches!(e, FixError::SourceDataStreamCorrupt(_)) {
                    self.mark_source_corrupt(src);
                }
                return Err(e);
            }
        };
        // The source must be what the database says before anything is believed about the copy.
        if src_node.size.is_some_and(|s| s != written)
            || src_node
                .crc
                .as_deref()
                .zip(digests.crc.as_ref())
                .is_some_and(|(a, b)| a != &b[..])
        {
            self.mark_source_corrupt(src);
            return Err(FixError::SourceDataStreamCorrupt(format!(
                "{src} did not read back as the database says."
            )));
        }
        self.fail("after_temp_write")?;
        let (sf, alt) = self.finish_loose(&mut at, &target, p)?;
        self.commit_loose(at, p, sf, alt)
    }

    /// Sync the temp file, read it back, and check it against what the DAT wants.
    fn finish_loose(
        &mut self,
        at: &mut AtomicFile,
        target: &rv_core::RvFile,
        p: &NodePath,
    ) -> Res<(rv_scan::db::item::ScannedFile, bool)> {
        at.sync().map_err(|e| io_err(e, at.tmp_path()))?;
        self.fail("after_fsync")?;
        let name = p.last().expect("a file has a name").name.clone();
        let sf = item::read_loose_file(at.tmp_path(), &name, READ_BACK)
            .map_err(|e| FixError::DestinationCheckSumMismatch(format!("{p}: {e}")))?;
        let Some(alt) = item::matches(target, &sf) else {
            return Err(FixError::DestinationCheckSumMismatch(format!(
                "{p}: what was written is not what the DAT lists."
            )));
        };
        Ok((sf, alt))
    }

    /// Rename the checked temp file into place and tell the tree.
    fn commit_loose(
        &mut self,
        at: AtomicFile,
        p: &NodePath,
        sf: rv_scan::db::item::ScannedFile,
        alt: bool,
    ) -> Res<()> {
        self.fail("before_rename")?;
        let dest = self.phys(p);
        at.commit_new().map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                FixError::RescanNeeded(format!(
                    "{} appeared while it was being made.",
                    dest.display()
                ))
            } else {
                io_err(e, &dest)
            }
        })?;
        self.fail("after_rename")?;
        self.with_cx(p, |n, cx| item::merge_file(n, &sf, alt, cx));
        Ok(())
    }

    /// Move the loose file `src` to the place of `p` (`MoveFile`, `MoveFile.cs:13-78`): the data
    /// is not read, the database and the time stamp vouch for it.
    fn move_into_place(&mut self, src: &NodePath, p: &NodePath) -> Res<()> {
        self.check_unchanged(src)?;
        let src_node = self.leaf(src)?;
        let target = self.leaf(p)?;
        let name = p.last().expect("a file has a name").name.clone();
        let mut sf = scanned_from_node(&src_node, &name);
        sf.file_type = FileType::FILE;
        sf.got_status = GotStatus::GOT;
        let Some(alt) = item::matches(&target, &sf) else {
            return Err(FixError::Logic(format!("{src} does not match {p}.")));
        };
        let from = self.phys(src);
        let to = self.phys(p);
        move_new(&from, &to, &self.opts.failpoints)?;
        self.fail("after_rename")?;
        self.with_cx(p, |n, cx| item::merge_file(n, &sf, alt, cx));
        self.check_delete_file(src);
        Ok(())
    }
}
