use std::fmt;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// The address is not usable (not `https://`, or `http://` to somewhere but the loopback).
    BadUrl(String),
    /// The connection or the transfer failed.
    Network(String),
    /// The server answered with a status this client does not accept.
    Status { url: String, code: u16 },
    /// The answer is not what the Releases API sends.
    BadReply(String),
    /// A download was bigger than the limit for it.
    TooBig { url: String, limit: u64 },
    /// The release has no file for this platform.
    NoAsset { name: String, release: String },
    /// The release has no `SHA256SUMS`. Nothing is installed without one.
    SumsMissing { release: String },
    /// `SHA256SUMS` has no line for the file.
    SumsNoEntry { name: String },
    /// `SHA256SUMS` is unreadable.
    SumsBad(String),
    /// The download is not the file `SHA256SUMS` names.
    HashMismatch {
        name: String,
        expected: String,
        actual: String,
    },
    /// The download is not as long as the release says it is.
    SizeMismatch {
        name: String,
        expected: u64,
        actual: u64,
    },
    /// Nothing can be created next to the program, and this crate never asks for more rights.
    NotWritable { dir: PathBuf, why: String },
    /// Any other file system failure.
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadUrl(m) => write!(f, "bad address: {m}"),
            Error::Network(m) => write!(f, "network error: {m}"),
            Error::Status { url, code } => write!(f, "{url} answered {code}"),
            Error::BadReply(m) => write!(f, "unexpected reply: {m}"),
            Error::TooBig { url, limit } => {
                write!(f, "{url} is bigger than the limit of {limit} bytes")
            }
            Error::NoAsset { name, release } => {
                write!(f, "release {release} has no file named {name} for this system")
            }
            Error::SumsMissing { release } => write!(
                f,
                "release {release} has no SHA256SUMS file, so nothing was installed"
            ),
            Error::SumsNoEntry { name } => write!(
                f,
                "SHA256SUMS has no line for {name}, so nothing was installed"
            ),
            Error::SumsBad(m) => write!(f, "SHA256SUMS is not readable: {m}"),
            Error::HashMismatch {
                name,
                expected,
                actual,
            } => write!(
                f,
                "{name} does not match SHA256SUMS (expected {expected}, got {actual}), nothing was installed"
            ),
            Error::SizeMismatch {
                name,
                expected,
                actual,
            } => write!(
                f,
                "{name} is {actual} bytes, the release says {expected}; nothing was installed"
            ),
            Error::NotWritable { dir, why } => {
                write!(f, "cannot write in {}: {why}", dir.display())
            }
            Error::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {}
