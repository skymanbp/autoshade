//! The reverse-fit fold's ONE strength control: five steps from the library's
//! table (`autoshade::fit::FitLevel`), each standing for the honesty budget
//! and the extra passes it turns on (user request 2026-11-10: the fold's
//! slider and four switches collapsed into one stepped control).
//!
//! Laid out like the panel's other sliders (`slider_impl`): the name on the
//! left and the step's name on the right of one line, the rail the column's
//! full width under them, so no step name can widen the side panel. Under the
//! rail one weak line says what the selected step does.

use crate::*;
use autoshade::fit::FitLevel;

/// The step's short name, numbered so the five read as one scale.
pub(crate) fn fit_level_name(lang: Lang, level: FitLevel) -> &'static str {
    match level {
        FitLevel::Light => tr(lang, "1 · Light"),
        FitLevel::Standard => tr(lang, "2 · Standard"),
        FitLevel::Strong => tr(lang, "3 · Strong"),
        FitLevel::Full => tr(lang, "4 · Full"),
        FitLevel::Reviewed => tr(lang, "5 · Full + AI review"),
    }
}

/// What the step does, in one line read off the table: the budget, the
/// local areas and their region count, and — for the paid step — its cost.
pub(crate) fn fit_level_summary(lang: Lang, level: FitLevel) -> String {
    let s = level.settings();
    let pct = format!("{:.0}", s.strength * 100.0);
    let mut line = if !s.zoned {
        trf(lang, "Whole-frame fit at {pct}%", &[("pct", &pct)])
    } else if s.regions > autoshade::fit_zoned::semantic::DEFAULT_SEMANTIC_REGIONS {
        trf(
            lang,
            "Whole-frame fit at {pct}% + local areas, up to {n} semantic regions",
            &[("pct", &pct), ("n", &s.regions.to_string())],
        )
    } else {
        trf(lang, "Whole-frame fit at {pct}% + local areas", &[("pct", &pct)])
    };
    if s.ai_judge {
        line.push_str(tr(lang, " + AI review, retry once · paid API: up to two vision calls"));
    }
    line
}

/// The hover text: what the steps share, then every step's own line.
fn fit_level_tooltip(lang: Lang) -> String {
    let mut tip = tr(lang,
        "How far the reverse-fit goes. Each step keeps everything the one before turns on. A higher strength lets the fit move sliders further on weak evidence; from 85% such moves are flagged in the notes instead of withheld.",
    )
    .to_string();
    for level in FitLevel::ALL {
        tip.push_str(&format!("\n{} — {}", fit_level_name(lang, level), fit_level_summary(lang, level)));
    }
    tip.push_str("\n\n");
    tip.push_str(tr(lang,
        "Local areas: after the whole-frame fit, extra masks are tried where the photo differs region by region (the sky, bright and dark ranges, tiles of a 4×4 grid); each is kept only if it brings the result closer without making any area worse. Sky masks go to Lightroom as its own Select Sky; masks Lightroom cannot hold are named when you save. Needs Python with transformers and torch; without them only the tonal ranges are tried. Four semantic regions cost one more OneFormer pass per frame.",
    ));
    tip.push_str("\n\n");
    tip.push_str(tr(lang,
        "AI review (step 5): before saving, the vision model scores how well the fitted render matches the target; if it suggests a fix, the fit tries it once and keeps whichever scores higher. Needs the image API key; the save waits for it and there is no cancel.",
    ));
    tip.push_str("\n\n");
    tip.push_str(tr(lang, "Double-click to reset to 2 · Standard."));
    tip
}

impl AutoShadeApp {
    /// The stepped 「Reverse-fit strength」. Not gated on `busy`: like the
    /// slider it replaces it is a preference, and `start_fit` reads it once
    /// when a fit starts.
    pub(crate) fn fit_level_control(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let (min, max) = (1.0, FitLevel::ALL.len() as f32);
        let mut step = f32::from(self.fit_level.step());
        // The right-hand name's width is budgeted at the WIDEST step name, so
        // the row is the same at every step (the runaway rule `slider_impl`
        // states for its number box).
        let name_w = {
            let style = egui::TextStyle::Body.resolve(ui.style());
            ui.fonts(|f| {
                FitLevel::ALL
                    .into_iter()
                    .map(|l| f.layout_no_wrap(fit_level_name(lang, l).to_owned(), style.clone(), egui::Color32::WHITE).size().x)
                    .fold(0.0f32, f32::max)
            })
        };
        let scope = ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.spacing_mut().interact_size.y = SLIDER_LINE_H;
            let full = ui.available_width();
            let label = ui
                .horizontal(|ui| {
                    let label_w = (full - name_w - ui.spacing().item_spacing.x).max(0.0);
                    let label = ui
                        .allocate_ui_with_layout(
                            egui::vec2(label_w, SLIDER_LINE_H),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(label_w);
                                ui.add(
                                    egui::Label::new(tr(lang, "Reverse-fit strength"))
                                        .truncate()
                                        .sense(egui::Sense::click()),
                                )
                            },
                        )
                        .inner;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(fit_level_name(lang, self.fit_level));
                    });
                    label
                })
                .inner;
            ui.spacing_mut().slider_width = full;
            let rail = ui.add(egui::Slider::new(&mut step, min..=max).step_by(1.0).show_value(false));
            paint_slider_knob(ui, &rail, step, min, max, false);
            (label, rail)
        });
        let (label, rail) = scope.inner;
        let tip = fit_level_tooltip(lang);
        let label = label.on_hover_text(&tip);
        let rail = rail.on_hover_text(&tip);
        if [&label, &rail].iter().any(|r| r.double_clicked() || r.secondary_clicked()) {
            self.fit_level = FitLevel::default();
        } else if rail.changed() {
            // `step_by(1.0)` keeps the value on a stop; the rounding is the
            // belt for a typed or dragged in-between value.
            self.fit_level = FitLevel::from_step(step.round() as u8).unwrap_or_default();
        }
        ui.label(egui::RichText::new(fit_level_summary(lang, self.fit_level)).weak().small());
    }
}
