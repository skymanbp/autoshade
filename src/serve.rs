//! Local web UI server. `autoshade serve <dir>` starts a tiny HTTP server; open
//! the printed URL in a browser. Photos are addressed by their index in the
//! in-memory source list (`?id=N`) so we never URL-encode Windows paths. The list
//! is mutable (behind a lock) so the UI can **import** more files/folders at
//! runtime.
//!
//! Interactive feedback (before/after, slider tweaks) runs [`render::develop_preview`]
//! over a downscaled **neutral develop of the RAW** ([`develop_base`]) — the same
//! decode Export develops from, so a slider means the same thing in both; only
//! explicit **Export** / **Download** run the full-resolution
//! [`render::render_to_file`]. The gallery grid keeps the camera's cheap embedded
//! rendition ([`thumb_base`]).

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use image::{DynamicImage, ImageFormat};
use serde::Deserialize;
use serde_json::json;
use tiny_http::{Header, Request, Response, ResponseBox, Server};

use crate::config::{Config, LocalSettings};
use crate::decode;
use crate::denoise::DenoiseOpts;
use crate::pipeline;
use crate::recipe::EditRecipe;
use crate::render;

const INDEX_HTML: &str = include_str!("web/index.html");
const LIST_CAP: usize = 1000; // cap thumbnails shown
const PREVIEW_EDGE: u32 = 1200; // max edge of the cached develop/preview base

/// A fresh, unguessable capability for this server run. It lives only in
/// memory and in the same-origin HTML that needs it.
fn session_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("generate web session token: {e}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// Put the run's capability on the two URLs browsers load as images, and in
/// the page's fetch wrapper, which stamps it onto every state-changing
/// request (`X-AutoShade-Token` — the POST guard in `handle`).
fn tokenized_index(token: &str) -> String {
    INDEX_HTML
        .replace("/api/thumb?id=", &format!("/api/thumb?token={token}&id="))
        .replace("/api/preview?id=", &format!("/api/preview?token={token}&id="))
        .replace("__AUTOSHADE_SESSION_TOKEN__", token)
}

/// Browsers with Fetch Metadata identify a request initiated by another site.
/// Missing is accepted deliberately: curl and other local non-browser clients
/// do not send this header.
fn fetch_site_is_cross_site(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim().eq_ignore_ascii_case("cross-site"))
}

fn image_token_matches(url: &str, expected: &str) -> bool {
    query_param(url, "token").as_deref() == Some(expected)
}

struct AppState {
    /// The working directory the gallery lists from. Behind a lock so the UI can
    /// switch folders at runtime (POST /api/setdir re-scans and swaps it + `raws`).
    dir: RwLock<PathBuf>,
    /// The source list, mutable so the UI can import more / switch folders at runtime.
    raws: RwLock<Vec<PathBuf>>,
    /// Config behind a lock so the Settings panel can hot-reload it (POST
    /// /api/settings rewrites the local file, then swaps in a fresh `Config`).
    cfg: RwLock<Config>,
    /// Folder-switch CLAIM counter: each /api/setdir claims the next number
    /// BEFORE scanning and only installs its result while still current —
    /// without this, a SLOW scan of folder A finishing after a fast scan of
    /// folder B silently swapped the server back to A under B's UI.
    dir_gen: std::sync::atomic::AtomicU64,
    /// The generation actually INSTALLED — stamped inside the same
    /// write-lock scope that swaps `raws`/`dir`. Listings and the stale-id
    /// guards compare against THIS, never the claim counter: the claim
    /// advances at the START of a possibly-slow scan, so stamping listings
    /// with it handed rows still read from the OLD table the NEW generation
    /// — a stale id then passed the guard against the swapped table.
    installed_gen: std::sync::atomic::AtomicU64,
    /// The port we are actually listening on — the loopback-origin guard
    /// compares it against the request's Host/Origin.
    port: u16,
}

impl AppState {
    /// The path at index `id` (cloned, lock released immediately).
    fn at(&self, id: usize) -> Option<PathBuf> {
        self.raws.read().ok()?.get(id).cloned()
    }
    fn count(&self) -> usize {
        self.raws.read().map(|r| r.len()).unwrap_or(0)
    }
    /// Current working-directory path as a display string (recovers from poison).
    fn dir_display(&self) -> String {
        self.dir.read().unwrap_or_else(|e| e.into_inner()).display().to_string()
    }
    /// Current config snapshot (read guard; recovers from a poisoned lock).
    fn config(&self) -> std::sync::RwLockReadGuard<'_, Config> {
        self.cfg.read().unwrap_or_else(|e| e.into_inner())
    }
    /// Resolve `id` for an id-bound MUTATION: the generation check and the
    /// index lookup happen under ONE read guard, so a folder switch can
    /// never slip between them (`stamp` = the client's X-AutoShade-Gen
    /// value). Needed in ADDITION to the pre-dispatch
    /// `stale_generation` bail — that one runs before the request BODY is
    /// read, and a body can take seconds (a full-res mask), long enough for
    /// a switch to install a new table under the old id.
    fn at_checked(&self, id: usize, stamp: Option<u64>) -> std::result::Result<PathBuf, ResponseBox> {
        let Ok(raws) = self.raws.read() else {
            return Err(status_response(500, "listing lock poisoned"));
        };
        if stamp != Some(self.installed_gen.load(std::sync::atomic::Ordering::SeqCst)) {
            return Err(status_response(
                409,
                "the gallery moved to another folder — reload the page and try again",
            ));
        }
        match raws.get(id) {
            Some(p) => Ok(p.clone()),
            None => Err(status_response(400, "bad id")),
        }
    }
}

pub fn serve(dir: &Path, port: u16) -> Result<()> {
    // Sources = RAWs + already-baked PNG/TIFF/JPEG (the PNG-source edit mode).
    let raws = pipeline::find_sources(dir)?;
    let n = raws.len();
    let image_token = Arc::new(session_token()?);
    let requested_addr = format!("127.0.0.1:{port}");
    let server = Server::http(&requested_addr)
        .map_err(|e| anyhow!("start server on {requested_addr}: {e}"))?;
    // Preserve the useful `--port 0` convention: the socket is already bound,
    // so this is the single authoritative port for the banner and all guards.
    let port = server_port(&server)?;
    let addr = format!("127.0.0.1:{port}");
    let state = Arc::new(AppState {
        dir: RwLock::new(dir.to_path_buf()),
        raws: RwLock::new(raws),
        cfg: RwLock::new(Config::load()),
        dir_gen: std::sync::atomic::AtomicU64::new(0),
        installed_gen: std::sync::atomic::AtomicU64::new(0),
        port,
    });
    // Reclaim download/mask temp files a previous run failed to unlink —
    // Windows has no automatic %TEMP% cleaner, so nobody else ever would.
    sweep_stale_temp_files();
    println!("AutoShade UI: {n} source(s) under {}", dir.display());
    println!("  open  →  http://{addr}");
    if state.config().openai_api_key.is_none() {
        println!("  note: no image API key set — Analyze will use the heuristic baseline.");
        println!("        configure providers + keys in the in-app Settings (⚙) panel.");
    }

    // Bounded request concurrency: thread-per-request with NO cap let a burst
    // of expensive requests (an upload alone can hold ~500 MiB) stack
    // unbounded threads and memory. Eight far exceeds a single-user browser's
    // real parallelism; excess requests WAIT in the accept loop (browsers
    // queue politely) instead of failing.
    const MAX_CONCURRENT: usize = 8;
    let gate = Arc::new((std::sync::Mutex::new(0usize), std::sync::Condvar::new()));
    for request in server.incoming_requests() {
        let state = Arc::clone(&state);
        let image_token = Arc::clone(&image_token);
        let permit = Permit::acquire(Arc::clone(&gate), MAX_CONCURRENT);
        std::thread::spawn(move || {
            let _permit = permit;
            if let Err(e) = handle(request, &state, image_token.as_str()) {
                eprintln!("request error: {e}");
            }
        });
    }
    Ok(())
}

fn server_port(server: &Server) -> Result<u16> {
    server
        .server_addr()
        .to_ip()
        .map(|addr| addr.port())
        .ok_or_else(|| anyhow!("local HTTP server did not bind an IP socket"))
}

/// One of the eight request slots, released on DROP.
///
/// Releasing it with straight-line code after `handle` looked equivalent and
/// was not: a panic inside a handler unwinds past that code, so the slot was
/// never given back and the condvar never notified. Eight panicking requests
/// wedged the accept loop forever — the server stopped answering and the only
/// symptom was a browser that hung. This file already assumes handlers panic
/// (every `Mutex::lock` here is `unwrap_or_else(|p| p.into_inner())`); the gate
/// was the one place that assumption was not carried through, and the one
/// place the consequence was unrecoverable. The handler thread reads image and
/// RAW bytes through third-party parsers, which is exactly where a panic on a
/// malformed file comes from.
struct Permit(Arc<(std::sync::Mutex<usize>, std::sync::Condvar)>);

impl Permit {
    fn acquire(gate: Arc<(std::sync::Mutex<usize>, std::sync::Condvar)>, max: usize) -> Self {
        {
            let (lock, cv) = &*gate;
            let mut n = lock.lock().unwrap_or_else(|p| p.into_inner());
            while *n >= max {
                n = cv.wait(n).unwrap_or_else(|p| p.into_inner());
            }
            *n += 1;
        }
        Permit(gate)
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        // Deliberately NOT conditioned on `std::thread::panicking()`: the whole
        // point is that the panicking path releases too. A poisoned mutex is
        // recovered rather than re-panicked, matching every other lock here —
        // turning a past panic into a second one inside a Drop would abort the
        // process.
        let (lock, cv) = &*self.0;
        let mut n = lock.lock().unwrap_or_else(|p| p.into_inner());
        *n = n.saturating_sub(1);
        cv.notify_one();
    }
}

/// Read a request header (case-insensitive), if present.
fn req_header(request: &Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

/// The client's `X-AutoShade-Gen` stamp (the listing generation its photo
/// ids came from), if present and numeric.
fn request_gen(request: &Request) -> Option<u64> {
    req_header(request, "X-AutoShade-Gen").and_then(|v| v.trim().parse::<u64>().ok())
}

/// Is this `Host`/`Origin` authority OUR loopback authority? Module-level and
/// not nested inside [`foreign_origin`] so the rule can be unit-tested
/// directly — it is the whole cross-origin guard, and the one bug it had was
/// invisible from outside.
fn loopback(h: &str, port: u16) -> bool {
    let h = h.trim();
    // "127.0.0.1:8080" / "localhost" / "[::1]:8080" / "[::1]"
    //
    // Split the BRACKETS first. Splitting on the last colon and then asking
    // whether the left side ended in `]` mis-parsed a port-less IPv6 literal:
    // `"[::1]".rsplit_once(':')` cuts at the literal's OWN last colon, giving
    // name `"[:"`, which matches nothing. That failed closed, so it was never
    // a bypass — but it meant the rule this function exists to state ("an
    // absent port is the scheme default, 80") was not implemented for the one
    // authority form the fix that introduced it called out, and the assertion
    // covering it passed for the wrong reason.
    let (name, p): (&str, Option<&str>) = if h.starts_with('[') {
        let Some(close) = h.find(']') else { return false };
        let (name, tail) = h.split_at(close + 1);
        let p = match tail {
            "" => None,
            t => match t.strip_prefix(':') {
                Some(p) => Some(p),
                // Anything else after the bracket is not an authority.
                None => return false,
            },
        };
        (name, p)
    } else {
        match h.rsplit_once(':') {
            Some((n, p)) => (n, Some(p)),
            None => (h, None),
        }
    };
    // Host names are case-insensitive (RFC 3986). Browsers normalise to lower
    // case before sending, so this only ever admits our own tooling and a
    // hand-typed `http://LOCALHOST:8080` — never a foreign host.
    let name_ok = ["127.0.0.1", "localhost", "[::1]"]
        .iter()
        .any(|n| name.eq_ignore_ascii_case(n));
    // An ABSENT port is not a wildcard — it is the scheme default, 80.
    // Treating it as "matches whatever we listen on" made
    // `Origin: http://localhost` same-origin for a server on 8080, so any page
    // served on loopback port 80 (a local IIS/XAMPP, a docker -p 80, a docs
    // server with an HTML injection) could POST simple requests to us:
    // /api/settings would repoint the AI base URL at the attacker and the next
    // Analyze would hand over the user's API key.
    let port_ok = match p {
        Some(p) => p == port.to_string(),
        None => port == 80,
    };
    name_ok && port_ok
}

/// Refuse anything a HOSTILE WEB PAGE could send us.
///
/// Binding 127.0.0.1 protects nothing on its own: another page can address
/// that socket directly, and cross-site image GETs need not carry `Origin`.
/// Browsers that implement Fetch Metadata identify those requests with
/// `Sec-Fetch-Site: cross-site`; an absent header remains accepted so curl
/// and other local tooling keep working. The literal loopback `Host` and
/// same-origin (or absent) `Origin` checks still stop DNS rebinding and
/// cross-origin API requests. Returns the refusal to send, or None to accept.
fn foreign_origin(request: &Request, port: u16) -> Option<ResponseBox> {
    if fetch_site_is_cross_site(req_header(request, "Sec-Fetch-Site").as_deref()) {
        return Some(status_response(
            403,
            "refused: this request was initiated by a cross-site web page",
        ));
    }
    if let Some(host) = req_header(request, "Host")
        && !loopback(&host, port)
    {
        return Some(status_response(
            403,
            "refused: this server only answers requests addressed to localhost",
        ));
    }
    if let Some(origin) = req_header(request, "Origin") {
        let ok = origin.strip_prefix("http://").is_some_and(|h| loopback(h, port));
        if !ok {
            return Some(status_response(
                403,
                "refused: cross-origin requests are not accepted by the local AutoShade server",
            ));
        }
    }
    None
}

/// Routes that RESOLVE A PHOTO ID and then write (or render what a write will
/// use). Photo ids are indexes into the CURRENT folder listing, so one built
/// before a folder switch would silently land on an unrelated photo.
fn id_bound_mutation(path: &str) -> bool {
    matches!(
        path,
        "/api/analyze"
            | "/api/develop"
            | "/api/export"
            | "/api/download"
            | "/api/xmp"
            | "/api/retouch"
            | "/api/heal"
    )
}

/// Refuse an id-bearing mutation built for a DIFFERENT folder listing. The
/// client stamps every /api request with the generation its ids came from
/// (`X-AutoShade-Gen`, see index.html); a mismatch means the gallery moved
/// under it and the id now points at another photo.
/// EARLY bail only — it runs before the request body is read; the
/// authoritative re-check happens at id-resolution time under the listing
/// lock (`AppState::at_checked`), because a folder switch can complete
/// while a large body is still uploading.
fn stale_generation(request: &Request, state: &AppState) -> Option<ResponseBox> {
    let cur = state.installed_gen.load(std::sync::atomic::Ordering::SeqCst);
    match request_gen(request) {
        Some(g) if g == cur => None,
        _ => Some(status_response(
            409,
            "the gallery moved to another folder — reload the page and try again",
        )),
    }
}

fn handle(mut request: Request, state: &AppState, image_token: &str) -> Result<()> {
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("/");
    let is_post = request.method() == &tiny_http::Method::Post;

    if let Some(refusal) = foreign_origin(&request, state.port) {
        return request.respond(refusal).map_err(Into::into);
    }
    if matches!(path, "/api/thumb" | "/api/preview") && !image_token_matches(&url, image_token) {
        return request
            .respond(status_response(
                403,
                "this image link belongs to another AutoShade server session; reopen the printed \
                 AutoShade UI URL to refresh the page and its image links",
            ))
            .map_err(Into::into);
    }
    // EVERY state-changing route needs the session capability, not just the
    // image GETs: the Origin/Sec-Fetch/Host trio blocks modern browsers, but
    // a client that sends none of those headers used to reach
    // /api/settings & friends unauthenticated (16-lane scan L10). The page's
    // fetch wrapper stamps the header; anything else gets 403.
    if is_post && req_header(&request, "X-AutoShade-Token").as_deref() != Some(image_token) {
        return request
            .respond(status_response(
                403,
                "this request carries no valid AutoShade session token; reopen the printed \
                 AutoShade UI URL and retry from that page",
            ))
            .map_err(Into::into);
    }
    if id_bound_mutation(path)
        && let Some(stale) = stale_generation(&request, state)
    {
        return request.respond(stale).map_err(Into::into);
    }

    // Handlers BUILD a response instead of consuming the request, so the request
    // is still ours when one of them fails.
    let reply = match (is_post, path) {
        (false, "/") => Ok(html_response(&tokenized_index(image_token))),
        (false, "/api/list") => api_list(&request, state),
        (false, "/api/thumb") => api_image(&request, state, 256),
        (false, "/api/preview") => api_image(&request, state, PREVIEW_EDGE),
        (false, "/api/recipe") => api_recipe(&request, state),
        (false, "/api/fresh-base") => api_fresh_base(&request, state),
        (false, "/api/style-info") => api_style_info(state),
        (true, "/api/style-build") => api_style_build(&mut request),
        (false, "/api/settings") => api_settings_get(state),
        (true, "/api/settings") => api_settings_post(&mut request, state),
        (true, "/api/setdir") => api_setdir(&mut request, state),
        (true, "/api/import") => api_import(&mut request, state),
        (true, "/api/upload") => api_upload(&mut request, state),
        (true, "/api/analyze") => api_analyze(&mut request, state),
        (true, "/api/develop") => api_develop(&mut request, state),
        (true, "/api/retouch") => api_retouch(&mut request, state),
        (true, "/api/heal") => api_heal(&mut request, state),
        (true, "/api/export") => api_export(&mut request, state),
        (true, "/api/download") => api_download(&mut request, state),
        (true, "/api/xmp") => api_xmp(&mut request, state),
        _ => Ok(status_response(404, "not found")),
    };
    // A failure has to reach the BROWSER: a dropped request answers with a
    // BODYLESS 500 (tiny_http's `impl Drop for Request`), which is why the
    // advisor's real message ("Not logged in · run /login") only ever appeared in
    // the terminal. `{:#}` carries anyhow's whole source chain into the body.
    let reply = reply.unwrap_or_else(|e| {
        eprintln!("request error ({path}): {e:#}");
        // A CLIENT-caused failure (malformed JSON, bad id) answers 400 — the
        // blanket 500 told the web UI the SERVER broke.
        let status = if e.downcast_ref::<ClientErr>().is_some() {
            400
        } else {
            // GuardedBody's two invented kinds (anyhow downcast passes
            // through .context layers): a pacing timeout is the client's
            // 408; an exhausted aggregate budget is a 503, not our 500.
            match e.downcast_ref::<std::io::Error>().map(|io| io.kind()) {
                Some(std::io::ErrorKind::TimedOut) => 408,
                Some(std::io::ErrorKind::OutOfMemory) => 503,
                _ => 500,
            }
        };
        status_response(status, &format!("{e:#}"))
    });
    request.respond(reply).map_err(Into::into)
}

/// Full-resolution work runs ONE AT A TIME.
///
/// The request gate in `serve` bounds how many requests are in flight, not
/// how much memory they use: a single 61-megapixel export peaks near 1.7 GiB
/// (decoded sensor + f32 develop buffer + the packed 16-bit frame + the lens
/// geometry destination), so eight of them admitted together reach roughly
/// 13.6 GiB before caches, request bodies and encoders — an out-of-memory
/// kill on any ordinary machine. Preview-sized work (list, thumbnails,
/// /api/develop at PREVIEW_EDGE) stays parallel; only the full-resolution
/// RENDER paths queue here. One at a time is also what the user is waiting
/// for anyway: they cannot look at two exports at once, and finishing the
/// first sooner beats thrashing through both.
///
/// Deliberately NOT taken by /api/retouch and /api/heal. Their wall clock is
/// dominated by a generative API call that can run for MINUTES, and holding
/// this across the network would block every export for that entire time to
/// solve a memory problem that exists only during their brief local
/// compositing phase. Scoping a permit to that phase is the open follow-up;
/// their peak is recorded with the other engine buffer-lifetime items.
///
/// Lock ORDER: taken at handler entry, before SAVE_LOCK. No path takes
/// SAVE_LOCK first and then this, so the two cannot deadlock.
static HEAVY: std::sync::Mutex<()> = std::sync::Mutex::new(());



/// Marker for client-caused failures — `handle` maps these to HTTP 400.
#[derive(Debug)]
struct ClientErr(String);
impl std::fmt::Display for ClientErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ClientErr {}

// --- handlers --------------------------------------------------------------

fn api_list(request: &Request, state: &AppState) -> Result<ResponseBox> {
    // Pagination: `?offset=&limit=` page through the full list (a folder can hold
    // thousands). `id` stays the GLOBAL index (enumerate BEFORE skip), so
    // selecting / previewing by id works across pages. `limit` is capped at
    // LIST_CAP to bound the per-request decode/JSON work.
    let offset = query_param(request.url(), "offset")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = query_param(request.url(), "limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(LIST_CAP)
        .clamp(1, LIST_CAP);
    // The INSTALLED generation, read while HOLDING the read guard: the
    // install stamps it inside the write-guard scope, so (gen, items) is one
    // coherent pair. The old pre-lock read of the CLAIM counter — bumped
    // BEFORE the possibly-slow folder scan — stamped rows still read from
    // the OLD table with the NEW generation whenever a listing landed inside
    // a switch's scan window, and a mutation built from those rows then
    // passed the stale-generation guard against the swapped table.
    let raws = state.raws.read().map_err(|_| anyhow!("lock poisoned"))?;
    let listing_gen = state.installed_gen.load(std::sync::atomic::Ordering::SeqCst);
    let total = raws.len();
    let items: Vec<_> = raws
        .iter()
        .enumerate()
        .skip(offset)
        .take(limit)
        .map(|(id, raw)| {
            // Central store OR legacy ./out, recipe OR XMP — pre-migration
            // libraries keep their "analyzed" badges — OR Lightroom's own
            // sidecar (L13#2: the web restore already prefers it, so the
            // badge must agree with what a click restores).
            let analyzed = crate::store::has_develop_or_sidecar(raw);
            json!({
                "id": id,
                "stem": pipeline::stem(raw),
                "baked": !decode::is_raw(raw),
                "analyzed": analyzed,
            })
        })
        .collect();
    let body = json!({
        "dir": state.dir_display(),
        "total": total,
        "offset": offset,
        "limit": limit,
        "shown": items.len(),
        "items": items,
        "gen": listing_gen,
    });
    Ok(json_response(&body))
}

#[derive(Deserialize)]
struct SetDirReq {
    /// A folder path on disk to make the new working directory.
    path: String,
}

/// Switch the working directory at runtime: re-scan `path` for sources and
/// replace the gallery. Path-based (a browser can't hand a local server a picked
/// folder's real disk path), mirroring the Import field. Any files uploaded into
/// ./out/imported drop out of the view but stay on disk.
fn api_setdir(request: &mut Request, state: &AppState) -> Result<ResponseBox> {
    let req: SetDirReq = read_json(request)?;
    // Tolerate Windows "Copy as path" (quotes) + stray whitespace, like Import.
    let cleaned = req.path.trim().trim_matches('"').trim();
    let p = PathBuf::from(cleaned);
    if !p.is_dir() {
        return Ok(status_response(400, &format!("not a folder: {cleaned}")));
    }
    // Claim a generation BEFORE the (possibly slow) scan; install only while
    // still the newest request. SeqCst: the claim and the install check must
    // be totally ordered across request threads.
    use std::sync::atomic::Ordering;
    let claimed_gen = state.dir_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let found = pipeline::find_sources(&p)?;
    let total = found.len();
    {
        // Replace BOTH under simultaneously-held locks — separate scopes let
        // two concurrent setdir calls interleave into raws(A) + dir(B),
        // permanently mismatching the list and its folder.
        let mut raws = state.raws.write().map_err(|_| anyhow!("lock poisoned"))?;
        let mut dir = state.dir.write().map_err(|_| anyhow!("lock poisoned"))?;
        if state.dir_gen.load(Ordering::SeqCst) != claimed_gen {
            // A newer switch already claimed the state (or will momentarily):
            // this stale scan must not overwrite it.
            return Ok(status_response(409, "superseded by a newer folder switch"));
        }
        *raws = found;
        *dir = p.clone();
        // Inside the write-guard scope: readers holding the read guard see
        // (installed_gen, raws) as one coherent pair.
        state.installed_gen.store(claimed_gen, Ordering::SeqCst);
    }
    Ok(json_response(
        &json!({ "dir": p.display().to_string(), "total": total, "gen": claimed_gen }),
    ))
}

#[derive(Deserialize)]
struct ImportReq {
    /// A file or folder path on disk (this server runs locally).
    path: String,
}

/// Add a file or (recursively) a folder of sources to the gallery at runtime.
fn api_import(request: &mut Request, state: &AppState) -> Result<ResponseBox> {
    let req: ImportReq = read_json(request)?;
    // Tolerate Windows "Copy as path" (wraps the path in quotes) + stray spaces.
    let cleaned = req.path.trim().trim_matches('"').trim();
    let p = PathBuf::from(cleaned);
    let found: Vec<PathBuf> = if p.is_dir() {
        pipeline::find_sources(&p)?
    } else if p.is_file() && (decode::is_raw(&p) || is_baked_ext(&p)) {
        vec![p.clone()]
    } else {
        return Ok(status_response(400, &format!("not a file/folder I can read: {cleaned}")));
    };

    let mut added = 0usize;
    let mut first_id: Option<usize> = None;
    {
        let mut raws = state.raws.write().map_err(|_| anyhow!("lock poisoned"))?;
        // Set-based dedupe: the old linear `contains` per candidate was
        // O(M×N) under the EXCLUSIVE lock — a big folder import froze every
        // reader for the whole scan. `insert` (not `contains`) keeps the old
        // semantics for duplicates WITHIN one import batch too.
        let mut existing: std::collections::HashSet<PathBuf> = raws.iter().cloned().collect();
        for np in found {
            if existing.insert(np.clone()) {
                raws.push(np);
                if first_id.is_none() {
                    first_id = Some(raws.len() - 1);
                }
                added += 1;
            }
        }
    }
    // first_id is authoritative — the client used to INFER it as
    // total − added, which a concurrent import/upload could shift.
    Ok(json_response(
        &json!({ "added": added, "total": state.count(), "first_id": first_id }),
    ))
}

/// Accept dropped/picked file BYTES, save under ./out/imported, and add it to the
/// gallery. Browsers can't hand a local server the original disk path, so
/// drag-drop uploads the bytes (path-based Import stays for your on-disk library).
/// Filename comes from the `X-Filename` header.
fn api_upload(request: &mut Request, state: &AppState) -> Result<ResponseBox> {
    let name = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("X-Filename"))
        .map(|h| percent_decode(h.value.as_str()))
        .unwrap_or_default();
    // basename only — never let an upload name escape ./out/imported.
    let safe = Path::new(&name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let as_path = PathBuf::from(&safe);
    if safe.is_empty() || !(decode::is_raw(&as_path) || is_baked_ext(&as_path)) {
        return Ok(status_response(400, "unsupported or unnamed file"));
    }

    // Bounded read: an unbounded read_to_end let one oversized (or malicious)
    // upload exhaust process memory. 500 MB comfortably covers any current
    // RAW/TIFF; past the cap the request is refused, not truncated.
    const MAX_UPLOAD: usize = 500 * 1024 * 1024;
    // Declared-size pre-reject: a 4 GB declared upload used to read 500 MB
    // before refusing (chunked bodies fall through to the loop cap).
    if let Some(len) = request.body_length()
        && len > MAX_UPLOAD
    {
        return Ok(status_response(413, "upload exceeds the 500 MB limit"));
    }
    let mut bytes = Vec::new();
    // Bounded manual read loop (Take<T> on a trait object fights the method
    // resolver): refuse — never truncate — anything past the cap. The reads
    // go through GuardedBody, so this body also charges the aggregate
    // budget and obeys the pacing deadline.
    let mut budget = BodyBudget(0);
    let mut gb = GuardedBody::new(request.as_reader(), &mut budget);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = std::io::Read::read(&mut gb, &mut chunk).context("read upload body")?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() > MAX_UPLOAD {
            return Ok(status_response(413, "upload exceeds the 500 MB limit"));
        }
    }

    if bytes.is_empty() {
        // An empty body passed every gate and landed as a PERMANENT
        // zero-byte gallery item nothing can open. The name said "photo";
        // the payload must at least exist — deeper sniffing stays the
        // decoder's job at open time.
        return Ok(status_response(400, "the uploaded file is empty"));
    }
    // NOT the delivery root (M8), deliberately: an upload is library INPUT —
    // the SOURCE photo a develop is about to be made from — and it only ever
    // shared the `out` folder name with the deliverables. Repointing the
    // delivery root would otherwise file a stranger's upload into the user's
    // finished-work folder. See `config::delivery_root`'s "does not cover".
    let dir = PathBuf::from("out").join("imported");
    std::fs::create_dir_all(&dir).context("create out/imported")?;
    // Same basename ≠ same photo: never truncate an existing import — pick
    // "name (2).ext" style until free, so two shoots' DSC0001.ARW coexist.
    // The claim must be ATOMIC (create_new): request handlers run
    // concurrently, and two same-named uploads could both pass a bare
    // exists() probe and then truncate each other.
    let stem = as_path.file_stem().and_then(|s| s.to_str()).unwrap_or("upload").to_string();
    let ext = as_path.extension().and_then(|s| s.to_str()).unwrap_or("bin").to_string();
    let mut dest = dir.join(&safe);
    let mut file = None;
    for n in 1u32.. {
        if n > 1 {
            dest = dir.join(format!("{stem} ({n}).{ext}"));
        }
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&dest) {
            Ok(f) => {
                file = Some(f);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("create {}", dest.display()));
            }
        }
    }
    {
        use std::io::Write as _;
        let mut f = file.expect("the claim loop only breaks with an open file");
        if let Err(e) = f.write_all(&bytes) {
            // Drop the claim on failure — a partial file left at the intended
            // basename would corrupt the import AND push every retry to
            // "name (2)" forever.
            drop(f);
            let _ = std::fs::remove_file(&dest);
            return Err(e).with_context(|| format!("write {}", dest.display()));
        }
    }

    let id = {
        let mut raws = state.raws.write().map_err(|_| anyhow!("lock poisoned"))?;
        match raws.iter().position(|p| p == &dest) {
            Some(i) => i,
            None => {
                raws.push(dest.clone());
                raws.len() - 1
            }
        }
    };
    Ok(json_response(
        &json!({ "id": id, "total": state.count(), "stem": pipeline::stem(&dest) }),
    ))
}

/// A process-wide (path, mtime)-keyed cache of decoded bases (same pattern as
/// render.rs's bitmap-mask cache). The UI re-asks for the same base on EVERY
/// slider gesture, and decoding it is orders of magnitude dearer than the develop
/// itself. mtime in the key is the invalidation: an overwritten source misses and
/// re-decodes. `cap` bounds the memory; entries are evicted in insertion order.
// Identity = (mtime, size): mtime alone misses a same-timestamp overwrite on
// coarse-granularity filesystems (thumb + mask caches carry size for the same
// reason).
type BaseCache = Mutex<Vec<(PathBuf, (std::time::SystemTime, u64), Arc<DynamicImage>)>>;

fn cached_base(
    cache: &BaseCache,
    cap: usize,
    raw: &Path,
    build: impl FnOnce() -> Result<DynamicImage>,
) -> Result<Arc<DynamicImage>> {
    let ident = std::fs::metadata(raw)
        .ok()
        .and_then(|m| m.modified().ok().map(|t| (t, m.len())));
    if let Some(t) = ident {
        // No user code runs under the lock, so poisoning is not reachable —
        // recover anyway rather than turning a past panic into a new one.
        let list = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((_, _, img)) = list.iter().find(|(p, ct, _)| *ct == t && p == raw) {
            return Ok(img.clone());
        }
    }
    let img = Arc::new(build()?);
    if let Some(t) = ident {
        let mut list = cache.lock().unwrap_or_else(|p| p.into_inner());
        list.retain(|(p, _, _)| p != raw); // a stale-identity entry for this path is dead
        if list.len() >= cap {
            list.remove(0); // small Vec in insertion order → evict the oldest insert
        }
        list.push((raw.to_path_buf(), t, img.clone()));
    }
    Ok(img)
}

/// The camera's embedded rendition, resized to the web base — for the GALLERY
/// GRID only, where an already tone-mapped JPEG is exactly what we want and a
/// full demosaic per tile would be unusable (a page shows 200).
fn thumb_base(raw: &Path) -> Result<Arc<DynamicImage>> {
    static CACHE: OnceLock<BaseCache> = OnceLock::new();
    cached_base(CACHE.get_or_init(Default::default), 8, raw, || {
        Ok(decode::preview_only(raw)?
            .resize(PREVIEW_EDGE, PREVIEW_EDGE, image::imageops::FilterType::Triangle))
    })
}

/// The base BOTH panes of the editor use: the RAW sensor data developed with a
/// NEUTRAL recipe and downscaled — the same decode `render_to_file` exports from,
/// and the same base the GUI builds (`gui.rs` `open_path`). This used to develop
/// the camera's baked 8-bit JPEG instead, which double-processes it: highlights
/// already clipped to 255 cannot be recovered, so Highlights/Whites (and grain
/// under clarity/contrast) behaved nothing like the export of the same recipe.
///
/// RESIDUAL DIVERGENCE from the export, inherent to a fast preview and NOT fixed
/// here: the base is 8-bit and downscaled to `PREVIEW_EDGE`, so radius-based
/// stages (clarity, noise reduction, sharpening) act on fewer pixels and read
/// slightly softer; crop / straighten / lens distortion are deliberately skipped
/// so the sliders keep full-frame feedback (see `render::develop_preview`); and
/// AI denoise is export-only. The tone/colour/WB stages the sliders drive now see
/// the same signal as the export.
fn develop_base(raw: &Path) -> Result<Arc<DynamicImage>> {
    static CACHE: OnceLock<BaseCache> = OnceLock::new();
    // A full-sensor develop costs seconds and ~1.5 GB of transients. The before
    // pane and the first slider gesture ask for the SAME photo at the same time
    // (each request gets its own thread), so serialise the build: the loser waits
    // and then finds the cache entry the winner just wrote.
    static BUILD: Mutex<()> = Mutex::new(());
    let _serialised = BUILD.lock().unwrap_or_else(|p| p.into_inner());
    cached_base(CACHE.get_or_init(Default::default), 4, raw, || {
        // ONE dispatch for both source kinds (`render::source_pixels`): a RAW
        // develops AT the preview edge (the cap runs before tone/geometry — the
        // old full-sensor develop existed only to be thumbnailed), a baked
        // source is decoded and `thumbnail`ed to the same edge, only ever DOWN
        // (a source already under it keeps its own pixels; the browser scales
        // the <img> anyway).
        let fitted = render::source_pixels(raw, Some(PREVIEW_EDGE))?;
        // Store 8-bit: `develop_preview` and the JPEG encode both work in
        // 8-bit, so keeping the 16-bit buffer would only double the cache and
        // re-convert on every gesture.
        Ok(DynamicImage::ImageRgb8(fitted.to_rgb8()))
    })
}

fn api_image(request: &Request, state: &AppState, max_edge: u32) -> Result<ResponseBox> {
    let raw = raw_for(request, state)?;
    if max_edge >= PREVIEW_EDGE {
        // The "before" pane must show the rendition the "after" is developed
        // FROM, or the pair compares two different pictures.
        let base = develop_base(&raw)?;
        jpeg_response(&base)
    } else {
        // Thumbnail: resize DOWN from the cached 1200px base instead of paying a
        // second full decode of the same source.
        let base = thumb_base(&raw)?;
        jpeg_response(&base.resize(max_edge, max_edge, image::imageops::FilterType::Triangle))
    }
}

/// Fresh camera-matched base-look knots for the web's fresh-open and
/// XMP-only paths — the SAME develop_base the panes already build (cached,
/// so selecting a photo costs one demosaic total) CDF-matched against the
/// camera's own embedded rendition. A saved recipe.json's curve is NEVER
/// manufactured here: api_recipe serves those verbatim, legacy-empty included.
fn fresh_base_knots(raw: &Path) -> Vec<[f32; 2]> {
    if !decode::is_raw(raw) {
        return Vec::new();
    }
    let camera = match decode::embedded_preview(raw) {
        Ok(Some(c)) => c,
        Ok(None) => return Vec::new(),
        Err(e) => {
            eprintln!("⚠ base look skipped: embedded preview of {} failed ({e})", raw.display());
            return Vec::new();
        }
    };
    match develop_base(raw) {
        Ok(neutral) => {
            // Same contract as the GUI/pipeline estimators: match against the
            // profile-vignette-corrected neutral (render::estimation_base).
            // None (could not judge) = no base look in the fresh-open
            // payload, exactly like the GUI open.
            let est = render::estimation_base(&neutral, &pipeline::fresh_lens_profile(raw));
            render::camera_base_knots(&est, &camera).unwrap_or_default()
        }
        Err(e) => {
            // Disclosed, not silent — the pane render will fail loudly too,
            // but a darker-than-GUI fresh open needs a traceable cause.
            eprintln!("⚠ base look skipped: neutral develop of {} failed ({e})", raw.display());
            Vec::new()
        }
    }
}

fn api_recipe(request: &Request, state: &AppState) -> Result<ResponseBox> {
    let raw = raw_for(request, state)?;
    return crate::store::with_develop_lock(
        &raw,
        crate::store::DevelopLockMode::Wait,
        || {
            // One-time, per-photo migration FIRST (it can rewrite
            // recipe.json), then ONE sidecar read feeds BOTH the body and
            // the tag (review R12-04): Lightroom holds none of our locks,
            // so a second read inside develop_revision could rank a file
            // the body was not built from — body A under ETag B, and the
            // client's later If-Match: B overwrote B with stale A.
            crate::store::migrate_legacy(&raw);
            let ranked = crate::store::lightroom_sidecar(&raw);
            let resp = api_recipe_locked(&raw, &ranked)?;
            Ok(with_revision(
                resp,
                crate::store::develop_revision_of(&raw, &ranked).as_deref(),
            ))
        },
    );

    fn api_recipe_locked(raw: &Path, ranked: &crate::store::LrSidecar) -> Result<ResponseBox> {
    // The sidecar BESIDE the RAW is the one file Lightroom itself writes —
    // newest intent wins, the same contract as the GUI's read_saved_develop
    // (store::lightroom_sidecar: our own copied projection is skipped, ties
    // go to the store). The header says where the recipe came from; the BODY
    // stays a pure EditRecipe — clients post recipes back to /api/develop,
    // where an unknown field is a 422.
    // A6 disclosure survives a NO-OP import (Codex 32-#1): a sidecar whose
    // only edit is corrupt parses to neutral, restores nothing — and the
    // next save then overwrites it. The warning rides every answer below,
    // the 404 "not analyzed yet" included.
    let mut xmp_warn: Option<Header> = None;
    match ranked {
        crate::store::LrSidecar::NewerThanStore(text) | crate::store::LrSidecar::Only(text) => {
            let diag = crate::diag::photo(raw);
            let mut r = crate::xmp::xmp_to_recipe_with_diag(text, &diag);
            xmp_warn = recipe_warning_header(text);
            if !r.is_noop() {
                r.clamp();
                // Same stamp rule as the XMP fallback below: Lightroom tuned
                // this file over its own profile-corrected base.
                r.base_curve = fresh_base_knots(raw);
                r.lens_profile = pipeline::fresh_lens_profile(raw);
                // Stamp-if-None: an old-era AutoShade projection arrives with
                // the 5500 anchor PINNED by xmp_to_recipe (its Kelvin was
                // tuned relative) — overwriting the pin would reinterpret it.
                if r.as_shot_k.is_none() {
                    let (ask, ast) = pipeline::fresh_as_shot_wb(raw);
                    r.as_shot_k = ask;
                    r.as_shot_tint = ast;
                }
                let h = Header::from_bytes(&b"X-Recipe-Source"[..], &b"lightroom-sidecar"[..])
                    .expect("static ASCII header");
                let mut resp = json_text(serde_json::to_string(&r)?).with_header(h);
                if let Some(w) = xmp_warn {
                    resp = resp.with_header(w);
                }
                return Ok(resp);
            }
        }
        crate::store::LrSidecar::Unreadable(why) => {
            // The file EXISTS but cannot answer — the old fold into "absent"
            // answered from the store in silence (L08). The store still
            // answers below; the warning rides every answer.
            // Through header(), never raw from_bytes: {why} is a localized
            // (often non-ASCII) OS error string, and the raw ASCII-only
            // constructor silently killed this warning on such systems.
            xmp_warn = header(
                "X-Recipe-Warning",
                &format!(
                    "a Lightroom sidecar sits beside this photo but could not be read ({why}) - \
                     any Lightroom edits in it are NOT reflected"
                ),
            );
        }
        _ => {}
    }
    // Central first; then any legacy file a failed migration left behind.
    let mut parse_err: Option<String> = None;
    // ANY store file read below — a NEUTRAL one included — bars the embedded
    // packet at the tail (the GUI open path's `!any` rule; Codex L05
    // EMBED-01: a neutral save must not be out-answered by the baked packet).
    let mut store_answered = false;
    for path in [crate::store::recipe_target(raw), crate::store::legacy_recipe(raw)] {
        let text = match crate::store::read_text_capped(&path, crate::store::MAX_STORE_JSON) {
            Ok(t) => {
                store_answered = true;
                t
            }
            // Missing IS absence; any OTHER read failure (permissions, I/O)
            // is an EXISTING save we could not honour — treating it as
            // absent silently resurrected stale legacy/XMP edits over it.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                parse_err = Some(format!("cannot read {}: {e}", path.display()));
                continue;
            }
        };
        // VALIDATE before serving. The policy stays "verbatim" — the file is the
        // authority and re-serialising would drop anything a newer schema wrote —
        // but a CORRUPT file served as-is made the browser's `await rr.json()`
        // throw in the middle of `selectPhoto`, leaving an empty control panel
        // and no error anywhere. The check is schema-AGNOSTIC (`Value`, not
        // `EditRecipe`) so a newer-schema file still goes out byte-for-byte.
        // Syntax-validated WITHOUT building a tree: `IgnoredAny` walks the
        // whole document in O(depth) memory, where `Value` materialised a
        // ~10× tree of a hostile recipe.json just to throw it away
        // (L02/L16). The object test is syntactic: a JSON document whose
        // first token is not `{` — `null`, an array, a bare scalar — can
        // never be a recipe of ANY schema generation; that is corruption,
        // not a newer schema (which is still an object, served verbatim).
        match serde_json::from_str::<serde::de::IgnoredAny>(&text) {
            Err(e) => {
                parse_err = Some(e.to_string());
                continue;
            }
            Ok(_) if !text.trim_start().starts_with('{') => {
                parse_err = Some("recipe JSON is not an object".into());
                continue;
            }
            Ok(_) => {}
        }
        // A NEUTRAL recipe.json is not a develop: fall through to the XMP, the
        // GUI's `NoopOnly` rule (`read_saved_develop`). Returning it would tag
        // the photo SAVED for an edit that does nothing.
        if serde_json::from_str::<EditRecipe>(&text).is_ok_and(|r| r.is_noop()) {
            continue;
        }
        // Bare raster names stay bare — api_develop/api_export re-anchor them
        // before rendering.
        //
        // The load-time migrations happen BEFORE the browser gets a copy:
        // everything the web then does — preview, export, download, save — is
        // driven by the object it holds, so serving the file verbatim handed
        // the washed curve (and, since v0.30.0, the sensor-frame crop/masks of
        // a rotated RAW) to every one of them, and the client would have
        // written it straight back on the next save. This is also the ONLY
        // place the web reads a recipe FILE: the request bodies are stamped
        // display-frame on arrival (`live_frame_recipe`), so hooking the
        // render funnels instead would have turned browser-authored geometry.
        // Re-serialised only when a migration actually fired; an untouched
        // recipe still goes out byte-for-byte, forward-schema fields and all.
        if let Ok(mut r) = serde_json::from_str::<EditRecipe>(&text)
            && let Some(note) = crate::pipeline::migrate_loaded_recipe(raw, &mut r).note()
            && let Ok(fixed) = serde_json::to_string(&r)
        {
            // SAID, not just done: the photo renders differently than it did
            // yesterday, and the GUI tells its user exactly that. The client
            // already surfaces this header for corrupt-recipe fallbacks.
            // ASCII by construction — it travels in an HTTP header.
            let mut resp = json_text(fixed);
            if let Some(h) = header("X-Recipe-Warning", &note) {
                resp.add_header(h);
            }
            // The accumulated sidecar warning rides EVERY answer shape
            // (review R12-05): the valid-recipe returns used to drop it,
            // so "the Lightroom sidecar could not be read" vanished exactly
            // when a store recipe answered.
            if let Some(w) = xmp_warn.take() {
                resp = resp.with_header(w);
            }
            return Ok(resp);
        }
        let mut resp = json_text(text.clone());
        if let Some(w) = xmp_warn.take() {
            resp = resp.with_header(w);
        }
        return Ok(resp);
    }
    // No recipe.json → fall back to the XMP sidecar, exactly like the GUI's
    // `read_saved_develop`: a foreign / neutral sidecar parses to a no-op recipe,
    // and "restoring" that would only produce a misleading SAVED tag. An XMP
    // was tuned in Lightroom over ITS camera-profile base, so it gets the
    // photo's fresh base look — the same rule the GUI applies on open.
    for path in [pipeline::xmp_target(raw), crate::store::legacy_xmp(raw)] {
        let text = match crate::store::read_text_capped(&path, crate::store::MAX_STORE_JSON) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                // An EXISTING projection we cannot read is a save we cannot
                // honour: the silent skip served 404 "not analyzed yet" over
                // real saved edits — the same fold-into-absent this handler
                // already refuses for the LR sidecar and for recipe.json.
                // It bars the embedded packet below like any store file.
                store_answered = true;
                eprintln!(
                    "⚠ the saved XMP projection {} exists but cannot be read ({e})",
                    path.display()
                );
                if xmp_warn.is_none() {
                    xmp_warn = header(
                        "X-Recipe-Warning",
                        &format!(
                            "the saved XMP projection could not be read ({e}) - saved \
                             edits are NOT reflected"
                        ),
                    );
                }
                continue;
            }
        };
        {
            store_answered = true;
            let diag = crate::diag::photo(raw);
            let mut r = crate::xmp::xmp_to_recipe_with_diag(&text, &diag);
            // First consulted file wins the disclosure slot (GUI accumulates
            // the same way) — set regardless of noop-ness.
            if xmp_warn.is_none() {
                xmp_warn = recipe_warning_header(&text);
            }
            if !r.is_noop() {
                r.clamp();
                r.base_curve = fresh_base_knots(raw);
                // Same stamp rule as the GUI's XMP-only restore: Lightroom
                // tuned this file over its own profile-corrected base.
                r.lens_profile = pipeline::fresh_lens_profile(raw);
                // Stamp-if-None — same era rule as the sidecar branch above.
                if r.as_shot_k.is_none() {
                    let (ask, ast) = pipeline::fresh_as_shot_wb(raw);
                    r.as_shot_k = ask;
                    r.as_shot_tint = ast;
                }
                let mut resp = json_text(serde_json::to_string(&r)?);
                // A corrupt recipe.json used to fall through to THIS lossy
                // XMP answer SILENTLY — the authoritative save (bitmap
                // masks, engine-only fields) deserves a disclosure, or the
                // next explicit save quietly overwrites it with the
                // projection shown here. Detail to stderr (the path can be
                // non-ASCII; header values cannot).
                if let Some(err) = &parse_err {
                    eprintln!(
                        "⚠ recipe.json for {} is unreadable ({err}) — serving the XMP projection instead",
                        raw.display()
                    );
                    if let Some(h) = header(
                        "X-Recipe-Warning",
                        "the saved recipe.json is unreadable - showing the lossy XMP projection \
                         instead (bitmap masks and engine-only edits are missing); saving \
                         overwrites the unreadable file",
                    ) {
                        resp = resp.with_header(h);
                    }
                }
                if let Some(w) = xmp_warn.take() {
                    resp = resp.with_header(w);
                }
                return Ok(resp);
            }
        }
    }
    // The RAW's own embedded XMP packet — the develop Lightroom bakes INTO a
    // DNG — as the strictly LOWEST-priority source, the same rule and clear
    // gate as the GUI open path and the batch snapshot
    // (store::embedded_packet_for_restore; the surfaces must answer alike).
    // A store file that EXISTS — neutral or unreadable — bars it.
    match if store_answered || parse_err.is_some() {
        Ok(None)
    } else {
        crate::store::embedded_packet_for_restore(raw)
    } {
        Ok(Some(text)) => {
            let diag = crate::diag::photo(raw);
            let mut r = crate::xmp::xmp_to_recipe_with_diag(&text, &diag);
            if xmp_warn.is_none() {
                xmp_warn = recipe_warning_header(&text);
            }
            if !r.is_noop() {
                r.clamp();
                // Same stamp rule as the XMP branches above: the packet was
                // tuned in Lightroom over its own camera-profile base.
                r.base_curve = fresh_base_knots(raw);
                r.lens_profile = pipeline::fresh_lens_profile(raw);
                if r.as_shot_k.is_none() {
                    let (ask, ast) = pipeline::fresh_as_shot_wb(raw);
                    r.as_shot_k = ask;
                    r.as_shot_tint = ast;
                }
                let h = Header::from_bytes(&b"X-Recipe-Source"[..], &b"embedded-xmp"[..])
                    .expect("static ASCII header");
                let mut resp = json_text(serde_json::to_string(&r)?).with_header(h);
                if let Some(err) = &parse_err {
                    eprintln!(
                        "⚠ recipe.json for {} is unreadable ({err}) — serving the RAW's embedded XMP instead",
                        raw.display()
                    );
                    if let Some(h) = header(
                        "X-Recipe-Warning",
                        "the saved recipe.json is unreadable - showing the RAW's embedded \
                         XMP develop instead; saving overwrites the unreadable file",
                    ) {
                        resp = resp.with_header(h);
                    }
                }
                if let Some(w) = xmp_warn.take() {
                    resp = resp.with_header(w);
                }
                return Ok(resp);
            }
        }
        Ok(None) => {}
        // Unreadable ≠ absent: nothing else answered, so whatever the packet
        // holds WOULD have been shown — disclosed on the fresh-open answer.
        Err(why) => {
            if xmp_warn.is_none() {
                xmp_warn = header(
                    "X-Recipe-Warning",
                    "this RAW carries an embedded XMP develop that could not be read - \
                     it is NOT reflected",
                );
                eprintln!(
                    "⚠ embedded XMP develop in {} could not be read ({why}) — not reflected",
                    raw.display()
                );
            }
        }
    }
    // A damaged recipe.json with nothing else to fall back on is an ERROR, not
    // "no recipe yet" — say so, or the UI silently pretends the photo is unedited.
    if let Some(err) = parse_err {
        let mut resp = status_response(422, &format!("recipe.json is unreadable: {err}"));
        // Same rule on the terminal error shape (review R12-05).
        if let Some(w) = xmp_warn.take() {
            resp = resp.with_header(w);
        }
        return Ok(resp);
    }
    // Still 404 ("not analyzed yet" drives the client's fresh-open UI state),
    // but the body carries the photo's camera-matched base look so the fresh
    // web canvas starts as bright as a fresh GUI open instead of jumping
    // only after the first Analyze.
    let body = fresh_base_payload(raw);
    let ct = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .expect("static ASCII header");
    // no-store like every other /api answer — a CACHED "not analyzed yet"
    // 404 replayed after another surface saved would reopen a neutral
    // canvas whose next save clears the newer develop (Codex R11 #1).
    let mut resp = Response::from_string(body)
        .with_status_code(404)
        .with_header(ct)
        .with_header(no_store());
    if let Some(w) = xmp_warn {
        resp = resp.with_header(w);
    }
    Ok(resp.boxed())
    }
}

/// A6 disclosure header for XMP-derived recipes: numeric settings the import
/// could not read became silent neutrals — the client shows this beside the
/// SAVED verdict, because the next save overwrites the sidecar with those
/// neutrals. NOT ASCII by construction: the xmlns-conflict sentence carries
/// an em dash, which is why this goes through `header` (L04-7). `None` when
/// all parsed.
fn recipe_warning_header(xmp_text: &str) -> Option<Header> {
    let bad = crate::xmp::unparsable_crs_numbers(xmp_text);
    if bad.is_empty() {
        return None;
    }
    let msg = format!(
        "{} numeric XMP setting(s) unreadable ({}) - restored as neutral; saving overwrites \
         the sidecar with those neutrals",
        bad.len(),
        bad.join(", ")
    );
    header("X-Recipe-Warning", &msg)
}

/// The photo's FRESH camera-matched base-look knots, regardless of any saved
/// recipe — the web client's Reset needs them to mean "the fresh-open look"
/// (the GUI's `photo_knots` rule): a legacy save deliberately carries an
/// empty curve, and preserving that on Reset kept the photo dark. Cheap after
/// the first call — `fresh_base_knots` reuses the cached `develop_base`.
fn api_fresh_base(request: &Request, state: &AppState) -> Result<ResponseBox> {
    let raw = raw_for(request, state)?;
    Ok(json_text(fresh_base_payload(&raw)))
}

/// The fresh-open calibration payload, in ONE place: `/api/fresh-base` serves
/// it on demand and `/api/recipe`'s 404 ("not analyzed yet") carries it inline
/// so the first canvas is already calibrated. Two endpoints, one key set — a
/// key added to only one of them is a client that reads a photo's calibration
/// differently depending on which door it came through.
fn fresh_base_payload(raw: &Path) -> String {
    let (as_shot_k, as_shot_tint) = pipeline::fresh_as_shot_wb(raw);
    serde_json::json!({
        "base_curve": fresh_base_knots(raw),
        "lens_profile": pipeline::fresh_lens_profile(raw),
        "as_shot_k": as_shot_k,
        "as_shot_tint": as_shot_tint,
    })
    .to_string()
}

/// Style-library info for the UI's info box: is an index built, how many of the
/// user's edits it holds, and the scene "tags" it covers. Instant (just reads the
/// JSON; no per-photo decode).
fn api_style_info(state: &AppState) -> Result<ResponseBox> {
    let abs = |p: &str| {
        std::path::absolute(p).map(|x| x.display().to_string()).unwrap_or_else(|_| p.to_string())
    };
    // Style reference library status — read through the SHARED reader
    // (`style::index_info`), which owns the central-then-legacy walk and the
    // scene histogram for every surface. This handler used to be the only
    // place that logic existed, which is why the GUI shipped without it
    // (R23-2).
    let info = crate::style::index_info();
    let index_file = info.path.display().to_string();
    let style = match info.state {
        crate::style::StyleIndexState::Built { total, source_dir, scenes, .. } => {
            let scenes: Vec<_> =
                scenes.into_iter().map(|(t, n)| json!({ "tag": t, "n": n })).collect();
            json!({ "built": true, "total": total, "scenes": scenes,
                    "index_file": index_file, "source_dir": source_dir })
        }
        // Same two-key answer the web page has always parsed: `built:false`
        // (the reason is the GUI's status line, which has room for it).
        _ => json!({ "built": false }),
    };
    Ok(json_response(&json!({
        // Where the photos being browsed live (the "原图库"), where RENDERED
        // outputs land (the "成片库" = the delivery root, `./out` unless the
        // user moved it), where the develop store keeps recipes / XMP /
        // versions, and the style-library status. out_dir and store_dir are
        // DIFFERENT places — the XMP has lived in the store since the sidecars
        // moved out of ./out. Reported through `config::delivery_root` (M8)
        // rather than a second literal, so this page cannot name a folder the
        // renderer no longer writes to.
        "working_dir": state.dir_display(),
        "working_count": state.count(),
        "out_dir": abs(&crate::config::delivery_root().display().to_string()),
        "store_dir": crate::store::store_root().display().to_string(),
        "style": style,
    })))
}

#[derive(Deserialize)]
struct StyleBuildReq {
    /// Folder of the user's edited RAWs (each RAW with its Lightroom .xmp beside it).
    dir: String,
}

/// Build the style reference index from a folder of the user's RAW+.xmp pairs, so
/// non-CLI users can point the app at THEIR OWN library from the info panel. Writes
/// the central store's style-index.json (same as `autoshade style-index <dir>`).
/// Decodes every RAW, so it can take minutes on a large library.
fn api_style_build(request: &mut Request) -> Result<ResponseBox> {
    let req: StyleBuildReq = read_json(request)?;
    let cleaned = req.dir.trim().trim_matches('"').trim();
    let p = PathBuf::from(cleaned);
    if !p.is_dir() {
        return Ok(status_response(400, &format!("not a folder: {cleaned}")));
    }
    // The web surface has no `--embed` / `--describe` flag, so both switches
    // are what the environment says with the preference off — exactly what
    // `build` used to read for itself before they became values.
    let embed = crate::style::EmbeddingSwitch::resolve(None, false);
    let describe = crate::style::DescribeSwitch::resolve(None, false);
    let index = match crate::style::StyleIndex::build(&p, embed, describe) {
        Ok(ix) => ix,
        Err(e) => return Ok(status_response(500, &format!("build failed: {e}"))),
    };
    let total = index.exemplars.len();
    // An empty build is a FAILURE, not a success: `save` truncates the file in
    // place, so writing it would silently replace a good index (and every
    // surface's Style slider goes inert with nothing to say why). A folder
    // yields 0 whenever no RAW has its Lightroom .xmp SITTING BESIDE IT — and
    // AutoShade never puts one there: its own XMP projection lives in the
    // per-user develop store. Point this at the folder you edit in LIGHTROOM.
    // Refuse, and leave the previous index alone.
    if total == 0 {
        return Ok(status_response(
            400,
            &format!(
                "nothing indexed in {} — found no RAW with its .xmp sidecar beside it \
                 (AutoShade keeps its own .xmp in the develop store, never beside the \
                 RAW, so point this at the folder you edit in Lightroom). The existing \
                 style index was left untouched.",
                p.display()
            ),
        ));
    }
    if let Err(e) = index.save(&crate::store::style_index_path()) {
        return Ok(status_response(500, &format!("save index: {e}")));
    }
    Ok(json_response(
        &json!({ "ok": true, "total": total, "source_dir": p.display().to_string() }),
    ))
}

#[derive(Deserialize)]
struct AnalyzeReq {
    id: usize,
    /// Optional user direction woven into the AI prompt.
    #[serde(default)]
    guidance: Option<String>,
    /// Refine mode: the user's CURRENT edit to adjust instead of starting fresh.
    /// `None` (the default) = propose from the original.
    #[serde(default)]
    base: Option<EditRecipe>,
    /// 0..1 — how strongly to follow the user's historical style (the Style
    /// slider). `None` falls back to the configured default.
    #[serde(default)]
    style_strength: Option<f32>,
    /// 0..1 — how COMMITTED the grade should be (R23-3). A different axis from
    /// `style_strength`: 0.5 is the calibrated baseline, `None` = the shipped
    /// default (0.65), which is what an older client that never sends the field
    /// gets — the same answer the desktop app's own default gives.
    #[serde(default)]
    grade_strength: Option<f32>,
    // The web request has no Direction-adherence field yet; keeping this
    // explicit documents that web analyses use the shared Direct default
    // rather than silently inventing a second request shape.
    /// DEEP THINKING (R23-4, feedback #13): the proposer returns its structured
    /// working in the same response, its reasoning tier goes up one step, and
    /// the visual judge may converge over more than one round. Absent = `false`,
    /// which is the shape every older client sends and the only shape that
    /// leaves the request byte-identical to the previous release's.
    #[serde(default)]
    deep: Option<bool>,
    /// A box the user dragged on the image (normalized 0..1) to target a local
    /// edit; the direction is then applied to a mask over that region.
    #[serde(default)]
    region: Option<Region>,
    /// The recipe whose GEOMETRY (lens / straighten / crop) produced the
    /// preview the box was dragged on. Sent whenever a region is set — even on
    /// a fresh (non-refine) analyze — so the box can be mapped back into the
    /// original frame the analysis preview and recipe masks live in.
    #[serde(default)]
    view: Option<EditRecipe>,
}

#[derive(Deserialize)]
struct Region {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}
/// A recipe that arrives over HTTP is authored in the LIVE display frame —
/// the browser edits the oriented preview — so it is stamped
/// [`crate::recipe::COORD_ERA`] on the way in, whatever the body said. It is
/// stamped [`crate::recipe::SCHEMA_ERA`] for the same reason (R25 P8): the
/// page was served THIS build's control set, so the body's silence about a
/// key is this build's silence, not a v0.30 file's.
///
/// `EditRecipe::coord_era`'s serde default means "this JSON predates the
/// field", which is true of a FILE and false of a request body: the web UI
/// builds a neutral recipe client-side on Reset, and without this a save of
/// that recipe would land on disk claiming sensor-frame coordinates and the
/// next GUI open would turn a crop that was already the right way up.
/// Attached at the FIELD, not the handlers, so a new recipe-bearing endpoint
/// cannot forget it.
fn live_frame_recipe<'de, D>(d: D) -> std::result::Result<EditRecipe, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mut r = EditRecipe::deserialize(d)?;
    r.coord_era = crate::recipe::COORD_ERA;
    r.schema_era = crate::recipe::SCHEMA_ERA;
    Ok(r)
}

#[derive(Deserialize)]
struct DevelopReq {
    id: usize,
    #[serde(deserialize_with = "live_frame_recipe")]
    recipe: EditRecipe,
    /// Export/download only: run AI denoise first (ignored by live preview).
    #[serde(default)]
    denoise: bool,
    #[serde(default)]
    denoise_strength: Option<f32>,
    /// Export/download only: "tif" (16-bit master, default) or "jpg".
    #[serde(default)]
    format: Option<String>,
    /// Browser-session master (see `session_master`): render from THIS
    /// ./out file instead of the persisted pixel source.
    #[serde(default)]
    master: Option<String>,
}
#[derive(Deserialize)]
struct XmpReq {
    id: usize,
    #[serde(deserialize_with = "live_frame_recipe")]
    recipe: EditRecipe,
    /// Browser-session master to RECORD as the develop's pixel source on
    /// save (the GUI rule) — else the healed pixels evaporate on reopen.
    #[serde(default)]
    master: Option<String>,
}
#[derive(Deserialize)]
struct RetouchReq {
    id: usize,
    /// What should fill the painted region (e.g. "remove the trash can").
    prompt: String,
    /// RGBA PNG mask as a data URL or bare base64 — transparent pixels = the
    /// region to regenerate (the brush-painted area in the UI).
    mask: String,
    /// Output quality tier (low|medium|high|auto). Falls back to the config default.
    #[serde(default)]
    quality: Option<String>,
    /// Composite onto the full-sensor develop (61 MP) instead of the embedded
    /// preview. Slow; the regenerated patch is upscaled. RAW only.
    #[serde(default)]
    full_res: bool,
    /// The canvas recipe the mask was painted OVER (straighten / lens
    /// geometry): the server un-warps the mask into the source frame with it.
    /// Absent = no mapping (older clients / no geometry).
    #[serde(default)]
    view: Option<EditRecipe>,
    /// Browser-session master (see `session_master`): fill on top of THIS
    /// ./out file — a chained retouch — instead of the persisted source.
    #[serde(default)]
    master: Option<String>,
}

/// A browser-carried ./out master path for session chaining: the previous
/// fill/heal answer's X-Output-Path, posted back so the NEXT operation (or
/// the preview / an export) builds on it — without this a second fill/heal
/// restarted from the persisted master and silently discarded the first
/// (U30). UNTRUSTED input: accepted ONLY when THIS server process issued it
/// for THIS photo — every fill/heal output is registered at creation
/// (`issue_session_master`). Registry membership replaces the old
/// canonicalise-into-./out + stem-prefix checks and closes their gap: a
/// same-stem master of a DIFFERENT photo (same file name, different source
/// folder — the ./out namespace is global) passed the stem test and grafted
/// one photo's frame onto another's develop. The registered flag carries
/// whether the chain's ROOT source was a GENERATED master (the look baked
/// into its pixels): consumers must strip base curve / lens profile / the
/// as-shot anchor exactly as `render_source_checked` does, and Save must
/// record that provenance (it used to hardcode inplace). The registry dies
/// with the process; a browser holding masters from an earlier run is told
/// to reselect — never a silent fallback.
fn session_master(
    claim: Option<&str>,
    raw: &Path,
) -> std::result::Result<Option<(PathBuf, bool)>, String> {
    let Some(c) = claim else { return Ok(None) };
    if c.trim().is_empty() {
        return Ok(None);
    }
    let canon = std::fs::canonicalize(c)
        .map_err(|e| format!("session master {c} is not readable ({e}) — reselect the photo and retry"))?;
    let issued = issued_masters().lock().unwrap_or_else(|p| p.into_inner());
    match issued.get(&crate::store::photo_key(raw)).and_then(|m| m.get(&canon)) {
        Some(&generated) => Ok(Some((canon, generated))),
        None => Err(format!(
            "session master {c} was not issued for this photo by this server run — reselect the photo and retry"
        )),
    }
}

/// The issuance registry behind [`session_master`]: photo key → canonical
/// master path → root-was-generated flag.
fn issued_masters()
-> &'static Mutex<std::collections::HashMap<String, std::collections::HashMap<PathBuf, bool>>> {
    static ISSUED: OnceLock<
        Mutex<std::collections::HashMap<String, std::collections::HashMap<PathBuf, bool>>>,
    > = OnceLock::new();
    ISSUED.get_or_init(Default::default)
}

/// Register a master this process just wrote for `raw`; `generated` = the
/// chain's root source carried the look (see [`session_master`]).
fn issue_session_master(raw: &Path, out: &Path, generated: bool) {
    // Canonical form, because session_master compares canonical forms. A
    // failure (the file we just wrote vanishing) only means the claim is
    // refused later — with the reselect message, never a silent wrong master.
    let Ok(canon) = std::fs::canonicalize(out) else { return };
    let mut map = issued_masters().lock().unwrap_or_else(|p| p.into_inner());
    map.entry(crate::store::photo_key(raw)).or_default().insert(canon, generated);
}

/// Map a dragged region from the DISPLAYED After frame back into the ORIGINAL
/// frame via the engine's shared view→original map per corner; the bounding
/// box of the mapped corners is the honest axis-aligned target (same policy
/// as the GUI's radial display map). The web preview renders lens geometry +
/// straighten but deliberately NOT the crop (api_develop: whole-frame slider
/// feedback, the GUI's policy) — the old un-crop step here assumed a cropped
/// view and shifted every box whenever a crop was active. Identity when the
/// viewing recipe has no geometry; if the source dims can't be read the box
/// passes through unmapped rather than failing the analyze.
fn region_to_original(
    g: &Region,
    view: Option<&EditRecipe>,
    raw: &Path,
) -> (f32, f32, f32, f32) {
    let unmapped = (g.left, g.top, g.right, g.bottom);
    let Some(rec) = view else { return unmapped };
    if !view_geometry_active(rec) {
        return unmapped;
    }
    let Some(dims) = source_dims(raw) else { return unmapped };
    // Densely sampled edges (9 points per side): lens distortion is
    // nonlinear, and an ASYMMETRIC box's mapped extremum can sit between a
    // corner and the midpoint — corners+midpoints alone still clipped part
    // of the selection. Still a sampled bound (documented approximation),
    // but the residual is sub-percent at ±100 manual distortion.
    let mut corners: Vec<(f32, f32)> = Vec::with_capacity(36);
    const K: u32 = 8;
    for i in 0..=K {
        let f = i as f32 / K as f32;
        let x = g.left + f * (g.right - g.left);
        let y = g.top + f * (g.bottom - g.top);
        corners.push((x, g.top));
        corners.push((x, g.bottom));
        corners.push((g.left, y));
        corners.push((g.right, y));
    }
    let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    // One composed profile for the whole sweep (manual CA folded in, R25 B3).
    let geom = render::geometry_profile(rec);
    for (vx, vy) in corners {
        let (ox, oy) = render::view_to_original_norm(
            vx,
            vy,
            dims,
            rec.straighten_deg,
            &geom,
            rec.lens_distortion,
        );
        l = l.min(ox);
        t = t.min(oy);
        r = r.max(ox);
        b = b.max(oy);
    }
    (l, t, r, b)
}

/// The viewing recipe used for REGION/MASK inverse mapping. For a photo whose
/// persisted master is GENERATED, the render path strips lens_profile (the
/// look lives in the pixels) — so the DISPLAYED frame never had the profile
/// geometry applied, and mapping a box/mask through it would land off target.
/// Mirrors render_source_checked's strip on the mapping side.
fn view_for_mapping(raw: &Path, view: Option<&EditRecipe>) -> Option<EditRecipe> {
    let mut v = view?.clone();
    if crate::store::read_pixel_source(raw).is_some_and(|(_, generated)| generated) {
        v.lens_profile = Default::default();
    }
    Some(v)
}

/// Does this viewing recipe transform the displayed frame (straighten / lens
/// geometry)? Crop deliberately excluded — the web preview never crops.
fn view_geometry_active(rec: &EditRecipe) -> bool {
    // geometry_moves_frame, not a local distortion check (Codex AL F1):
    // a CA-only profile with knots above 1 zooms the shared frame by the
    // composite fill (L04-2), so a browser-painted mask unwarped as
    // identity would land on the wrong pixels.
    // The COMPOSED profile (R25 B3): manual CA rides the same per-channel
    // knots, so a manual value that pushes a channel past 1 zooms the frame
    // exactly like a profile knot does.
    rec.straighten_deg != 0.0
        || render::geometry_moves_frame(&render::geometry_profile(rec), rec.lens_distortion)
}

/// Un-warp a browser-painted mask from the DISPLAYED (post-geometry) frame
/// into the SOURCE frame: out(p) = in(original_to_view_norm(p)), so the
/// painted area lands on the pixels the user actually saw. The retouch
/// engine consumes normalized mask coordinates, so the raster keeps its own
/// resolution. Returns None (use the mask as-is) when the view carries no
/// geometry or anything fails — the pre-fix behaviour, never an error.
fn unwarp_mask(bytes: &[u8], view: Option<&EditRecipe>, raw: &Path) -> Option<Vec<u8>> {
    let rec = view?;
    if !view_geometry_active(rec) {
        return None;
    }
    let dims = source_dims(raw)?;
    let img = render::mask_from_memory_bounded(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let mut out = image::RgbaImage::new(w, h);
    let geom = render::geometry_profile(rec);
    for y in 0..h {
        for x in 0..w {
            let (vx, vy) = render::original_to_view_norm(
                (x as f32 + 0.5) / w as f32,
                (y as f32 + 0.5) / h as f32,
                dims,
                rec.straighten_deg,
                &geom,
                rec.lens_distortion,
            );
            // Outside the displayed frame = the user could not paint there =
            // unpainted (opaque, "keep"). Nearest-neighbour is enough — the
            // consumers threshold alpha.
            let px = if (0.0..1.0).contains(&vx) && (0.0..1.0).contains(&vy) {
                *img.get_pixel(
                    ((vx * w as f32) as u32).min(w - 1),
                    ((vy * h as f32) as u32).min(h - 1),
                )
            } else {
                image::Rgba([0, 0, 0, 255])
            };
            out.put_pixel(x, y, px);
        }
    }
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgba8(out)
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .ok()?;
    Some(buf)
}

/// Dims of the oriented source frame — only the ASPECT matters to the
/// normalised geometry maps (they are homogeneous in dims), so preview-scale
/// dims are exact. RAW: the cached neutral develop BOTH panes serve
/// (`develop_base` — one demosaic per photo, already paid the moment the
/// photo was selected). The embedded preview this used to decode was not
/// only a repeated full JPEG decode per mapping call — its aspect can
/// DIFFER from the develop output (a small-preview camera's 1616×1080
/// ≈ 1.496 vs the sensor's 3:2), and the maps must consume exactly the
/// frame the user pointed at. Baked sources: a header-only dimension read.
fn source_dims(raw: &Path) -> Option<(f32, f32)> {
    if decode::is_raw(raw) {
        let img = develop_base(raw).ok()?;
        Some((img.width() as f32, img.height() as f32))
    } else {
        // ORIENTED dims: load_image applies the EXIF orientation, so the
        // canvas is portrait for an orientation-6 JPEG while the raw header
        // reports landscape — the geometry maps then used the wrong aspect.
        // Header-only read (into_decoder decodes no pixels).
        use image::ImageDecoder as _;
        let mut dec = image::ImageReader::open(raw)
            .ok()?
            .with_guessed_format()
            .ok()?
            .into_decoder()
            .ok()?;
        let (w, h) = dec.dimensions();
        let o = dec
            .orientation()
            .unwrap_or(image::metadata::Orientation::NoTransforms);
        let swap = matches!(
            o,
            image::metadata::Orientation::Rotate90
                | image::metadata::Orientation::Rotate270
                | image::metadata::Orientation::Rotate90FlipH
                | image::metadata::Orientation::Rotate270FlipH
        );
        Some(if swap { (h as f32, w as f32) } else { (w as f32, h as f32) })
    }
}

fn api_analyze(request: &mut Request, state: &AppState) -> Result<ResponseBox> {
    let stamp = request_gen(request);
    let mut req: AnalyzeReq = read_json(request)?;
    // Untrusted network input — the same clamp the DevelopReq/XmpReq bodies
    // get (both nested recipes feed view_for_mapping / the AI proposal, and
    // an unclamped million-knot lens vector would occupy the handler).
    if let Some(b) = req.base.as_mut() {
        b.clamp();
    }
    if let Some(v) = req.view.as_mut() {
        v.clamp();
    }
    let raw = match state.at_checked(req.id, stamp) {
        Ok(r) => r,
        Err(resp) => return Ok(resp),
    };
    // A dragged region anchors the edit: fold its coords into the direction so the
    // AI places a mask over exactly that box (reuses the Phase-2 area→mask prompt).
    // The box was dragged on the DISPLAYED preview (post lens/straighten/crop);
    // the AI proposes over the embedded preview and masks live in the ORIGINAL
    // frame — map the corners back through the inverse geometry first, or any
    // active geometry offsets the mask from what the user boxed.
    let region_guidance = req.region.as_ref().map(|g| {
        let view = req.view.as_ref().or(req.base.as_ref());
        let mapped_view = view_for_mapping(&raw, view);
        // Dims of the DISPLAYED source: with a saved master the canvas shows
        // the master (possibly a different aspect than the RAW) — mapping
        // through the RAW's dims displaced every box/mask.
        let display_src = crate::store::read_pixel_source(&raw)
            .map(|(m, _)| m)
            .unwrap_or_else(|| raw.clone());
        let (l, t, r, b) = region_to_original(g, mapped_view.as_ref(), &display_src);
        format!(
            "The user SELECTED a target region (normalized 0..1 frame coords): left={l:.3} top={t:.3} \
             right={r:.3} bottom={b:.3}. Apply the direction ONLY to that region — emit a mask covering \
             it (a radial mask with those exact left/top/right/bottom bounds and feather ~0.4 is \
             ideal, or a linear gradient for a thin edge band). Direction: {}",
            req.guidance.as_deref().unwrap_or("make a tasteful local improvement"),
        )
    });
    let guidance = region_guidance.as_deref().or(req.guidance.as_deref());
    // base = Some → refine the current edit; None → fresh proposal from
    // original. SNAPSHOT the config — the read guard must not sit across the
    // whole AI chain blocking every settings save.
    let cfg = state.config().clone();
    let style = req.style_strength.unwrap_or(cfg.style_strength);
    // Untrusted network number: `GradeStrength::new` clamps to 0..1 and turns a
    // NaN into the default rather than into "maximum restraint" (the same door
    // every other body field goes through).
    let grade = crate::recipe::GradeStrength::from_optional(req.grade_strength);
    // produce_recipe itself strips the base look + lens profile from the
    // PROMPT copy (they are already IN the embedded preview's pixels) and
    // needs the UNSTRIPPED base so carry_over_unrepresentable can keep the
    // user's unsaved lens toggles — pre-stripping here made every web Refine
    // revert them to the saved profile.
    let refine_base = req.base.clone();
    // judge = true: the web Analyze is the same interactive single-photo
    // surface as the GUI button — the visual closed loop is part of it
    // (batch surfaces pass false; the rationale disclosed in the response
    // carries the judge's score and any revision).
    // Text reference only: the opt-in reference PHOTO is a GUI switch (R23-2),
    // and a browser session must not start spending an extra image per call
    // through a field nothing in the page can see.
    let (recipe, verdict, _notes) = pipeline::produce_recipe(
        &raw,
        &cfg,
        false,
        guidance,
        refine_base.as_ref(),
        // `deep` is opt-in per request and absent by default (R23-4): a browser
        // session must not start paying for a thinking envelope, a raised
        // reasoning tier and up to three judge rounds through a field it never
        // set — the same rule as the reference photo above.
        pipeline::GradeRequest {
            style,
            send_reference_image: false,
            strength: grade,
            think: req.deep.unwrap_or(false),
            adherence: crate::recipe::DirectionAdherence::default(),
            use_looks: true,
            // `..default()` for the two resolved-once fields: the browser has
            // no way to set them, and spelling them out here would be a second
            // place that has to agree with `GradeRequest::default`.
            ..Default::default()
        },
        true,
        // The shipped channel (R29-1). The web surface is one request, one
        // photo, on its own thread: there is no pooled transcript to order
        // these into, and the browser's own disclosure is the `warning` field
        // this handler already builds below.
        crate::diag::stderr(),
    )?;
    // A non-Accept verdict may not auto-save (user decision): the verifier
    // itself judged the result not ready, so the develop on disk stays
    // untouched and the browser gets the proposal back as an UNSAVED edit —
    // its saved-baseline machinery shows it as such and asks before
    // discarding.
    if verdict.decision != crate::advisor::Decision::Accept {
        return Ok(json_response(&json!({
            "recipe": recipe,
            "verdict": verdict,
            "saved": false,
            "warning": format!(
                "verdict {:?} — not saved (a non-Accept verdict never auto-saves); \
                 Save XMP keeps it, switching photos discards it",
                verdict.decision
            ),
        })));
    }
    // Analyze is a PROGRAMMATIC writer: it may not destroy an explicit save
    // without a `v<N>` snapshot — the same contract the GUI enforces. A backup
    // that FAILS (locked / unreadable existing save) means we must not write at
    // all: handing back an unsaved proposal beats overwriting a save we could
    // not protect.
    crate::store::with_develop_lock(
        &raw,
        crate::store::DevelopLockMode::Wait,
        || match crate::store::backup_saved_develop(&raw, Some(&recipe)) {
        Ok(backed_up) => {
            pipeline::write_recipe(&raw, &recipe, None, crate::diag::stderr())?;
            // The recipe write ALONE decides the saved state (the GUI/CLI
            // rule): a failed XMP projection degrades to a warning — the old
            // `?` answered 500 and hid the committed save from the browser.
            let mut body = json!({ "recipe": recipe, "verdict": verdict, "saved": true });
            // The NEW revision rides the answer: the tab adopts it so its
            // own next Save does not 412 against this very analyze. (The
            // not-saved arms change nothing on disk — the tab's held tag
            // stays valid, so they carry none.) Analyze itself takes NO
            // If-Match, deliberately: its overwrite is version-protected —
            // backup_saved_develop snapshots the existing save to v<N> and
            // the reply discloses it — where the explicit Save path had no
            // net at all. A 412 after a paid AI call would protect less
            // than the snapshot already does.
            body["revision"] = json!(crate::store::develop_revision(&raw));
            if decode::is_raw(&raw) {
                match pipeline::write_xmp(&raw, &recipe, crate::diag::stderr()) {
                    // Two independent disclosures share the one `warning`
                    // field (the existing shape — no new API): the merge note
                    // and, since M6a, what the mask PROJECTION itself could
                    // not carry. Joined, never overwritten: whichever one wrote
                    // first used to be the only one the tab ever saw.
                    Ok((_, merge_note, losses)) => {
                        let mut warns: Vec<String> = Vec::new();
                        warns.extend(merge_note);
                        warns.extend(crate::xmp::describe_mask_losses(&losses));
                        if !warns.is_empty() {
                            body["warning"] = json!(warns.join("; "));
                        }
                    }
                    Err(e) => {
                        body["warning"] = json!(format!(
                            "saved, but the Lightroom XMP projection failed: {e:#}"
                        ));
                    }
                }
            }
            if let Some(n) = backed_up {
                body["backed_up"] = json!(n);
            }
            Ok(json_response(&body))
        }
        Err(e) => Ok(json_response(&json!({
            "recipe": recipe,
            "verdict": verdict,
            "saved": false,
            "warning": format!(
                "not saved: backing up your existing save failed ({e}); \
                 save explicitly to overwrite"
            ),
        }))),
    },
    )
}

// The photo's RENDER INPUT is store::render_source_checked everywhere here:
// the persisted pixels.json master when the store records one (a GUI-saved
// heal/denoise/reimagine — the web used to silently render the un-retouched
// source while the GUI showed the master), else the photo itself. A GENERATED
// master carries the look in its pixels, so the recipe drops base_curve +
// lens_profile + the as-shot anchor (the strip rule every GUI render path
// applies). A recorded-but-unhonourable master REFUSES on deliverables and
// degrades-with-warning on the preview (A6).

fn api_develop(request: &mut Request, state: &AppState) -> Result<ResponseBox> {
    let stamp = request_gen(request);
    let mut req: DevelopReq = read_json(request)?;
    let raw = match state.at_checked(req.id, stamp) {
        Ok(r) => r,
        Err(resp) => return Ok(resp),
    };
    // Store recipes reference rasters by bare name (api_recipe serves them
    // verbatim) — anchor them to the photo's develop dir before rendering.
    // UNTRUSTED input, like any hand-edited recipe: clamp before it reaches
    // the engine (the CLI's apply and the GUI both do this; the web path
    // did not, so `exposure_ev: 1e30` went straight into powf and a
    // thousand-mask body monopolised the render thread).
    req.recipe.clamp();
    crate::store::resolve_mask_paths(&mut req.recipe, &crate::store::develop_dir(&raw));
    // Same decode source as `api_export` below — see `develop_base`. A
    // browser-session master (fill/heal chained this session, not yet
    // persisted) overrides the persisted source; whether the calibration
    // renders on top depends on the chain's ROOT: rooted at the photo or an
    // inplace heal = a neutral develop (no strip); rooted at a GENERATED
    // master = the look lives in the pixels (strip, exactly like
    // render_source_checked). The issuance registry carries that provenance.
    // A recorded master that cannot be honoured must not fail the PREVIEW —
    // the user needs a canvas to act on — but silence was the A6 defect: the
    // degradation rides back as a header the status line surfaces.
    let mut preview_warning: Option<String> = None;
    let mut recipe_note: Option<String> = None;
    let mut funnel_ran = false;
    let src = match session_master(req.master.as_deref(), &raw) {
        Ok(Some((p, generated))) => {
            if generated {
                req.recipe.base_curve = Vec::new();
                req.recipe.lens_profile = Default::default();
                req.recipe.as_shot_k = None;
                req.recipe.as_shot_tint = None;
            }
            p
        }
        Ok(None) => match crate::store::render_source_checked(&raw, &mut req.recipe) {
            Ok((p, note)) => {
                recipe_note = note;
                funnel_ran = true;
                p
            }
            Err(msg) => {
                preview_warning = Some(msg);
                raw.clone()
            }
        },
        Err(msg) => return Ok(status_response(400, &msg)),
    };
    // ONE repair for the arms the funnel did not cover — a SESSION master
    // (non-generated pixels render the curve on top) and the degraded Err
    // fallback above (which renders the RAW). Keyed on whether the funnel
    // RAN, never on the note: a None note also means "the funnel tried and
    // the estimate was an inability" (uncached by design), and re-running
    // the identical call paid the failed decode + develop twice per request.
    // The note rides X-Recipe-Warning below, and the browser DOES read it on
    // render fetches: a select-time inability makes api_recipe serve the
    // washed recipe verbatim with no warning, and the retry that succeeds
    // happens here — "for a browser it is None" was false in exactly that
    // designed-for state.
    if !funnel_ran {
        recipe_note = crate::pipeline::repair_pre_era_base_curve(&raw, &mut req.recipe);
    }
    let preview = develop_base(&src)?;
    let mut after = render::develop_preview(&preview, &req.recipe);
    // Geometry, mirroring the GUI preview chain (lens geometry → straighten;
    // the frame stays uncropped for whole-frame slider feedback, same policy).
    // The web pane previously skipped ALL of it — a recipe with distortion or
    // straighten previewed one framing and exported another.
    let geom = render::geometry_profile(&req.recipe);
    if geom.geometry_active() || req.recipe.lens_distortion != 0.0 {
        after = render::apply_lens_geometry(&after, &geom, req.recipe.lens_distortion);
    }
    if req.recipe.straighten_deg != 0.0 {
        after = render::rotate_straighten(&after, req.recipe.straighten_deg);
    }
    let mut resp = jpeg_response(&after)?;
    if let Some(h) = preview_warning
        .and_then(|m| header("X-Preview-Warning", &m))
    {
        resp = resp.with_header(h);
    }
    if let Some(h) = recipe_note
        // The remedy rides with the fact (ASCII — it travels in a header):
        // the recipe the client HOLDS still carries the washed curve until a
        // reselect refreshes it, or a save repairs it on the way to disk.
        .map(|m| format!("{m} - reselect the photo to refresh the loaded recipe"))
        .and_then(|m| header("X-Recipe-Warning", &m))
    {
        resp = resp.with_header(h);
    }
    Ok(resp)
}

/// Resolve the output extension from the request ("jpg" → jpg, else 16-bit tif).
fn fmt_ext(req: &DevelopReq) -> &'static str {
    match req.format.as_deref() {
        Some("jpg") | Some("jpeg") => "jpg",
        _ => "tif",
    }
}

fn denoise_opts(req: &DevelopReq, cfg: &Config) -> Option<DenoiseOpts> {
    req.denoise
        .then(|| DenoiseOpts::from_config(cfg, None, req.denoise_strength.unwrap_or(1.0)))
}

fn export_slot_path(
    out_dir: &Path,
    stem: &str,
    suffix: u32,
    kind: &str,
    ext: &str,
) -> PathBuf {
    let named_stem =
        if suffix == 1 { stem.to_string() } else { format!("{stem} ({suffix})") };
    out_dir.join(format!("{named_stem}.{kind}.{ext}"))
}

/// The registry directory's name, kept at its PRE-RENAME spelling.
///
/// This is an on-disk identity namespace, not a label: it is what keeps one
/// photo exporting to the same deliverable filename run after run, and the
/// claim below is the only thing that stops two server processes assigning one
/// suffix to two photos. Renaming the directory would abandon every existing
/// claim and start reassigning suffixes from 1 -- "a re-key would silently
/// change deliverable filenames", which is the exact harm this registry
/// exists to prevent. Same call as the installer's AppId. Moving it wants a
/// migration of its own, not a rename.
const EXPORT_REGISTRY_DIR: &str = ".autoshop-export-registry";

/// Assign one persistent output slot to this lexical `photo_key`. Registry
/// entries are atomically claimed, so parallel server processes cannot assign
/// the same suffix to different photos.
fn registered_export_out(
    raw: &Path,
    kind: &str,
    ext: &str,
    out_dir: &Path,
) -> Result<PathBuf> {
    let stem = pipeline::stem(raw);
    let registry_stem =
        if cfg!(windows) { stem.to_ascii_lowercase() } else { stem.to_string() };
    let group = out_dir
        .join(EXPORT_REGISTRY_DIR)
        .join(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(registry_stem.as_bytes()));
    std::fs::create_dir_all(&group)
        .with_context(|| format!("create export registry {}", group.display()))?;

    // The store's identity key (canonical since the C1/F10 rework), with
    // the pre-rework LEXICAL key accepted on READ: slots claimed by an
    // older build must keep matching their photo, or a re-key would
    // silently change deliverable filenames. The wider match means "the
    // same photo under two spellings", never "any matching key" — and the
    // owner file is NOT rewritten on a legacy match (rewriting is a
    // clobbering write into a create_new-claimed namespace; accepting both
    // spellings IS identity). The absolute source line is only a
    // human-readable breadcrumb analogous to the develop store's
    // source.txt; it is never canonicalized or compared.
    let photo_key = crate::store::photo_key(raw);
    let legacy_key = crate::store::photo_key_lexical(raw);
    let source = std::path::absolute(raw).unwrap_or_else(|_| raw.to_path_buf());
    let owner_record = format!("{photo_key}\n{}\n", source.display());

    for suffix in 1..=999u32 {
        let entry = group.join(format!("{suffix}.owner"));
        loop {
            match crate::store::read_text_capped(&entry, crate::store::MAX_STORE_JSON) {
                Ok(text) => {
                    if matches!(text.lines().next(), Some(k) if k == photo_key || k == legacy_key)
                    {
                        return Ok(export_slot_path(out_dir, stem, suffix, kind, ext));
                    }
                    // Another photo, an unregistered legacy artifact, or a
                    // malformed-but-readable claim owns this slot. Never
                    // reinterpret it from request order after a restart.
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let out = export_slot_path(out_dir, stem, suffix, kind, ext);
                    // One suffix owns both formats. Otherwise a pre-registry
                    // TIFF could be preserved today but overwritten when the
                    // same assigned photo later switched from JPEG to TIFF.
                    let disk_occupied = ["jpg", "tif"].into_iter().any(|candidate_ext| {
                        export_slot_path(out_dir, stem, suffix, kind, candidate_ext).exists()
                    });
                    let claim =
                        if disk_occupied { "unclaimed\n".to_string() } else { owner_record.clone() };

                    use std::io::Write as _;
                    match std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&entry)
                    {
                        Ok(mut file) => {
                            if let Err(write_err) =
                                file.write_all(claim.as_bytes()).and_then(|()| file.sync_all())
                            {
                                drop(file);
                                let _ = std::fs::remove_file(&entry);
                                return Err(write_err).with_context(|| {
                                    format!("write export owner {}", entry.display())
                                });
                            }
                            if disk_occupied {
                                // An artifact with no ownership record cannot
                                // safely be attributed to whichever photo
                                // happened to export first after the upgrade.
                                break;
                            }
                            return Ok(out);
                        }
                        Err(claim_err)
                            if claim_err.kind() == std::io::ErrorKind::AlreadyExists =>
                        {
                            // A parallel process decided this slot; reread its
                            // completed or conservative partial claim.
                            continue;
                        }
                        Err(claim_err) => {
                            return Err(claim_err).with_context(|| {
                                format!("claim export owner {}", entry.display())
                            });
                        }
                    }
                }
                Err(e) => {
                    // Guessing past an unreadable ownership record could make
                    // the same photo change suffix after a transient I/O error.
                    return Err(e)
                        .with_context(|| format!("read export owner {}", entry.display()));
                }
            }
        }
    }
    anyhow::bail!("no free export name for {stem} (999 persistent slots in ./out)")
}

/// Resolve the pixel source for a DELIVERABLE (export / download) and repair
/// the recipe's base curve exactly once. `Err` carries the response to return.
///
/// A session master wins here as it does in the preview (`api_develop`'s
/// rule) — the deliverable must match the healed canvas the user just
/// approved — and a generated master strips the roots that describe the RAW
/// rather than those pixels. Unlike the preview, a broken master link
/// REFUSES instead of silently delivering the un-retouched source (A6):
/// server-side state, so a 500 whose body names the remedy, while an unissued
/// or forged claim is the client's fault and gets a 400.
///
/// The repair is keyed on whether the funnel RAN, never on its note — a None
/// note also means "tried, inability", and re-running the identical call paid
/// the failed decode twice per request while holding the HEAVY lock.
fn deliverable_source(
    req: &mut DevelopReq,
    raw: &Path,
) -> std::result::Result<(PathBuf, Option<String>), ResponseBox> {
    let mut note: Option<String> = None;
    let mut funnel_ran = false;
    let src = match session_master(req.master.as_deref(), raw) {
        Ok(Some((p, generated))) => {
            if generated {
                req.recipe.base_curve = Vec::new();
                req.recipe.lens_profile = Default::default();
                req.recipe.as_shot_k = None;
                req.recipe.as_shot_tint = None;
            }
            p
        }
        Ok(None) => match crate::store::render_source_checked(raw, &mut req.recipe) {
            Ok((p, n)) => {
                note = n;
                funnel_ran = true;
                p
            }
            Err(msg) => return Err(status_response(500, &msg)),
        },
        Err(msg) => return Err(status_response(400, &msg)),
    };
    if !funnel_ran {
        note = crate::pipeline::repair_pre_era_base_curve(raw, &mut req.recipe);
    }
    Ok((src, note))
}

/// Export to ./out (the library stays read-only). Returns the written path.
fn api_export(request: &mut Request, state: &AppState) -> Result<ResponseBox> {
    // Full-resolution work — see HEAVY. Held for the whole handler.
    let _heavy = HEAVY.lock().unwrap_or_else(|p| p.into_inner());

    let stamp = request_gen(request);
    let mut req: DevelopReq = read_json(request)?;
    let raw = match state.at_checked(req.id, stamp) {
        Ok(r) => r,
        Err(resp) => return Ok(resp),
    };
    req.recipe.clamp(); // untrusted network input — see api_develop
    crate::store::resolve_mask_paths(&mut req.recipe, &crate::store::develop_dir(&raw));
    // The client reads X-Recipe-Warning on this fetch too (see api_develop).
    let (src, recipe_note) = match deliverable_source(&mut req, &raw) {
        Ok(v) => v,
        Err(resp) => return Ok(resp),
    };
    // Re-exporting one photo replaces its own persistent slot, while a
    // different same-stem photo keeps the lowest separately-owned suffix.
    // Claims land before rendering, so failure cannot let a later run assign
    // this photo's name to another source.
    // The delivery root (M8) — the same folder the CLI and the GUI's
    // "Delivery folder" destination write to; `./out` until the user moves it.
    let out = registered_export_out(
        &raw,
        "developed",
        fmt_ext(&req),
        &crate::config::delivery_root(),
    )?;
    // Config SNAPSHOT: the read guard held across a multi-minute render
    // blocked every settings save (same rule as api_retouch/api_heal).
    let cfg = state.config().clone();
    // Render into a unique sibling, then rename: two concurrent exports of
    // the same photo/format used to interleave encoders into ONE fixed path.
    // The tmp keeps the real ".jpg"/".tif" suffix — the encoder picks its
    // format from the extension.
    let ext = fmt_ext(&req);
    let tmp = out.with_file_name(format!(
        "{}.developed.{}-{}.{ext}",
        pipeline::stem(&raw),
        std::process::id(),
        DL_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(e) = render::render_to_file(
        &src,
        &req.recipe,
        &tmp,
        denoise_opts(&req, &cfg).as_ref(),
        None,
        // The shipped channel (R29-1): one request, one photo, one thread.
        crate::diag::stderr(),
    ) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, &out) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("publish export {}", out.display()));
    }
    let mut resp = text_response(&out.display().to_string());
    if let Some(h) =
        recipe_note.and_then(|m| header("X-Recipe-Warning", &m))
    {
        resp = resp.with_header(h);
    }
    Ok(resp)
}

/// Render and stream the image back as a download (browser "Save As"), without
/// leaving a copy in ./out. Renders to a temp file, then streams + deletes it.
fn api_download(request: &mut Request, state: &AppState) -> Result<ResponseBox> {
    // Full-resolution work — see HEAVY. Held for the whole handler.
    let _heavy = HEAVY.lock().unwrap_or_else(|p| p.into_inner());

    let stamp = request_gen(request);
    let mut req: DevelopReq = read_json(request)?;
    let raw = match state.at_checked(req.id, stamp) {
        Ok(r) => r,
        Err(resp) => return Ok(resp),
    };
    req.recipe.clamp(); // untrusted network input — see api_develop
    crate::store::resolve_mask_paths(&mut req.recipe, &crate::store::develop_dir(&raw));
    let (src, recipe_note) = match deliverable_source(&mut req, &raw) {
        Ok(v) => v,
        Err(resp) => return Ok(resp),
    };
    // Config SNAPSHOT — see api_export.
    let cfg = state.config().clone();
    let ext = fmt_ext(&req);
    let tmp = std::env::temp_dir().join(format!(
        "autoshade_dl_{}_{}.{ext}",
        std::process::id(),
        DL_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    // A failed render must not leave a multi-hundred-MB partial file to sit
    // in %TEMP% until the next server start's sweep.
    if let Err(e) =
        render::render_to_file(
            &src,
            &req.recipe,
            &tmp,
            denoise_opts(&req, &cfg).as_ref(),
            None,
            // The shipped channel (R29-1): one request, one photo, one thread.
            crate::diag::stderr(),
        )
    {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // STREAM the file instead of fs::read-ing it whole: a 61 MP 16-bit TIFF
    // is ~366 MB — materialising it as a Vec on top of the renderer's own
    // buffers was the server's single biggest peak-memory hazard. The temp
    // file is removed after the response completes (best-effort on Windows,
    // where an open handle blocks deletion until the stream closes).
    let file = std::fs::File::open(&tmp).with_context(|| format!("open {}", tmp.display()))?;
    let len = file.metadata().ok().map(|m| m.len() as usize);
    let ctype = if ext == "jpg" { "image/jpeg" } else { "image/tiff" };
    // A header value is ASCII-only (tiny_http REJECTS non-ASCII bytes), so the
    // photo's own stem cannot go in raw: a Chinese/accented name used to make
    // `from_bytes(..).unwrap()` panic the request thread, and Download answered
    // with an empty body. RFC 5987: a fixed ASCII `filename=` every client
    // understands, plus `filename*=UTF-8''…` percent-encoded with the real name
    // (which every current browser prefers).
    let disposition = format!(
        "attachment; filename=\"download.developed.{ext}\"; filename*=UTF-8''{}",
        percent_encode(&format!("{}.developed.{ext}", pipeline::stem(&raw)))
    );
    let mut resp = Response::new(
        tiny_http::StatusCode(200),
        Vec::new(),
        file,
        len,
        None,
    );
    if let Some(h) = header("Content-Type", ctype) {
        resp = resp.with_header(h);
    }
    if let Some(h) = header("Content-Disposition", &disposition) {
        resp = resp.with_header(h);
    }
    if let Some(h) =
        recipe_note.and_then(|m| header("X-Recipe-Warning", &m))
    {
        resp = resp.with_header(h);
    }
    // Unlink while the stream handle is open: works on Unix, and on Windows
    // too (std opens files with FILE_SHARE_DELETE, and Win10+ deletes
    // POSIX-style — verified empirically on this exact pattern). The failure
    // tail — an AV scanner/indexer briefly holding the file without delete
    // sharing, or a crash between render and unlink — would leak forever
    // because Windows has NO automatic %TEMP% cleaner; those stragglers are
    // age-swept at the next server start (`sweep_stale_temp_files`).
    let _ = std::fs::remove_file(&tmp);
    Ok(resp.boxed())
}

static DL_SEQ: AtomicU64 = AtomicU64::new(0);

/// Best-effort sweep of leftover `autoshade_dl_*` / `autoshade_mask_*` temp
/// files from previous runs (crash before unlink, or an unlink refused by a
/// scanner's non-sharing handle). Age-gated at one hour so an in-flight
/// download owned by a parallel server instance is never touched.
fn sweep_stale_temp_files() {
    let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else { return };
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if !(name.starts_with("autoshade_dl_")
            || name.starts_with("autoshade_mask_")
            || name.starts_with("autoshade_heal_"))
        {
            continue;
        }
        let stale = e.metadata().and_then(|m| m.modified()).map(|t| t < cutoff).unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn api_xmp(request: &mut Request, state: &AppState) -> Result<ResponseBox> {
    let stamp = request_gen(request);
    // Read BEFORE read_json takes the body reader. The precondition itself
    // is evaluated inside the develop lock below, where the current
    // revision cannot move under it.
    let if_match = req_header(request, "If-Match");
    let mut req: XmpReq = read_json(request)?;
    // "Did the client ask to clear?" is decided on what the client SENT,
    // BEFORE clamping — because clamping can manufacture a neutral recipe, and
    // the neutral branch below DELETES the photo's saved edits.
    //
    // The previous comment here asserted the opposite ("clamping cannot flip
    // the is_noop branch — it only ever removes or bounds values a neutral
    // recipe does not have") and that is false. `EditRecipe` is
    // `#[serde(default)]`, so a partial body defaults every other field, and
    // clamp DROPS whole components rather than merely bounding them: a
    // degenerate crop becomes `None` (`recipe.rs`: under 1e-3 wide or tall), a
    // non-finite slider collapses to 0.0, a non-finite mask is retained away.
    // So `POST /api/xmp {"id":0,"recipe":{"crop":{"left":0.5,"top":0.5,
    // "right":0.5,"bottom":0.5}}}` clamped to exactly `EditRecipe::default()`,
    // took the clear branch, and answered "cleared — saved edits removed"
    // after unlinking recipe.json, the XMP and the legacy sidecar. The same
    // request took the SAVE path before the clamp was added, so this was a
    // destructive regression introduced with the fix above it.
    let client_asked_to_clear = req.recipe.is_noop();
    // Untrusted network input — and this is the route that PERSISTS it, so the
    // clamp matters more here than on the render routes that already had it.
    // `EditRecipe::clamp`'s size caps name "a hostile POST to the local web
    // server" as their reason, yet the write path skipped them: a 100 000-mask
    // body landed on disk as the photo's authoritative recipe.json, was
    // re-parsed by every later open, and was copied verbatim into the next
    // version snapshot.
    req.recipe.clamp();
    let raw = match state.at_checked(req.id, stamp) {
        Ok(r) => r,
        Err(resp) => return Ok(resp),
    };
    crate::store::with_develop_lock(
        &raw,
        crate::store::DevelopLockMode::Wait,
        || {
    // Recovery FIRST (Codex round-12 durability review, finding 1): the
    // If-Match gate below must compare against the CURRENT generation. A
    // crashed save that reached its COMMIT marker but not the live files
    // leaves recipe.json one generation behind — a stale tab's revision
    // would PASS the gate, and the writer's own commit head would roll the
    // newer committed generation forward only for this stale body to
    // overwrite it. `recipe_revision` deliberately reads without recovering,
    // so the resolution happens here, once, under the request's lock; an
    // unresolvable state refuses the request instead of guessing what
    // "current" means.
    if let Err(e) = crate::store::recover_orphan_baks(&raw) {
        return Ok(status_response(
            500,
            &format!("the saved develop needs a recovery that failed ({e}) - retry, or repair the store as the error describes"),
        ));
    }
    // The If-Match gate covers BOTH branches — the clear below deletes the
    // develop, which destroys a lost update just as surely as a write.
    if let Some(resp) = precondition_failed(
        if_match.as_deref(),
        crate::store::develop_revision(&raw).as_deref(),
    ) {
        return Ok(resp);
    }
    // Resolve the session master FIRST: a rejected master must fail the save
    // BEFORE the recipe commits, or the client baselines (markSaved) on a
    // 200 whose pixels never landed — and a later navigation discards the
    // only retouched canvas.
    let master = match session_master(req.master.as_deref(), &raw) {
        Ok(m) => m,
        Err(msg) => return Ok(status_response(400, &msg)),
    };
    // Reset-then-Save means "clear my edits", exactly as in the GUI's `save_xmp`:
    // writing a NEUTRAL pair would pin a permanent ● edited badge with no in-app
    // way to remove it (and the GUI then reports a no-op save on every open).
    // BOTH homes are cleared — the central store AND any legacy ./out sidecar,
    // which would otherwise resurrect the edits through the read fallbacks.
    // Version snapshots are kept.
    // `&& master.is_none()`: a baked pixel retouch IS an edit even under a
    // neutral recipe. Without it, "Fill (or heal) then Save" on a photo whose
    // sliders were never touched took the CLEAR path — the accepted master was
    // dropped, pixels.json was deleted, and the 200 said "cleared", after which
    // the browser baselined the canvas as saved and the retouch died silently
    // on the next thumbnail click. The GUI has carried exactly this guard since
    // it hit the same defect (its `active_variant().origin` clause).
    // AND the PERSISTED master: `session_master` resolves only THIS browser
    // session's heal chain, so a GUI-saved pixels.json was invisible here and
    // a neutral web Save deleted the saved retouch through clear_develop.
    // `has_pixel_source`, not `read_pixel_source` — a recorded master that
    // fails to LOAD right now must block the delete too. The GUI's own CLEAR
    // guard keys on the CANVAS instead (origin.is_none(): the user visibly
    // removed the baked pixels before saving neutral); the web has no canvas
    // state to consult, so the master's existence is the conservative proxy —
    // detaching a persisted retouch stays a GUI operation, and the
    // fall-through below DISCLOSES that the master was kept.
    if client_asked_to_clear && master.is_none() && !crate::store::has_pixel_source(&raw) {
        // ONE primitive for every surface (`store::clear_develop`). This branch
        // and the GUI's Ctrl+S each kept their own copy of the file list and
        // drifted twice: the marker landed only in the GUI, and BOTH unlinked
        // `pixels.json` directly — leaving the retired `pixels.json.bak` as
        // bait for the next open's `recover_orphan_baks`, which handed the user
        // back the very retouch this call reported as cleared.
        return match crate::store::clear_develop(&raw) {
            Ok(outcome) => {
                // Said, never swallowed: the store copies ARE gone, but without
                // the marker a projection the user copied beside the RAW now
                // out-ranks this clear and restores the edits on the next open.
                let note = match &outcome.marker_warning {
                    Some(w) => format!(
                        " - but the clear could not be marked ({w}); a sidecar beside the RAW \
                         may restore these edits when you reopen"
                    ),
                    None => String::new(),
                };
                Ok(with_revision(
                    text_response(&format!("cleared — saved edits removed{note}")),
                    crate::store::develop_revision(&raw).as_deref(),
                ))
            }
            Err(e) => Ok(status_response(500, &format!("could not clear the saved edits: {e}"))),
        };
    }
    // The WRITER's rule (produce_recipe, match, the GUI savers): a washed
    // pre-era curve must not be re-persisted verbatim. The browser can hold
    // one legitimately — a select-time inability made api_recipe serve it
    // unrepaired — and saving it back froze the defect on disk while the
    // render surfaces kept disclosing a repair the file never received.
    // AFTER the clear branch above: the repair changes only the curve and
    // its stamp, both of which is_noop neutralises, so the branch decision
    // cannot flip.
    let mut save_note = String::new();
    if let Some(note) = crate::pipeline::repair_pre_era_base_curve(&raw, &mut req.recipe) {
        save_note = format!(" — {note}");
    }
    // Dual-write, exactly as the GUI's `save_xmp` does: the XMP alone is lossy
    // (no bitmap masks / recolour gains) AND neither surface reads it back while
    // a `recipe.json` exists — so an XMP-only save was unreachable, silently
    // shadowed by whatever recipe.json Analyze had left behind. recipe.json
    // FIRST (same order as the GUI): it is the authoritative projection, and
    // writing the XMP first meant a failed recipe write left a NEW XMP shadowed
    // by the STALE recipe both surfaces prefer.
    //
    // ONE single-generation commit (the Ctrl+S rule, L03): the recipe and the
    // browser-session master's pixels.json land whole or the save fails whole
    // — the client then never baselines (markSaved) on a 200 whose pixels
    // never landed. A session master (fill/heal chained this session) becomes
    // the develop's persisted pixel source on save — the GUI rule: saving
    // records pixel identity, else the healed pixels evaporate on the very
    // reopen that follows "saved". Only ever WRITTEN here: an absent claim
    // must not clear a GUI-persisted master (`Keep`).
    let pixels = match &master {
        // GENERATED provenance rides from the issuance registry: hardcoding
        // inplace here made every later open render the base curve / lens
        // profile / anchor on top of pixels that already carry them.
        Some((p, generated)) => crate::store::CommitMember::Write(
            crate::store::pixel_source_record_bytes(&raw, p, *generated)?,
        ),
        None => crate::store::CommitMember::Keep,
    };
    // The strip half goes through the ONE owner (R24-4): the web writes no
    // strip of its own, so it states only what this save really establishes
    // — a GENERATED session master in the same generation makes the active
    // card pixel-state, and anything else leaves the record standing
    // (`Unknown`; the web cannot name a card it never renders, and claiming
    // the base negative's slot would mint a second, undeletable Original).
    let variants = crate::store::variants_member(
        &raw,
        match &master {
            Some((_, true)) => crate::store::ActiveWrite::Kind("generated"),
            _ => crate::store::ActiveWrite::Unknown,
        },
    )?;
    crate::store::commit_develop(
        &raw,
        crate::store::DevelopCommit {
            recipe: Some(pipeline::recipe_store_bytes(&raw, &req.recipe, crate::diag::stderr())?),
            pixels,
            variants,
        },
    )?;
    let mut master_note = String::new();
    // The SAME question the clear branch asked, so it must read the SAME
    // recipe — the one the client sent. Testing the clamped copy here made the
    // note fire for a body that was routed past the clear branch because it
    // carried a crop, and then told the user it had been routed past "solely
    // because of the persisted master".
    if client_asked_to_clear && master.is_none() && crate::store::has_pixel_source(&raw) {
        // This save was routed PAST the clear branch above solely because of
        // the persisted master — say so, or the 200 reads as "everything is
        // neutral now" while the baked retouch still backs every render.
        // No removal instruction: the only route that detaches a healthy
        // in-place master is SESSION-BOUND and undiscoverable — in the GUI,
        // undo to the pre-retouch step and save (the save sees an
        // origin-free canvas and clears the recorded link: through the
        // neutral-clear guard when the sliders are neutral, through the
        // ordinary save's clear_pixel_source otherwise); gone after a
        // reopen, when the undo stack is empty and origin restores from
        // pixels.json. The web has no route at all. The roadmap records the
        // missing explicit action; until then the note states what
        // happened, not a route the user cannot find.
        master_note = " — the saved retouch master was kept: a neutral save never deletes \
                       baked pixels"
            .to_string();
    }
    // Recipe committed = saved (the cross-surface rule): a failed XMP
    // projection reports success WITH the warning instead of a 500 that
    // contradicts the on-disk state.
    // The NEW revision rides both success replies (recipe.json committed
    // either way), so the tab adopts it without a re-GET. Computed AFTER
    // the projection write in each arm: the develop tag folds in a sidecar
    // that out-ranks the store, and the tag the tab adopts must describe
    // the state its next If-Match is compared against.
    match pipeline::write_xmp(&raw, &req.recipe, crate::diag::stderr()) {
        // A regenerated (rather than merged) sidecar is a LOSS of the user's
        // Lightroom-only properties, so it rides the same reply as the path —
        // reporting a bare success here is what made the loss silent.
        Ok((path, merge_note, losses)) => {
            let merge_note = merge_note.map(|m| format!("\n⚠ {m}")).unwrap_or_default();
            // M6a export-side disclosure, in the SAME "\n⚠ …" note shape the
            // merge loss already uses (the reply is plain text; a new field
            // would be a new API the tab does not read).
            let mask_note = crate::xmp::describe_mask_losses(&losses)
                .map(|m| format!("\n⚠ {m}"))
                .unwrap_or_default();
            Ok(with_revision(
                text_response(&format!(
                    "{}{save_note}{master_note}{merge_note}{mask_note}",
                    path.display()
                )),
                crate::store::develop_revision(&raw).as_deref(),
            ))
        }
        Err(e) => Ok(with_revision(
            text_response(&format!(
                "saved (recipe.json) — but the Lightroom XMP projection failed: {e:#}{save_note}{master_note}"
            )),
            crate::store::develop_revision(&raw).as_deref(),
        )),
    }
        },
    )
}

/// Generative fill (Phase 4 in the UI): the browser posts a painted RGBA mask
/// (transparent = regenerate) + a prompt; we run [`generative::retouch`], which
/// composites the regenerated region back onto the FULL-resolution source and
/// writes the master to ./out. We return a resized JPEG of the result for inline
/// display, with the saved master path in `X-Output-Path`. Needs OPENAI_API_KEY.
fn api_retouch(request: &mut Request, state: &AppState) -> Result<ResponseBox> {

    let stamp = request_gen(request);
    let mut req: RetouchReq = read_json(request)?;
    // Untrusted network input — see api_analyze.
    if let Some(v) = req.view.as_mut() {
        v.clamp();
    }
    let raw = match state.at_checked(req.id, stamp) {
        Ok(r) => r,
        Err(resp) => return Ok(resp),
    };
    // Accept either a "data:image/png;base64,XXXX" URL or bare base64.
    let b64 = req.mask.rsplit(',').next().unwrap_or(&req.mask).trim();
    let mask_bytes = match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(b) => b,
        Err(e) => return Ok(status_response(400, &format!("bad mask base64: {e}"))),
    };
    // Retouch the photo's RENDER INPUT — the browser-session master when the
    // client carries one (a fill/heal chained THIS session; without it a
    // second operation restarted from the persisted master and silently
    // discarded the first, U30), else the persisted pixels.json master, else
    // the photo itself (api_develop's rule).
    let (src, src_generated) = match session_master(req.master.as_deref(), &raw) {
        Ok(Some((p, g))) => (p, g),
        Ok(None) => crate::store::read_pixel_source(&raw)
            .unwrap_or_else(|| (raw.clone(), false)),
        Err(msg) => return Ok(status_response(400, &msg)),
    };
    // The mask was painted over the DISPLAYED (post-geometry) preview —
    // un-warp it into the source frame the retouch engine works in, using
    // the DISPLAYED source's dims (the master, not the RAW: their aspects
    // can differ and the engine consumes the master).
    let mask_bytes =
        unwarp_mask(&mask_bytes, view_for_mapping(&raw, req.view.as_ref()).as_ref(), &src)
            .unwrap_or(mask_bytes);
    // generative::retouch takes a mask FILE path, so stage the PNG in a temp file.
    let mask_tmp = std::env::temp_dir().join(format!(
        "autoshade_mask_{}_{}.png",
        std::process::id(),
        DL_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(e) = std::fs::write(&mask_tmp, &mask_bytes) {
        return Ok(status_response(500, &format!("stage mask: {e}")));
    }
    // Atomically CLAIMED unique name (retouch, retouch-2, …): the fixed
    // default_out name let a rerun overwrite a master an earlier develop —
    // possibly a GUI-saved pixels.json — still references.
    let Some(out) = pipeline::unique_out(&raw, "retouch") else {
        return Ok(status_response(500, "no free retouch output name (999 in ./out)"));
    };
    // Config SNAPSHOT, not the read guard: holding the RwLock across the
    // multi-minute AI call blocked every settings save until it finished.
    let cfg = state.config().clone();
    let quality = req.quality.unwrap_or_else(|| cfg.openai_image_quality.clone());
    let result =
        crate::generative::retouch(&cfg, &src, &mask_tmp, &req.prompt, &quality, req.full_res, &out);
    let _ = std::fs::remove_file(&mask_tmp);
    match result {
        Ok(()) => {
            // The new master inherits the chain root's generated-ness: a fill
            // composited ONTO a generated master still carries its look.
            issue_session_master(&raw, &out, src_generated);
            // baked-by-construction: the ./out retouch master written just above.
            let img = decode::load_image(&out)?
                .resize(1400, 1400, image::imageops::FilterType::Triangle);
            let mut buf = Vec::new();
            img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
                .context("encode jpeg")?;
            // The saved path carries the photo's stem: percent-encoded, because
            // a non-ASCII header value is rejected outright (`.unwrap()` there
            // used to kill Fill on any non-ASCII filename). The UI decodes it.
            Ok(image_with_path(buf, &out))
        }
        Err(e) => {
            // Release the atomically claimed name when nothing real landed
            // (0-byte placeholder) — failed runs used to consume the 999-name
            // cap; a non-empty partial stays for diagnosis (GUI rule).
            if std::fs::metadata(&out).is_ok_and(|m| m.len() == 0) {
                let _ = std::fs::remove_file(&out);
            }
            Ok(status_response(500, &format!("retouch failed: {e}")))
        }
    }
}

#[derive(Deserialize)]
struct HealReq {
    id: usize,
    /// Optional painted RGBA PNG mask (data URL or bare base64); transparent = heal here.
    #[serde(default)]
    mask: Option<String>,
    /// Auto-detect spots with the vision model (default true).
    #[serde(default = "default_true")]
    auto: bool,
    #[serde(default)]
    full_res: bool,
    /// See RetouchReq.view — the mask's viewing geometry for un-warping.
    #[serde(default)]
    view: Option<EditRecipe>,
    /// Browser-session master (see `session_master`): heal on top of THIS
    /// ./out file — a chained retouch — instead of the persisted source.
    #[serde(default)]
    master: Option<String>,
}
fn default_true() -> bool {
    true
}

/// Pixel-retouch (heal) mode: the vision model auto-detects small defects and/or
/// the browser posts a painted mask; the deterministic engine heals each from
/// SURROUNDING REAL pixels (no generation). Saves a pixel master to ./out and
/// returns a JPEG of the result for inline display, path in `X-Output-Path`.
fn api_heal(request: &mut Request, state: &AppState) -> Result<ResponseBox> {

    let stamp = request_gen(request);
    let mut req: HealReq = read_json(request)?;
    // Untrusted network input — see api_analyze.
    if let Some(v) = req.view.as_mut() {
        v.clamp();
    }
    let raw = match state.at_checked(req.id, stamp) {
        Ok(r) => r,
        Err(resp) => return Ok(resp),
    };
    // Stage the optional painted mask (data URL or bare base64) to a temp PNG.
    let mask_tmp = match &req.mask {
        Some(m) if !m.trim().is_empty() => {
            let b64 = m.rsplit(',').next().unwrap_or(m).trim();
            match base64::engine::general_purpose::STANDARD.decode(b64) {
                Ok(bytes) => {
                    // Same un-warp as api_retouch (see there) — through the
                    // mapping view AND the DISPLAYED source's dims, which is
                    // the session master when one is chained.
                    let display_src = match session_master(req.master.as_deref(), &raw) {
                        Ok(Some((p, _))) => p,
                        Ok(None) => crate::store::read_pixel_source(&raw)
                            .map(|(m, _)| m)
                            .unwrap_or_else(|| raw.clone()),
                        Err(msg) => return Ok(status_response(400, &msg)),
                    };
                    let bytes = unwarp_mask(
                        &bytes,
                        view_for_mapping(&raw, req.view.as_ref()).as_ref(),
                        &display_src,
                    )
                    .unwrap_or(bytes);
                    let t = std::env::temp_dir().join(format!(
                        "autoshade_heal_{}_{}.png",
                        std::process::id(),
                        DL_SEQ.fetch_add(1, Ordering::Relaxed)
                    ));
                    if let Err(e) = std::fs::write(&t, &bytes) {
                        return Ok(status_response(500, &format!("stage mask: {e}")));
                    }
                    Some(t)
                }
                Err(e) => return Ok(status_response(400, &format!("bad mask base64: {e}"))),
            }
        }
        _ => None,
    };
    // Same master-input rule as api_retouch (see there): session master
    // first — a second heal must build ON the first, not beside it.
    //
    // RESOLVED BEFORE the output claim, in api_retouch's order. `unique_out`
    // claims its name by creating a 0-byte placeholder, and the rejection arm
    // below returns without releasing it — so every rejected claim used to
    // leave a 0-byte out/<stem>.heal-N.png that the next claim then skips.
    // Batch 46 made rejection routine (any master not issued by THIS server
    // run is refused, which is the normal state of a tab that outlived a
    // restart), and 999 leaks retire that photo's heal output names for good.
    let (src, src_generated) = match session_master(req.master.as_deref(), &raw) {
        Ok(Some((p, g))) => (p, g),
        Ok(None) => crate::store::read_pixel_source(&raw)
            .unwrap_or_else(|| (raw.clone(), false)),
        Err(msg) => return Ok(status_response(400, &msg)),
    };
    // Same unique-claim + config-snapshot rules as api_retouch (see there).
    let Some(out) = pipeline::unique_out(&raw, "heal") else {
        return Ok(status_response(500, "no free heal output name (999 in ./out)"));
    };
    let cfg = state.config().clone();
    let result = crate::retouch::heal(&cfg, &src, mask_tmp.as_deref(), req.auto, req.full_res, &out);
    if let Some(t) = &mask_tmp {
        let _ = std::fs::remove_file(t);
    }
    match result {
        Ok(rep) => {
            // Same inheritance rule as api_retouch (see there).
            issue_session_master(&raw, &out, src_generated);
            let img =
                // baked-by-construction: the ./out heal master written just above.
                decode::load_image(&out)?.resize(1400, 1400, image::imageops::FilterType::Triangle);
            let mut buf = Vec::new();
            img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
                .context("encode jpeg")?;
            // Same non-ASCII trap as Fill: the path goes out percent-encoded.
            let mut resp = image_with_path(buf, &out);
            if let Some(h) = header("X-Heal-Spots", &rep.spots.to_string()) {
                resp = resp.with_header(h);
            }
            // The rationale can disclose a partial outcome (e.g. "AI
            // spot-detection failed; healed the painted mask only") —
            // dropping it reported unqualified success. Percent-encoded:
            // header values are ASCII-only.
            if !rep.rationale.is_empty()
                && let Some(h) = header("X-Heal-Rationale", &percent_encode(&rep.rationale))
            {
                resp = resp.with_header(h);
            }
            Ok(resp)
        }
        Err(e) => {
            // Same 0-byte claim release as api_retouch.
            if std::fs::metadata(&out).is_ok_and(|m| m.len() == 0) {
                let _ = std::fs::remove_file(&out);
            }
            Ok(status_response(500, &format!("heal failed: {e}")))
        }
    }
}

/// Current provider/model settings for the Settings panel. Never returns the raw
/// API keys — only whether each is present.
///
/// Reads the FILE's current truth (`Config::load`), not this server's cached
/// snapshot: the desktop GUI merges onto the same settings file, and a form
/// filled from a boot-time cache round-tripped that stale snapshot on the
/// next save — a web save of an unrelated field erased whatever the GUI had
/// set since the server started (the cache itself still only swaps on POST:
/// it feeds AI calls, and a GET must not change what calls use).
fn api_settings_get(_state: &AppState) -> Result<ResponseBox> {
    let cfg = Config::load();
    let body = json!({
        "analysis": {
            "provider": cfg.analysis_provider,
            "model": cfg.analysis_model,
            "base_url": cfg.analysis_base_url,
            "key_present": cfg.analysis_api_key.is_some(),
            // "" means "send no effort parameter" — a real choice, not a
            // missing value (see `config::LocalSettings::analysis_effort`).
            "effort": cfg.analysis_effort.clone().unwrap_or_default(),
        },
        "image": {
            "model": cfg.openai_model,
            "base_url": cfg.openai_base_url,
            "gen_model": cfg.openai_image_model,
            "key_present": cfg.openai_api_key.is_some(),
            "effort": cfg.image_effort.clone().unwrap_or_default(),
        },
        // The `claude` CLI has no image input in print mode → image-via-OAuth is
        // not available; the image role always uses an OpenAI-compatible API.
        "image_oauth_supported": false,
        "settings_file": crate::config::local_settings_path().display().to_string(),
    });
    Ok(json_response(&body))
}

/// Persist provider/model/key changes to the gitignored local file, then
/// hot-reload the running config. Blank key fields are left unchanged (the GET
/// side never reveals existing keys, so the UI sends a key only when it changes).
/// The read-modify-write cycle runs inside `config::update_local_settings`,
/// under the CROSS-PROCESS settings lock: the in-process Mutex that used to
/// sit here serialized only this server's threads, while the GUI process
/// merges onto the same file — either side's save landing between the other's
/// load and rename was silently erased (L01).
fn api_settings_post(request: &mut Request, state: &AppState) -> Result<ResponseBox> {
    let inc: LocalSettings = read_json(request)?;
    let path = crate::config::update_local_settings(|cur| {
        // Non-secret fields: take whatever the UI sent (empty ⇒ falls back to default).
        if inc.analysis_provider.is_some() {
            cur.analysis_provider = inc.analysis_provider;
        }
        if inc.analysis_model.is_some() {
            cur.analysis_model = inc.analysis_model;
        }
        if inc.analysis_base_url.is_some() {
            cur.analysis_base_url = inc.analysis_base_url;
        }
        if inc.image_model.is_some() {
            cur.image_model = inc.image_model;
        }
        if inc.image_base_url.is_some() {
            cur.image_base_url = inc.image_base_url;
        }
        if inc.image_gen_model.is_some() {
            cur.image_gen_model = inc.image_gen_model;
        }
        // Effort rides the same "present ⇒ take it" rule as the other
        // non-secret fields, so a request that sends `""` CLEARS the tier back
        // to the provider default instead of being unable to express it.
        if inc.analysis_effort.is_some() {
            cur.analysis_effort = inc.analysis_effort;
        }
        if inc.image_effort.is_some() {
            cur.image_effort = inc.image_effort;
        }
        // Secrets: only overwrite when a non-empty value was actually
        // provided — and a provided key is FOR the base being saved with it
        // (the base fields were applied above), so record that home:
        // `config::file_key_for` refuses to send it anywhere else later.
        if let Some(k) = inc.analysis_api_key.filter(|s| !s.trim().is_empty()) {
            cur.analysis_api_key = Some(k);
            cur.analysis_api_key_base =
                cur.analysis_base_url.clone().filter(|s| !s.trim().is_empty());
        }
        if let Some(k) = inc.image_api_key.filter(|s| !s.trim().is_empty()) {
            cur.image_api_key = Some(k);
            cur.image_api_key_base =
                cur.image_base_url.clone().filter(|s| !s.trim().is_empty());
        }
    })
    .map_err(|e| anyhow!("write settings: {e}"))?;
    *state.cfg.write().unwrap_or_else(|e| e.into_inner()) = Config::load();
    Ok(json_response(&json!({ "ok": true, "saved": path.display().to_string() })))
}

// --- helpers ---------------------------------------------------------------

/// DERIVED, not re-typed (R27 A2): this was the second hand-written copy of
/// the baked extension list, and a copy is a drift waiting to happen — the
/// third copy, in `web/index.html`, had already lost `.orf`/`.rw2`/`.raw`.
/// `pipeline::BAKED_EXTS` is the one source; `list_coherence` pins it.
fn is_baked_ext(p: &Path) -> bool {
    pipeline::is_baked(p)
}

fn raw_for(request: &Request, state: &AppState) -> Result<PathBuf> {
    let id = query_param(request.url(), "id")
        .and_then(|v| v.parse::<usize>().ok())
        .ok_or_else(|| anyhow!(ClientErr("missing/invalid id".into())))?;
    state.at(id).ok_or_else(|| anyhow!(ClientErr("bad id".into())))
}

/// Percent-decode a value (e.g. an `encodeURIComponent`-encoded filename) back to
/// its UTF-8 string. HTTP header values are ISO-8859-1 only, so the browser must
/// percent-encode non-ASCII filenames (Chinese, emoji, …) — we decode them here.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) =
                ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Percent-ENCODE for an HTTP header value (the inverse of [`percent_decode`]):
/// everything outside the RFC 3986 unreserved set becomes `%XX` of its UTF-8
/// bytes. Header values are ASCII-only — `Header::from_bytes` rejects anything
/// else — so any value carrying a user filename/path must come through here or
/// the header cannot be built at all.
fn percent_encode(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
    }
    out
}

/// Build a header whose VALUE is forced into printable ASCII.
///
/// TWO hazards, and tiny_http covers only part of one. `Header::from_bytes`
/// validates with `AsciiString::from_ascii`, which accepts every byte
/// 0x00–0x7F — **including CR and LF** — and writes the value verbatim: it
/// rejects non-ASCII but NOT response splitting. Control bytes are therefore
/// STRIPPED here, so the no-splitting guarantee lives with the constructor.
/// Non-ASCII does not refuse either (L04-7): the dynamic strings that flow
/// through here — localized Windows IO errors, the xmlns-conflict sentence's
/// em dash — used to fail the ASCII check and die in an `.ok()`, which
/// silenced X-Recipe-Warning in exactly the cases that most needed it. They
/// degrade per-character instead (dashes kept legible, the rest `?`), so the
/// warning always ships.
fn header(field: &str, value: &str) -> Option<Header> {
    let clean = ascii_sanitize(value);
    Header::from_bytes(field.as_bytes(), clean.as_bytes()).ok()
}

/// See [`header`]: printable-ASCII projection of a header value — control
/// bytes stripped, the house dashes kept legible, all other non-ASCII `?`.
fn ascii_sanitize(value: &str) -> String {
    value
        .chars()
        .filter_map(|c| match c {
            '\u{2014}' | '\u{2013}' => Some('-'),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => None,
            c if c.is_ascii() => Some(c),
            _ => Some('?'),
        })
        .collect()
}

/// A JPEG body plus the `X-Output-Path` of the master that was saved, with the
/// path percent-encoded so a non-ASCII stem can never break the header.
fn image_with_path(buf: Vec<u8>, out: &Path) -> ResponseBox {
    let mut resp = Response::from_data(buf).with_header(no_store());
    if let Some(h) = header("Content-Type", "image/jpeg") {
        resp = resp.with_header(h);
    }
    if let Some(h) = header("X-Output-Path", &percent_encode(&out.display().to_string())) {
        resp = resp.with_header(h);
    }
    resp.boxed()
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == key).then(|| v.to_string())
    })
}

/// Aggregate cap on request-body bytes buffered across ALL in-flight
/// requests (L02): the per-request caps alone let 8 concurrent bodies stack
/// 8 × 500 MiB ≈ 4 GiB. 1 GiB keeps every single-tab flow byte-identical
/// (the bundled UI uploads sequentially, and one 500 MiB upload fits); two
/// tabs racing 500 MiB drops see one refused instead of the stack.
const BODY_BUDGET: u64 = 1024 * 1024 * 1024;
static BODY_BYTES: AtomicU64 = AtomicU64::new(0);

/// The bytes THIS request charged against [`BODY_BUDGET`] — refunded on
/// Drop, unconditionally: a panicking handler must give the budget back
/// (the exact leak `Permit`'s docs record).
struct BodyBudget(u64);

impl BodyBudget {
    fn charge(&mut self, n: u64) -> bool {
        // fetch_update (CAS), not add-then-check: two threads must never
        // both pass the check and overshoot the ceiling together.
        if BODY_BYTES
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                cur.checked_add(n).filter(|&t| t <= BODY_BUDGET)
            })
            .is_err()
        {
            return false;
        }
        self.0 += n;
        true
    }
}

impl Drop for BodyBudget {
    fn drop(&mut self) {
        BODY_BYTES.fetch_sub(self.0, Ordering::Relaxed);
    }
}

/// A request-body reader that charges the aggregate budget and enforces a
/// pacing deadline (idle 30 s between reads, 10 min for the whole body —
/// nothing had ANY deadline before). HONEST LIMIT: the deadline fires
/// BETWEEN reads, so it kills byte-trickle clients but cannot interrupt a
/// peer that went silent mid-read — tiny_http 0.12 exposes no socket handle
/// and sets no read timeout, so a fully wedged `read` parks until TCP
/// keepalive. Bounding THAT needs a listener-level SO_RCVTIMEO experiment,
/// not this adapter.
struct GuardedBody<'a> {
    inner: &'a mut dyn std::io::Read,
    budget: &'a mut BodyBudget,
    hard: std::time::Instant,
    last: std::time::Instant,
}

impl<'a> GuardedBody<'a> {
    const IDLE: std::time::Duration = std::time::Duration::from_secs(30);
    const HARD: std::time::Duration = std::time::Duration::from_secs(600);
    fn new(inner: &'a mut dyn std::io::Read, budget: &'a mut BodyBudget) -> Self {
        let now = std::time::Instant::now();
        GuardedBody { inner, budget, hard: now + Self::HARD, last: now }
    }
}

impl std::io::Read for GuardedBody<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let now = std::time::Instant::now();
        if now > self.hard || now.duration_since(self.last) > Self::IDLE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request body read timed out",
            ));
        }
        let n = std::io::Read::read(self.inner, buf)?;
        self.last = std::time::Instant::now();
        if n > 0 && !self.budget.charge(n as u64) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::OutOfMemory,
                "server busy: aggregate request-body budget exhausted — retry shortly",
            ));
        }
        Ok(n)
    }
}

fn read_json<T: serde::de::DeserializeOwned>(request: &mut Request) -> Result<T> {
    // Bounded: an unbounded read_to_string materialized an arbitrarily large
    // (or hostile) body in memory before parsing ever ran. 256 MiB clears any
    // legitimate payload (the largest is a full-res mask base64) many times
    // over.
    use std::io::Read as _;
    const BODY_CAP: u64 = 256 * 1024 * 1024;
    let mut body = String::new();
    // Declared-size pre-reject: a Content-Length past the cap refuses before
    // the first byte is read (chunked bodies carry no length and fall
    // through to the read cap below).
    if let Some(len) = request.body_length()
        && len as u64 > BODY_CAP
    {
        anyhow::bail!(ClientErr(format!(
            "request body exceeds {} MiB",
            BODY_CAP / (1024 * 1024)
        )));
    }
    let mut budget = BodyBudget(0);
    let mut gb = GuardedBody::new(request.as_reader(), &mut budget);
    // UFCS: `take` on the sized adapter (autoref on the UNSIZED trait object
    // fights the method resolver).
    std::io::Read::take(&mut gb, BODY_CAP + 1)
        .read_to_string(&mut body)
        .context("read body")?;
    if body.len() as u64 > BODY_CAP {
        anyhow::bail!(ClientErr(format!("request body exceeds {} MiB", BODY_CAP / (1024 * 1024))));
    }
    serde_json::from_str(&body).map_err(|e| anyhow!(ClientErr(format!("parse request JSON: {e}"))))
}

// Response builders. They BUILD a response rather than consuming the request, so
// `handle` still owns it when a handler fails and can answer with the error text
// instead of tiny_http's bodyless Drop-500.

fn json_response(v: &serde_json::Value) -> ResponseBox {
    json_text(v.to_string())
}

/// Dynamic `/api/*` payloads must never be answered from a browser cache: a
/// standards-permitted reuse of a cached `/api/recipe` 404 after another
/// surface saved a develop reopens a NEUTRAL state whose next save clears
/// the newer develop (16-lane scan L10). Ids are also reused across folder
/// switches, so images are no-store too.
fn no_store() -> Header {
    Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap()
}

/// A JSON body that is already serialised (e.g. a sidecar served verbatim).
fn json_text(text: String) -> ResponseBox {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    Response::from_string(text).with_header(header).with_header(no_store()).boxed()
}

fn html_response(html: &str) -> ResponseBox {
    let ct = Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap();
    // No-cache: the UI HTML is embedded in the binary and changes on every rebuild,
    // so the browser MUST re-fetch it after a restart — otherwise a stale cached
    // page hides fixes/features until a manual Ctrl+F5.
    let cc = Header::from_bytes(
        &b"Cache-Control"[..],
        &b"no-cache, no-store, must-revalidate"[..],
    )
    .unwrap();
    // The UI listens on a FIXED, well-known port (8080 by default), so any page
    // can blind-frame it and clickjack a destructive control — Save on a
    // neutral recipe routes to `clear_develop`. The Origin/Host guard does not
    // help: a framed click is a same-origin action by the real page.
    let frame = Header::from_bytes(&b"X-Frame-Options"[..], &b"DENY"[..]).unwrap();
    Response::from_string(html)
        .with_header(ct)
        .with_header(cc)
        .with_header(frame)
        .boxed()
}

fn text_response(text: &str) -> ResponseBox {
    Response::from_string(text).boxed()
}

fn jpeg_response(img: &DynamicImage) -> Result<ResponseBox> {
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
        .context("encode jpeg")?;
    let header = Header::from_bytes(&b"Content-Type"[..], &b"image/jpeg"[..]).unwrap();
    Ok(Response::from_data(buf).with_header(header).with_header(no_store()).boxed())
}

fn status_response(code: u16, msg: &str) -> ResponseBox {
    Response::from_string(msg).with_status_code(code).with_header(no_store()).boxed()
}

/// Stamp the saved recipe's revision as a strong ETag. `None` (an existing
/// file that cannot be read — untaggable) stamps nothing: a conditional
/// writer then gets 412, an unconditional one keeps today's behaviour.
fn with_revision(resp: ResponseBox, rev: Option<&str>) -> ResponseBox {
    match rev.and_then(|r| header("ETag", &format!("\"{r}\""))) {
        Some(h) => resp.with_header(h),
        None => resp,
    }
}

/// The `If-Match` precondition for the recipe writers, evaluated INSIDE the
/// develop lock. No header = no precondition (curl / the CLI / older pages
/// stay unconditional — the tag is opt-in; the bundled client always sends
/// it). A mismatch — or an untaggable current state — answers 412 carrying
/// the live revision and writes NOTHING: the develop lock serializes two
/// tabs' saves but cannot order them, so without this the later tab
/// silently destroyed the earlier tab's edit (lost update, L01).
fn precondition_failed(if_match: Option<&str>, current: Option<&str>) -> Option<ResponseBox> {
    let raw = if_match?;
    let matched = match current {
        Some(cur) => {
            // RFC 9110 shapes: a comma list admits when ANY listed tag
            // matches (strong comparison — a `W/` tag never gates a write);
            // `*` means "only if a saved state exists", which the "none"
            // tag by definition is not. Each tag sheds ONE quote layer (the
            // wire form); a bare tag is our own lenient form. Untaggable
            // current state admits nothing — refusing beats overwriting
            // what cannot be named.
            (raw.trim() == "*" && cur != "none")
                || raw.split(',').any(|t| {
                    let t = t.trim();
                    t.strip_prefix('"').and_then(|t| t.strip_suffix('"')).unwrap_or(t) == cur
                })
        }
        None => false,
    };
    if matched {
        return None;
    }
    Some(with_revision(
        status_response(
            412,
            "this photo's develop changed since this tab loaded it — nothing was \
             overwritten; reselect the photo to load the newer save, then redo the change",
        ),
        current,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R27 A2, tier 2 — the web page's `<input accept>` list against the
    /// formats the app actually opens.
    ///
    /// This is the ONE copy of the extension set that cannot be derived at
    /// runtime: `index.html` is a static asset (`include_str!` at the top of
    /// this file), so a browser's file picker is filtered by a string nothing
    /// checked. It had drifted exactly as you would expect — `.orf`, `.rw2`
    /// and `.raw` were openable by the gallery, the desktop dialog and the
    /// upload endpoint, and invisible in the web picker, for four releases.
    ///
    /// The assertion is set equality, not textual equality, so re-ordering the
    /// attribute is allowed and dropping or inventing a format is not. The
    /// failure message prints the exact replacement string.
    ///
    /// MUTATION THIS CATCHES: add a format to `decode::RAW_EXTS` (which R27
    /// did, 15 of them) or to `pipeline::BAKED_EXTS` (3 more) and forget the
    /// HTML — this fails and hands you the line to paste.
    #[test]
    fn the_web_accept_list_matches_the_formats_the_app_opens() {
        let marker = "id=\"file-input\"";
        let line = INDEX_HTML
            .lines()
            .find(|l| l.contains(marker))
            .expect("index.html still has the file-input element");
        let accept = line
            .split("accept=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .expect("the file input still carries an accept attribute");
        let mut declared: Vec<String> = accept
            .split(',')
            .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        let mut expected: Vec<String> =
            pipeline::photo_exts().iter().map(|e| e.to_string()).collect();
        declared.sort();
        declared.dedup();
        expected.sort();
        expected.dedup();
        let want_attr: String =
            pipeline::photo_exts().iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(",");
        assert_eq!(
            declared, expected,
            "the web accept list has drifted from decode::RAW_EXTS ∪ pipeline::BAKED_EXTS.\n\
             Replace the attribute in src/web/index.html with:\n  accept=\"{want_attr}\""
        );
    }

    /// R27 A2, tier 2 — the server's own baked gate is the SHARED list, not a
    /// second copy of it. `is_baked_ext` is now a one-line delegation, and
    /// this is what keeps it one: re-inline the `matches!` and any format
    /// added to `pipeline::BAKED_EXTS` starts being refused by the add-path
    /// and upload endpoints while the gallery scan accepts it.
    ///
    /// MUTATION THIS CATCHES: give `is_baked_ext` back its own hard-coded
    /// arm — the three R27 additions (webp/bmp/gif) fail here immediately.
    #[test]
    fn the_servers_baked_gate_is_the_shared_list() {
        for ext in pipeline::BAKED_EXTS {
            let p = std::path::PathBuf::from(format!("photo.{ext}"));
            assert!(is_baked_ext(&p), "{ext} is in BAKED_EXTS but the server refuses it");
            // Case folding is part of the contract — cameras and editors write
            // .JPG as often as .jpg.
            let up = std::path::PathBuf::from(format!("photo.{}", ext.to_ascii_uppercase()));
            assert!(is_baked_ext(&up), "{ext} must match case-insensitively");
        }
        for ext in crate::decode::RAW_EXTS {
            let p = std::path::PathBuf::from(format!("photo.{ext}"));
            assert!(
                !is_baked_ext(&p),
                "{ext} is a camera RAW — the baked gate must not claim it"
            );
        }
        assert!(!is_baked_ext(std::path::Path::new("notes.txt")));
        // The two endpoints' real predicate is the union.
        assert!(
            pipeline::is_source(std::path::Path::new("a.CR3"))
                && pipeline::is_source(std::path::Path::new("b.WEBP")),
            "add-path/upload accept both halves"
        );
    }

    /// R23-3 shell (feedback #5): the web body's own end of the grade-strength
    /// axis. An older tab that never sends the field must land on the SAME
    /// default the desktop app and the CLI use — and a hostile number must go
    /// through the door, not into the axis. (0.0 would be the most TIMID setting
    /// on a dial that exists because the AI was too timid, which is why the
    /// absent field cannot simply decode to a bare `f32` zero.)
    #[test]
    fn an_analyze_body_without_a_grade_strength_lands_on_the_shipped_default() {
        use crate::recipe::GradeStrength;
        let parse = |body: &str| serde_json::from_str::<AnalyzeReq>(body).expect("body parses");
        // The PRODUCTION resolver, not a re-implementation of it — a test that
        // spells the decision itself proves only that the test agrees with
        // itself.
        let resolve = |r: &AnalyzeReq| GradeStrength::from_optional(r.grade_strength).get();

        // The pre-R23 body shape, verbatim: no field at all.
        let old = parse(r#"{"id":0,"style_strength":0.3}"#);
        assert_eq!(old.grade_strength, None);
        assert_eq!(resolve(&old), GradeStrength::DEFAULT);

        // Sent, and honoured — on its OWN axis, not the style one.
        let sent = parse(r#"{"id":0,"style_strength":0.3,"grade_strength":0.9}"#);
        assert_eq!(resolve(&sent), 0.9);
        assert_eq!(sent.style_strength, Some(0.3), "the two axes must not alias");

        // Untrusted numbers: clamped, and a non-finite one is the default.
        assert_eq!(resolve(&parse(r#"{"id":0,"grade_strength":9}"#)), 1.0);
        assert_eq!(resolve(&parse(r#"{"id":0,"grade_strength":-4}"#)), 0.0);
        // JSON cannot spell NaN, but 1e39 overflows f32 to +inf on the way in —
        // the same route the judge's score test uses.
        assert_eq!(resolve(&parse(r#"{"id":0,"grade_strength":1e39}"#)), GradeStrength::DEFAULT);
    }

    /// L04-7: the warning header must SURVIVE its worst real payloads — the
    /// xmlns-conflict sentence carries an em dash, and localized IO errors
    /// are non-ASCII; the raw ASCII-only constructor silently dropped both.
    #[test]
    fn the_warning_header_survives_a_non_ascii_disclosure() {
        let doc = r#"<rdf:Description xmlns:crs="urn:other" crs:Exposure2012="+1.00"/>"#;
        let h = recipe_warning_header(doc)
            .expect("a conflict sentence with an em dash must still build the header");
        assert!(h.value.as_str().is_ascii());
        assert!(h.value.as_str().contains("not imported"), "{}", h.value.as_str());

        let localized = header("X-Recipe-Warning", "cannot read (拒绝访问。) — not reflected")
            .expect("a localized OS error must not kill the warning");
        assert!(localized.value.as_str().is_ascii());

        // The anti-splitting half is unchanged: control bytes are stripped.
        let split = ascii_sanitize("a\r\nX-Injected: b");
        assert!(!split.contains('\r') && !split.contains('\n'));
    }

    /// C1/F10: an export slot claimed by a pre-canonical build under the
    /// LEXICAL key must keep matching its photo after the re-key — else a
    /// re-key silently changes deliverable filenames. Junction fixture so
    /// the two keys genuinely differ; it must build LOUDLY (the L14#5
    /// lesson: a fixture that cannot build must never silently pass green).
    #[test]
    fn an_export_slot_claimed_under_the_lexical_key_still_matches_its_photo() {
        let base = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join("autoshade-serve-test-c1-registry");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let target = base.join("real");
        std::fs::create_dir_all(&target).unwrap();
        let link = base.join("alias");
        #[cfg(windows)]
        let ok = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .is_ok_and(|o| o.status.success());
        #[cfg(unix)]
        let ok = std::os::unix::fs::symlink(&target, &link).is_ok();
        assert!(ok, "directory-alias fixture could not be built");
        let photo = target.join("_c1_registry.arw");
        std::fs::write(&photo, b"raw").unwrap();
        let via_alias = link.join("_c1_registry.arw");
        let (ck, lk) =
            (crate::store::photo_key(&via_alias), crate::store::photo_key_lexical(&via_alias));
        assert_ne!(ck, lk, "premise: the two keys differ through the junction");

        // The registry namespace is on-disk identity: a blanket rename that
        // moved it would abandon every existing claim and start reassigning
        // deliverable suffixes from 1.
        assert_eq!(
            EXPORT_REGISTRY_DIR, ".autoshop-export-registry",
            "the export registry namespace must keep the spelling users already have on disk"
        );
        let out_dir = base.join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        let stem = pipeline::stem(&via_alias);
        let registry_stem =
            if cfg!(windows) { stem.to_ascii_lowercase() } else { stem.to_string() };
        let group = out_dir
            .join(EXPORT_REGISTRY_DIR)
            .join(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(registry_stem.as_bytes()));
        std::fs::create_dir_all(&group).unwrap();
        // An older build claimed slot 1 under the lexical key.
        std::fs::write(group.join("1.owner"), format!("{lk}\nbreadcrumb\n")).unwrap();

        let slot = registered_export_out(&via_alias, "developed", "jpg", &out_dir).unwrap();
        assert_eq!(
            slot,
            export_slot_path(&out_dir, stem, 1, "developed", "jpg"),
            "the legacy-keyed slot is this photo's slot — no second slot is claimed"
        );
        assert!(!group.join("2.owner").exists(), "no fresh claim was minted");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// L02: the aggregate body budget — a small charge fits, and a charge
    /// that would pass the ceiling refuses (CAS, so concurrent holders can
    /// never overshoot together). Only concurrency-independent assertions:
    /// other tests drive real requests through read_json in parallel.
    /// Drop-refund is exercised by every one of those requests.
    #[test]
    fn body_budget_refuses_past_the_ceiling() {
        let mut a = BodyBudget(0);
        assert!(a.charge(1), "a 1-byte charge must fit");
        assert!(!a.charge(BODY_BUDGET), "own hold + the full ceiling must refuse");
    }

    /// L01: the If-Match gate. No header = unconditional (curl/CLI/E2E
    /// keep working); a matching tag — quoted or bare — admits; a stale tag,
    /// or an untaggable current state, refuses with 412 and writes nothing.
    #[test]
    fn a_write_without_the_revision_it_read_is_refused() {
        assert!(precondition_failed(None, Some("r1")).is_none());
        assert!(precondition_failed(Some("\"r1\""), Some("r1")).is_none());
        assert!(precondition_failed(Some("r1"), Some("r1")).is_none());
        let stale = precondition_failed(Some("\"r0\""), Some("r1")).expect("stale must refuse");
        assert_eq!(stale.status_code().0, 412);
        let untagged =
            precondition_failed(Some("\"r1\""), None).expect("untaggable must refuse");
        assert_eq!(untagged.status_code().0, 412);
        // RFC forms (Codex R12 #2): a list admits when ANY tag matches; a
        // weak tag never gates a write; `*` requires an EXISTING saved
        // state — "none" and untaggable both refuse it.
        assert!(precondition_failed(Some("\"r0\", \"r1\""), Some("r1")).is_none());
        let weak = precondition_failed(Some("W/\"r1\""), Some("r1")).expect("weak refuses");
        assert_eq!(weak.status_code().0, 412);
        assert!(precondition_failed(Some("*"), Some("r1")).is_none());
        let star_none = precondition_failed(Some("*"), Some("none")).expect("no save yet");
        assert_eq!(star_none.status_code().0, 412);
        let star_untagged = precondition_failed(Some("*"), None).expect("untaggable");
        assert_eq!(star_untagged.status_code().0, 412);
    }

    /// GUI review 2026-08-12 F5: both Settings clients submit every
    /// non-secret field on save, so the form they were FILLED from must be
    /// current. This server's `cfg` cache only swaps on its own POST — a web
    /// form filled from it round-tripped a boot-time snapshot, and saving any
    /// unrelated web field then erased whatever the desktop GUI had saved
    /// since the server started (`image_effort: ""` over a GUI-set tier).
    #[test]
    fn the_settings_form_is_filled_from_the_file_not_the_boot_snapshot() {
        use std::io::Read as _;
        use std::sync::atomic::AtomicU64;
        let mut boot = Config::load();
        boot.analysis_model = "boot-snapshot-sentinel-7f3a".into();
        let state = AppState {
            dir: RwLock::new(PathBuf::new()),
            raws: RwLock::new(Vec::new()),
            cfg: RwLock::new(boot),
            dir_gen: AtomicU64::new(0),
            installed_gen: AtomicU64::new(0),
            port: 0,
        };
        let resp = api_settings_get(&state).expect("settings GET");
        let mut body = String::new();
        resp.into_reader().read_to_string(&mut body).expect("read body");
        assert!(
            !body.contains("boot-snapshot-sentinel-7f3a"),
            "the GET served the server's cached snapshot, not the settings file"
        );
        assert!(body.contains("\"analysis\""), "sanity: the response shape is intact");
    }

    #[test]
    fn at_checked_binds_id_resolution_to_the_installed_generation() {
        use std::sync::atomic::AtomicU64;
        let state = AppState {
            dir: RwLock::new(PathBuf::new()),
            raws: RwLock::new(vec![PathBuf::from("D:/x/a.arw")]),
            cfg: RwLock::new(Config::load()),
            dir_gen: AtomicU64::new(9), // a scan in flight has CLAIMED 9…
            installed_gen: AtomicU64::new(3), // …but the listing still says 3
            port: 0,
        };
        assert!(state.at_checked(0, Some(3)).is_ok(), "current generation resolves");
        // The claim counter must never be the authority (the H5 window).
        assert!(state.at_checked(0, Some(9)).is_err(), "claimed-but-not-installed refused");
        assert!(state.at_checked(0, Some(2)).is_err(), "stale stamp refused");
        assert!(state.at_checked(0, None).is_err(), "missing stamp refused");
        assert!(state.at_checked(7, Some(3)).is_err(), "unknown id refused");
    }

    #[test]
    fn session_masters_are_issued_per_photo_and_per_run() {
        let dir = std::env::temp_dir().join(format!("autoshade-serve-test-masters-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Two photos with the SAME stem in different folders — the pre-registry
        // stem-prefix check accepted either's master for both.
        let photo_a = dir.join("trip-a").join("DSC001.ARW");
        let photo_b = dir.join("trip-b").join("DSC001.ARW");
        let master = dir.join("DSC001.retouch.png");
        std::fs::write(&master, b"px").unwrap();
        let claim = master.to_string_lossy().into_owned();
        // Nothing issued yet: an earlier run's (or forged) claim is refused.
        assert!(session_master(Some(&claim), &photo_a).is_err(), "unissued claim refused");
        issue_session_master(&photo_a, &master, true);
        let (_, generated) = session_master(Some(&claim), &photo_a)
            .expect("issued claim accepted")
            .expect("a real master, not None");
        assert!(generated, "the generated provenance rides the registry");
        assert!(
            session_master(Some(&claim), &photo_b).is_err(),
            "same stem, different folder: photo B never sees A's master"
        );
        // Absent / blank claims mean "no session master", never an error.
        assert!(session_master(None, &photo_a).unwrap().is_none());
        assert!(session_master(Some("  "), &photo_a).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The cross-origin guard is the ONLY thing standing between a hostile page
    /// and every POST route (including the one that repoints the AI endpoint,
    /// after which the next Analyze hands over the user's API key). A port-less
    /// authority is the scheme default, NOT a wildcard.
    #[test]
    fn a_portless_origin_is_port_80_and_not_a_wildcard() {
        // The hole: `http://localhost` was accepted by a server on 8080, so
        // anything served on loopback port 80 became same-origin.
        assert!(!loopback("localhost", 8080), "port-less origin is port 80, not ours");
        assert!(!loopback("127.0.0.1", 8080));
        assert!(!loopback("[::1]", 8080));
        // …and it IS ours when we really are on 80.
        assert!(loopback("localhost", 80));
        assert!(loopback("127.0.0.1", 80));
        // The ordinary cases stay exactly as before.
        assert!(loopback("127.0.0.1:8080", 8080));
        assert!(loopback("localhost:8080", 8080));
        assert!(loopback("[::1]:8080", 8080), "an IPv6 literal's own colons are bracketed");
        assert!(!loopback("localhost:3000", 8080), "a different port is a different origin");
        assert!(!loopback("evil.example:8080", 8080));
        assert!(!loopback("127.0.0.1.evil.example:8080", 8080), "suffix trick");
        assert!(!loopback("[::1]:80", 8080));
        // The POSITIVE bracketed case, which nothing pinned before: the old
        // parser cut `"[::1]"` at the literal's own last colon and produced the
        // name `"[:"`, so `!loopback("[::1]", 8080)` above passed because the
        // NAME failed, not because the port rule worked. Reverting the port fix
        // left that assertion green. This one does not survive it.
        assert!(loopback("[::1]", 80), "a port-less IPv6 literal is port 80, like any other");
        assert!(loopback("[::1]:8080", 8080));
        assert!(!loopback("[::1]", 8080));
        // Host names are case-insensitive; a browser lower-cases them, but our
        // own tooling and a hand-typed URL need not.
        assert!(loopback("LOCALHOST:8080", 8080));
        assert!(loopback("LocalHost", 80));
        // Malformed bracket forms are refused outright rather than re-parsed.
        assert!(!loopback("[::1", 8080));
        assert!(!loopback("[::1]x", 8080));
        assert!(!loopback("[::1]:8080x", 8080));
        // A bracketed literal that is not loopback stays refused.
        assert!(!loopback("[::2]:8080", 8080));
        assert!(!loopback("[2001:db8::1]:8080", 8080));
    }

    /// A request permit must come back even when the handler PANICS: releasing
    /// it after the call unwound past the release, and eight panics wedged the
    /// accept loop forever with no error anywhere.
    #[test]
    fn a_panicking_handler_still_gives_its_request_slot_back() {
        let gate = std::sync::Arc::new((std::sync::Mutex::new(0usize), std::sync::Condvar::new()));
        let count = || *gate.0.lock().unwrap_or_else(|p| p.into_inner());
        for _ in 0..3 {
            let g = std::sync::Arc::clone(&gate);
            let h = std::thread::spawn(move || {
                let _permit = Permit::acquire(g, 8);
                panic!("a third-party parser met a malformed file");
            });
            assert!(h.join().is_err(), "the thread really did panic");
            assert_eq!(count(), 0, "the slot came back through Drop, not through the tail");
        }
        // The gate is still usable: a taken permit occupies exactly one slot.
        let p = Permit::acquire(std::sync::Arc::clone(&gate), 8);
        assert_eq!(count(), 1);
        drop(p);
        assert_eq!(count(), 0);

        // And now the half the counter cannot see. Everything above runs with
        // `max = 8` and one permit at a time, so no thread ever enters
        // `cv.wait` — delete `cv.notify_one()` from Drop and every assertion
        // above still passes, while the real server wedges permanently after
        // eight panics. That is the exact bug this test is named for, so pin
        // the WAKE-UP: with `max = 1` a second acquirer must block, and only a
        // notify from the panicking permit's Drop can release it.
        let gate1 = std::sync::Arc::new((std::sync::Mutex::new(0usize), std::sync::Condvar::new()));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (woke_tx, woke_rx) = std::sync::mpsc::channel();
        let holder = {
            let g = std::sync::Arc::clone(&gate1);
            std::thread::spawn(move || {
                let _permit = Permit::acquire(g, 1);
                ready_tx.send(()).unwrap();
                // Give the waiter time to actually reach `cv.wait`. This orders
                // the two threads, it does not paper over a race: if the waiter
                // has not blocked yet it would sail through on the count alone
                // and the missing-notify mutant would survive. The assertion
                // below is a hard timeout, not a sleep-and-hope.
                std::thread::sleep(std::time::Duration::from_millis(150));
                panic!("a third-party parser met a malformed file");
            })
        };
        ready_rx.recv().expect("the holder took the only slot");
        let waiter = {
            let g = std::sync::Arc::clone(&gate1);
            std::thread::spawn(move || {
                let permit = Permit::acquire(g, 1); // blocks: the slot is taken
                woke_tx.send(()).unwrap();
                drop(permit);
            })
        };
        assert!(holder.join().is_err(), "the holder really did panic");
        woke_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the waiter was never woken — Drop decremented but did not notify");
        waiter.join().expect("the waiter finished");
        assert_eq!(
            *gate1.0.lock().unwrap_or_else(|p| p.into_inner()),
            0,
            "both permits came back"
        );
    }

    /// The fresh-open calibration reaches the client through TWO doors
    /// (`/api/fresh-base`, and `/api/recipe`'s 404 body) and they must carry
    /// the SAME key set — a key added to one door only is a photo whose
    /// calibration depends on how the client asked for it. One producer is the
    /// enforcement; this pins the key set that producer emits.
    #[test]
    fn the_fresh_open_calibration_is_one_key_set_for_both_doors() {
        let dir = std::env::temp_dir().join(format!("autoshade-serve-freshbase-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let baked = dir.join("baked.png");
        image::DynamicImage::ImageRgb8(image::RgbImage::new(4, 3)).save(&baked).unwrap();

        let v: serde_json::Value =
            serde_json::from_str(&fresh_base_payload(&baked)).expect("the payload is JSON");
        let mut keys: Vec<&str> = v.as_object().expect("an object").keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["as_shot_k", "as_shot_tint", "base_curve", "lens_profile"],
            "the fresh-open contract is exactly these four keys"
        );
        // A baked source has no camera rendition and no as-shot reading: the
        // keys are still PRESENT (the client branches on null, not on absence).
        assert!(v["base_curve"].as_array().expect("an array").is_empty());
        assert!(v["as_shot_k"].is_null() && v["as_shot_tint"].is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The A6 rule for DELIVERABLES, now that export and download share one
    /// resolver: a broken master link must REFUSE rather than quietly ship the
    /// un-retouched source. Nothing pinned it before the two handlers were
    /// merged into `deliverable_source`.
    #[test]
    fn a_deliverable_refuses_a_broken_master_instead_of_lying() {
        let dir = std::env::temp_dir().join(format!("autoshade-serve-deliv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let raw = dir.join("DSC_DELIV.ARW");
        std::fs::write(&raw, b"raw").unwrap();
        let dev = crate::store::develop_dir(&raw);
        let _ = std::fs::remove_dir_all(&dev);
        let req = || DevelopReq {
            id: 0,
            recipe: EditRecipe::default(),
            denoise: false,
            denoise_strength: None,
            format: None,
            master: None,
        };

        // Nothing recorded: the RAW itself is the source, and the call
        // succeeds — the baseline the refusals below must be measured against.
        let (src, _note) =
            deliverable_source(&mut req(), &raw).unwrap_or_else(|_| panic!("no master = no refusal"));
        assert_eq!(src, raw);

        // A RECORDED master that is gone: server-side state, so 500 — and
        // never a silent fall back to the un-retouched RAW.
        let gone = dir.join("gone.retouch.png");
        std::fs::write(&gone, b"png").unwrap();
        crate::store::write_pixel_source(&raw, &gone, false).unwrap();
        std::fs::remove_file(&gone).unwrap();
        let err = deliverable_source(&mut req(), &raw)
            .err()
            .unwrap_or_else(|| panic!("a dead master link must refuse the deliverable"));
        assert_eq!(err.status_code().0, 500, "server-side state → 500");

        // A claim this run never issued is the CLIENT's fault → 400.
        let forged = dir.join("forged.retouch.png");
        std::fs::write(&forged, b"png").unwrap();
        let mut r = req();
        r.master = Some(forged.to_string_lossy().into_owned());
        let err = deliverable_source(&mut r, &raw)
            .err()
            .unwrap_or_else(|| panic!("an unissued claim must be refused"));
        assert_eq!(err.status_code().0, 400, "unissued/forged claim → 400");

        let _ = std::fs::remove_dir_all(&dev);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn percent_encode_is_ascii_and_round_trips() {
        // The whole point: whatever goes into a header value is pure ASCII, so
        // `Header::from_bytes` can never reject it (that rejection + `.unwrap()`
        // is what killed Download / Fill / Heal on non-ASCII photo names).
        for s in ["DSC09528.developed.tif", "测试照片.developed.tif", "a b%c.jpg", "D:\\照片\\x.png"] {
            let enc = percent_encode(s);
            assert!(enc.is_ascii(), "{enc} is not ASCII");
            assert_eq!(percent_decode(&enc), s, "round trip failed for {s}");
        }
        assert_eq!(percent_encode("a b.jpg"), "a%20b.jpg");
        assert_eq!(percent_encode("~-_."), "~-_."); // unreserved set passes through
    }

    #[test]
    fn percent_decode_unicode_and_literals() {
        assert_eq!(percent_decode("DSC09528.ARW"), "DSC09528.ARW"); // ASCII untouched
        // encodeURIComponent("测试照片.png")
        assert_eq!(percent_decode("%E6%B5%8B%E8%AF%95%E7%85%A7%E7%89%87.png"), "测试照片.png");
        assert_eq!(percent_decode("a%20b.jpg"), "a b.jpg"); // %20 = space
        assert_eq!(percent_decode("100%25.png"), "100%.png"); // literal percent round-trips
        assert_eq!(percent_decode("bad%ZZ"), "bad%ZZ"); // invalid escape passes through
        assert_eq!(percent_decode("tail%E6"), "tail\u{fffd}"); // truncated UTF-8 → replacement
    }

        #[test]
        fn cross_site_fetches_and_stale_image_links_are_refused_without_breaking_local_tools() {
            assert!(fetch_site_is_cross_site(Some("cross-site")));
            assert!(fetch_site_is_cross_site(Some(" Cross-Site ")));
            assert!(!fetch_site_is_cross_site(Some("same-origin")));
            assert!(
                !fetch_site_is_cross_site(None),
                "curl and other non-browser clients omit Fetch Metadata"
            );

            let token = "this-server-session";
            let html = tokenized_index(token);
            assert!(html.contains("/api/thumb?token=this-server-session&id="));
            assert!(html.contains("/api/preview?token=this-server-session&id="));
            assert!(!html.contains("/api/thumb?id="));
            assert!(!html.contains("/api/preview?id="));

            assert!(image_token_matches(
                "/api/thumb?token=this-server-session&id=3",
                token
            ));
            assert!(!image_token_matches("/api/thumb?id=3", token));
            assert!(!image_token_matches(
                "/api/thumb?token=an-earlier-session&id=3",
                token
            ));

            let first = session_token().unwrap();
            let second = session_token().unwrap();
            assert_eq!(first.len(), 43, "32 random bytes in unpadded base64url");
            assert!(first.is_ascii());
            assert_ne!(first, second, "each server start gets a fresh capability");
        }

        #[test]
        fn same_stem_exports_keep_their_persistent_slots_and_preserve_unknown_artifacts() {
            let dir = std::env::temp_dir().join(format!(
                "autoshade-serve-export-registry-{}-{}",
                std::process::id(),
                line!()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            let out = dir.join("out");

            let photo_a = dir.join("roll-a").join("DSC001.ARW");
            let photo_b = dir.join("roll-b").join("DSC001.ARW");
            let a = registered_export_out(&photo_a, "developed", "tif", &out).unwrap();
            assert_eq!(a, out.join("DSC001.developed.tif"));
            std::fs::write(&a, b"photo A").unwrap();

            let b = registered_export_out(&photo_b, "developed", "tif", &out).unwrap();
            assert_eq!(b, out.join("DSC001 (2).developed.tif"));
            // Simulate the opposite request order after a restart: ownership is
            // read from disk, not reassigned from encounter order.
            assert_eq!(
                registered_export_out(&photo_b, "developed", "tif", &out).unwrap(),
                b
            );
            assert_eq!(
                registered_export_out(&photo_a, "developed", "tif", &out).unwrap(),
                a
            );

            // A pre-registry artifact has no trustworthy photo identity. It stays
            // untouched and permanently reserves the bare slot.
            let unknown = out.join("DSC002.developed.tif");
            std::fs::write(&unknown, b"an earlier unknown artifact").unwrap();
            let photo_c = dir.join("roll-c").join("DSC002.ARW");
            assert_eq!(
                registered_export_out(&photo_c, "developed", "tif", &out).unwrap(),
                out.join("DSC002 (2).developed.tif")
            );
            assert_eq!(
                std::fs::read(&unknown).unwrap(),
                b"an earlier unknown artifact"
            );

            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn an_ephemeral_bind_reports_and_checks_the_actual_port() {
            let server = Server::http("127.0.0.1:0").unwrap();
            let port = server_port(&server).unwrap();
            assert_ne!(port, 0, "the OS assigned a usable ephemeral port");
            assert!(
                loopback(&format!("127.0.0.1:{port}"), port),
                "the authority printed to the browser must pass the same origin guard"
            );
        }
}
