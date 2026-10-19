# rv-rs

An unofficial, unaffiliated Linux rewrite of [RomVault](https://github.com/RomVault/RVWorld) in Rust: a ROM collection manager that reads DAT files, scans your ROM library and fixes it. It is translated from the RVWorld 3.8.0 source (see [NOTICE](NOTICE)). It is not made or endorsed by the RomVault authors.

**Status: pre-alpha.** The database layers work. `rv-core` and `rv-cache` read and write ROMVault `.Cache` v2/v3 files byte for byte and compute every `RepStatus`; `rv-dat` reads ClrMamePro, XML, MAME, software-list, DOSCenter and RomCenter DATs, writes the XML, MAME XML and DOSCenter formats ROMVault writes, and updates the database from the DATs under `DatRoot`; `rv-scan` hashes files and scans the ROM directories into the database; `rv-archive` reads zips, writes TorrentZips byte for byte, reads 7z archives and verifies CHDs the way ROMVault does. Find fixes and fix are not written yet, and the `rv` and `rv-gtk` programs do nothing yet.

## Crates

Responsibilities. `rv-core` (the database tree, statuses, settings), `rv-cache`, `rv-dat` (readers, writers, the DAT update), `rv-scan` (hashing and the directory scan) and `rv-archive` (readers and the TorrentZip writer; no 7z writer) are implemented; every other crate is an empty placeholder today.

| Crate | Notes |
|---|---|
| `rv-core` | The database tree (the cache model plus in-memory `RepStatus`), the `RepStatus` table and `DirStatus` counters, `RomVault3cfg.xml` settings (DAT rules, directory mappings, ignore patterns), the name sorters, the copier-header table, progress and cancel types. No global state |
| `rv-cache` | Byte-compatible `.Cache` v2/v3 read+write. **Fixes:** temp file, fsync, then rename (the old file is kept as `.CacheBackup`); falls back to `.CacheBackup` on corruption instead of exiting |
| `rv-dat` | Readers for ClrMamePro text (and RAINE), XML (`<datafile>`, MAME `<mame>`/`<mess>`), software lists, DOSCenter and RomCenter; writers for XML, MAME XML and DOSCenter, which are the ones ROMVault has (it has no JSON reader or writer, and none for ClrMamePro, RomCenter or software lists). Also ROMVault's 18-step DAT clean-up and the DAT update: walk `DatRoot`, take out DATs that changed or are gone, merge new ones into the database. **Differences:** a DAT that fails to clean up is skipped, not fatal; `/` in every path |
| `rv-scan` | CRC32/MD5/SHA1/SHA256 in one pass over any `Read`, copier-header detection and header-stripped ("alt") hashes, hash comparison; the directory scan into the database (zips, 7z, loose files, CHDs, scan levels, ToSort). **Fixed:** inverted MD5 alternate match, SNES/PCE header detection that could never fire, per-directory rule lookups that append a `\\` to a `/` path. **Differences:** read-only (stale `__RomVault.*.tmp` files are reported, not deleted); no periodic cache save |
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

Tests that need the maintainer's own ROMVault files (DATs, caches, ROMs) are `#[ignore]`d and never run in CI. Run them with `RV_FIXTURES_DIR=<dir> cargo test --workspace -- --include-ignored`; they fail, rather than pass, when the variable or a file is missing. The DAT-update and scan tests copy the DATs and hard-link the ROM files into `RV_WORK_DIR` (default: the temp directory) and only read them.

## License

Apache-2.0, see [LICENSE](LICENSE) and [NOTICE](NOTICE).
