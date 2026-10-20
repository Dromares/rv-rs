//! Writing files so that a reader never sees half of one.

use std::io::Write;
use std::path::Path;

use crate::error::{Error, Result};

/// Write `bytes` to `path` through a temporary file in the same directory (created with the
/// directory if need be), `fsync`ed and then renamed over `path`.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let io =
        |what: &str, e: std::io::Error| Error::Io(format!("cannot {what} {}: {e}", path.display()));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io("create the directory of", e))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".rvtmp");
    let tmp = std::path::PathBuf::from(tmp);
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    result.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io("write", e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_whole_files_and_leaves_no_temporary() {
        let d = std::env::temp_dir().join(format!("rv-online-fsio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = d.join("a").join("b.txt");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let names: Vec<_> = std::fs::read_dir(p.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn a_directory_in_the_way_is_an_error() {
        let d = std::env::temp_dir().join(format!("rv-online-fsio2-{}", std::process::id()));
        std::fs::create_dir_all(d.join("x")).unwrap();
        assert!(matches!(
            write_atomic(&d.join("x"), b"1"),
            Err(Error::Io(_))
        ));
        assert!(!d.join("x.rvtmp").exists());
    }
}
