//! A user-described image-edit endpoint (2026-10-17), for providers whose edit
//! call is not OpenAI's `POST /images/edits` multipart form.
//!
//! Venice is the case that asked for it: its base URL serves the OpenAI-shaped
//! `/responses` and `/chat/completions` the vision role uses, but answers
//! `/images/edits` with 404 — its edit call is `POST /image/multi-edit`, a JSON
//! body carrying base64 images, answered with the raw image bytes. Rather than
//! hard-code one provider, the user writes the call down once as a JSON
//! template (Settings → Image → Image-gen API → Custom, or
//! `AUTOSHADE_IMAGE_GEN_ENDPOINT`):
//!
//! ```json
//! { "path": "/image/multi-edit",
//!   "body": { "modelId": "{model}", "prompt": "{prompt}{marked_note}",
//!             "images": ["{image}", "{marked}"], "aspect_ratio": "auto" },
//!   "reply": "image" }
//! ```
//!
//! * `path` is appended to the image role's base URL. It may not name another
//!   host: the request carries the user's key, and the base URL is the one
//!   setting that decides where that key goes.
//! * `body` is any JSON value. Inside its strings, `{name}` is replaced by the
//!   request's value ([`PLACEHOLDERS`]); a string that is EXACTLY `{width}` or
//!   `{height}` becomes a JSON number; a string that is exactly one of the
//!   painted-area placeholders is dropped (array element / object key) when the
//!   edit has no painted area, and `{marked_note}` is then empty. `{mask}` is
//!   white where the picture should change.
//! * `{marked}` is for providers with NO mask parameter at all (Venice's edit
//!   calls have none — checked against its API reference 2026-10-17): the
//!   picture with the painted area tinted red, sent as another image, and
//!   `{marked_note}` the sentence that tells the model what the red means.
//!   Without it a blank-prompt removal — "remove everything inside the masked
//!   area" — reached the model with nothing saying where.
//! * `reply` is `"image"` (the body is the picture) or a JSON pointer such as
//!   `/data/0/b64_json` (the string there is base64 or a `data:` URL). Absent ⇒
//!   OpenAI's own shape, `/data/0/b64_json`.
//!
//! The answer is brought to the size that was asked for: the same picture at
//! the same aspect (within [`ASPECT_TOLERANCE`]) is resized to it — an endpoint
//! like Venice picks its own output resolution — and anything else is refused.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView};

use crate::generative::encode_png;

/// The default `reply`: where OpenAI's blocking edits response keeps the picture.
const OPENAI_REPLY: &str = "/data/0/b64_json";

/// Every name a template may use between braces.
pub const PLACEHOLDERS: [&str; 13] = [
    "model", "prompt", "image", "image_data_url", "mask", "mask_data_url", "marked",
    "marked_data_url", "marked_note", "width", "height", "size", "quality",
];

/// The placeholders that exist only when an area was painted.
const PAINTED: [&str; 5] = ["mask", "mask_data_url", "marked", "marked_data_url", "marked_note"];

/// What `{marked_note}` says. "The last image" rather than a number, so it
/// holds wherever the template puts `{marked}` after the picture.
const MARKED_NOTE: &str = "\n\nThe last image is this same photograph with the area to edit \
    tinted red. Apply the instruction ONLY inside that red area of the first image, leave \
    everything outside it exactly as it is, and do not reproduce the red tint anywhere.";

/// How strongly `{marked}` tints the painted area: clearly red, with the
/// content still readable, because the model has to see what it is removing.
const MARK_ALPHA: f32 = 0.55;

/// How far the answer's aspect may sit from the requested one and still be the
/// same picture at another resolution. Endpoints that choose from a fixed list
/// of ratios snap to the nearest (1.52:1 comes back 3:2), and 3% admits that
/// rounding while still refusing a neighbouring ratio (3:2 vs 4:3 differ by 12%).
const ASPECT_TOLERANCE: f64 = 0.03;

/// The shape of a template, shown in the empty Settings box. Provider-neutral
/// on purpose (user ruling 2026-10-17): the product offers OpenAI's call and a
/// custom one, and any given provider — Venice for this project's own setup —
/// is just one user's filled-in template, not a built-in.
pub const EXAMPLE: &str = r#"{
  "path": "/image/edit",
  "body": {
    "model": "{model}",
    "prompt": "{prompt}{marked_note}",
    "images": ["{image}", "{marked}"]
  },
  "reply": "/data/0/b64_json"
}"#;

/// A validated template.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageEndpoint {
    path: String,
    body: serde_json::Value,
    reply: Reply,
}

#[derive(Clone, Debug, PartialEq)]
enum Reply {
    /// The response body is the image.
    Image,
    /// A JSON pointer to a base64 string (or `data:` URL) in a JSON body.
    Pointer(String),
}

impl ImageEndpoint {
    /// Parse and check a template. Every refusal names what is wrong, because
    /// the user typed it.
    pub fn parse(text: &str) -> Result<Self> {
        let v: serde_json::Value =
            serde_json::from_str(text).context("the custom image endpoint is not valid JSON")?;
        let obj = v.as_object().ok_or_else(|| anyhow!("the custom image endpoint must be a JSON object"))?;
        if let Some(k) = obj.keys().find(|k| !matches!(k.as_str(), "path" | "body" | "reply")) {
            return Err(anyhow!("unknown key {k:?} in the custom image endpoint (path, body, reply)"));
        }
        let path = obj
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| anyhow!("the custom image endpoint needs a \"path\" string, e.g. \"/image/multi-edit\""))?;
        check_path(path)?;
        let body = obj.get("body").cloned().ok_or_else(|| anyhow!("the custom image endpoint needs a \"body\""))?;
        check_placeholders(&body)?;
        let reply = match obj.get("reply") {
            None => Reply::Pointer(OPENAI_REPLY.into()),
            Some(serde_json::Value::String(s)) if s == "image" => Reply::Image,
            Some(serde_json::Value::String(s)) if s.starts_with('/') => Reply::Pointer(s.clone()),
            Some(_) => {
                return Err(anyhow!(
                    "\"reply\" must be \"image\" or a JSON pointer such as \"/data/0/b64_json\""
                ))
            }
        };
        Ok(Self { path: path.to_string(), body, reply })
    }

    /// The URL this template posts to, under `base`.
    pub fn url(&self, base: &str) -> String {
        format!("{}{}", base.trim_end_matches('/'), self.path)
    }

    /// Does the body tell the model WHERE the painted area is — as a mask or
    /// as the red-marked copy? (`{marked_note}` alone shows it nothing.)
    pub fn takes_mask(&self) -> bool {
        ["mask", "mask_data_url", "marked", "marked_data_url"].iter().any(|n| mentions(&self.body, n))
    }
}

/// `path` stays on the base URL's host: a leading `/`, and nothing that could
/// make the joined string name a different authority or leave the path.
fn check_path(path: &str) -> Result<()> {
    let bad = !path.starts_with('/')
        || path.starts_with("//")
        || path.contains("://")
        || path.contains('@')
        || path.contains('\\')
        || path.contains('#')
        || path.split('/').any(|seg| seg == "..")
        || path.chars().any(|c| c.is_whitespace() || c.is_control());
    if bad {
        return Err(anyhow!(
            "\"path\" must be a path on the base URL, starting with / (e.g. \"/image/multi-edit\") — \
             no host, no spaces, no \"..\""
        ));
    }
    Ok(())
}

/// The `{name}` tokens in one string, as (start, end, name): lowercase ASCII
/// letters and `_` between braces. Anything else in braces is literal text.
fn tokens(s: &str) -> Vec<(usize, usize, &str)> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let name_len = bytes[i + 1..].iter().take_while(|b| b.is_ascii_lowercase() || **b == b'_').count();
            let close = i + 1 + name_len;
            if name_len > 0 && bytes.get(close) == Some(&b'}') {
                out.push((i, close + 1, &s[i + 1..close]));
                i = close + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn check_placeholders(v: &serde_json::Value) -> Result<()> {
    match v {
        serde_json::Value::String(s) => match tokens(s).into_iter().find(|(_, _, n)| !PLACEHOLDERS.contains(n)) {
            Some((_, _, name)) => {
                Err(anyhow!("unknown placeholder {{{name}}} — known: {}", PLACEHOLDERS.join(", ")))
            }
            None => Ok(()),
        },
        serde_json::Value::Array(a) => a.iter().try_for_each(check_placeholders),
        serde_json::Value::Object(o) => o.values().try_for_each(check_placeholders),
        _ => Ok(()),
    }
}

fn mentions(v: &serde_json::Value, name: &str) -> bool {
    match v {
        serde_json::Value::String(s) => tokens(s).iter().any(|(_, _, n)| *n == name),
        serde_json::Value::Array(a) => a.iter().any(|x| mentions(x, name)),
        serde_json::Value::Object(o) => o.values().any(|x| mentions(x, name)),
        _ => false,
    }
}

/// One request's values.
pub(crate) struct Values<'a> {
    pub model: &'a str,
    pub prompt: &'a str,
    pub quality: &'a str,
    pub width: u32,
    pub height: u32,
    pub image_png: &'a [u8],
    /// The painted area in OpenAI's form: an RGBA PNG whose TRANSPARENT pixels
    /// are the area to change.
    pub mask_png: Option<&'a [u8]>,
}

/// The template with `vals` filled in. The painted-area renderings are built
/// only when the template asks for them.
pub(crate) fn render_body(ep: &ImageEndpoint, vals: &Values<'_>) -> Result<serde_json::Value> {
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    let wants = |names: &[&str]| names.iter().any(|n| mentions(&ep.body, n));
    let image = b64(vals.image_png);
    let painted = vals.mask_png.map(painted_weight).transpose()?;
    let mask = match &painted {
        Some(w) if wants(&["mask", "mask_data_url"]) => Some(b64(&encode_png(&DynamicImage::ImageLuma8(w.clone()))?)),
        _ => None,
    };
    let marked = match &painted {
        Some(w) if wants(&["marked", "marked_data_url"]) => Some(b64(&mark_area(vals.image_png, w)?)),
        _ => None,
    };
    let lookup = |name: &str| -> Option<String> {
        Some(match name {
            "model" => vals.model.to_string(),
            "prompt" => vals.prompt.to_string(),
            "quality" => vals.quality.to_string(),
            "width" => vals.width.to_string(),
            "height" => vals.height.to_string(),
            "size" => format!("{}x{}", vals.width, vals.height),
            "image" => image.clone(),
            "image_data_url" => format!("data:image/png;base64,{image}"),
            "mask" => mask.clone()?,
            "mask_data_url" => format!("data:image/png;base64,{}", mask.as_ref()?),
            "marked" => marked.clone()?,
            "marked_data_url" => format!("data:image/png;base64,{}", marked.as_ref()?),
            "marked_note" => painted.as_ref().map(|_| MARKED_NOTE.to_string())?,
            _ => return None,
        })
    };
    Ok(fill(&ep.body, &lookup).unwrap_or(serde_json::Value::Null))
}

/// `None` ⇒ this value is a lone painted-area placeholder with nothing
/// painted, and is dropped from its container.
fn fill(v: &serde_json::Value, lookup: &dyn Fn(&str) -> Option<String>) -> Option<serde_json::Value> {
    use serde_json::Value;
    match v {
        Value::String(s) => {
            let toks = tokens(s);
            if let [(0, end, name)] = toks.as_slice()
                && *end == s.len()
            {
                if matches!(*name, "width" | "height") {
                    return lookup(name).and_then(|n| n.parse::<u64>().ok()).map(Value::from);
                }
                if PAINTED.contains(name) {
                    return lookup(name).map(Value::String);
                }
            }
            // ONE pass over the template's own text: a value that itself
            // contains "{prompt}" is never expanded again.
            let mut out = String::with_capacity(s.len());
            let mut at = 0;
            for (start, end, name) in toks {
                out.push_str(&s[at..start]);
                out.push_str(&lookup(name).unwrap_or_default());
                at = end;
            }
            out.push_str(&s[at..]);
            Some(Value::String(out))
        }
        Value::Array(a) => Some(Value::Array(a.iter().filter_map(|x| fill(x, lookup)).collect())),
        Value::Object(o) => Some(Value::Object(
            o.iter().filter_map(|(k, x)| fill(x, lookup).map(|x| (k.clone(), x))).collect(),
        )),
        other => Some(other.clone()),
    }
}

/// OpenAI's mask convention (transparent = change) as a weight plane, which
/// is also the black-and-white picture most inpainting APIs read (white =
/// change).
fn painted_weight(png: &[u8]) -> Result<image::GrayImage> {
    let m = image::load_from_memory(png).context("decode the mask")?.to_rgba8();
    Ok(image::GrayImage::from_fn(m.width(), m.height(), |x, y| image::Luma([255 - m.get_pixel(x, y)[3]])))
}

/// The picture with the painted area tinted red, for a model that takes no
/// mask. The mask is built at the picture's own size (`generative::edit_onto`
/// resizes both to the requested size); a mismatch is refused, not stretched.
fn mark_area(image_png: &[u8], weight: &image::GrayImage) -> Result<Vec<u8>> {
    let mut img = image::load_from_memory(image_png).context("decode the picture to mark")?.to_rgb8();
    if img.dimensions() != weight.dimensions() {
        return Err(anyhow!(
            "the painted area is {:?} but the picture is {:?}",
            weight.dimensions(),
            img.dimensions()
        ));
    }
    for (p, w) in img.pixels_mut().zip(weight.pixels()) {
        let a = MARK_ALPHA * f32::from(w[0]) / 255.0;
        for (c, target) in p.0.iter_mut().zip([255.0f32, 0.0, 0.0]) {
            *c = (f32::from(*c) * (1.0 - a) + target * a).round() as u8;
        }
    }
    encode_png(&DynamicImage::ImageRgb8(img))
}

/// The picture bytes in a reply, before decoding.
pub(crate) fn reply_bytes(ep: &ImageEndpoint, body: Vec<u8>) -> Result<Vec<u8>> {
    let ptr = match &ep.reply {
        Reply::Image => return Ok(body),
        Reply::Pointer(p) => p,
    };
    let v: serde_json::Value = serde_json::from_slice(&body).context("parse image API response")?;
    let s = v
        .pointer(ptr)
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("no image at {ptr} in the response"))?;
    let s = match s.strip_prefix("data:") {
        Some(rest) => rest.split_once(',').map(|(_, d)| d).ok_or_else(|| anyhow!("malformed data: URL in the response"))?,
        None => s,
    };
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .context("decode the response's base64 image")
}

/// Decode the endpoint's picture (PNG, JPEG or WebP) and bring it to `want` as
/// a PNG — refusing a different aspect, which would be a different picture.
/// Returns the PNG and the size the endpoint actually produced.
pub(crate) fn to_requested_size(bytes: &[u8], want: (u32, u32)) -> Result<(Vec<u8>, (u32, u32))> {
    let img = image::load_from_memory(bytes).context("decode the generated image")?;
    let got = img.dimensions();
    if got == want {
        return Ok((encode_png(&DynamicImage::ImageRgba8(img.to_rgba8()))?, got));
    }
    let (w, h) = got;
    if w == 0 || h == 0 || want.1 == 0 {
        return Err(anyhow!("image API returned {w}x{h}"));
    }
    let (a, b) = (f64::from(w) / f64::from(h), f64::from(want.0) / f64::from(want.1));
    if (a - b).abs() > b * ASPECT_TOLERANCE {
        return Err(anyhow!(
            "image API returned {w}x{h} for requested {}x{} — a different aspect, so a different picture",
            want.0,
            want.1
        ));
    }
    let resized = img.resize_exact(want.0, want.1, FilterType::Lanczos3).to_rgba8();
    Ok((encode_png(&DynamicImage::ImageRgba8(resized))?, got))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real provider's call, written as a user would (Venice's
    /// `/image/multi-edit`, checked against its API reference 2026-10-17): no
    /// mask parameter, the answer is the picture itself.
    const VENICE: &str = r#"{
      "path": "/image/multi-edit",
      "body": { "modelId": "{model}", "prompt": "{prompt}{marked_note}",
                "images": ["{image}", "{marked}"], "aspect_ratio": "auto",
                "output_format": "png", "safe_mode": false },
      "reply": "image" }"#;

    #[test]
    fn the_settings_example_is_a_valid_template() {
        let ep = ImageEndpoint::parse(EXAMPLE).unwrap();
        assert!(ep.takes_mask());
        assert_eq!(ep.url("https://x.example/v1"), "https://x.example/v1/image/edit");
    }

    fn blank(w: u32, h: u32) -> Vec<u8> {
        encode_png(&DynamicImage::ImageRgba8(image::RgbaImage::new(w, h))).unwrap()
    }

    /// A 2x1 mask whose LEFT pixel is painted (transparent = change).
    fn left_painted() -> Vec<u8> {
        let mut m = image::RgbaImage::new(2, 1);
        m.put_pixel(1, 0, image::Rgba([0, 0, 0, 255]));
        encode_png(&DynamicImage::ImageRgba8(m)).unwrap()
    }

    fn vals<'a>(image: &'a [u8], mask: Option<&'a [u8]>) -> Values<'a> {
        Values { model: "m-edit", prompt: "make it {blue}", quality: "high", width: 1536, height: 1024, image_png: image, mask_png: mask }
    }

    fn decoded(b64: &serde_json::Value) -> image::RgbImage {
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64.as_str().unwrap()).unwrap();
        image::load_from_memory(&bytes).unwrap().to_rgb8()
    }

    /// The case that asked for `{marked}`: a blank-prompt removal on an
    /// endpoint with no mask parameter. Painted ⇒ a second, red-marked image
    /// and the note saying what red means; nothing painted ⇒ neither.
    #[test]
    fn the_venice_example_shows_the_painted_area_only_when_there_is_one() {
        let ep = ImageEndpoint::parse(VENICE).unwrap();
        assert_eq!(ep.url("https://api.venice.ai/api/v1/"), "https://api.venice.ai/api/v1/image/multi-edit");
        assert!(ep.takes_mask());
        let img = encode_png(&DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 1, image::Rgb([0, 0, 200])))).unwrap();

        let body = render_body(&ep, &vals(&img, None)).unwrap();
        assert_eq!(body["modelId"], "m-edit");
        assert_eq!(body["prompt"], "make it {blue}", "no note without a painted area");
        assert_eq!(body["images"].as_array().unwrap().len(), 1);
        assert_eq!(body["images"][0], base64::engine::general_purpose::STANDARD.encode(&img));

        let mask = left_painted();
        let body = render_body(&ep, &vals(&img, Some(&mask))).unwrap();
        assert_eq!(body["prompt"], format!("make it {{blue}}{MARKED_NOTE}"));
        assert_eq!(body["images"].as_array().unwrap().len(), 2);
        let marked = decoded(&body["images"][1]);
        assert_eq!(marked.get_pixel(1, 0).0, [0, 0, 200], "outside the painted area: untouched");
        let p = marked.get_pixel(0, 0).0;
        assert!(p[0] > 120 && p[2] < 120, "inside: tinted red, got {p:?}");
    }

    #[test]
    fn placeholders_expand_once_numbers_stay_numbers_and_missing_masks_drop() {
        let ep = ImageEndpoint::parse(
            r#"{"path":"/x","body":{"p":"say: {prompt}","w":"{width}","s":"{size}",
                "images":["{image_data_url}","{mask}"],"m":"{mask_data_url}","lit":"{Not} {a-b} {"}}"#,
        )
        .unwrap();
        assert!(ep.takes_mask());
        let img = blank(2, 1);
        let body = render_body(&ep, &vals(&img, None)).unwrap();
        // The prompt's own "{blue}" is text, not a placeholder.
        assert_eq!(body["p"], "say: make it {blue}");
        assert_eq!(body["w"], 1536);
        assert_eq!(body["s"], "1536x1024");
        assert_eq!(body["images"].as_array().unwrap().len(), 1, "no mask ⇒ the element is dropped");
        assert!(body["images"][0].as_str().unwrap().starts_with("data:image/png;base64,"));
        assert!(body.get("m").is_none(), "no mask ⇒ the key is dropped");
        assert_eq!(body["lit"], "{Not} {a-b} {");

        // With a mask: transparent (change) becomes white.
        let mask = left_painted();
        let body = render_body(&ep, &vals(&img, Some(&mask))).unwrap();
        let g = decoded(&body["images"][1]);
        assert_eq!((g.get_pixel(0, 0)[0], g.get_pixel(1, 0)[0]), (255, 0));
    }

    #[test]
    fn templates_that_could_misroute_the_key_or_typo_are_refused() {
        for path in ["image/edit", "//evil.example/x", "/x://y", "/@evil", "/a/../../b", "/a b", "/a#b"] {
            let t = format!(r#"{{"path":"{path}","body":{{}}}}"#);
            assert!(ImageEndpoint::parse(&t).is_err(), "{path} must be refused");
        }
        assert!(ImageEndpoint::parse(r#"{"path":"/x","body":{"p":"{promt}"}}"#).is_err());
        assert!(ImageEndpoint::parse(r#"{"path":"/x","body":{},"replay":"image"}"#).is_err());
        assert!(ImageEndpoint::parse(r#"{"path":"/x","body":{},"reply":"json"}"#).is_err());
        assert!(ImageEndpoint::parse("not json").is_err());
        // A note with no picture shows the model nothing.
        assert!(!ImageEndpoint::parse(r#"{"path":"/x","body":{"p":"{prompt}{marked_note}"}}"#).unwrap().takes_mask());
    }

    #[test]
    fn replies_are_read_by_pointer_or_as_the_body() {
        let img = blank(3, 2);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&img);
        let openai = ImageEndpoint::parse(r#"{"path":"/x","body":{}}"#).unwrap();
        let body = format!(r#"{{"data":[{{"b64_json":"{b64}"}}]}}"#).into_bytes();
        assert_eq!(reply_bytes(&openai, body).unwrap(), img);
        let url = ImageEndpoint::parse(r#"{"path":"/x","body":{},"reply":"/out"}"#).unwrap();
        let body = format!(r#"{{"out":"data:image/png;base64,{b64}"}}"#).into_bytes();
        assert_eq!(reply_bytes(&url, body).unwrap(), img);
        let raw = ImageEndpoint::parse(VENICE).unwrap();
        assert_eq!(reply_bytes(&raw, img.clone()).unwrap(), img);
        assert!(reply_bytes(&openai, b"{}".to_vec()).is_err());
    }

    #[test]
    fn the_answer_is_resized_at_the_same_aspect_and_refused_at_another() {
        let (png, got) = to_requested_size(&blank(1536, 1024), (3072, 2048)).unwrap();
        assert_eq!(got, (1536, 1024));
        assert_eq!(image::load_from_memory(&png).unwrap().dimensions(), (3072, 2048));
        // 1264x848 is 1.4906:1 — a provider's rounding of 3:2.
        assert!(to_requested_size(&blank(1264, 848), (1536, 1024)).is_ok());
        assert!(to_requested_size(&blank(1024, 768), (1536, 1024)).is_err(), "4:3 is not 3:2");
    }
}
