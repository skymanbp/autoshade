//! Bounded, MEMORY-BUDGETED parallelism for the per-photo CLI loops (`batch`,
//! `eval`) — the `--jobs N` machinery, plus the output sequencer that keeps a
//! parallel run's report readable.
//!
//! # Why a memory budget and not just a worker count (R27 Batch-7)
//!
//! One photo's non-network pipeline pass peaks at **~1.77 GB of COMMIT charge**
//! on this project's reference corpus (61 MP Sony ARW). Measured 2026-08-19 on
//! 12 RAW+`.xmp` pairs from the eval corpus, replaying exactly what
//! `pipeline::produce_recipe` runs between the network calls, and reading the
//! process's own `PeakPagefileUsage` between photos:
//!
//! ```text
//!   stage=decode  (decode_any + preview_resized)      peak commit   151 MB
//!   stage=cal     (photo_base_knots -> render_to_image) peak commit 1771 MB
//!   both                                              peak commit 1771 MB
//! ```
//!
//! The 1.77 GB lives in the demosaic: `render::render_to_image` produces the
//! full-frame oriented f32 buffer (61 MP x 3 x 4 B ~ 732 MB, plus rawler's own
//! sensor buffers and the orientation transient) BEFORE it applies `max_edge`
//! (render.rs:266-272), so `photo_base_knots`' "capped at 2048" bounds the
//! develop stages but NOT the peak.
//!
//! What the same measurement REFUTES is accumulation. Over 11 consecutive
//! photos the peak-commit high-water mark moved 1771.3 -> 1772.3 MB (+1.0 MB
//! total) and the resident commit BETWEEN photos sat at 12.2-12.6 MB. A leak
//! would push the high-water mark up every iteration; it does not. The
//! 147-photo decode-only probe agrees — flat at 112-127 MB mean commit across
//! all four quartiles of the run. So the failure mode the `--jobs` cap has to
//! defend against is PEAK, hit once per photo and multiplied by every worker,
//! not growth.
//!
//! That is why the cap is derived from free memory rather than from CPU count:
//! `available_parallelism()` on an 8-core box would authorise ~14 GB of
//! concurrent commit for work that is anyway dominated by network latency.
//!
//! # Output discipline
//!
//! Workers never print. Each writes its whole per-photo block into a `String`
//! and hands it to the [`Sequencer`], which releases block `i` only once every
//! block before it has been written — so a parallel run's transcript is
//! byte-identical in ORDER to a serial one, whatever order the photos actually
//! finish in. (The cost is latency, not correctness: a slow photo 1 holds the
//! finished blocks of 2..k until it lands.)
//!
//! # API concurrency, stated plainly
//!
//! There is NO rate limiter here. `N` workers means up to `N` proposer calls in
//! flight, and that is the whole of the throttling — each call keeps the retry
//! and timeout budget it already had (`AUTOSHOP_HTTP_TIMEOUT_SECS`), and nothing
//! coordinates between them. The endpoint these calls now reach is a relay whose
//! requests-per-minute ceiling is unknown to this codebase, so a global limiter
//! would have to invent a number; the honest bound is the worker count the user
//! chose. If a run starts collecting 429s, lower `--jobs`.
//!
//! Not covered, and disclosed rather than pretended away: `eprintln!` lines
//! raised deep inside the pipeline (e.g. the GPT-proposer fallback at
//! pipeline.rs:390) go straight to the process stderr and therefore appear in
//! COMPLETION order, not index order. They cannot interleave mid-line — Rust
//! holds the stderr lock across the whole `write_fmt` — but they are not
//! attributable to a photo by position. Callers that need the attribution read
//! the typed `rationale::Note` channel `produce_recipe` returns as its third
//! element and render it INTO the block (that is what `eval` does above one
//! job); the notes carry the same fallback disclosure by construction.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Peak COMMIT charge, in MB, that ONE photo's pipeline pass reaches.
///
/// Provenance: measured 2026-08-19 (R27 Batch-7) — see the module docs for the
/// per-stage table and the method. Rounded UP from the observed 1771 MB, since
/// the budget it divides is a safety margin and a low estimate over-subscribes.
/// It is a property of the CORPUS as much as of the code (a 24 MP body peaks
/// far lower), so it is deliberately the pessimistic end of the libraries this
/// project is used on rather than a per-file estimate: reading each RAW's
/// dimensions to size the budget would cost a decode per photo before the pool
/// even starts.
pub const PER_PHOTO_PEAK_COMMIT_MB: u64 = 1_800;

/// Share of the machine's free memory the pool may commit to photos, as a
/// percentage. Half: the other half is the rest of the machine — a browser, an
/// editor, and (on the `eval` path) the `claude` verifier subprocess this
/// process spawns per photo, which is a Node runtime of its own and is NOT
/// counted in [`PER_PHOTO_PEAK_COMMIT_MB`].
const BUDGET_PCT: u64 = 50;

/// Free memory this machine can hand out right now, in MB, or `None` when the
/// OS cannot be asked (then no memory cap applies and `--jobs` is taken at its
/// word — a wrong guess must not silently serialise the user's run).
///
/// The MINIMUM of free physical memory and free commit, because either one
/// running out ends the run: exhausting commit fails the allocation outright,
/// while committing far past physical memory just moves the 1.8 GB peak onto
/// the pagefile and turns a 2-second demosaic into a disk-bound crawl.
pub fn free_memory_mb() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{
            GlobalMemoryStatusEx, MEMORYSTATUSEX,
        };
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        // 0 = failure; the struct is then untouched and must not be read.
        if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
            return None;
        }
        Some(m.ullAvailPhys.min(m.ullAvailPageFile) / (1024 * 1024))
    }
    // `_SC_AVPHYS_PAGES` is a Linux/glibc name; the other unixes either lack it
    // or report something else entirely, and guessing there would be the
    // "confident wrong answer" this cap exists to avoid. They get `None` (no
    // cap), which is exactly the pre-R27 behaviour.
    #[cfg(target_os = "linux")]
    {
        let pages = unsafe { libc::sysconf(libc::_SC_AVPHYS_PAGES) };
        let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if pages <= 0 || size <= 0 {
            return None;
        }
        Some((pages as u64).saturating_mul(size as u64) / (1024 * 1024))
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        None
    }
}

/// How many concurrent photos `headroom_mb` of free memory pays for, or `None`
/// when the machine could not be measured. Never 0: one photo at a time is the
/// serial behaviour, and refusing to run at all because memory is tight would
/// be worse than running slowly.
fn memory_cap(headroom_mb: Option<u64>) -> Option<usize> {
    let budget = headroom_mb? * BUDGET_PCT / 100;
    Some(((budget / PER_PHOTO_PEAK_COMMIT_MB) as usize).max(1))
}

/// The decided worker count plus the DISCLOSURE the caller must print when the
/// memory budget overruled what the user asked for. Silently running fewer
/// workers than `--jobs N` would look like the flag did nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub jobs: usize,
    pub note: Option<String>,
}

/// Resolve `--jobs` against the work on hand and the machine's free memory.
pub fn plan(requested: usize, work: usize) -> Plan {
    plan_with(requested, work, free_memory_mb())
}

/// [`plan`] with the memory reading injected — the whole decision as a pure
/// function, so the cap is testable without a machine in a particular state.
pub fn plan_with(requested: usize, work: usize, headroom_mb: Option<u64>) -> Plan {
    // 0 is a user typo, not "no workers"; more workers than photos is waste.
    let asked = requested.max(1);
    let by_work = asked.min(work.max(1));
    match memory_cap(headroom_mb) {
        Some(cap) if cap < by_work => Plan {
            jobs: cap,
            note: Some(format!(
                "  memory budget: running {cap} worker(s), not {asked} — one photo peaks at \
                 ~{PER_PHOTO_PEAK_COMMIT_MB} MB and only {} MB is free ({BUDGET_PCT}% of it \
                 budgeted for this run).",
                headroom_mb.unwrap_or(0)
            )),
        },
        _ => Plan { jobs: by_work, note: None },
    }
}

/// Releases per-photo output blocks in INDEX order, whatever order the workers
/// finish in.
///
/// A completed block whose turn has not come is HELD, not waited on: parking
/// the worker would idle a whole pipeline slot behind one slow photo. At most
/// one block per not-yet-released index is buffered, and a block is a few
/// hundred bytes of text.
pub struct Sequencer {
    state: Mutex<SeqState>,
}

struct SeqState {
    /// The index whose block may be written next.
    next: usize,
    held: BTreeMap<usize, String>,
}

impl Default for Sequencer {
    fn default() -> Self {
        Self::new()
    }
}

impl Sequencer {
    pub fn new() -> Self {
        Self { state: Mutex::new(SeqState { next: 0, held: BTreeMap::new() }) }
    }

    /// Hand `index`'s finished block over, then write out every block that has
    /// now become releasable.
    ///
    /// Poison is recovered rather than re-panicked: one worker panicking must
    /// not turn every OTHER worker's release into a second panic, which inside
    /// `thread::scope` would abort instead of unwinding to a joinable end.
    pub fn release(&self, index: usize, block: String) {
        let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
        // Split the guard once: `held` and `next` are two fields of one struct
        // reached through a DerefMut, which cannot be borrowed apart inline.
        let st = &mut *guard;
        st.held.insert(index, block);
        let ready = st.drain_ready();
        if ready.is_empty() {
            return;
        }
        // stdout taken INSIDE the state lock, and only here: two releases can
        // never write between each other's lines. No other path in this module
        // takes the two locks, so the nesting cannot invert.
        let mut out = std::io::stdout().lock();
        for text in ready {
            let _ = out.write_all(text.as_bytes());
        }
        let _ = out.flush();
    }
}

impl SeqState {
    fn drain_ready(&mut self) -> Vec<String> {
        let mut ready = Vec::new();
        while let Some(text) = self.held.remove(&self.next) {
            ready.push(text);
            self.next += 1;
        }
        ready
    }
}

/// Run `body` over `0..n` on at most `jobs` threads, releasing each index's
/// output block in index order and returning every index's result in index
/// order.
///
/// `body` writes its lines into the `&mut String` it is handed; it must NOT
/// print, or the sequencer's ordering guarantee is void for those lines.
///
/// The results come back positionally so the caller can FOLD them in index
/// order — which is what keeps a parallel aggregate bit-identical to the serial
/// one, since summing f64s in completion order would drift in the last ulp.
///
/// A panic inside `body` propagates out of the scope, as it did in the
/// hand-rolled pool this replaces (main.rs' batch loop): the blocks of indices
/// that completed after the panicking one are then forfeited with the run.
pub fn for_each_indexed<T, F>(jobs: usize, n: usize, body: F) -> Vec<Option<T>>
where
    F: Fn(usize, &mut String) -> T + Sync,
    T: Send,
{
    let jobs = jobs.max(1).min(n.max(1));
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<T>>> = Mutex::new((0..n).map(|_| None).collect());
    let seq = Sequencer::new();
    std::thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| {
                loop {
                    // The shared counter over a FIXED list preserves `--limit`
                    // semantics exactly (the rule the batch pool already had).
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let mut block = String::new();
                    let value = body(i, &mut block);
                    // The result lands BEFORE the block is released, so a
                    // caller reading results after the scope can never see a
                    // printed photo whose result is still missing.
                    out.lock().unwrap_or_else(|p| p.into_inner())[i] = Some(value);
                    seq.release(i, block);
                }
            });
        }
    });
    out.into_inner().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MUTATION THIS KILLS: make `Sequencer::release` write the block it was
    /// just handed (or drain without the `next` gate) instead of draining only
    /// the consecutive run from `next`. Completion order here is deliberately
    /// 3,1,2,0, so a "print on arrival" sequencer produces "d b c a" and this
    /// assertion fails; the index-ordered one produces "a b c d".
    ///
    /// Written mutation-first: the release-order rule is the whole point of the
    /// type, and it is invisible to any test that submits in order.
    #[test]
    fn sequencer_releases_in_index_order_not_completion_order() {
        let seq = Sequencer::new();
        let mut order = Vec::new();
        // Drive the state machine directly (the real one writes to stdout, so
        // the observable is WHICH indices became releasable and when).
        {
            let mut g = seq.state.lock().unwrap();
            for (i, text) in [(3usize, "d"), (1, "b"), (2, "c"), (0, "a")] {
                g.held.insert(i, text.to_string());
                order.extend(g.drain_ready());
            }
        }
        assert_eq!(order, vec!["a", "b", "c", "d"], "blocks must leave in index order");
        // …and nothing is released twice or left behind.
        let g = seq.state.lock().unwrap();
        assert!(g.held.is_empty() && g.next == 4);
    }

    /// A block whose turn has not come is HELD, and holding it must not block
    /// the worker that produced it — the property that lets a fast photo 9
    /// finish while a slow photo 0 is still running.
    #[test]
    fn a_block_out_of_turn_is_held_and_releases_nothing() {
        let seq = Sequencer::new();
        let mut g = seq.state.lock().unwrap();
        g.held.insert(7, "seven".into());
        assert!(g.drain_ready().is_empty(), "index 7 cannot precede 0..6");
        assert_eq!(g.next, 0);
    }

    /// The cap arithmetic, at the boundaries that decide a run.
    ///
    /// MUTATION THIS KILLS: dropping the `.max(1)` in `memory_cap` (a
    /// nearly-full machine would then plan 0 workers and the pool would run
    /// nothing at all), and swapping the budget fraction for the raw free
    /// figure (which would authorise double the concurrency the constant was
    /// measured for).
    #[test]
    fn the_memory_cap_divides_the_budget_by_the_measured_per_photo_peak() {
        // 16 GB free, half budgeted = 8192 MB / 1800 = 4 photos.
        assert_eq!(memory_cap(Some(16_384)), Some(4));
        // 8 GB free -> 4096/1800 = 2.
        assert_eq!(memory_cap(Some(8_192)), Some(2));
        // Under one photo's peak: still 1, never 0.
        assert_eq!(memory_cap(Some(512)), Some(1));
        assert_eq!(memory_cap(Some(0)), Some(1));
        // Unmeasurable machine = no cap at all, not a cap of 1.
        assert_eq!(memory_cap(None), None);
    }

    #[test]
    fn the_plan_takes_the_smallest_of_asked_work_and_memory() {
        // Plenty of memory: the ask wins, bounded by the work on hand.
        assert_eq!(plan_with(4, 10, Some(64_000)), Plan { jobs: 4, note: None });
        assert_eq!(plan_with(8, 3, Some(64_000)), Plan { jobs: 3, note: None });
        // 0 and 1 are the serial default and must never mean "no workers".
        assert_eq!(plan_with(0, 10, Some(64_000)), Plan { jobs: 1, note: None });
        assert_eq!(plan_with(1, 10, None), Plan { jobs: 1, note: None });
        // An empty work list still plans a valid (unused) pool.
        assert_eq!(plan_with(4, 0, None), Plan { jobs: 1, note: None });
        // Memory overrules the ask, and SAYS so — a silent downgrade would
        // read as "--jobs did nothing".
        let tight = plan_with(8, 100, Some(8_192));
        assert_eq!(tight.jobs, 2);
        let note = tight.note.expect("capping must disclose");
        assert!(note.contains("not 8") && note.contains("8192"), "{note}");
        // …and it stays quiet when it did not intervene.
        assert!(plan_with(2, 100, Some(64_000)).note.is_none());
    }

    /// The pool itself: out-of-order completion, in-order output, in-order
    /// results — and every index visited exactly once.
    #[test]
    fn the_pool_visits_every_index_once_and_returns_results_positionally() {
        let n = 64;
        let seen = Mutex::new(Vec::new());
        let got = for_each_indexed(4, n, |i, block| {
            // Uneven work so completion order really does diverge from index
            // order (index 0 is the slowest).
            std::thread::sleep(std::time::Duration::from_micros(((n - i) * 50) as u64));
            block.push_str(&format!("[{i}]\n"));
            seen.lock().unwrap().push(i);
            i * 2
        });
        assert_eq!(got.len(), n);
        for (i, v) in got.iter().enumerate() {
            assert_eq!(*v, Some(i * 2), "result {i} landed at the wrong index");
        }
        let mut visited = seen.into_inner().unwrap();
        assert_eq!(visited.len(), n, "every index runs exactly once");
        visited.sort_unstable();
        assert_eq!(visited, (0..n).collect::<Vec<_>>());
    }

    /// jobs = 1 is the serial path, not a degenerate pool.
    #[test]
    fn one_job_runs_strictly_in_order() {
        let order = Mutex::new(Vec::new());
        let _ = for_each_indexed(1, 20, |i, _b| {
            order.lock().unwrap().push(i);
        });
        assert_eq!(order.into_inner().unwrap(), (0..20).collect::<Vec<_>>());
    }
}
