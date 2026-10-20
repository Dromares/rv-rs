//! Why a fix stopped. These are the `ReturnCode`s of `Fix.cs` (`CopyFile.cs:28-43`) as an error
//! type: 3.8.0 calls `Environment.Exit` on its "impossible" branches (`Fix.cs:250-252`,
//! `FixAFile.cs:83-86`, `DoubleCheckDelete.cs:62-76`, `FixAZip.cs:319-321,574-590`) and skips
//! saving the database; here every one of them is an error value, and the caller saves.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FixError {
    /// A file is not what the last scan saw. Scan again.
    RescanNeeded(String),
    /// Fix needs Find Fixes first: a file has no group.
    FindFixesMissingFileGroups(String),
    /// Fix needs Find Fixes first: a status is not one Fix can act on.
    FindFixesInvalidStatus(String),
    /// Two tree paths lead to the same place on disk.
    TreeStructureError(String),
    /// A state that should not happen.
    Logic(String),
    FileSystem(String),
    /// A source did not read back as the file the database says it is.
    SourceDataStreamCorrupt(String),
    SourceCheckSumMismatch(String),
    /// What was written is not what the DAT wants. Nothing was put in place.
    DestinationCheckSumMismatch(String),
    ToSortNotFound(String),
    CannotMove(String),
    /// The cancel flag was set while a copy was in progress. Not an error for the caller.
    Cancelled,
    /// A test hook fired (`RV_FIX_FAILPOINT`, [`crate::Failpoints`]).
    Injected(String),
}

impl fmt::Display for FixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use FixError::*;
        match self {
            RescanNeeded(m) => write!(f, "{m} Rescan needed."),
            FindFixesMissingFileGroups(m) | FindFixesInvalidStatus(m) => {
                write!(f, "{m} You need to Find Fixes before Fixing.")
            }
            TreeStructureError(m) | Logic(m) | SourceCheckSumMismatch(m) => f.write_str(m),
            FileSystem(m) => write!(f, "File system error: {m}"),
            SourceDataStreamCorrupt(m) => write!(f, "Source data is corrupt: {m}"),
            DestinationCheckSumMismatch(m) => {
                write!(f, "{m} Level 2 scan your files and find fixes again.")
            }
            ToSortNotFound(m) => write!(f, "ToSort directory could not be created: {m}"),
            CannotMove(m) => write!(f, "Cannot move: {m}"),
            Cancelled => f.write_str("Cancelled"),
            Injected(m) => write!(f, "injected failure at {m}"),
        }
    }
}

impl std::error::Error for FixError {}

impl From<std::io::Error> for FixError {
    fn from(e: std::io::Error) -> FixError {
        FixError::FileSystem(e.to_string())
    }
}

pub type Res<T> = Result<T, FixError>;
