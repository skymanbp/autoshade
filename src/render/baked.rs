//! The lens geometry a pixel file carries (v1.17.0), written into the file
//! itself.
//!
//! A develop bakes no lens geometry into a master: the tail resamples the
//! lens on every render. A file that DOES carry geometry in its pixels says
//! so in its metadata under one key, [`BAKED_GEOMETRY_KEY`]: a PNG `iTXt`
//! chunk, a TIFF private tag ([`BAKED_GEOMETRY_TIFF_TAG`]) or a JPEG comment
//! segment, read through [`SourceFrame::of`] by every surface that renders,
//! fits or bakes from the file, so the marker cannot be lost on the way.
//!
//! The marker names WHICH geometry was baked ([`BakedGeometry`]): the lens
//! profile, which of its corrections, the manual Distortion amount, and a
//! digest of the composed knots. A recipe that now asks for a different lens
//! geometry than the one in the pixels is refused ([`SourceFrame::check`])
//! rather than corrected twice or not at all. A file with no marker is in the
//! sensor frame — every master written before v1.17.0, and every baked photo
//! that is not a master.
//!
//! A MASK raster carries the same marker for the same reason (v1.17.0): the
//! reverse fit draws its zone rasters on a source in the lens-corrected frame
//! (`pipeline::fit_source`), and a raster says which frame it is drawn in
//! rather than being resampled into another one. The marker names the lens
//! geometry whose corrected frame it is ([`BakedGeometry::frame_of`]); a
//! raster with none is in the sensor frame, as every brush, segmentation and
//! raster written before v1.17.0 is. One entry reads a raster with its frame
//! ([`open_mask_raster`]), one writes it ([`save_mask_raster`]), and a raster
//! read under a recipe whose lens geometry is not the one it was drawn under
//! is refused ([`MaskRaster::check`]), never sampled in the wrong frame.

use super::*;
use std::io::{Read, Seek, SeekFrom, Write};

/// The one key the marker is stored under, in every format.
pub const BAKED_GEOMETRY_KEY: &str = "autoshade:baked-geometry";

/// The TIFF tag the marker is stored in (ASCII) — from the private range
/// (65000–65535) TIFF 6.0 leaves to applications.
pub const BAKED_GEOMETRY_TIFF_TAG: u16 = 65117;

/// The marker's format version. A file written by a newer format is refused
/// rather than read as something it may not mean.
const VERSION: u32 = 1;

/// The lens geometry baked into a master's pixels. See the module docs.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BakedGeometry {
    /// The marker's format version ([`VERSION`]).
    pub version: u32,
    /// The lens profile: `adobe:<profile name>` for an Adobe lens profile,
    /// `camera` for the correction data the camera wrote into the RAW.
    pub lens_profile: String,
    /// The profile's distortion is in the pixels.
    pub distortion: bool,
    /// The profile's lateral chromatic aberration (with any manual pair folded
    /// in, [`geometry_profile`]) is in the pixels.
    pub ca: bool,
    /// The profile's vignetting is in the pixels.
    pub vignette: bool,
    /// The manual Distortion amount in the pixels.
    pub manual_distortion: f32,
    /// FNV-1a 64 of the composed knots and the vignetting strength, so a
    /// changed Distortion strength or manual CA pair is told apart from the
    /// geometry in the pixels.
    pub knots: String,
}

impl BakedGeometry {
    /// Is a frame with this geometry baked in the lens-CORRECTED frame?
    /// Vignetting alone moves no pixel.
    pub fn corrects_frame(&self) -> bool {
        self.distortion || self.ca || self.manual_distortion != 0.0
    }

    /// The lens geometry that moves `r`'s SHARED frame — the green map a mask
    /// is read through: the composed profile distortion ([`geometry_profile`])
    /// and the manual Distortion amount, whether the camera stage or the tail
    /// resamples it ([`geometry_moves_frame`]). Chromatic aberration and
    /// vignetting move no mask, so they are not part of it: the frame a mask
    /// raster drawn on `r`'s corrected picture is in. `None` when `r` moves
    /// no mask.
    pub fn frame_of(r: &EditRecipe) -> Option<Self> {
        let lens_profile = match &r.lens_profile.adobe {
            Some(a) => format!("adobe:{}", a.name),
            None => "camera".to_string(),
        };
        Self::frame_marker(&geometry_profile(r), r.lens_distortion, lens_profile)
    }

    /// [`BakedGeometry::frame_of`] of a composed profile `geom` at the manual
    /// amount `amount`, named `lens_profile`.
    pub(super) fn frame_marker(geom: &crate::recipe::LensProfile, amount: f32, lens_profile: String) -> Option<Self> {
        if !geometry_moves_frame(geom, amount) {
            return None;
        }
        let distortion = geom.distortion_on && !geom.distortion.is_empty();
        let mut h = Fnv::default();
        h.u32(if distortion { geom.distortion.len() as u32 } else { 0 });
        if distortion {
            geom.distortion.iter().for_each(|k| h.u32(k.to_bits()));
        }
        Some(Self {
            version: VERSION,
            lens_profile,
            distortion,
            ca: false,
            vignette: false,
            manual_distortion: amount,
            knots: format!("fnv1a64:{:016x}", h.0),
        })
    }
}

impl std::fmt::Display for BakedGeometry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let on = |b: bool| if b { "on" } else { "off" };
        write!(
            f,
            "lens profile {} (distortion {}, chromatic aberration {}, vignetting {}, manual distortion {}, {})",
            self.lens_profile,
            on(self.distortion),
            on(self.ca),
            on(self.vignette),
            self.manual_distortion,
            self.knots
        )
    }
}

/// FNV-1a 64 over little-endian words: a digest that is the same on every
/// platform and build, which `std`'s hashers do not promise.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn u32(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// The frame a source's pixels are in — THE answer every surface takes from
/// [`SourceFrame::of`].
#[derive(Clone, Debug, PartialEq)]
pub enum SourceFrame {
    /// A camera RAW: its base develop is in the sensor frame.
    Raw,
    /// A baked file, with the geometry its marker says is in its pixels;
    /// `None` = no marker, the sensor frame.
    Baked(Option<BakedGeometry>),
}

impl SourceFrame {
    /// The frame of the file at `path`. A RAW is told by its kind; a baked
    /// file by its marker, read once per (path, modification time, length)
    /// for the process. `Err` when the file cannot be read or its marker
    /// cannot be parsed — never a silent "sensor frame".
    pub fn of(path: &Path) -> Result<Self> {
        if crate::decode::is_raw(path) {
            return Ok(Self::Raw);
        }
        type Slot = (std::path::PathBuf, Option<(Option<std::time::SystemTime>, u64)>, SourceFrame);
        static SEEN: std::sync::OnceLock<std::sync::Mutex<Vec<Slot>>> = std::sync::OnceLock::new();
        let ident = std::fs::metadata(path).ok().map(|m| (m.modified().ok(), m.len()));
        let seen = SEEN.get_or_init(Default::default);
        if let Some(hit) = seen
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .find(|(p, i, _)| p == path && *i == ident && i.is_some())
        {
            return Ok(hit.2.clone());
        }
        let frame = Self::Baked(read_baked_geometry(path)?);
        let mut held = seen.lock().unwrap_or_else(|p| p.into_inner());
        held.retain(|(p, _, _)| p != path);
        if held.len() >= 32 {
            held.remove(0);
        }
        held.push((path.to_path_buf(), ident, frame.clone()));
        Ok(frame)
    }

    /// Is this a camera RAW?
    pub fn is_raw(&self) -> bool {
        matches!(self, Self::Raw)
    }

    /// The geometry a marked baked file carries.
    pub fn baked(&self) -> Option<&BakedGeometry> {
        match self {
            Self::Baked(g) => g.as_ref(),
            Self::Raw => None,
        }
    }

    /// Are this source's pixels in the lens-corrected frame? A RAW's base
    /// never is; a baked file's is when its marker says so — this is what
    /// the pixels ARE ([`SourceFrame::check`] is what refuses a recipe over
    /// them).
    pub fn corrected(&self) -> bool {
        match self {
            Self::Raw => false,
            Self::Baked(g) => g.as_ref().is_some_and(BakedGeometry::corrects_frame),
        }
    }

    /// Refuse `r` on a marked master: a develop bakes no lens geometry, and
    /// applying `r`'s over pixels that carry one would correct the lens
    /// twice. A RAW and an unmarked file always pass.
    pub fn check(&self, r: &EditRecipe) -> Result<()> {
        let _ = r;
        let Some(baked) = self.baked() else { return Ok(()) };
        bail!(
            "this master has {baked} in its pixels, but a develop bakes no lens geometry — bake \
             the master again from the RAW"
        )
    }
}

/// The file's format, by its first bytes (a staged name's extension is
/// machinery, not a format).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Png,
    Tiff { big_endian: bool },
    Jpeg,
}

fn sniff(f: &mut std::fs::File) -> Result<Option<Kind>> {
    let mut head = [0u8; 8];
    f.seek(SeekFrom::Start(0))?;
    let n = read_up_to(f, &mut head)?;
    f.seek(SeekFrom::Start(0))?;
    let head = &head[..n];
    Ok(if head.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some(Kind::Png)
    } else if head.starts_with(b"II*\0") {
        Some(Kind::Tiff { big_endian: false })
    } else if head.starts_with(b"MM\0*") {
        Some(Kind::Tiff { big_endian: true })
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Kind::Jpeg)
    } else {
        None
    })
}

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

fn parse(text: &[u8], path: &Path) -> Result<BakedGeometry> {
    let g: BakedGeometry = serde_json::from_slice(text)
        .with_context(|| format!("{}: the {BAKED_GEOMETRY_KEY} marker does not parse", path.display()))?;
    if g.version != VERSION {
        bail!(
            "{}: the {BAKED_GEOMETRY_KEY} marker is version {}, this build reads version {VERSION}",
            path.display(),
            g.version
        );
    }
    Ok(g)
}

/// The geometry the file at `path` says is baked into it; `None` when it
/// carries no marker (or is a format that cannot carry one).
pub fn read_baked_geometry(path: &Path) -> Result<Option<BakedGeometry>> {
    let mut f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let text = match sniff(&mut f)? {
        Some(Kind::Png) => png_find(&mut f)?,
        Some(Kind::Tiff { big_endian }) => tiff_find(&mut f, big_endian)?,
        Some(Kind::Jpeg) => jpeg_find(&mut f)?,
        None => None,
    };
    text.map(|t| parse(&t, path)).transpose()
}

/// Write `g` into the file at `path` (a staged master, before it is
/// published), replacing a marker it already carries. PNG, TIFF and JPEG
/// carry one; any other format is refused, since a lens-corrected master
/// without its marker would be read in the sensor frame.
pub fn stamp_baked_geometry(path: &Path, g: &BakedGeometry) -> Result<()> {
    let text = serde_json::to_vec(g)?;
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    match sniff(&mut f)? {
        Some(Kind::Tiff { big_endian }) => tiff_stamp(&mut f, big_endian, &text)
            .with_context(|| format!("mark {}", path.display())),
        Some(kind) => {
            drop(f);
            rewrite_stamped(path, kind, &text).with_context(|| format!("mark {}", path.display()))
        }
        None => bail!(
            "{} is not PNG, TIFF or JPEG, so it cannot carry the lens geometry baked into it",
            path.display()
        ),
    }
}

/// Stamp `to` with the marker `from` carries, if any — for a product made from
/// a master pixel for pixel (a denoise).
pub fn carry_baked_geometry(from: &Path, to: &Path) -> Result<()> {
    match read_baked_geometry(from)? {
        Some(g) => stamp_baked_geometry(to, &g),
        None => Ok(()),
    }
}

// --- mask rasters ------------------------------------------------------------

/// A mask raster with the frame it is drawn in (see the module docs).
pub struct MaskRaster {
    /// The pixels, as decoded.
    pub image: DynamicImage,
    /// The lens geometry whose CORRECTED frame the pixels are drawn in;
    /// `None` is the sensor frame.
    pub frame: Option<BakedGeometry>,
}

impl MaskRaster {
    /// Refuse `r` on a raster drawn under another lens geometry than the one
    /// `r` corrects: sampled anyway, the mask would land a lens field away
    /// from where it was drawn. A sensor-frame raster suits every recipe.
    pub fn check(&self, r: &EditRecipe, path: &Path) -> Result<()> {
        check_mask_frame(self.frame.as_ref(), r, path)
    }

    /// The raster in the SENSOR frame, for a surface that draws there (the
    /// GUI's raster edits): itself, or a drawn-corrected raster read back
    /// through the engine's own map of `r`'s geometry ([`MaskRaster::check`]
    /// first), each sensor pixel taking the corrected point it lands on.
    pub fn into_sensor_gray(self, r: &EditRecipe, path: &Path) -> Result<image::GrayImage> {
        self.check(r, path)?;
        let gray = self.image.to_luma8();
        Ok(match self.frame {
            Some(_) => corrected_raster_to_sensor(&gray, &geometry_profile(r), r.lens_distortion),
            None => gray,
        })
    }
}

/// The lens-corrected frame a reverse fit draws its zone rasters in: the
/// marker its files carry ([`BakedGeometry::frame_of`]) and the map back to
/// the sensor frame, for the one coordinate the fit stores beside a raster
/// (a Select Sky component's reference point, which keeps the sensor-frame
/// convention every sidecar has carried).
#[derive(Clone, Debug)]
pub struct RasterFrame {
    marker: BakedGeometry,
    profile: crate::recipe::LensProfile,
    amount: f32,
}

impl RasterFrame {
    /// The corrected frame of `r`'s lens geometry; `None` when it moves no mask.
    pub fn of(r: &EditRecipe) -> Option<Self> {
        Some(Self {
            marker: BakedGeometry::frame_of(r)?,
            profile: geometry_profile(r).into_owned(),
            amount: r.lens_distortion,
        })
    }

    /// The marker a raster drawn in this frame carries.
    pub fn marker(&self) -> &BakedGeometry {
        &self.marker
    }

    /// The SENSOR-frame normalised point a normalised point of a `dims`
    /// raster in this frame shows ([`lens_geom_norm`], the map the resample
    /// reads the sensor frame through).
    pub fn sensor_point(&self, (nx, ny): (f32, f32), dims: (f32, f32)) -> (f32, f32) {
        lens_geom_norm(nx, ny, dims, &self.profile, self.amount)
    }
}

/// The frame a reverse fit on `src` under the calibration `base` draws its
/// rasters in: `base`'s corrected frame when `src` is a plate in the
/// lens-corrected frame ([`super::plate_corrected`]), else the sensor frame.
pub fn fit_raster_frame(src: &DynamicImage, base: &EditRecipe) -> Option<RasterFrame> {
    if super::plate_corrected(src) { RasterFrame::of(base) } else { None }
}

/// [`MaskRaster::check`] on a frame alone.
pub(super) fn check_mask_frame(frame: Option<&BakedGeometry>, r: &EditRecipe, path: &Path) -> Result<()> {
    let Some(g) = frame else { return Ok(()) };
    let wanted = BakedGeometry::frame_of(r);
    if wanted.as_ref() != Some(g) {
        let w = wanted.map_or_else(|| "no lens geometry".to_string(), |w| w.to_string());
        bail!(
            "the mask raster {} is drawn in the frame of {g}, but the develop corrects {w} — redraw the mask, \
             or set the lens corrections back",
            path.display()
        );
    }
    Ok(())
}

/// THE read of a mask raster: its pixels (bounded, [`open_mask_bounded`]) and
/// the frame its marker names. A marker that moves no pixel reads as the
/// sensor frame.
pub fn open_mask_raster(path: &Path) -> Result<MaskRaster> {
    let image = open_mask_bounded(path)?;
    let frame = read_baked_geometry(path)?.filter(BakedGeometry::corrects_frame);
    Ok(MaskRaster { image, frame })
}

/// THE write of a mask raster drawn in `frame` (`None`: the sensor frame) —
/// a PNG, marked when the frame is a corrected one, so the file says which
/// frame its pixels are in.
pub fn save_mask_raster(mask: &image::GrayImage, path: &Path, frame: Option<&BakedGeometry>) -> Result<()> {
    mask.save(path).with_context(|| format!("write {}", path.display()))?;
    stamp_mask_frame(path, frame)
}

/// Mark the raster file at `path` (written by someone else — a segmentation
/// sidecar) as drawn in `frame`; nothing for the sensor frame.
pub fn stamp_mask_frame(path: &Path, frame: Option<&BakedGeometry>) -> Result<()> {
    match frame.filter(|g| g.corrects_frame()) {
        Some(g) => stamp_baked_geometry(path, g),
        None => Ok(()),
    }
}

// --- PNG ---------------------------------------------------------------------

fn crc32(parts: &[&[u8]]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        std::array::from_fn(|n| {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            c
        })
    });
    let mut c = 0xFFFF_FFFFu32;
    for b in parts.iter().flat_map(|p| p.iter()) {
        c = t[((c ^ u32::from(*b)) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

fn itxt_payload(text: &[u8]) -> Vec<u8> {
    // keyword NUL, compression flag 0, method 0, empty language NUL, empty
    // translated keyword NUL, then the UTF-8 text.
    let mut d = Vec::with_capacity(BAKED_GEOMETRY_KEY.len() + 5 + text.len());
    d.extend_from_slice(BAKED_GEOMETRY_KEY.as_bytes());
    d.extend_from_slice(&[0, 0, 0, 0, 0]);
    d.extend_from_slice(text);
    d
}

/// The text of an `iTXt` chunk that is ours, or `None`.
fn our_itxt(data: &[u8]) -> Option<Vec<u8>> {
    let rest = data.strip_prefix(BAKED_GEOMETRY_KEY.as_bytes())?.strip_prefix(&[0, 0, 0])?;
    let lang_end = rest.iter().position(|b| *b == 0)?;
    let rest = &rest[lang_end + 1..];
    let tr_end = rest.iter().position(|b| *b == 0)?;
    Some(rest[tr_end + 1..].to_vec())
}

/// Walk the chunks before the image data for our marker chunk's text.
fn png_find(f: &mut std::fs::File) -> Result<Option<Vec<u8>>> {
    f.seek(SeekFrom::Start(8))?;
    loop {
        let mut head = [0u8; 8];
        if read_up_to(f, &mut head)? < 8 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
        let ty = &head[4..8];
        if ty == b"IDAT" || ty == b"IEND" {
            return Ok(None);
        }
        if ty == b"iTXt" && (len as usize) < (1 << 20) {
            let mut data = vec![0u8; len as usize];
            f.read_exact(&mut data)?;
            if let Some(text) = our_itxt(&data) {
                return Ok(Some(text));
            }
            f.seek(SeekFrom::Current(4))?;
        } else {
            f.seek(SeekFrom::Current(i64::from(len) + 4))?;
        }
    }
}

// --- the rewriting formats (PNG, JPEG) ---------------------------------------

/// Copy the file into a sibling with the marker inserted (and any old one
/// dropped), then put the sibling in its place.
fn rewrite_stamped(path: &Path, kind: Kind, text: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("mark.{}.{}", std::process::id(), crate::store::next_tmp_seq()));
    let res = (|| -> Result<()> {
        let mut src = std::io::BufReader::new(std::fs::File::open(path)?);
        let mut dst = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        match kind {
            Kind::Png => png_rewrite(&mut src, &mut dst, text)?,
            Kind::Jpeg => jpeg_rewrite(&mut src, &mut dst, text)?,
            Kind::Tiff { .. } => unreachable!("a TIFF is marked in place"),
        }
        dst.flush()?;
        drop(dst);
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

fn png_rewrite(src: &mut impl Read, dst: &mut impl Write, text: &[u8]) -> Result<()> {
    let mut sig = [0u8; 8];
    src.read_exact(&mut sig)?;
    dst.write_all(&sig)?;
    let mut stamped = false;
    loop {
        let mut head = [0u8; 8];
        let n = read_up_to(src, &mut head)?;
        if n == 0 {
            break;
        }
        if n < 8 {
            bail!("truncated PNG chunk header");
        }
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
        let ty = [head[4], head[5], head[6], head[7]];
        let ours = if &ty == b"iTXt" && (len as usize) < (1 << 20) {
            let mut data = vec![0u8; len as usize];
            src.read_exact(&mut data)?;
            let mut crc = [0u8; 4];
            src.read_exact(&mut crc)?;
            if our_itxt(&data).is_some() {
                true
            } else {
                dst.write_all(&head)?;
                dst.write_all(&data)?;
                dst.write_all(&crc)?;
                false
            }
        } else {
            dst.write_all(&head)?;
            let copied = std::io::copy(&mut (&mut *src).take(u64::from(len) + 4), dst)?;
            if copied != u64::from(len) + 4 {
                bail!("truncated PNG chunk");
            }
            false
        };
        // Right after the header chunk, before any image data: a reader
        // stops at the first IDAT ([`png_find`]).
        if !ours && !stamped && &ty == b"IHDR" {
            let data = itxt_payload(text);
            dst.write_all(&(data.len() as u32).to_be_bytes())?;
            dst.write_all(b"iTXt")?;
            dst.write_all(&data)?;
            dst.write_all(&crc32(&[b"iTXt", &data]).to_be_bytes())?;
            stamped = true;
        }
    }
    if !stamped {
        bail!("the PNG has no header chunk");
    }
    Ok(())
}

const JPEG_PREFIX: &str = "autoshade:baked-geometry=";

fn jpeg_rewrite(src: &mut impl Read, dst: &mut impl Write, text: &[u8]) -> Result<()> {
    let mut soi = [0u8; 2];
    src.read_exact(&mut soi)?;
    dst.write_all(&soi)?;
    let mut stamped = false;
    let emit = |dst: &mut dyn Write| -> std::io::Result<()> {
        let mut body = JPEG_PREFIX.as_bytes().to_vec();
        body.extend_from_slice(text);
        dst.write_all(&[0xFF, 0xFE])?;
        dst.write_all(&((body.len() + 2) as u16).to_be_bytes())?;
        dst.write_all(&body)
    };
    loop {
        let mut m = [0u8; 2];
        src.read_exact(&mut m)?;
        if m[0] != 0xFF {
            bail!("malformed JPEG marker");
        }
        let marker = m[1];
        // The marker goes after the APPn run (JFIF wants APP0 first) and
        // before the tables; everything from the scan on is copied as is.
        if !stamped && !(0xE0..=0xEF).contains(&marker) {
            emit(dst)?;
            stamped = true;
        }
        if marker == 0xDA || marker == 0xD9 {
            dst.write_all(&m)?;
            std::io::copy(src, dst)?;
            return Ok(());
        }
        let mut lb = [0u8; 2];
        src.read_exact(&mut lb)?;
        let len = u16::from_be_bytes(lb) as usize;
        if len < 2 {
            bail!("malformed JPEG segment length");
        }
        let mut body = vec![0u8; len - 2];
        src.read_exact(&mut body)?;
        if marker == 0xFE && body.starts_with(JPEG_PREFIX.as_bytes()) {
            continue;
        }
        dst.write_all(&m)?;
        dst.write_all(&lb)?;
        dst.write_all(&body)?;
    }
}

fn jpeg_find(f: &mut std::fs::File) -> Result<Option<Vec<u8>>> {
    let mut r = std::io::BufReader::new(&mut *f);
    r.seek(SeekFrom::Start(2))?;
    loop {
        let mut m = [0u8; 2];
        if read_up_to(&mut r, &mut m)? < 2 || m[0] != 0xFF {
            return Ok(None);
        }
        if m[1] == 0xDA || m[1] == 0xD9 {
            return Ok(None);
        }
        let mut lb = [0u8; 2];
        r.read_exact(&mut lb)?;
        let len = u16::from_be_bytes(lb) as usize;
        if len < 2 {
            return Ok(None);
        }
        if m[1] == 0xFE {
            let mut body = vec![0u8; len - 2];
            r.read_exact(&mut body)?;
            if let Some(text) = body.strip_prefix(JPEG_PREFIX.as_bytes()) {
                return Ok(Some(text.to_vec()));
            }
        } else {
            r.seek_relative(len as i64 - 2)?;
        }
    }
}

// --- TIFF --------------------------------------------------------------------

fn tiff_u16(b: &[u8], be: bool) -> u16 {
    if be { u16::from_be_bytes([b[0], b[1]]) } else { u16::from_le_bytes([b[0], b[1]]) }
}

fn tiff_u32(b: &[u8], be: bool) -> u32 {
    if be { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) }
}

/// The first IFD's entries (12 bytes each) and the offset of the next IFD.
fn tiff_ifd0(f: &mut std::fs::File, be: bool) -> Result<(Vec<[u8; 12]>, u32)> {
    let mut head = [0u8; 8];
    f.seek(SeekFrom::Start(0))?;
    f.read_exact(&mut head)?;
    let ifd = tiff_u32(&head[4..8], be);
    f.seek(SeekFrom::Start(u64::from(ifd)))?;
    let mut nb = [0u8; 2];
    f.read_exact(&mut nb)?;
    let n = tiff_u16(&nb, be) as usize;
    let mut raw = vec![0u8; n * 12 + 4];
    f.read_exact(&mut raw)?;
    let entries = raw[..n * 12].chunks_exact(12).map(|c| c.try_into().expect("12-byte entry")).collect();
    Ok((entries, tiff_u32(&raw[n * 12..], be)))
}

fn tiff_find(f: &mut std::fs::File, be: bool) -> Result<Option<Vec<u8>>> {
    let (entries, _) = tiff_ifd0(f, be)?;
    let Some(e) = entries.iter().find(|e| tiff_u16(&e[0..2], be) == BAKED_GEOMETRY_TIFF_TAG) else {
        return Ok(None);
    };
    let count = tiff_u32(&e[4..8], be) as usize;
    if count > (1 << 20) {
        bail!("the {BAKED_GEOMETRY_KEY} tag is {count} bytes long");
    }
    let mut v = vec![0u8; count];
    if count <= 4 {
        v.copy_from_slice(&e[8..8 + count]);
    } else {
        f.seek(SeekFrom::Start(u64::from(tiff_u32(&e[8..12], be))))?;
        f.read_exact(&mut v)?;
    }
    // ASCII: the text and its terminating NUL.
    while v.last() == Some(&0) {
        v.pop();
    }
    Ok(Some(v))
}

/// Append a copy of the first IFD with the marker tag added (or replaced) at
/// the end of the file and point the header at it. Every other entry is
/// copied as it was, so its out-of-line values stay where they are; the old
/// IFD is left in place, unreferenced.
fn tiff_stamp(f: &mut std::fs::File, be: bool, text: &[u8]) -> Result<()> {
    let (mut entries, next) = tiff_ifd0(f, be)?;
    entries.retain(|e| tiff_u16(&e[0..2], be) != BAKED_GEOMETRY_TIFF_TAG);
    let mut value = text.to_vec();
    value.push(0);
    let end = f.seek(SeekFrom::End(0))?;
    let ifd_at = end + (end & 1); // word-aligned
    let value_at = ifd_at + 2 + 12 * (entries.len() as u64 + 1) + 4;
    if value_at + value.len() as u64 > u64::from(u32::MAX) {
        bail!("the TIFF is too large to take its marker");
    }
    let w16 = |v: u16| if be { v.to_be_bytes() } else { v.to_le_bytes() };
    let w32 = |v: u32| if be { v.to_be_bytes() } else { v.to_le_bytes() };
    let mut ours = [0u8; 12];
    ours[0..2].copy_from_slice(&w16(BAKED_GEOMETRY_TIFF_TAG));
    ours[2..4].copy_from_slice(&w16(2)); // ASCII
    ours[4..8].copy_from_slice(&w32(value.len() as u32));
    ours[8..12].copy_from_slice(&w32(value_at as u32));
    entries.push(ours);
    entries.sort_by_key(|e| tiff_u16(&e[0..2], be));
    let mut block = Vec::with_capacity((value_at - end) as usize + value.len());
    if ifd_at > end {
        block.push(0);
    }
    block.extend_from_slice(&w16(entries.len() as u16));
    entries.iter().for_each(|e| block.extend_from_slice(e));
    block.extend_from_slice(&w32(next));
    block.extend_from_slice(&value);
    f.write_all(&block)?;
    f.seek(SeekFrom::Start(4))?;
    f.write_all(&w32(ifd_at as u32))?;
    f.flush()?;
    Ok(())
}
