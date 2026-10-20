//! Reading the data of a file the tree says we have, safely: the file must still be what the last
//! scan saw, and what comes out is hashed.

use std::cell::RefCell;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::PathBuf;

use rv_archive::sevenzip::SevenZipArchive;
use rv_archive::zip::ZipArchive;
use rv_core::progress::CancelFlag;
use rv_core::time::ticks_of;
use rv_core::{FileStatus, FileType, GotStatus, HeaderFileType, RvFile, Settings};
use rv_scan::db::item::ScannedFile;
use rv_scan::{HashKinds, HashSet};

use crate::error::{FixError, Res};
use crate::nodepath::{get, physical, NodePath};

type ZipFile = ZipArchive<BufReader<File>>;
type SevenZ = SevenZipArchive<BufReader<File>>;

/// The archive that was opened last, so the members of one archive do not open it again each.
#[derive(Default)]
pub struct SourceCache {
    zip: Option<(PathBuf, ZipFile)>,
    sevenz: Option<(PathBuf, SevenZ)>,
}

impl SourceCache {
    /// Forget everything (an archive on disk was replaced).
    pub fn clear(&mut self) {
        self.zip = None;
        self.sevenz = None;
    }
}

/// A reader that hashes what passes through it, stops when the run is cancelled, and remembers
/// that a read failed (so a failed *write* by the zip writer is not mistaken for a bad source).
pub struct HashingReader<'a> {
    inner: &'a mut dyn Read,
    pub hash: HashSet,
    pub bytes: u64,
    cancel: &'a CancelFlag,
    pub failure: &'a RefCell<Option<ReadFailure>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadFailure {
    Cancelled,
    Source(String),
}

impl<'a> HashingReader<'a> {
    pub fn new(
        inner: &'a mut dyn Read,
        cancel: &'a CancelFlag,
        failure: &'a RefCell<Option<ReadFailure>>,
    ) -> HashingReader<'a> {
        HashingReader {
            inner,
            hash: HashSet::new(HashKinds::ROMVAULT),
            bytes: 0,
            cancel,
            failure,
        }
    }
}

impl Read for HashingReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            *self.failure.borrow_mut() = Some(ReadFailure::Cancelled);
            return Err(io::Error::other("cancelled"));
        }
        match self.inner.read(buf) {
            Ok(n) => {
                self.hash.update(&buf[..n]);
                self.bytes += n as u64;
                Ok(n)
            }
            Err(e) => {
                *self.failure.borrow_mut() = Some(ReadFailure::Source(e.to_string()));
                Err(e)
            }
        }
    }
}

/// Turn what a copy through a [`HashingReader`] reported into the error to return.
pub fn copy_failure(failure: &RefCell<Option<ReadFailure>>, what: &str, other: String) -> FixError {
    match failure.borrow().clone() {
        Some(ReadFailure::Cancelled) => FixError::Cancelled,
        Some(ReadFailure::Source(why)) => {
            FixError::SourceDataStreamCorrupt(format!("{what}: {why}"))
        }
        None => FixError::FileSystem(format!("{what}: {other}")),
    }
}

/// Open the data of the file at `src` and give it to `f` with its size. The container must still
/// have the time stamp (and a loose file the length) that the database recorded; if not, the
/// answer is "scan again" (`OpenInputStream`, `CopyFile.cs:436-554`).
pub fn with_source<T>(
    root: &RvFile,
    settings: &Settings,
    cache: &mut SourceCache,
    src: &NodePath,
    f: impl FnOnce(&mut dyn Read, u64) -> Res<T>,
) -> Res<T> {
    let node =
        get(root, src).ok_or_else(|| FixError::Logic(format!("{src} is not in the tree")))?;
    if node.got_status != GotStatus::GOT {
        return Err(FixError::RescanNeeded(format!(
            "{src} is not a file we have."
        )));
    }
    match node.file_type {
        FileType::FILE => {
            let path = physical(root, settings, src);
            let meta = fs::metadata(&path)
                .map_err(|_| FixError::RescanNeeded(format!("{} is not there.", path.display())))?;
            if ticks_of(&meta) != node.file_mod_time_stamp || Some(meta.len()) != node.size {
                return Err(FixError::RescanNeeded(format!(
                    "{} has changed since the last scan.",
                    path.display()
                )));
            }
            let file = File::open(&path)
                .map_err(|e| FixError::FileSystem(format!("{}: {e}", path.display())))?;
            f(&mut BufReader::with_capacity(1 << 20, file), meta.len())
        }
        FileType::FILE_ZIP | FileType::FILE_SEVEN_ZIP => {
            let parent_path = src.parent();
            let archive = get(root, &parent_path)
                .ok_or_else(|| FixError::Logic(format!("{src} has no archive")))?;
            let path = physical(root, settings, &parent_path);
            let meta = fs::metadata(&path)
                .map_err(|_| FixError::RescanNeeded(format!("{} is not there.", path.display())))?;
            if ticks_of(&meta) != archive.file_mod_time_stamp {
                return Err(FixError::RescanNeeded(format!(
                    "{} has changed since the last scan.",
                    path.display()
                )));
            }
            if node.file_type == FileType::FILE_ZIP {
                if cache.zip.as_ref().map(|(p, _)| p) != Some(&path) {
                    let file = File::open(&path)
                        .map_err(|e| FixError::FileSystem(format!("{}: {e}", path.display())))?;
                    let z = ZipArchive::open(BufReader::with_capacity(1 << 16, file))
                        .map_err(|e| FixError::RescanNeeded(format!("{}: {e}.", path.display())))?;
                    cache.zip = Some((path.clone(), z));
                }
                let z = &mut cache.zip.as_mut().expect("opened above").1;
                let idx = entry_index(node, z.entries().iter().map(|e| e.name.as_str()))
                    .ok_or_else(|| {
                        FixError::RescanNeeded(format!("{src} is not in {}.", path.display()))
                    })?;
                let size = z.entries()[idx].uncompressed_size;
                let mut r = z
                    .open_entry(idx)
                    .map_err(|e| FixError::SourceDataStreamCorrupt(format!("{src}: {e}")))?;
                f(&mut r, size)
            } else {
                if cache.sevenz.as_ref().map(|(p, _)| p) != Some(&path) {
                    let file = File::open(&path)
                        .map_err(|e| FixError::FileSystem(format!("{}: {e}", path.display())))?;
                    let z = SevenZipArchive::open(BufReader::with_capacity(1 << 16, file))
                        .map_err(|e| FixError::RescanNeeded(format!("{}: {e}.", path.display())))?;
                    cache.sevenz = Some((path.clone(), z));
                }
                let z = &mut cache.sevenz.as_mut().expect("opened above").1;
                let idx = entry_index(node, z.entries().iter().map(|e| e.name.as_str()))
                    .ok_or_else(|| {
                        FixError::RescanNeeded(format!("{src} is not in {}.", path.display()))
                    })?;
                let size = z.entries()[idx].size;
                let mut r = z
                    .open_entry(idx)
                    .map_err(|e| FixError::SourceDataStreamCorrupt(format!("{src}: {e}")))?;
                f(&mut r, size)
            }
        }
        other => Err(FixError::Logic(format!(
            "{src} is a {other:?}, which holds no data"
        ))),
    }
}

/// The entry of an archive that the node stands for: the recorded index when it still names the
/// node, else the first entry of that name.
fn entry_index<'a>(
    node: &RvFile,
    mut names: impl Iterator<Item = &'a str> + Clone,
) -> Option<usize> {
    let want = node.zip_index();
    if want >= 0 {
        if let Some(n) = names.clone().nth(want as usize) {
            if n == node.name {
                return Some(want as usize);
            }
        }
    }
    names.position(|n| n == node.name)
}

/// The scanner's view of what the database already knows about `node`, with a new name: the
/// values and flags are copied, the children too. Used when a file is moved, so the tree says
/// what a scan of the new place would say without reading the data again.
pub fn scanned_from_node(node: &RvFile, name: &str) -> ScannedFile {
    let mut sf = ScannedFile::new(node.file_type, name);
    sf.file_mod_time_stamp = node.file_mod_time_stamp;
    sf.got_status = node.got_status;
    sf.size = node.size;
    sf.crc = node.crc.clone();
    sf.sha1 = node.sha1.clone();
    sf.md5 = node.md5.clone();
    sf.alt_size = node.alt_size;
    sf.alt_crc = node.alt_crc.clone();
    sf.alt_sha1 = node.alt_sha1.clone();
    sf.alt_md5 = node.alt_md5.clone();
    sf.header_file_type = node.header_file_type.unwrap_or(HeaderFileType::NOTHING);
    sf.chd_version = node.chd_version;
    sf.index = node.zip_index().max(0);
    sf.local_header_offset = node.zip_file_header_position;
    sf.deep_scanned = true;
    sf.status_flags = FileStatus(
        node.file_status.0 & FileStatus::HEADER_FLAGS.0
            | node.file_status.0 & FileStatus::VERIFIED_FLAGS.0,
    );
    if let Some(z) = node.zip_struct {
        sf.zip_struct = z;
    }
    sf.children = node
        .children
        .iter()
        .map(|c| scanned_from_node(c, &c.name))
        .collect();
    sf
}
