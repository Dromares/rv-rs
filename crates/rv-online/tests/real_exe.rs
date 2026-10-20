//! Against the maintainer's own `ROMVault38.exe` (`RV_ROMVAULT_EXE=<path>`); ignored in CI.
//! It checks that the exe is the known one and that a salt of 13 bytes is found at the recorded
//! place. It prints and asserts nothing about the salt's value.

use std::path::PathBuf;

#[test]
#[ignore = "needs RV_ROMVAULT_EXE"]
fn the_salt_is_found_in_the_real_exe() {
    let exe =
        PathBuf::from(std::env::var_os("RV_ROMVAULT_EXE").expect("RV_ROMVAULT_EXE is not set"));
    let salt = rv_online::salt::read_salt(&exe, &rv_online::salt::ROMVAULT_3_8_0)
        .expect("the exe and the salt");
    assert_eq!(salt.as_bytes().len(), 13);
}
