# rv-rs

An unofficial, unaffiliated Linux rewrite of [RomVault](https://github.com/RomVault/RVWorld) in Rust: a ROM collection manager that reads DAT files, scans your ROM library and fixes it. It is translated from the RVWorld 3.8.0 source (see [NOTICE](NOTICE)). It is not made or endorsed by the RomVault authors.

**Status: pre-alpha.** The database layers work. `rv-core` and `rv-cache` read and write ROMVault `.Cache` v2/v3 files byte for byte and compute every `RepStatus`; `rv-dat` reads ClrMamePro, XML, MAME, software-list, DOSCenter and RomCenter DATs, writes the XML, MAME XML and DOSCenter formats ROMVault writes, and updates the database from the DATs under `DatRoot`; `rv-scan` hashes files and scans the ROM directories into the database; `rv-archive` reads zips, writes TorrentZips byte for byte, reads 7z archives and verifies CHDs the way ROMVault does. `rv-fix` finds what is wrong with the library and fixes it without losing a file, and the `rv` program runs the whole engine from the command line (`update-dats`, `scan`, `find-fixes`, `fix`, `report`, `status`). `rv-online` is the client of ROMVault's online services (DatVault key check, group list and DAT download, the MIA list); it is built from the recovered protocol and has only been run against a mock on 127.0.0.1 and a local HTTPS test server, never against the real service. TLS was broken in 0.1.0 and 0.2.0 (the first `https://` request panicked: no rustls crypto provider); that is fixed in 0.2.1. `rv-gtk` is a working GTK4 window over the engine: the DAT tree with its counts, the game and ROM lists, Update DATs, Scan, Find Fixes, Fix Files, the Online menu and the settings. Fix Files asks first, with the number of files it will write, move and delete and a dry-run view of the plan; the files are written by `rv-fix`, as `rv fix` does.

## Crates

Responsibilities. `rv-core` (the database tree, statuses, settings), `rv-cache`, `rv-dat` (readers, writers, the DAT update), `rv-scan` (hashing and the directory scan), `rv-archive` (readers and the TorrentZip writer; no 7z writer), `rv-fix` (Find Fixes, Fix, the plan and the FixDat report), `rv-online`, `rv-update`, `rv-cli` (the `rv` program) and `rv-gtk` (the window) are implemented.

| Crate | Notes |
|---|---|
| `rv-core` | The database tree (the cache model plus in-memory `RepStatus`), the `RepStatus` table and `DirStatus` counters, `RomVault3cfg.xml` settings (DAT rules, directory mappings, ignore patterns; read, and written back with the elements it does not model kept as they were), a guard that refuses directories the maintainer has fenced off, the name sorters, the copier-header table, progress and cancel types. No global state |
| `rv-cache` | Byte-compatible `.Cache` v2/v3 read+write. **Fixes:** temp file, fsync, then rename (the old file is kept as `.CacheBackup`); falls back to `.CacheBackup` on corruption instead of exiting |
| `rv-dat` | Readers for ClrMamePro text (and RAINE), XML (`<datafile>`, MAME `<mame>`/`<mess>`), software lists, DOSCenter and RomCenter; writers for XML, MAME XML and DOSCenter, which are the ones ROMVault has (it has no JSON reader or writer, and none for ClrMamePro, RomCenter or software lists). Also ROMVault's 18-step DAT clean-up and the DAT update: walk `DatRoot`, take out DATs that changed or are gone, merge new ones into the database. **Differences:** a DAT that fails to clean up is skipped, not fatal; `/` in every path |
| `rv-scan` | CRC32/MD5/SHA1/SHA256 in one pass over any `Read`, copier-header detection and header-stripped ("alt") hashes, hash comparison; the directory scan into the database (zips, 7z, loose files, CHDs, scan levels, ToSort). **Fixed:** inverted MD5 alternate match, SNES/PCE header detection that could never fire, per-directory rule lookups that append a `\\` to a `/` path. **Differences:** read-only (stale `__RomVault.*.tmp` files are reported, not deleted); no periodic cache save |
| `rv-archive` | zip read and byte-exact TorrentZip (own writer and own port of ROMVault's deflate, because stock zlib does not reproduce its streams), 7z read, CHD read and verify (v1-v5; no child CHDs, no `avhu`). **Fixes:** the writers produce a temp file that the caller fsyncs and renames, so nothing is deleted before its replacement is on disk |
| `rv-fix` | Find Fixes (the file groups, the fix status of every file, `CompleteOnly`, ToSort empty files), Fix (loose files, TorrentZip rebuilds, ToSort and the corrupt folder, source selection, double-checked deletes), the dry-run plan and the FixDat report. **Fixes:** every file and archive is written to a temp file, `fsync`ed, read back and compared with the DAT, and only then renamed into place; a source is removed only after its copy is on disk and proven; the old archive is replaced by one rename, never deleted first; the run never exits without saving, and "impossible" branches are errors, not `exit`; corrupt files are kept whole in `ToSort/Corrupt` (3.8.0 overwrites them, and numbers the file and the record differently). **Differences:** no raw copy and no 7z writer, so a zip that is not a TorrentZip/TDC/DTD and any 7z are moved whole but not rewritten (they are listed as left alone); a 7z source is decoded where it is, not extracted to a ToSort cache; the whole-archive "ZipMove" shortcut is replaced by a rebuild; Find Fixes is single-threaded so the source chosen is the same on every run; the `copy rules.txt` hook is not ported |
| `rv-online` | SOAP 1.1 client for the 12 operations ROMVault 3.8.0 calls that the rewrite keeps; DatVault (key check, groups, DAT download with seed files, status rules of the exe); MIA (fetch, cache, AES-256-CBC with the `PasswordDeriveBytes` key derivation, apply to the tree); opt-in found-report library. `https://` by default, `http://` only with `allow_plain_http`. The MIA salt is read at run time from your own `ROMVault38.exe` and is not in this repository. **Not built:** error telemetry (`SendErrorMessage*`), AirDC++, the usage ping. **Not verified:** the wire format against the real service (no live call has been made), the key derivation against Microsoft's .NET runtime. TLS was broken in 0.1.0 and 0.2.0 (no rustls crypto provider, so the first `https://` request panicked); fixed in 0.2.1; the real service is still untested. Tested against a mock on 127.0.0.1 and a local HTTPS server with a test certificate only |
| `rv-update` | Reads the newest release of `Dromares/rv-rs` from the GitHub Releases API (anonymously; pre-releases ignored unless a setting turns them on), compares it with the running version (semver), downloads the file for this system and `SHA256SUMS`, and replaces the program only if the hash matches and only after the caller confirmed: the new file is written next to the old one, `fsync`ed and hashed again from disk, the old program is kept as `<program>.rv-backup`, then one rename puts the new one in place. If the program's directory is not writable by you it installs nothing and reports the release page; it never asks for more rights. `rv-gtk` run from an AppImage (`APPIMAGE` names an existing file) replaces that AppImage file with the release's `rv-gtk-x86_64.AppImage`, by the same steps, and never with the bare binary. Linux x86_64 only. **Does not work in 0.1.0 or 0.2.0:** `rv update` panics there before any download (no rustls crypto provider); fixed in 0.2.1. 0.1.0 and 0.2.0 cannot update themselves, so download 0.2.1 by hand and run `sha256sum -c SHA256SUMS`. An update between two published releases has still not been done. It has been run against a mock of the Releases API on 127.0.0.1 (serving the files of a draft built by `release.yml`) and a local HTTPS server with a test certificate only |
| `rv-cli` | The `rv` program, every engine feature headless: `update-dats`, `scan [--level]`, `find-fixes`, `fix [--dry-run]`, `report` (FixDat) and `status`. Every path is given (`--cache`, `--root`, ...); none has a default, and a path inside `/mnt/HHD16TB/` is refused unless `--allow-live-library` is passed. Also `datvault check|list|fetch` and `mia update` (online settings in `~/.config/rv-rs/online.conf`), and `update check|install` (see below) |
| `rv-gtk` | The GTK4 UI (binary `rv-gtk`): `gtk4-rs` + `relm4`. DAT tree, game list and ROM list on `ListView`/`ColumnView` models that build only the rows on screen; Update DATs, Scan, Find Fixes and Fix Files run on a worker thread with progress and Cancel (Fix Files only after a confirmation that states the counts, with Cancel as the default and a dry-run view of the plan); Settings dialog that writes `RomVault3cfg.xml` back, with an Online page (DatVault key, ROMVault exe for MIA) and an Online menu (check key, update DATs from DatVault, update MIA), a Help menu with Check for updates (asks before it installs anything), an Updates page in Settings; file dialogs through the XDG portal. **Differences:** the status pictures are drawn, not ROMVault's bitmaps; Find Fixes does not save the cache; Fix Files runs Find Fixes again on the database as it is when you confirm; refuses any directory under `/mnt/HHD16TB`, Fix Files included, unless you tick Settings → General → Safety → "Allow paths under /mnt/HHD16TB". Every refusal names that switch: the red banner (with an Open Settings button) and, when Settings OK finds such a path, the dialog stays open, lists the paths and offers Allow and save |

The library crates sit under the two binaries (`rv-cli`, `rv-gtk`); nothing depends on `rv-gtk`.

## Install

`rv-gtk` as an AppImage (from release 0.2.2 on; it bundles GTK4 and needs no `libgtk-4` and no `libfuse2`):

```sh
sha256sum -c --ignore-missing SHA256SUMS    # in the folder you downloaded rv-gtk-x86_64.AppImage and SHA256SUMS to
chmod +x rv-gtk-x86_64.AppImage
./rv-gtk-x86_64.AppImage
```

The AppImage is Linux x86_64 only. Help > Check for updates in an AppImage replaces the AppImage file itself (the old one stays as `rv-gtk-x86_64.AppImage.rv-backup`) with the AppImage of the newer release, and never with the plain `rv-gtk-linux-x86_64` binary. The plain `rv` and `rv-gtk` binaries are still in each release, and still need GTK 4.12 or newer installed for `rv-gtk`.

## Using `rv`

```sh
rv update-dats --cache lib.Cache --root RomRoot --dat-root DatRoot
rv scan        --cache lib.Cache --root RomRoot            # --level 1|2|3
rv find-fixes  --cache lib.Cache --root RomRoot            # says what would be done, changes nothing
rv fix         --cache lib.Cache --root RomRoot --dry-run  # the same plan; writes nothing at all
rv fix         --cache lib.Cache --root RomRoot
rv report      --cache lib.Cache --root RomRoot --out fixdats
rv status      --cache lib.Cache --root RomRoot
```

### Online (DatVault and MIA)

```sh
rv datvault check                                   # is the key in online.conf valid?
rv datvault list                                    # the groups
rv datvault list  --group nointro --dat-root DatRoot   # its DATs and how they compare with yours
rv datvault fetch --group nointro --dat-root DatRoot   # download them (--updates: the groups you have)
rv mia update --cache lib.Cache --root RomRoot      # needs rom_vault_exe in online.conf
```

The settings are in `~/.config/rv-rs/online.conf` (`--online-config` names another file): `dat_vault_key`, `rom_vault_exe` (the path of your own ROMVault 3.8.0 exe; the MIA list is decrypted with a value read from it at run time), `allow_plain_http` (off), `endpoint`, `mia_days`. The key is stored in that file only, readable by you, and is never printed or logged. The service is reached over `https://`; a plain `http://` address is refused unless `allow_plain_http=1`. Reporting found MIA files is off, asks once before the first send, and is a library function (`rv_online::report`) that nothing in `rv` or `rv-gtk` calls yet. TLS was broken in 0.1.0 and 0.2.0 (the first `https://` request panicked); fixed in 0.2.1. Nothing here has been run against the real service.

### Updates

In 0.1.0 and 0.2.0 `rv update` panics before any download (no rustls crypto provider); fixed in 0.2.1. 0.1.0 and 0.2.0 cannot update themselves, so download 0.2.1 by hand and check it with `sha256sum -c SHA256SUMS`. An update between two published releases has still not been done.

```sh
rv update check                # is there a newer release of rv-rs on GitHub? changes nothing
rv update install              # shows what it would do and asks; --yes skips the question
rv --version
```

`rv update install` downloads the file for this system and the release's `SHA256SUMS`, refuses to go on if `SHA256SUMS` is missing, has no line for the file, or the hash differs, and replaces the running `rv` only after you answer yes. The old program stays as `rv.rv-backup` next to it. If its directory is not writable by you, it prints the release page and does nothing else. The settings are in `~/.config/rv-rs/update.conf` (`--update-config` names another file): `include_prereleases` (off), `check_on_startup` (off, `rv-gtk` only) and `api_base`. `rv-gtk` has Help > Check for updates and the same two switches in Settings; it updates itself, not `rv`. Run from an AppImage (the `APPIMAGE` variable names an existing file) it updates that file from the release's `rv-gtk-x86_64.AppImage`; a release without one (0.2.1 and earlier) is reported with its page and installs nothing. How a release is cut, and that publishing it is the board's step, is in [RELEASING.md](RELEASING.md).

`--tosort` defaults to a `ToSort` directory next to `--root`; `--config` reads a `RomVault3cfg.xml` for the DAT rules, with `--root`, `--tosort` and `--dat-root` replacing the directories it names. Ctrl-C stops at the next safe point and saves the cache. A crash leaves `__<name>.rvtmp` files, never a half-written file at a final name; the next `fix` removes them.

## Build

You need a Rust toolchain (edition 2021) and GTK 4.12 or newer with its development files (`libgtk-4-dev` on Debian/Ubuntu 24.04, `gtk4` on Arch).

```sh
cargo build --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

To run the window: `cargo run -p rv-gtk -- --config path/to/RomVault3cfg.xml`. The directory of the config file is where its relative paths (DatRoot, the cache, the ROM directories) are taken from, as with ROMVault. The window is tested headless: `RV_GTK_SMOKE=1 xvfb-run -a cargo test -p rv-gtk --test smoke` starts it on a synthetic collection and checks what it shows (without the variable the test says it is skipped, because most machines that run `cargo test` have no display).

The crash tests of `rv-fix` kill a child process at every step of a fix (`RV_FIX_FAILPOINT=<step>[:<nth>]` is the hook it uses) and check that nothing was lost. `cargo test -p rv-fix --features mutation-3-8-0-order --test crash` puts the order of 3.8.0 back (delete the archive, then rename the new one in) and the same test then fails, which is how the test is known to catch it.

Tests that need the maintainer's own ROMVault files (DATs, caches, ROMs) are `#[ignore]`d and never run in CI. Run them with `RV_FIXTURES_DIR=<dir> cargo test --workspace -- --include-ignored`; they fail, rather than pass, when the variable or a file is missing. The DAT-update and scan tests copy the DATs and hard-link the ROM files into `RV_WORK_DIR` (default: the temp directory) and only read them.

## License

Apache-2.0, see [LICENSE](LICENSE) and [NOTICE](NOTICE).
