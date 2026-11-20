// One part of the engine's tests (src/render/tests.rs includes it): the colour field's read and its guide, each run on all cores, pinned bit for bit against the one-pixel-after-another bodies they were before (2026-11-20).

/// The guide as it was read before its planes ran side by side: the body of
/// `field_guide_luma` as of v1.18.4, copied verbatim.
fn serial_field_guide_luma(px: &[[f32; 3]], width: usize, height: usize) -> Vec<f32> {
    let luma: Vec<f32> = px.iter().map(luma601).collect();
    let mut horizontal = vec![0.0f32; luma.len()];
    for (i, slot) in horizontal.iter_mut().enumerate() {
        let (row, x) = (i - i % width, i % width);
        let (left, right) = (row + x.saturating_sub(1), row + (x + 1).min(width - 1));
        *slot = (luma[left] + luma[i] + luma[right]) / 3.0;
    }
    let mut out = vec![0.0f32; luma.len()];
    for (i, slot) in out.iter_mut().enumerate() {
        let (y, x) = (i / width, i % width);
        let (up, down) = (y.saturating_sub(1) * width + x, (y + 1).min(height - 1) * width + x);
        *slot = (horizontal[up] + horizontal[i] + horizontal[down]) / 3.0;
    }
    out
}

/// The field's read as it was before its rows ran side by side: the body of
/// `apply_colour_field_in` as of v1.18.4, copied verbatim but for two lines —
/// the axes taken through `FieldFrame::axes` (which builds the same
/// `FieldAxes::new(w, h, x, y, corrected)`), and the partition handed in as a
/// slice in the closure's place — and its guide the serial one above.
fn serial_colour_field(
    data: &mut [[f32; 3]],
    w: usize,
    h: usize,
    field: Option<&crate::recipe::ColourField>,
    frame: &FieldFrame,
    sky: Option<&[f32]>,
) {
    let Some(field) = field.filter(|f| f.renderable()) else { return };
    if w == 0 || h == 0 || data.len() != w * h {
        return;
    }
    let guide = serial_field_guide_luma(data, w, h);
    let sky = if field.owners.is_empty() { None } else { sky };
    let sky = sky.filter(|s| s.len() == data.len());
    let axes = frame.axes(w, h, field.x, field.y);
    let amount = field.amount.clamp(0.0, 1.0);
    for (i, c) in data.iter_mut().enumerate() {
        let (gx, gy) = axes.at(i);
        let coords = [gx, gy, guide[i].clamp(0.0, 1.0) * (field.b - 1) as f32];
        let limits = [field.x, field.y, field.b];
        let (mut low, mut high, mut frac) = ([0usize; 3], [0usize; 3], [0.0f32; 3]);
        for (axis, &limit) in limits.iter().enumerate() {
            let floor = (coords[axis].floor() as i64).clamp(0, limit as i64 - 1) as usize;
            (low[axis], high[axis]) = (floor, (floor + 1).min(limit - 1));
            frac[axis] = coords[axis] - floor as f32;
        }
        let (mut ids, mut weights) = ([0usize; 8], [0.0f32; 8]);
        for slot in 0..8 {
            let up = [slot >> 2 & 1, slot >> 1 & 1, slot & 1];
            let at = |a: usize| if up[a] == 1 { high[a] } else { low[a] };
            let mass = |a: usize| if up[a] == 1 { frac[a] } else { 1.0 - frac[a] };
            ids[slot] = (at(1) * field.x + at(0)) * field.b + at(2);
            weights[slot] = mass(0) * mass(1) * mass(2);
        }
        if let Some(sky) = sky {
            field_owned_weights(&mut weights, &ids.map(|id| field.owners[id]), sky[i]);
        }
        let mut p = [0.0f64; 5];
        for (&id, &weight) in ids.iter().zip(&weights) {
            let vertex = &field.grid[id];
            for (q, slot) in p.iter_mut().enumerate() {
                *slot += weight as f64 * vertex[q] as f64;
            }
        }
        let g = guide[i];
        let before = *c;
        for ch in 0..3 {
            let d = std::f64::consts::LN_2 * c[ch] as f64 * p[0]
                + c[ch] as f64 * p[1 + ch]
                + (c[ch] - g) as f64 * p[4];
            c[ch] = (c[ch] + amount * d as f32).clamp(0.0, 1.0);
        }
        // A pixel blown in every channel takes its brightness from the field
        // and no hue (`keep_blown_white`); for that reason the solve asks
        // nothing of such a pixel's channels one by one
        // (`fit_field::testifying`, on the same predicate, `blown_white`).
        *c = keep_blown_white(&before, *c);
    }
}

/// A frame with something in every corner of the field's read: a hashed
/// texture over a gradient, a few pixels blown in every channel, a few at 0.
fn rows_frame(w: usize, h: usize, seed: u32) -> Vec<[f32; 3]> {
    (0..w * h)
        .map(|i| {
            let hash = |k: u32| ((i as u32).wrapping_mul(2_654_435_761) ^ seed.wrapping_mul(40_503) ^ k.wrapping_mul(97_531)).wrapping_mul(2_246_822_519) >> 8;
            let unit = |k: u32| (hash(k) & 0xffff) as f32 / 65535.0;
            match hash(9) % 41 {
                0 => [1.0; 3],
                1 => [0.0; 3],
                _ => {
                    let (x, y) = ((i % w) as f32 / w.max(2) as f32, (i / w) as f32 / h.max(2) as f32);
                    [0.15 + 0.6 * x + 0.2 * unit(0), 0.1 + 0.5 * y + 0.3 * unit(1), 0.05 + 0.4 * (x * y) + 0.5 * unit(2)]
                }
            }
        })
        .collect()
}

/// A field `x` by `y` by `b` vertices with every parameter moving, and
/// owners — sky and land in a checker, some vertices owned by neither — where
/// `owned`.
fn rows_field(x: usize, y: usize, b: usize, owned: bool) -> crate::recipe::ColourField {
    use crate::recipe::ColourField;
    let n = x * y * b;
    let grid = (0..n).map(|v| std::array::from_fn(|q| 0.04 * ((v * 7 + q * 3) % 11) as f32 - 0.2)).collect();
    let owners = if owned { (0..n).map(|v| [ColourField::OWNER_SKY, ColourField::OWNER_LAND, 0][v % 3]).collect() } else { Vec::new() };
    ColourField { x, y, b, grid, owners, amount: 0.85, enabled: true }
}

fn bits(px: &[[f32; 3]]) -> Vec<u32> {
    px.iter().flatten().map(|v| v.to_bits()).collect()
}

/// The guide read side by side is the guide read one pixel after another, to
/// the bit, on frames of every shape: odd, one pixel, one row, one column,
/// and wider than 1024.
///
/// MUTATION THIS KILLS: a tap of the parallel guide taken from the wrong
/// neighbour (the row's start in `left`'s place).
#[test]
fn the_field_guide_read_side_by_side_is_the_serial_guide_bit_for_bit() {
    for (w, h) in [(37usize, 23usize), (1, 1), (1, 29), (31, 1), (1031, 7), (2, 2)] {
        let px = rows_frame(w, h, (w * 31 + h) as u32);
        let guide = field_guide_luma(&px, w, h);
        let serial = serial_field_guide_luma(&px, w, h);
        assert_eq!(guide.len(), w * h);
        assert!(guide.iter().map(|v| v.to_bits()).eq(serial.iter().map(|v| v.to_bits())), "{w}x{h}: the guide is the serial guide");
    }
    assert!(field_guide_luma(&[], 0, 0).is_empty() && serial_field_guide_luma(&[], 0, 0).is_empty());
}

/// The field read side by side is the field read one pixel after another, to
/// the bit: on frames of every shape, with and without owners, with and
/// without the sky plane, on a frame read at its own place and on a
/// lens-corrected one.
///
/// MUTATION THIS KILLS: a row of the parallel read taking its pixels' index
/// from the row alone (`i = x`).
#[test]
fn the_colour_field_read_side_by_side_is_the_serial_read_bit_for_bit() {
    // A lens-corrected frame, as the fit builds one for a registered plate.
    let corrected = |w: usize, h: usize| {
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w as u32, h as u32, |x, y| image::Rgb([(x * 5 % 256) as u8, (y * 3 % 256) as u8, 90])));
        let plate = crate::render::Plate::baked_in(image, true);
        FieldFrame::of_preview(&plate.shared(), &marked_recipe())
    };
    let mut ran = 0;
    for (w, h) in [(37usize, 23usize), (1, 1), (1, 29), (31, 1), (1031, 7)] {
        let own = FieldFrame::default();
        let lens = corrected(w, h);
        assert_eq!(format!("{lens:?}"), "FieldFrame(lens-corrected)", "premise: {w}x{h}: the plate reads through the lens");
        let px = rows_frame(w, h, (w * 7 + h * 13) as u32);
        let sky: Vec<f32> = (0..w * h).map(|i| ((i * 37 % 101) as f32 / 100.0) * 1.2 - 0.1).collect();
        for frame in [&own, &lens] {
            for (fx, fy, fb) in [(4usize, 3usize, 5usize), (2, 2, 2), (5, 1, 3)] {
                for owned in [false, true] {
                    for plane in [None, Some(sky.as_slice())] {
                        let field = rows_field(fx, fy, fb, owned);
                        assert!(field.renderable(), "premise: the field renders");
                        let (mut side, mut serial) = (px.clone(), px.clone());
                        apply_colour_field(&mut side, w, h, Some(&field), frame, plane);
                        serial_colour_field(&mut serial, w, h, Some(&field), frame, plane);
                        assert!(bits(&side) == bits(&serial), "{w}x{h} {frame:?} {fx}x{fy}x{fb} owned {owned} sky {}: the read is the serial read", plane.is_some());
                        if w * h > 1 {
                            assert!(bits(&side) != bits(&px), "premise: {w}x{h} {fx}x{fy}x{fb}: the field moves the frame");
                        }
                        ran += 1;
                    }
                }
            }
        }
    }
    assert_eq!(ran, 5 * 2 * 3 * 2 * 2);
}
