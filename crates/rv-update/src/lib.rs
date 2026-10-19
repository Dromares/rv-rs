//! `rv-update`: placeholder crate, no functionality yet.

/// Name of this crate, used by the placeholder test.
pub const CRATE_NAME: &str = "rv-update";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_matches_package() {
        assert_eq!(CRATE_NAME, env!("CARGO_PKG_NAME"));
    }
}
