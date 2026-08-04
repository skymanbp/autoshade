//! Local web UI server. `autoshop serve <dir>` starts a tiny HTTP server; open
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

struct AppState {
    /// The working directory the gallery lists from. Behind a lock so the UI can
    /// switch folders at runtime (POST /api/setdir re-scans and swaps it + `raws`).
    dir: RwLock<PathBuf>,
    /// The source list, mutable so the UI can import more / switch folders at runtime.
    raws: RwLock<Vec<PathBuf>>,
    /// Config behind a lock so the Settings panel can hot-reload it (POST
    /// /api/settings rewrites the local file, then swaps in a fresh `Config`).
    cfg: RwLock<Config>,
    /// Folder-switch generation: each /api/setdir claims the next number
    /// BEFORE scanning and only installs its result while still current —
    /// without this, a SLOW scan of folder A finishing after a fast scan of
    /// folder B silently swapped the server back to A under B's UI. It also
    /// STAMPS every listing, so a request built for the old folder cannot
    /// resolve its index against the new one (see `stale_generation`).
    dir_gen: std::sync::atomic::AtomicU64,
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
}

pub fn serve(dir: &Path, port: u16) -> Result<()> {
    // Sources = RAWs + already-baked PNG/TIFF/JPEG (the PNG-source edit mode).
    let raws = pipeline::find_sources(dir)?;
    let n = raws.len();
    let state = Arc::new(AppState {
        dir: RwLock::new(dir.to_path_buf()),
        raws: RwLock::new(raws),
        cfg: RwLock::new(Config::load()),
        dir_gen: std::sync::atomic::AtomicU64::new(0),
        port,
    });
    let addr = format!("127.0.0.1:{port}");
    let server = Server::http(&addr).map_err(|e| anyhow!("start server on {addr}: {e}"))?;
    // Reclaim download/mask temp files a previous run failed to unlink —
    // Windows has no automatic %TEMP% cleaner, so nobody else ever would.
    sweep_stale_temp_files();
    println!("Autoshop UI: {n} source(s) under {}", dir.display());
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
        let gate = Arc::clone(&gate);
        {
            let (lock, cv) = &*gate;
            let mut n = lock.lock().unwrap_or_else(|p| p.into_inner());
            while *n >= MAX_CONCURRENT {
                n = cv.wait(n).unwrap_or_else(|p| p.into_inner());
            }
            *n += 1;
        }
        std::thread::spawn(move || {
            if let Err(e) = handle(request, &state) {
                eprintln!("request error: {e}");
            }
            let (lock, cv) = &*gate;
            let mut n = lock.lock().unwrap_or_else(|p| p.into_inner());
            *n = n.saturating_sub(1);
            cv.notify_one();
        });
    }
    Ok(())
}

/// Read a request header (case-insensitive), if present.
fn req_header(request: &Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

/// Refuse anything a HOSTILE WEB PAGE could send us.
///
/// Binding 127.0.0.1 protects nothing on its own: any page the user visits
/// can POST to `http://127.0.0.1:<port>` without a preflight (simple
/// requests), and a DNS-rebinding host — a name that resolves to the
/// attacker first and to 127.0.0.1 seconds later — becomes "same origin" and
/// can READ the answers too (gallery paths, photos, settings incl. the API
/// key). Both attacks necessarily carry a foreign `Host` (the rebound name)
/// or a foreign `Origin`, so requiring the literal loopback host and a
/// same-origin (or absent — same-origin GETs and our own tooling) Origin
/// removes the entire class. Returns the refusal to send, or None to accept.
fn foreign_origin(request: &Request, port: u16) -> Option<ResponseBox> {
    fn loopback(h: &str, port: u16) -> bool {
        let h = h.trim();
        // "127.0.0.1:8080" / "localhost" / "[::1]:8080"
        let (name, p) = match h.rsplit_once(':') {
            // An IPv6 literal's own colons live INSIDE the brackets.
            Some((n, p)) if !n.ends_with(']') || p.chars().all(|c| c.is_ascii_digit()) => {
                (n, Some(p))
            }
            _ => (h, None),
        };
        let name_ok = matches!(name, "127.0.0.1" | "localhost" | "[::1]");
        let port_ok = p.is_none_or(|p| p == port.to_string());
        name_ok && port_ok
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
                "refused: cross-origin requests are not accepted by the local Autoshop server",
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
/// (`X-Autoshop-Gen`, see index.html); a mismatch means the gallery moved
/// under it and the id now points at another photo.
fn stale_generation(request: &Request, state: &AppState) -> Option<ResponseBox> {
    let cur = state.dir_gen.load(std::sync::atomic::Ordering::SeqCst);
    match req_header(request, "X-Autoshop-Gen").and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(g) if g == cur => None,
        _ => Some(status_response(
            409,
            "the gallery moved to another folder — reload the page and try again",
        )),
    }
}

fn handle(mut request: Request, state: &AppState) -> Result<()> {
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("/");
    let is_post = request.method() == &tiny_http::Method::Post;

    if let Some(refusal) = foreign_origin(&request, state.port) {
        return request.respond(refusal).map_err(Into::into);
    }
    if id_bound_mutation(path)
        && let Some(stale) = stale_generation(&request, state)
    {
        return request.respond(stale).map_err(Into::into);
    }

    // Handlers BUILD a response instead of consuming the request, so the request
    // is still ours when one of them fails.
    let reply = match (is_post, path) {
        (false, "/") => Ok(html_response(INDEX_HTML)),
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
        let status = if e.downcast_ref::<ClientErr>().is_some() { 400 } else { 500 };
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

/// Serializes the backup-gate + sidecar writes WITHIN this process: two
/// threaded requests saving the same photo could interleave between
/// `backup_saved_develop` and `write_recipe`, overwriting a save that never
/// got its v<N> snapshot. (Cross-PROCESS races remain the recorded v0.13
/// trade-off.)
static SAVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    // Read the generation BEFORE the list: /api/setdir bumps it, then scans,
    // then installs. Reading it first can only under-report (items newer than
    // the stamp -> the client's next mutation 409s and it reloads); reading it
    // after could stamp OLD items with the NEW generation, which would let a
    // stale id through — fail closed, never open.
    let listing_gen = state.dir_gen.load(std::sync::atomic::Ordering::SeqCst);
    let raws = state.raws.read().map_err(|_| anyhow!("lock poisoned"))?;
    let total = raws.len();
    let items: Vec<_> = raws
        .iter()
        .enumerate()
        .skip(offset)
        .take(limit)
        .map(|(id, raw)| {
            // Central store OR legacy ./out, recipe OR XMP — pre-migration
            // libraries keep their "analyzed" badges.
            let analyzed = crate::store::has_develop(raw);
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
    let mut bytes = Vec::new();
    // Bounded manual read loop (Take<T> on a trait object fights the method
    // resolver): refuse — never truncate — anything past the cap.
    let reader = request.as_reader();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut chunk).context("read upload body")?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() > MAX_UPLOAD {
            return Ok(status_response(413, "upload exceeds the 500 MB limit"));
        }
    }

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
        let full = if decode::is_raw(raw) {
            // Develop AT the preview edge (the cap runs before tone/geometry)
            // — the old full-sensor develop existed only to be thumbnailed.
            render::render_to_image(raw, &EditRecipe::default(), None, Some(PREVIEW_EDGE))?
        } else {
            decode::load_image(raw)?
        };
        // `thumbnail` (not `resize`) for the big downscale, like the GUI — and
        // only ever DOWN: a source already under the edge is left alone, since
        // its own pixels beat any upsample and the browser scales the <img>
        // anyway. Store 8-bit: `develop_preview` and the JPEG encode both work in
        // 8-bit, so keeping the 16-bit buffer would only double the cache and
        // re-convert on every gesture.
        let fitted = if full.width().max(full.height()) > PREVIEW_EDGE {
            full.thumbnail(PREVIEW_EDGE, PREVIEW_EDGE)
        } else {
            full
        };
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
            let est = render::estimation_base(&neutral, &pipeline::fresh_lens_profile(raw));
            render::camera_base_knots(&est, &camera)
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
    // Under SAVE_LOCK: migration MUTATES the central store, and racing a
    // concurrent /api/xmp neutral-clear could republish the legacy recipe
    // right after the clear deleted both homes — resurrecting the cleared
    // edit. The lock also keeps the read below coherent with any in-flight
    // save.
    let _save = SAVE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // One-time, per-photo migration of pre-store ./out sidecars into the
    // central develop dir (no-op when nothing legacy remains).
    crate::store::migrate_legacy(&raw);
    // The sidecar BESIDE the RAW is the one file Lightroom itself writes —
    // newest intent wins, the same contract as the GUI's read_saved_develop
    // (store::lightroom_sidecar: our own copied projection is skipped, ties
    // go to the store). The header says where the recipe came from; the BODY
    // stays a pure EditRecipe — clients post recipes back to /api/develop,
    // where an unknown field is a 422.
    match crate::store::lightroom_sidecar(&raw) {
        crate::store::LrSidecar::NewerThanStore(text) | crate::store::LrSidecar::Only(text) => {
            let mut r = crate::xmp::xmp_to_recipe(&text);
            if !r.is_noop() {
                r.clamp();
                // Same stamp rule as the XMP fallback below: Lightroom tuned
                // this file over its own profile-corrected base.
                r.base_curve = fresh_base_knots(&raw);
                r.lens_profile = pipeline::fresh_lens_profile(&raw);
                let h = Header::from_bytes(&b"X-Recipe-Source"[..], &b"lightroom-sidecar"[..])
                    .expect("static ASCII header");
                return Ok(json_text(serde_json::to_string(&r)?).with_header(h));
            }
        }
        _ => {}
    }
    // Central first; then any legacy file a failed migration left behind.
    let mut parse_err: Option<String> = None;
    for path in [crate::store::recipe_target(&raw), crate::store::legacy_recipe(&raw)] {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
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
        match serde_json::from_str::<serde_json::Value>(&text) {
            Err(e) => {
                parse_err = Some(e.to_string());
                continue;
            }
            // `null`, an array or a bare scalar parses as Value yet can never
            // be a recipe of ANY schema generation — that is corruption, not
            // a newer schema (which is still an object and goes out verbatim).
            Ok(v) if !v.is_object() => {
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
        return Ok(json_text(text));
    }
    // No recipe.json → fall back to the XMP sidecar, exactly like the GUI's
    // `read_saved_develop`: a foreign / neutral sidecar parses to a no-op recipe,
    // and "restoring" that would only produce a misleading SAVED tag. An XMP
    // was tuned in Lightroom over ITS camera-profile base, so it gets the
    // photo's fresh base look — the same rule the GUI applies on open.
    for path in [pipeline::xmp_target(&raw), crate::store::legacy_xmp(&raw)] {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let mut r = crate::xmp::xmp_to_recipe(&text);
            if !r.is_noop() {
                r.clamp();
                r.base_curve = fresh_base_knots(&raw);
                // Same stamp rule as the GUI's XMP-only restore: Lightroom
                // tuned this file over its own profile-corrected base.
                r.lens_profile = pipeline::fresh_lens_profile(&raw);
                return Ok(json_text(serde_json::to_string(&r)?));
            }
        }
    }
    // A damaged recipe.json with nothing else to fall back on is an ERROR, not
    // "no recipe yet" — say so, or the UI silently pretends the photo is unedited.
    if let Some(err) = parse_err {
        return Ok(status_response(422, &format!("recipe.json is unreadable: {err}")));
    }
    // Still 404 ("not analyzed yet" drives the client's fresh-open UI state),
    // but the body carries the photo's camera-matched base look so the fresh
    // web canvas starts as bright as a fresh GUI open instead of jumping
    // only after the first Analyze.
    let body = serde_json::json!({
        "base_curve": fresh_base_knots(&raw),
        "lens_profile": pipeline::fresh_lens_profile(&raw),
    })
    .to_string();
    let ct = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .expect("static ASCII header");
    Ok(Response::from_string(body).with_status_code(404).with_header(ct).boxed())
}

/// The photo's FRESH camera-matched base-look knots, regardless of any saved
/// recipe — the web client's Reset needs them to mean "the fresh-open look"
/// (the GUI's `photo_knots` rule): a legacy save deliberately carries an
/// empty curve, and preserving that on Reset kept the photo dark. Cheap after
/// the first call — `fresh_base_knots` reuses the cached `develop_base`.
fn api_fresh_base(request: &Request, state: &AppState) -> Result<ResponseBox> {
    let raw = raw_for(request, state)?;
    let body = serde_json::json!({
        "base_curve": fresh_base_knots(&raw),
        "lens_profile": pipeline::fresh_lens_profile(&raw),
    })
    .to_string();
    Ok(json_text(body))
}

/// Style-library info for the UI's info box: is an index built, how many of the
/// user's edits it holds, and the scene "tags" it covers. Instant (just reads the
/// JSON; no per-photo decode).
fn api_style_info(state: &AppState) -> Result<ResponseBox> {
    let abs = |p: &str| {
        std::path::absolute(p).map(|x| x.display().to_string()).unwrap_or_else(|_| p.to_string())
    };
    // Style reference library status (built? how many edits? scene tags?).
    // Central store first; a legacy cwd-relative index (built before the store
    // existed) still counts — and we report whichever file actually answered.
    let ix_path = crate::store::style_index_path();
    let loaded = crate::style::StyleIndex::load(&ix_path)
        .map(|ix| (ix, ix_path.display().to_string()))
        .or_else(|_| {
            crate::style::StyleIndex::load(Path::new("out/style-index.json"))
                .map(|ix| (ix, abs("out/style-index.json")))
        });
    let style = match loaded {
        Ok((ix, index_file)) => {
            let mut tags: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
            for e in &ix.exemplars {
                *tags.entry(e.tag.clone()).or_default() += 1;
            }
            let mut top: Vec<_> = tags.into_iter().collect();
            top.sort_by(|a, b| b.1.cmp(&a.1));
            top.truncate(6);
            let scenes: Vec<_> = top.into_iter().map(|(t, n)| json!({ "tag": t, "n": n })).collect();
            json!({ "built": true, "total": ix.exemplars.len(), "scenes": scenes,
                    "index_file": index_file, "source_dir": ix.source_dir })
        }
        Err(_) => json!({ "built": false }),
    };
    Ok(json_response(&json!({
        // Where the photos being browsed live (the "原图库"), where RENDERED
        // outputs land (the "成片库" = ./out), where the develop store keeps
        // recipes / XMP / versions, and the style-library status. out_dir and
        // store_dir are DIFFERENT places — the XMP has lived in the store since
        // the sidecars moved out of ./out.
        "working_dir": state.dir_display(),
        "working_count": state.count(),
        "out_dir": abs("out"),
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
/// the central store's style-index.json (same as `autoshop style-index <dir>`).
/// Decodes every RAW, so it can take minutes on a large library.
fn api_style_build(request: &mut Request) -> Result<ResponseBox> {
    let req: StyleBuildReq = read_json(request)?;
    let cleaned = req.dir.trim().trim_matches('"').trim();
    let p = PathBuf::from(cleaned);
    if !p.is_dir() {
        return Ok(status_response(400, &format!("not a folder: {cleaned}")));
    }
    let index = match crate::style::StyleIndex::build(&p) {
        Ok(ix) => ix,
        Err(e) => return Ok(status_response(500, &format!("build failed: {e}"))),
    };
    let total = index.exemplars.len();
    // An empty build is a FAILURE, not a success: `save` truncates the file in
    // place, so writing it would silently replace a good index (and every
    // surface's Style slider goes inert with nothing to say why). A folder
    // yields 0 whenever no RAW has its Lightroom .xmp SITTING BESIDE IT — and
    // Autoshop never puts one there: its own XMP projection lives in the
    // per-user develop store. Point this at the folder you edit in LIGHTROOM.
    // Refuse, and leave the previous index alone.
    if total == 0 {
        return Ok(status_response(
            400,
            &format!(
                "nothing indexed in {} — found no RAW with its .xmp sidecar beside it \
                 (Autoshop keeps its own .xmp in the develop store, never beside the \
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
#[derive(Deserialize)]
struct DevelopReq {
    id: usize,
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
/// (U30). UNTRUSTED input: accept only a path that canonicalises INSIDE the
/// project's ./out tree (where every fill/heal master lives). Anything else
/// is a hard error — falling back silently would re-introduce the exact
/// stealth loss this exists to fix.
fn session_master(
    claim: Option<&str>,
    raw: &Path,
) -> std::result::Result<Option<PathBuf>, String> {
    let Some(c) = claim else { return Ok(None) };
    if c.trim().is_empty() {
        return Ok(None);
    }
    let canon = std::fs::canonicalize(c)
        .map_err(|e| format!("session master {c} is not readable ({e}) — reselect the photo and retry"))?;
    let root = std::fs::canonicalize("out")
        .map_err(|e| format!("no ./out tree ({e}) — reselect the photo and retry"))?;
    if !canon.starts_with(&root) {
        return Err(format!("session master {c} is outside ./out — reselect the photo and retry"));
    }
    // The master must BELONG to this photo: every retouch master is named by
    // its photo's stem (unique_out → default_out gives "<stem>.<tag>.png"),
    // so a claim whose file name does not start with "<stem>." carries some
    // OTHER photo's pixels — replaying it here would graft photo A's frame
    // onto photo B's develop, and Save would persist that graft.
    let stem = pipeline::stem(raw);
    let name = canon.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !name.starts_with(&format!("{stem}.")) {
        return Err(format!(
            "session master {c} does not belong to this photo — reselect the photo and retry"
        ));
    }
    Ok(Some(canon))
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
    for (vx, vy) in corners {
        let (ox, oy) = render::view_to_original_norm(
            vx,
            vy,
            dims,
            rec.straighten_deg,
            &rec.lens_profile,
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
/// Mirrors render_source's strip on the mapping side.
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
    rec.straighten_deg != 0.0
        || rec.lens_distortion != 0.0
        || (rec.lens_profile.distortion_on && !rec.lens_profile.distortion.is_empty())
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
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let mut out = image::RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (vx, vy) = render::original_to_view_norm(
                (x as f32 + 0.5) / w as f32,
                (y as f32 + 0.5) / h as f32,
                dims,
                rec.straighten_deg,
                &rec.lens_profile,
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
/// normalised geometry maps, so preview-scale dims are exact. RAW: the
/// oriented embedded preview (a decode, but analyze is already a multi-second
/// AI call); baked sources: a header-only dimension read.
fn source_dims(raw: &Path) -> Option<(f32, f32)> {
    if decode::is_raw(raw) {
        let img = decode::embedded_preview(raw).ok()??;
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
    let req: AnalyzeReq = read_json(request)?;
    let raw = state.at(req.id).ok_or_else(|| anyhow!(ClientErr("bad id".into())))?;
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
    // The analyze pipeline proposes/verifies over the camera's embedded
    // preview, where the base look AND the lens corrections are already IN
    // the pixels: strip both from the refine base (either would double-apply
    // in the verifier's render). produce_recipe stamps the photo's own back
    // onto the result — the single authority for every surface.
    let refine_base = req.base.clone().map(|mut b| {
        b.base_curve = Vec::new();
        b.lens_profile = Default::default();
        b
    });
    let (recipe, verdict) =
        pipeline::produce_recipe(&raw, &cfg, false, guidance, refine_base.as_ref(), style)?;
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
    let _save = SAVE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    match crate::store::backup_saved_develop(&raw, Some(&recipe)) {
        Ok(backed_up) => {
            pipeline::write_recipe(&raw, &recipe, None)?;
            // The recipe write ALONE decides the saved state (the GUI/CLI
            // rule): a failed XMP projection degrades to a warning — the old
            // `?` answered 500 and hid the committed save from the browser.
            let mut body = json!({ "recipe": recipe, "verdict": verdict, "saved": true });
            if decode::is_raw(&raw)
                && let Err(e) = pipeline::write_xmp(&raw, &recipe)
            {
                body["warning"] =
                    json!(format!("saved, but the Lightroom XMP projection failed: {e:#}"));
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
    }
}

/// The photo's RENDER INPUT: the persisted pixels.json master when the store
/// records one (a GUI-saved heal/denoise/reimagine — the web used to silently
/// render the un-retouched source while the GUI showed the master), else the
/// photo itself. A GENERATED master carries the look in its pixels, so the
/// recipe drops base_curve + lens_profile (the same strip rule every GUI
/// render path applies).
use crate::store::render_source;

fn api_develop(request: &mut Request, state: &AppState) -> Result<ResponseBox> {
    let mut req: DevelopReq = read_json(request)?;
    let raw = state.at(req.id).ok_or_else(|| anyhow!(ClientErr("bad id".into())))?;
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
    // persisted) overrides the persisted source; such masters are NEUTRAL
    // develops (InPlace), so the recipe — calibration included — renders on
    // top, no strip.
    let src = match session_master(req.master.as_deref(), &raw) {
        Ok(Some(p)) => p,
        Ok(None) => render_source(&raw, &mut req.recipe),
        Err(msg) => return Ok(status_response(400, &msg)),
    };
    let preview = develop_base(&src)?;
    let mut after = render::develop_preview(&preview, &req.recipe);
    // Geometry, mirroring the GUI preview chain (lens geometry → straighten;
    // the frame stays uncropped for whole-frame slider feedback, same policy).
    // The web pane previously skipped ALL of it — a recipe with distortion or
    // straighten previewed one framing and exported another.
    if req.recipe.lens_profile.geometry_active() || req.recipe.lens_distortion != 0.0 {
        after =
            render::apply_lens_geometry(&after, &req.recipe.lens_profile, req.recipe.lens_distortion);
    }
    if req.recipe.straighten_deg != 0.0 {
        after = render::rotate_straighten(&after, req.recipe.straighten_deg);
    }
    jpeg_response(&after)
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

/// Export to ./out (the library stays read-only). Returns the written path.
fn api_export(request: &mut Request, state: &AppState) -> Result<ResponseBox> {
    // Full-resolution work — see HEAVY. Held for the whole handler.
    let _heavy = HEAVY.lock().unwrap_or_else(|p| p.into_inner());

    let mut req: DevelopReq = read_json(request)?;
    let raw = state.at(req.id).ok_or_else(|| anyhow!(ClientErr("bad id".into())))?;
    req.recipe.clamp(); // untrusted network input — see api_develop
    crate::store::resolve_mask_paths(&mut req.recipe, &crate::store::develop_dir(&raw));
    // Session master wins here too (api_develop's rule) — the deliverable
    // must match the healed preview the user just approved.
    let src = match session_master(req.master.as_deref(), &raw) {
        Ok(Some(p)) => p,
        Ok(None) => render_source(&raw, &mut req.recipe),
        Err(msg) => return Ok(status_response(400, &msg)),
    };
    let out = pipeline::default_out(&raw, "developed", fmt_ext(&req));
    pipeline::ensure_parent(&out)?;
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
    if let Err(e) = render::render_to_file(&src, &req.recipe, &tmp, denoise_opts(&req, &cfg).as_ref(), None)
    {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, &out) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("publish export {}", out.display()));
    }
    Ok(text_response(&out.display().to_string()))
}

/// Render and stream the image back as a download (browser "Save As"), without
/// leaving a copy in ./out. Renders to a temp file, then streams + deletes it.
fn api_download(request: &mut Request, state: &AppState) -> Result<ResponseBox> {
    // Full-resolution work — see HEAVY. Held for the whole handler.
    let _heavy = HEAVY.lock().unwrap_or_else(|p| p.into_inner());

    let mut req: DevelopReq = read_json(request)?;
    let raw = state.at(req.id).ok_or_else(|| anyhow!(ClientErr("bad id".into())))?;
    req.recipe.clamp(); // untrusted network input — see api_develop
    crate::store::resolve_mask_paths(&mut req.recipe, &crate::store::develop_dir(&raw));
    // Session master wins here too (api_develop's rule).
    let src = match session_master(req.master.as_deref(), &raw) {
        Ok(Some(p)) => p,
        Ok(None) => render_source(&raw, &mut req.recipe),
        Err(msg) => return Ok(status_response(400, &msg)),
    };
    // Config SNAPSHOT — see api_export.
    let cfg = state.config().clone();
    let ext = fmt_ext(&req);
    let tmp = std::env::temp_dir().join(format!(
        "autoshop_dl_{}_{}.{ext}",
        std::process::id(),
        DL_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    // A failed render must not leave a multi-hundred-MB partial file to sit
    // in %TEMP% until the next server start's sweep.
    if let Err(e) =
        render::render_to_file(&src, &req.recipe, &tmp, denoise_opts(&req, &cfg).as_ref(), None)
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

/// Best-effort sweep of leftover `autoshop_dl_*` / `autoshop_mask_*` temp
/// files from previous runs (crash before unlink, or an unlink refused by a
/// scanner's non-sharing handle). Age-gated at one hour so an in-flight
/// download owned by a parallel server instance is never touched.
fn sweep_stale_temp_files() {
    let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else { return };
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if !(name.starts_with("autoshop_dl_")
            || name.starts_with("autoshop_mask_")
            || name.starts_with("autoshop_heal_"))
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
    let req: XmpReq = read_json(request)?;
    let raw = state.at(req.id).ok_or_else(|| anyhow!(ClientErr("bad id".into())))?;
    // Same intra-process save serialization as api_analyze.
    let _save = SAVE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
    if req.recipe.is_noop() {
        // A file already missing IS the desired end state; any other removal
        // failure (lock, permissions) must not let us claim the edits were
        // cleared — the surviving sidecar would resurrect them on reopen.
        let del = |p: &Path| match std::fs::remove_file(p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Some(e),
            _ => None,
        };
        let mut first_err: Option<std::io::Error> = None;
        for p in [
            crate::store::recipe_target(&raw),
            pipeline::xmp_target(&raw),
            crate::store::legacy_recipe(&raw),
            crate::store::legacy_xmp(&raw),
            // The GUI's baked-pixels link too — a "cleared" answer here must
            // not let the next GUI open resurrect a retouched master.
            crate::store::pixel_source_path(&raw),
        ] {
            if let Some(e) = del(&p) {
                first_err.get_or_insert(e);
            }
        }
        return match first_err {
            None => Ok(text_response("cleared — saved edits removed")),
            Some(e) => {
                Ok(status_response(500, &format!("could not clear the saved edits: {e}")))
            }
        };
    }
    // Dual-write, exactly as the GUI's `save_xmp` does: the XMP alone is lossy
    // (no bitmap masks / recolour gains) AND neither surface reads it back while
    // a `recipe.json` exists — so an XMP-only save was unreachable, silently
    // shadowed by whatever recipe.json Analyze had left behind. recipe.json
    // FIRST (same order as the GUI): it is the authoritative projection, and
    // writing the XMP first meant a failed recipe write left a NEW XMP shadowed
    // by the STALE recipe both surfaces prefer.
    pipeline::write_recipe(&raw, &req.recipe, None)?;
    // A browser-session master (fill/heal chained this session) becomes the
    // develop's persisted pixel source on save — the GUI rule: saving
    // records pixel identity, else the healed pixels evaporate on the very
    // reopen that follows "saved". Only ever WRITTEN here: an absent claim
    // must not clear a GUI-persisted master.
    let mut master_note = String::new();
    if let Some(p) = &master {
        // Recipe already committed (the cross-surface rule); an I/O failure
        // recording the pixels degrades to a disclosed warning, same as the
        // GUI's pixels_ok path. Claim REJECTION was a 400 before any write.
        if let Err(e) = crate::store::write_pixel_source(&raw, p, false) {
            master_note = format!(
                " — but recording the retouched master failed ({e}); reopening shows the un-retouched source"
            );
        }
    }
    // Recipe committed = saved (the cross-surface rule): a failed XMP
    // projection reports success WITH the warning instead of a 500 that
    // contradicts the on-disk state.
    match pipeline::write_xmp(&raw, &req.recipe) {
        Ok(path) => Ok(text_response(&format!("{}{master_note}", path.display()))),
        Err(e) => Ok(text_response(&format!(
            "saved (recipe.json) — but the Lightroom XMP projection failed: {e:#}{master_note}"
        ))),
    }
}

/// Generative fill (Phase 4 in the UI): the browser posts a painted RGBA mask
/// (transparent = regenerate) + a prompt; we run [`generative::retouch`], which
/// composites the regenerated region back onto the FULL-resolution source and
/// writes the master to ./out. We return a resized JPEG of the result for inline
/// display, with the saved master path in `X-Output-Path`. Needs OPENAI_API_KEY.
fn api_retouch(request: &mut Request, state: &AppState) -> Result<ResponseBox> {

    let req: RetouchReq = read_json(request)?;
    let raw = match state.at(req.id) {
        Some(r) => r,
        None => return Ok(status_response(400, "bad id")),
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
    let src = match session_master(req.master.as_deref(), &raw) {
        Ok(Some(p)) => p,
        Ok(None) => crate::store::read_pixel_source(&raw)
            .map(|(m, _)| m)
            .unwrap_or_else(|| raw.clone()),
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
        "autoshop_mask_{}_{}.png",
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

    let req: HealReq = read_json(request)?;
    let raw = match state.at(req.id) {
        Some(r) => r,
        None => return Ok(status_response(400, "bad id")),
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
                        Ok(Some(p)) => p,
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
                        "autoshop_heal_{}_{}.png",
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
    // Same unique-claim + config-snapshot rules as api_retouch (see there).
    let Some(out) = pipeline::unique_out(&raw, "heal") else {
        return Ok(status_response(500, "no free heal output name (999 in ./out)"));
    };
    let cfg = state.config().clone();
    // Same master-input rule as api_retouch (see there): session master
    // first — a second heal must build ON the first, not beside it.
    let src = match session_master(req.master.as_deref(), &raw) {
        Ok(Some(p)) => p,
        Ok(None) => crate::store::read_pixel_source(&raw)
            .map(|(m, _)| m)
            .unwrap_or_else(|| raw.clone()),
        Err(msg) => return Ok(status_response(400, &msg)),
    };
    let result = crate::retouch::heal(&cfg, &src, mask_tmp.as_deref(), req.auto, req.full_res, &out);
    if let Some(t) = &mask_tmp {
        let _ = std::fs::remove_file(t);
    }
    match result {
        Ok(rep) => {
            let img =
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
fn api_settings_get(state: &AppState) -> Result<ResponseBox> {
    let cfg = state.config();
    let body = json!({
        "analysis": {
            "provider": cfg.analysis_provider,
            "model": cfg.analysis_model,
            "base_url": cfg.analysis_base_url,
            "key_present": cfg.analysis_api_key.is_some(),
        },
        "image": {
            "model": cfg.openai_model,
            "base_url": cfg.openai_base_url,
            "gen_model": cfg.openai_image_model,
            "key_present": cfg.openai_api_key.is_some(),
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
/// Serializes the settings read-modify-write cycle: two concurrent POSTs
/// each loading the same old file and changing different fields silently
/// lost whichever save landed first.
static SETTINGS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn api_settings_post(request: &mut Request, state: &AppState) -> Result<ResponseBox> {
    let inc: LocalSettings = read_json(request)?;
    let _settings = SETTINGS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut cur = crate::config::load_local_settings();
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
    // Secrets: only overwrite when a non-empty value was actually provided.
    if let Some(k) = inc.analysis_api_key.filter(|s| !s.trim().is_empty()) {
        cur.analysis_api_key = Some(k);
    }
    if let Some(k) = inc.image_api_key.filter(|s| !s.trim().is_empty()) {
        cur.image_api_key = Some(k);
    }

    let path = crate::config::save_local_settings(&cur).map_err(|e| anyhow!("write settings: {e}"))?;
    *state.cfg.write().unwrap_or_else(|e| e.into_inner()) = Config::load();
    Ok(json_response(&json!({ "ok": true, "saved": path.display().to_string() })))
}

// --- helpers ---------------------------------------------------------------

fn is_baked_ext(p: &Path) -> bool {
    p.extension().and_then(|x| x.to_str()).is_some_and(|x| {
        matches!(x.to_ascii_lowercase().as_str(), "png" | "tif" | "tiff" | "jpg" | "jpeg")
    })
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

/// Build a header, or `None` when the value is not a legal header value.
/// `Header::from_bytes(..).unwrap()` is only safe on compile-time-ASCII
/// literals; on anything derived from a photo's name it is a panic waiting for
/// the first non-ASCII library, and a panicking handler thread answers with an
/// EMPTY body (tiny_http's `impl Drop for Request`).
fn header(field: &str, value: &str) -> Option<Header> {
    Header::from_bytes(field.as_bytes(), value.as_bytes()).ok()
}

/// A JPEG body plus the `X-Output-Path` of the master that was saved, with the
/// path percent-encoded so a non-ASCII stem can never break the header.
fn image_with_path(buf: Vec<u8>, out: &Path) -> ResponseBox {
    let mut resp = Response::from_data(buf);
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

fn read_json<T: serde::de::DeserializeOwned>(request: &mut Request) -> Result<T> {
    // Bounded: an unbounded read_to_string materialized an arbitrarily large
    // (or hostile) body in memory before parsing ever ran. 256 MiB clears any
    // legitimate payload (the largest is a full-res mask base64) many times
    // over.
    use std::io::Read as _;
    const BODY_CAP: u64 = 256 * 1024 * 1024;
    let mut body = String::new();
    // UFCS: `as_reader` hands out `&mut dyn Read`; autoref would try `take`
    // on the UNSIZED trait object, but the sized `&mut dyn Read` itself
    // implements Read.
    std::io::Read::take(request.as_reader(), BODY_CAP + 1)
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

/// A JSON body that is already serialised (e.g. a sidecar served verbatim).
fn json_text(text: String) -> ResponseBox {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    Response::from_string(text).with_header(header).boxed()
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
    Response::from_string(html).with_header(ct).with_header(cc).boxed()
}

fn text_response(text: &str) -> ResponseBox {
    Response::from_string(text).boxed()
}

fn jpeg_response(img: &DynamicImage) -> Result<ResponseBox> {
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
        .context("encode jpeg")?;
    let header = Header::from_bytes(&b"Content-Type"[..], &b"image/jpeg"[..]).unwrap();
    Ok(Response::from_data(buf).with_header(header).boxed())
}

fn status_response(code: u16, msg: &str) -> ResponseBox {
    Response::from_string(msg).with_status_code(code).boxed()
}

#[cfg(test)]
mod tests {
    use super::{percent_decode, percent_encode};

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
}
