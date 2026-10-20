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
4. writes `SHA256SUMS` for the two files;
5. creates a **draft** release for the tag with `rv-linux-x86_64`, `rv-gtk-linux-x86_64` and
   `SHA256SUMS`, targeting that commit (`--draft --latest=false`; a pre-release version such as
   `0.1.0-rc.1` is also flagged as a pre-release).

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
sha256sum -c SHA256SUMS                  # both files: OK
./rv-linux-x86_64 --version              # rv 0.1.0
./rv-gtk-linux-x86_64 --version          # rv-gtk 0.1.0
```

Also check that the draft's commit (`gh release view v0.1.0 --json targetCommitish`) is the
commit you meant, and that the run that made it (linked in the draft's notes) was green.

## Publish (the board)

Publishing is not done by the workflow, by `rv`, or by an agent. When the draft is right, publish
it in the GitHub UI (or `gh release edit v0.1.0 --draft=false`). Publishing creates the tag at
the draft's commit. For a stable release, leave "Set as the latest release" on: `rv update
check` reads `releases/latest`.

## Discarding a draft

`gh release delete v0.1.0 --repo Dromares/rv-rs --yes` (without `--cleanup-tag`: there is no tag
to clean). Delete test drafts the same way once you are done with them.

## Limits

* Linux x86_64 only. The files are linked against the glibc of `ubuntu-latest` and `rv-gtk`
  needs GTK 4.12 or newer installed.
* The update check reads `releases/latest` anonymously; GitHub rate-limits anonymous API calls
  (60 an hour per address), and a check that is rate-limited is reported as an error, not as "up
  to date".
