//! `rv-fix`: Find Fixes and Fix ROMs.
//!
//! * [`find_fixes`] looks at the whole tree and gives every file a fix status (`CanBeFixed`,
//!   `MoveToSort`, `Delete`, ...); nothing on disk changes.
//! * [`perform_fixes`] then makes it so: missing files are made from files that have their data,
//!   files nobody wants go to ToSort, duplicates are deleted once a second copy is proven, and
//!   archives are rebuilt as TorrentZips.
//! * [`plan`] says what a fix would do, without doing it.
//! * [`fixdat`] writes the DATs of what is still missing.
//! * [`plan`] says what a fix would do without doing it.
//!
//! The ways this crate is safer than ROMVault 3.8.0 are in [`fixer`]'s module documentation;
//! the part that matters most: **a file is never deleted before its replacement has been written,
//! synced, read back and checked, and nothing is written at its final name.**

pub mod archive;
pub mod dbhelper;
pub mod error;
pub mod find;
pub mod fixdat;
pub mod fixer;
pub mod fsx;
pub mod groups;
mod loose;
pub mod nodepath;
pub mod plan;
pub mod source;

pub use error::{FixError, Res};
pub use find::{find_fixes, FindFixes, FindReport};
pub use fixer::{action, perform_fixes, ActionRow, FixOptions, FixReport};
pub use fsx::{FailAction, Failpoints, STEPS};
pub use groups::{FileGroup, Groups};
pub use nodepath::NodePath;
