//! The SOAP 1.1 client for the operations ROMVault 3.8.0 calls on `IRVService`
//! (`online-protocol.md` sections 1 and 1.2).
//!
//! The envelopes are derived from WCF `basicHttpBinding` defaults; no real exchange was seen
//! (no network contact is allowed in this phase), so the first live call is also the first
//! test of the wire format. One request is one `POST` with the `SOAPAction` header and a body
//! with one element per operation, in the `http://tempuri.org/` namespace.
//!
//! The DatVault key is an argument of most operations. It is put in the request body and
//! nowhere else, and everything the server or the transport says back goes through
//! [`scrub`](crate::error::scrub) before it becomes an error.
//!
//! **Operations.** Implemented (12 of the 13 the exe calls): `StartUpV2`, `LastestVersionCheck`,
//! `GetUpdateLink`, `GetUserPermissions`, `CheckDVUser`, `GetDVGroupInfoV2`, `ListOfGroupsV2`,
//! `GetDVDat`, `GetDVCue`, `GetAutoMIA`, `GetAutoMIAVersion`, `uploadAutoMIAFoundList`.
//! Left out on purpose: `SendErrorMessageV2` (no error telemetry). The 13 operations the exe
//! never calls are not implemented either.

use crate::b64;
use crate::endpoint::Endpoint;
use crate::error::{scrub, Error, Result};
use crate::transport::{HttpRequest, Timeouts, Transport, UreqTransport};
use crate::xml::{self, Node};

pub const TEMPURI: &str = "http://tempuri.org/";
const DATACONTRACT: &str = "http://schemas.datacontract.org/2004/07/RVServices";
const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";

/// `RVServ1.groups`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Group {
    pub basepath: String,
    pub display_name: String,
    pub group: String,
    pub url: String,
}

/// `RVServ1.miaReporting`: one found file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MiaReporting {
    pub crc: Option<Vec<u8>>,
    pub dat_name: String,
    pub file_name: String,
    pub md5: Option<Vec<u8>>,
    pub set_name: String,
    pub sha1: Option<Vec<u8>>,
    /// The exe always sends `null` here.
    pub sha256: Option<Vec<u8>>,
    pub size: Option<u64>,
}

enum Value<'a> {
    Str(&'a str),
    Int(i64),
    /// Already XML.
    Raw(String),
}

pub struct Client {
    endpoint: Endpoint,
    transport: Box<dyn Transport>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("endpoint", &self.endpoint)
            .finish()
    }
}

fn arg(e: String) -> Error {
    Error::BadArgument(e)
}

impl Client {
    /// A client with the real transport.
    pub fn new(endpoint: Endpoint) -> Client {
        Client::with_transport(endpoint, Box::new(UreqTransport))
    }

    pub fn with_transport(endpoint: Endpoint, transport: Box<dyn Transport>) -> Client {
        Client {
            endpoint,
            transport,
        }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    fn envelope(op: &str, params: &[(&str, Value<'_>)]) -> Result<String> {
        let mut s = String::with_capacity(256);
        s.push_str(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body>",
        );
        s.push_str(&format!("<{op} xmlns=\"{TEMPURI}\">"));
        for (name, v) in params {
            match v {
                Value::Str(t) => s.push_str(&format!(
                    "<{name}>{}</{name}>",
                    xml::escape(t).map_err(arg)?
                )),
                Value::Int(n) => s.push_str(&format!("<{name}>{n}</{name}>")),
                Value::Raw(x) => s.push_str(x),
            }
        }
        s.push_str(&format!("</{op}></s:Body></s:Envelope>"));
        Ok(s)
    }

    /// One call. `secrets` are taken out of every error text. Returns the `<OpResponse>`.
    fn call(
        &self,
        op: &str,
        params: &[(&str, Value<'_>)],
        timeouts: Timeouts,
        secrets: &[&str],
    ) -> Result<Node> {
        let body = Self::envelope(op, params)?;
        let action = format!("{TEMPURI}IRVService/{op}");
        let reply = self
            .transport
            .post(&HttpRequest {
                url: self.endpoint.url(),
                soap_action: &action,
                body: body.as_bytes(),
                timeouts,
            })
            .map_err(|e| match e {
                Error::Network(m) => Error::Network(scrub(&m, secrets)),
                other => other,
            })?;
        let parsed = xml::parse(&reply.body);
        if let Ok(doc) = &parsed {
            if let Some(fault) = doc.find("Fault") {
                let text = fault
                    .child("faultstring")
                    .map(|n| n.text.trim().to_owned())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| "(no text)".to_owned());
                return Err(Error::Fault(scrub(&text, secrets)));
            }
        }
        if !(200..300).contains(&reply.status) {
            return Err(Error::HttpStatus(reply.status));
        }
        let doc = parsed.map_err(|m| Error::BadResponse(scrub(&m, secrets)))?;
        let name = format!("{op}Response");
        doc.find(&name)
            .cloned()
            .ok_or_else(|| Error::BadResponse(format!("no {name} in the reply")))
    }

    fn result<'a>(resp: &'a Node, op: &str) -> Option<&'a Node> {
        resp.child(&format!("{op}Result")).filter(|n| !n.nil)
    }

    fn text_result(resp: &Node, op: &str) -> Result<String> {
        Self::result(resp, op)
            .map(|n| n.text.clone())
            .ok_or_else(|| Error::BadResponse(format!("no {op}Result")))
    }

    fn int_result(resp: &Node, op: &str) -> Result<i64> {
        let t = Self::text_result(resp, op)?;
        t.trim()
            .parse::<i64>()
            .map_err(|_| Error::BadResponse(format!("{op}Result is not a number")))
    }

    /// A `byte[]` result: `None` for `nil`.
    fn bytes_result(resp: &Node, op: &str) -> Result<Option<Vec<u8>>> {
        match Self::result(resp, op) {
            None => Ok(None),
            Some(n) => b64::decode(&n.text)
                .map(Some)
                .ok_or_else(|| Error::BadResponse(format!("{op}Result is not base64"))),
        }
    }

    // ---- DatVault (operations 13 to 17) -------------------------------------------------------

    /// `CheckDVUser`: the reply text. `"ok"` in any case is a valid key (`DatVault::CheckLoginName`).
    pub fn check_dv_user(&self, key: &str) -> Result<String> {
        let r = self.call(
            "CheckDVUser",
            &[("userkey", Value::Str(key))],
            Timeouts::DATVAULT,
            &[key],
        )?;
        Self::text_result(&r, "CheckDVUser")
    }

    /// `ListOfGroupsV2`.
    pub fn list_of_groups_v2(&self, key: &str) -> Result<Vec<Group>> {
        let r = self.call(
            "ListOfGroupsV2",
            &[("passcode", Value::Str(key))],
            Timeouts::DATVAULT,
            &[key],
        )?;
        let Some(list) = Self::result(&r, "ListOfGroupsV2") else {
            return Ok(Vec::new());
        };
        let text = |n: &Node, name: &str| n.child(name).map(|c| c.text.clone()).unwrap_or_default();
        Ok(list
            .children
            .iter()
            .filter(|n| n.name == "groups")
            .map(|n| Group {
                basepath: text(n, "basepath"),
                display_name: text(n, "displayName"),
                group: text(n, "group"),
                url: text(n, "url"),
            })
            .collect())
    }

    /// `GetDVGroupInfoV2`: the Zstandard-compressed JSON of one group, `None` for `nil`.
    pub fn get_dv_group_info_v2(&self, key: &str, group: &str) -> Result<Option<Vec<u8>>> {
        let r = self.call(
            "GetDVGroupInfoV2",
            &[("passcode", Value::Str(key)), ("group", Value::Str(group))],
            Timeouts::DATVAULT,
            &[key],
        )?;
        Self::bytes_result(&r, "GetDVGroupInfoV2")
    }

    fn dv_file(
        &self,
        op: &str,
        key: &str,
        group: &str,
        system: &str,
        url: &str,
    ) -> Result<Option<Vec<u8>>> {
        let r = self.call(
            op,
            &[
                ("passcode", Value::Str(key)),
                ("group", Value::Str(group)),
                ("system", Value::Str(system)),
                ("datURL", Value::Str(url)),
            ],
            Timeouts::DATVAULT,
            &[key],
        )?;
        Self::bytes_result(&r, op)
    }

    /// `GetDVDat`: a zip with the DAT inside.
    pub fn get_dv_dat(
        &self,
        key: &str,
        group: &str,
        system: &str,
        dat_url: &str,
    ) -> Result<Option<Vec<u8>>> {
        self.dv_file("GetDVDat", key, group, system, dat_url)
    }

    /// `GetDVCue`: the raw bytes of a cue file.
    pub fn get_dv_cue(
        &self,
        key: &str,
        group: &str,
        system: &str,
        cue_url: &str,
    ) -> Result<Option<Vec<u8>>> {
        self.dv_file("GetDVCue", key, group, system, cue_url)
    }

    // ---- MIA (operations 24 to 26) ------------------------------------------------------------

    /// `GetAutoMIAVersion`.
    pub fn get_auto_mia_version(&self, key: &str) -> Result<i64> {
        let r = self.call(
            "GetAutoMIAVersion",
            &[("passcode", Value::Str(key))],
            Timeouts::MIA,
            &[key],
        )?;
        Self::int_result(&r, "GetAutoMIAVersion")
    }

    /// `GetAutoMIA`: the encrypted MIA list.
    pub fn get_auto_mia(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let r = self.call(
            "GetAutoMIA",
            &[("passcode", Value::Str(key))],
            Timeouts::MIA,
            &[key],
        )?;
        Self::bytes_result(&r, "GetAutoMIA")
    }

    /// `uploadAutoMIAFoundList`. `passcode` is the DatVault key (never `"Anon"`: see
    /// [`crate::report`]). `anon` is the exe's 0 to 3.
    pub fn upload_auto_mia_found_list(
        &self,
        passcode: &str,
        anon: i32,
        files: &[MiaReporting],
    ) -> Result<()> {
        let mut x = format!("<files xmlns:a=\"{DATACONTRACT}\" xmlns:i=\"{XSI}\">");
        for f in files {
            x.push_str("<a:miaReporting>");
            x.push_str(&bytes_member("crc", &f.crc));
            x.push_str(&str_member("datName", &f.dat_name)?);
            x.push_str(&str_member("fileName", &f.file_name)?);
            x.push_str(&bytes_member("md5", &f.md5));
            x.push_str(&str_member("setName", &f.set_name)?);
            x.push_str(&bytes_member("sha1", &f.sha1));
            x.push_str(&bytes_member("sha256", &f.sha256));
            match f.size {
                Some(n) => x.push_str(&format!("<a:size>{n}</a:size>")),
                None => x.push_str("<a:size i:nil=\"true\"/>"),
            }
            x.push_str("</a:miaReporting>");
        }
        x.push_str("</files>");
        self.call(
            "uploadAutoMIAFoundList",
            &[
                ("passcode", Value::Str(passcode)),
                ("anon", Value::Int(i64::from(anon))),
                ("files", Value::Raw(x)),
            ],
            Timeouts::MIA,
            &[passcode],
        )?;
        Ok(())
    }

    // ---- Startup and update (operations 3, 7, 8, 12): present for completeness, nothing in the
    // ---- rewrite calls them: updates come from GitHub, and there is no usage ping.

    /// `StartUpV2`, the usage ping.
    pub fn start_up_v2(&self, major: i32, minor: i32, build: i32) -> Result<()> {
        self.call(
            "StartUpV2",
            &[
                ("vMajor", Value::Int(i64::from(major))),
                ("vMinor", Value::Int(i64::from(minor))),
                ("vBuild", Value::Int(i64::from(build))),
            ],
            Timeouts::DEFAULT,
            &[],
        )?;
        Ok(())
    }

    /// `LastestVersionCheck` (sic).
    pub fn lastest_version_check(&self) -> Result<String> {
        let r = self.call("LastestVersionCheck", &[], Timeouts::DEFAULT, &[])?;
        Self::text_result(&r, "LastestVersionCheck")
    }

    /// `GetUpdateLink`.
    pub fn get_update_link(&self) -> Result<String> {
        let r = self.call("GetUpdateLink", &[], Timeouts::DEFAULT, &[])?;
        Self::text_result(&r, "GetUpdateLink")
    }

    /// `GetUserPermissions`.
    pub fn get_user_permissions(&self, key: &str) -> Result<i32> {
        let r = self.call(
            "GetUserPermissions",
            &[("userkey", Value::Str(key))],
            Timeouts::DEFAULT,
            &[key],
        )?;
        i32::try_from(Self::int_result(&r, "GetUserPermissions")?)
            .map_err(|_| Error::BadResponse("GetUserPermissionsResult is out of range".into()))
    }
}

fn str_member(name: &str, v: &str) -> Result<String> {
    Ok(format!(
        "<a:{name}>{}</a:{name}>",
        xml::escape(v).map_err(arg)?
    ))
}

fn bytes_member(name: &str, v: &Option<Vec<u8>>) -> String {
    match v {
        Some(b) => format!("<a:{name}>{}</a:{name}>", b64::encode(b)),
        None => format!("<a:{name} i:nil=\"true\"/>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{self, fault, loopback_client, MockReply, MockServer};

    const KEY: &str = "KEY-1234-secret";

    fn svc() -> (mock::SharedState, MockServer, Client) {
        let st = mock::state(KEY);
        let srv = mock::start_service(&st);
        let c = loopback_client(&srv);
        (st, srv, c)
    }

    #[test]
    fn check_dv_user_sends_the_envelope_the_spec_describes() {
        let (_st, srv, c) = svc();
        assert_eq!(c.check_dv_user(KEY).unwrap(), "ok");
        let r = &srv.requests()[0];
        assert_eq!(r.path, "/RVService.svc");
        assert_eq!(r.soap_action, "http://tempuri.org/IRVService/CheckDVUser");
        assert!(r.body.contains("<CheckDVUser xmlns=\"http://tempuri.org/\"><userkey>KEY-1234-secret</userkey></CheckDVUser>"));
        assert!(r.body.contains("http://schemas.xmlsoap.org/soap/envelope/"));
        // A wrong key is an answer, not an error.
        assert_eq!(c.check_dv_user("nope").unwrap(), "invalid");
    }

    #[test]
    fn list_of_groups_v2() {
        let (st, srv, c) = svc();
        st.lock().unwrap().groups = vec![
            Group {
                basepath: "Console".into(),
                display_name: "No-Intro & more".into(),
                group: "nointro".into(),
                url: "u".into(),
            },
            Group::default(),
        ];
        let g = c.list_of_groups_v2(KEY).unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].display_name, "No-Intro & more");
        assert_eq!(g[0].group, "nointro");
        assert!(srv.requests()[0].body.contains("<passcode>"));
        // A bad key gets nil: no groups.
        assert!(c.list_of_groups_v2("bad").unwrap().is_empty());
    }

    #[test]
    fn byte_results_and_nil() {
        let (st, srv, c) = svc();
        {
            let mut s = st.lock().unwrap();
            s.group_info.insert("g".into(), Some(vec![1, 2, 3, 250]));
            s.files.insert("g|sys|url".into(), b"zipbytes".to_vec());
            s.files.insert("g|sys|cue".into(), b"cuebytes".to_vec());
        }
        assert_eq!(
            c.get_dv_group_info_v2(KEY, "g").unwrap(),
            Some(vec![1, 2, 3, 250])
        );
        assert_eq!(c.get_dv_group_info_v2(KEY, "other").unwrap(), None);
        assert_eq!(
            c.get_dv_dat(KEY, "g", "sys", "url").unwrap().unwrap(),
            b"zipbytes"
        );
        assert_eq!(
            c.get_dv_cue(KEY, "g", "sys", "cue").unwrap().unwrap(),
            b"cuebytes"
        );
        assert_eq!(c.get_dv_dat(KEY, "g", "sys", "missing").unwrap(), None);
        let body = &srv.requests()[2].body;
        let want = "<passcode>KEY-1234-secret</passcode><group>g</group><system>sys</system><datURL>url</datURL>";
        assert!(body.contains(want), "{body}");
    }

    #[test]
    fn mia_operations() {
        let (st, _srv, c) = svc();
        {
            let mut s = st.lock().unwrap();
            s.mia_version = Some(638_000_000_000_000_000);
            s.mia_blob = Some(vec![9; 40]);
        }
        assert_eq!(
            c.get_auto_mia_version(KEY).unwrap(),
            638_000_000_000_000_000
        );
        assert_eq!(c.get_auto_mia(KEY).unwrap().unwrap(), vec![9; 40]);
        assert_eq!(c.get_auto_mia_version("bad").unwrap(), -1);
    }

    #[test]
    fn the_upload_carries_the_data_contract_shape() {
        let (st, srv, c) = svc();
        let f = MiaReporting {
            crc: Some(vec![1, 2, 3, 4]),
            dat_name: "dat & co".into(),
            file_name: "a.rom".into(),
            md5: None,
            set_name: "set".into(),
            sha1: Some(vec![7; 20]),
            sha256: None,
            size: Some(1234),
        };
        let nil_size = MiaReporting {
            size: None,
            ..f.clone()
        };
        c.upload_auto_mia_found_list(KEY, 1, &[f.clone(), nil_size.clone()])
            .unwrap();
        let body = &srv.requests()[0].body;
        assert!(body.contains("<anon>1</anon>"));
        assert!(body.contains("<a:crc>AQIDBA==</a:crc>"));
        assert!(body.contains("<a:md5 i:nil=\"true\"/>"));
        assert!(body.contains("<a:size>1234</a:size>"));
        assert!(body.contains("<a:size i:nil=\"true\"/>"));
        assert!(body.contains("dat &amp; co"));
        let s = st.lock().unwrap();
        assert_eq!(s.uploads.len(), 1);
        assert_eq!(s.uploads[0].0, KEY);
        assert_eq!(s.uploads[0].2, vec![f, nil_size]);
    }

    #[test]
    fn startup_and_update_operations() {
        let (_st, srv, c) = svc();
        c.start_up_v2(3, 8, 0).unwrap();
        assert!(srv.requests()[0]
            .body
            .contains("<vMajor>3</vMajor><vMinor>8</vMinor><vBuild>0</vBuild>"));
        assert_eq!(c.lastest_version_check().unwrap(), "3.8.0.0");
        assert_eq!(c.get_update_link().unwrap(), "https://updates.invalid/rv");
        assert_eq!(c.get_user_permissions(KEY).unwrap(), 28);
    }

    #[test]
    fn a_fault_that_echoes_the_key_is_scrubbed() {
        let (st, _srv, c) = svc();
        st.lock().unwrap().faulting.insert("CheckDVUser".into());
        let e = c.check_dv_user(KEY).unwrap_err();
        assert!(matches!(e, Error::Fault(_)));
        let text = format!("{e} {e:?}");
        assert!(!text.contains(KEY), "{text}");
        assert!(text.contains("***"));
    }

    #[test]
    fn http_errors_and_garbage_are_errors() {
        let srv = MockServer::start(|_| MockReply {
            status: 503,
            body: b"down".to_vec(),
        });
        let c = loopback_client(&srv);
        assert_eq!(c.check_dv_user(KEY).unwrap_err(), Error::HttpStatus(503));

        let srv = MockServer::start(|_| MockReply {
            status: 200,
            body: b"not xml at all <".to_vec(),
        });
        assert!(matches!(
            loopback_client(&srv).check_dv_user(KEY),
            Err(Error::BadResponse(_))
        ));

        let srv = MockServer::start(|_| fault("boom"));
        assert_eq!(
            loopback_client(&srv).get_update_link().unwrap_err(),
            Error::Fault("boom".into())
        );

        // Valid SOAP, but not the operation's reply.
        let srv = MockServer::start(|_| MockReply {
            status: 200,
            body: b"<s:Envelope xmlns:s=\"x\"><s:Body><Other/></s:Body></s:Envelope>".to_vec(),
        });
        assert!(matches!(
            loopback_client(&srv).get_update_link(),
            Err(Error::BadResponse(_))
        ));
    }

    #[test]
    fn a_dead_port_is_a_network_error() {
        // Bind and drop to learn a port nobody listens on.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let ep = Endpoint::new(&format!("http://127.0.0.1:{port}/x"), true).unwrap();
        let c = Client::with_transport(ep, Box::new(mock::LoopbackOnly(UreqTransport)));
        assert!(matches!(c.check_dv_user(KEY), Err(Error::Network(_))));
    }

    #[test]
    fn a_redirect_is_not_followed() {
        let srv = MockServer::start(|_| MockReply {
            status: 302,
            body: Vec::new(),
        });
        assert_eq!(
            loopback_client(&srv).get_update_link().unwrap_err(),
            Error::HttpStatus(302)
        );
    }

    #[test]
    fn the_loopback_guard_refuses_other_hosts() {
        let ep = Endpoint::new("https://example.invalid/RVService.svc", false).unwrap();
        let c = Client::with_transport(ep, Box::new(mock::LoopbackOnly(UreqTransport)));
        let e = c.get_update_link().unwrap_err();
        assert!(e.to_string().contains("test guard"), "{e}");
    }

    #[test]
    fn a_control_character_in_the_key_is_not_sent_altered() {
        let (_st, srv, c) = svc();
        assert!(matches!(
            c.check_dv_user("a\u{1}b"),
            Err(Error::BadArgument(_))
        ));
        assert!(srv.requests().is_empty());
    }
}
