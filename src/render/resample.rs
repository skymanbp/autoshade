//! Resampling of the frame buffer: flips, orientation, downscale, straightening and the bilinear fetches.

use super::*;

/// In-place horizontal flip that stays in the image's OWN pixel type.
/// Calling `flip_horizontal_in_place(&mut DynamicImage)` goes through the
/// GenericImage adapter, whose Pixel is Rgba<u8> — that QUANTIZED f32/u16
/// frames to 8 bits (and clamped f32, killing wide-gamut negatives) on every
/// Transpose/Transverse-oriented photo since the A7 in-place rewrite; the
/// eight-state orientation test caught it (U14).
pub(super) fn flip_h_in_place(img: &mut DynamicImage) {
    use image::imageops::flip_horizontal_in_place as flip;
    match img {
        DynamicImage::ImageLuma8(b) => flip(b),
        DynamicImage::ImageLumaA8(b) => flip(b),
        DynamicImage::ImageRgb8(b) => flip(b),
        DynamicImage::ImageRgba8(b) => flip(b),
        DynamicImage::ImageLuma16(b) => flip(b),
        DynamicImage::ImageLumaA16(b) => flip(b),
        DynamicImage::ImageRgb16(b) => flip(b),
        DynamicImage::ImageRgba16(b) => flip(b),
        DynamicImage::ImageRgb32F(b) => flip(b),
        DynamicImage::ImageRgba32F(b) => flip(b),
        // DynamicImage is #[non_exhaustive]: a future variant falls back to
        // the adapter path (quantizing, but never silently skipped).
        other => flip(other),
    }
}

/// Orient the demosaiced f32 buffer BEFORE develop, so masks / crop /
/// straighten all live in the display frame (the C2 contract's "original").
/// Implemented by round-tripping through [`oriented`] on a lossless Rgb32F
/// image — one function owns the orientation semantics, no hand-derived
/// index math to drift. Identity (no copy) for Normal/Unknown.
pub(super) fn orient_f32(
    data: Vec<[f32; 3]>,
    w: usize,
    h: usize,
    o: Orientation,
) -> (Vec<[f32; 3]>, usize, usize) {
    if matches!(o, Orientation::Normal | Orientation::Unknown) {
        return (data, w, h);
    }
    // [f32;3] and 3×f32 share layout, so both casts are zero-copy — the old
    // flatten/collect + to_rgb32f() + pixels().collect() chain made THREE full
    // copies of a ~732 MB frame for every portrait RAW. try_cast_vec only
    // falls back to a real copy when the vec's capacity isn't an exact
    // multiple of the element ratio.
    let flat: Vec<f32> = bytemuck::cast_vec(data);
    let img = ImageBuffer::<Rgb<f32>, Vec<f32>>::from_raw(w as u32, h as u32, flat)
        .expect("orient_f32: buffer size matches dims");
    let out = match oriented(DynamicImage::ImageRgb32F(img), o) {
        DynamicImage::ImageRgb32F(b) => b, // rotations/flips keep the variant
        other => other.to_rgb32f(),
    };
    let (ow, oh) = out.dimensions();
    let data: Vec<[f32; 3]> = bytemuck::try_cast_vec(out.into_raw())
        .unwrap_or_else(|(_, v)| v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect());
    (data, ow as usize, oh as usize)
}

/// `DynamicImage::thumbnail` without its float bias: the same output size
/// (`image`'s own aspect rule, replicated in [`thumbnail_dims`]) and the same
/// sampling as [`thumbnail_exact`].
pub fn thumbnail(img: &DynamicImage, nwidth: u32, nheight: u32) -> DynamicImage {
    let (w, h) = thumbnail_dims(img.width(), img.height(), nwidth, nheight);
    thumbnail_exact(img, w, h)
}

/// `DynamicImage::thumbnail_exact` without its float bias. `image`'s block
/// average is `(sum + n/2) / n`, round-to-nearest for an integer sample but
/// `mean + 0.5` for an f32 one ([`downscale_f32`]'s note), so every float
/// frame it thumbnails comes back half a unit too bright. A float frame — a
/// base that keeps its over-range light (`render::base_develop`) — is
/// sampled here by the same windows with the plain mean; every integer frame
/// goes to `image` unchanged.
pub fn thumbnail_exact(img: &DynamicImage, nwidth: u32, nheight: u32) -> DynamicImage {
    match img {
        DynamicImage::ImageRgb32F(b) => {
            let (w, h) = b.dimensions();
            let out = thumbnail_f32::<3>(b.as_raw(), w, h, nwidth, nheight);
            DynamicImage::ImageRgb32F(ImageBuffer::from_raw(nwidth, nheight, out).expect("thumbnail size"))
        }
        DynamicImage::ImageRgba32F(b) => {
            let (w, h) = b.dimensions();
            let out = thumbnail_f32::<4>(b.as_raw(), w, h, nwidth, nheight);
            DynamicImage::ImageRgba32F(ImageBuffer::from_raw(nwidth, nheight, out).expect("thumbnail size"))
        }
        other => other.thumbnail_exact(nwidth, nheight),
    }
}

/// `image`'s `resize_dimensions(.., fill = false)`: the largest size of the
/// frame's aspect inside the box, each side at least 1.
fn thumbnail_dims(width: u32, height: u32, nwidth: u32, nheight: u32) -> (u32, u32) {
    let ratio = (f64::from(nwidth) / f64::from(width)).min(f64::from(nheight) / f64::from(height));
    let side = |v: u32| ((f64::from(v) * ratio).round() as u64).clamp(1, u64::from(u32::MAX)) as u32;
    (side(width), side(height))
}

/// `image::imageops::thumbnail` on an interleaved f32 frame of `C` channels:
/// its windows and its three fractional cases exactly, its block case with
/// the plain mean (f64 accumulation, as [`downscale_f32`]).
fn thumbnail_f32<const C: usize>(src: &[f32], width: u32, height: u32, nw: u32, nh: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; nw as usize * nh as usize * C];
    if width == 0 || height == 0 {
        return out;
    }
    let px = |x: u32, y: u32| &src[(y as usize * width as usize + x as usize) * C..][..C];
    let (x_ratio, y_ratio) = (width as f32 / nw as f32, height as f32 / nh as f32);
    out.par_chunks_mut(nw as usize * C).enumerate().for_each(|(outy, row)| {
        let bottomf = outy as f32 * y_ratio;
        let topf = bottomf + y_ratio;
        let bottom = (bottomf.ceil() as u32).clamp(0, height - 1);
        let top = (topf.ceil() as u32).clamp(bottom, height);
        for outx in 0..nw {
            let leftf = outx as f32 * x_ratio;
            let rightf = leftf + x_ratio;
            let left = (leftf.ceil() as u32).clamp(0, width - 1);
            let right = (rightf.ceil() as u32).clamp(left, width);
            let o = &mut row[outx as usize * C..][..C];
            if bottom != top && left != right {
                let mut sum = [0.0f64; C];
                for y in bottom..top {
                    for x in left..right {
                        for (s, v) in sum.iter_mut().zip(px(x, y)) {
                            *s += f64::from(*v);
                        }
                    }
                }
                let n = f64::from((right - left) * (top - bottom));
                for (o, s) in o.iter_mut().zip(sum) {
                    *o = (s / n) as f32;
                }
            } else if bottom != top {
                let fract = (leftf.fract() + rightf.fract()) / 2.0;
                let (l, n) = (right - 1, (top - bottom) as f32);
                for (c, o) in o.iter_mut().enumerate() {
                    let (mut a, mut b) = (0.0f32, 0.0f32);
                    for y in bottom..top {
                        a += px(l, y)[c];
                        b += px(l + 1, y)[c];
                    }
                    *o = (1.0 - fract) / n * a + fract / n * b;
                }
            } else if left != right {
                let fract = (topf.fract() + bottomf.fract()) / 2.0;
                let (t, n) = (top - 1, (right - left) as f32);
                for (c, o) in o.iter_mut().enumerate() {
                    let (mut a, mut b) = (0.0f32, 0.0f32);
                    for x in left..right {
                        a += px(x, t)[c];
                        b += px(x, t + 1)[c];
                    }
                    *o = (1.0 - fract) / n * a + fract / n * b;
                }
            } else {
                let fv = (topf.fract() + bottomf.fract()) / 2.0;
                let fh = (leftf.fract() + rightf.fract()) / 2.0;
                let (l, b) = (right - 1, top - 1);
                for (c, o) in o.iter_mut().enumerate() {
                    let (bl, tl, br, tr) = (px(l, b)[c], px(l, b + 1)[c], px(l + 1, b)[c], px(l + 1, b + 1)[c]);
                    *o = (1.0 - fv) * fh * br + fv * fh * tr + (1.0 - fv) * (1.0 - fh) * bl + fv * (1.0 - fh) * tl;
                }
            }
        }
    });
    out
}

/// Downscale the oriented f32 buffer to fit `max_edge` (aspect preserved).
/// The averaging is hand-rolled — a fresh `Vec<[f32; 3]>` filled through an
/// f64 accumulator, NOT a cast-and-delegate — for the bias reason spelled out
/// in the body. It must be the ORIENTED buffer: this binning only commutes
/// with pure axis swaps — reversing orientations shift the bin edges by one
/// source bin, so capping in the sensor frame would change preview pixels
/// (see the probe note in `render_to_image_in`). No-op when
/// the frame already fits. Backs `render_to_image`'s working-resolution
/// cap: developing 61 MP only to thumbnail the result wasted a ~1.5 GB
/// transient chain on every preview-resolution retouch base.
pub(super) fn downscale_f32(
    data: Vec<[f32; 3]>,
    w: usize,
    h: usize,
    max_edge: u32,
) -> (Vec<[f32; 3]>, usize, usize) {
    // Clamped ONCE and used for the bin ratios too — a raw 0 would produce a
    // zero-dimensional working image.
    let edge = max_edge.max(1);
    if w.max(h) <= edge as usize || w == 0 || h == 0 {
        return (data, w, h);
    }
    // The averaging is hand-rolled because `image::imageops::thumbnail` adds
    // an INTEGER rounding term before dividing: `(sum + n/2) / n`. For u8/u16
    // that is round-to-nearest; for f32 (whose `Enlargeable::Larger` is f64)
    // `n/2` stays a float, so every channel of every capped frame came back
    // exactly +0.5 too bright — GUI previews, retouch/generative bases, the
    // web preview and the camera-base-curve estimate all washed to white.
    //
    // An export's own pixels never went through here (deliverables pass
    // max_edge = None) — but saying exports "were never affected" was WRONG,
    // and this comment said it. `photo_base_knots` estimates the camera base
    // curve from a CAPPED develop and that curve is PERSISTED, so every
    // recipe saved by a build with the bias carries a curve fitted to a
    // washed frame, and the full-resolution render composes it. Fixing the
    // sampler made those saved curves worse, not better — they now sit over a
    // correct develop. See `pipeline::repair_pre_era_base_curve`.
    // The BIN GEOMETRY below is a faithful replica of that function (the same
    // aspect-preserving output dims and the same ceil-based windows), so the
    // orientation behaviour the canary test pins is unchanged — only the bias
    // is gone. Downscale-only means both ratios are ≥ 1, so every window
    // holds at least one source pixel and the fractional-edge cases of the
    // original cannot arise.
    let ratio = (edge as f64 / w as f64).min(edge as f64 / h as f64);
    let nw = ((w as f64 * ratio).round() as usize).max(1).min(w);
    let nh = ((h as f64 * ratio).round() as usize).max(1).min(h);
    let (x_ratio, y_ratio) = (w as f32 / nw as f32, h as f32 / nh as f32);
    let mut out = vec![[0.0f32; 3]; nw * nh];
    out.par_chunks_mut(nw).enumerate().for_each(|(oy, row)| {
        let bottomf = oy as f32 * y_ratio;
        let bottom = (bottomf.ceil() as usize).min(h - 1);
        let top = ((bottomf + y_ratio).ceil() as usize).clamp(bottom + 1, h);
        for (ox, px) in row.iter_mut().enumerate() {
            let leftf = ox as f32 * x_ratio;
            let left = (leftf.ceil() as usize).min(w - 1);
            let right = ((leftf + x_ratio).ceil() as usize).clamp(left + 1, w);
            // f64 accumulation: a 61 MP frame capped to 1280 sums ~2800
            // samples per output pixel, where f32 addition would drift.
            let mut sum = [0.0f64; 3];
            for y in bottom..top {
                for x in left..right {
                    let p = &data[y * w + x];
                    for c in 0..3 {
                        sum[c] += p[c] as f64;
                    }
                }
            }
            let n = ((top - bottom) * (right - left)) as f64;
            for c in 0..3 {
                px[c] = (sum[c] / n) as f32;
            }
        }
    });
    (out, nw, nh)
}

/// The largest axis-aligned rectangle (same aspect freedom as Lightroom's
/// auto-constrain) inscribed in a `w`×`h` rectangle rotated by `deg` degrees —
/// the closed-form solution, so a straightened image never shows black
/// corners. Public: the GUI shares this exact formula to map interaction
/// coordinates between the straightened view and the original frame.
pub fn inscribed_dims(w: f32, h: f32, deg: f32) -> (f32, f32) {
    let a = deg.abs().to_radians();
    if w <= 0.0 || h <= 0.0 {
        return (0.0, 0.0);
    }
    if a < 1e-6 {
        return (w, h);
    }
    let (s, c) = (a.sin(), a.cos());
    let (long, short) = (w.max(h), w.min(h));
    let cos2 = c * c - s * s;
    // At exactly 45° a square lands in the general branch with 0/0 → NaN →
    // a 1×1 output; cos2 ≈ 0 always means the half-diagonal fit applies.
    if short <= 2.0 * s * c * long || cos2.abs() < 1e-6 {
        // Thin case: the short side limits both dimensions (half-diagonal fit).
        let x = 0.5 * short;
        if w >= h { (x / s, x / c) } else { (x / c, x / s) }
    } else {
        ((w * c - h * s) / cos2, (h * c - w * s) / cos2)
    }
}

/// Straighten: rotate the image `deg` degrees CLOCKWISE about its centre
/// (bilinear resample) and auto-crop to the largest inscribed axis-aligned
/// rectangle ([`inscribed_dims`]) so no black corners survive. Identity when
/// `deg` rounds to zero. Works in 16-bit so the export path loses nothing;
/// the preview's 8-bit input survives the round-trip exactly.
pub fn rotate_straighten(img: &DynamicImage, deg: f32) -> DynamicImage {
    if deg.abs() < 1e-3 {
        return img.clone();
    }
    // A zero-size frame has no geometry to rotate, and the inscribed-rect math
    // below would hand the bilinear sampler an upper bound of -1 and panic.
    // The lens resamplers already guard this; this one did not.
    if img.width() == 0 || img.height() == 0 {
        return img.clone();
    }
    let src = rgb16_source(img);
    let src = &*src; // the samplers take a plain &ImageBuffer
    let (w, h) = (src.width() as f32, src.height() as f32);
    let (cw, ch) = inscribed_dims(w, h, deg);
    let (ow, oh) = ((cw.floor() as u32).max(1), (ch.floor() as u32).max(1));
    let rad = deg.to_radians();
    // Content rotates clockwise ⇒ inverse-map each dest pixel by the
    // counter-clockwise matrix (y-down screen coords): [c, s; -s, c].
    let (s, c) = (rad.sin(), rad.cos());
    let (cx_src, cy_src) = ((w - 1.0) * 0.5, (h - 1.0) * 0.5);
    let (cx_dst, cy_dst) = ((ow as f32 - 1.0) * 0.5, (oh as f32 - 1.0) * 0.5);
    let mut out: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::new(ow, oh);
    let obuf: &mut [u16] = &mut out;
    // Output rows are independent → parallel; per-pixel math is unchanged.
    obuf.par_chunks_mut(ow as usize * 3).enumerate().for_each(|(y, orow)| {
        let dy = y as f32 - cy_dst;
        for x in 0..ow as usize {
            let dx = x as f32 - cx_dst;
            let sx = c * dx + s * dy + cx_src;
            let sy = -s * dx + c * dy + cy_src;
            // Bilinear sample, clamped to the frame (the inscribed crop keeps
            // samples in-bounds up to float rounding at the very edge).
            orow[x * 3..x * 3 + 3].copy_from_slice(&sample_bilinear_rgb16(src, sx, sy).0);
        }
    });
    DynamicImage::ImageRgb16(out)
}

/// Single-channel bilinear fetch — SAME per-channel math as
/// [`sample_bilinear_rgb16`] (bit-identical result), for the CA path where
/// each channel samples at its own radius and the other two would be wasted.
pub(super) fn sample_bilinear_ch(src: &ImageBuffer<Rgb<u16>, Vec<u16>>, sx: f32, sy: f32, ch: usize) -> u16 {
    let (w, h) = (src.width() as f32, src.height() as f32);
    let x0 = sx.floor().clamp(0.0, w - 1.0);
    let y0 = sy.floor().clamp(0.0, h - 1.0);
    let x1 = (x0 + 1.0).min(w - 1.0);
    let y1 = (y0 + 1.0).min(h - 1.0);
    let (fx, fy) = ((sx - x0).clamp(0.0, 1.0), (sy - y0).clamp(0.0, 1.0));
    let p00 = src.get_pixel(x0 as u32, y0 as u32)[ch] as f32;
    let p10 = src.get_pixel(x1 as u32, y0 as u32)[ch] as f32;
    let p01 = src.get_pixel(x0 as u32, y1 as u32)[ch] as f32;
    let p11 = src.get_pixel(x1 as u32, y1 as u32)[ch] as f32;
    let top = p00 * (1.0 - fx) + p10 * fx;
    let bot = p01 * (1.0 - fx) + p11 * fx;
    (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 65535.0) as u16
}

/// Clamped bilinear lookup in a 16-bit RGB buffer — the shared resampling core
/// of the geometric ops ([`rotate_straighten`], [`apply_lens_distortion`]).
pub(super) fn sample_bilinear_rgb16(src: &ImageBuffer<Rgb<u16>, Vec<u16>>, sx: f32, sy: f32) -> Rgb<u16> {
    let (w, h) = (src.width() as f32, src.height() as f32);
    let x0 = sx.floor().clamp(0.0, w - 1.0);
    let y0 = sy.floor().clamp(0.0, h - 1.0);
    let x1 = (x0 + 1.0).min(w - 1.0);
    let y1 = (y0 + 1.0).min(h - 1.0);
    let (fx, fy) = ((sx - x0).clamp(0.0, 1.0), (sy - y0).clamp(0.0, 1.0));
    let p00 = src.get_pixel(x0 as u32, y0 as u32);
    let p10 = src.get_pixel(x1 as u32, y0 as u32);
    let p01 = src.get_pixel(x0 as u32, y1 as u32);
    let p11 = src.get_pixel(x1 as u32, y1 as u32);
    let mut v = [0u16; 3];
    for (ch_i, out_v) in v.iter_mut().enumerate() {
        let top = p00[ch_i] as f32 * (1.0 - fx) + p10[ch_i] as f32 * fx;
        let bot = p01[ch_i] as f32 * (1.0 - fx) + p11[ch_i] as f32 * fx;
        *out_v = (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 65535.0) as u16;
    }
    Rgb(v)
}
