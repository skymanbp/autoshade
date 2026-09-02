//! GUI-side memory & thread budget — `autoshade::jobs`' discipline applied to
//! the desktop app's own workers.
//!
//! # Why (user report ③, 2026-08-25: "AS makes the whole machine unusable")
//!
//! One full-frame pipeline pass peaks at ~1.77 GB of COMMIT charge (measured;
//! see [`autoshade::jobs::PER_PHOTO_PEAK_COMMIT_MB`]'s provenance tables), and
//! the reporting machine sat at 26.7 GB committed of 31.2 GB with ~4.4 GB of
//! commit headroom BEFORE AutoShade started. The GUI's workers were each
//! bounded within their own class (`busy` gates user commands, the
//! `big_decode_gate` mutex serialises large thumbnail decodes) but nothing
//! bounded the PROCESS-WIDE SUM: an open's demosaic, a variant master load, a
//! retouch composite and an export could each bring their own ~1.8 GB peak at
//! once, and the engine's row-parallel stages ran on rayon's default pool
//! (one worker per logical core). On a machine with no headroom that
//! over-commit evicts every other process's working set — which is exactly
//! "the whole machine is unusable", and closing AutoShade only starts the slow
//! fault-back-in.
//!
//! # The budget
//!
//! * **Memory**: a process-wide byte reservation. Every worker that pays a
//!   full-frame peak takes a [`HeavyPermit`] for its estimated commit before
//!   the expensive call; the permit is admitted immediately when it is the
//!   only heavy work, and otherwise only while the machine's CURRENT free
//!   memory (min of free physical and free commit —
//!   [`autoshade::jobs::free_memory_mb`]) still covers the estimate plus
//!   [`RESERVED_HEADROOM_MB`] for the rest of the machine. Work that cannot
//!   be admitted WAITS on the worker thread it already owns — queued, never
//!   refused, never downscaled: output bytes are identical either way, later.
//!   The floor of one running job mirrors `jobs::memory_cap`'s `.max(1)`:
//!   refusing to run anything on a tight machine would be worse than running
//!   one thing slowly.
//! * **Threads**: sampled once at startup. On a machine already inside the
//!   budget's danger zone (free memory under the reserve plus two peaks) the
//!   global rayon pool is built with at most [`RAYON_THREADS_MAX`] workers
//!   instead of one per logical core. Measured cost on the reference 61 MP
//!   pass: 16 → 8 threads is 8.03 s → 9.78 s wall and −2 MB peak — the clamp
//!   buys almost no memory, so a healthy machine keeps its full pool; what it
//!   halves is the page-touch RATE, on a machine whose baseline already
//!   faulted at ~61 k demand-zero faults/s.
//!
//! What is deliberately NOT gated: preview-sized work (the develop preview
//! renders from the cached preview base; thumbnails are bounded by
//! `big_decode_gate` and their own six-slot cap) and the python segmentation
//! sidecar (an external process this accounting cannot see; its jobs are
//! busy-gated one at a time already).

use std::sync::{Condvar, Mutex};

/// The commit-charge floor, in MB, budgeted for ONE full-frame pass — the
/// measured corpus constant the CLI planner divides by, shared so the two
/// budgets cannot drift apart.
pub(crate) const HEAVY_PEAK_COMMIT_MB: u64 = autoshade::jobs::PER_PHOTO_PEAK_COMMIT_MB;

/// Free memory kept for the REST of the machine before a second heavy job is
/// admitted: the browser, the editor, and Windows itself. The CLI expresses
/// the same idea as a percentage (`jobs::BUDGET_PCT`); the GUI reserves an
/// absolute floor because it runs beside the user's whole desktop session,
/// whose size does not scale with free memory.
pub(crate) const RESERVED_HEADROOM_MB: u64 = 2_048;

/// Startup rayon-pool cap applied ONLY on an already-tight machine (see
/// [`render_threads`]). 8 keeps the measured single-pass cost near 20 % while
/// halving the demosaic's concurrent page-touch pressure.
pub(crate) const RAYON_THREADS_MAX: usize = 8;

/// MB of heavy commit currently reserved, guarded with its wait queue.
static INFLIGHT: Mutex<u64> = Mutex::new(0);
static RELEASED: Condvar = Condvar::new();

/// A byte reservation for one heavy pass. Dropping it releases the bytes and
/// wakes every queued worker (each re-checks the machine, not a ticket — free
/// memory may have changed while it waited).
pub(crate) struct HeavyPermit(u64);

impl Drop for HeavyPermit {
    fn drop(&mut self) {
        let mut held = INFLIGHT.lock().unwrap_or_else(|p| p.into_inner());
        *held = held.saturating_sub(self.0);
        RELEASED.notify_all();
    }
}

/// The admission decision as a pure function — the whole policy, testable
/// without a machine in a particular state (the `jobs::plan_with` seam,
/// applied here).
///
/// * the FIRST heavy job is always admitted (the `.max(1)` floor);
/// * an unmeasurable machine admits (a wrong guess must not serialise the
///   user's app — `jobs::free_memory_mb`'s own contract);
/// * otherwise the estimate plus the reserve must fit in CURRENT free memory,
///   which already reflects what the running jobs have actually touched.
pub(crate) fn admit(inflight_mb: u64, estimate_mb: u64, free_mb: Option<u64>) -> bool {
    if inflight_mb == 0 {
        return true;
    }
    match free_mb {
        None => true,
        Some(free) => estimate_mb.saturating_add(RESERVED_HEADROOM_MB) <= free,
    }
}

/// Reserve `estimate_mb` of heavy commit, WAITING on this (worker) thread
/// until [`admit`] says the machine can carry it. Call from worker bodies
/// only — the UI thread must never block here.
pub(crate) fn heavy_permit(estimate_mb: u64) -> HeavyPermit {
    let mut held = INFLIGHT.lock().unwrap_or_else(|p| p.into_inner());
    loop {
        if admit(*held, estimate_mb, autoshade::jobs::free_memory_mb()) {
            *held = held.saturating_add(estimate_mb);
            return HeavyPermit(estimate_mb);
        }
        eprintln!(
            "memory budget: ~{estimate_mb} MB pass queued behind {held} MB in flight \
             (free {} MB, reserve {RESERVED_HEADROOM_MB} MB)",
            autoshade::jobs::free_memory_mb().unwrap_or(0)
        );
        // Timed, not indefinite: the machine's free memory moves for reasons
        // no permit-drop announces (another app exiting), so a waiter
        // re-samples at least once a second.
        let (guard, _) = RELEASED
            .wait_timeout(held, std::time::Duration::from_secs(1))
            .unwrap_or_else(|p| p.into_inner());
        held = guard;
    }
}

/// The per-pass estimate for a source: what its own header says when that is
/// cheap to ask (baked masters — Lightroom's "Edit in…" TIFFs are the real
/// overshoot), floored at the corpus constant, exactly like
/// `jobs::survey_peak_mb`'s divisor. RAW headers answer `None` there and get
/// the floor.
pub(crate) fn estimate_mb(src: Option<&std::path::Path>) -> u64 {
    src.and_then(autoshade::decode::cheap_develop_peak_mb)
        .unwrap_or(0)
        .max(HEAVY_PEAK_COMMIT_MB)
}

/// The startup thread decision as a pure function: a machine already inside
/// the danger zone (free under reserve + two heavy peaks — i.e. two passes
/// could not overlap anyway) trades ~20 % single-pass wall for half the
/// page-touch rate; a healthy or unmeasurable machine keeps its full pool.
pub(crate) fn render_threads(cores: usize, free_mb: Option<u64>) -> usize {
    match free_mb {
        Some(free) if free < RESERVED_HEADROOM_MB + 2 * HEAVY_PEAK_COMMIT_MB => {
            cores.clamp(1, RAYON_THREADS_MAX)
        }
        _ => cores.max(1),
    }
}

/// Apply [`render_threads`] to the process-global rayon pool. Must run before
/// the first render (main(), before eframe); once any rayon work has run the
/// global pool is fixed and `build_global` errors — ignored deliberately,
/// leaving the default pool, because a budget must not be the thing that
/// fails the app.
pub(crate) fn clamp_global_rayon() {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let threads = render_threads(cores, autoshade::jobs::free_memory_mb());
    if threads < cores {
        eprintln!("memory budget: rayon pool clamped to {threads} of {cores} threads");
        let _ = rayon::ThreadPoolBuilder::new().num_threads(threads).build_global();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole admission policy, pinned decision by decision.
    #[test]
    fn admission_is_floor_one_then_headroom_gated() {
        // The first heavy job always runs, however tight the machine.
        assert!(admit(0, HEAVY_PEAK_COMMIT_MB, Some(100)));
        assert!(admit(0, u64::MAX, Some(0)));
        // An unmeasurable machine never queues (the CLI's no-cap contract).
        assert!(admit(HEAVY_PEAK_COMMIT_MB, HEAVY_PEAK_COMMIT_MB, None));
        // A second job needs its estimate plus the reserve in CURRENT free.
        let est = HEAVY_PEAK_COMMIT_MB;
        assert!(admit(est, est, Some(est + RESERVED_HEADROOM_MB)));
        assert!(!admit(est, est, Some(est + RESERVED_HEADROOM_MB - 1)));
        // Saturating: a huge estimate cannot wrap into admission.
        assert!(!admit(1, u64::MAX, Some(u64::MAX - 1)));
    }

    /// The reservation is RAII: released bytes wake a queued worker, and a
    /// poisoned-lock panic elsewhere cannot leak the reservation.
    #[test]
    fn heavy_permit_reserves_and_releases() {
        // Serialised against any other budget test via the real statics: use
        // distinctive sizes so a concurrent test's permit cannot be confused
        // with ours.
        let a = heavy_permit(7);
        assert!(*INFLIGHT.lock().unwrap() >= 7);
        drop(a);
        let held = *INFLIGHT.lock().unwrap();
        assert!(!(7..14).contains(&held), "our 7 MB reservation must be gone: {held}");
    }

    /// The startup thread clamp: tight machines cap, healthy and
    /// unmeasurable machines keep their cores, and the floor is one.
    #[test]
    fn thread_clamp_only_bites_on_a_tight_machine() {
        let tight = RESERVED_HEADROOM_MB + 2 * HEAVY_PEAK_COMMIT_MB - 1;
        assert_eq!(render_threads(16, Some(tight)), RAYON_THREADS_MAX);
        assert_eq!(render_threads(16, Some(tight + 1)), 16);
        assert_eq!(render_threads(16, None), 16);
        assert_eq!(render_threads(4, Some(tight)), 4);
        assert_eq!(render_threads(0, Some(tight)), 1);
        assert_eq!(render_threads(0, None), 1);
    }

    /// The estimate floors at the corpus constant — a small header must not
    /// under-reserve the demosaic transient, and a RAW (header answers
    /// nothing) gets the floor.
    #[test]
    fn estimate_floors_at_the_corpus_constant() {
        assert_eq!(estimate_mb(None), HEAVY_PEAK_COMMIT_MB);
        assert_eq!(
            estimate_mb(Some(std::path::Path::new("does-not-exist.tif"))),
            HEAVY_PEAK_COMMIT_MB
        );
    }

    /// A18 (v1.2.4). The wiring census: WHERE the permit is taken, and that
    /// each site actually holds it.
    ///
    /// [`heavy_permit`] is a reservation whose value must live as long as the
    /// work it pays for. Every policy test above proves the reservation is
    /// correct; none of them proves it is TAKEN, so a new full-frame commit
    /// added on a fresh worker was one forgotten line away from decoding a
    /// 60 MP raster beside two others while every admission test stayed green.
    ///
    /// Two things are pinned here, both read off the GUI's own source at
    /// runtime (the module tree is walked, as the inclusion law and the font
    /// gate do — an `include_str!` list would lose a new file silently).
    ///
    /// The SITES, file by file: a thirteenth is not necessarily wrong, but it
    /// is a new heavy path and it stops here until someone says so in this
    /// list. And the BINDING: `heavy_permit(...)` as a bare statement drops
    /// its guard at the semicolon and reserves nothing at all, which reads
    /// exactly like the working line and is the failure this census exists to
    /// catch. `let _ = heavy_permit(..)` has the same defect and is the same
    /// single character away, so the test demands a NAMED binding.
    #[test]
    fn every_budget_wiring_site_is_named_and_holds_its_permit() {
        fn walk_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("gui source dir listable") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    walk_rs(&path, out);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    out.push(path);
                }
            }
        }
        // The twelve wiring points, as of v1.2.4. `budget.rs` itself is the
        // definition and its own tests, not a wiring site.
        const CENSUS: [(&str, usize); 5] = [
            ("actions.rs", 3),
            ("export.rs", 2),
            ("masks.rs", 1),
            ("retouch.rs", 5),
            ("workers.rs", 1),
        ];
        let gui_src =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("bin").join("gui");
        let mut sources = Vec::new();
        walk_rs(&gui_src, &mut sources);
        assert!(sources.len() >= 6, "expected the split module tree, found {}", sources.len());
        let mut found: std::collections::BTreeMap<String, usize> = Default::default();
        for path in &sources {
            if path.file_name().is_some_and(|n| n == "budget.rs") {
                continue;
            }
            let text = std::fs::read_to_string(path).expect("gui source readable");
            let name = path.file_name().expect("file name").to_string_lossy().into_owned();
            for line in text.lines() {
                let Some(at) = line.find("crate::budget::heavy_permit(") else { continue };
                *found.entry(name.clone()).or_default() += 1;
                let binding = line[..at].trim_start();
                assert!(
                    binding.starts_with("let _") && !binding.starts_with("let _ ="),
                    "a permit must be BOUND or it is released at the semicolon \
                     and reserves nothing: {name}: {}",
                    line.trim(),
                );
            }
        }
        let expected: std::collections::BTreeMap<String, usize> =
            CENSUS.iter().map(|(name, n)| ((*name).to_string(), *n)).collect();
        assert_eq!(
            found, expected,
            "the budget wiring moved: every heavy full-frame path takes a permit, \
             and this census is where a new one is declared",
        );
        assert_eq!(found.values().sum::<usize>(), 12, "twelve wiring points");
    }
}

