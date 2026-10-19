//! Outcomes of reading a DAT.
//!
//! ROMVault's readers return a `bool` and report text through a callback, and the caller
//! ignores the `bool` (`RomVaultCore/ReadDat/DatReader.cs:76-81`): a reader that fails half way
//! still hands back the partly filled header and the DAT is imported with what was read. This
//! crate keeps all three pieces in a [`ReadOutcome`] and leaves the policy to the caller;
//! [`crate::read_dat`] is the strict view.

use std::fmt;

/// What a reader produced.
#[derive(Clone, Debug)]
pub struct ReadOutcome {
    /// The header, with whatever was parsed before a failure. `None` only when the reader
    /// never got as far as creating it (empty file, "Invalid DAT File", a file that could not
    /// be opened or decompressed).
    pub header: Option<crate::DatHeader>,
    /// The `bool` the C# reader returned. `false` means a parse error.
    pub ok: bool,
    /// Every message the reader reported, in order. Unknown keywords are reported but do not
    /// make `ok` false.
    pub messages: Vec<String>,
}

impl ReadOutcome {
    pub(crate) fn failed(message: impl Into<String>) -> ReadOutcome {
        ReadOutcome {
            header: None,
            ok: false,
            messages: vec![message.into()],
        }
    }

    /// The strict view: the header only if the reader succeeded.
    pub fn into_result(self) -> Result<crate::DatHeader, DatError> {
        match (self.ok, self.header) {
            (true, Some(header)) => Ok(header),
            (_, _) => Err(DatError {
                messages: self.messages,
            }),
        }
    }
}

/// A DAT that could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatError {
    pub messages: Vec<String>,
}

impl fmt::Display for DatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.messages.is_empty() {
            write!(f, "DAT could not be read")
        } else {
            write!(f, "{}", self.messages.join("; "))
        }
    }
}

impl std::error::Error for DatError {}

/// Where C# throws an exception that the caller catches: the reader stops, the header so far
/// stays (`DatReader.cs:39-43`).
#[derive(Debug)]
pub(crate) struct Abort(pub String);

pub(crate) type R<T> = Result<T, Abort>;

/// Collects reader messages (`ReportError`).
#[derive(Default)]
pub(crate) struct Report {
    pub messages: Vec<String>,
}

impl Report {
    pub fn say(&mut self, text: impl Into<String>) {
        self.messages.push(text.into());
    }
}
