//! The update settings: `~/.config/rv-rs/update.conf` (`$XDG_CONFIG_HOME/rv-rs/update.conf`), a
//! `key=value` file like `online.conf`, shared by `rv` and `rv-gtk`.
//!
//! Nothing is checked, and nothing leaves the machine, unless the user runs a check or turns
//! `check_on_startup` on.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::http;

/// Where the Releases API is, unless `api_base` says otherwise.
pub const DEFAULT_API_BASE: &str = "https://api.github.com";

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UpdateConfig {
    /// `rv-gtk` checks once when it starts. Off by default.
    pub check_on_startup: bool,
    /// Offer pre-releases (`1.0.0-rc.1`, or a GitHub release marked as pre-release). Off by
    /// default.
    pub include_prereleases: bool,
    /// The API address; `None` is [`DEFAULT_API_BASE`]. `https://`, or `http://` on the loopback
    /// (the tests point it at a mock).
    pub api_base: Option<String>,
}

impl UpdateConfig {
    /// The API address in use, checked.
    pub fn api_base(&self) -> Result<String> {
        let base = self.api_base.as_deref().unwrap_or(DEFAULT_API_BASE);
        http::check_url(base)?;
        Ok(base.trim_end_matches('/').to_owned())
    }

    pub fn to_text(&self) -> String {
        let b = |v: bool| if v { "1" } else { "0" };
        format!(
            "check_on_startup={}\ninclude_prereleases={}\napi_base={}\n",
            b(self.check_on_startup),
            b(self.include_prereleases),
            self.api_base.as_deref().unwrap_or("")
        )
    }

    /// Unknown keys and unreadable values are ignored: the default stays (so a flag that is
    /// not exactly `1`/`0` stays off).
    pub fn from_text(text: &str) -> UpdateConfig {
        let mut c = UpdateConfig::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "check_on_startup" => c.check_on_startup = v == "1",
                "include_prereleases" => c.include_prereleases = v == "1",
                "api_base" if !v.is_empty() => c.api_base = Some(v.to_owned()),
                _ => {}
            }
        }
        c
    }

    /// A missing or unreadable file gives the defaults.
    pub fn load(path: &Path) -> UpdateConfig {
        std::fs::read_to_string(path)
            .map(|t| UpdateConfig::from_text(&t))
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let io = |e: std::io::Error| Error::Io(format!("cannot write {}: {e}", path.display()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        std::fs::write(path, self.to_text()).map_err(io)
    }
}

/// `$XDG_CONFIG_HOME/rv-rs/update.conf`, or `~/.config/rv-rs/update.conf`.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("rv-rs").join("update.conf")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off() {
        let c = UpdateConfig::default();
        assert!(!c.check_on_startup && !c.include_prereleases);
        assert_eq!(c.api_base().unwrap(), DEFAULT_API_BASE);
        assert_eq!(UpdateConfig::from_text(""), c);
    }

    #[test]
    fn round_trip() {
        let c = UpdateConfig {
            check_on_startup: true,
            include_prereleases: true,
            api_base: Some("http://127.0.0.1:9/".into()),
        };
        assert_eq!(UpdateConfig::from_text(&c.to_text()), c);
        assert_eq!(c.api_base().unwrap(), "http://127.0.0.1:9");
    }

    #[test]
    fn junk_leaves_the_defaults() {
        let c =
            UpdateConfig::from_text("check_on_startup=yes\ninclude_prereleases=true\nnoise\n=\n");
        assert_eq!(c, UpdateConfig::default());
    }

    #[test]
    fn plain_http_only_on_the_loopback() {
        for bad in [
            "http://example.invalid",
            "ftp://x",
            "https://u:p@h/",
            "file:///x",
        ] {
            let c = UpdateConfig {
                api_base: Some(bad.into()),
                ..Default::default()
            };
            assert!(c.api_base().is_err(), "{bad}");
        }
        for ok in [
            "http://127.0.0.1:1",
            "http://localhost",
            "https://example.invalid",
        ] {
            let c = UpdateConfig {
                api_base: Some(ok.into()),
                ..Default::default()
            };
            assert!(c.api_base().is_ok(), "{ok}");
        }
    }
}
