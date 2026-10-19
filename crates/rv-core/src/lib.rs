//! `rv-core`: the data model that the ROMVault `.Cache` file serializes.
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
mod model;

pub use enums::*;
pub use model::{MetaEntry, RvDat, RvFile, RvGame, RvTreeRow};
