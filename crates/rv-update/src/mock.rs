//! A stand-in for the GitHub Releases API and the release downloads, on `127.0.0.1`, for the tests
//! of this crate and of `rv-cli`. The server binds `127.0.0.1:0`; nothing here reaches beyond
//! the loopback interface.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::sums::sha256_hex;

/// One release the mock serves.
#[derive(Clone, Debug)]
pub struct MockRelease {
    pub tag: String,
    pub prerelease: bool,
    /// File name and content of each asset. `SHA256SUMS` is made by [`Mock::start`] from the
    /// others unless one is given here.
    pub files: Vec<(String, Vec<u8>)>,
}

impl MockRelease {
    pub fn new(tag: &str, files: &[(&str, &[u8])]) -> MockRelease {
        MockRelease {
            tag: tag.to_owned(),
            prerelease: tag.contains('-'),
            files: files
                .iter()
                .map(|(n, c)| ((*n).to_owned(), c.to_vec()))
                .collect(),
        }
    }

    /// Add the `SHA256SUMS` that matches the files (call it last).
    pub fn with_sums(mut self) -> MockRelease {
        let text: String = self
            .files
            .iter()
            .map(|(n, c)| format!("{}  {n}\n", sha256_hex(c)))
            .collect();
        self.files
            .push(("SHA256SUMS".to_owned(), text.into_bytes()));
        self
    }

    pub fn replace(mut self, name: &str, content: &[u8]) -> MockRelease {
        for f in &mut self.files {
            if f.0 == name {
                f.1 = content.to_vec();
            }
        }
        self
    }
}

/// What the mock serves and how it misbehaves.
#[derive(Clone, Debug, Default)]
pub struct MockState {
    /// `GET /releases/latest` answers with the newest of these that is not a pre-release;
    /// `GET /releases` lists them all, newest first. Empty: 404 and `[]`.
    pub releases: Vec<MockRelease>,
    /// Send only this many bytes of every asset download, though the length header says all.
    pub truncate_assets_to: Option<usize>,
    /// Answer 302 for downloads, to `/cdn/<tag>/<name>`.
    pub redirect_downloads: bool,
}

pub struct Mock {
    addr: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    log: Arc<Mutex<Vec<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl Mock {
    pub fn start(state: MockState) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1");
        let addr = listener.local_addr().expect("local addr");
        let stop = Arc::new(AtomicBool::new(false));
        let log = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(state);
        let (s2, l2) = (stop.clone(), log.clone());
        let thread = std::thread::spawn(move || {
            for conn in listener.incoming() {
                if s2.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(conn) = conn else { continue };
                let (st, l) = (state.clone(), l2.clone());
                std::thread::spawn(move || serve(conn, &st, addr, &l));
            }
        });
        Mock {
            addr,
            stop,
            log,
            thread: Some(thread),
        }
    }

    /// The API address to put in `api_base`.
    pub fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Every request path seen, in order.
    pub fn requests(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn release_json(base: &str, r: &MockRelease) -> String {
    let assets: Vec<String> = r
        .files
        .iter()
        .map(|(n, c)| {
            format!(
                r#"{{"name":"{n}","size":{},"browser_download_url":"{base}/download/{}/{n}"}}"#,
                c.len(),
                r.tag
            )
        })
        .collect();
    format!(
        r#"{{"tag_name":"{}","html_url":"{base}/release-page/{}","draft":false,"prerelease":{},"assets":[{}]}}"#,
        r.tag,
        r.tag,
        r.prerelease,
        assets.join(",")
    )
}

fn respond(
    conn: &mut TcpStream,
    status: u16,
    headers: &[(&str, String)],
    body: &[u8],
    send: usize,
) {
    let reason = match status {
        200 => "OK",
        302 => "Found",
        404 => "Not Found",
        _ => "Status",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = conn.write_all(head.as_bytes());
    let _ = conn.write_all(&body[..send.min(body.len())]);
    let _ = conn.flush();
}

fn serve(
    mut conn: TcpStream,
    st: &MockState,
    addr: std::net::SocketAddr,
    log: &Mutex<Vec<String>>,
) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let path = head
        .lines()
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .unwrap_or("/")
        .to_owned();
    log.lock().unwrap().push(path.clone());
    let base = format!("http://{addr}");
    let route = path.split('?').next().unwrap_or("");
    let by_tag: BTreeMap<&str, &MockRelease> =
        st.releases.iter().map(|r| (r.tag.as_str(), r)).collect();
    let json = [("Content-Type", "application/json".to_owned())];
    match route {
        "/repos/Dromares/rv-rs/releases/latest" => {
            let newest = st.releases.iter().find(|r| !r.prerelease);
            match newest {
                Some(r) => {
                    let b = release_json(&base, r);
                    respond(&mut conn, 200, &json, b.as_bytes(), usize::MAX);
                }
                None => respond(
                    &mut conn,
                    404,
                    &[],
                    b"{\"message\":\"Not Found\"}",
                    usize::MAX,
                ),
            }
        }
        "/repos/Dromares/rv-rs/releases" => {
            let all: Vec<String> = st.releases.iter().map(|r| release_json(&base, r)).collect();
            let b = format!("[{}]", all.join(","));
            respond(&mut conn, 200, &json, b.as_bytes(), usize::MAX);
        }
        p => {
            let parts: Vec<&str> = p.trim_start_matches('/').splitn(3, '/').collect();
            match parts.as_slice() {
                ["download", tag, name] if st.redirect_downloads => {
                    let to = format!("{base}/cdn/{tag}/{name}");
                    respond(&mut conn, 302, &[("Location", to)], b"", 0);
                }
                ["download" | "cdn", tag, name] => {
                    let file = by_tag
                        .get(tag)
                        .and_then(|r| r.files.iter().find(|(n, _)| n == name));
                    match file {
                        Some((n, c)) => {
                            let send = if n == "SHA256SUMS" {
                                usize::MAX
                            } else {
                                st.truncate_assets_to.unwrap_or(usize::MAX)
                            };
                            respond(&mut conn, 200, &[], c, send);
                        }
                        None => respond(&mut conn, 404, &[], b"", usize::MAX),
                    }
                }
                _ => respond(&mut conn, 404, &[], b"", usize::MAX),
            }
        }
    }
}
