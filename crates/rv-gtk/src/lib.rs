//! `rv-gtk`: the GTK4 front end of rv-rs.
//!
//! The windows are in [`ui`]; everything they show comes from [`model`] (plain data built from
//! the `rv-core` database tree) and everything they make the engine do goes through [`engine`],
//! the only module that calls the library crates. [`colours`] is ROMVault's status colour table
//! and [`prefs`] the interface's own settings.

pub mod colours;
pub mod engine;
pub mod model;
pub mod prefs;
pub mod ui;
