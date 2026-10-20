//! "Found" reports: telling the service that a file on the list of missing files has been found
//! (`MIA::SendMIAFound`, `threadworker`, `online-protocol.md` section 5.3).
//!
//! ROMVault 3.8.0 sends these by default, without a DatVault key (as `"Anon"`), from a thread that
//! never ends. Here:
//!
//! * the setting is **off** until the user turns it on, and the first send asks once
//!   ([`gate`]); a "no" turns it off again;
//! * nothing is sent without a DatVault key: there is no `"Anon"` passcode;
//! * a batch is at most 5000 files, as in the exe, and the first batch that fails stops the run
//!   (the exe re-sent it forever).
//!
//! What is sent: the size and the CRC32, SHA-1 and MD5 of the file, the name of the DAT, the name
//! of the set and the name of the file ([`CONSENT_TEXT`] says so to the user).

use rv_core::{HeaderFileType, RvFile};

use crate::client::{Client, MiaReporting};
use crate::config::OnlineConfig;
use crate::error::Error;

/// The most files in one `uploadAutoMIAFoundList` call (`threadworker @00bf`).
pub const BATCH: usize = 5000;

/// What the user is asked before the first send.
pub const CONSENT_TEXT: &str = "Reporting found MIA files sends, for each file, its size and its CRC32, SHA-1 and MD5, the name of its DAT, its set and its file name, to the RomVault service together with your DatVault key (unless you chose to be anonymous). Send reports?";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// The setting is off: send nothing, ask nothing.
    Off,
    /// The setting is on but there is no DatVault key: nothing is sent.
    NoKey,
    /// The user was asked and said no; the setting has been turned off.
    Declined,
    /// Send.
    Go,
}

/// Decide whether a report may be sent now. `ask` is called at most once ever: when the setting
/// is on, a key is set and the user has not confirmed yet. The caller saves `cfg` afterwards.
pub fn gate(cfg: &mut OnlineConfig, ask: &mut dyn FnMut(&str) -> bool) -> Gate {
    if !cfg.mia_report_found {
        return Gate::Off;
    }
    if !cfg.has_key() {
        return Gate::NoKey;
    }
    if cfg.mia_report_confirmed {
        return Gate::Go;
    }
    if ask(CONSENT_TEXT) {
        cfg.mia_report_confirmed = true;
        Gate::Go
    } else {
        cfg.mia_report_found = false;
        Gate::Declined
    }
}

/// `anon` of `uploadAutoMIAFoundList` for a user with a key: 1 when anonymous, else 0. (2 and 3
/// are the exe's codes for a report without a key, which is never sent.)
pub fn anon_code(cfg: &OnlineConfig) -> i32 {
    i32::from(cfg.mia_report_anon)
}

fn some<const N: usize>(v: &Option<Vec<u8>>) -> Option<Vec<u8>> {
    v.as_ref().filter(|b| b.len() == N).cloned()
}

/// The report records for a file: one for the file as it is, and a second for the
/// header-stripped ("alt") hashes when the file has a copier header and any alt hash
/// (`SendMIAFound @005d..@0197`).
pub fn records_for(file: &RvFile, dat_name: &str, set_name: &str) -> Vec<MiaReporting> {
    let rec = |size, crc, sha1, md5| MiaReporting {
        crc,
        dat_name: dat_name.to_owned(),
        file_name: file.name.clone(),
        md5,
        set_name: set_name.to_owned(),
        sha1,
        sha256: None,
        size,
    };
    let mut out = vec![rec(
        file.size,
        some::<4>(&file.crc),
        some::<20>(&file.sha1),
        some::<16>(&file.md5),
    )];
    let has_alt = file.alt_size.is_some()
        || file.alt_crc.is_some()
        || file.alt_sha1.is_some()
        || file.alt_md5.is_some();
    if file.header_type() != HeaderFileType::NOTHING && has_alt {
        out.push(rec(
            file.alt_size,
            some::<4>(&file.alt_crc),
            some::<20>(&file.alt_sha1),
            some::<16>(&file.alt_md5),
        ));
    }
    out
}

/// What [`send`] did.
#[derive(Debug, PartialEq, Eq)]
pub struct Sent {
    pub sent: usize,
    pub error: Option<Error>,
}

/// Send `records` in batches of [`BATCH`] after the gate said [`Gate::Go`]. Without a key
/// nothing is sent and the error is [`Error::KeyRequired`].
pub fn send(client: &Client, cfg: &OnlineConfig, records: &[MiaReporting]) -> Sent {
    if !cfg.has_key() {
        return Sent {
            sent: 0,
            error: Some(Error::KeyRequired),
        };
    }
    let mut sent = 0;
    for chunk in records.chunks(BATCH) {
        if let Err(e) = client.upload_auto_mia_found_list(&cfg.dat_vault_key, anon_code(cfg), chunk)
        {
            return Sent {
                sent,
                error: Some(e),
            };
        }
        sent += chunk.len();
    }
    Sent { sent, error: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{self, loopback_client};
    use rv_core::FileType;

    const KEY: &str = "KEY-1234-secret";

    fn cfg(on: bool) -> OnlineConfig {
        OnlineConfig {
            dat_vault_key: KEY.into(),
            mia_report_found: on,
            ..OnlineConfig::default()
        }
    }

    #[test]
    fn off_by_default_and_it_asks_nothing() {
        let mut c = OnlineConfig {
            dat_vault_key: KEY.into(),
            ..OnlineConfig::default()
        };
        let mut asked = 0;
        assert_eq!(
            gate(&mut c, &mut |_| {
                asked += 1;
                true
            }),
            Gate::Off
        );
        assert_eq!(asked, 0);
        assert!(!OnlineConfig::default().mia_report_found);
    }

    #[test]
    fn it_asks_once_and_remembers_yes() {
        let mut c = cfg(true);
        let mut asked = 0;
        let mut yes = |t: &str| {
            asked += 1;
            assert!(t.contains("CRC32") && t.contains("file name"));
            true
        };
        assert_eq!(gate(&mut c, &mut yes), Gate::Go);
        assert!(c.mia_report_confirmed);
        assert_eq!(gate(&mut c, &mut yes), Gate::Go);
        assert_eq!(asked, 1);
    }

    #[test]
    fn no_turns_the_setting_off() {
        let mut c = cfg(true);
        assert_eq!(gate(&mut c, &mut |_| false), Gate::Declined);
        assert!(!c.mia_report_found);
        assert!(!c.mia_report_confirmed);
        assert_eq!(gate(&mut c, &mut |_| panic!("asked again")), Gate::Off);
    }

    #[test]
    fn no_key_no_question_and_no_send() {
        let mut c = cfg(true);
        c.dat_vault_key = "  ".into();
        assert_eq!(
            gate(&mut c, &mut |_| panic!("asked without a key")),
            Gate::NoKey
        );
        let st = mock::state(KEY);
        let srv = mock::start_service(&st);
        let r = send(&loopback_client(&srv), &c, &[MiaReporting::default()]);
        assert_eq!(
            r,
            Sent {
                sent: 0,
                error: Some(Error::KeyRequired)
            }
        );
        assert!(
            srv.requests().is_empty(),
            "nothing leaves without a key, and never as Anon"
        );
    }

    fn many(n: usize) -> Vec<MiaReporting> {
        (0..n)
            .map(|i| MiaReporting {
                dat_name: "dat".into(),
                file_name: format!("f{i}"),
                set_name: "set".into(),
                size: Some(i as u64),
                ..MiaReporting::default()
            })
            .collect()
    }

    #[test]
    fn batches_are_at_most_5000() {
        let st = mock::state(KEY);
        let srv = mock::start_service(&st);
        let r = send(&loopback_client(&srv), &cfg(true), &many(5001));
        assert_eq!(
            r,
            Sent {
                sent: 5001,
                error: None
            }
        );
        let s = st.lock().unwrap();
        let sizes: Vec<usize> = s.uploads.iter().map(|u| u.2.len()).collect();
        assert_eq!(sizes, [5000, 1]);
        assert!(s.uploads.iter().all(|u| u.0 == KEY && u.1 == 0));
    }

    #[test]
    fn the_anonymous_setting_changes_the_code() {
        let st = mock::state(KEY);
        let srv = mock::start_service(&st);
        let mut c = cfg(true);
        c.mia_report_anon = true;
        send(&loopback_client(&srv), &c, &many(1));
        assert_eq!(st.lock().unwrap().uploads[0].1, 1);
    }

    #[test]
    fn a_failed_batch_stops_the_run_and_the_error_has_no_key() {
        let st = mock::state(KEY);
        st.lock()
            .unwrap()
            .faulting
            .insert("uploadAutoMIAFoundList".into());
        let srv = mock::start_service(&st);
        let r = send(&loopback_client(&srv), &cfg(true), &many(5001));
        assert_eq!(r.sent, 0);
        let text = format!("{:?} {}", r.error, r.error.as_ref().unwrap());
        assert!(!text.contains(KEY), "{text}");
        assert_eq!(
            srv.count("uploadAutoMIAFoundList"),
            1,
            "it did not go on to the next batch"
        );
    }

    #[test]
    fn records_for_a_file_with_a_header() {
        let mut f = RvFile::new(FileType::FILE, "game.nes");
        f.size = Some(40976);
        f.crc = Some(vec![1, 2, 3, 4]);
        f.sha1 = Some(vec![5; 20]);
        f.md5 = Some(vec![6; 16]);
        assert_eq!(records_for(&f, "d", "s").len(), 1);
        f.alt_size = Some(40960);
        f.alt_crc = Some(vec![9, 9, 9, 9]);
        assert_eq!(
            records_for(&f, "d", "s").len(),
            1,
            "no header type, no second record"
        );
        f.set_header_type_raw(HeaderFileType::NES);
        let r = records_for(&f, "d", "s");
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].size, Some(40976));
        assert_eq!(r[1].size, Some(40960));
        assert_eq!(r[1].crc, Some(vec![9, 9, 9, 9]));
        assert_eq!(r[1].sha1, None);
        assert_eq!(r[0].dat_name, "d");
        assert_eq!(r[0].set_name, "s");
        assert_eq!(r[0].file_name, "game.nes");
        assert!(r.iter().all(|x| x.sha256.is_none()));
    }

    #[test]
    fn hashes_of_the_wrong_length_are_not_sent() {
        let mut f = RvFile::new(FileType::FILE, "x");
        f.crc = Some(vec![1, 2, 3]);
        assert_eq!(records_for(&f, "d", "s")[0].crc, None);
    }
}
