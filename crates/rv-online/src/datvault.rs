//! DatVault: key validation, the group list, and DAT download into the DAT root
//! (`DatVault` and `FrmDatVault`, `online-protocol.md` sections 5.1 and 5.2).
//!
//! A DAT that was fetched has a **seed** next to it: `<system>.json`, the [`DatInfo`] the server
//! gave for it, in the directory the DAT is in. Comparing the seeds under the DAT root with what
//! the server lists gives each DAT a [`DatUpdate`] status, exactly as the exe's
//! `UpdateMasterLists` does.
//!
//! **Differences from the exe**, on purpose:
//! * the exe deletes the old DAT file before it asks for the new one; here the new DAT is
//!   downloaded and read out of its zip first, and only then does it replace the old one;
//! * the exe tests the wrong variable after `GetDVCue` (`@0186`); here a missing cue is noticed;
//! * every name the server sends (`basepath`, `group`, `directory`, `system`, `datName`,
//!   `cueURL`) is checked before it becomes a path: it may not leave the DAT root;
//! * a network error while listing a group is an error, not "this group is gone" (which would
//!   mark every DAT of it discontinued);
//! * `PurgeOldDats` (deleting `*.old` files) is not ported.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use rv_core::progress::{CancelFlag, ProgressEvent, ProgressSink};
use serde::{Deserialize, Deserializer, Serialize};

use crate::client::{Client, Group};
use crate::error::{Error, Result};
use crate::fsio::write_atomic;

/// The largest group listing, DAT or cue accepted once decompressed.
const MAX_INFO: u64 = 256 * 1024 * 1024;
const MAX_DAT: u64 = 2 * 1024 * 1024 * 1024;

fn null_string<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

fn null_i64<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<i64, D::Error> {
    Ok(Option::<i64>::deserialize(d)?.unwrap_or_default())
}

/// One DAT of a group, and the seed file format (`DatInfo`, System.Text.Json names verbatim).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatInfo {
    #[serde(default, deserialize_with = "null_string")]
    pub group: String,
    #[serde(default, deserialize_with = "null_string")]
    pub system: String,
    #[serde(default, deserialize_with = "null_string")]
    pub date: String,
    #[serde(default, rename = "dateCreated", deserialize_with = "null_string")]
    pub date_created: String,
    #[serde(default, deserialize_with = "null_string")]
    pub version: String,
    #[serde(default, rename = "datURL", deserialize_with = "null_string")]
    pub dat_url: String,
    #[serde(default, rename = "cueURL", deserialize_with = "null_string")]
    pub cue_url: String,
    #[serde(default, rename = "datName", deserialize_with = "null_string")]
    pub dat_name: String,
    #[serde(default, deserialize_with = "null_string")]
    pub directory: String,
    #[serde(default, rename = "datROMsSize", deserialize_with = "null_i64")]
    pub dat_roms_size: i64,
}

/// `datUpdate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DatUpdate {
    None = 0,
    /// Up to date.
    Good = 1,
    /// Same version, another date.
    Replace = 2,
    /// Another version.
    Update = 3,
    /// Not listed any more.
    Discontinued = 4,
    /// The server does not know the group.
    Unknown = 5,
    Failed = 6,
    /// Up to date on record, but the DAT file is gone.
    Missing = 7,
    /// Listed, and not here.
    Available = 8,
}

impl DatUpdate {
    pub fn name(self) -> &'static str {
        match self {
            DatUpdate::None => "none",
            DatUpdate::Good => "good",
            DatUpdate::Replace => "replace",
            DatUpdate::Update => "update",
            DatUpdate::Discontinued => "discontinued",
            DatUpdate::Unknown => "unknown",
            DatUpdate::Failed => "failed",
            DatUpdate::Missing => "missing",
            DatUpdate::Available => "available",
        }
    }
}

/// Zstandard, then the JSON array of [`DatInfo`]. `None` for what the exe treats as "no such
/// group": nothing, or a single byte.
pub fn decode_group_info(bytes: &[u8]) -> Result<Option<Vec<DatInfo>>> {
    if bytes.len() <= 1 {
        return Ok(None);
    }
    let bad = |m: String| Error::BadResponse(format!("group listing: {m}"));
    let mut dec = ruzstd::decoding::StreamingDecoder::new(bytes).map_err(|e| bad(e.to_string()))?;
    let mut json = Vec::new();
    (&mut dec)
        .take(MAX_INFO + 1)
        .read_to_end(&mut json)
        .map_err(|e| bad(e.to_string()))?;
    if json.len() as u64 > MAX_INFO {
        return Err(bad("it is larger than allowed".into()));
    }
    serde_json::from_slice::<Vec<DatInfo>>(&json)
        .map(Some)
        .map_err(|e| bad(e.to_string()))
}

/// A group listing as the service sends it (Zstandard over the JSON). For the tests and the mock.
#[cfg(any(test, feature = "mock"))]
pub fn encode_group_info(infos: &[DatInfo]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(
        &serde_json::to_vec(infos).expect("serializable")[..],
        ruzstd::encoding::CompressionLevel::Uncompressed,
    )
}

// ---- names from the server become paths -------------------------------------------------------

/// The parts of a relative path the server named, `\` taken for `/` (`FixSlash`). `..`, an
/// absolute path and a NUL are errors; empty and `.` parts are dropped.
fn rel_parts(s: &str) -> Result<Vec<String>> {
    let unsafe_path = || Error::UnsafePath(s.to_owned());
    if s.contains('\0') {
        return Err(unsafe_path());
    }
    let s = s.replace('\\', "/");
    if s.starts_with('/') {
        return Err(unsafe_path());
    }
    let mut out = Vec::new();
    for p in s.split('/') {
        match p {
            "" | "." => {}
            ".." => return Err(unsafe_path()),
            p => out.push(p.to_owned()),
        }
    }
    Ok(out)
}

/// A single file name: no separators, not empty, not `.` or `..`.
fn file_name(s: &str) -> Result<&str> {
    if s.is_empty() || s == "." || s == ".." || s.contains(['/', '\\', '\0']) {
        return Err(Error::UnsafePath(s.to_owned()));
    }
    Ok(s)
}

fn join(root: &Path, dirs: &[&str], name: &str) -> Result<PathBuf> {
    let mut p = root.to_path_buf();
    for d in dirs {
        for part in rel_parts(d)? {
            p.push(part);
        }
    }
    p.push(file_name(name)?);
    Ok(p)
}

// ---- the seeds on disk ------------------------------------------------------------------------

/// A seed file found under the DAT root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSeed {
    pub json_path: PathBuf,
    pub info: DatInfo,
    /// `File.Exists(<seed dir>/<datName>)`.
    pub dat_exists: bool,
}

impl LocalSeed {
    fn dir(&self) -> &Path {
        self.json_path.parent().unwrap_or(Path::new("."))
    }
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 32 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let Ok(t) = e.file_type() else { continue };
        if t.is_symlink() {
            continue; // a link could lead out of the DAT root
        }
        let p = e.path();
        if t.is_dir() {
            walk(&p, depth + 1, out);
        } else if p
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("json"))
        {
            out.push(p);
        }
    }
}

/// Every `*.json` under `dat_root`. A file that is not a seed is a warning and is ignored (the
/// exe says the same: "This file will be ignored").
pub fn read_seeds(dat_root: &Path) -> (Vec<LocalSeed>, Vec<String>) {
    let mut files = Vec::new();
    walk(dat_root, 0, &mut files);
    let mut seeds = Vec::new();
    let mut warnings = Vec::new();
    for f in files {
        let parsed = std::fs::read(&f)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<DatInfo>(&b).map_err(|e| e.to_string()));
        match parsed {
            Ok(info) => {
                let dat_exists = !info.dat_name.is_empty()
                    && file_name(&info.dat_name).is_ok()
                    && f.parent().is_some_and(|d| d.join(&info.dat_name).is_file());
                seeds.push(LocalSeed {
                    json_path: f,
                    info,
                    dat_exists,
                });
            }
            Err(e) => warnings.push(format!(
                "{} is not a seed file and is ignored: {e}",
                f.display()
            )),
        }
    }
    (seeds, warnings)
}

// ---- comparing --------------------------------------------------------------------------------

/// One DAT: what is here, what the server lists, and what to do (`DatInfoPair`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    /// `group|system`.
    pub key: String,
    pub local: Option<LocalSeed>,
    pub remote: Option<DatInfo>,
    pub status: DatUpdate,
    /// Whether a fetch does it by default.
    pub checked: bool,
}

/// The status rules of `UpdateMasterLists` (`online-protocol.md` 5.2). `group_known` is whether
/// the server listed the pair's group at all.
pub fn status_of(
    local: Option<&LocalSeed>,
    remote: Option<&DatInfo>,
    group_known: bool,
) -> (DatUpdate, bool) {
    match (local, remote) {
        (Some(l), Some(r)) => {
            let same_version = l.info.version == r.version;
            let same_date = l.info.date == r.date;
            let mut status = DatUpdate::Good;
            let mut checked = true;
            if same_version && same_date && !l.dat_exists {
                status = DatUpdate::Missing;
            }
            if same_version && !same_date {
                status = DatUpdate::Replace;
            }
            if !same_version {
                status = DatUpdate::Update;
            }
            if same_version && same_date && l.dat_exists {
                checked = false;
            }
            (status, checked)
        }
        (Some(_), None) if group_known => (DatUpdate::Discontinued, false),
        (Some(_), None) => (DatUpdate::Unknown, false),
        (None, Some(_)) => (DatUpdate::Available, false),
        (None, None) => (DatUpdate::None, false),
    }
}

/// The result of comparing.
#[derive(Debug, Default)]
pub struct Plan {
    pub pairs: Vec<Pair>,
    pub warnings: Vec<String>,
}

/// Compare the seeds with what the server listed. `listed` has an entry per group asked about:
/// `Some(list)` or `None` when the server had nothing for it.
pub fn compare(
    seeds: Vec<LocalSeed>,
    listed: &BTreeMap<String, Option<Vec<DatInfo>>>,
) -> Vec<Pair> {
    let mut map: BTreeMap<String, Pair> = BTreeMap::new();
    for s in seeds {
        let key = format!("{}|{}", s.info.group, s.info.system);
        map.insert(
            key.clone(),
            Pair {
                key,
                local: Some(s),
                remote: None,
                status: DatUpdate::None,
                checked: false,
            },
        );
    }
    for infos in listed.values().flatten() {
        for r in infos {
            let key = format!("{}|{}", r.group, r.system);
            match map.get_mut(&key) {
                Some(p) => p.remote = Some(r.clone()),
                None => {
                    map.insert(
                        key.clone(),
                        Pair {
                            key,
                            local: None,
                            remote: Some(r.clone()),
                            status: DatUpdate::None,
                            checked: false,
                        },
                    );
                }
            }
        }
    }
    for p in map.values_mut() {
        let group = p
            .local
            .as_ref()
            .map(|l| l.info.group.as_str())
            .or(p.remote.as_ref().map(|r| r.group.as_str()))
            .unwrap_or_default();
        let known = matches!(listed.get(group), Some(Some(_)));
        let (s, c) = status_of(p.local.as_ref(), p.remote.as_ref(), known);
        p.status = s;
        p.checked = c;
    }
    map.into_values().collect()
}

// ---- the service ------------------------------------------------------------------------------

pub struct Vault<'a> {
    client: &'a Client,
    key: &'a str,
}

/// What to do besides the DATs.
#[derive(Clone, Debug, Default)]
pub struct FetchOptions {
    /// Where cue files go (`<ToSort>/cues`); none: cues are not fetched.
    pub cues_dir: Option<PathBuf>,
    /// `Settings.DeleteOldCueFiles`.
    pub delete_old_cue_files: bool,
    /// Take discontinued DATs out (the DAT becomes `.old`, the seed goes).
    pub remove_discontinued: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct FetchReport {
    pub downloaded: Vec<String>,
    pub removed: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub cancelled: bool,
}

impl<'a> Vault<'a> {
    pub fn new(client: &'a Client, key: &'a str) -> Result<Vault<'a>> {
        if key.trim().is_empty() {
            return Err(Error::KeyRequired);
        }
        Ok(Vault { client, key })
    }

    /// `CheckLoginName`: the reply `"ok"` in any case is a valid key; any other text is not.
    pub fn check_key(&self) -> Result<bool> {
        Ok(self.client.check_dv_user(self.key)?.to_lowercase() == "ok")
    }

    /// `GetGroupList`, sorted by base path and then group.
    pub fn groups(&self) -> Result<Vec<Group>> {
        let mut g = self.client.list_of_groups_v2(self.key)?;
        g.sort_by(|a, b| (&a.basepath, &a.group).cmp(&(&b.basepath, &b.group)));
        Ok(g)
    }

    /// `GetSourceList` for one group.
    pub fn group_info(&self, group: &str) -> Result<Option<Vec<DatInfo>>> {
        match self.client.get_dv_group_info_v2(self.key, group)? {
            None => Ok(None),
            Some(b) => decode_group_info(&b),
        }
    }

    /// `UpdateMasterLists`: read the seeds under `dat_root`, list the groups they belong to and
    /// the groups in `extra_groups`, and compare.
    pub fn plan(
        &self,
        dat_root: &Path,
        extra_groups: &[String],
        sink: &dyn ProgressSink,
    ) -> Result<Plan> {
        sink.report(ProgressEvent::Text("Scanning DatRoot".into()));
        let (seeds, warnings) = read_seeds(dat_root);
        self.plan_seeds(seeds, warnings, extra_groups, sink)
    }

    /// [`Vault::plan`] for seeds already in hand (none: just list the groups).
    pub fn plan_seeds(
        &self,
        seeds: Vec<LocalSeed>,
        warnings: Vec<String>,
        extra_groups: &[String],
        sink: &dyn ProgressSink,
    ) -> Result<Plan> {
        let mut wanted: BTreeSet<String> = seeds.iter().map(|s| s.info.group.clone()).collect();
        wanted.extend(extra_groups.iter().cloned());
        let mut listed = BTreeMap::new();
        for (i, g) in wanted.iter().enumerate() {
            sink.report(ProgressEvent::Text(format!("Checking for updates: {g}")));
            sink.report(ProgressEvent::Overall {
                done: i as u64,
                total: wanted.len() as u64,
            });
            listed.insert(g.clone(), self.group_info(g)?);
        }
        Ok(Plan {
            pairs: compare(seeds, &listed),
            warnings,
        })
    }

    /// Do what `plan` found, for the pairs that are `checked` (and, with
    /// `opts.remove_discontinued`, the discontinued ones). `groups` is the list from
    /// [`Vault::groups`], for the base path of a DAT that is not here yet. `plan` is updated.
    pub fn fetch(
        &self,
        dat_root: &Path,
        plan: &mut Plan,
        groups: &[Group],
        opts: &FetchOptions,
        sink: &dyn ProgressSink,
        cancel: &CancelFlag,
    ) -> FetchReport {
        let mut report = FetchReport::default();
        let todo: Vec<usize> = plan
            .pairs
            .iter()
            .enumerate()
            .filter(|(_, p)| match p.status {
                DatUpdate::Missing
                | DatUpdate::Update
                | DatUpdate::Replace
                | DatUpdate::Available => p.checked,
                DatUpdate::Discontinued => opts.remove_discontinued,
                _ => false,
            })
            .map(|(i, _)| i)
            .collect();
        for (n, i) in todo.iter().enumerate() {
            if cancel.is_cancelled() {
                report.cancelled = true;
                break;
            }
            sink.report(ProgressEvent::Overall {
                done: n as u64,
                total: todo.len() as u64,
            });
            let pair = &mut plan.pairs[*i];
            let key = pair.key.clone();
            let outcome = if pair.status == DatUpdate::Discontinued {
                self.remove(pair).map(|()| false)
            } else {
                sink.report(ProgressEvent::Text(format!("Downloading {key}")));
                self.download(dat_root, pair, groups, opts).map(|()| true)
            };
            match outcome {
                Ok(true) => report.downloaded.push(key),
                Ok(false) => report.removed.push(key),
                Err(e) => {
                    sink.report(ProgressEvent::Error {
                        file: key.clone(),
                        message: e.to_string(),
                    });
                    report.failed.push((key, e.to_string()));
                }
            }
        }
        report
    }

    /// `Delete(file, dGot)`: the DAT becomes `<name>.old`, the seed is deleted.
    fn remove(&self, pair: &mut Pair) -> Result<()> {
        let local = pair
            .local
            .as_ref()
            .ok_or_else(|| Error::BadArgument("no local seed".into()))?;
        let io = |e: std::io::Error| Error::Io(e.to_string());
        if local.info.dat_name.is_empty() {
            // nothing to rename
        } else {
            let dat = local.dir().join(file_name(&local.info.dat_name)?);
            if dat.is_file() {
                move_to_old(&dat)?;
            }
        }
        std::fs::remove_file(&local.json_path).map_err(io)?;
        pair.local = None;
        pair.status = DatUpdate::None;
        Ok(())
    }

    /// `AddNewSeed` (when the DAT is new) and `UpdateDat(...)`.
    fn download(
        &self,
        dat_root: &Path,
        pair: &mut Pair,
        groups: &[Group],
        opts: &FetchOptions,
    ) -> Result<()> {
        let new = pair
            .remote
            .clone()
            .ok_or_else(|| Error::BadArgument("the server no longer lists it".into()))?;
        let name = file_name(&new.dat_name)?.to_owned();
        let seed_name = format!("{}.json", file_name(&new.system)?);
        let (dir, old) = match &pair.local {
            Some(l) => (l.dir().to_path_buf(), Some(l.clone())),
            None => {
                let base = groups
                    .iter()
                    .find(|g| g.group == new.group)
                    .map(|g| g.basepath.as_str())
                    .ok_or_else(|| {
                        Error::BadArgument(format!("the server does not list group {}", new.group))
                    })?;
                let p = join(dat_root, &[base, &new.group, &new.directory], &seed_name)?;
                (
                    p.parent()
                        .expect("a joined path has a parent")
                        .to_path_buf(),
                    None,
                )
            }
        };
        let seed_path = dir.join(&seed_name);
        let json = serde_json::to_vec(&new).map_err(|e| Error::Io(e.to_string()))?;

        // A new DAT gets its seed first, so that a failed download is tried again next time.
        if old.is_none() {
            write_atomic(&seed_path, &json)?;
            pair.local = Some(LocalSeed {
                json_path: seed_path.clone(),
                info: new.clone(),
                dat_exists: false,
            });
            pair.status = DatUpdate::Missing;
        }

        let zip = self
            .client
            .get_dv_dat(self.key, &new.group, &new.system, &new.dat_url)?
            .ok_or_else(|| Error::BadResponse("the service has no such DAT".into()))?;
        let dat_bytes = extract_dat(&zip, &name)?;

        // Only now is anything replaced.
        if let Some(o) = &old {
            if o.info.dat_name != new.dat_name && !o.info.dat_name.is_empty() {
                let old_dat = dir.join(file_name(&o.info.dat_name)?);
                if old_dat.is_file() {
                    move_to_old(&old_dat)?;
                }
            }
        }
        write_atomic(&dir.join(&name), &dat_bytes)?;

        if !new.cue_url.trim().is_empty() {
            if let Some(cues) = &opts.cues_dir {
                let cue_name = file_name(&new.cue_url)?;
                let cue = self
                    .client
                    .get_dv_cue(self.key, &new.group, &new.system, &new.cue_url)?
                    .ok_or_else(|| Error::BadResponse("the service has no such cue file".into()))?;
                if opts.delete_old_cue_files {
                    if let Some(o) = &old {
                        if !o.info.cue_url.is_empty() && o.info.cue_url != new.cue_url {
                            let _ = std::fs::remove_file(cues.join(file_name(&o.info.cue_url)?));
                        }
                    }
                }
                write_atomic(&cues.join(cue_name), &cue)?;
            }
        }

        write_atomic(&seed_path, &json)?;
        pair.local = Some(LocalSeed {
            json_path: seed_path,
            info: new,
            dat_exists: true,
        });
        pair.status = DatUpdate::Good;
        pair.checked = false;
        Ok(())
    }
}

/// `<name>` to `<name>.old`, replacing an earlier `.old`.
fn move_to_old(path: &Path) -> Result<()> {
    let mut old = path.as_os_str().to_owned();
    old.push(".old");
    let old = PathBuf::from(old);
    if old.exists() {
        std::fs::remove_file(&old)
            .map_err(|e| Error::Io(format!("cannot remove {}: {e}", old.display())))?;
    }
    std::fs::rename(path, &old)
        .map_err(|e| Error::Io(format!("cannot rename {}: {e}", path.display())))
}

/// The entry `name` of the zip `bytes`.
fn extract_dat(bytes: &[u8], name: &str) -> Result<Vec<u8>> {
    let bad = |m: String| Error::BadResponse(format!("DAT zip: {m}"));
    let mut z = rv_archive::zip::ZipArchive::open(std::io::Cursor::new(bytes))
        .map_err(|e| bad(e.to_string()))?;
    let i = z
        .entries()
        .iter()
        .position(|e| !e.is_directory && e.name == name)
        .ok_or_else(|| bad(format!("it has no entry {name}")))?;
    if z.entries()[i].uncompressed_size > MAX_DAT {
        return Err(bad("the DAT is larger than allowed".into()));
    }
    z.read_entry(i).map_err(|e| bad(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{self, fault, loopback_client, MockReply, MockServer};
    use ruzstd::encoding::{compress_to_vec, CompressionLevel};
    use rv_archive::zip::{TorrentZipWriter, ZipStructure};
    use rv_core::progress::NoProgress;

    const KEY: &str = "KEY-1234-secret";

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rv-online-dv-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn info(group: &str, system: &str, version: &str, date: &str) -> DatInfo {
        DatInfo {
            group: group.into(),
            system: system.into(),
            date: date.into(),
            date_created: "2020-01-01".into(),
            version: version.into(),
            dat_url: format!("url-{system}-{version}"),
            cue_url: String::new(),
            dat_name: format!("{system}.dat"),
            directory: String::new(),
            dat_roms_size: 1234,
        }
    }

    fn zst(infos: &[DatInfo]) -> Vec<u8> {
        compress_to_vec(
            &serde_json::to_vec(infos).unwrap()[..],
            CompressionLevel::Uncompressed,
        )
    }

    fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut entries = entries.to_vec();
        entries.sort_by(|a, b| rv_archive::sort::trrntzip_compare_case(a.0, b.0));
        let mut w = TorrentZipWriter::new(std::io::Cursor::new(Vec::new()), ZipStructure::ZipTrrnt)
            .unwrap();
        for (n, d) in entries {
            w.add_bytes(n, d, None).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn group(base: &str, g: &str) -> Group {
        Group {
            basepath: base.into(),
            display_name: g.into(),
            group: g.into(),
            url: String::new(),
        }
    }

    fn seed(dir: &Path, i: &DatInfo, with_dat: bool) -> LocalSeed {
        std::fs::create_dir_all(dir).unwrap();
        let json_path = dir.join(format!("{}.json", i.system));
        std::fs::write(&json_path, serde_json::to_vec(i).unwrap()).unwrap();
        if with_dat {
            std::fs::write(dir.join(&i.dat_name), b"old dat").unwrap();
        }
        LocalSeed {
            json_path,
            info: i.clone(),
            dat_exists: with_dat,
        }
    }

    struct Fx {
        st: mock::SharedState,
        srv: MockServer,
        client: Client,
    }

    fn fx() -> Fx {
        let st = mock::state(KEY);
        let srv = mock::start_service(&st);
        let client = loopback_client(&srv);
        Fx { st, srv, client }
    }

    // ---- key and groups --------------------------------------------------------------------

    #[test]
    fn the_key_check_accepts_ok_in_any_case_and_nothing_else() {
        let f = fx();
        let v = Vault::new(&f.client, KEY).unwrap();
        for ok in ["ok", "OK", "Ok", "oK"] {
            f.st.lock().unwrap().check_reply = ok.into();
            assert!(v.check_key().unwrap(), "{ok}");
        }
        f.st.lock().unwrap().check_reply = "okay".into();
        assert!(!v.check_key().unwrap());
        // The service answers a bad key with something else.
        f.st.lock().unwrap().check_reply = "ok".into();
        assert!(!Vault::new(&f.client, "wrong").unwrap().check_key().unwrap());
    }

    #[test]
    fn a_blank_key_is_refused_before_anything_is_sent() {
        let f = fx();
        for k in ["", "  \t"] {
            assert_eq!(Vault::new(&f.client, k).err(), Some(Error::KeyRequired));
        }
        assert!(f.srv.requests().is_empty());
    }

    #[test]
    fn network_and_service_errors_are_errors_not_an_invalid_key() {
        let f = fx();
        f.st.lock().unwrap().faulting.insert("CheckDVUser".into());
        let e = Vault::new(&f.client, KEY).unwrap().check_key().unwrap_err();
        assert!(matches!(e, Error::Fault(_)));
        assert!(!e.to_string().contains(KEY));
    }

    #[test]
    fn groups_come_sorted_by_base_path_then_group() {
        let f = fx();
        f.st.lock().unwrap().groups = vec![
            group("b", "z"),
            group("a", "y"),
            group("b", "a"),
            group("a", "b"),
        ];
        let g = Vault::new(&f.client, KEY).unwrap().groups().unwrap();
        let order: Vec<_> = g
            .iter()
            .map(|g| format!("{}/{}", g.basepath, g.group))
            .collect();
        assert_eq!(order, ["a/b", "a/y", "b/a", "b/z"]);
    }

    // ---- the listing -----------------------------------------------------------------------

    #[test]
    fn a_group_listing_is_zstd_then_json_with_the_exes_names() {
        let i = info("g", "nes", "1", "d1");
        let bytes = zst(std::slice::from_ref(&i));
        assert_eq!(decode_group_info(&bytes).unwrap().unwrap(), vec![i.clone()]);
        let json = serde_json::to_string(&i).unwrap();
        for name in [
            "\"group\"",
            "\"system\"",
            "\"date\"",
            "\"dateCreated\"",
            "\"version\"",
            "\"datURL\"",
            "\"cueURL\"",
            "\"datName\"",
            "\"directory\"",
            "\"datROMsSize\"",
        ] {
            assert!(json.contains(name), "{name} in {json}");
        }
        // The declaration order is the order written.
        assert!(json.find("\"group\"").unwrap() < json.find("\"system\"").unwrap());
        assert!(json.find("\"directory\"").unwrap() < json.find("\"datROMsSize\"").unwrap());
    }

    #[test]
    fn null_and_missing_fields_are_empty() {
        let bytes = compress_to_vec(
            &br#"[{"group":"g","system":null,"datROMsSize":null,"extra":1}]"#[..],
            CompressionLevel::Uncompressed,
        );
        let got = decode_group_info(&bytes).unwrap().unwrap();
        assert_eq!(got[0].group, "g");
        assert_eq!(got[0].system, "");
        assert_eq!(got[0].dat_roms_size, 0);
    }

    #[test]
    fn nothing_or_one_byte_is_no_group_and_junk_is_an_error() {
        assert_eq!(decode_group_info(&[]).unwrap(), None);
        assert_eq!(decode_group_info(&[0]).unwrap(), None);
        assert!(matches!(
            decode_group_info(b"this is not zstd"),
            Err(Error::BadResponse(_))
        ));
        let not_json = compress_to_vec(&b"not json"[..], CompressionLevel::Uncompressed);
        assert!(matches!(
            decode_group_info(&not_json),
            Err(Error::BadResponse(_))
        ));
    }

    // ---- the statuses ----------------------------------------------------------------------

    #[test]
    fn every_status_rule_of_update_master_lists() {
        let dir = tmp("status");
        let l = |v: &str, d: &str, dat: bool| {
            seed(
                &dir.join(format!("{v}{d}{dat}")),
                &info("g", "s", v, d),
                dat,
            )
        };
        let r = |v: &str, d: &str| info("g", "s", v, d);
        // Same version and date, the DAT is there: good, and not ticked.
        assert_eq!(
            status_of(Some(&l("1", "d", true)), Some(&r("1", "d")), true),
            (DatUpdate::Good, false)
        );
        // Same, the DAT file is gone: missing, ticked.
        assert_eq!(
            status_of(Some(&l("1", "d", false)), Some(&r("1", "d")), true),
            (DatUpdate::Missing, true)
        );
        // Same version, another date: replace.
        assert_eq!(
            status_of(Some(&l("1", "d", true)), Some(&r("1", "e")), true),
            (DatUpdate::Replace, true)
        );
        // Another version: update (whatever the date and the file).
        assert_eq!(
            status_of(Some(&l("1", "d", true)), Some(&r("2", "d")), true),
            (DatUpdate::Update, true)
        );
        assert_eq!(
            status_of(Some(&l("1", "d", false)), Some(&r("2", "e")), true),
            (DatUpdate::Update, true)
        );
        // Only here: discontinued when the server listed the group, unknown when it did not.
        assert_eq!(
            status_of(Some(&l("1", "d", true)), None, true),
            (DatUpdate::Discontinued, false)
        );
        assert_eq!(
            status_of(Some(&l("1", "d", true)), None, false),
            (DatUpdate::Unknown, false)
        );
        // Only there: available, not ticked.
        assert_eq!(
            status_of(None, Some(&r("1", "d")), true),
            (DatUpdate::Available, false)
        );
        assert_eq!(DatUpdate::Missing as u8, 7);
        assert_eq!(DatUpdate::Available as u8, 8);
    }

    #[test]
    fn plan_reads_seeds_and_asks_for_their_groups_only() {
        let f = fx();
        let root = tmp("plan");
        seed(&root.join("a"), &info("g1", "nes", "1", "d"), true);
        seed(&root.join("b"), &info("g2", "snes", "1", "d"), true);
        std::fs::write(root.join("a").join("broken.json"), b"{not json").unwrap();
        {
            let mut s = f.st.lock().unwrap();
            s.group_info.insert(
                "g1".into(),
                Some(zst(&[
                    info("g1", "nes", "2", "d"),
                    info("g1", "gb", "1", "d"),
                ])),
            );
            // g2 is not known to the server: nil.
        }
        let plan = Vault::new(&f.client, KEY)
            .unwrap()
            .plan(&root, &[], &NoProgress)
            .unwrap();
        let st: Vec<_> = plan
            .pairs
            .iter()
            .map(|p| (p.key.as_str(), p.status))
            .collect();
        assert_eq!(
            st,
            [
                ("g1|gb", DatUpdate::Available),
                ("g1|nes", DatUpdate::Update),
                ("g2|snes", DatUpdate::Unknown)
            ]
        );
        assert_eq!(plan.warnings.len(), 1);
        assert!(plan.warnings[0].contains("broken.json"));
        assert_eq!(f.srv.count("GetDVGroupInfoV2"), 2);
    }

    #[test]
    fn a_failed_listing_is_an_error_and_never_marks_dats_discontinued() {
        let f = fx();
        let root = tmp("planfail");
        seed(&root, &info("g1", "nes", "1", "d"), true);
        f.st.lock()
            .unwrap()
            .faulting
            .insert("GetDVGroupInfoV2".into());
        let e = Vault::new(&f.client, KEY)
            .unwrap()
            .plan(&root, &[], &NoProgress)
            .unwrap_err();
        assert!(matches!(e, Error::Fault(_)));
        // Garbage in place of a listing too.
        let srv = MockServer::start(|_| MockReply {
            status: 200,
            body: b"<x/>".to_vec(),
        });
        let c = loopback_client(&srv);
        assert!(Vault::new(&c, KEY)
            .unwrap()
            .plan(&root, &[], &NoProgress)
            .is_err());
    }

    #[test]
    fn extra_groups_are_listed_too() {
        let f = fx();
        let root = tmp("extra");
        f.st.lock()
            .unwrap()
            .group_info
            .insert("g9".into(), Some(zst(&[info("g9", "x", "1", "d")])));
        let plan = Vault::new(&f.client, KEY)
            .unwrap()
            .plan(&root, &["g9".into()], &NoProgress)
            .unwrap();
        assert_eq!(plan.pairs.len(), 1);
        assert_eq!(plan.pairs[0].status, DatUpdate::Available);
    }

    // ---- downloading -----------------------------------------------------------------------

    fn serve_dat(f: &Fx, i: &DatInfo, contents: &[u8]) {
        f.st.lock().unwrap().files.insert(
            format!("{}|{}|{}", i.group, i.system, i.dat_url),
            zip(&[(&i.dat_name, contents), ("readme.txt", b"other")]),
        );
    }

    fn fetch_all(
        f: &Fx,
        root: &Path,
        plan: &mut Plan,
        groups: &[Group],
        opts: &FetchOptions,
    ) -> FetchReport {
        Vault::new(&f.client, KEY).unwrap().fetch(
            root,
            plan,
            groups,
            opts,
            &NoProgress,
            &CancelFlag::new(),
        )
    }

    fn available(i: &DatInfo) -> Plan {
        Plan {
            pairs: vec![Pair {
                key: format!("{}|{}", i.group, i.system),
                local: None,
                remote: Some(i.clone()),
                status: DatUpdate::Available,
                checked: true,
            }],
            warnings: vec![],
        }
    }

    #[test]
    fn a_new_dat_is_written_with_its_seed_under_base_path_group_directory() {
        let f = fx();
        let root = tmp("new");
        let mut i = info("nointro", "nes", "1", "d");
        i.directory = "Nintendo\\NES".into();
        serve_dat(&f, &i, b"the dat contents");
        let mut plan = available(&i);
        let r = fetch_all(
            &f,
            &root,
            &mut plan,
            &[group("Console", "nointro")],
            &FetchOptions::default(),
        );
        assert_eq!(r.downloaded, ["nointro|nes"]);
        assert!(r.failed.is_empty());
        let dir = root
            .join("Console")
            .join("nointro")
            .join("Nintendo")
            .join("NES");
        assert_eq!(
            std::fs::read(dir.join("nes.dat")).unwrap(),
            b"the dat contents"
        );
        let on_disk: DatInfo =
            serde_json::from_slice(&std::fs::read(dir.join("nes.json")).unwrap()).unwrap();
        assert_eq!(on_disk, i);
        assert_eq!(plan.pairs[0].status, DatUpdate::Good);
        // The next plan sees it as up to date.
        let (seeds, w) = read_seeds(&root);
        assert!(w.is_empty());
        assert_eq!(seeds.len(), 1);
        assert!(seeds[0].dat_exists);
        let request = &f.srv.requests()[0].body;
        assert!(
            request.contains("<datURL>url-nes-1</datURL>")
                && request.contains("<group>nointro</group>")
        );
    }

    #[test]
    fn an_update_replaces_the_dat_and_keeps_the_old_one_when_the_name_changes() {
        let f = fx();
        let root = tmp("upd");
        let dir = root.join("x");
        let old = info("g", "s", "1", "d");
        let local = seed(&dir, &old, true);
        let mut new = info("g", "s", "2", "e");
        new.dat_name = "s v2.dat".into();
        serve_dat(&f, &new, b"new contents");
        let mut plan = Plan {
            pairs: vec![Pair {
                key: "g|s".into(),
                local: Some(local),
                remote: Some(new.clone()),
                status: DatUpdate::Update,
                checked: true,
            }],
            warnings: vec![],
        };
        std::fs::write(dir.join("s.dat.old"), b"previous .old").unwrap();
        let r = fetch_all(&f, &root, &mut plan, &[], &FetchOptions::default());
        assert_eq!(r.downloaded, ["g|s"]);
        assert_eq!(
            std::fs::read(dir.join("s v2.dat")).unwrap(),
            b"new contents"
        );
        assert!(!dir.join("s.dat").exists());
        assert_eq!(std::fs::read(dir.join("s.dat.old")).unwrap(), b"old dat");
        let seed_now: DatInfo =
            serde_json::from_slice(&std::fs::read(dir.join("s.json")).unwrap()).unwrap();
        assert_eq!(seed_now.version, "2");
    }

    #[test]
    fn a_failed_download_leaves_the_old_dat_and_seed_alone() {
        let f = fx();
        let root = tmp("keep");
        let dir = root.join("x");
        let old = info("g", "s", "1", "d");
        let local = seed(&dir, &old, true);
        let new = info("g", "s", "2", "e");
        let mk = |remote: &DatInfo| Plan {
            pairs: vec![Pair {
                key: "g|s".into(),
                local: Some(local.clone()),
                remote: Some(remote.clone()),
                status: DatUpdate::Update,
                checked: true,
            }],
            warnings: vec![],
        };
        // 1. The service has no such DAT (nil).
        let mut plan = mk(&new);
        let r = fetch_all(&f, &root, &mut plan, &[], &FetchOptions::default());
        assert_eq!(r.failed.len(), 1);
        // 2. A zip without the entry.
        f.st.lock()
            .unwrap()
            .files
            .insert(format!("g|s|{}", new.dat_url), zip(&[("other.dat", b"x")]));
        let mut plan = mk(&new);
        let r = fetch_all(&f, &root, &mut plan, &[], &FetchOptions::default());
        assert!(r.failed[0].1.contains("no entry"), "{:?}", r.failed);
        // 3. Not a zip at all.
        f.st.lock()
            .unwrap()
            .files
            .insert(format!("g|s|{}", new.dat_url), b"junk".to_vec());
        let mut plan = mk(&new);
        assert_eq!(
            fetch_all(&f, &root, &mut plan, &[], &FetchOptions::default())
                .failed
                .len(),
            1
        );
        // 4. The service faults.
        f.st.lock().unwrap().faulting.insert("GetDVDat".into());
        let mut plan = mk(&new);
        assert_eq!(
            fetch_all(&f, &root, &mut plan, &[], &FetchOptions::default())
                .failed
                .len(),
            1
        );
        assert_eq!(std::fs::read(dir.join("s.dat")).unwrap(), b"old dat");
        let seed_now: DatInfo =
            serde_json::from_slice(&std::fs::read(dir.join("s.json")).unwrap()).unwrap();
        assert_eq!(seed_now.version, "1");
    }

    #[test]
    fn a_new_dat_that_fails_keeps_its_seed_as_missing_for_next_time() {
        let f = fx();
        let root = tmp("seedfirst");
        let i = info("g", "s", "1", "d");
        let mut plan = available(&i);
        let r = fetch_all(
            &f,
            &root,
            &mut plan,
            &[group("b", "g")],
            &FetchOptions::default(),
        );
        assert_eq!(r.failed.len(), 1);
        let (seeds, _) = read_seeds(&root);
        assert_eq!(seeds.len(), 1);
        assert!(!seeds[0].dat_exists);
        assert_eq!(plan.pairs[0].status, DatUpdate::Missing);
    }

    #[test]
    fn cues_are_fetched_into_the_cue_directory_and_the_old_one_can_go() {
        let f = fx();
        let root = tmp("cue");
        let cues = tmp("cue-dir");
        std::fs::write(cues.join("old.cue"), b"old cue").unwrap();
        let dir = root.join("x");
        let mut old = info("g", "s", "1", "d");
        old.cue_url = "old.cue".into();
        let local = seed(&dir, &old, true);
        let mut new = info("g", "s", "2", "d");
        new.cue_url = "new.cue".into();
        serve_dat(&f, &new, b"dat");
        f.st.lock()
            .unwrap()
            .files
            .insert("g|s|new.cue".into(), b"cue bytes".to_vec());
        let mut plan = Plan {
            pairs: vec![Pair {
                key: "g|s".into(),
                local: Some(local),
                remote: Some(new.clone()),
                status: DatUpdate::Update,
                checked: true,
            }],
            warnings: vec![],
        };
        let opts = FetchOptions {
            cues_dir: Some(cues.clone()),
            delete_old_cue_files: true,
            ..FetchOptions::default()
        };
        let r = fetch_all(&f, &root, &mut plan, &[], &opts);
        assert_eq!(r.downloaded.len(), 1, "{:?}", r.failed);
        assert_eq!(std::fs::read(cues.join("new.cue")).unwrap(), b"cue bytes");
        assert!(!cues.join("old.cue").exists());
        // Without a cue directory no cue is requested.
        let before = f.srv.count("GetDVCue");
        let mut plan = Plan {
            pairs: vec![Pair {
                key: "g|s".into(),
                local: plan.pairs[0].local.clone(),
                remote: Some(info("g", "s", "3", "d")),
                status: DatUpdate::Update,
                checked: true,
            }],
            warnings: vec![],
        };
        serve_dat(&f, &info("g", "s", "3", "d"), b"dat3");
        fetch_all(&f, &root, &mut plan, &[], &FetchOptions::default());
        assert_eq!(f.srv.count("GetDVCue"), before);
    }

    #[test]
    fn a_missing_cue_is_noticed_unlike_the_exe() {
        let f = fx();
        let root = tmp("nocue");
        let cues = tmp("nocue-dir");
        let mut i = info("g", "s", "1", "d");
        i.cue_url = "gone.cue".into();
        serve_dat(&f, &i, b"dat");
        let mut plan = available(&i);
        let opts = FetchOptions {
            cues_dir: Some(cues),
            ..FetchOptions::default()
        };
        let r = fetch_all(&f, &root, &mut plan, &[group("b", "g")], &opts);
        assert_eq!(r.failed.len(), 1);
        assert!(r.failed[0].1.contains("cue"), "{:?}", r.failed);
    }

    #[test]
    fn names_that_leave_the_dat_root_write_nothing() {
        let f = fx();
        let base = tmp("evil");
        let root = base.join("DatRoot");
        std::fs::create_dir_all(&root).unwrap();
        let cues = base.join("cues");
        let cases: Vec<(&str, DatInfo, Group)> = vec![
            (
                "dir ..",
                DatInfo {
                    directory: "../../escape".into(),
                    ..info("g", "s", "1", "d")
                },
                group("b", "g"),
            ),
            (
                "dir backslash ..",
                DatInfo {
                    directory: "..\\..\\escape".into(),
                    ..info("g", "s", "1", "d")
                },
                group("b", "g"),
            ),
            (
                "dir absolute",
                DatInfo {
                    directory: "/tmp/escape".into(),
                    ..info("g", "s", "1", "d")
                },
                group("b", "g"),
            ),
            (
                "system",
                DatInfo {
                    system: "../s".into(),
                    ..info("g", "s", "1", "d")
                },
                group("b", "g"),
            ),
            (
                "datName",
                DatInfo {
                    dat_name: "../x.dat".into(),
                    ..info("g", "s", "1", "d")
                },
                group("b", "g"),
            ),
            (
                "datName dotdot",
                DatInfo {
                    dat_name: "..".into(),
                    ..info("g", "s", "1", "d")
                },
                group("b", "g"),
            ),
            (
                "cue",
                DatInfo {
                    cue_url: "../c.cue".into(),
                    ..info("g", "s", "1", "d")
                },
                group("b", "g"),
            ),
            ("basepath", info("g", "s", "1", "d"), group("../../up", "g")),
            (
                "group",
                DatInfo {
                    group: "g/../../up".into(),
                    ..info("g", "s", "1", "d")
                },
                group("b", "g/../../up"),
            ),
        ];
        for (label, i, g) in cases {
            serve_dat(&f, &i, b"dat");
            f.st.lock().unwrap().files.insert(
                format!("{}|{}|{}", i.group, i.system, i.cue_url),
                b"cue".to_vec(),
            );
            let mut plan = available(&i);
            let opts = FetchOptions {
                cues_dir: Some(cues.clone()),
                ..FetchOptions::default()
            };
            let r = fetch_all(&f, &root, &mut plan, &[g], &opts);
            assert_eq!(r.failed.len(), 1, "{label}: {r:?}");
            assert!(
                r.failed[0].1.contains("outside the DAT root"),
                "{label}: {:?}",
                r.failed
            );
        }
        // Nothing appeared next to the DAT root or in the cue directory.
        let names: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["DatRoot"], "{names:?}");
        assert!(!cues.exists());
        assert!(!Path::new("/tmp/escape").exists());
    }

    #[test]
    fn discontinued_dats_go_only_when_asked() {
        let f = fx();
        let root = tmp("disc");
        let dir = root.join("x");
        let local = seed(&dir, &info("g", "s", "1", "d"), true);
        let mk = || Plan {
            pairs: vec![Pair {
                key: "g|s".into(),
                local: Some(local.clone()),
                remote: None,
                status: DatUpdate::Discontinued,
                checked: false,
            }],
            warnings: vec![],
        };
        let mut plan = mk();
        let r = fetch_all(&f, &root, &mut plan, &[], &FetchOptions::default());
        assert_eq!(r, FetchReport::default());
        assert!(dir.join("s.dat").exists() && dir.join("s.json").exists());
        let mut plan = mk();
        let r = fetch_all(
            &f,
            &root,
            &mut plan,
            &[],
            &FetchOptions {
                remove_discontinued: true,
                ..FetchOptions::default()
            },
        );
        assert_eq!(r.removed, ["g|s"]);
        assert!(!dir.join("s.dat").exists() && !dir.join("s.json").exists());
        assert_eq!(std::fs::read(dir.join("s.dat.old")).unwrap(), b"old dat");
    }

    #[test]
    fn up_to_date_and_unknown_dats_are_left_alone_and_cancel_stops() {
        let f = fx();
        let root = tmp("idle");
        let i = info("g", "s", "1", "d");
        let local = seed(&root, &i, true);
        let mut plan = Plan {
            pairs: vec![
                Pair {
                    key: "g|s".into(),
                    local: Some(local),
                    remote: Some(i.clone()),
                    status: DatUpdate::Good,
                    checked: false,
                },
                Pair {
                    key: "u|x".into(),
                    local: None,
                    remote: None,
                    status: DatUpdate::Unknown,
                    checked: true,
                },
            ],
            warnings: vec![],
        };
        assert_eq!(
            fetch_all(&f, &root, &mut plan, &[], &FetchOptions::default()),
            FetchReport::default()
        );
        assert!(f.srv.requests().is_empty());
        // Cancelled before the first DAT: nothing is asked.
        let cancel = CancelFlag::new();
        cancel.cancel();
        let mut plan = available(&i);
        let r = Vault::new(&f.client, KEY).unwrap().fetch(
            &root,
            &mut plan,
            &[group("b", "g")],
            &FetchOptions::default(),
            &NoProgress,
            &cancel,
        );
        assert!(r.cancelled);
        assert!(f.srv.requests().is_empty());
    }

    #[test]
    fn symlinks_under_the_dat_root_are_not_followed() {
        let base = tmp("link");
        let root = base.join("DatRoot");
        std::fs::create_dir_all(&root).unwrap();
        let outside = base.join("outside");
        seed(&outside, &info("g", "s", "1", "d"), true);
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        assert!(read_seeds(&root).0.is_empty());
    }

    #[test]
    fn a_fault_that_echoes_the_key_does_not_reach_the_report() {
        let f = fx();
        f.st.lock().unwrap().faulting.insert("GetDVDat".into());
        let root = tmp("echo");
        let i = info("g", "s", "1", "d");
        let mut plan = available(&i);
        let r = fetch_all(
            &f,
            &root,
            &mut plan,
            &[group("b", "g")],
            &FetchOptions::default(),
        );
        assert!(!format!("{r:?}").contains(KEY));
        let _ = fault("unused");
    }
}
