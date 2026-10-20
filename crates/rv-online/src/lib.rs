//! `rv-online`: ROMVault's online services, rebuilt from `online-protocol.md` of the decompile
//! notes (not part of this repository).
//!
//! * [`client`]: the SOAP 1.1 client for the operations ROMVault 3.8.0 calls;
//! * [`datvault`]: key check, the group list, DAT download into the DAT root;
//! * [`mia`]: the MIA list, its cache and the status of every file;
//! * [`kdf`], [`salt`]: the decryption key of the MIA list.
//!
//! The service is reached over `https://` by default; `http://` only with `allow_plain_http`.
//! No test in this repository connects to anything but `127.0.0.1` (see `tests/no_network.rs`).
//! Error telemetry (`SendErrorMessage*`) and the AirDC++ search are not built.

pub mod b64;
pub mod client;
pub mod endpoint;
pub mod error;
mod xml;

pub mod transport;

#[cfg(any(test, feature = "mock"))]
pub mod mock;

pub use client::{Client, Group, MiaReporting};
pub use endpoint::{Endpoint, DEFAULT_ENDPOINT};
pub use error::{Error, Result};
