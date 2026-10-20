//! File system steps with the crash-safety the fixer needs, and the test hooks that let a test
//! stop the process between them.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Arc;

use rv_archive::{sync_dir, AtomicFile};
use rv_scan::{HashKinds, HashSet};

use crate::error::{FixError, Res};

type Hook = Arc<dyn Fn(&str) -> Option<FailAction> + Send + Sync>;

/// What a failpoint does when it fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailAction {
    /// Return [`FixError::Injected`]: the run stops like on any other error and saves the cache.
    Error,
    /// `std::process::abort()`: the process dies on the spot, as a kill or a power cut would
    /// leave it. Nothing is saved or cleaned up.
    Abort,
}

/// The steps of a fix that a test can stop at. Names are listed in [`STEPS`].
#[derive(Clone, Default)]
pub struct Failpoints {
    hook: Option<Hook>,
}

/// Every failpoint, in the order a fix passes them.
pub const STEPS: &[&str] = &[
    "after_temp_write",
    "after_fsync",
    "before_rename",
    "after_rename",
    "before_source_delete",
    "before_cache_save",
];

impl Failpoints {
    pub fn none() -> Failpoints {
        Failpoints { hook: None }
    }

    /// Fire `action` when the step called `name` is reached for the `nth` time (1 is the first).
    pub fn at(name: &str, nth: usize, action: FailAction) -> Failpoints {
        let name = name.to_owned();
        let hits = std::sync::atomic::AtomicUsize::new(0);
        Failpoints {
            hook: Some(Arc::new(move |step| {
                if step != name {
                    return None;
                }
                let n = hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                (n == nth).then_some(action)
            })),
        }
    }

    /// `RV_FIX_FAILPOINT=<step>[:<nth>]` aborts the process at that step. The hook the tests use
    /// on a child process; it does nothing when the variable is not set.
    pub fn from_env() -> Failpoints {
        match std::env::var("RV_FIX_FAILPOINT") {
            Ok(v) if !v.is_empty() => {
                let (name, nth) = match v.split_once(':') {
                    Some((n, k)) => (n, k.parse().unwrap_or(1)),
                    None => (v.as_str(), 1),
                };
                Failpoints::at(name, nth, FailAction::Abort)
            }
            _ => Failpoints::none(),
        }
    }

    /// Reach the step called `name`.
    pub fn hit(&self, name: &str) -> Res<()> {
        let Some(hook) = &self.hook else {
            return Ok(());
        };
        match hook(name) {
            None => Ok(()),
            Some(FailAction::Error) => Err(FixError::Injected(name.to_owned())),
            Some(FailAction::Abort) => std::process::abort(),
        }
    }
}

/// Put `tmp` at `dst` without ever replacing something that is there, then `fsync` the
/// directory. The source is left in place.
pub fn link_new(src: &Path, dst: &Path) -> io::Result<()> {
    match fs::hard_link(src, dst) {
        Ok(()) => sync_dir(dst),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(e) => Err(e),
    }
}

/// Move the file `src` to `dst`, which must not exist. A link then an unlink on one file system,
/// so a crash leaves the file at one path or both, never neither; across file systems (or where
/// there are no links) the data is copied, `fsync`ed, read back and compared, and only then is
/// `src` removed. `before_unlink` runs between the two halves.
pub fn move_new(src: &Path, dst: &Path, fail: &Failpoints) -> Res<()> {
    if dst.exists() {
        return Err(FixError::CannotMove(format!(
            "{} already exists",
            dst.display()
        )));
    }
    match link_new(src, dst) {
        Ok(()) => {
            fail.hit("before_source_delete")?;
            fs::remove_file(src)?;
            sync_dir(src)?;
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(FixError::CannotMove(format!(
            "{} already exists",
            dst.display()
        ))),
        Err(_) => {
            // Another file system, or one without links.
            let mut input = fs::File::open(src)?;
            let mut out = AtomicFile::create(dst)?;
            let mut want = HashSet::new(HashKinds::ROMVAULT);
            copy_hashing(&mut input, &mut out, &mut want)?;
            out.sync()?;
            let mut check = HashSet::new(HashKinds::ROMVAULT);
            let mut tmp = fs::File::open(out.tmp_path())?;
            copy_hashing(&mut tmp, &mut io::sink(), &mut check)?;
            if check.finish() != want.finish() {
                return Err(FixError::DestinationCheckSumMismatch(format!(
                    "{} did not read back as it was written.",
                    dst.display()
                )));
            }
            out.commit_new()
                .map_err(|e| FixError::CannotMove(e.to_string()))?;
            fail.hit("before_source_delete")?;
            fs::remove_file(src)?;
            sync_dir(src)?;
            Ok(())
        }
    }
}

/// Copy `r` to `w` until the end, hashing what passes. Short reads are fine.
pub fn copy_hashing(r: &mut dyn Read, w: &mut dyn Write, h: &mut HashSet) -> io::Result<u64> {
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => return Ok(total),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        h.update(&buf[..n]);
        w.write_all(&buf[..n])?;
        total += n as u64;
    }
}
