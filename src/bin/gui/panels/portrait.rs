//! The Portraits mode's panel (design-portraits-mode.md §3, the user's ruling
//! of 2026-10-23: the full workspace, and one "Portraits" panel at the top of
//! the sidebar). Drawn above the shared panels when the mode's
//! `Shell::portrait_panel` is on (`app.rs` `controls_column`); nothing else is
//! hidden.
//!
//! Four groups, in the order a portrait is worked: find the people and the
//! faces (and an AI look made for portraits), the skin, the eyes, the
//! background. The finding is two sidecars on the original-frame preview —
//! the subject matte (`segment.py --target subject`) and the face finder
//! (`face.py`, MediaPipe) — and lands four ordinary bitmap masks carrying
//! their `MaskRole` (`autoshade::face`): Face, Eyes, Skin (the face narrowed
//! by a colour range seeded from the face) and Background (the subject,
//! inverted). Every slider here writes a field of the mask with that role,
//! or of the lens blur, so the recipe format does not change and every one
//! of them is also in the mask list below, editable there.
//!
//! "Keep skin natural" (§4) is a switch of this panel, on by default, not a
//! recipe field. While it is on, every write of the recipe in this mode — a
//! slider here or in the mask list, a paste, an AI landing — meets the same
//! limits at one place (`guard_portrait_skin`, run once per frame after the
//! panels drew and the workers landed). A write that goes to DISK in the
//! same step — an AI landing's save, a paste onto other photos — meets them
//! where it is written (`skin_held` / `skin_held_for`), before the file.

use crate::*;
use autoshade::recipe::{LocalAdjustment, MaskGeometry, MaskRole};

/// How much the skin and eyes sliders move their fields. Softening is
/// texture down by the slider and clarity down by half of it — the
/// guardrail's two floors (−60 / −40) meet at the same slider position.
const SOFTEN_CLARITY_SHARE: f32 = 0.5;
/// The eyes' "Crisp" slider: sharpness by the slider, clarity by half.
const CRISP_CLARITY_SHARE: f32 = 0.5;
/// The background's "Darken" slider: 100 is two stops down.
const DARKEN_EV_AT_FULL: f32 = 2.0;
/// The eyes' "Brighten" slider's reach, in stops.
const EYES_BRIGHTEN_MAX_EV: f32 = 1.5;

/// What the find step found, as facts for the landing (L12#4: worded in the
/// language live when it lands).
pub(crate) struct PortraitFind {
    /// The photo it ran on: a landing for another photo is dropped.
    pub(crate) photo: PathBuf,
    /// The subject matte's raster, or why there is none.
    pub(crate) subject: Result<PathBuf, String>,
    /// The face finder's answer: `Ok(None)` = it ran and found no face.
    pub(crate) faces: Result<Option<FoundFaces>, String>,
}

/// The faces of one find step: how many, the two rasters, and the skin
/// colour seed (`face::skin_seed`; `None` when the face holds no pixel).
pub(crate) struct FoundFaces {
    pub(crate) n: usize,
    pub(crate) face: PathBuf,
    pub(crate) eyes: PathBuf,
    pub(crate) seed: Option<([f32; 3], [f32; 2])>,
}

/// The one line under the first group.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum PortraitLine {
    #[default]
    Idle,
    Finding,
    /// The finding landed: faces found, and what failed, in the error's
    /// own words (the subject matte or the face finder).
    Found { faces: usize, failed: Option<String> },
    /// Nothing could be found at all.
    Failed(String),
    Looking,
    Looked,
}

/// The Portraits panel's own state. Nothing here is persisted (the
/// preferences file is unchanged); the masks it makes live in the recipe.
pub(crate) struct PortraitState {
    pub(crate) line: PortraitLine,
    /// The photo the last find step ran on, and how many faces it found —
    /// which words the greyed groups use ("no face found" vs "find them
    /// first").
    pub(crate) found: Option<(PathBuf, usize)>,
    /// "Keep skin natural" (design §4), per photo and card: (photo, card
    /// id) → on; a card with no entry is on. Opening never changes a value:
    /// when a photo or one of its saved versions is read from disk, every
    /// card whose saved edit already goes past the limits is entered OFF
    /// (`decide_keep_skin_for_photo` / `_for_card`), and nothing is clamped
    /// there until the user turns it on (`set_keep_skin`). A card made in
    /// this session (an AI look, a fit, a fork's default) starts on, so the
    /// app's own writes meet the limits; a fork inherits its parent's switch
    /// (`inherit_keep_skin`).
    pub(crate) keep_skin: std::collections::HashMap<(PathBuf, String), bool>,
    /// The analysis epoch an AI portrait look started under, and the
    /// portrait masks and lens blur the photo had before it: a fresh look
    /// replaces the recipe whole, and these come back on top of it.
    pub(crate) look: Option<(u64, Vec<LocalAdjustment>, Option<autoshade::render::LensBlur>)>,
    /// A depth estimate was started from the Background group: its landing
    /// aims the focus at the face and keeps the person sharp.
    pub(crate) blur_from_panel: bool,
    /// Test seam: what the last heal was started with (painted mask?, the
    /// limit raster) — `start_heal`, shared by the Retouch panel's heal and
    /// this panel's 🤖 Remove blemishes.
    #[cfg(test)]
    pub(crate) heal_asked: Option<(bool, Option<PathBuf>)>,
    /// Test seam: stands in for `retouch::heal` inside the heal worker.
    #[cfg(test)]
    pub(crate) heal_runner: Option<HealRunner>,
}

/// What the heal worker runs: `retouch::heal`, or a test's stand-in for it
/// — (the source, the limit raster, the output).
pub(crate) type HealRunner =
    fn(&std::path::Path, Option<&std::path::Path>, &std::path::Path) -> anyhow::Result<autoshade::retouch::HealReport>;

impl Default for PortraitState {
    fn default() -> Self {
        PortraitState {
            line: PortraitLine::Idle,
            found: None,
            keep_skin: Default::default(),
            look: None,
            blur_from_panel: false,
            #[cfg(test)]
            heal_asked: None,
            #[cfg(test)]
            heal_runner: None,
        }
    }
}

/// The four roles the find step makes.
const PORTRAIT_ROLES: [MaskRole; 4] = [MaskRole::Face, MaskRole::Eyes, MaskRole::Skin, MaskRole::Background];

/// The find step itself, off the UI thread: the subject matte and the faces
/// on the original-frame preview, every raster under a freshly claimed name
/// beside the photo's develop. The skin seed is sampled from `base` as
/// `recipe` develops it up to the skin mask ([`panel_skin_seed`]); `film` /
/// `raw_source` are the canvas's (`film_short_edge`, `source_is_raw`).
pub(crate) fn find_people(
    base: &image::DynamicImage,
    photo: &std::path::Path,
    recipe: &EditRecipe,
    film: Option<u32>,
    raw_source: bool,
) -> PortraitFind {
    let cfg = autoshade::config::Config::load();
    let staged = match crate::masks::stage_preview_png(base, "portrait") {
        Ok(p) => p,
        Err(e) => {
            let why = format!("{e:#}");
            return PortraitFind { photo: photo.to_path_buf(), subject: Err(why.clone()), faces: Err(why) };
        }
    };
    let subject = (|| -> anyhow::Result<PathBuf> {
        let opts = autoshade::segment::SegmentOpts::from_config(&cfg, "subject");
        let mask = autoshade::store::claim_raster(photo, "mask-subject")?;
        autoshade::segment::segment_file(&opts, &staged, &mask).inspect_err(|_| {
            // A failed run releases its claimed (0-byte) name.
            let _ = std::fs::remove_file(&mask);
        })?;
        Ok(mask)
    })()
    .map_err(|e| format!("{e:#}"));
    let json = staged.with_extension("json");
    let faces = (|| -> anyhow::Result<Option<FoundFaces>> {
        let report = autoshade::face::detect_faces(&cfg, &staged, &json)?;
        if report.faces.is_empty() {
            return Ok(None);
        }
        let (w, h) = (base.width(), base.height());
        let face_mask = autoshade::face::face_mask(&report, w, h);
        let eyes_mask = autoshade::face::eyes_mask(&report, w, h);
        let seed = panel_skin_seed(base, &face_mask, recipe, film, raw_source);
        let face = autoshade::face::save_mask(&face_mask, photo, "mask-face")?;
        let eyes = autoshade::face::save_mask(&eyes_mask, photo, "mask-eyes")?;
        Ok(Some(FoundFaces { n: report.faces.len(), face, eyes, seed }))
    })()
    .map_err(|e| format!("{e:#}"));
    let _ = std::fs::remove_file(&staged);
    let _ = std::fs::remove_file(&json);
    PortraitFind { photo: photo.to_path_buf(), subject, faces }
}

/// The skin seed the find step writes: `face::skin_seed` on the pixels the
/// skin mask's colour range is matched on — `base` developed with `recipe`'s
/// masks before the skin mask (all of them when there is none yet), geometry
/// stripped (`face::skin_seed_reference`, the eyedropper's convention).
pub(crate) fn panel_skin_seed(
    base: &image::DynamicImage,
    face_mask: &image::GrayImage,
    recipe: &EditRecipe,
    film: Option<u32>,
    raw_source: bool,
) -> Option<([f32; 3], [f32; 2])> {
    let before = recipe.masks.iter().position(|m| m.role == MaskRole::Skin).unwrap_or(recipe.masks.len());
    let reference = autoshade::face::skin_seed_reference(base, recipe, before, film, raw_source);
    autoshade::face::skin_seed(&reference, face_mask)
}

/// The first mask with `role`.
pub(crate) fn role_mask(r: &mut EditRecipe, role: MaskRole) -> Option<&mut LocalAdjustment> {
    r.masks.iter_mut().find(|m| m.role == role)
}

/// The mask the skin group writes: the skin mask, or — when only an AI
/// portrait look's face mask is there — the face mask.
pub(crate) fn skin_target(r: &mut EditRecipe) -> Option<&mut LocalAdjustment> {
    let i = r.masks.iter().position(|m| m.role == MaskRole::Skin).or_else(|| r.masks.iter().position(|m| m.role == MaskRole::Face))?;
    r.masks.get_mut(i)
}

/// The subject matte a "keep the person sharp" blur protects with: the
/// background mask's own raster (the subject, before its inversion).
pub(crate) fn subject_raster(r: &EditRecipe) -> Option<String> {
    r.masks.iter().filter(|m| m.role == MaskRole::Background).find_map(|m| match &m.mask {
        MaskGeometry::Bitmap { path } if !path.is_empty() => Some(path.clone()),
        MaskGeometry::AiMask { raster: Some(path), .. } => Some(path.clone()),
        _ => None,
    })
}

/// The raster of the first face mask — what 🤖 Remove blemishes holds the
/// heal to — or `None` when there is no face mask with one.
pub(crate) fn face_raster(r: &EditRecipe) -> Option<PathBuf> {
    r.masks
        .iter()
        .filter(|m| m.role == MaskRole::Face)
        .find_map(|m| autoshade::render::geometry_raster_path(&m.mask).filter(|p| !p.is_empty()).map(PathBuf::from))
}

/// An AI portrait look replaces the recipe whole: the masks the photo had
/// come back under it — with their sliders at rest, the look is the AI's —
/// wherever the look has no mask of that role, and the lens blur where the
/// look carries none. Done on the landed recipe BEFORE `on_analyzed` writes
/// it, so the saved file is the look the panel shows.
pub(crate) fn restore_portrait_masks(recipe: &mut EditRecipe, kept: Vec<LocalAdjustment>, blur: Option<autoshade::render::LensBlur>) {
    for m in kept {
        if recipe.masks.iter().any(|n| n.role == m.role) {
            continue;
        }
        recipe.masks.push(LocalAdjustment {
            mask: m.mask,
            range: m.range,
            inverted: m.inverted,
            name: m.name,
            role: m.role,
            ..Default::default()
        });
    }
    if recipe.lens_blur.is_none() {
        recipe.lens_blur = blur;
    }
}

/// The soften slider (0..=100) a skin mask shows, and what it writes.
pub(crate) fn soften_of(m: &LocalAdjustment) -> f32 {
    (-m.texture).max(0.0)
}
pub(crate) fn set_soften(m: &mut LocalAdjustment, s: f32) {
    m.texture = -s;
    m.clarity = -s * SOFTEN_CLARITY_SHARE;
}
/// The eyes' "Crisp" slider (0..=100): sharpness, and clarity by half.
pub(crate) fn crisp_of(m: &LocalAdjustment) -> f32 {
    m.sharpness.max(0.0)
}
pub(crate) fn set_crisp(m: &mut LocalAdjustment, c: f32) {
    m.sharpness = c;
    m.clarity = c * CRISP_CLARITY_SHARE;
}
/// The background's "Darken" slider (0..=100): exposure down, 100 = −2 EV.
pub(crate) fn darken_of(m: &LocalAdjustment) -> f32 {
    (-m.exposure_ev / DARKEN_EV_AT_FULL * 100.0).max(0.0)
}
pub(crate) fn set_darken(m: &mut LocalAdjustment, d: f32) {
    m.exposure_ev = -d / 100.0 * DARKEN_EV_AT_FULL;
}
/// The background's "Fade colour" slider (0..=100): saturation down.
pub(crate) fn fade_of(m: &LocalAdjustment) -> f32 {
    (-m.saturation).max(0.0)
}
pub(crate) fn set_fade(m: &mut LocalAdjustment, f: f32) {
    m.saturation = -f;
}

/// A slider's reach: the guardrail's limit while the switch is on, the
/// field's own travel when it is off.
fn reach(guarded: bool, limit: f32, free: f32) -> f32 {
    if guarded { limit } else { free }
}

impl AutoShadeApp {
    /// 🤖 Find people and faces: both sidecars in one worker.
    pub(crate) fn start_find_people(&mut self) {
        if self.busy {
            return;
        }
        let (Some(base), Some(photo)) = (self.base_preview.clone(), self.src_path.clone()) else { return };
        let recipe = self.recipe.clone();
        let film = self.film_short_edge();
        let raw_source = self.source_is_raw();
        self.busy = true;
        self.portrait.line = PortraitLine::Finding;
        self.status = tr(self.lang, "Finding the people and faces… (the first run downloads the models)").into();
        self.spawn_worker(
            move || Msg::PortraitFound(Box::new(find_people(&base, &photo, &recipe, film, raw_source))),
            |e| Msg::PortraitFound(Box::new(PortraitFind { photo: PathBuf::new(), subject: Err(e.to_string()), faces: Err(e.to_string()) })),
        );
    }

    /// `Msg::PortraitFound` landing: the four masks into the recipe — a mask
    /// that already has its role is re-pointed at the new raster and keeps
    /// its sliders — and the line says what was found.
    pub(crate) fn on_portrait_found(&mut self, lang: Lang, found: PortraitFind) {
        self.busy = false;
        if self.src_path.as_deref() != Some(found.photo.as_path()) && !found.photo.as_os_str().is_empty() {
            // Another photo is open now: its masks are not this one's.
            self.portrait.line = PortraitLine::Idle;
            return;
        }
        let mut fresh: Vec<LocalAdjustment> = Vec::new();
        let mut faces_n = 0;
        if let Ok(None) = &found.faces {
            // No face this time: the face, eyes and skin masks of an earlier
            // search stood on a face that is not found — left in place they
            // keep acting with their sliders while the groups say "no face".
            self.recipe.masks.retain(|m| !matches!(m.role, MaskRole::Face | MaskRole::Eyes | MaskRole::Skin));
        }
        if let Ok(Some(f)) = &found.faces {
            faces_n = f.n;
            let face = f.face.to_string_lossy().into_owned();
            fresh.push(autoshade::face::portrait_adjustment(MaskRole::Face, face.clone()));
            fresh.push(autoshade::face::portrait_adjustment(MaskRole::Eyes, f.eyes.to_string_lossy().into_owned()));
            if let Some((seed, at)) = f.seed {
                fresh.push(autoshade::face::skin_adjustment(face, seed, at));
            }
        }
        if let Ok(subject) = &found.subject {
            let subject = subject.to_string_lossy().into_owned();
            fresh.push(autoshade::face::background_adjustment(subject.clone()));
            // A blur that protects the person follows the new matte.
            if let Some(p) = self.recipe.lens_blur.as_mut().and_then(|lb| lb.protect.as_mut()) {
                *p = subject;
            }
        }
        for m in fresh {
            match role_mask(&mut self.recipe, m.role) {
                Some(old) => {
                    old.mask = m.mask;
                    if m.range.is_some() {
                        old.range = m.range;
                    }
                }
                None => self.recipe.masks.push(m),
            }
        }
        let failed = match (&found.subject, &found.faces) {
            (Err(a), Err(b)) => {
                let t = trf(lang, "Could not find the people: {err}", &[("err", &format!("{a}; {b}"))]);
                self.portrait.line = PortraitLine::Failed(t.clone());
                self.status = t.clone();
                self.toast(ToastKind::Error, t);
                return;
            }
            (Err(e), _) | (_, Err(e)) => Some(e.clone()),
            _ => None,
        };
        if found.faces.is_ok() {
            self.portrait.found = Some((found.photo.clone(), faces_n));
        }
        self.portrait.line = PortraitLine::Found { faces: faces_n, failed: failed.clone() };
        self.overlay_stale = true;
        self.dirty = true;
        self.status = self.portrait_line_text(lang);
        if let Some(why) = failed {
            self.toast(ToastKind::Error, trf(lang, "Part of the search did not work: {err}", &[("err", &why)]));
        }
    }

    /// ✨ AI portrait look: the portrait direction through the analysis door
    /// (`normal::one_click_request`'s preset arm — the same request a Normal
    /// preset card sends), a fresh look at the look strength.
    pub(crate) fn start_portrait_look(&mut self) {
        let Some(path) = self.src_path.clone() else { return };
        if self.busy || self.analyze_inflight {
            return;
        }
        let (req, guidance) = self.one_click_request(crate::panels::normal::OneClick::Preset("portrait"));
        let kept: Vec<LocalAdjustment> = self.recipe.masks.iter().filter(|m| PORTRAIT_ROLES.contains(&m.role)).cloned().collect();
        let blur = self.recipe.lens_blur.clone();
        self.busy = true;
        self.portrait.line = PortraitLine::Looking;
        self.status = tr(self.lang, "Making the portrait look…").into();
        let epoch = self.spawn_analyze(path, None, guidance, None, req, autoshade::library::DEFAULT_NAME);
        self.portrait.look = Some((epoch, kept, blur));
    }

    /// 🤖 Remove blemishes: the Retouch panel's AI heal, held to the face
    /// mask — only the spots whose centre is on the face are healed.
    pub(crate) fn start_remove_blemishes(&mut self) {
        if let Some(face) = face_raster(&self.recipe) {
            self.start_heal(false, Some(face));
        }
    }

    /// Before `on_analyzed` consumes a result: is it this panel's look?
    pub(crate) fn portrait_look_landing(&mut self, epoch: u64) -> Option<(Vec<LocalAdjustment>, Option<autoshade::render::LensBlur>)> {
        match self.portrait.look.take() {
            Some((e, kept, blur)) if e == epoch && epoch == self.gen_epoch => Some((kept, blur)),
            other => {
                self.portrait.look = other.filter(|(e, _, _)| *e != epoch);
                None
            }
        }
    }

    /// After the portrait look landed through `on_analyzed` (its masks and
    /// blur were put back before it was written — `restore_portrait_masks`).
    pub(crate) fn after_portrait_look(&mut self, landed: bool) {
        self.portrait.line = if landed { PortraitLine::Looked } else { PortraitLine::Failed(self.status.clone()) };
    }

    /// The frame's one place for "Keep skin natural": run after the panels
    /// drew and the workers landed, so a slider, a paste and an AI landing
    /// all meet it before the develop or the undo commit sees the recipe.
    /// Only while the switch is on for this card (`keep_skin_on`).
    pub(crate) fn guard_portrait_skin(&mut self) {
        if self.skin_held() && autoshade::face::keep_skin_natural_in(&mut self.recipe) {
            self.dirty = true;
        }
    }

    /// Does "Keep skin natural" hold the open card's writes: Portraits, and
    /// the switch on for this card.
    pub(crate) fn skin_held(&self) -> bool {
        self.mode.shell().portrait_panel && self.keep_skin_on()
    }

    /// Does it hold a write landing on `photo` (a paste): the open card's
    /// switch for the open photo, a stashed photo's card's own, and on for a
    /// photo not opened this session — the write replaces the edit opening
    /// would decide it on. Outside Portraits: never.
    pub(crate) fn skin_held_for(&self, photo: &std::path::Path) -> bool {
        if self.src_path.as_deref() == Some(photo) {
            return self.skin_held();
        }
        self.mode.shell().portrait_panel
            && self.nav_stash.get(photo).is_none_or(|st| {
                self.portrait.keep_skin.get(&(photo.to_path_buf(), st.id.clone())).copied().unwrap_or(true)
            })
    }

    /// Undo or redo put back an edit past the skin limits (made before the
    /// switch was on, or in another mode): it is put back as it was, so the
    /// switch goes off for the card — the next frame must not pull it back
    /// and record a new step over the redo history.
    pub(crate) fn release_skin_for_history(&mut self) {
        if self.skin_held()
            && autoshade::face::skin_values_beyond(&self.recipe) > 0
            && let Some(key) = self.skin_key()
        {
            self.portrait.keep_skin.insert(key, false);
        }
    }

    /// The open card's key for the per-card switch.
    fn skin_key(&self) -> Option<(PathBuf, String)> {
        let photo = self.src_path.clone()?;
        Some((photo, self.variants.get(self.active).map(|v| v.id.clone()).unwrap_or_default()))
    }

    /// Is "Keep skin natural" on for the open card? On unless the card was
    /// read from disk past the limits or the user turned it off. No photo:
    /// on.
    pub(crate) fn keep_skin_on(&self) -> bool {
        self.skin_key().and_then(|k| self.portrait.keep_skin.get(&k).copied()).unwrap_or(true)
    }

    /// The user's own flip of the switch. Turning it on is the explicit act
    /// that may bring values back within the limits, and the status line
    /// says how many it did.
    pub(crate) fn set_keep_skin(&mut self, on: bool) {
        let Some(key) = self.skin_key() else { return };
        self.portrait.keep_skin.insert(key, on);
        if !on {
            return;
        }
        let n = autoshade::face::skin_values_beyond(&self.recipe);
        if n > 0 && autoshade::face::keep_skin_natural_in(&mut self.recipe) {
            self.dirty = true;
            self.status = if n == 1 {
                tr(self.lang, "Skin kept natural: 1 value brought back within the limits").into()
            } else {
                trf(self.lang, "Skin kept natural: {n} values brought back within the limits", &[("n", &n.to_string())])
            };
        }
    }

    /// The photo was just read from disk (its strip settled): every card is
    /// decided afresh against the edit that was read — off where it already
    /// goes past the limits, on (no entry) elsewhere.
    pub(crate) fn decide_keep_skin_for_photo(&mut self) {
        let Some(photo) = self.src_path.clone() else { return };
        self.portrait.keep_skin.retain(|(p, _), _| *p != photo);
        self.switch_off_cards_past_limits();
    }

    /// Portraits was just entered from another mode: a card whose edit was
    /// pushed past the skin limits there is switched off, as opening decides
    /// it — entering a mode changes no value (design-mode-select.md §2). The
    /// other cards keep the switch they had.
    pub(crate) fn decide_keep_skin_on_entry(&mut self) {
        self.switch_off_cards_past_limits();
    }

    /// Every card of the open photo whose edit goes past the limits: off.
    fn switch_off_cards_past_limits(&mut self) {
        let Some(photo) = self.src_path.clone() else { return };
        for (i, v) in self.variants.iter().enumerate() {
            let recipe = if i == self.active { &self.recipe } else { &v.recipe };
            if autoshade::face::skin_values_beyond(recipe) > 0 {
                self.portrait.keep_skin.insert((photo.clone(), v.id.clone()), false);
            }
        }
    }

    /// A saved version was just read onto the open card: that card alone is
    /// decided afresh, the same way.
    pub(crate) fn decide_keep_skin_for_card(&mut self) {
        let Some(key) = self.skin_key() else { return };
        self.portrait.keep_skin.remove(&key);
        if autoshade::face::skin_values_beyond(&self.recipe) > 0 {
            self.portrait.keep_skin.insert(key, false);
        }
    }

    /// A card forked from the open one takes over its switch — a fork is an
    /// edit, not an opening.
    pub(crate) fn inherit_keep_skin(&mut self, from: &str, to: &str) {
        let Some(photo) = self.src_path.clone() else { return };
        if let Some(on) = self.portrait.keep_skin.get(&(photo.clone(), from.to_string())).copied() {
            self.portrait.keep_skin.insert((photo, to.to_string()), on);
        }
    }

    /// A depth estimate started from the Background group landed: aim the
    /// focus at the face (the skin colour range's sample point is the faces'
    /// centre) and keep the person sharp — instead of the canvas pick the
    /// Lens Blur section arms.
    pub(crate) fn after_portrait_depth(&mut self) {
        if !std::mem::take(&mut self.portrait.blur_from_panel) {
            return;
        }
        let centre = self.recipe.masks.iter().find(|m| m.role == MaskRole::Skin).and_then(|m| match m.range {
            Some(autoshade::recipe::RangeMask::Color { px, py, .. }) => Some([px, py]),
            _ => None,
        });
        let subject = subject_raster(&self.recipe);
        let Some(lb) = self.recipe.lens_blur.as_mut() else { return };
        if let Some(c) = centre {
            lb.focus = c;
            self.lens_focus_picking = false;
        }
        if lb.protect.is_none() {
            lb.protect = subject;
        }
        self.dirty = true;
    }

    /// "Keep the person sharp": the blur protects the subject matte (on), or
    /// nothing (off). `false` when there is no blur, or nothing to protect
    /// with.
    pub(crate) fn set_keep_person_sharp(&mut self, on: bool) -> bool {
        let subject = subject_raster(&self.recipe);
        let Some(lb) = self.recipe.lens_blur.as_mut() else { return false };
        let protect = if on { subject } else { None };
        if on && protect.is_none() {
            return false;
        }
        lb.protect = protect;
        lb.enabled = true;
        true
    }

    /// The first group's line, in the session language.
    pub(crate) fn portrait_line_text(&self, lang: Lang) -> String {
        match &self.portrait.line {
            PortraitLine::Idle => tr(lang, "Find the people first: the skin, eyes and background tools work on what is found.").into(),
            PortraitLine::Finding => tr(lang, "Finding the people and faces…").into(),
            PortraitLine::Found { faces: 0, .. } => tr(lang, "No face found in this photo").into(),
            PortraitLine::Found { faces: 1, .. } => tr(lang, "Found 1 face").into(),
            PortraitLine::Found { faces, .. } => trf(lang, "Found {n} faces", &[("n", &faces.to_string())]),
            PortraitLine::Failed(why) => why.clone(),
            PortraitLine::Looking if self.analyze_inflight => tr(lang, "Making the portrait look…").into(),
            PortraitLine::Looking => tr(lang, "Find the people first: the skin, eyes and background tools work on what is found.").into(),
            PortraitLine::Looked => tr(lang, "Done — Ctrl+Z goes back").into(),
        }
    }

    /// Why the skin and eyes groups are greyed, or `None` when they work.
    pub(crate) fn portrait_face_gate(&self, lang: Lang) -> Option<&'static str> {
        if self.recipe.masks.iter().any(|m| matches!(m.role, MaskRole::Face | MaskRole::Skin)) {
            return None;
        }
        let searched = self.portrait.found.as_ref().is_some_and(|(p, n)| self.src_path.as_ref() == Some(p) && *n == 0);
        Some(if searched { tr(lang, "No face found in this photo") } else { tr(lang, "Find the people and faces first (the first group).") })
    }

    /// The panel: a first-level heading and the four groups, behind the same
    /// mid-open gate as the shared panels.
    pub(crate) fn portrait_panel(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let editable = !self.open_in_flight;
        ui.add_space(SPACE_LG);
        panel_heading(ui, tr(lang, "Portraits"), false, |_| {});
        let mut changed = false;
        ui.add_enabled_ui(editable, |ui| {
            self.portrait_people(ui);
            changed |= self.portrait_skin(ui);
            changed |= self.portrait_eyes(ui);
            changed |= self.portrait_background(ui);
        });
        if changed {
            self.recipe.clamp();
            self.dirty = true;
        }
        ui.add_space(SPACE_MD);
        ui.separator();
    }

    /// A group's sub-heading.
    fn portrait_group(ui: &mut egui::Ui, title: &str) {
        ui.add_space(SPACE_MD);
        ui.label(egui::RichText::new(title).strong());
    }

    /// Group 1 — the people and the background, and the AI portrait look.
    fn portrait_people(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        Self::portrait_group(ui, tr(lang, "People and background"));
        let ready = self.src_path.is_some() && self.base_preview.is_some() && !self.busy && !self.analyze_inflight;
        let mut find = false;
        let mut look = false;
        ui.horizontal(|ui| {
            let cell = columns(ui, 2);
            find = action_in(ui, cell, ready, tr(lang, "🤖 Find people and faces"))
                .on_hover_text(tr(lang, "Finds the person and every face on this computer (MediaPipe finds the faces and eyes — install it with pip install mediapipe). The first run downloads the models."))
                .clicked();
            look = action_in(ui, cell, ready, tr(lang, "✨ AI portrait look"))
                .on_hover_text(tr(lang, "The AI looks at this photo and gives it a portrait look: softer skin, brighter eyes, a calmer background — Ctrl+Z goes back. Needs a service key in Settings (⚙)."))
                .clicked();
        });
        Self::slider_pct_hinted(
            ui,
            lang,
            tr(lang, "Look strength"),
            &mut self.grade_strength,
            1.0,
            GRADE_STRENGTH_DEFAULT,
            tr(lang, "How strong the look is. Double-click to reset."),
        );
        let line = self.portrait_line_text(lang);
        ui.horizontal_wrapped(|ui| {
            if self.portrait.line == PortraitLine::Finding || (self.portrait.line == PortraitLine::Looking && self.analyze_inflight) {
                ui.spinner();
            }
            ui.label(egui::RichText::new(line).small());
        });
        if find {
            self.start_find_people();
        }
        if look {
            self.start_portrait_look();
        }
    }

    /// Group 2 — the skin: soften, warmth, colour, the guardrail switch.
    fn portrait_skin(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;
        Self::portrait_group(ui, tr(lang, "Skin"));
        let gate = self.portrait_face_gate(lang);
        let guarded = self.keep_skin_on();
        let mut changed = false;
        ui.add_enabled_ui(gate.is_none(), |ui| {
            let mut probe = LocalAdjustment::default();
            let m = skin_target(&mut self.recipe).unwrap_or(&mut probe);
            let mut soften = soften_of(m);
            let soft_max = reach(guarded, -autoshade::face::SKIN_TEXTURE_MIN, 100.0);
            if Self::slider_hinted(ui, lang, tr(lang, "Soften skin"), &mut soften, 0.0, soft_max, 0.0, tr(lang, "Smooths the skin's fine texture; the face's shape and the eyes stay as they are.")) {
                set_soften(m, soften);
                changed = true;
            }
            let warm = reach(guarded, autoshade::face::SKIN_TEMPERATURE_MAX, 100.0);
            changed |= Self::slider_hinted(ui, lang, tr(lang, "Skin warmth"), &mut m.temperature, -warm, warm, 0.0, tr(lang, "Cooler to the left, warmer to the right — on the skin only."));
            let rich = reach(guarded, autoshade::face::SKIN_SATURATION_MAX, 100.0);
            changed |= Self::slider_hinted(ui, lang, tr(lang, "Skin colour intensity"), &mut m.saturation, -100.0, rich, 0.0, tr(lang, "Paler to the left, richer to the right — on the skin only."));
        });
        let mut on = guarded;
        let flipped = ui
            .checkbox(&mut on, tr(lang, "Keep skin natural"))
            .on_hover_text(tr(
                lang,
                "On: skin colour can be made only a little richer, warmer, cooler or shifted in tone, softening stops before the skin looks fake, and skin is never sharpened. This holds for your own changes and for the AI's.",
            ))
            .changed();
        if flipped {
            self.set_keep_skin(on);
        }
        let can_heal = face_raster(&self.recipe).is_some() && !self.busy && self.src_path.is_some();
        if action_row(ui, can_heal, tr(lang, "🤖 Remove blemishes"))
            .on_hover_text(tr(
                lang,
                "The AI finds small spots and blemishes on the face, and each is healed from the skin around it; only spots on the face are healed, and the heal of a spot at the face's edge reaches a little past it — Ctrl+Z goes back. Needs a service key in Settings (⚙).",
            ))
            .clicked()
        {
            self.start_remove_blemishes();
        }
        if let Some(why) = gate {
            ui.label(egui::RichText::new(why).weak().small());
        }
        changed
    }

    /// Group 3 — the eyes: brighten, crisp.
    fn portrait_eyes(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;
        Self::portrait_group(ui, tr(lang, "Eyes"));
        let gate = self.portrait_face_gate(lang).or_else(|| {
            (!self.recipe.masks.iter().any(|m| m.role == MaskRole::Eyes)).then(|| tr(lang, "Find the people and faces first (the first group)."))
        });
        let mut changed = false;
        ui.add_enabled_ui(gate.is_none(), |ui| {
            let mut probe = LocalAdjustment::default();
            let m = role_mask(&mut self.recipe, MaskRole::Eyes).unwrap_or(&mut probe);
            changed |= Self::slider_hinted(ui, lang, tr(lang, "Brighten eyes"), &mut m.exposure_ev, 0.0, EYES_BRIGHTEN_MAX_EV, 0.0, tr(lang, "Lifts the eyes a little."));
            let mut crisp = crisp_of(m);
            if Self::slider_hinted(ui, lang, tr(lang, "Crisp eyes"), &mut crisp, 0.0, 100.0, 0.0, tr(lang, "Brings out the detail in the eyes.")) {
                set_crisp(m, crisp);
                changed = true;
            }
        });
        if let Some(why) = gate {
            ui.label(egui::RichText::new(why).weak().small());
        }
        changed
    }

    /// Group 4 — the background: the blur (with the person kept sharp), and
    /// darken / fade on the background mask.
    fn portrait_background(&mut self, ui: &mut egui::Ui) -> bool {
        let lang = self.lang;
        Self::portrait_group(ui, tr(lang, "Background"));
        let mut changed = false;
        let subject = subject_raster(&self.recipe);
        if self.recipe.lens_blur.is_none() {
            let can = !self.busy && self.base_preview.is_some() && self.src_path.is_some() && depth_helper_available();
            if action_row(ui, can, tr(lang, "🤖 Prepare the background blur"))
                .on_hover_text(tr(lang, "Measures how far away every part of the photo is, on this computer (the first run downloads a 1.3 GB model); then the blur sliders appear."))
                .clicked()
            {
                self.portrait.blur_from_panel = true;
                self.start_depth();
            }
        } else if let Some(lb) = self.recipe.lens_blur.as_mut() {
            changed |= Self::slider(ui, lang, tr(lang, "Blur amount"), &mut lb.amount, 0.0, 100.0, 50.0);
            changed |= Self::slider_hinted(ui, lang, tr(lang, "Sharp range"), &mut lb.range, 0.0, 100.0, 10.0, tr(lang, "How deep the sharp part of the picture is, around the face."));
            let mut keep = lb.protect.is_some();
            let toggled = ui
                .add_enabled(subject.is_some() || keep, egui::Checkbox::new(&mut keep, tr(lang, "Keep the person sharp")))
                .on_hover_text(tr(lang, "The whole person stays sharp, however far the blur reaches."))
                .on_disabled_hover_text(tr(lang, "Find the people and faces first (the first group)."))
                .changed();
            if toggled {
                changed |= self.set_keep_person_sharp(keep);
            }
        }
        let has_bg = self.recipe.masks.iter().any(|m| m.role == MaskRole::Background);
        ui.add_enabled_ui(has_bg, |ui| {
            let mut probe = LocalAdjustment::default();
            let m = role_mask(&mut self.recipe, MaskRole::Background).unwrap_or(&mut probe);
            let mut darken = darken_of(m);
            if Self::slider_hinted(ui, lang, tr(lang, "Darken background"), &mut darken, 0.0, 100.0, 0.0, tr(lang, "Everything but the person, a little darker.")) {
                set_darken(m, darken);
                changed = true;
            }
            let mut fade = fade_of(m);
            if Self::slider_hinted(ui, lang, tr(lang, "Fade background colour"), &mut fade, 0.0, 100.0, 0.0, tr(lang, "Everything but the person, a little greyer.")) {
                set_fade(m, fade);
                changed = true;
            }
        });
        if !has_bg {
            ui.label(egui::RichText::new(tr(lang, "Find the people and faces first (the first group).")).weak().small());
        }
        changed
    }
}
