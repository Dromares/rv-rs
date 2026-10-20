//! Asking the Releases API what is newest, and comparing it with the running build.

use serde::Deserialize;

use crate::config::UpdateConfig;
use crate::error::{Error, Result};
use crate::http;
use crate::version::Version;

/// The repository releases are read from.
pub const REPO: &str = "Dromares/rv-rs";

const JSON: &str = "application/vnd.github+json";
const MAX_API_REPLY: u64 = 4 * 1024 * 1024;
/// How many releases the list endpoint is asked for, when pre-releases are on.
const LIST_PAGE: usize = 30;

/// Which program is being updated: each has its own file in a release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Product {
    Rv,
    RvGtk,
}

impl Product {
    pub fn binary(self) -> &'static str {
        match self {
            Product::Rv => "rv",
            Product::RvGtk => "rv-gtk",
        }
    }

    /// The release file for `os`/`arch` (Rust's `std::env::consts` names): `rv-linux-x86_64`.
    /// Only Linux x86_64 is built (see `RELEASING.md`), so there is none for anything else.
    pub fn asset_name_for(self, os: &str, arch: &str) -> Option<String> {
        (os == "linux" && arch == "x86_64").then(|| format!("{}-{os}-{arch}", self.binary()))
    }

    /// The release file for the system this is running on.
    pub fn asset_name(self) -> Option<String> {
        self.asset_name_for(std::env::consts::OS, std::env::consts::ARCH)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// A release newer than the running build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub version: Version,
    pub tag: String,
    /// The release page, for a person to open when nothing can be installed here.
    pub page: String,
    pub assets: Vec<Asset>,
}

impl Update {
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    /// No release is published (404 or an empty list), or none that qualifies.
    NothingPublished,
    /// The newest release is not newer than the running build (`latest` can be equal or older).
    UpToDate {
        current: Version,
        latest: Version,
    },
    Available(Update),
}

#[derive(Deserialize)]
struct RawRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Deserialize)]
struct RawAsset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

struct Candidate {
    version: Version,
    raw: RawRelease,
}

impl RawRelease {
    fn candidate(self) -> Option<Candidate> {
        // The API does not list drafts to an anonymous caller; one that shows up is skipped.
        if self.draft {
            return None;
        }
        Version::parse(&self.tag_name)
            .ok()
            .map(|version| Candidate { version, raw: self })
    }
}

/// Ask for the newest release and compare it with `current`.
pub fn check(cfg: &UpdateConfig, current: &Version) -> Result<Check> {
    let base = format!("{}/repos/{REPO}/releases", cfg.api_base()?);
    let mut best: Option<Candidate> = None;
    if cfg.include_prereleases {
        let url = format!("{base}?per_page={LIST_PAGE}");
        let Some(body) = http::get_small(&url, JSON, MAX_API_REPLY)? else {
            return Ok(Check::NothingPublished);
        };
        let list: Vec<RawRelease> = serde_json::from_slice(&body)
            .map_err(|e| Error::BadReply(format!("the release list: {e}")))?;
        for c in list.into_iter().filter_map(RawRelease::candidate) {
            if best.as_ref().is_none_or(|b| c.version > b.version) {
                best = Some(c);
            }
        }
    } else {
        // `latest` is the newest release that is neither a draft nor a pre-release.
        let url = format!("{base}/latest");
        let Some(body) = http::get_small(&url, JSON, MAX_API_REPLY)? else {
            return Ok(Check::NothingPublished);
        };
        let raw: RawRelease = serde_json::from_slice(&body)
            .map_err(|e| Error::BadReply(format!("the latest release: {e}")))?;
        let tag = raw.tag_name.clone();
        let c = raw
            .candidate()
            .ok_or_else(|| Error::BadReply(format!("the tag {tag:?} is not a version")))?;
        // A pre-release that slipped through (its flag unset, or a version like 1.0.0-rc.1).
        if !c.raw.prerelease && !c.version.is_prerelease() {
            best = Some(c);
        }
    }
    let Some(best) = best else {
        return Ok(Check::NothingPublished);
    };
    if best.version <= *current {
        return Ok(Check::UpToDate {
            current: current.clone(),
            latest: best.version,
        });
    }
    Ok(Check::Available(Update {
        version: best.version,
        tag: best.raw.tag_name,
        page: best.raw.html_url,
        assets: best
            .raw
            .assets
            .into_iter()
            .map(|a| Asset {
                name: a.name,
                url: a.browser_download_url,
                size: a.size,
            })
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names() {
        assert_eq!(
            Product::Rv.asset_name_for("linux", "x86_64").as_deref(),
            Some("rv-linux-x86_64")
        );
        assert_eq!(
            Product::RvGtk.asset_name_for("linux", "x86_64").as_deref(),
            Some("rv-gtk-linux-x86_64")
        );
        assert_eq!(Product::Rv.asset_name_for("linux", "aarch64"), None);
        assert_eq!(Product::Rv.asset_name_for("windows", "x86_64"), None);
    }
}
