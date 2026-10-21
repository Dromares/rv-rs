# Releasing

How a release of rv-rs is cut. **Publishing is a manual step done by the board.** The workflow
only ever creates a *draft*; nothing in this repository publishes a release, pushes a tag or
marks a release as the latest one.

## What the workflow does

`.github/workflows/release.yml` runs only when dispatched by hand (`workflow_dispatch`, input:
the tag name). At the commit it is dispatched on it:

1. checks that the tag is `v<version>` where `<version>` is `[workspace.package] version` in
   `Cargo.toml` at that commit, and fails if they differ;
2. fails if the tag or a release for it already exists;
3. builds `rv` and `rv-gtk` for Linux x86_64 (`cargo build --release --locked`, on
   `ubuntu-latest` with `libgtk-4-dev`), strips them, and checks that `rv --version` and
   `rv-gtk --version` print the version the tag names;
4. packs the stripped `rv-gtk` as `rv-gtk-x86_64.AppImage` (`packaging/build-appimage.sh`):
   GTK4 and what it loads are bundled, with their licence texts, behind the static type-2 runtime
   (no `libfuse2` needed). The build tools (linuxdeploy, its GTK plugin, appimagetool, the
   runtime) are downloaded from fixed release URLs (a fixed commit for the plugin) and each is
   checked against a SHA-256 written in the workflow; a mismatch fails the job. The step then runs
   the AppImage with `--version` and checks it prints the version the tag names;
5. writes `SHA256SUMS` for the three files;
6. creates a **draft** release for the tag with `rv-linux-x86_64`, `rv-gtk-linux-x86_64`,
   `rv-gtk-x86_64.AppImage` and `SHA256SUMS`, targeting that commit (`--draft --latest=false`; a
   pre-release version such as `0.1.0-rc.1` is also flagged as a pre-release).

It has `contents: write` permission and uses the job's `GITHUB_TOKEN`; it uses no other secret.
A draft release has no git tag: GitHub creates the tag only when the draft is published, so the
workflow pushes none, and `git ls-remote --tags origin` shows nothing after it ran.

## The version and the tag

The version of the program is the workspace version in `Cargo.toml`, and it is what `rv
--version`, `rv-gtk --version` and the update check compare. The tag is always `v` plus that
version. To cut `0.1.0`:

1. In a pull request, set `[workspace.package] version = "0.1.0"` in `Cargo.toml` (and let
   `Cargo.lock` follow: `cargo build`), merge it.
2. Dispatch the workflow on `main` with the tag `v0.1.0`:
   `gh workflow run release.yml --ref main -f tag=v0.1.0`.

A tag with a `-` (`v0.1.0-rc.1`) is a pre-release: `rv update` ignores it unless
`include_prereleases=1` is set.

## Check a draft before publishing

A draft is visible to people with write access only (the anonymous API and the download URLs
answer 404). Download its files with `gh`, which is authenticated:

```sh
gh release download v0.1.0 --repo Dromares/rv-rs --dir /tmp/rv-draft
cd /tmp/rv-draft
sha256sum -c SHA256SUMS                  # all three files: OK
chmod +x rv-linux-x86_64 rv-gtk-linux-x86_64 rv-gtk-x86_64.AppImage   # gh does not keep the mode
./rv-linux-x86_64 --version              # rv 0.1.0
./rv-gtk-linux-x86_64 --version          # rv-gtk 0.1.0
./rv-gtk-x86_64.AppImage --version       # rv-gtk 0.1.0
```

The draft must list exactly `rv-linux-x86_64`, `rv-gtk-linux-x86_64`, `rv-gtk-x86_64.AppImage`
and `SHA256SUMS`. Start the AppImage once (`./rv-gtk-x86_64.AppImage`, no flags, on a machine
where FUSE works) and see the window open.

Also check that the draft's commit (`gh release view v0.1.0 --json targetCommitish`) is the
commit you meant, and that the run that made it (linked in the draft's notes) was green.

### Run the network path of the draft's `rv`

`--version` and `--help` never touch the network. 0.1.0 and 0.2.0 passed them and then panicked
on the first `https://` request (`No CryptoProvider for Rustls`). So run `rv update check` from
the draft's own downloaded `rv-linux-x86_64`, in an empty directory, with an empty `HOME`:

```sh
mkdir /tmp/rv-draft-net && cp /tmp/rv-draft/rv-linux-x86_64 /tmp/rv-draft-net/ && cd /tmp/rv-draft-net
env HOME=$(mktemp -d) ./rv-linux-x86_64 update check --update-config $PWD/update.conf; echo exit=$?
```

(`--update-config` is the flag `rv --help` lists for the update settings; the file need not
exist, and the empty `HOME` keeps your own `~/.config/rv-rs/update.conf` out of it.)

A pass is exactly this, and paste it as it came out:

```
rv <draft version> is up to date (the newest release is <latest published version>).
exit=0
```

with nothing on stderr, and no `panicked` or `CryptoProvider` anywhere. Any other result fails
the draft:

* `rv: cannot check for updates: ...` and `exit=1`: the request failed (a TLS error, no network,
  or the anonymous API rate limit). A rate limit is not a pass; wait and run it again.
* `rv <draft version>: version X is available: <url>` (and a second line, "Run rv update install
  to install it."): the newest *published* release is newer than the draft, so this is the wrong
  draft, or the version in `Cargo.toml` was not raised.

This is a valid check against a draft because the anonymous API sees only the published Latest,
never the draft: the binary compares its own version with that Latest, and the request is still a
real TLS call to `api.github.com`. Its first line names the Latest, so check that it is the
release you expect.

The AppImage and `rv-gtk-linux-x86_64` cannot take this step: `rv-gtk` has no `update`
subcommand (`./rv-gtk-x86_64.AppImage update check` prints `unknown argument update` and its
usage) and its updater runs inside the window. It uses the same `rv-update` crate as `rv`, so
the draft's `rv` is the command-line evidence of that code, and no more.

## Publish (the board)

Publishing is not done by the workflow, by `rv`, or by an agent. When the draft is right, publish
it in the GitHub UI (or `gh release edit v0.1.0 --draft=false`). Do not run
`gh release edit --draft=false` (or press the button) unless the verdict on the draft check for
these assets pastes the output of the "Run the network path" step above. If the assets have not
changed since that output, reuse it; do not run it again. Publishing creates the tag at
the draft's commit. For a stable release, leave "Set as the latest release" on: `rv update
check` reads `releases/latest`.

## Discarding a draft

`gh release delete v0.1.0 --repo Dromares/rv-rs --yes` (without `--cleanup-tag`: there is no tag
to clean). Delete test drafts the same way once you are done with them.

## Limits

* Linux x86_64 only. The files are linked against the glibc of `ubuntu-latest`. The plain
  `rv-gtk` needs GTK 4.12 or newer installed; the AppImage carries its own GTK4 but not glibc, so
  it needs a glibc at least as new as the one it was built with (the test build on Ubuntu 24.04 needs 2.38).
* An AppImage updates itself from the AppImage of a newer release: a release without
  `rv-gtk-x86_64.AppImage` (0.2.1 and earlier) cannot update one.
* The update check reads `releases/latest` anonymously; GitHub rate-limits anonymous API calls
  (60 an hour per address), and a check that is rate-limited is reported as an error, not as "up
  to date".
