//! The camera profile stage (v1.5.0, F7): Adobe's `.dcp` tables, rendered.
//!
//! # Where this sits
//!
//! Between the camera→working-space matrix and the working transfer encode, in
//! LINEAR light — the only place the profile's tables mean what they were
//! authored to mean. Everything downstream (exposure, tone, the mixer, the
//! masks) sees a frame that already carries the profile, which is the order
//! Lightroom's own panel implies: the profile establishes the rendering and the
//! sliders adjust from there.
//!
//! The stage is COLOUR DATA only: a camera profile is read from the user's
//! own Camera Raw install, like a lens profile, and it never changes how a
//! control behaves. Its tables go on in Adobe's order (RawTherapee's
//! `rtengine/dcp.cc`, `DCPProfile::apply`): reference-space matrix → HSV →
//! HueSatMap → RGB → HSV → LookTable → RGB. Its tone — `BaselineExposure`
//! and `ProfileToneCurve` — is not rendered: tone, exposure and curves are
//! the develop's own, the same for every recipe.
//!
//! # The colour matrix (v1.17.0)
//!
//! A profile that carries a `ForwardMatrix` OWNS the camera → working-space
//! transform of the render it is in, the way the DNG SDK builds it
//! (`dng_color_spec.cpp`): the frame's white is solved from its camera
//! neutral by iterating the profile's two `ColorMatrix`es interpolated at the
//! white's own temperature (`NeutralToXY`, `FindXYZtoCamera`; the temperature
//! by Robertson's method, `dng_temperature`), the two calibrations are
//! weighted by reciprocal temperature between their illuminants — the same
//! weight the hue/sat maps blend by, one function for both — and the
//! white-balanced camera triple goes to XYZ D50 through the interpolated,
//! normalised `ForwardMatrix`, into ProPhoto, and out through a Bradford
//! adaptation D50 → D65 into the working space. Until v1.17.0 the engine's
//! own D65-only, row-normalised camera matrix stood in for all of that; it
//! still does for a profile with no `ForwardMatrix` (the SDK's other branch,
//! inverse `ColorMatrix` with white preservation, is what that path already
//! computes at D65) and for every render with no profile at all, so those
//! renders are byte-identical to before.
//!
//! # The reference space
//!
//! Adobe authors these tables in ProPhoto RGB's primaries, so the stage
//! converts into them and back. The round trip is exact in the absence of a
//! table (the two matrices are inverses), which is what lets a profile with no
//! tables cost nothing but arithmetic.

use crate::dcp::{Profile, Table};

/// ProPhoto RGB (ROMM) primaries — the DNG specification's reference space, and
/// the space Adobe's tables are authored in.
///
/// Built against the WORKING space's white rather than ProPhoto's own D50.
/// That is a chromatic adaptation, written as one argument instead of a
/// Bradford matrix, and it is not optional: these tables are indexed by HSV, so
/// a SATURATION of zero has to mean the same colour on both sides of the
/// conversion. Pairing ProPhoto's D50 with the working space's D65 and calling
/// them one XYZ turns a neutral into a tinted triple on the way in — measured:
/// a grey through a table whose saturation scale is 0 came back
/// [0.560, 0.464, 0.343] instead of grey. Mapping white to white is what a
/// white-balanced frame means, and it is what makes the achromatic axis exact.
const PROPHOTO_PRIM: [[f32; 2]; 3] = [[0.7347, 0.2653], [0.1596, 0.8404], [0.0366, 0.0001]];

/// A camera profile, prepared for one photograph.
///
/// Built once per render and then asked per pixel, so everything that depends
/// only on the profile and the white balance — the illuminant blend and the
/// two matrices — is resolved here rather than in the loop.
pub(crate) struct Stage {
    /// Working space → ProPhoto linear, and back.
    to_ref: [[f32; 3]; 3],
    from_ref: [[f32; 3]; 3],
    /// White-balanced camera RGB → ProPhoto linear (D50), when the profile's
    /// `ForwardMatrix` owns the colour transform — and then `from_ref` is
    /// ProPhoto (D50) → Bradford → the working space.
    camera: Option<[[f32; 3]; 3]>,
    hue_sat: Option<Table>,
    /// The look tables, in the order they go on: the camera profile's own
    /// `ProfileLookTableData`, then the creative Look's ([`Stage::build_for`]).
    looks: Vec<Table>,
}

/// What one photograph brings to a profile besides the profile itself.
#[derive(Debug, Clone, Default)]
pub(crate) struct Frame {
    /// The frame's white balance as a temperature, for a profile whose
    /// calibrations cannot be solved from `neutral` (no `ColorMatrix`).
    pub kelvin: Option<f32>,
    /// The frame's camera-space neutral (the reciprocal of its white-balance
    /// multipliers). With the profile's `ColorMatrix`es it fixes the white and
    /// its temperature the way the DNG SDK does, and with a `ForwardMatrix` it
    /// lets the profile own the colour transform.
    pub neutral: Option<[f32; 3]>,
    /// The creative profile's own look table (`crs:LookTable`, decoded by
    /// [`crate::dcp::find_look_table`]). It goes on AFTER the camera
    /// profile's own `ProfileLookTableData`, which stays: see
    /// [`Stage::build_for`].
    pub creative: Option<Table>,
    /// The body's `CameraCalibration` diagonal, which the DNG SDK puts in
    /// front of every `ColorMatrix` (`XYZtoCamera = CC · CM`) when it turns
    /// a camera neutral into a white and a white into a neutral. Camera Raw
    /// writes it into the DNG of a Sony ARW (p39: diag(1.0031, 1, 0.9653),
    /// Adobe DNG Converter 18.7 — the file's Daylight preset against the
    /// body's reference, [`super::as_shot::camera_calibration`]). `None` is
    /// the identity.
    pub calibration: Option<[f32; 3]>,
}

impl Stage {
    /// Prepare `profile` for a frame in `space`, or `None` when the profile
    /// carries nothing this stage can act on.
    ///
    /// `kelvin` is the frame's own white balance, which selects between the
    /// profile's two calibrations. Absent — a file with no usable white balance
    /// — takes the FIRST hue/sat map, because a profile that ships only one
    /// ships it as Data1 and guessing a temperature to blend with would be
    /// inventing a measurement.
    ///
    /// The renders call [`Stage::build_for`]; this is its no-neutral,
    /// no-creative-table form, kept for the stage's own tests.
    #[cfg(test)]
    pub(crate) fn build(profile: &Profile, space: super::ExportColorSpace, kelvin: Option<f32>) -> Option<Self> {
        Self::build_for(profile, space, Frame { kelvin, ..Frame::default() })
    }

    /// [`Stage::build`] with everything the photograph brings ([`Frame`]).
    ///
    /// With a camera `neutral` and the profile's two `ColorMatrix`es, the
    /// white and its temperature are solved as the DNG SDK does
    /// ([`white_temperature_with`]) and that temperature — not `kelvin` — weights
    /// the calibrations. With a `ForwardMatrix` as well, the stage owns the
    /// camera → working transform ([`Stage::owns_camera`],
    /// [`Stage::develop_camera`]).
    ///
    /// A creative table goes on AFTER the profile's own look table, which
    /// stays: a creative Look names its base camera profile and carries its own
    /// table, and the base profile's maps are still in force under it.
    /// Measured against Camera Raw (v1.17.0) on a synthetic linear-ProPhoto
    /// bench whose profile carries 「Adobe Standard」's own 36 × 8 × 16 look
    /// table, with the 「Adobe Color」 Look (curves the identity), 4256 nodes,
    /// ΔE median / p95 / max: creative after the profile's 0.078 / 0.70 / 1.86;
    /// the profile's after the creative 0.13 / 2.94 / 21.8; the creative
    /// replacing the profile's 1.19 / 8.07 / 22.4. On p39's flat sidecar
    /// (Camera Raw 16-bit, 1536 px, warm h 20–90° C > 12 and sky-ish h 190–280°
    /// C > 8, Δb): replacing +2.51 and −0.89, stacking −0.19 and +0.02 (no
    /// Look −0.20 and +0.04). An earlier reading that preferred replacing was
    /// taken while the Look's curve went by the luma ratio, whose own error
    /// (Δb −1.1 on the same colours) offset the missing table.
    pub(crate) fn build_for(
        profile: &Profile,
        space: super::ExportColorSpace,
        frame: Frame,
    ) -> Option<Self> {
        let white = frame.neutral.and_then(|n| white_temperature_with(profile, n, frame.calibration));
        let kelvin = white.or(frame.kelvin);
        let hue_sat = blend_calibrations(profile, kelvin);
        let looks: Vec<Table> = profile.look_table.clone().into_iter().chain(frame.creative).collect();
        let camera = white.and_then(|_| forward_matrix(profile, kelvin));
        // Working RGB → XYZ (D65-adapted, as the export spaces are) → ProPhoto.
        let space_to_xyz = super::rgb_to_xyz(super::space_primaries(space), super::D65_XY);
        let ref_to_xyz = super::rgb_to_xyz(PROPHOTO_PRIM, super::D65_XY);
        let to_ref = super::mat_mul3(&super::inv3(&ref_to_xyz), &space_to_xyz);
        let (camera, from_ref) = match camera {
            // camera → XYZ D50 → ProPhoto (D50); out: ProPhoto → XYZ D50 →
            // Bradford → XYZ D65 → working. The SDK's `fCameraToRGB` and
            // `fRGBtoFinal`, with the working space where it has the output.
            Some(fm) => {
                let pp_to_xyz50 = mat3_f32(&rgb_to_xyz64(PROPHOTO_PRIM, D50_XY));
                let cam_to_pp = super::mat_mul3(&super::inv3(&pp_to_xyz50), &fm);
                let adapt = mat3_f32(&map_white(D50_XY, D65_XY64));
                let out = super::mat_mul3(
                    &super::inv3(&space_to_xyz),
                    &super::mat_mul3(&adapt, &pp_to_xyz50),
                );
                (Some(cam_to_pp), out)
            }
            None => (None, super::inv3(&to_ref)),
        };
        if camera.is_none() && hue_sat.is_none() && looks.is_empty() {
            return None;
        }
        Some(Stage { from_ref, to_ref, camera, hue_sat, looks })
    }

    /// Does this stage own the camera → working-space transform? Then the
    /// caller hands it WHITE-BALANCED CAMERA RGB through
    /// [`Stage::develop_camera`] instead of a working-space pixel through
    /// [`Stage::apply`].
    pub(crate) fn owns_camera(&self) -> bool {
        self.camera.is_some()
    }

    /// White-balanced camera RGB → the profiled, LINEAR working-space pixel.
    /// Only meaningful when [`Stage::owns_camera`]; otherwise the triple is
    /// taken as working RGB.
    pub(crate) fn develop_camera(&self, cam: [f32; 3]) -> [f32; 3] {
        match &self.camera {
            Some(m) => self.render_ref(super::mat_vec3(m, &cam)),
            None => {
                let mut px = cam;
                self.apply(&mut px);
                px
            }
        }
    }

    /// Apply the profile to one LINEAR working-space pixel, in place.
    pub(crate) fn apply(&self, px: &mut [f32; 3]) {
        *px = self.render_ref(super::mat_vec3(&self.to_ref, px));
    }

    /// The stage proper, on a ProPhoto-linear triple: the tables, then out to
    /// the working space.
    fn render_ref(&self, mut r: [f32; 3]) -> [f32; 3] {
        // HSV is undefined for a negative component, and ProPhoto's gamut is
        // wide enough that a real photograph almost never has one here. The
        // clamp is a guard, not a gamut decision: it keeps a stray negative out
        // of the hue arithmetic instead of producing a colour from a sign.
        for c in &mut r {
            *c = c.max(0.0);
        }
        if let Some(t) = &self.hue_sat {
            let mut hsv = to_hsv(r);
            t.apply(&mut hsv);
            r = from_hsv(hsv);
        }
        // The profile's own look table, then the creative Look's on top
        // ([`Stage::build_for`]).
        for t in &self.looks {
            let mut hsv = to_hsv(r);
            t.apply(&mut hsv);
            r = from_hsv(hsv);
        }
        super::mat_vec3(&self.from_ref, &r)
    }
}

// --- the colour spec: Adobe's camera → XYZ, from the profile's matrices ------

/// The PCS white (D50) and D65, as the DNG SDK states them
/// (`D50_xy_coord`, `D65_xy_coord`).
const D50_XY: [f64; 2] = [0.3457, 0.3585];
const D65_XY64: [f64; 2] = [0.3127, 0.3290];

fn xy_to_xyz(xy: [f64; 2]) -> [f64; 3] {
    [xy[0] / xy[1], 1.0, (1.0 - xy[0] - xy[1]) / xy[1]]
}

fn mat3_f64(m: &[[f32; 3]; 3]) -> [[f64; 3]; 3] {
    m.map(|r| r.map(f64::from))
}

fn mat3_f32(m: &[[f64; 3]; 3]) -> [[f32; 3]; 3] {
    m.map(|r| r.map(|v| v as f32))
}

fn mul64(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

fn vec64(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn inv64(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let c = |r1: usize, c1: usize, r2: usize, c2: usize| m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1];
    let co = [
        [c(1, 1, 2, 2), -c(1, 0, 2, 2), c(1, 0, 2, 1)],
        [-c(0, 1, 2, 2), c(0, 0, 2, 2), -c(0, 0, 2, 1)],
        [c(0, 1, 1, 2), -c(0, 0, 1, 2), c(0, 0, 1, 1)],
    ];
    let det = m[0][0] * co[0][0] + m[0][1] * co[0][1] + m[0][2] * co[0][2];
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    // inverse = adjugate / det, adjugate = transpose of the cofactors.
    Some([0, 1, 2].map(|i| [0, 1, 2].map(|j| co[j][i] / det)))
}

/// RGB → XYZ for primaries `prim` and white `white`, white at Y = 1 — the
/// render's own `rgb_to_xyz`, in f64 and with an f64 white.
fn rgb_to_xyz64(prim: [[f32; 2]; 3], white: [f64; 2]) -> [[f64; 3]; 3] {
    let cols = prim.map(|p| xy_to_xyz([f64::from(p[0]), f64::from(p[1])]));
    let m = [0, 1, 2].map(|i| [cols[0][i], cols[1][i], cols[2][i]]);
    let s = inv64(&m).map(|inv| vec64(&inv, xy_to_xyz(white))).unwrap_or([1.0; 3]);
    [0, 1, 2].map(|i| [0, 1, 2].map(|j| m[i][j] * s[j]))
}

/// The DNG SDK's `MapWhiteMatrix`: a linearised Bradford adaptation from
/// `from` to `to`, its per-cone scale pinned to [0.1, 10] as the SDK pins it.
fn map_white(from: [f64; 2], to: [f64; 2]) -> [[f64; 3]; 3] {
    const MB: [[f64; 3]; 3] =
        [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
    let (w1, w2) = (vec64(&MB, xy_to_xyz(from)), vec64(&MB, xy_to_xyz(to)));
    let mut a = [[0.0; 3]; 3];
    for i in 0..3 {
        let (p, q) = (w1[i].max(0.0), w2[i].max(0.0));
        a[i][i] = if p > 0.0 { (q / p).clamp(0.1, 10.0) } else { 10.0 };
    }
    let inv = inv64(&MB).unwrap_or(MB);
    mul64(&inv, &mul64(&a, &MB))
}

/// Robertson's isotherms, `dng_temperature`'s table (Wyszecki & Stiles,
/// *Color Science*, 2nd ed., p. 228: reciprocal megakelvin, u, v, isotherm
/// slope).
#[allow(clippy::excessive_precision)]
const ROBERTSON: [[f64; 4]; 31] = [
    [0.0, 0.18006, 0.26352, -0.24341],
    [10.0, 0.18066, 0.26589, -0.25479],
    [20.0, 0.18133, 0.26846, -0.26876],
    [30.0, 0.18208, 0.27119, -0.28539],
    [40.0, 0.18293, 0.27407, -0.30470],
    [50.0, 0.18388, 0.27709, -0.32675],
    [60.0, 0.18494, 0.28021, -0.35156],
    [70.0, 0.18611, 0.28342, -0.37915],
    [80.0, 0.18740, 0.28668, -0.40955],
    [90.0, 0.18880, 0.28997, -0.44278],
    [100.0, 0.19032, 0.29326, -0.47888],
    [125.0, 0.19462, 0.30141, -0.58204],
    [150.0, 0.19962, 0.30921, -0.70471],
    [175.0, 0.20525, 0.31647, -0.84901],
    [200.0, 0.21142, 0.32312, -1.0182],
    [225.0, 0.21807, 0.32909, -1.2168],
    [250.0, 0.22511, 0.33439, -1.4512],
    [275.0, 0.23247, 0.33904, -1.7298],
    [300.0, 0.24010, 0.34308, -2.0637],
    [325.0, 0.24702, 0.34655, -2.4681],
    [350.0, 0.25591, 0.34951, -2.9641],
    [375.0, 0.26400, 0.35200, -3.5814],
    [400.0, 0.27218, 0.35407, -4.3633],
    [425.0, 0.28039, 0.35577, -5.3762],
    [450.0, 0.28863, 0.35714, -6.7262],
    [475.0, 0.29685, 0.35823, -8.5955],
    [500.0, 0.30505, 0.35907, -11.324],
    [525.0, 0.31320, 0.35968, -15.628],
    [550.0, 0.32129, 0.36011, -23.325],
    [575.0, 0.32931, 0.36038, -40.770],
    [600.0, 0.33724, 0.36051, -116.45],
];

/// The chromaticity of a temperature and tint, `dng_temperature::Get_xy_coord`
/// — the white Camera Raw's Temperature and Tint name: the blackbody point at
/// the temperature, moved along its isotherm by the tint (`kTintScale`, −3000
/// tint units per unit of uv). The render reads whites the other way
/// ([`xy_to_temp_tint`]); this direction pins that inverse.
#[cfg(test)]
pub(crate) fn temp_tint_to_xy(kelvin: f64, tint: f64) -> [f64; 2] {
    let r = 1.0e6 / kelvin;
    let offset = tint / -3000.0;
    let unit = |s: f64| {
        let len = (1.0 + s * s).sqrt();
        (1.0 / len, s / len)
    };
    let mut i = 0;
    while i < ROBERTSON.len() - 2 && r >= ROBERTSON[i + 1][0] {
        i += 1;
    }
    let (a, b) = (ROBERTSON[i], ROBERTSON[i + 1]);
    let f = (b[0] - r) / (b[0] - a[0]);
    let (u, v) = (a[1] * f + b[1] * (1.0 - f), a[2] * f + b[2] * (1.0 - f));
    let ((u1, v1), (u2, v2)) = (unit(a[3]), unit(b[3]));
    let (du, dv) = (u1 * f + u2 * (1.0 - f), v1 * f + v2 * (1.0 - f));
    let len = (du * du + dv * dv).sqrt();
    let (u, v) = (u + du / len * offset, v + dv / len * offset);
    [1.5 * u / (u - 4.0 * v + 2.0), v / (u - 4.0 * v + 2.0)]
}

/// Correlated colour temperature of a chromaticity by Robertson's method —
/// `dng_temperature::Set_xy_coord`.
fn xy_to_kelvin(xy: [f64; 2]) -> f64 {
    xy_to_temp_tint(xy).0
}

/// The temperature and tint of a chromaticity, `dng_temperature::Set_xy_coord`
/// — [`temp_tint_to_xy`] the other way: the temperature by Robertson's
/// isotherms, the tint as the signed distance from the blackbody along the
/// interpolated isotherm (`kTintScale`, −3000 tint units per unit of uv).
/// The SDK's pair is inverse to within 0.03 mired and 0.002 tint units over
/// 2000–15000 K and ±100 tint, not to the bit (pinned by test).
pub(crate) fn xy_to_temp_tint(xy: [f64; 2]) -> (f64, f64) {
    const TABLE: [[f64; 4]; 31] = ROBERTSON;
    let den = 1.5 - xy[0] + 6.0 * xy[1];
    let (u, v) = (2.0 * xy[0] / den, 3.0 * xy[1] / den);
    let (mut last_dt, mut last_du, mut last_dv) = (0.0, 0.0, 0.0);
    for i in 1..TABLE.len() {
        let len = (1.0 + TABLE[i][3] * TABLE[i][3]).sqrt();
        let (du, dv) = (1.0 / len, TABLE[i][3] / len);
        let dt = -(u - TABLE[i][1]) * dv + (v - TABLE[i][2]) * du;
        if dt <= 0.0 || i == TABLE.len() - 1 {
            let dt = -dt.min(0.0);
            let f = if i == 1 { 0.0 } else { dt / (last_dt + dt) };
            let kelvin = 1.0e6 / (TABLE[i - 1][0] * f + TABLE[i][0] * (1.0 - f));
            let uu = u - (TABLE[i - 1][1] * f + TABLE[i][1] * (1.0 - f));
            let vv = v - (TABLE[i - 1][2] * f + TABLE[i][2] * (1.0 - f));
            let (du, dv) = (du * (1.0 - f) + last_du * f, dv * (1.0 - f) + last_dv * f);
            let len = (du * du + dv * dv).sqrt();
            return (kelvin, (uu * du + vv * dv) / len * -3000.0);
        }
        (last_dt, last_du, last_dv) = (dt, du, dv);
    }
    unreachable!("the loop returns on its last row")
}

/// The profile's XYZ → camera matrix at temperature `kelvin`: the two
/// `ColorMatrix`es weighted like the hue/sat maps ([`illuminant_weight`]),
/// or the one the profile has.
fn color_matrix_at(profile: &Profile, kelvin: f64) -> Option<[[f64; 3]; 3]> {
    match profile.color_matrix {
        [Some(a), Some(b)] => {
            let w = f64::from(illuminant_weight(profile, Some(kelvin as f32)));
            let (a, b) = (mat3_f64(&a), mat3_f64(&b));
            Some([0, 1, 2].map(|i| [0, 1, 2].map(|j| a[i][j] + (b[i][j] - a[i][j]) * w)))
        }
        [Some(a), None] | [None, Some(a)] => Some(mat3_f64(&a)),
        [None, None] => None,
    }
}

/// The temperature of the white this camera `neutral` stands for, solved as
/// the DNG SDK's `NeutralToXY` does: start at D50, map the neutral to XYZ
/// through the inverse of the `ColorMatrix` interpolated at the current
/// estimate, repeat until the chromaticity stops moving (30 passes at most;
/// a two-value oscillation is averaged, as the SDK averages it). `None` when
/// the profile has no `ColorMatrix` or the neutral is unusable.
#[cfg(test)]
pub(crate) fn white_temperature(profile: &Profile, neutral: [f32; 3]) -> Option<f32> {
    white_temperature_with(profile, neutral, None)
}

/// [`white_temperature`] with the body's `CameraCalibration` diagonal in
/// front of the matrices ([`Frame::calibration`]).
pub(crate) fn white_temperature_with(profile: &Profile, neutral: [f32; 3], cc: Option<[f32; 3]>) -> Option<f32> {
    if neutral.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return None;
    }
    let xy = neutral_to_xy(neutral.map(f64::from), |k| calibrated_matrix_at(profile, k, cc))?;
    let k = xy_to_kelvin(xy);
    k.is_finite().then_some(k as f32)
}

/// `CC · ColorMatrix(kelvin)`: the SDK's XYZ → camera with the body's
/// calibration diagonal on each camera row.
fn calibrated_matrix_at(profile: &Profile, kelvin: f64, cc: Option<[f32; 3]>) -> Option<[[f64; 3]; 3]> {
    let m = color_matrix_at(profile, kelvin)?;
    Some(match cc {
        Some(c) => [0, 1, 2].map(|i| m[i].map(|v| v * f64::from(c[i]))),
        None => m,
    })
}

/// The chromaticity of the white a camera `neutral` stands for, the DNG
/// SDK's `NeutralToXY`, with the XYZ → camera matrix at a temperature from
/// `matrix_at`: [`white_temperature`]'s iteration, and the as-shot reader's
/// (`camera_raw_as_shot`). `None` when there is no matrix or the neutral maps
/// to no colour.
pub(crate) fn neutral_to_xy(
    n: [f64; 3],
    matrix_at: impl Fn(f64) -> Option<[[f64; 3]; 3]>,
) -> Option<[f64; 2]> {
    let mut last = D50_XY;
    for pass in 0..30 {
        let cm = matrix_at(xy_to_kelvin(last))?;
        let xyz = vec64(&inv64(&cm)?, n);
        let s = xyz[0] + xyz[1] + xyz[2];
        if !s.is_finite() || s <= 0.0 {
            return None;
        }
        let mut next = [xyz[0] / s, xyz[1] / s];
        if (next[0] - last[0]).abs() + (next[1] - last[1]).abs() < 1e-7 {
            last = next;
            break;
        }
        if pass == 29 {
            next = [(last[0] + next[0]) * 0.5, (last[1] + next[1]) * 0.5];
        }
        last = next;
    }
    Some(last)
}

/// The `ForwardMatrix` for this temperature, normalised so camera white maps
/// to the D50 white (`NormalizeForwardMatrix`) and weighted like the other
/// calibrations; `None` when the profile has none — that profile keeps the
/// engine's own matrix path.
fn forward_matrix(profile: &Profile, kelvin: Option<f32>) -> Option<[[f32; 3]; 3]> {
    let norm = |m: [[f32; 3]; 3]| -> Option<[[f64; 3]; 3]> {
        let m = mat3_f64(&m);
        let pcs = xy_to_xyz(D50_XY);
        let mut o = m;
        for i in 0..3 {
            let s: f64 = m[i].iter().sum();
            if !s.is_finite() || s.abs() < 1e-9 {
                return None;
            }
            o[i] = m[i].map(|v| v * pcs[i] / s);
        }
        Some(o)
    };
    let m = match profile.forward_matrix {
        [Some(a), Some(b)] => {
            let (a, b) = (norm(a)?, norm(b)?);
            let w = f64::from(illuminant_weight(profile, kelvin));
            [0, 1, 2].map(|i| [0, 1, 2].map(|j| a[i][j] + (b[i][j] - a[i][j]) * w))
        }
        [Some(a), None] | [None, Some(a)] => norm(a)?,
        [None, None] => return None,
    };
    Some(mat3_f32(&m))
}

/// The hue/sat map for this frame's white balance.
///
/// A profile with two calibrations states them at two illuminants — Standard A
/// (2856 K) and D65 (6504 K) on every Sony profile measured — and Adobe blends
/// between them in RECIPROCAL temperature, which is the axis the two ends are
/// evenly spaced on. One calibration is used as itself.
fn blend_calibrations(profile: &Profile, kelvin: Option<f32>) -> Option<Table> {
    let (a, b) = (profile.hue_sat_map[0].as_ref(), profile.hue_sat_map[1].as_ref());
    match (a, b) {
        (Some(a), Some(b)) if a.hue == b.hue && a.sat == b.sat && a.val == b.val => {
            let w = illuminant_weight(profile, kelvin);
            if w <= 0.0 {
                return Some(a.clone());
            }
            if w >= 1.0 {
                return Some(b.clone());
            }
            let data = a
                .data
                .iter()
                .zip(&b.data)
                .map(|(x, y)| {
                    [
                        x[0] + (y[0] - x[0]) * w,
                        x[1] + (y[1] - x[1]) * w,
                        x[2] + (y[2] - x[2]) * w,
                    ]
                })
                .collect();
            Some(Table { data, ..a.clone() })
        }
        // Two tables of different shapes cannot be blended entry by entry, and
        // interpolating one onto the other's grid would be inventing samples.
        // Adobe ships none like that; if one appears, the first calibration is
        // used whole rather than a fabricated mixture.
        (Some(a), _) => Some(a.clone()),
        (None, b) => b.cloned(),
    }
}

/// Where this frame's white balance sits between the profile's two
/// calibrations: 0 = the first, 1 = the second.
fn illuminant_weight(profile: &Profile, kelvin: Option<f32>) -> f32 {
    let (Some(k), Some(t1), Some(t2)) = (
        kelvin.filter(|k| k.is_finite() && *k > 1000.0),
        profile.illuminant[0].and_then(illuminant_kelvin),
        profile.illuminant[1].and_then(illuminant_kelvin),
    ) else {
        return 0.0;
    };
    if (t1 - t2).abs() < 1.0 {
        return 0.0;
    }
    let (i, i1, i2) = (1.0 / k, 1.0 / t1, 1.0 / t2);
    ((i - i1) / (i2 - i1)).clamp(0.0, 1.0)
}

/// The EXIF light-source codes a camera profile actually uses, as correlated
/// colour temperatures.
///
/// Measured on the installed pool rather than transcribed whole: every Sony,
/// Canon, Apple and Leica profile read carries 17 (Standard A) and 21 (D65).
/// The daylight family is included because it costs nothing and a third-party
/// profile may state it; a code outside the list answers `None` and the frame
/// takes the first calibration rather than a guessed temperature.
pub(crate) fn illuminant_kelvin(code: u16) -> Option<f32> {
    Some(match code {
        1 | 4 | 9 => 5500.0,  // Daylight, Flash, Fine weather
        3 | 17 => 2856.0,     // Tungsten, Standard light A
        10 => 6000.0,         // Cloudy
        11 => 7500.0,         // Shade
        18 => 5500.0,         // Standard light B
        19 => 6500.0,         // Standard light C
        20 => 5503.0,         // D55
        21 => 6504.0,         // D65
        22 => 7504.0,         // D75
        23 => 5003.0,         // D50
        _ => return None,
    })
}

/// RGB → (hue degrees, saturation, value), the plain HSV the DNG tables index.
fn to_hsv(rgb: [f32; 3]) -> [f32; 3] {
    let (r, g, b) = (rgb[0], rgb[1], rgb[2]);
    let v = r.max(g).max(b);
    let m = r.min(g).min(b);
    let c = v - m;
    if c <= 0.0 || v <= 0.0 {
        return [0.0, 0.0, v.max(0.0)];
    }
    let h = if v == r {
        ((g - b) / c).rem_euclid(6.0)
    } else if v == g {
        (b - r) / c + 2.0
    } else {
        (r - g) / c + 4.0
    };
    [(h * 60.0).rem_euclid(360.0), c / v, v]
}

fn from_hsv(hsv: [f32; 3]) -> [f32; 3] {
    let (h, s, v) = (hsv[0].rem_euclid(360.0) / 60.0, hsv[1].clamp(0.0, 1.0), hsv[2].max(0.0));
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}


#[cfg(test)]
mod tests;
