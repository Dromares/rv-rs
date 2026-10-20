//! The errors of the online layer. Their text is safe to show and to log: nothing here carries
//! the DatVault key, and [`scrub`] takes it out of anything the server sent back.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// An `http://` URL while `allow_plain_http` is off.
    PlainHttpRefused(String),
    BadUrl(String),
    /// An argument the protocol cannot carry (a control character in a name, say).
    BadArgument(String),
    /// No answer: DNS, connect, TLS, timeout.
    Network(String),
    /// An HTTP status that is not a SOAP reply.
    HttpStatus(u16),
    /// A SOAP fault.
    Fault(String),
    /// Something came back that is not what the operation returns.
    BadResponse(String),
    /// The operation needs a DatVault key and there is none.
    KeyRequired,
    /// The MIA salt could not be read from the user's ROMVault exe.
    Salt(String),
    /// A file could not be read or written.
    Io(String),
    /// What an MIA update could not do, in the words of the menu (`MIA::UpdateMIA`).
    Mia(String),
    /// A name from the server that would write outside the DAT root.
    UnsafePath(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::PlainHttpRefused(u) => write!(
                f,
                "{u} is a plain http:// address and allow_plain_http is off; the key would travel in clear text"
            ),
            Error::BadUrl(m) => write!(f, "bad service address: {m}"),
            Error::BadArgument(m) => write!(f, "bad argument: {m}"),
            Error::Network(m) => write!(f, "no answer from the service: {m}"),
            Error::HttpStatus(s) => write!(f, "the service answered HTTP {s}"),
            Error::Fault(m) => write!(f, "the service reported a fault: {m}"),
            Error::BadResponse(m) => write!(f, "unexpected answer from the service: {m}"),
            Error::KeyRequired => f.write_str("a DatVault key is required"),
            Error::Salt(m) => write!(f, "cannot read the MIA salt: {m}"),
            Error::Io(m) => f.write_str(m),
            Error::Mia(m) => f.write_str(m),
            Error::UnsafePath(m) => write!(f, "the service named a path outside the DAT root: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// `text` with every occurrence of each secret replaced by `***`. An empty secret is ignored.
pub fn scrub(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_owned();
    for s in secrets {
        if !s.is_empty() {
            out = out.replace(s, "***");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_takes_the_secret_out_everywhere() {
        assert_eq!(
            scrub("bad key k3y and k3y", &["k3y"]),
            "bad key *** and ***"
        );
        assert_eq!(scrub("nothing", &[""]), "nothing");
    }
}
