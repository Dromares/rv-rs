//! A stand-in for the RomVault service on `127.0.0.1`, for the tests of this crate and of
//! `rv-cli`. Nothing here reaches beyond the loopback interface: the server binds
//! `127.0.0.1:0`, and [`loopback_client`] refuses to send anywhere else.
//!
//! The service it emulates is what `online-protocol.md` says the client expects; where the spec
//! says the real behaviour is unknown (the reply for a bad key, say) the choice made here is
//! the simplest one and is written next to it.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::b64;
use crate::client::{Client, Group, MiaReporting};
use crate::endpoint::{host_of, Endpoint};
use crate::error::{Error, Result};
use crate::transport::{HttpReply, HttpRequest, Transport, UreqTransport};
use crate::xml::{self, Node};

#[derive(Clone, Debug)]
pub struct MockRequest {
    pub path: String,
    /// The `SOAPAction` header with its quotes removed.
    pub soap_action: String,
    pub body: String,
}

pub struct MockReply {
    pub status: u16,
    pub body: Vec<u8>,
}

pub struct MockServer {
    addr: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<MockRequest>>>,
    thread: Option<JoinHandle<()>>,
}

impl MockServer {
    /// Listen on a free loopback port and answer each request with `handler`.
    pub fn start(
        handler: impl Fn(&MockRequest) -> MockReply + Send + Sync + 'static,
    ) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1");
        let addr = listener.local_addr().expect("local addr");
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler = Arc::new(handler);
        let (s2, r2) = (stop.clone(), requests.clone());
        let thread = std::thread::spawn(move || {
            for conn in listener.incoming() {
                if s2.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(conn) = conn else { continue };
                let (h, r) = (handler.clone(), r2.clone());
                std::thread::spawn(move || serve(conn, &*h, &r));
            }
        });
        MockServer {
            addr,
            stop,
            requests,
            thread: Some(thread),
        }
    }

    /// The service address, `http://127.0.0.1:<port>/RVService.svc`.
    pub fn url(&self) -> String {
        format!("http://{}/RVService.svc", self.addr)
    }

    pub fn requests(&self) -> Vec<MockRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// How many requests named this operation (by `SOAPAction`).
    pub fn count(&self, op: &str) -> usize {
        let want = format!("http://tempuri.org/IRVService/{op}");
        self.requests()
            .iter()
            .filter(|r| r.soap_action == want)
            .count()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn serve(
    mut conn: TcpStream,
    handler: &(dyn Fn(&MockRequest) -> MockReply + Send + Sync),
    log: &Mutex<Vec<MockRequest>>,
) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let path = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .unwrap_or("/")
        .to_owned();
    let mut length = 0usize;
    let mut action = String::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => length = v.trim().parse().unwrap_or(0),
                "soapaction" => action = v.trim().trim_matches('"').to_owned(),
                _ => {}
            }
        }
    }
    while buf.len() < head_end + length {
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let req = MockRequest {
        path,
        soap_action: action,
        body: String::from_utf8_lossy(&buf[head_end..head_end + length]).into_owned(),
    };
    log.lock().unwrap().push(req.clone());
    let reply = handler(&req);
    let head = format!(
        "HTTP/1.1 {} X\r\nContent-Type: text/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.status,
        reply.body.len()
    );
    let _ = conn.write_all(head.as_bytes());
    let _ = conn.write_all(&reply.body);
    let _ = conn.flush();
}

/// A transport that sends only to `127.0.0.1`. The tests build every client with it.
pub struct LoopbackOnly(pub UreqTransport);

impl Transport for LoopbackOnly {
    fn post(&self, req: &HttpRequest<'_>) -> Result<HttpReply> {
        if host_of(req.url) != "127.0.0.1" {
            return Err(Error::Network(format!(
                "test guard: refusing to send to {}",
                host_of(req.url)
            )));
        }
        self.0.post(req)
    }
}

/// A client for `server` that cannot talk to anything else.
pub fn loopback_client(server: &MockServer) -> Client {
    let ep = Endpoint::new(&server.url(), true).expect("mock url");
    Client::with_transport(ep, Box::new(LoopbackOnly(UreqTransport)))
}

// ---- the emulated service ----------------------------------------------------------------------

/// What the emulated service holds. Tests set what they need before `start` and look at
/// `uploads` afterwards.
#[derive(Default)]
pub struct ServiceState {
    /// The only key the service accepts.
    pub valid_key: String,
    /// What `CheckDVUser` says for a valid key (`"ok"` in any case).
    pub check_reply: String,
    pub groups: Vec<Group>,
    /// `GetDVGroupInfoV2` by group: the (compressed) bytes, `None` for `nil`.
    pub group_info: BTreeMap<String, Option<Vec<u8>>>,
    /// `GetDVDat` / `GetDVCue` by `"group|system|url"`.
    pub files: BTreeMap<String, Vec<u8>>,
    /// `GetAutoMIAVersion`; `None` makes the operation fault.
    pub mia_version: Option<i64>,
    /// `GetAutoMIA`; `None` makes the operation fault.
    pub mia_blob: Option<Vec<u8>>,
    /// Operations that fault with a message that contains the key the client sent.
    pub faulting: BTreeSet<String>,
    pub uploads: Vec<(String, i32, Vec<MiaReporting>)>,
}

pub type SharedState = Arc<Mutex<ServiceState>>;

pub fn state(valid_key: &str) -> SharedState {
    Arc::new(Mutex::new(ServiceState {
        valid_key: valid_key.to_owned(),
        check_reply: "ok".to_owned(),
        ..ServiceState::default()
    }))
}

const NS: &str = "http://tempuri.org/";

fn soap(op: &str, inner: &str) -> MockReply {
    MockReply {
        status: 200,
        body: format!(
            "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><{op}Response xmlns=\"{NS}\">{inner}</{op}Response></s:Body></s:Envelope>"
        )
        .into_bytes(),
    }
}

fn result_text(op: &str, text: &str) -> MockReply {
    soap(op, &format!("<{op}Result>{text}</{op}Result>"))
}

fn result_bytes(op: &str, b: &Option<Vec<u8>>) -> MockReply {
    match b {
        Some(b) => result_text(op, &b64::encode(b)),
        None => soap(
            op,
            &format!("<{op}Result i:nil=\"true\" xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\"/>"),
        ),
    }
}

pub fn fault(message: &str) -> MockReply {
    MockReply {
        status: 500,
        body: format!(
            "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><s:Fault><faultcode>s:Server</faultcode><faultstring xml:lang=\"en-US\">{}</faultstring></s:Fault></s:Body></s:Envelope>",
            xml::escape(message).unwrap_or_default()
        )
        .into_bytes(),
    }
}

fn param(op: &Node, name: &str) -> String {
    op.child(name).map(|n| n.text.clone()).unwrap_or_default()
}

fn opt_bytes(n: &Node, name: &str) -> Option<Vec<u8>> {
    n.child(name)
        .filter(|c| !c.nil)
        .and_then(|c| b64::decode(&c.text))
}

/// Answer one request from `state`.
pub fn answer(state: &SharedState, req: &MockRequest) -> MockReply {
    let Ok(doc) = xml::parse(req.body.as_bytes()) else {
        return MockReply {
            status: 400,
            body: b"bad request".to_vec(),
        };
    };
    let Some(op) = doc.find("Body").and_then(|b| b.children.first()) else {
        return MockReply {
            status: 400,
            body: b"no operation".to_vec(),
        };
    };
    let name = op.name.as_str();
    let mut st = state.lock().unwrap();
    let key = if op.child("userkey").is_some() {
        param(op, "userkey")
    } else {
        param(op, "passcode")
    };
    if st.faulting.contains(name) {
        // Real servers may echo what they were sent; the client must not pass the key on.
        return fault(&format!("could not process {name} for {key}"));
    }
    let ok = key == st.valid_key;
    match name {
        "CheckDVUser" => result_text(name, if ok { &st.check_reply } else { "invalid" }),
        "ListOfGroupsV2" => {
            if !ok {
                return soap(name, &format!("<{name}Result i:nil=\"true\" xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\"/>"));
            }
            let mut x = format!("<{name}Result xmlns:a=\"http://schemas.datacontract.org/2004/07/RVServices\" xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\">");
            for g in &st.groups {
                x.push_str(&format!(
                    "<a:groups><a:basepath>{}</a:basepath><a:displayName>{}</a:displayName><a:group>{}</a:group><a:url>{}</a:url></a:groups>",
                    xml::escape(&g.basepath).unwrap_or_default(),
                    xml::escape(&g.display_name).unwrap_or_default(),
                    xml::escape(&g.group).unwrap_or_default(),
                    xml::escape(&g.url).unwrap_or_default()
                ));
            }
            x.push_str(&format!("</{name}Result>"));
            soap(name, &x)
        }
        "GetDVGroupInfoV2" => {
            let info = if ok {
                st.group_info.get(&param(op, "group")).cloned().flatten()
            } else {
                None
            };
            result_bytes(name, &info)
        }
        "GetDVDat" | "GetDVCue" => {
            let k = format!(
                "{}|{}|{}",
                param(op, "group"),
                param(op, "system"),
                param(op, "datURL")
            );
            let f = if ok { st.files.get(&k).cloned() } else { None };
            result_bytes(name, &f)
        }
        "GetAutoMIAVersion" => match (ok, st.mia_version) {
            (true, Some(v)) => result_text(name, &v.to_string()),
            (true, None) => fault("no MIA version"),
            // The spec does not say what an invalid key gets; the client maps -1 and faults alike.
            (false, _) => result_text(name, "-1"),
        },
        "GetAutoMIA" => match (ok, st.mia_blob.clone()) {
            (true, Some(b)) => result_bytes(name, &Some(b)),
            _ => fault("no MIA list"),
        },
        "uploadAutoMIAFoundList" => {
            let anon = param(op, "anon").parse().unwrap_or(-1);
            let files = op
                .child("files")
                .map(|f| {
                    f.children
                        .iter()
                        .map(|m| MiaReporting {
                            crc: opt_bytes(m, "crc"),
                            dat_name: param(m, "datName"),
                            file_name: param(m, "fileName"),
                            md5: opt_bytes(m, "md5"),
                            set_name: param(m, "setName"),
                            sha1: opt_bytes(m, "sha1"),
                            sha256: opt_bytes(m, "sha256"),
                            size: m
                                .child("size")
                                .filter(|c| !c.nil)
                                .and_then(|c| c.text.parse().ok()),
                        })
                        .collect()
                })
                .unwrap_or_default();
            st.uploads.push((key, anon, files));
            soap(name, "")
        }
        "StartUpV2" => soap(name, ""),
        "LastestVersionCheck" => result_text(name, "3.8.0.0"),
        "GetUpdateLink" => result_text(name, "https://updates.invalid/rv"),
        "GetUserPermissions" => result_text(name, if ok { "28" } else { "0" }),
        other => fault(&format!("no such operation {other}")),
    }
}

/// A server that answers from `state`.
pub fn start_service(state: &SharedState) -> MockServer {
    let st = state.clone();
    MockServer::start(move |req| answer(&st, req))
}
