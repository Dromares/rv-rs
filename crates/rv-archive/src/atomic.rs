//! Write-then-rename, so a fix can never lose the original (ROMVault deletes the source before
//! it renames the new file in, `TorrentZipRebuild.cs:222-258`; a crash in between loses the data).
//!
//! [`AtomicFile`] is a temp file next to the final path. Writing to it, or dropping it, cannot
//! change the final path. [`AtomicFile::commit`] flushes, `fsync`s the file, renames it over the
//! final path (atomic on one file system, and an existing file is replaced in the same step)
//! and `fsync`s the directory.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// A temp file that becomes `final_path` only when [`commit`](AtomicFile::commit) succeeds.
pub struct AtomicFile {
    final_path: PathBuf,
    tmp_path: PathBuf,
    file: Option<File>,
}

impl AtomicFile {
    /// Create the temp file `<dir>/__<name>.rvtmp` for `final_path`. A leftover temp file from a
    /// crashed run is truncated; `final_path` itself is not touched.
    pub fn create(final_path: impl Into<PathBuf>) -> io::Result<AtomicFile> {
        let final_path = final_path.into();
        let name = final_path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "the path has no file name")
        })?;
        let mut tmp_name = std::ffi::OsString::from("__");
        tmp_name.push(name);
        tmp_name.push(".rvtmp");
        let tmp_path = final_path.with_file_name(tmp_name);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;
        Ok(AtomicFile {
            final_path,
            tmp_path,
            file: Some(file),
        })
    }

    pub fn tmp_path(&self) -> &Path {
        &self.tmp_path
    }

    pub fn final_path(&self) -> &Path {
        &self.final_path
    }

    fn file(&mut self) -> &mut File {
        self.file.as_mut().expect("the file is open until commit")
    }

    /// Flush and `fsync` the temp file. [`commit`](Self::commit) does it again (it costs nothing
    /// once the data is on disk); calling it first lets the caller validate what was written,
    /// knowing that it is what a crash would leave.
    pub fn sync(&mut self) -> io::Result<()> {
        let f = self.file();
        f.flush()?;
        f.sync_all()
    }

    /// Flush, `fsync`, rename over the final path, `fsync` the directory. On an error the temp
    /// file is removed and the final path is as it was.
    pub fn commit(self) -> io::Result<()> {
        self.finish(true)
    }

    /// [`commit`](Self::commit) for a path that must not exist yet: the temp file is linked to
    /// the final path, which fails if something is there, and the temp name is removed. Where the
    /// file system cannot link, the existence check and the rename are separate steps.
    pub fn commit_new(self) -> io::Result<()> {
        self.finish(false)
    }

    fn finish(mut self, replace: bool) -> io::Result<()> {
        let file = self.file.take().expect("the file is open until commit");
        let result = (|| {
            file.sync_all()?;
            drop(file);
            if replace {
                fs::rename(&self.tmp_path, &self.final_path)?;
            } else {
                match fs::hard_link(&self.tmp_path, &self.final_path) {
                    Ok(()) => fs::remove_file(&self.tmp_path)?,
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(e),
                    Err(_) => {
                        if self.final_path.exists() {
                            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
                        }
                        fs::rename(&self.tmp_path, &self.final_path)?;
                    }
                }
            }
            sync_dir(&self.final_path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&self.tmp_path);
        }
        // Either the rename happened or the temp file is gone: nothing for Drop to clean up.
        self.file = None;
        std::mem::forget(self);
        result
    }
}

/// `fsync` the directory that holds `path`, so a rename or link in it survives a crash.
pub fn sync_dir(path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        File::open(dir)?.sync_all()?;
    }
    Ok(())
}

impl Drop for AtomicFile {
    fn drop(&mut self) {
        if self.file.take().is_some() {
            let _ = fs::remove_file(&self.tmp_path);
        }
    }
}

impl Read for AtomicFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file().read(buf)
    }
}

impl Write for AtomicFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file().flush()
    }
}

impl Seek for AtomicFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file().seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("rv-archive-atomic-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn commit_replaces_the_target() {
        let d = dir("commit");
        let target = d.join("a.zip");
        fs::write(&target, b"old").unwrap();
        let mut f = AtomicFile::create(&target).unwrap();
        f.write_all(b"new data").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"old", "untouched until commit");
        f.commit().unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new data");
        assert!(!d.join("__a.zip.rvtmp").exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn commit_new_refuses_to_replace() {
        let d = dir("new");
        let target = d.join("a.zip");
        fs::write(&target, b"precious").unwrap();
        let mut f = AtomicFile::create(&target).unwrap();
        f.write_all(b"new data").unwrap();
        f.sync().unwrap();
        let e = f.commit_new().unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&target).unwrap(), b"precious");
        assert!(!d.join("__a.zip.rvtmp").exists());

        let fresh = d.join("b.zip");
        let mut f = AtomicFile::create(&fresh).unwrap();
        f.write_all(b"fresh").unwrap();
        f.commit_new().unwrap();
        assert_eq!(fs::read(&fresh).unwrap(), b"fresh");
        assert!(!d.join("__b.zip.rvtmp").exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn dropping_without_commit_keeps_the_original() {
        let d = dir("drop");
        let target = d.join("a.zip");
        fs::write(&target, b"precious").unwrap();
        {
            let mut f = AtomicFile::create(&target).unwrap();
            f.write_all(b"half a zip").unwrap();
        }
        assert_eq!(fs::read(&target).unwrap(), b"precious");
        assert!(!d.join("__a.zip.rvtmp").exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_failed_commit_keeps_the_original_and_removes_the_temp() {
        let d = dir("fail");
        // The target is a directory, so the rename must fail.
        let target = d.join("a.zip");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), b"x").unwrap();
        let mut f = AtomicFile::create(&target).unwrap();
        f.write_all(b"data").unwrap();
        assert!(f.commit().is_err());
        assert!(target.join("keep").exists());
        assert!(!d.join("__a.zip.rvtmp").exists());
        fs::remove_dir_all(&d).unwrap();
    }
}
