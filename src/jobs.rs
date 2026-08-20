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
//! ## The render tail, measured (R28 Batch-4)
//!
//! What R27 did NOT measure — and what the R28 adjudication (F2) registered as
//! the one UNVERIFIABLE part of the constant — is that `batch --render` also
//! pays `render::render_to_file` at FULL resolution afterwards. That is now
//! measured, by the same method, on the same 60.2 MP A7R V frame
//! (9504x6336), release profile, ONE STAGE PER PROCESS so no row inherits an
//! earlier row's high-water mark (`tests::probe_per_photo_peak_commit`,
//! 2026-08-20):
//!
//! ```text
//!   stage=decode  alone                               peak commit   151 MB
//!   stage=cal     alone                               peak commit  1771 MB
//!   stage=render  alone (render_to_file, max_edge=None,
//!                 clarity/texture/sharpen/NR all on)  peak commit  1766 MB
//!   all three, one process                            peak commit  1771 MB
//! ```
//!
//! **The tail does not raise the peak, and 1,800 MB stands** — but only just.
//! The two big stages land 5 MB apart (1771 vs 1766) because they share the
//! expensive moment: BOTH run the same full-frame demosaic and orientation,
//! and `max_edge` only decides what happens after it. The full-resolution
//! render's own extra — the 16-bit pack beside the f32 plane — is smaller than
//! the working-resolution path's orientation transient, so the tail comes in
//! marginally UNDER the stage that was already measured. The `all` row equals
//! the `cal` row, which is the same fact read from the other side: no stage
//! stacks on another, because each frees its buffers before the next
//! allocates.
//!
//! The margin is thin and worth saying so: 1,800 is a round-UP of 1,771, not a
//! safety factor. It holds for this corpus (one body, one CFA); a decoder that
//! peaked higher per pixel would eat it. What bounds THAT case is the
//! per-file ceiling this batch added at the develop door, not this constant.
//!
//! Per source pixel that peak is 30.8 B, which is where
//! `decode::RAW_DEVELOP_BYTES_PER_PIXEL` (31, rounded UP) comes from — the
//! per-FILE half of the same accounting, added in this batch because the
//! constant below is deliberately per-corpus and a single 150 MP file can
//! exceed it on its own.
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
/// Provenance: measured 2026-08-19 (R27 Batch-7), RE-MEASURED and extended
/// through `render_to_file` 2026-08-20 (R28 Batch-4) — see the module docs for
/// both per-stage tables and the method, and `tests::probe_per_photo_peak_commit`
/// for the harness. Rounded UP from the observed 1771 MB, since the budget it
/// divides is a safety margin and a low estimate over-subscribes.
///
/// It is a property of the CORPUS as much as of the code (a 24 MP body peaks
/// far lower), so it is deliberately the pessimistic end of the libraries this
/// project is used on. What it is NOT is a per-file figure, and R27's reason
/// for that — "reading each RAW's dimensions would cost a decode per photo" —
/// was only ever true for RAWs. [`survey_peak_mb`] now asks the BAKED sources
/// on the work list, whose headers are free, and RAWs are bounded per-file at
/// the develop door instead (`decode::refuse_raw_develop_over_ceiling`). This
/// constant is the floor of that pair, not the whole of it.
pub const PER_PHOTO_PEAK_COMMIT_MB: u64 = 1_800;

/// Share of the machine's free memory the pool may commit to photos, as a
/// percentage. Half: the other half is the rest of the machine — a browser, an
/// editor, and (on the `eval` path) the `claude` verifier subprocess this
/// process spawns per photo, which is a Node runtime of its own and is NOT
/// counted in [`PER_PHOTO_PEAK_COMMIT_MB`].
const BUDGET_PCT: u64 = 50;

/// The per-photo peak THIS run must budget for, plus the disclosure that
/// belongs with it — [`PER_PHOTO_PEAK_COMMIT_MB`] unless the work list's own
/// headers say a file on it is bigger than the corpus default.
///
/// **This is the dimensionless-constant fix (R28 Batch-4 4a; adjudication
/// F2).** The constant is a property of the corpus, not of the files in front
/// of it, and nothing re-checked it against them: four 143 MP exports on a
/// 16 GB machine planned four workers against a 7.2 GB budget and asked for
/// ~16 GB, silently. The planner now ASKS, for every source whose header is
/// cheap to read.
///
/// Deliberately asymmetric, and that asymmetry is the honest part:
///
/// * **Baked** sources answer ([`crate::decode::cheap_develop_peak_mb`]) — a
///   header parse, no pixel decoded. These are also the reachable overshoot:
///   Lightroom's "Edit in…" writes native-resolution 16-bit TIFFs, and
///   `batch --include-baked` develops them.
/// * **Camera RAWs** answer `None`, because asking would map the whole file
///   per photo before the pool starts. They are bounded at the develop door
///   instead, by the per-file 4 GiB ceiling this batch added
///   (`decode::refuse_raw_develop_over_ceiling`), which did not exist when F2
///   was written. Between the two, no admitted file can exceed 4 GiB and no
///   surveyed file is budgeted at less than it needs.
///
/// The MAXIMUM, not the mean: the budget divides into concurrent workers that
/// can each be handed the largest photo, and a mean would authorise exactly
/// the overshoot this exists to stop.
pub fn survey_peak_mb(work: &[&std::path::Path]) -> (u64, Option<String>) {
    survey_peak_with(work, crate::decode::cheap_develop_peak_mb)
}

/// [`survey_peak_mb`] with the per-file estimate INJECTED — the same
/// discipline [`plan_with`] applies to the memory reading, and for the same
/// reason: the decision (max, threshold, disclosure) is then testable without
/// a 64 MP file on disk to produce a number with.
fn survey_peak_with(
    work: &[&std::path::Path],
    estimate: impl Fn(&std::path::Path) -> Option<u64>,
) -> (u64, Option<String>) {
    let mut worst: Option<(u64, &std::path::Path)> = None;
    for p in work {
        if let Some(mb) = estimate(p)
            && mb > worst.map_or(0, |(v, _)| v)
        {
            worst = Some((mb, p));
        }
    }
    match worst {
        Some((mb, p)) if mb > PER_PHOTO_PEAK_COMMIT_MB => (
            mb,
            // Says what was found, not what will follow: the cap note below
            // only prints when the budget actually overrules the flag, so
            // promising a smaller worker count here would be a claim this
            // function cannot keep on a machine with memory to spare.
            Some(format!(
                "  memory budget: {} alone peaks at ~{mb} MB — more than the \
                 {PER_PHOTO_PEAK_COMMIT_MB} MB this corpus usually needs, so this run is \
                 budgeted at {mb} MB per photo.",
                p.display()
            )),
        ),
        _ => (PER_PHOTO_PEAK_COMMIT_MB, None),
    }
}

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
/// Saturating like [`free_memory_mb`]'s own Linux arm (`saturating_mul` at the
/// page-count multiply): this is a public `lib` entry point via [`plan_with`],
/// so the reading is not always one this module produced. `plan()`'s own path
/// cannot overflow by construction — both `free_memory_mb` arms divide by 2^20
/// last, leaving four orders of magnitude of headroom under `×50` — but an
/// injected `u64::MAX` panicked here under debug overflow checks while the
/// identical hazard one function up was already handled (R28 2d, adjudication
/// F10). Saturating then dividing keeps the arithmetic honest at the top: a
/// saturated budget still divides down to a worker count, and the `.max(1)`
/// below means no reading can ever produce "run nothing".
fn memory_cap(headroom_mb: Option<u64>, per_photo_peak_mb: u64) -> Option<usize> {
    let budget = headroom_mb?.saturating_mul(BUDGET_PCT) / 100;
    // `.max(1)` on the divisor too: `per_photo_peak_mb` reaches here from
    // [`survey_peak_mb`], and a header that answered 0 MB would otherwise
    // divide by zero. The constant itself can never be 0.
    Some(((budget / per_photo_peak_mb.max(1)) as usize).max(1))
}

/// The decided worker count plus the DISCLOSURE the caller must print when the
/// memory budget overruled what the user asked for. Silently running fewer
/// workers than `--jobs N` would look like the flag did nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub jobs: usize,
    pub note: Option<String>,
}

/// Resolve `--jobs` against the work list, the machine's free memory, and the
/// files' OWN per-photo peaks: the divisor is the larger of the corpus
/// constant and what these files' headers say ([`survey_peak_mb`]), and both
/// disclosures ride out together.
///
/// THE door. There was a count-only `plan(requested, work: usize)` beside it
/// until R28 Batch-4; it is gone rather than kept, because a shorter name that
/// silently skips the per-file survey is the footgun this batch exists to
/// remove — the whole finding (adjudication F2) was that the budget never
/// looked at the files it was budgeting for.
pub fn plan_for(requested: usize, work: &[&std::path::Path]) -> Plan {
    let (peak, surveyed) = survey_peak_mb(work);
    let mut plan = plan_with_peak(requested, work.len(), free_memory_mb(), peak);
    // BOTH lines, never one: the survey note explains where the divisor came
    // from and the cap note explains what it cost, and printing only the
    // second would leave "not 4" looking arbitrary on a machine with plenty of
    // memory for the corpus default.
    plan.note = match (surveyed, plan.note.take()) {
        (Some(a), Some(b)) => Some(format!("{a}\n{b}")),
        (a, b) => a.or(b),
    };
    plan
}

/// [`plan_for`] with the memory reading injected and the corpus constant taken
/// as the per-photo peak — the whole decision as a pure function, so the cap is
/// testable without a machine in a particular state or files on disk.
///
/// A TEST SEAM, not a second door: it cannot consult the work list because it
/// is not given one. Production callers take [`plan_for`].
pub fn plan_with(requested: usize, work: usize, headroom_mb: Option<u64>) -> Plan {
    plan_with_peak(requested, work, headroom_mb, PER_PHOTO_PEAK_COMMIT_MB)
}

/// [`plan_with`] with the per-photo peak injected too — the whole decision,
/// including the R28 per-file half, as a pure function.
pub fn plan_with_peak(
    requested: usize,
    work: usize,
    headroom_mb: Option<u64>,
    per_photo_peak_mb: u64,
) -> Plan {
    // 0 is a user typo, not "no workers"; more workers than photos is waste.
    let asked = requested.max(1);
    let by_work = asked.min(work.max(1));
    match memory_cap(headroom_mb, per_photo_peak_mb) {
        Some(cap) if cap < by_work => Plan {
            jobs: cap,
            note: Some(format!(
                "  memory budget: running {cap} worker(s), not {asked} — one photo peaks at \
                 ~{per_photo_peak_mb} MB and only {} MB is free ({BUDGET_PCT}% of it \
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
        let c = |mb| memory_cap(mb, PER_PHOTO_PEAK_COMMIT_MB);
        // 16 GB free, half budgeted = 8192 MB / 1800 = 4 photos.
        assert_eq!(c(Some(16_384)), Some(4));
        // 8 GB free -> 4096/1800 = 2.
        assert_eq!(c(Some(8_192)), Some(2));
        // Under one photo's peak: still 1, never 0.
        assert_eq!(c(Some(512)), Some(1));
        assert_eq!(c(Some(0)), Some(1));
        // Unmeasurable machine = no cap at all, not a cap of 1.
        assert_eq!(c(None), None);
        // A surveyed per-file peak divides the SAME budget (R28 4a): four
        // 4 GiB-class exports on the same 16 GB machine plan two workers, not
        // four. MUTATION THIS KILLS: ignoring the injected peak and using the
        // corpus constant regardless — then this reads 4 and the constructed
        // scenario overshoots by 2.28x exactly as F2 measured.
        assert_eq!(memory_cap(Some(16_384), 4_095), Some(2));
        // A header that answered 0 must not divide by zero (8192 MB / 1 MB).
        assert_eq!(memory_cap(Some(16_384), 0), Some(8_192));
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

    /// [`plan_with`] is a public `lib` entry point, so the memory reading is
    /// not always one [`free_memory_mb`] produced — and the whole point of
    /// injecting it is that a caller supplies the number.
    ///
    /// MUTATION THIS KILLS: revert `memory_cap`'s `saturating_mul` to the bare
    /// `headroom_mb? * BUDGET_PCT`. `u64::MAX × 50` overflows and this test
    /// panics with "attempt to multiply with overflow" under the debug
    /// overflow checks the test profile compiles with (adjudication F10).
    #[test]
    fn an_absurd_memory_reading_plans_a_pool_instead_of_panicking() {
        let p = plan_with(1, 10, Some(u64::MAX));
        // Saturated budget / 1800 MB is astronomically more than the ask, so
        // the cap never binds: the ask wins and nothing is disclosed.
        assert_eq!(p, Plan { jobs: 1, note: None });
        // The whole neighbourhood of the boundary, not just the top value.
        for h in [u64::MAX, u64::MAX / 2, u64::MAX / 50 + 1] {
            let cap =
                memory_cap(Some(h), PER_PHOTO_PEAK_COMMIT_MB).expect("a measured machine is capped");
            assert!(cap >= 1, "headroom {h} planned {cap} workers");
        }
    }

    /// The per-file half of the budget (R28 Batch-4 4a, adjudication F2): the
    /// survey takes the MAXIMUM of what the work list's own headers say, only
    /// raises the divisor above the corpus constant, and SAYS which file did
    /// it.
    ///
    /// MUTATION THIS KILLS: taking a mean (or the first answer, or the last)
    /// instead of the max. The budget divides into workers that can each be
    /// handed the largest photo, so a mean re-authorises exactly the overshoot
    /// this exists to stop — with the list below, a mean plans against
    /// ~1,900 MB while one photo really needs 4,000.
    #[test]
    fn the_survey_budgets_for_the_biggest_file_and_names_it() {
        use std::path::Path;
        let big = Path::new("huge.tif");
        let work = [Path::new("a.arw"), Path::new("small.png"), big, Path::new("b.arw")];
        // RAWs decline (mapping the file per photo is the cost the constant
        // exists to avoid); the two baked sources answer.
        let est = |p: &Path| match p.to_str() {
            Some("small.png") => Some(600),
            Some("huge.tif") => Some(4_000),
            _ => None,
        };
        let (peak, note) = survey_peak_with(&work, est);
        assert_eq!(peak, 4_000, "the biggest file sets the divisor");
        let note = note.expect("raising the divisor must disclose");
        assert!(note.contains("huge.tif") && note.contains("4000"), "{note}");

        // Below the corpus constant, the constant wins and nothing is said —
        // a note on every ordinary run would be noise.
        let (peak, note) = survey_peak_with(&work[..2], est);
        assert_eq!(peak, PER_PHOTO_PEAK_COMMIT_MB);
        assert!(note.is_none());
        // An all-RAW list (what `eval` always has) surveys to the constant.
        let raws = [Path::new("a.arw"), Path::new("b.arw")];
        assert_eq!(survey_peak_with(&raws, est), (PER_PHOTO_PEAK_COMMIT_MB, None));
        // …and an empty list plans without panicking.
        assert_eq!(survey_peak_with(&[], est), (PER_PHOTO_PEAK_COMMIT_MB, None));
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

    /// The process's own peak COMMIT charge, in MB — `PeakPagefileUsage`, a
    /// MONOTONE high-water mark the kernel keeps for the life of the process
    /// (it never falls back when a buffer is freed). That is exactly the
    /// quantity [`PER_PHOTO_PEAK_COMMIT_MB`] is denominated in, and reading it
    /// between stages is what makes a stage table like the module docs' — each
    /// row the high-water AFTER that stage — mean anything.
    #[cfg(windows)]
    fn peak_commit_mb() -> u64 {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut c: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        c.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        // 0 = failure; the struct is then untouched and must not be read.
        assert!(
            unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) } != 0,
            "GetProcessMemoryInfo failed — the probe has no reading to report"
        );
        (c.PeakPagefileUsage as u64) / (1024 * 1024)
    }

    /// PEAK-COMMIT PROBE — the measurement behind [`PER_PHOTO_PEAK_COMMIT_MB`],
    /// re-runnable instead of remembered.
    ///
    /// R27 Batch-7 measured `produce_recipe`'s NON-NETWORK half (the module
    /// docs' 151 / 1771 MB table) from a scratch harness that was never
    /// committed, so the number could not be re-derived afterwards. The R28
    /// adjudication (F2) registered the gap that mattered: `batch --render`
    /// also pays `render::render_to_file` at FULL resolution, and that tail had
    /// never been measured at all. This is the same method — one process, one
    /// photo, `PeakPagefileUsage` read between stages — extended through it.
    ///
    /// Deliberately network-free: stages 1 and 2 are precisely what
    /// `produce_recipe` runs BETWEEN its API calls, and stage 3 is the render
    /// `process_one` runs after the verdict lands. No key, no billing.
    ///
    /// `AUTOSHOP_PEAK_PROBE_STAGE` selects `decode` / `cal` / `render` / `all`
    /// (default). ONE STAGE PER PROCESS is the honest way to read a stage's own
    /// peak: the counter never falls back, and a freed buffer stays COMMITTED
    /// while the allocator holds the pages — so in an `all` run every later row
    /// inherits the largest earlier one and can only ever be reported as
    /// "≤ that". The `all` run gives the pass's total; the three single-stage
    /// runs give which stage owns it.
    ///
    /// ```text
    /// set AUTOSHOP_PEAK_PROBE_RAW=C:\…\a-61MP.ARW
    /// set AUTOSHOP_PEAK_PROBE_STAGE=render
    /// cargo test --release --lib -- --ignored --nocapture --test-threads=1 \
    ///     jobs::tests::probe_per_photo_peak_commit
    /// ```
    #[cfg(windows)]
    #[test]
    #[ignore = "real-machine probe: set AUTOSHOP_PEAK_PROBE_RAW to a big RAW (writes one TIFF)"]
    fn probe_per_photo_peak_commit() {
        let Ok(raw) = std::env::var("AUTOSHOP_PEAK_PROBE_RAW") else {
            panic!("set AUTOSHOP_PEAK_PROBE_RAW to a RAW path");
        };
        let raw = std::path::PathBuf::from(raw);
        let out_dir = std::env::var("AUTOSHOP_PEAK_PROBE_OUT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        let stage = std::env::var("AUTOSHOP_PEAK_PROBE_STAGE").unwrap_or_else(|_| "all".into());
        let want = |s: &str| stage == "all" || stage == s;
        assert!(
            ["all", "decode", "cal", "render"].contains(&stage.as_str()),
            "AUTOSHOP_PEAK_PROBE_STAGE must be all|decode|cal|render, got {stage:?}"
        );
        // `frame_size` is an UPPER BOUND on what the per-file RAW ceiling
        // costs at the develop door: it opens the file, maps it, builds a
        // decoder AND takes the `dummy = true` read, where
        // `render_to_image_in`'s gate reuses the first three and pays only the
        // last. Timed here so the gate's price is a measured bound rather than
        // an assurance.
        let t0 = std::time::Instant::now();
        let (w, h) = crate::decode::frame_size(&raw).expect("frame size");
        let header_ms = t0.elapsed().as_secs_f64() * 1e3;
        let px = (w as u64) * (h as u64);
        println!(
            "stage={stage} frame {w}x{h} = {px} px; header probe (upper bound on the \
             per-file ceiling's cost) {header_ms:.1} ms; baseline peak_commit {} MB",
            peak_commit_mb()
        );
        // 1. decode + the advisor's preview: produce_recipe's first half.
        if want("decode") {
            let d = crate::decode::decode_any(&raw).expect("decode");
            let _preview = d.preview_resized(1536);
            println!("  decode  peak commit {:>6} MB", peak_commit_mb());
        }
        // 2. base-look estimation: `render_to_image` at the 2048 working edge —
        //    the stage that dominated the R27 table.
        let knots = if want("cal") {
            let k = crate::pipeline::photo_base_knots(&raw);
            println!("  cal     peak commit {:>6} MB ({} knots)", peak_commit_mb(), k.len());
            k
        } else {
            // A `render`-only run cannot estimate the base look without paying
            // the `cal` stage it is trying to exclude. The curve is 13 knots of
            // metadata, not a buffer — dropping it changes the tone applied,
            // never the footprint.
            Vec::new()
        };
        // 3. THE TAIL F2 REGISTERED AS UNMEASURED. Full resolution (`export =
        //    None` means no long-edge cap), and with the full-frame spatial
        //    stages actually switched ON — a default recipe skips clarity /
        //    texture / sharpen / NR entirely, so measuring one would report a
        //    floor and call it a peak.
        if want("render") {
            let recipe = crate::recipe::EditRecipe {
                base_curve: knots,
                clarity: 20.0,
                texture: 20.0,
                sharpening: 40.0,
                noise_reduction: 25.0,
                ..Default::default()
            };
            let out = out_dir.join("autoshop-peak-probe.tif");
            let dims =
                crate::render::render_to_file(&raw, &recipe, &out, None, None).expect("render");
            let _ = std::fs::remove_file(&out);
            println!("  render  peak commit {:>6} MB (saved {dims:?})", peak_commit_mb());
        }
        let peak = peak_commit_mb();
        println!(
            "=> stage={stage} peak {peak} MB = {:.1} B per source pixel \
             (PER_PHOTO_PEAK_COMMIT_MB is {PER_PHOTO_PEAK_COMMIT_MB}, \
             decode::RAW_DEVELOP_BYTES_PER_PIXEL is {})",
            (peak as f64) * 1_048_576.0 / (px as f64),
            crate::decode::RAW_DEVELOP_BYTES_PER_PIXEL,
        );
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
