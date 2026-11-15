//! Is a panic on THIS thread one that AutoShade is going to contain?
//!
//! Two `catch_unwind` guards exist for exactly that purpose —
//! [`crate::decode::guard_parser_panic`] around every third-party parser call,
//! and the GUI's `spawn_worker` around every worker body — and both turn a
//! panic into a sentence the photographer reads and acts on. The GUI's panic
//! HOOK (`src/bin/gui/main.rs`) runs on every panic, contained or not, and had
//! no way to tell the two apart: one unreadable file raised a modal saying
//! "AutoShade hit an internal error and must close" over an app that was
//! neither closing nor broken. `install_panic_reporter`'s own stated rule is
//! that the claim must match the outcome (R12-10); this module is what lets it
//! keep that rule for the contained half.
//!
//! Thread-local and RAII, both load-bearing. The hook runs ON the panicking
//! thread BEFORE unwinding starts, so a flag set here is still set when the
//! hook reads it — and `Drop`, which runs during that unwind (after the hook,
//! before `catch_unwind` returns), is the only restore that survives the very
//! panic it exists to describe.
//!
//! Every access is `try_with`: a hook that panicked while reporting a panic
//! would abort the process, which is the outcome this whole file exists to
//! avoid.

use std::cell::Cell;

thread_local! {
    /// A depth, not a bool: the guards NEST — a worker body runs parser calls
    /// inside itself — and with a bool the inner guard's exit would clear the
    /// outer one's, leaving the rest of a still-contained worker looking fatal.
    static RECOVERABLE_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Marks its scope as one whose panic an AutoShade `catch_unwind` will catch.
/// Hold it for exactly as long as that `catch_unwind` body runs — the RAII
/// shape is what makes the restore happen on the panicking path too.
#[must_use = "the scope lasts only as long as this value — a bare `Recoverable::enter();` \
              ends it on the same line"]
pub struct Recoverable(());

impl Recoverable {
    /// Enter a contained scope. Saturating, so an implausible depth cannot
    /// wrap around into "fatal" and mis-describe a contained panic.
    pub fn enter() -> Self {
        let _ = RECOVERABLE_DEPTH.try_with(|d| d.set(d.get().saturating_add(1)));
        Self(())
    }
}

impl Drop for Recoverable {
    fn drop(&mut self) {
        let _ = RECOVERABLE_DEPTH.try_with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Would a panic raised right here be caught by one of AutoShade's own
/// `catch_unwind` guards? Read from the panic hook, which must treat an
/// unanswerable question as "fatal" — announcing a crash that did not happen
/// is a smaller lie than staying silent about one that did.
pub fn panic_is_recoverable() -> bool {
    RECOVERABLE_DEPTH.try_with(|d| d.get() > 0).unwrap_or(false)
}

/// The calling thread's containment, carried into work it hands to another
/// thread. The mark is per thread, so a body moved onto a pool thread — a
/// solve run beside its siblings (`fit::joint::side_by_side`) — would raise
/// its panic on ground the hook reads as fatal, although the pool hands that
/// panic back to the thread that waits and the guard there catches it. Taken
/// where the work is handed out, entered where it runs.
#[derive(Clone, Copy)]
pub struct Carried(bool);

impl Carried {
    /// The containment of the thread that hands the work out.
    pub fn here() -> Self {
        Self(panic_is_recoverable())
    }

    /// Run `body` under the containment [`Carried::here`] took.
    pub fn run<R>(self, body: impl FnOnce() -> R) -> R {
        let _scope = self.0.then(Recoverable::enter);
        body()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scope_nests_and_a_caught_panic_still_restores_it() {
        assert!(!panic_is_recoverable(), "a bare thread is fatal ground");
        {
            let _outer = Recoverable::enter();
            assert!(panic_is_recoverable());
            {
                let _inner = Recoverable::enter();
                assert!(panic_is_recoverable());
            }
            // MUTATION THIS KILLS: a bool instead of a depth — the inner
            // scope's exit would have cleared the outer one's here.
            assert!(panic_is_recoverable(), "the outer scope outlives the inner");
        }
        assert!(!panic_is_recoverable(), "both scopes ended");

        // The restore that matters: Drop runs while UNWINDING, so a thread
        // that caught a panic inside the scope is fatal ground again after.
        let caught = std::panic::catch_unwind(|| {
            let _g = Recoverable::enter();
            assert!(panic_is_recoverable());
            panic!("contained");
        });
        assert!(caught.is_err(), "the panic was caught, not swallowed");
        assert!(!panic_is_recoverable(), "the guard unwound with the panic");
    }

    /// Before, inside and after the handed-out body, read on the thread it
    /// was handed to. MUTATION THIS KILLS: `Carried::run` not entering the
    /// scope — the middle reading is then fatal ground under a contained
    /// caller.
    #[test]
    fn work_handed_to_another_thread_runs_under_the_callers_containment() {
        let on_another_thread = |carried: Carried| {
            std::thread::spawn(move || {
                (panic_is_recoverable(), carried.run(panic_is_recoverable), panic_is_recoverable())
            })
            .join()
            .expect("the thread ran")
        };
        let contained = {
            let _g = Recoverable::enter();
            on_another_thread(Carried::here())
        };
        assert_eq!(contained, (false, true, false), "a contained caller's work is contained, and only it");
        assert_eq!(on_another_thread(Carried::here()), (false, false, false), "a bare caller hands out bare ground");
    }
}
