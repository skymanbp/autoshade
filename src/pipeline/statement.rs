//! The STATEMENT path of the analysis: the vision model states the finished
//! LOOK of each area of the neutral frame in CIELAB numbers
//! ([`crate::advisor::statement`]), the frame is moved to those numbers
//! ([`crate::advisor::look::moved`]) — a synthetic target that is the same
//! photograph pixel for pixel — and the ordinary same-scene reverse fit
//! ([`super::fit_run::run_fit`]) finds the recipe that renders it.
//!
//! Only for a RAW analysed fresh: the frame is the fit's own source developed
//! under the photo's calibration ("the room the solve runs in"), which an
//! ADJUST, a baked file or caller-supplied pixels do not have.
//!
//! Everything that touches the outside — the RAW's develop, the segmenter,
//! the reference pictures, the fit, the deliverable render — goes through
//! [`StatementTools`], so the tests drive the path with fakes.

use std::path::Path;

use anyhow::{Context, Result};

use crate::advisor::look::{self, AreaLook, AreaName};
use crate::advisor::statement::{ShownReference, Statement, StatementAsk};
use crate::config::Config;
use crate::rationale::{Note, keys};
use crate::recipe::EditRecipe;

/// The reverse fit's settings on the statement path, in ONE place: full
/// strength (the statement already says how far to go), local areas on with
/// the default number of semantic regions, and no correspondence provider —
/// the target is the same scene pixel for pixel, so there is nothing to pair
/// across content.
pub struct LookFitSettings {
    pub strength: f32,
    pub zoned: bool,
    pub regions: usize,
}

pub const LOOK_FIT: LookFitSettings =
    LookFitSettings { strength: 1.0, zoned: true, regions: crate::fit_zoned::semantic::DEFAULT_SEMANTIC_REGIONS };

/// What the fit hands back to the statement path.
pub struct LookFitResult {
    pub recipe: EditRecipe,
    pub notes: Vec<Note>,
    pub err_before: f32,
    pub err_after: f32,
}

type SourceFn<'a> = Box<dyn Fn(&Path) -> Result<(crate::render::Plate, EditRecipe)> + 'a>;
type SkyFn<'a> = Box<dyn Fn(&image::DynamicImage) -> Result<image::GrayImage> + 'a>;
type ReferenceFn<'a> = Box<dyn Fn(&Path) -> Result<Vec<u8>> + 'a>;
type FitFn<'a> = Box<dyn Fn(&Path, &mut crate::render::Plate, &mut EditRecipe, &image::DynamicImage) -> Result<LookFitResult> + 'a>;
type RenderFn<'a> = Box<dyn Fn(&Path, &EditRecipe) -> Result<image::DynamicImage> + 'a>;

/// The statement path's contact with the outside world.
pub struct StatementTools<'a> {
    /// Is this photo a camera RAW (the only source the path runs on)?
    pub is_raw: Box<dyn Fn(&Path) -> bool + 'a>,
    /// The fit's source plate of the RAW and the calibration composed into the
    /// solve ([`super::fit_source`]).
    pub source: SourceFn<'a>,
    /// The sky mask of a frame (any size; resized to the frame), or why not.
    pub sky: SkyFn<'a>,
    /// A reference photograph's picture, a JPEG at the advisor's edge.
    pub reference: ReferenceFn<'a>,
    /// The reverse fit of the plate onto the target under the calibration, as
    /// [`LOOK_FIT`] sets it.
    pub fit: FitFn<'a>,
    /// The deliverable of a recipe rendered from the RAW at the advisor's edge.
    pub render: RenderFn<'a>,
    /// Where to write what one statement produced ([`dump`]), or `None`. A
    /// diagnostic switch: production reads `AUTOSHADE_STATEMENT_DUMP` once.
    pub dump: Option<std::path::PathBuf>,
}

impl<'a> StatementTools<'a> {
    /// The production tools: the RAW's own develop, the local segmenter, the
    /// library's reference pictures, the shared reverse fit and the export's
    /// render, with progress and disclosures on `sink`.
    pub fn production(cfg: &'a Config, sink: &'a dyn crate::diag::Sink) -> Self {
        StatementTools {
            is_raw: Box::new(crate::decode::is_raw),
            source: Box::new(super::fit_source),
            sky: Box::new(move |frame| sky_of(cfg, frame)),
            reference: Box::new(super::reference_preview),
            fit: Box::new(move |raw, plate, base, target| production_fit(cfg, raw, plate, base, target, sink)),
            render: Box::new(move |raw, recipe| {
                let _permit = crate::decode::DecodePermit::acquire();
                crate::render::render_to_image_in(
                    raw,
                    recipe,
                    None,
                    Some(super::ADVISOR_PREVIEW_EDGE),
                    crate::render::ExportColorSpace::Srgb,
                    sink,
                )
            }),
            dump: crate::config::live_env_os("AUTOSHADE_STATEMENT_DUMP").map(std::path::PathBuf::from),
        }
    }
}

/// A unique scratch path under the temp root.
fn scratch(tag: &str, ext: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("autoshade-statement-{tag}-{}-{nanos}-{n}.{ext}", std::process::id()))
}

/// The sky mask of `frame` by the local segmenter: the frame staged to a temp
/// PNG, segmented, read back bounded; both temp files removed.
fn sky_of(cfg: &Config, frame: &image::DynamicImage) -> Result<image::GrayImage> {
    let input = scratch("frame", "png");
    let output = scratch("sky", "png");
    let run = || -> Result<image::GrayImage> {
        frame.to_rgb8().save(&input).context("stage the frame for the segmenter")?;
        crate::segment::segment_file(&crate::segment::SegmentOpts::from_config(cfg, "sky"), &input, &output)?;
        Ok(crate::render::open_mask_bounded(&output)?.to_luma8())
    };
    let mask = run();
    // Best effort: a temp file that cannot be removed is left to the OS.
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&output);
    mask
}

/// The production fit: the target put in the source's frame
/// ([`super::fit_target`]), then the shared sequence ([`super::fit_run::run_fit`])
/// with [`LOOK_FIT`]'s settings.
fn production_fit(
    cfg: &Config,
    raw: &Path,
    plate: &mut crate::render::Plate,
    base: &mut EditRecipe,
    target: &image::DynamicImage,
    sink: &dyn crate::diag::Sink,
) -> Result<LookFitResult> {
    let fitted = super::fit_target(plate, Some(raw), target, base)?;
    let seg = crate::segment::SegmentOpts::from_config(cfg, "sky");
    let zoned = LOOK_FIT.zoned.then_some(super::fit_run::ZonedRun { photo: raw, seg: &seg, regions: LOOK_FIT.regions });
    let (report, _claimed) = super::fit_run::run_fit(&super::fit_run::FitRun {
        plate,
        target: &fitted,
        base,
        options: crate::fit::FitOptions { strength: crate::recipe::GradeStrength::new(LOOK_FIT.strength), provider: None },
        zoned,
        reread: Some(raw),
        sink,
    })?;
    Ok(LookFitResult { recipe: report.recipe, notes: report.notes, err_before: report.err_before, err_after: report.err_after })
}

/// What the statement path produced.
pub struct StatementOutcome {
    /// The fitted recipe: the calibration it was composed with, the fit's own
    /// rationale and the statement's notes.
    pub recipe: EditRecipe,
    /// The model's sentence naming the look.
    pub look: String,
    /// The reference numbers the model used as direct guides.
    pub guides: Vec<usize>,
    /// The typed notes rendering the rationale's deterministic suffix.
    pub notes: Vec<Note>,
}

/// A frame's areas and what each measures, plus its Lab (for the move).
struct Measured {
    lab: Vec<[f32; 3]>,
    areas: Vec<look::Area>,
    looks: Vec<(AreaName, AreaLook)>,
}

/// `mask` at `w` x `h` (bilinear when it is another size).
fn mask_at(mask: &image::GrayImage, w: u32, h: u32) -> image::GrayImage {
    if mask.dimensions() == (w, h) {
        mask.clone()
    } else {
        image::imageops::resize(mask, w, h, image::imageops::FilterType::Triangle)
    }
}

/// Measure `img` in the statement's areas, the sky weight read off `sky`
/// (resized to the image) or, without one, the whole frame as one area.
fn measure_frame(img: &image::DynamicImage, sky: Option<&image::GrayImage>) -> Measured {
    let rgb = img.to_rgb8();
    let lab = look::lab_of_image(&rgb);
    let weight: Vec<f32> = match sky {
        Some(m) => mask_at(m, rgb.width(), rgb.height()).pixels().map(|p| f32::from(p.0[0]) / 255.0).collect(),
        None => vec![0.0; lab.len()],
    };
    let areas = look::areas_of(&weight);
    let looks = areas.iter().map(|a| (a.name, look::measure(&lab, &a.weight))).collect();
    Measured { lab, areas, looks }
}

fn jpeg_of(img: &image::DynamicImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    img.to_rgb8().write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg).context("encode JPEG")?;
    Ok(out)
}

/// One reference prepared for the request: its picture and, when they could
/// be measured, its areas.
struct PreparedReference {
    number: usize,
    jpeg: Vec<u8>,
    areas: Option<Vec<(AreaName, AreaLook)>>,
}

/// The references SHOWN to the model (only when the opt-in is on): each
/// hit's picture, measured in the statement's areas — from the store's cache
/// when its entry was measured before ([`super::statement_cache`]). A
/// reference whose picture cannot be prepared is left out and named in the
/// returned error line; one whose sky cannot be segmented goes without numbers.
fn prepare_references(
    hits: &[crate::library::Hit<'_>],
    tools: &StatementTools<'_>,
) -> (Vec<PreparedReference>, Option<String>) {
    let mut out = Vec::new();
    let mut err: Option<String> = None;
    for (i, h) in hits.iter().enumerate() {
        let jpeg = match (tools.reference)(Path::new(&h.entry.path)) {
            Ok(j) => j,
            Err(e) => {
                err.get_or_insert_with(|| crate::rationale::error_line(&e));
                continue;
            }
        };
        let areas = super::statement_cache::load(h.entry).or_else(|| {
            let img = image::load_from_memory(&jpeg).ok()?;
            let sky = (tools.sky)(&img).ok()?;
            let looks = measure_frame(&img, Some(&sky)).looks;
            super::statement_cache::store(h.entry, &looks);
            Some(looks)
        });
        out.push(PreparedReference { number: i + 1, jpeg, areas });
    }
    (out, err)
}

/// The statement path for ONE fresh RAW analysis: measure the neutral frame,
/// ask for the look, move the frame to it, fit the RAW onto that target.
///
/// `hits` are the reference library's results for this develop (empty when
/// the direction guides it or nothing was found); `direction` the
/// photographer's typed words when the direction guides it.
#[allow(clippy::too_many_arguments)]
pub fn statement_recipe(
    raw: &Path,
    cfg: &Config,
    req: &super::GradeRequest,
    direction: Option<&str>,
    hits: &[crate::library::Hit<'_>],
    tools: &StatementTools<'_>,
    sink: &dyn crate::diag::Sink,
) -> Result<StatementOutcome> {
    let diag = crate::diag::Diag::about(sink, raw);
    let say = |s: &str| diag.emit(crate::diag::Mark::Bare, format!("stated look: {s}"));
    let mut notes: Vec<Note> = Vec::new();
    // 1. The fit's own source and calibration, and the neutral frame they make.
    let (mut plate, mut fit_base) = (tools.source)(raw).context("develop the neutral frame for the stated look")?;
    let frame = crate::render::develop_preview(&plate, &fit_base);
    // 2. The sky of that frame; without one, the frame is one area.
    let sky = match (tools.sky)(&frame) {
        Ok(m) => Some(m),
        Err(e) => {
            notes.push(Note::new(keys::STATEMENT_NO_SKY, vec![("e", crate::rationale::error_line(&e))]));
            None
        }
    };
    // 3. Measure the frame; the references too, when their pictures go.
    let measured = measure_frame(&frame, sky.as_ref());
    let (references, ref_err) =
        if req.send_reference_image && !hits.is_empty() { prepare_references(hits, tools) } else { (Vec::new(), None) };
    if let Some(e) = ref_err {
        notes.push(Note::new(keys::STYLE_REF_IMAGE_FAILED, vec![("e", e)]));
    }
    // Each hit's words in the statement's own voice (never the proposer's
    // reference block, whose steering sentence and settings lines do not
    // belong in a call that chooses no sliders).
    let phrases: Vec<Option<String>> = hits.iter().map(crate::library::phrases_line).collect();
    let shown: Vec<ShownReference<'_>> = references
        .iter()
        .map(|r| ShownReference { number: r.number, areas: r.areas.as_deref(), jpeg: &r.jpeg })
        .collect();
    // 4. ONE image-role call, the frame at the advisor's edge.
    let edge = super::ADVISOR_PREVIEW_EDGE;
    let frame_jpeg = jpeg_of(&crate::render::thumbnail(&frame, edge, edge))?;
    say("asking the image model for the finished look of each area");
    let statement: Statement = crate::advisor::statement::ask_statement(
        cfg,
        &StatementAsk {
            frame_jpeg: &frame_jpeg,
            areas: &measured.looks,
            direction,
            adherence: req.adherence,
            strength: req.strength,
            phrases: &phrases,
            shown: &shown,
        },
    )
    .map_err(anyhow::Error::from)
    .context("the look statement call")?;
    // 5. The synthetic target: the frame at the plate's own size, moved.
    let moved = look::moved(&measured.lab, &measured.areas, &statement.areas)?;
    let (w, h) = (frame.width(), frame.height());
    let target = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb(look::rgb8_of(moved[(y * w + x) as usize]))
    }));
    if let Some(dir) = &tools.dump
        && let Err(e) = dump(dir, &frame, sky.as_ref(), &target, &statement, &measured.looks)
    {
        diag.warn(format!("the stated-look dump could not be written ({e:#})"));
    }
    // 6. The reverse fit onto it.
    say("reverse-fitting the RAW onto the stated look (local compute; this can take many minutes)");
    let fitted = (tools.fit)(raw, &mut plate, &mut fit_base, &target).context("the reverse fit onto the stated look")?;
    say(&format!("look error {:.3} -> {:.3}", fitted.err_before, fitted.err_after));
    // 7. The recipe as a fit finishes it — the calibration is composed into
    // the solve, so nothing is stamped afterwards — with the statement's notes.
    let mut recipe = fitted.recipe;
    let mut all_notes = fitted.notes;
    notes.insert(0, Note::new(keys::STATEMENT_LOOK, vec![("look", statement.look.clone())]));
    let files: Vec<String> =
        statement.guides.iter().filter_map(|&n| hits.get(n - 1)).map(|h| super::stem(Path::new(&h.entry.path)).to_string()).collect();
    if !files.is_empty() {
        notes.insert(1, Note::new(keys::STATEMENT_GUIDES, vec![("n", files.len().to_string()), ("files", files.join(", "))]));
    }
    for n in notes {
        crate::rationale::push_note(&mut recipe.rationale, &mut all_notes, n);
    }
    Ok(StatementOutcome { recipe, look: statement.look, guides: statement.guides, notes: all_notes })
}

/// The diagnostic dump (`AUTOSHADE_STATEMENT_DUMP`): what one paid statement
/// produced, enough to fit it again without paying — the neutral frame that
/// was measured, the sky mask at its size, the synthetic target the fit was
/// handed, and the validated statement with each area's measured numbers
/// beside its stated ones. No file name, no key, no request text.
fn dump(
    dir: &Path,
    frame: &image::DynamicImage,
    sky: Option<&image::GrayImage>,
    target: &image::DynamicImage,
    statement: &Statement,
    measured: &[(AreaName, AreaLook)],
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    frame.to_rgb8().save(dir.join("statement-frame.png"))?;
    if let Some(m) = sky {
        mask_at(m, frame.width(), frame.height()).save(dir.join("statement-sky.png"))?;
    }
    target.to_rgb8().save(dir.join("statement-target.png"))?;
    let areas: Vec<serde_json::Value> = statement
        .areas
        .iter()
        .map(|(name, s)| {
            serde_json::json!({
                "area": name.as_str(),
                "stated": s,
                "measured": measured.iter().find(|(n, _)| n == name).map(|(_, m)| m),
            })
        })
        .collect();
    let reply = serde_json::json!({ "look": statement.look, "guides": statement.guides, "areas": areas });
    std::fs::write(dir.join("statement-reply.json"), serde_json::to_string_pretty(&reply)?)?;
    Ok(())
}

#[cfg(test)]
#[path = "statement_tests.rs"]
mod tests;
