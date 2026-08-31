//! The develop chain's DIAGNOSTICS CHANNEL — a sink the caller injects.
//!
//! # What this replaces, and why a prefix was not enough
//!
//! R28 Batch-5 5c stamped every worker-reachable `eprintln!` with the stem of
//! the photograph it belonged to, because `batch --jobs 3` interleaves three
//! photos' warnings on one stderr in COMPLETION order and the lines named no
//! photograph at all. That fixed attribution and nothing else, and its own
//! registration said so: it was a PREFIX, not a sink, and the deepest root the
//! R27 adjudication names (F6) is that this pipeline had no caller-supplied
//! diagnostics channel — the disclosure lines were hard-coded `eprintln!`s and
//! the only way to route, suppress or ORDER them was to be the process.
//!
//! This module is that channel. Three things change:
//!
//! * The attribution travels as DATA ([`Subject`]), not as a pre-formatted
//!   `"<stem>: "` string. A sink can group by photograph, drop the stem
//!   entirely (a per-photo block already names it), or key a UI row on it.
//! * The DESTINATION is the caller's ([`Sink`]). [`Stderr`] reproduces the
//!   shipped transcript byte for byte; [`Collector`] hands the lines back so
//!   the caller decides when and in what order they appear; [`Silent`] drops
//!   them for a surface whose real disclosure is somewhere else.
//! * The pure-pixel preview arm is no longer an undocumented exception. It has
//!   no photograph, and now says so in the type ([`Subject::PixelOnly`])
//!   instead of passing a `None` whose meaning lived in a comment.
//!
//! # The injection rule
//!
//! Two shapes, and which one a function takes is decided by what it already
//! knows:
//!
//! * An entry point that ALREADY carries the photograph (`render_to_file`,
//!   `produce_recipe`, `write_xmp`, …) takes `sink: &dyn Sink` and binds the
//!   subject itself. A caller cannot then hand it a subject that disagrees
//!   with the file it was asked to work on.
//! * A helper that does NOT carry it (`tag_icc`, the mask-raster loader,
//!   `ValidatedRecipe::disclose`) takes `&Diag` — the sink and the subject
//!   together, exactly the pair the old `photo: Option<&Path>` parameter was
//!   standing in for.
//!
//! # What is NOT routed through here
//!
//! This channel covers the DEVELOP CHAIN's per-photo disclosures — the lines a
//! pooled `batch` / `eval` worker raises while developing, rendering and
//! persisting one photograph. Bare `eprintln!`s elsewhere in `src/` that a
//! worker can transitively reach (the store's, the decoder's, the folder scan's,
//! the base-look estimator's) are NOT converted, and the gate in
//! `pipeline::tests` pins that boundary file by file so it cannot drift
//! silently: each of those already names its own photograph by stem or by path,
//! and threading a sink through the ~30 GUI/web/store call sites of
//! `photo_base_knots` / `repair_pre_era_base_curve` is a separate sweep, not a
//! side effect of this one.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// WHO a diagnostic is about.
///
/// The three states are exhaustive on purpose, and none of them is a string:
/// "no photograph" used to be a `None` that meant *the preview arm has only
/// pixels* in one place and *this fact belongs to the whole run* in another,
/// with the difference living in a comment beside the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    /// One photograph, by path. The default sink renders its stem.
    Photo(PathBuf),
    /// PIXELS, with no file behind them — the `develop_preview` arm is handed a
    /// buffer, a width and a height. There is no photograph to name and the
    /// type says so rather than a comment.
    PixelOnly,
    /// The RUN, not any one photograph — a once-per-process fact (the stale
    /// style index disables the Style slider for every photo in the batch, not
    /// for whichever worker reached the `Once` first).
    Run,
}

impl Subject {
    /// The photograph, when there is one.
    pub fn photo(&self) -> Option<&Path> {
        match self {
            Subject::Photo(p) => Some(p.as_path()),
            Subject::PixelOnly | Subject::Run => None,
        }
    }

    /// What the DEFAULT sink puts in front of the message — `"<stem>: "` for a
    /// photograph, nothing for the two subjects that have none (which is what
    /// those lines have always printed).
    pub fn stamp(&self) -> String {
        match self {
            Subject::Photo(p) => crate::pipeline::attribution(p),
            Subject::PixelOnly | Subject::Run => String::new(),
        }
    }
}

/// How a line OPENS on stderr, before its attribution.
///
/// Data rather than text baked into the message, for one concrete reason: a
/// sink that renders into a per-photo transcript block, a GUI row or a JSON
/// record must be able to drop the glyph and the leading spaces, and it cannot
/// if they are the first bytes of `Line::text`.
///
/// The four variants are the four openers the shipped CLI actually prints
/// today. They are kept apart so [`Stderr`] reproduces the existing transcript
/// byte for byte; unifying them on one marker is a product change, and a
/// refactor batch is the wrong place to make it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// `"⚠ "` — the app-wide disclosure glyph, and what most of these lines
    /// already wore.
    Warn,
    /// `"  ⚠ "` — [`Mark::Warn`] indented as a sub-line of a command's own
    /// output (the CLI's "recipe saved, but the XMP failed" family).
    WarnNested,
    /// `"warning: "` — the word, kept by the two recipe-clamp disclosures that
    /// have always spelled it that way.
    WarningWord,
    /// No opener at all — the aggregate mask-raster budget refusals.
    Bare,
}

impl Mark {
    /// The literal bytes [`Stderr`] emits for this mark.
    pub fn opener(self) -> &'static str {
        match self {
            Mark::Warn => "⚠ ",
            Mark::WarnNested => "  ⚠ ",
            Mark::WarningWord => "warning: ",
            Mark::Bare => "",
        }
    }
}

/// ONE diagnostic: who it is about, how it opens, and what it says.
///
/// `text` carries neither the opener nor the attribution — both are the
/// sink's to render, which is the whole difference between this and the prefix
/// it replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub subject: Subject,
    pub mark: Mark,
    pub text: String,
}

impl Line {
    /// The SHIPPED spelling: `<opener><stem>: <text>`, exactly the bytes the
    /// CLI printed before this module existed. Used by [`Stderr`], and by any
    /// caller that wants the familiar rendering on a stream of its own.
    pub fn shipped(&self) -> String {
        format!("{}{}{}", self.mark.opener(), self.subject.stamp(), self.text)
    }
}

/// Where diagnostics GO. Implemented by the caller when the default is wrong.
///
/// `Send + Sync` because the pooled surfaces raise these lines from worker
/// threads; a sink shared across a `jobs::for_each_indexed` scope is the
/// ordinary case, not the exotic one.
pub trait Sink: Send + Sync {
    fn emit(&self, line: Line);
}

/// The default: the process stderr, in the spelling that shipped.
///
/// This is the ONE `eprintln!` the develop chain is allowed to reach, and the
/// source-scan gate in `pipeline::tests` pins it at one.
pub struct Stderr;

impl Sink for Stderr {
    fn emit(&self, line: Line) {
        eprintln!("{}", line.shipped());
    }
}

/// Drops every line.
///
/// For a surface whose real disclosure is elsewhere and whose stderr copy
/// would be noise — the GUI mask list's `dead_bitmap_rasters` probe asks the
/// loader a question per frame and puts the answer ON THE ROW.
pub struct Silent;

impl Sink for Silent {
    fn emit(&self, _line: Line) {}
}

/// Holds the lines until the caller asks for them.
///
/// This is what makes the ORDER the caller's: a pooled worker collects its own
/// photograph's diagnostics and hands them to the transcript block the
/// sequencer releases in INDEX order, so a `--jobs 3` run no longer prints
/// three photos' warnings interleaved by whoever finished first.
#[derive(Default)]
pub struct Collector {
    lines: Mutex<Vec<Line>>,
}

impl Collector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything emitted so far, in emission order, leaving the collector
    /// empty. Poison is recovered rather than re-panicked: a panicking worker
    /// must not turn every other worker's drain into a second panic.
    pub fn take(&self) -> Vec<Line> {
        let mut guard = self.lines.lock().unwrap_or_else(|p| p.into_inner());
        std::mem::take(&mut *guard)
    }

    pub fn is_empty(&self) -> bool {
        self.lines.lock().unwrap_or_else(|p| p.into_inner()).is_empty()
    }
}

impl Sink for Collector {
    fn emit(&self, line: Line) {
        self.lines.lock().unwrap_or_else(|p| p.into_inner()).push(line);
    }
}

/// The process-wide default sink. A `&'static` so every entry point can take
/// `&dyn Sink` without the caller owning anything.
static STDERR_SINK: Stderr = Stderr;
static SILENT_SINK: Silent = Silent;

/// The default destination — stderr, in the shipped spelling.
pub fn stderr() -> &'static Stderr {
    &STDERR_SINK
}

/// The destination that discards.
pub fn silent() -> &'static Silent {
    &SILENT_SINK
}

/// A [`Sink`] BOUND to one subject: the pair every deep helper needs, and the
/// typed replacement for the `photo: Option<&Path>` parameters R28 threaded.
#[derive(Clone)]
pub struct Diag<'a> {
    sink: &'a dyn Sink,
    subject: Subject,
}

impl<'a> Diag<'a> {
    pub fn new(sink: &'a dyn Sink, subject: Subject) -> Self {
        Diag { sink, subject }
    }

    /// Bound to a photograph.
    pub fn about(sink: &'a dyn Sink, photo: &Path) -> Self {
        Diag::new(sink, Subject::Photo(photo.to_path_buf()))
    }

    /// Bound to pixels with no file behind them.
    pub fn pixels_only(sink: &'a dyn Sink) -> Self {
        Diag::new(sink, Subject::PixelOnly)
    }

    /// The same CHANNEL, re-bound to another subject — how a per-photo call
    /// raises the one fact that belongs to the run instead.
    pub fn rebound(&self, subject: Subject) -> Diag<'a> {
        Diag::new(self.sink, subject)
    }

    pub fn sink(&self) -> &'a dyn Sink {
        self.sink
    }

    pub fn subject(&self) -> &Subject {
        &self.subject
    }

    /// The photograph this channel is about, when it is about one. Callers that
    /// need the PATH (not just the attribution) read it from here rather than
    /// taking a second parameter that could disagree with the first.
    pub fn photo(&self) -> Option<&Path> {
        self.subject.photo()
    }

    /// Raise a `⚠` line.
    pub fn warn(&self, text: impl Into<String>) {
        self.emit(Mark::Warn, text);
    }

    /// Raise a line with an explicit opener — for the handful of sites whose
    /// shipped spelling is not `⚠`.
    pub fn emit(&self, mark: Mark, text: impl Into<String>) {
        self.sink.emit(Line { subject: self.subject.clone(), mark, text: text.into() });
    }
}

/// stderr, about `photo` — what an un-injected caller gets, and exactly what
/// the CLI printed before this module existed.
pub fn photo(p: &Path) -> Diag<'static> {
    Diag::about(stderr(), p)
}

/// stderr, about pixels with no file behind them.
pub fn pixels() -> Diag<'static> {
    Diag::pixels_only(stderr())
}

/// Discarded, about pixels with no file behind them — a probe whose answer is
/// the caller's return value, not a line on a console.
pub fn dropped() -> Diag<'static> {
    Diag::pixels_only(silent())
}

/// Render `lines` into a caller-owned transcript block, one per line, each
/// under `indent`.
///
/// The pooled surfaces already do exactly this with `produce_recipe`'s typed
/// `rationale::Note` channel (R28 Batch-5 5c); this is the same move for the
/// lines that used to go straight to the process stderr, and it is what puts
/// them in index order — the block, not the emission, decides when they print.
pub fn write_into_block(block: &mut String, indent: &str, lines: &[Line]) {
    use std::fmt::Write as _;
    for line in lines {
        // The stem stays: a block can be redirected, quoted into a bug report
        // or split from its header, and a line that names its own photograph
        // survives all three. The cost is one repetition per warning, which is
        // the trade R28 5c already made on stderr.
        let _ = writeln!(block, "{indent}{}", line.shipped());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    /// The DEFAULT sink is a rendering decision, not a channel: the same three
    /// facts must come out in the bytes the CLI shipped.
    #[test]
    fn the_default_rendering_is_the_shipped_spelling() {
        let photo = p("P34.ARW");
        assert_eq!(
            Line {
                subject: Subject::Photo(photo.clone()),
                mark: Mark::Warn,
                text: "GPT proposer failed (boom)".into(),
            }
            .shipped(),
            "⚠ P34: GPT proposer failed (boom)"
        );
        // No photograph = no stamp, which is what both of these printed before.
        assert_eq!(
            Line { subject: Subject::PixelOnly, mark: Mark::Bare, text: "x".into() }.shipped(),
            "x"
        );
        assert_eq!(
            Line { subject: Subject::Run, mark: Mark::WarningWord, text: "x".into() }.shipped(),
            "warning: x"
        );
        assert_eq!(
            Line { subject: Subject::PixelOnly, mark: Mark::WarnNested, text: "x".into() }
                .shipped(),
            "  ⚠ x"
        );
    }

    /// R29-1 acceptance ①.
    ///
    /// Three photos in flight on three workers, each raising its own failure,
    /// FINISHING in the order 2 → 0 → 1 (deterministically, through a turn
    /// cursor — not a sleep). Before this batch those three lines hit a shared
    /// stderr in exactly that completion order and the caller had no way to
    /// touch them.
    ///
    /// What is asserted is the property the sink buys, in three parts: each
    /// line is attributed to ITS OWN photograph, the transcript is in INDEX
    /// order, and — the premise, or the test is vacuous — the completion order
    /// really was not the index order.
    #[test]
    fn a_pooled_runs_diagnostics_are_ordered_by_the_caller_not_by_completion() {
        use std::sync::{Condvar, Mutex as StdMutex};

        const FINISH_ORDER: [usize; 3] = [2, 0, 1];
        let stems = ["DSC00001", "DSC00002", "DSC00003"];

        // A turn cursor: worker `i` may finish only when FINISH_ORDER[cursor]
        // == i. Deterministic where a sleep would only be probable.
        let turn = StdMutex::new(0usize);
        let bell = Condvar::new();
        let completed: StdMutex<Vec<usize>> = StdMutex::new(Vec::new());

        let blocks = crate::jobs::for_each_indexed(3, 3, |i, block| {
            // The caller's channel for THIS photo — one collector per worker,
            // which is what makes the lines separable at all.
            let sink = Collector::new();
            let photo = p(&format!("{}.ARW", stems[i]));
            let diag = Diag::about(&sink, &photo);
            diag.warn(format!("GPT proposer failed (constructed failure {i})"));

            let mut guard = turn.lock().unwrap_or_else(|e| e.into_inner());
            while FINISH_ORDER[*guard] != i {
                // Timed, so a scheduling surprise fails the test instead of
                // hanging CI forever.
                let (g, timeout) = bell
                    .wait_timeout(guard, std::time::Duration::from_secs(30))
                    .unwrap_or_else(|e| e.into_inner());
                guard = g;
                assert!(!timeout.timed_out(), "worker {i} never got its turn");
            }
            completed.lock().unwrap_or_else(|e| e.into_inner()).push(i);
            *guard += 1;
            drop(guard);
            bell.notify_all();

            // The drain: the diagnostics land in this photo's own block, which
            // the sequencer releases in index order.
            write_into_block(block, "       ", &sink.take());
            block.clone()
        });

        // PREMISE: completion order really was scrambled.
        assert_eq!(
            completed.into_inner().unwrap_or_else(|e| e.into_inner()),
            FINISH_ORDER.to_vec(),
            "the fixture failed to construct a non-index completion order"
        );
        // ORDER is the caller's: block i is photo i's, whatever finished first.
        for (i, stem) in stems.iter().enumerate() {
            let got = blocks[i].as_deref().expect("every job ran");
            assert_eq!(
                got,
                format!("       ⚠ {stem}: GPT proposer failed (constructed failure {i})\n"),
                "block {i} must carry photo {stem}'s line and nothing else"
            );
        }
    }

    /// Suppression and routing are the caller's, which is the other half of
    /// what a prefix could not do.
    #[test]
    fn a_caller_can_drop_or_keep_the_lines() {
        let sink = Collector::new();
        let photo = p("DSC00009.ARW");
        Diag::about(&sink, &photo).warn("kept");
        Diag::about(silent(), &photo).warn("dropped");
        let lines = sink.take();
        assert_eq!(lines.len(), 1, "the silent sink must not reach the collector");
        assert_eq!(lines[0].text, "kept");
        assert_eq!(lines[0].subject, Subject::Photo(photo));
        assert!(sink.is_empty(), "take() drains");
    }

    /// The subject is DATA: a sink can group by it without parsing a prefix.
    #[test]
    fn the_subject_travels_as_data() {
        let sink = Collector::new();
        let a = p("A.ARW");
        let d = Diag::about(&sink, &a);
        d.warn("about the photo");
        d.rebound(Subject::Run).warn("about the run");
        d.rebound(Subject::PixelOnly).warn("about the pixels");
        let lines = sink.take();
        assert_eq!(lines[0].subject.photo(), Some(a.as_path()));
        assert_eq!(lines[1].subject, Subject::Run);
        assert_eq!(lines[2].subject, Subject::PixelOnly);
        // …and the run/pixel lines carry no stem, so re-binding cannot invent
        // an attribution the caller does not have.
        assert_eq!(lines[1].shipped(), "⚠ about the run");
        assert_eq!(lines[2].shipped(), "⚠ about the pixels");
    }
}
