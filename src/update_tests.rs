//! `update::` against a local stub (tiny_http, already a dependency) — never
//! the network. Each stub answers ONE request, records what it received, and
//! gives up after a few seconds so a broken client fails instead of hanging.

use super::*;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// What the stub received: the method, the path, and every header line as
/// (lower-cased name, value).
#[derive(Debug, Default, Clone)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

/// A one-request loopback server answering `status` with `body` after
/// `delay`. The endpoint has short timeouts so no test waits long.
fn stub(status: u16, body: Vec<u8>, delay: Duration) -> (Endpoint, Arc<Mutex<Option<Seen>>>, JoinHandle<()>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap_or_else(|e| panic!("bind loopback stub: {e}"));
    let addr = server.server_addr().to_ip().expect("loopback stub has an IP address");
    let seen = Arc::new(Mutex::new(None));
    let rec = Arc::clone(&seen);
    let handle = std::thread::spawn(move || {
        let Ok(Some(req)) = server.recv_timeout(Duration::from_secs(5)) else { return };
        *rec.lock().unwrap() = Some(Seen {
            method: req.method().to_string(),
            path: req.url().to_owned(),
            headers: req
                .headers()
                .iter()
                .map(|h| (h.field.as_str().as_str().to_ascii_lowercase(), h.value.as_str().to_owned()))
                .collect(),
        });
        std::thread::sleep(delay);
        let _ = req.respond(tiny_http::Response::from_data(body).with_status_code(status));
    });
    let ep = Endpoint {
        url: format!("http://{addr}/repos/skymanbp/autoshade/releases/latest"),
        connect_timeout: Duration::from_secs(2),
        timeout: Duration::from_secs(5),
    };
    (ep, seen, handle)
}

fn release_json(tag: &str, page: &str) -> Vec<u8> {
    serde_json::json!({ "tag_name": tag, "html_url": page, "name": "ignored", "body": "notes" }).to_string().into_bytes()
}

fn fetch_once(status: u16, body: Vec<u8>) -> Result<Release, CheckError> {
    let (ep, _, h) = stub(status, body, Duration::ZERO);
    let r = fetch_latest(&ep);
    h.join().unwrap();
    r
}

fn v(s: &str) -> Version {
    Version::parse(s).unwrap_or_else(|| panic!("{s:?} should parse"))
}

#[test]
fn update_newer_same_and_older_compare_against_this_build() {
    let me = Version::current();
    let newer = Version { patch: me.patch + 1, ..me };
    let older = Version { major: 0, ..me };
    for (tag, want) in [
        (format!("v{newer}"), std::cmp::Ordering::Greater),
        (format!("v{me}"), std::cmp::Ordering::Equal),
        (format!("v{older}"), std::cmp::Ordering::Less),
    ] {
        let page = format!("https://github.com/skymanbp/autoshade/releases/tag/{tag}");
        let r = fetch_once(200, release_json(&tag, &page)).expect("a good reply reads");
        assert_eq!(r.version.cmp(&me), want, "{tag}");
        assert_eq!(r.page, page, "a page inside the repository is kept");
    }
}

#[test]
fn update_version_compare_is_numeric_field_by_field() {
    assert!(v("1.18.10") > v("1.18.9"));
    assert!(v("1.10.0") > v("1.9.99"));
    assert!(v("2.0.0") > v("1.99.99"));
    assert!(v("1.18.5") == v("v1.18.5"));
    assert!(v("10.0.0") > v("9.0.0"));
    assert_eq!(v("v1.18.10").to_string(), "1.18.10");
}

#[test]
fn update_tag_reads_with_and_without_v() {
    for tag in ["v1.18.7", "1.18.7"] {
        let r = fetch_once(200, release_json(tag, RELEASES_PAGE)).expect("plain tag reads");
        assert_eq!(r.version, Version { major: 1, minor: 18, patch: 7 }, "{tag}");
    }
}

#[test]
fn update_prerelease_or_malformed_tags_are_refused() {
    for tag in [
        "v1.18.5-rc1",
        "1.18.5-beta",
        "1.18.5+build",
        "1.18",
        "1.18.5.1",
        "v 1.18.5",
        "V1.18.5",
        "vv1.18.5",
        "1.18.x",
        "1..5",
        "",
        "+1.18.5",
        "-1.18.5",
        "1.18.5 ",
        "\u{ff11}.18.5",
        "99999999999999999999.0.0",
        "latest",
    ] {
        assert_eq!(Version::parse(tag), None, "{tag:?} must not parse");
        assert_eq!(fetch_once(200, release_json(tag, RELEASES_PAGE)), Err(CheckError::Unreadable), "{tag:?}");
    }
}

#[test]
fn update_garbage_or_incomplete_replies_are_unreadable() {
    for body in [
        b"<html>rate limited</html>".to_vec(),
        b"".to_vec(),
        b"{\"tag_name\": \"v1.2.3\"".to_vec(),
        b"{\"html_url\": \"https://github.com/skymanbp/autoshade/releases/tag/v9.9.9\"}".to_vec(),
        b"{\"tag_name\": 123}".to_vec(),
        b"[\"v1.2.3\"]".to_vec(),
        b"\"v1.2.3\"".to_vec(),
    ] {
        let shown = String::from_utf8_lossy(&body).into_owned();
        assert_eq!(fetch_once(200, body), Err(CheckError::Unreadable), "{shown:?}");
    }
}

#[test]
fn update_refusals_and_redirects_say_refused() {
    for status in [403, 404, 429, 500, 301, 302] {
        let body = release_json("v99.0.0", RELEASES_PAGE);
        assert_eq!(fetch_once(status, body), Err(CheckError::Refused), "HTTP {status}");
    }
}

#[test]
fn update_an_oversized_reply_is_cut_off_and_refused() {
    // A VALID record padded past the cap: refused, never truncated and read.
    let head = br#"{"tag_name":"v99.0.0","html_url":"https://github.com/skymanbp/autoshade/releases/latest","pad":""#;
    let tail = br#""}"#;
    let pad = |total: usize| {
        let mut b = head.to_vec();
        b.resize(total - tail.len(), b'x');
        b.extend_from_slice(tail);
        assert_eq!(b.len(), total);
        b
    };
    // The cap is 1 MiB, pinned as a number: a test written against
    // `MAX_BODY` alone would follow a raised cap and stay green.
    let cap: usize = 1 << 20;
    assert_eq!(MAX_BODY, cap as u64, "the reply cap is 1 MiB");
    assert_eq!(fetch_once(200, pad(cap + 1)), Err(CheckError::Unreadable), "one byte over the cap");
    assert_eq!(fetch_once(200, pad(cap + 4096)), Err(CheckError::Unreadable), "well over the cap");
    // At the cap exactly it still reads: the cut is at the cap, not below.
    assert_eq!(fetch_once(200, pad(cap)).map(|r| r.version), Ok(v("99.0.0")), "exactly the cap");
}

#[test]
fn update_a_slow_server_hits_the_timeout() {
    let (mut ep, _, h) = stub(200, release_json("v99.0.0", RELEASES_PAGE), Duration::from_millis(1500));
    ep.timeout = Duration::from_millis(300);
    let t0 = std::time::Instant::now();
    let r = fetch_latest(&ep);
    let took = t0.elapsed();
    assert_eq!(r, Err(CheckError::Offline));
    assert!(took < Duration::from_millis(1200), "the whole-request timeout bounds the wait ({took:?})");
    h.join().unwrap();
}

#[test]
fn update_nothing_listening_is_offline() {
    // Bind, learn the port, close: nothing answers there.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let ep = Endpoint {
        url: format!("http://127.0.0.1:{port}/x"),
        connect_timeout: Duration::from_secs(2),
        timeout: Duration::from_secs(3),
    };
    assert_eq!(fetch_latest(&ep), Err(CheckError::Offline));
}

#[test]
fn update_a_page_outside_the_repository_is_replaced() {
    for page in [
        "https://evil.example/skymanbp/autoshade/releases/latest",
        "https://github.com/skymanbp/autoshade-fake/releases/latest",
        "https://github.com/skymanbp/autoshadex/releases",
        "http://github.com/skymanbp/autoshade/releases/latest",
        "https://github.com.evil.example/skymanbp/autoshade/",
        "https://github.com/skymanbp/autoshade/releases/tag/v1 javascript:alert(1)",
        "https://github.com/skymanbp/autoshade/releases/tag/v1\nhttps://evil.example",
        "file:///C:/Windows/System32/calc.exe",
        "",
    ] {
        let r = parse_release(&release_json("v1.2.3", page)).expect("the tag still reads");
        assert_eq!(r.page, RELEASES_PAGE, "{page:?} must be replaced");
    }
    // Missing or not a string: the fixed page too.
    let r = parse_release(br#"{"tag_name":"v1.2.3","html_url":42}"#).unwrap();
    assert_eq!(r.page, RELEASES_PAGE);
    let r = parse_release(br#"{"tag_name":"v1.2.3"}"#).unwrap();
    assert_eq!(r.page, RELEASES_PAGE);
    // Inside the repository: kept.
    let ok = "https://github.com/skymanbp/autoshade/releases/tag/v1.2.3";
    assert_eq!(parse_release(&release_json("v1.2.3", ok)).unwrap().page, ok);
}

#[test]
fn update_the_request_carries_exactly_our_two_headers_and_no_authorization() {
    let (ep, seen, h) = stub(200, release_json("v1.2.3", RELEASES_PAGE), Duration::ZERO);
    fetch_latest(&ep).expect("reads");
    h.join().unwrap();
    let seen = seen.lock().unwrap().clone().expect("the stub received the request");
    assert_eq!(seen.method, "GET");
    assert_eq!(seen.path, "/repos/skymanbp/autoshade/releases/latest");
    let get = |name: &str| seen.headers.iter().filter(|(n, _)| n == name).map(|(_, v)| v.clone()).collect::<Vec<_>>();
    assert_eq!(get("user-agent"), vec![format!("AutoShade/{}", env!("CARGO_PKG_VERSION"))]);
    assert_eq!(get("accept"), vec!["application/vnd.github+json".to_owned()]);
    assert!(get("authorization").is_empty(), "no Authorization: {:?}", seen.headers);
    // Nothing else of ours: only what HTTP itself needs — the host, and the
    // client's own compression offer (its gzip feature).
    for (name, value) in &seen.headers {
        assert!(
            matches!(name.as_str(), "host" | "user-agent" | "accept" | "accept-encoding"),
            "unexpected header {name}: {value}"
        );
    }
    assert!(get("accept-encoding").iter().all(|v| v == "gzip"), "{:?}", seen.headers);
}
