# rv-rs

An unofficial, unaffiliated Linux rewrite of [RomVault](https://github.com/RomVault/RVWorld) in Rust: a ROM collection manager that reads DAT files, scans your ROM library and fixes it. It is translated from the RVWorld 3.8.0 source (see [NOTICE](NOTICE)). It is not made or endorsed by the RomVault authors.

**Status: pre-alpha.** Four layers work. `rv-core` and `rv-cache` read and write ROMVault `.Cache` v2/v3 files byte for byte; `rv-dat` reads ClrMamePro, XML, MAME, software-list, DOSCenter and RomCenter DATs and writes the XML, MAME XML and DOSCenter formats ROMVault writes; `rv-scan` hashes files and detects copier headers; `rv-archive` reads zips, writes TorrentZips byte for byte, reads 7z archives and verifies CHDs the way ROMVault does. The `rv` and `rv-gtk` programs do nothing yet.

## Crates

Responsibilities. `rv-core` (cache data model only), `rv-cache`, `rv-dat` (parsed DATs only, see below), `rv-scan` and `rv-archive` (readers and the TorrentZip writer; no 7z writer) are implemented; every other crate is an empty placeholder today.

| Crate | Notes |
|---|---|
| `rv-core` | Data model (the cache tree is done); settings will stay XML-compatible with `RomVault3cfg.xml` |
| `rv-cache` | Byte-compatible `.Cache` v2/v3 read+write. **Fixes:** temp file, fsync, then rename (the old file is kept as `.CacheBackup`); falls back to `.CacheBackup` on corruption instead of exiting |
| `rv-dat` | Readers for ClrMamePro text (and RAINE), XML (`<datafile>`, MAME `<mame>`/`<mess>`), software lists, DOSCenter and RomCenter; writers for XML, MAME XML and DOSCenter, which are the ones ROMVault has (it has no JSON reader or writer, and none for ClrMamePro, RomCenter or software lists). Stops at the parsed DAT: ROMVault's clean-up pipeline and the merge into the database come in a later phase |
| `rv-scan` | CRC32/MD5/SHA1/SHA256 in one pass over any `Read`, copier-header detection and header-stripped ("alt") hashes, hash comparison. Does not open archives. **Fixed:** inverted MD5 alternate match, SNES/PCE header detection that could never fire |
| `rv-archive` | zip read and byte-exact TorrentZip (own writer and own port of ROMVault's deflate, because stock zlib does not reproduce its streams), 7z read, CHD read and verify (v1-v5; no child CHDs, no `avhu`). **Fixes:** the writers produce a temp file that the caller fsyncs and renames, so nothing is deleted before its replacement is on disk |
| `rv-fix` | **Fixes:** write to temp, fsync, then rename, never delete-then-rename; validate before the final name; no process exit without saving |
| `rv-online` | Rebuilt from the Phase 0 spec. **Error telemetry (`SendErrorMessage*`) is not kept** |
| `rv-update` | GitHub Releases API, semver compare, sha256 check, replaces the binary only after you confirm |
| `rv-cli` | Every engine feature headless, with `--dry-run` on fix (binary `rv`) |
| `rv-gtk` | The GTK4 UI (binary `rv-gtk`) |

The library crates sit under the two binaries (`rv-cli`, `rv-gtk`); nothing depends on `rv-gtk`.

## Build

You need a Rust toolchain (edition 2021) and the GTK4 development files (`libgtk-4-dev` on Debian/Ubuntu, `gtk4` on Arch).

```sh
cargo build --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Tests that need the maintainer's own ROMVault files (DATs, caches, ROMs) are `#[ignore]`d and never run in CI. Run them with `RV_FIXTURES_DIR=<dir> cargo test --workspace -- --include-ignored`; they fail, rather than pass, when the variable or a file is missing.

## License

Apache-2.0, see [LICENSE](LICENSE) and [NOTICE](NOTICE).
