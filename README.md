# rv-rs

An unofficial, unaffiliated Linux rewrite of [RomVault](https://github.com/RomVault/RVWorld) in Rust: a ROM collection manager that reads DAT files, scans your ROM library and fixes it. It is translated from the RVWorld 3.8.0 source (see [NOTICE](NOTICE)). It is not made or endorsed by the RomVault authors.

**Status: pre-alpha.** Only the cache layer works: `rv-core` and `rv-cache` read and write ROMVault `.Cache` v2/v3 files byte for byte. The `rv` and `rv-gtk` programs do nothing yet.

## Crates

Responsibilities. `rv-core` (cache data model only) and `rv-cache` are implemented; every other crate is an empty placeholder today.

| Crate | Notes |
|---|---|
| `rv-core` | Data model (the cache tree is done); settings will stay XML-compatible with `RomVault3cfg.xml` |
| `rv-cache` | Byte-compatible `.Cache` v2/v3 read+write. **Fixes:** temp file, fsync, then rename (the old file is kept as `.CacheBackup`); falls back to `.CacheBackup` on corruption instead of exiting |
| `rv-dat` | CMP/XML/MAME/ROMCenter/DOS/JSON readers and writers |
| `rv-scan` | CRC32/MD5/SHA1/SHA256, copier headers. **Fixes:** inverted MD5 alternate match, SNES/PCE header detection |
| `rv-archive` | zip and byte-exact TorrentZip (own writer), 7z, CHD read |
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

## License

Apache-2.0, see [LICENSE](LICENSE) and [NOTICE](NOTICE).
