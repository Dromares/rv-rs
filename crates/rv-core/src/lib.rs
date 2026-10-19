//! `rv-core`: the data model that the ROMVault `.Cache` file serializes, the database tree built
//! on it, the `RepStatus` table and the settings.
//!
//! The model is deliberately *lossless*: every value that the cache stores is kept as
//! read (enums are `u8`/`u32` newtypes so unknown values survive), so that
//! `rv-cache` can write a parsed file back byte for byte. Derived state (`RepStatus`,
//! `DirStatus`, parent pointers, ...) is not part of the model, because it is not
//! stored in the file (`docs/cache-format.md` section 5.13).
//!
//! Citations like `rvFile.cs:405-509` refer to the RVWorld 3.8.0 source through
//! `docs/cache-format.md` of the decompile notes, which is not part of this repository.

mod enums;
pub mod header;
mod model;
pub mod progress;
mod rep_status;
pub mod settings;
pub mod sorters;
pub mod time;
mod tree;

pub use enums::*;
pub use model::{MetaEntry, RvDat, RvFile, RvGame, RvTreeRow};
pub use rep_status::{baseline_status, status_check, ui_status, DirStatus, RepStatus};
pub use settings::{
    DatRule, DirMapping, FilterType, FixLevel, HeaderType, MergeType, RemoveSubType, ScanLevel,
    Settings, SettingsError,
};
pub use tree::{Cx, DatTable, DatUpdateStatus, Database, DbError, EFile};
