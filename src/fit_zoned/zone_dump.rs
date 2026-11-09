//! The zone dump (2026-11-08): the populations the dilution guard reads in
//! [`super::one_zone`], exported as f32 LE `.bin` arrays + `meta.json` for the
//! NumPy prototype of the reliability-weighted ruler (RULING-1180-joint-r2,
//! addendum 2026-11-08 22:30). Off unless `AUTOSHADE_ZONE_DUMP` names a
//! directory; one sub-directory per Full zone whose tone the guard examined,
//! numbered in attachment order within the process.

use crate::fit::PairedRobustTone;
use std::sync::atomic::{AtomicUsize, Ordering};

static ZONE_DUMP_SEQ: AtomicUsize = AtomicUsize::new(0);

/// What the guard read, borrowed at the guard.
pub(super) struct ZoneDump<'a> {
    pub label: &'a str,
    /// The zone's structural divergence D, if measured.
    pub d: Option<f32>,
    pub width: u32,
    pub height: u32,
    /// The recoloured render the paired map was re-estimated on (`rp`).
    pub source: &'a [[f32; 3]],
    /// The target the zone pairs against (corresponded where a field exists).
    pub target: &'a [[f32; 3]],
    /// The mask alphas (the populations' presence).
    pub sw: &'a [f32],
    pub tw: &'a [f32],
    /// The robust weights (presence × the map's own verdict per pixel).
    pub zw_source: &'a [f32],
    pub zw_target: &'a [f32],
    /// The paired robust map, where the zone's pixels pair.
    pub refit: Option<&'a PairedRobustTone>,
    /// `(mapped, target)` p10–p90 spreads the guard compared, when it could.
    pub spreads: Option<(f32, f32)>,
    /// Whether the guard discarded the paired map.
    pub fired: bool,
}

pub(super) fn write(dir: &std::path::Path, z: &ZoneDump<'_>) {
    let n = ZONE_DUMP_SEQ.fetch_add(1, Ordering::SeqCst);
    let slug: String = z.label.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let out = dir.join(format!("{n:02}-{slug}"));
    std::fs::create_dir_all(&out).expect("create the zone dump directory");
    let write = |name: &str, values: &[f32]| {
        let mut bytes = Vec::with_capacity(values.len() * 4);
        for v in values {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(out.join(name), bytes).unwrap_or_else(|e| panic!("write {name}: {e}"));
    };
    let flat = |px: &[[f32; 3]]| px.iter().flatten().copied().collect::<Vec<f32>>();
    write("source.bin", &flat(z.source));
    write("target.bin", &flat(z.target));
    write("sw.bin", z.sw);
    write("tw.bin", z.tw);
    write("zw_source.bin", z.zw_source);
    write("zw_target.bin", z.zw_target);
    if let Some(r) = z.refit {
        write("points.bin", &r.points.iter().flat_map(|&(x, y)| [x, y]).collect::<Vec<f32>>());
        write("masses.bin", &r.masses);
        write("robust_weights.bin", &r.weights);
    }
    let meta = serde_json::json!({
        "label": z.label,
        "d": z.d,
        "width": z.width,
        "height": z.height,
        "points": z.refit.map_or(0, |r| r.points.len()),
        "rejected_share": z.refit.map(|r| r.rejected_share),
        "spread_mapped": z.spreads.map(|s| s.0),
        "spread_target": z.spreads.map(|s| s.1),
        "floor": super::limits::ZONE_PAIRED_SPREAD_FLOOR,
        "fired": z.fired,
    });
    std::fs::write(out.join("meta.json"), serde_json::to_vec_pretty(&meta).expect("meta json")).expect("write meta");
}
