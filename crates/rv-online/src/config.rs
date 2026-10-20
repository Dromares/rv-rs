//! The online settings: the DatVault key and the rest. They live in the rewrite's own file,
//! `~/.config/rv-rs/online.conf` (`$XDG_CONFIG_HOME/rv-rs/online.conf`), a `key=value` file like
//! `rv-gtk.conf`, and not in ROMVault's `RomVault3cfg.xml`: the 3.8.0 exe keeps the key there in
//! clear text next to every other setting, and `rv` never writes that file.
//!
//! The file is written readable by its owner only. The key is never shown: `Debug` prints
//! whether it is set, and nothing in this crate puts it in an error.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::endpoint::Endpoint;
use crate::error::{Error, Result};

#[derive(Clone, PartialEq, Eq)]
pub struct OnlineConfig {
    /// `Settings.DATUpdateKey`.
    pub dat_vault_key: String,
    /// The service address; `None` is [`crate::DEFAULT_ENDPOINT`].
    pub endpoint: Option<String>,
    /// Allow an `http://` endpoint. Off: the key never travels in clear text.
    pub allow_plain_http: bool,
    /// The user's own `ROMVault38.exe`, the source of the MIA salt.
    pub rom_vault_exe: Option<PathBuf>,
    /// Where the MIA list is cached; `None` is `~/.cache/rv-rs/MIA.Cache`.
    pub mia_cache: Option<PathBuf>,
    /// `Settings.MIADays`.
    pub mia_days: i64,
    /// Send "found" reports for MIA files. Off until the user turns it on and confirms once.
    pub mia_report_found: bool,
    /// `Settings.MIAAnon`: the report does not name the user.
    pub mia_report_anon: bool,
    /// The user has been asked and said yes.
    pub mia_report_confirmed: bool,
    /// `Settings.DeleteOldCueFiles`.
    pub delete_old_cue_files: bool,
}

impl Default for OnlineConfig {
    fn default() -> Self {
        OnlineConfig {
            dat_vault_key: String::new(),
            endpoint: None,
            allow_plain_http: false,
            rom_vault_exe: None,
            mia_cache: None,
            mia_days: 14,
            mia_report_found: false,
            mia_report_anon: false,
            mia_report_confirmed: false,
            delete_old_cue_files: false,
        }
    }
}

impl fmt::Debug for OnlineConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OnlineConfig")
            .field(
                "dat_vault_key",
                &if self.dat_vault_key.is_empty() {
                    "<none>"
                } else {
                    "<set>"
                },
            )
            .field("endpoint", &self.endpoint)
            .field("allow_plain_http", &self.allow_plain_http)
            .field("rom_vault_exe", &self.rom_vault_exe)
            .field("mia_cache", &self.mia_cache)
            .field("mia_days", &self.mia_days)
            .field("mia_report_found", &self.mia_report_found)
            .field("mia_report_anon", &self.mia_report_anon)
            .field("mia_report_confirmed", &self.mia_report_confirmed)
            .field("delete_old_cue_files", &self.delete_old_cue_files)
            .finish()
    }
}

fn b(v: bool) -> &'static str {
    if v {
        "1"
    } else {
        "0"
    }
}

impl OnlineConfig {
    /// The address to use, checked against `allow_plain_http`.
    pub fn endpoint(&self) -> Result<Endpoint> {
        match &self.endpoint {
            Some(u) => Endpoint::new(u, self.allow_plain_http),
            None => Ok(Endpoint::default_https()),
        }
    }

    /// The MIA cache file.
    pub fn mia_cache_path(&self) -> PathBuf {
        self.mia_cache.clone().unwrap_or_else(default_mia_cache)
    }

    pub fn has_key(&self) -> bool {
        !self.dat_vault_key.trim().is_empty()
    }

    /// The text of the file. Fails if a value has a line break in it.
    pub fn to_text(&self) -> Result<String> {
        let path = |p: &Option<PathBuf>| {
            p.as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let lines = [
            ("dat_vault_key", self.dat_vault_key.clone()),
            ("endpoint", self.endpoint.clone().unwrap_or_default()),
            ("allow_plain_http", b(self.allow_plain_http).to_owned()),
            ("rom_vault_exe", path(&self.rom_vault_exe)),
            ("mia_cache", path(&self.mia_cache)),
            ("mia_days", self.mia_days.to_string()),
            ("mia_report_found", b(self.mia_report_found).to_owned()),
            ("mia_report_anon", b(self.mia_report_anon).to_owned()),
            (
                "mia_report_confirmed",
                b(self.mia_report_confirmed).to_owned(),
            ),
            (
                "delete_old_cue_files",
                b(self.delete_old_cue_files).to_owned(),
            ),
        ];
        let mut out = String::new();
        for (k, v) in lines {
            if v.contains(['\n', '\r']) {
                return Err(Error::BadArgument(format!("{k} has a line break in it")));
            }
            out.push_str(&format!("{k}={v}\n"));
        }
        Ok(out)
    }

    /// Parse a file's text; unknown keys and unreadable values leave the default.
    pub fn from_text(text: &str) -> OnlineConfig {
        let mut c = OnlineConfig::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            let flag = |d: bool| match v {
                "1" => true,
                "0" => false,
                _ => d,
            };
            let opt = |s: &str| (!s.is_empty()).then(|| s.to_owned());
            match k {
                "dat_vault_key" => c.dat_vault_key = v.to_owned(),
                "endpoint" => c.endpoint = opt(v),
                "allow_plain_http" => c.allow_plain_http = flag(false),
                "rom_vault_exe" => c.rom_vault_exe = opt(v).map(PathBuf::from),
                "mia_cache" => c.mia_cache = opt(v).map(PathBuf::from),
                "mia_days" => {
                    if let Ok(n) = v.parse::<i64>() {
                        if (1..=3650).contains(&n) {
                            c.mia_days = n;
                        }
                    }
                }
                "mia_report_found" => c.mia_report_found = flag(false),
                "mia_report_anon" => c.mia_report_anon = flag(false),
                "mia_report_confirmed" => c.mia_report_confirmed = flag(false),
                "delete_old_cue_files" => c.delete_old_cue_files = flag(false),
                _ => {}
            }
        }
        c
    }

    /// Read the file. A missing file is the defaults.
    pub fn load(path: &Path) -> Result<OnlineConfig> {
        match std::fs::read_to_string(path) {
            Ok(t) => Ok(OnlineConfig::from_text(&t)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(OnlineConfig::default()),
            Err(e) => Err(Error::Io(format!("cannot read {}: {e}", path.display()))),
        }
    }

    /// Write the file (owner read and write only), through a temporary file and a rename.
    pub fn save(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        let text = self.to_text()?;
        let io = |what: &str, e: std::io::Error| {
            Error::Io(format!("cannot {what} {}: {e}", path.display()))
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| io("create the directory of", e))?;
        }
        let tmp = path.with_extension("conf.tmp");
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let result = (|| -> std::io::Result<()> {
            let mut f = opts.open(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, path)
        })();
        result.map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            io("write", e)
        })
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(fallback))
}

/// `$XDG_CONFIG_HOME/rv-rs/online.conf`, or `~/.config/rv-rs/online.conf`.
pub fn default_path() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
        .join("rv-rs")
        .join("online.conf")
}

/// `$XDG_CACHE_HOME/rv-rs/MIA.Cache`, or `~/.cache/rv-rs/MIA.Cache`.
pub fn default_mia_cache() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
        .join("rv-rs")
        .join("MIA.Cache")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rv-online-cfg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn the_defaults_are_the_safe_ones() {
        let c = OnlineConfig::default();
        assert!(!c.allow_plain_http);
        assert!(!c.mia_report_found);
        assert!(!c.mia_report_anon);
        assert!(!c.mia_report_confirmed);
        assert!(!c.has_key());
        assert_eq!(c.mia_days, 14);
        assert_eq!(
            c.endpoint().unwrap().url(),
            crate::endpoint::DEFAULT_ENDPOINT
        );
        assert!(!c.endpoint().unwrap().is_plain_http());
    }

    #[test]
    fn a_round_trip_keeps_everything() {
        let c = OnlineConfig {
            dat_vault_key: "k e y=1".into(),
            endpoint: Some("http://127.0.0.1:9/x".into()),
            allow_plain_http: true,
            rom_vault_exe: Some("/home/u/ROMVault38.exe".into()),
            mia_cache: Some("/tmp/m".into()),
            mia_days: 30,
            mia_report_found: true,
            mia_report_anon: true,
            mia_report_confirmed: true,
            delete_old_cue_files: true,
        };
        assert_eq!(OnlineConfig::from_text(&c.to_text().unwrap()), c);
    }

    #[test]
    fn junk_leaves_the_defaults() {
        let c = OnlineConfig::from_text(
            "allow_plain_http=yes\nmia_days=0\nmia_report_found=true\nnoise\n=\nunknown=1\n",
        );
        assert_eq!(c, OnlineConfig::default());
    }

    #[test]
    fn plain_http_endpoint_needs_the_switch() {
        let mut c = OnlineConfig {
            endpoint: Some("http://127.0.0.1:9/x".into()),
            ..OnlineConfig::default()
        };
        assert!(matches!(c.endpoint(), Err(Error::PlainHttpRefused(_))));
        c.allow_plain_http = true;
        assert!(c.endpoint().unwrap().is_plain_http());
    }

    #[test]
    fn debug_does_not_show_the_key() {
        let c = OnlineConfig {
            dat_vault_key: "SUPER-SECRET-KEY".into(),
            ..OnlineConfig::default()
        };
        let t = format!("{c:?} {c:#?}");
        assert!(!t.contains("SUPER-SECRET-KEY"));
        assert!(t.contains("<set>"));
    }

    #[test]
    fn a_line_break_in_a_value_is_refused() {
        let c = OnlineConfig {
            dat_vault_key: "a\nmia_report_found=1".into(),
            ..OnlineConfig::default()
        };
        assert!(matches!(c.to_text(), Err(Error::BadArgument(_))));
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private_and_replaced_whole() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("save");
        let p = d.join("sub").join("online.conf");
        let mut c = OnlineConfig {
            dat_vault_key: "one".into(),
            ..OnlineConfig::default()
        };
        c.save(&p).unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        c.dat_vault_key = "two".into();
        c.save(&p).unwrap();
        assert_eq!(OnlineConfig::load(&p).unwrap().dat_vault_key, "two");
        assert!(!p.with_extension("conf.tmp").exists());
    }

    #[test]
    fn a_missing_file_is_the_defaults_and_a_directory_is_an_error() {
        let d = dir("load");
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(
            OnlineConfig::load(&d.join("none.conf")).unwrap(),
            OnlineConfig::default()
        );
        assert!(matches!(OnlineConfig::load(&d), Err(Error::Io(_))));
    }
}
