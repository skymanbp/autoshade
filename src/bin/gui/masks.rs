//! Raster mask lifecycle: brush, bake, refine, PNG export.

use super::*;

impl AutoshopApp {
    /// Run the AI segmentation sidecar on the ORIGINAL-frame preview and attach
    /// the resulting raster as a Bitmap local mask (gap batch A②). The AI only
    /// picks WHERE — every actual edit stays a deterministic recipe slider.
    pub(crate) fn start_segment(&mut self, target: &'static str, label: &'static str) {
        if self.busy {
            return;
        }
        let lang = self.lang;
        // The STATUS shows a localised name; the mask's persisted `name` stays
        // the stable English label — recipe.json / XMP are data and must
        // survive a language switch (a translated name baked into the sidecar
        // can never be matched or re-keyed again).
        let disp = tr(lang, label).to_string();
        let Some(base) = self.base_preview.clone() else { return };
        let Some(src) = self.src_path.clone() else { return };
        self.busy = true;
        self.status = trf(
            lang,
            "AI segmenting {what}… (first run auto-downloads the model; failures are reported here)",
            &[("what", &disp)],
        );
        self.spawn_worker(
            move || {
                let res = (|| -> anyhow::Result<(String, PathBuf)> {
                    let cfg = autoshop::config::Config::load();
                    let opts = autoshop::segment::SegmentOpts::from_config(&cfg, target);
                    // The sidecar sees the ORIGINAL-frame preview — the space recipe
                    // masks live in. Preview resolution is enough: the engine samples
                    // the raster bilinearly in normalised coords at any render size.
                    let mut tmp = std::env::temp_dir();
                    tmp.push(format!("autoshop_seg_{}_{target}.png", std::process::id()));
                    base.to_rgb8()
                        .save(&tmp)
                        .map_err(|e| anyhow::anyhow!("write segmentation input {}: {e}", tmp.display()))?;
                    // A FRESH claimed name per run (mask-sky.png, -2, …):
                    // rewriting one fixed file replaced bytes a SAVED recipe
                    // still referenced before any save — the same corruption
                    // class the zoned-fit raster had. The Segmented handler
                    // re-points the existing mask entry by name FAMILY, so a
                    // rerun still refreshes instead of stacking.
                    let mask = autoshop::store::claim_raster(&src, &format!("mask-{target}"))?;
                    let run = autoshop::segment::segment_file(&opts, &tmp, &mask);
                    let _ = std::fs::remove_file(&tmp);
                    if let Err(e) = run {
                        // Release the claimed name: a failed run leaves its
                        // create_new 0-byte slot (segment.py publishes
                        // atomically, so an error means nothing real landed).
                        let _ = std::fs::remove_file(&mask);
                        return Err(e);
                    }
                    // English label into the recipe (stable data), not `disp`.
                    Ok((label.to_string(), mask))
                })();
                Msg::Segmented(res)
            },
            |e| Msg::Segmented(Err(e)),
        );
    }

    /// PNG bytes of the EXPORT mask: painted → transparent (regenerate / heal
    /// here), unpainted → opaque. None if nothing is painted — mirrors the web.
    pub(crate) fn export_mask_png(&self) -> Option<Vec<u8>> {
        let m = self.mask_paint.as_ref()?;
        let (w, h) = (m.width(), m.height());
        let mut out = image::RgbaImage::new(w, h);
        let mut any = false;
        for (x, y, p) in m.enumerate_pixels() {
            let painted = p.0[3] > 10;
            any |= painted;
            out.put_pixel(x, y, image::Rgba([0, 0, 0, if painted { 0 } else { 255 }]));
        }
        if !any {
            return None;
        }
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgba8(out)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .ok()?;
        Some(buf)
    }

    pub(crate) fn clear_mask(&mut self) {
        if let Some(m) = &mut self.mask_paint {
            for p in m.pixels_mut() {
                *p = image::Rgba([0, 0, 0, 0]);
            }
            self.mask_dirty = true;
            // The WHOLE canvas changed: a pending brush sub-rect from an
            // uncommitted stroke would make ensure_mask_tex's partial-upload
            // fast path re-upload only that rectangle, leaving previously
            // painted pixels alive in the GPU texture indefinitely.
            self.mask_dirty_rect = None;
        }
        // The display canvas and the greyscale weight buffer are a
        // DUAL-WRITE pair (the brush writes both; Apply bakes the GRAY) —
        // clearing only the display broke the session's own "bakes exactly
        // what it shows" contract: Apply after Clear still committed the
        // cleared strokes (L15-8). end_mask_brush clears the pair too.
        if let Some(g) = &mut self.mask_brush_gray {
            for p in g.pixels_mut() {
                *p = image::Luma([0]);
            }
        }
        self.paint_last = None;
    }

    /// Arm a mask-brush session: `target = None` paints a NEW Bitmap mask on
    /// Apply; `Some(i)` edits mask `i`'s raster — seeded into the canvas (as
    /// the red display wash) and the greyscale weight buffer, at canvas
    /// resolution, so the session bakes exactly what it shows.
    pub(crate) fn start_mask_brush(&mut self, target: Option<usize>) {
        self.disarm_tools();
        let Some(base) = self.base_preview.as_ref() else { return };
        let (mw, mh) = base.dimensions();
        let mut gray = image::GrayImage::new(mw, mh);
        let mut canvas = image::RgbaImage::new(mw, mh);
        if let Some(i) = target
            && let Some(MaskGeometry::Bitmap { path }) = self.recipe.masks.get(i).map(|m| &m.mask)
        {
            match autoshop::render::open_mask_bounded(std::path::Path::new(path)) {
                Ok(img) => {
                    let g = image::imageops::resize(
                        &img.to_luma8(),
                        mw,
                        mh,
                        image::imageops::FilterType::Triangle,
                    );
                    for (x, y, p) in g.enumerate_pixels() {
                        if p[0] > 0 {
                            let a = (p[0] as u16 * 160 / 255) as u8;
                            canvas.put_pixel(x, y, image::Rgba([255, 64, 64, a]));
                        }
                    }
                    gray = g;
                }
                Err(e) => {
                    let t = trf(
                        self.lang,
                        "could not load this mask's raster ({err}) — starting from an empty brush canvas",
                        &[("err", &e.to_string())],
                    );
                    self.toast(ToastKind::Error, t);
                }
            }
        }
        self.mask_paint = Some(canvas);
        self.mask_dirty = true;
        self.mask_dirty_rect = None;
        self.mask_brush_gray = Some(gray);
        self.mask_brush = Some((target, false));
        self.paint_mode = true;
        self.status = tr(
            self.lang,
            "Brush mask — paint to select; 「Erase」 removes; 「Apply」 bakes it · Esc cancels",
        )
        .into();
    }

    /// Bake the brush session: the greyscale weight buffer becomes a freshly
    /// CLAIMED raster (the input file is never mutated — saved recipes and
    /// version snapshots keep rendering what they rendered), then the target
    /// mask repoints at it, or a new Bitmap mask is pushed.
    pub(crate) fn commit_mask_brush(&mut self) {
        let Some((target, _)) = self.mask_brush else { return };
        let Some(gray) = self.mask_brush_gray.clone() else { return };
        let Some(src) = self.src_path.clone() else { return };
        let lang = self.lang;
        if target.is_none() {
            if gray.pixels().all(|p| p[0] == 0) {
                let t = tr(lang, "nothing painted yet — drag on the image first");
                self.toast(ToastKind::Error, t.to_string());
                return;
            }
            if self.recipe.masks.len() >= 64 {
                let t = tr(lang, "mask limit reached (64) — delete one first");
                self.toast(ToastKind::Error, t.to_string());
                return;
            }
        }
        let written = autoshop::store::claim_raster(&src, "mask-brush")
            .map_err(anyhow::Error::from)
            .and_then(|p| {
                gray.save(&p)?;
                // Durable BEFORE the recipe references it (L03): the JSON
                // commits through fsync, and a raster still in the page
                // cache leaves a saved develop pointing at pixels a power
                // cut never wrote.
                autoshop::store::durable_adopt(&p)?;
                Ok(p)
            });
        match written {
            Ok(path) => {
                let path_s = path.to_string_lossy().into_owned();
                match target {
                    Some(i) if i < self.recipe.masks.len() => {
                        self.recipe.masks[i].mask = MaskGeometry::Bitmap { path: path_s };
                        self.sel_mask = Some(i);
                        self.status = tr(lang, "mask raster updated — its adjustments now apply to the edited area").into();
                    }
                    _ => {
                        let n = self.recipe.masks.len();
                        self.recipe.masks.push(autoshop::recipe::LocalAdjustment {
                            mask: MaskGeometry::Bitmap { path: path_s },
                            name: trf(lang, "Brush {n}", &[("n", &(n + 1).to_string())]),
                            ..Default::default()
                        });
                        self.sel_mask = Some(n);
                        self.status = tr(lang, "brush mask created — pull its sliders in 「Local Masks」 (all 0 now, no visible effect yet)").into();
                    }
                }
                self.sel_component = None;
                self.dirty = true;
                self.overlay_stale = true;
                self.disarm_tools(); // ends the session and clears the canvas
            }
            Err(e) => {
                let t = trf(
                    lang,
                    "could not save the brush mask ({err})",
                    &[("err", &e.to_string())],
                );
                self.toast(ToastKind::Error, t);
            }
        }
    }

    /// Bake one raster op (feather / expand / contract) on mask `i`'s bitmap:
    /// load → transform → claim a fresh raster → repoint (immutability rule —
    /// see commit_mask_brush). Synchronous at the raster's own resolution.
    pub(crate) fn bake_mask_raster(
        &mut self,
        i: usize,
        op: impl FnOnce(&image::GrayImage) -> image::GrayImage,
        tag: &str,
    ) {
        let Some(src) = self.src_path.clone() else { return };
        let Some(MaskGeometry::Bitmap { path }) =
            self.recipe.masks.get(i).map(|m| m.mask.clone())
        else {
            return;
        };
        let lang = self.lang;
        let done = autoshop::render::open_mask_bounded(std::path::Path::new(&path))
            .map(|img| op(&img.to_luma8()))
            .and_then(|g| {
                let p = autoshop::store::claim_raster(&src, tag).map_err(anyhow::Error::from)?;
                g.save(&p).map_err(anyhow::Error::from)?;
                // Same L03 rule as the brush writer: payload durable first.
                autoshop::store::durable_adopt(&p)?;
                Ok(p)
            });
        match done {
            Ok(p) => {
                self.recipe.masks[i].mask =
                    MaskGeometry::Bitmap { path: p.to_string_lossy().into_owned() };
                self.dirty = true;
                self.overlay_stale = true;
                self.status =
                    tr(lang, "mask raster updated — its adjustments now apply to the edited area")
                        .into();
            }
            Err(e) => {
                let t = trf(
                    lang,
                    "could not edit this mask's raster ({err})",
                    &[("err", &e.to_string())],
                );
                self.toast(ToastKind::Error, t);
            }
        }
    }

    /// Refine mask `i`'s raster to FULL resolution: decode the full-res
    /// source in a worker, guided-filter the raster against it
    /// (render::refine_mask_guided), save under a fresh claim. The result is
    /// matched back by the stored raster reference — the strip may have
    /// changed while the worker ran.
    pub(crate) fn start_mask_refine(&mut self, i: usize) {
        if self.busy {
            return;
        }
        let Some(src) = self.src_path.clone() else { return };
        let Some(MaskGeometry::Bitmap { path }) =
            self.recipe.masks.get(i).map(|m| m.mask.clone())
        else {
            return;
        };
        let lang = self.lang;
        self.busy = true;
        self.status = tr(lang, "Refining the mask to full resolution (decoding the full-size source) …").into();
        let stored_ref = path.clone();
        self.spawn_worker(
            move || {
                let res = (|| {
                    let mask = autoshop::render::open_mask_bounded(std::path::Path::new(&path))
                        .map_err(|e| anyhow::anyhow!("load mask raster {path}: {e}"))?
                        .to_luma8();
                    let full = autoshop::decode::load_image(&src)?;
                    let long = full.width().max(full.height());
                    let refined = autoshop::render::refine_mask_guided(
                        &mask,
                        &full,
                        (long / 500).max(4) as usize,
                        1e-4,
                    );
                    let out = autoshop::store::claim_raster(&src, "mask-refined")?;
                    refined.save(&out)?;
                    // Same L03 rule as the brush writer: durable first.
                    autoshop::store::durable_adopt(&out)?;
                    Ok((i, stored_ref, out))
                })();
                Msg::MaskRefined(res)
            },
            |e| Msg::MaskRefined(Err(e)),
        );
    }
}
