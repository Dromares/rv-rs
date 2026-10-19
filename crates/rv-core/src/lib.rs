//! `rv-core`: the ROMVault database tree and the things around it that both the DAT update and
//! the scanner need.
//!
//! * [`RvFile`], [`RvDat`], [`RvGame`], [`RvTreeRow`]: the tree that the `.Cache` file
//!   serializes. The model is *lossless*: every value that the cache stores is kept as read
//!   (enums are `u8`/`u32` newtypes so unknown values survive), so that `rv-cache` can write a
//!   parsed file back byte for byte. Three fields are in memory only and never written: the
//!   [`RepStatus`] of a node (`docs/cache-format.md` section 5.13), the id that points a node at
//!   its [`RvDat`] while the DAT lists change, and the `search_found` mark of a merge.
//! * [`Database`]: a tree plus the DAT ids and their update status. [`Database::from_root`] links
//!   a tree read from a cache; `rv-cache` has the functions that load and save one.
//! * [`status_check`], [`RepStatus`], [`DirStatus`]: the statuses an item may have for its
//!   `(FileType, DatStatus, GotStatus)`, and the per-directory counters
//!   ([`RvFile::dir_status`]).
//! * [`Settings`]: `RomVault3cfg.xml`: DAT rules, directory mappings, ignore patterns.
//! * [`sorters`]: the name orders the cache depends on. [`header`]: the copier-header table.
//! * [`progress`]: [`progress::ProgressSink`] and [`progress::CancelFlag`], which the long
//!   operations take. There is no global state anywhere in the stack.
//!
//! How the crates fit (each step is a call, none needs the others' internals):
//!
//! ```text
//! rv_cache::load_database  ->  rv_dat::update::update_dats  ->  rv_scan::db::scan_files
//!                                                                  -> rv_cache::save_database
//! ```
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
