//! Replacing the running program with a downloaded one, safely.
//!
//! The order, and why:
//!
//! 1. A temporary file is created next to the target. That is also the test for "may this user
//!    write here?": if it cannot be created, the answer is [`Error::NotWritable`] and the caller
//!    reports the release page. No `sudo`, no `pkexec`, nothing is ever asked for.
//! 2. `SHA256SUMS` is fetched. Without it, or without a line for the file, nothing is installed.
//! 3. The file is streamed into the temporary file while it is hashed.
//! 4. The hash must equal the one in `SHA256SUMS`; the temporary file is `fsync`ed and read back
//!    from the disk, and the hash must still be equal.
//! 5. The old program is kept as `<target>.rv-backup` (a hard link, or a copy where that fails;
//!    made under a temporary name and renamed, so a backup is never half there).
//! 6. The temporary file is renamed over the target. That is atomic, so at no moment is there no
//!    program, and nothing of the old one is deleted before the new one is in place. If the
//!    rename fails the target is checked against the old hash and restored from the backup if it
//!    differs.
//!
//! Any failure before step 6 removes the temporary file and leaves the target as it was.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::http;
use crate::release::Update;
use crate::sums;
use crate::version::Version;

/// The biggest file downloaded as a program.
pub const MAX_BINARY: u64 = 512 * 1024 * 1024;
const MAX_SUMS: u64 = 1024 * 1024;
const SUMS_NAME: &str = "SHA256SUMS";

/// A point in the install, reported to the hook after it is done. The tests kill the process or
/// break something at one of them; nothing else uses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The temporary file exists, so the directory is writable.
    Probed,
    /// The whole file is in the temporary file and its hash matches `SHA256SUMS`.
    Downloaded,
    /// The temporary file was read back from the disk and the hash still matches.
    Verified,
    /// The old program is saved as the backup.
    BackedUp,
    /// The new program is in place.
    Renamed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub target: PathBuf,
    /// The old program; absent when there was none.
    pub backup: Option<PathBuf>,
    pub version: Version,
}

fn sibling(target: &Path, suffix: &str) -> PathBuf {
    let mut s = target.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Where the old program is kept: `<target>.rv-backup`.
pub fn backup_path(target: &Path) -> PathBuf {
    sibling(target, ".rv-backup")
}

/// The file the new program is written to first: `<target>.rv-update.tmp`.
pub fn temp_path(target: &Path) -> PathBuf {
    sibling(target, ".rv-update.tmp")
}

fn io_err(what: &str, path: &Path, e: &io::Error) -> Error {
    Error::Io(format!("cannot {what} {}: {e}", path.display()))
}

fn not_writable(e: &io::Error) -> bool {
    // 30 is EROFS: a read-only file system (ErrorKind::ReadOnlyFilesystem is newer than this
    // crate's minimum Rust).
    e.kind() == io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(30)
}

fn hash_file(path: &Path) -> Result<String> {
    let mut f = File::open(path).map_err(|e| io_err("read", path, &e))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| io_err("read", path, &e))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(sums::hex(&h.finalize()))
}

/// Writes through to a file while hashing and counting.
struct Tee<'a> {
    file: &'a mut File,
    hash: Sha256,
    len: u64,
}

impl Write for Tee<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.file.write(buf)?;
        self.hash.update(&buf[..n]);
        self.len += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn sync_dir(dir: &Path) {
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(unix)]
fn set_mode(tmp: &Path, like: Option<&fs::Metadata>) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = like.map_or(0o755, |m| m.permissions().mode() & 0o7777) | 0o111;
    fs::set_permissions(tmp, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_tmp: &Path, _like: Option<&fs::Metadata>) -> io::Result<()> {
    Ok(())
}

/// Install release file `asset_name` of `update` over `target`. `hook` is called after each
/// [`Step`].
pub fn install(
    update: &Update,
    asset_name: &str,
    target: &Path,
    hook: &dyn Fn(Step),
) -> Result<Installed> {
    let asset = update.asset(asset_name).ok_or_else(|| Error::NoAsset {
        name: asset_name.to_owned(),
        release: update.tag.clone(),
    })?;
    let sums_asset = update.asset(SUMS_NAME).ok_or_else(|| Error::SumsMissing {
        release: update.tag.clone(),
    })?;
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let tmp = temp_path(target);

    // 1. The probe. A stale file from an interrupted run is ours and is simply overwritten.
    let mut file = File::create(&tmp).map_err(|e| {
        if not_writable(&e) {
            Error::NotWritable {
                dir: dir.to_owned(),
                why: e.to_string(),
            }
        } else {
            io_err("create", &tmp, &e)
        }
    })?;
    hook(Step::Probed);

    let result = stage_and_swap(update, asset, sums_asset, target, &tmp, &mut file, hook);
    drop(file);
    match result {
        Ok(backup) => {
            sync_dir(dir);
            hook(Step::Renamed);
            Ok(Installed {
                target: target.to_owned(),
                backup,
                version: update.version.clone(),
            })
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn stage_and_swap(
    update: &Update,
    asset: &crate::release::Asset,
    sums_asset: &crate::release::Asset,
    target: &Path,
    tmp: &Path,
    file: &mut File,
    hook: &dyn Fn(Step),
) -> Result<Option<PathBuf>> {
    // 2. SHA256SUMS.
    let sums_bytes = http::get_small(&sums_asset.url, "application/octet-stream", MAX_SUMS)?
        .ok_or_else(|| Error::SumsMissing {
            release: update.tag.clone(),
        })?;
    let sums_text = String::from_utf8(sums_bytes)
        .map_err(|_| Error::SumsBad("it is not UTF-8 text".to_owned()))?;
    let expected = sums::find(&sums_text, &asset.name)?.ok_or_else(|| Error::SumsNoEntry {
        name: asset.name.clone(),
    })?;

    // 3. The download, hashed on the way.
    let url = asset.url.clone();
    let (actual, len) = http::get(
        &url,
        "application/octet-stream",
        MAX_BINARY,
        |status, body| {
            if status != 200 {
                return Err(Error::Status {
                    url: url.clone(),
                    code: status,
                });
            }
            let mut tee = Tee {
                file,
                hash: Sha256::new(),
                len: 0,
            };
            io::copy(body, &mut tee).map_err(|e| Error::Network(e.to_string()))?;
            tee.flush().map_err(|e| io_err("write", tmp, &e))?;
            Ok((sums::hex(&tee.hash.finalize()), tee.len))
        },
    )?;
    if asset.size != 0 && len != asset.size {
        return Err(Error::SizeMismatch {
            name: asset.name.clone(),
            expected: asset.size,
            actual: len,
        });
    }
    // 4. The hash, then the disk.
    if actual != expected {
        return Err(Error::HashMismatch {
            name: asset.name.clone(),
            expected,
            actual,
        });
    }
    hook(Step::Downloaded);
    file.sync_all().map_err(|e| io_err("sync", tmp, &e))?;
    let on_disk = hash_file(tmp)?;
    if on_disk != expected {
        return Err(Error::HashMismatch {
            name: asset.name.clone(),
            expected,
            actual: on_disk,
        });
    }
    hook(Step::Verified);

    // 5. The backup.
    let old = fs::metadata(target).ok();
    let old_hash = if old.is_some() {
        Some(hash_file(target)?)
    } else {
        None
    };
    set_mode(tmp, old.as_ref()).map_err(|e| io_err("set the mode of", tmp, &e))?;
    let backup = if old.is_some() {
        let backup = backup_path(target);
        let staging = sibling(&backup, ".new");
        let _ = fs::remove_file(&staging);
        if fs::hard_link(target, &staging).is_err() {
            fs::copy(target, &staging).map_err(|e| io_err("copy", target, &e))?;
        }
        fs::rename(&staging, &backup).map_err(|e| io_err("save the backup", &backup, &e))?;
        Some(backup)
    } else {
        None
    };
    hook(Step::BackedUp);

    // 6. The swap.
    if let Err(e) = fs::rename(tmp, target) {
        let why = io_err("put the new program in place at", target, &e);
        return Err(roll_back(
            target,
            backup.as_deref(),
            old_hash.as_deref(),
            why,
        ));
    }
    Ok(backup)
}

/// The rename failed. A failed rename leaves the target alone, but this checks rather than
/// assumes, and puts the backup back if the target is not the old program.
fn roll_back(target: &Path, backup: Option<&Path>, old_hash: Option<&str>, why: Error) -> Error {
    let (Some(backup), Some(old_hash)) = (backup, old_hash) else {
        return why;
    };
    if hash_file(target).is_ok_and(|h| h == old_hash) {
        return Error::Io(format!("{why}; the old program is untouched"));
    }
    let restore = sibling(target, ".rv-restore.tmp");
    let restored = fs::copy(backup, &restore)
        .and_then(|_| set_mode(&restore, fs::metadata(backup).ok().as_ref()))
        .and_then(|_| fs::rename(&restore, target));
    let _ = fs::remove_file(&restore);
    match restored {
        Ok(()) => Error::Io(format!(
            "{why}; the old program was restored from the backup"
        )),
        Err(e2) => Error::Io(format!(
            "{why}; restoring from {} failed too: {e2}",
            backup.display()
        )),
    }
}
