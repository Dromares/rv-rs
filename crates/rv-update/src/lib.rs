//! `rv-update`: look for a newer build on GitHub Releases and replace the running program with
//! it, but only on the user's say-so.
//!
//! * [`release::check`] reads `GET /repos/Dromares/rv-rs/releases/latest` (or the release list,
//!   when the setting for pre-releases is on), anonymously, and compares the tag with the
//!   running version ([`version`]). No release at all is a normal answer
//!   ([`release::Check::NothingPublished`]).
//! * [`install::install`] downloads the file for this system and `SHA256SUMS`, and refuses to
//!   install anything the sums do not vouch for ([`install`] has the order of the steps and the
//!   backup).
//! * [`appimage`]: `rv-gtk` run from an AppImage is updated by replacing the AppImage file with
//!   the release's AppImage, never with the bare binary.
//! * Nothing here asks the user anything and nothing installs by itself: the callers (`rv update
//!   install`, the `rv-gtk` menu) show what would happen and ask first.
//!
//! The address of the API is a setting ([`config::UpdateConfig::api_base`]); the tests point it
//! at a mock on `127.0.0.1` ([`mock`], feature `mock`).

pub mod appimage;
pub mod config;
mod error;
mod http;
pub mod install;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
pub mod release;
pub mod sums;
pub mod version;

pub use config::UpdateConfig;
pub use error::{Error, Result};
pub use install::{install, Installed, Step};
pub use release::{check, Asset, Check, Product, Update, REPO};
pub use version::Version;

/// The version of the running build: every crate of the workspace has the same one.
pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the workspace version is a semver")
}
