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
