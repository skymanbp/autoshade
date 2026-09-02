//! Zero-dependency i18n for the native GUI (English skeleton · Chinese overlay).
//!
//! English is the SKELETON: every user-facing string literal in the GUI
//! modules (`src/bin/gui/*`) is
//! passed to [`tr`] AS its English text, which doubles as the lookup key.
//! [`Lang::En`] returns that key verbatim (no table walk); [`Lang::Zh`] looks it
//! up in the single [`ZH_ENTRIES`] catalogue and FALLS BACK to the English key
//! when a translation is missing — so an un-translated string renders in English
//! rather than blank. That is the whole mechanism: no external crate, no
//! codegen, one catalogue to maintain (the project's "language version control").
//!
//! Runtime interpolation ([`trf`]): Rust's `format!` requires a compile-time
//! literal format string, so a *translated* (runtime) string can't be handed to
//! it. Instead callers pass named placeholders (`{name}`) plus their
//! substitutions and `trf` does a plain string replace — identical behaviour in
//! English and Chinese, and the placeholder order is free to differ per language.
//!
//! This file is a PRIVATE submodule of the GUI binary (`mod i18n;` in its
//! root), not a binary itself —
//! see `autobins = false` in Cargo.toml for why that distinction matters.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The UI language. `En` is both the default and the skeleton (see module docs).
/// Persisted in `Prefs` (eframe storage); a save from an older build that
/// predates this field decodes to `En` via `#[serde(default)]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Lang {
    #[default]
    En,
    Zh,
}

impl Lang {
    /// Native display name for the language picker (never translated).
    pub fn label(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Zh => "中文",
        }
    }
}

/// Translate a skeleton (English) string. `En` returns it verbatim; `Zh` looks
/// it up in [`ZH_ENTRIES`], falling back to the English key when untranslated.
pub fn tr(lang: Lang, en: &'static str) -> &'static str {
    let s = match lang {
        Lang::En => en,
        Lang::Zh => zh_map().get(en).copied().unwrap_or(en),
    };
    platform_modifier(s)
}

/// Say the modifier key this platform's users actually press.
///
/// The BINDINGS are already right: every shortcut is
/// `egui::Modifiers::COMMAND`, which egui maps to Command on a Mac and Ctrl
/// everywhere else (`app.rs` names `Modifiers::CTRL` nowhere at all). Only the
/// hint strings still said "Ctrl+", so on a Mac the app named a key its own
/// bindings do not listen for.
///
/// Rewritten HERE and not in the ~50 literals, and that is the point: those
/// literals are the i18n KEYS. Editing them means editing the Chinese
/// catalogue's keys in the same breath and re-proving the two still line up —
/// for a change no Windows user is meant to see. Off macOS this is the
/// identity function by `cfg`, not by branch, so those strings are the same
/// bytes they have always been.
///
/// "Cmd+" rather than ⌘, deliberately: the embedded font subsets are generated
/// from the catalogue (`scripts/subset_gui_fonts.py`), U+2318 appears nowhere
/// in it, and the runtime face that would supply it is the system CJK font —
/// the one most at risk of being refused by the per-face byte budget. A glyph
/// that renders as a box would be worse than the word.
#[cfg(not(target_os = "macos"))]
fn platform_modifier(s: &'static str) -> &'static str {
    s
}

/// Interned, because [`tr`] returns `&'static str` and a rewrite has to
/// outlive the call. Bounded by the number of DISTINCT strings that mention
/// the modifier — the English keys plus their translations — each rewritten
/// and leaked at most once, holding memory the literal would have held anyway.
#[cfg(target_os = "macos")]
fn platform_modifier(s: &'static str) -> &'static str {
    if !s.contains("Ctrl+") {
        return s;
    }
    static CACHE: OnceLock<std::sync::Mutex<HashMap<&'static str, &'static str>>> =
        OnceLock::new();
    let mut cache =
        CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    *cache
        .entry(s)
        .or_insert_with(|| Box::leak(s.replace("Ctrl+", "Cmd+").into_boxed_str()))
}

/// Translate + interpolate. `args` are `(name, value)` pairs; each `{name}`
/// placeholder in the (possibly translated) string is replaced by `value`.
/// Used for every string that was a `format!(…)` before i18n: `format!` needs a
/// compile-time literal, so a translated string is filled by plain replacement.
/// A placeholder a translation happens to drop is simply left as-is (visible),
/// never a panic.
/// An argument VALUE that is a word, not a measurement, renders in the session
/// language too.
///
/// [`trf`] copies argument values verbatim, which is right for a number, a path
/// or a model's own prose — and wrong for the handful of args whose value is an
/// enumerated English phrase the engine chose from a fixed set
/// (`rationale::values`). Those were the one untranslated fragment inside an
/// otherwise translated sentence: the zh Fit panel read
/// 「高强度拟合显示 luma ranges 中…」.
///
/// A MECHANISM, not a special case: every argument of every note passes
/// through here, and the registry is `rationale::values::ALL`, which the i18n
/// audit extracts — so a new enumerated value that has no zh row fails the
/// gate instead of appearing in English.
///
/// The match is on the WHOLE value, never on a substring: anything not in the
/// registry is returned untouched, so a measurement, a path, or a model's prose
/// that merely mentions one of these phrases can never be rewritten by a
/// catalogue lookup.
fn tr_value(lang: Lang, value: &str) -> &str {
    match autoshade::rationale::values::ALL.iter().find(|v| **v == value) {
        Some(enumerated) => tr(lang, enumerated),
        None => value,
    }
}

pub fn trf(lang: Lang, en: &'static str, args: &[(&str, &str)]) -> String {
    // ONE pass over the TEMPLATE, never over inserted values: the old
    // sequential replace rescanned everything already substituted, so a value
    // that happened to contain placeholder syntax — a directory literally
    // named `{count}`, an OS error quoting a braced path — was reinterpreted
    // as markup by the next argument's pass and silently rewritten.
    let template = tr(lang, en);
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        match after.find('}') {
            Some(close) => {
                let name = &after[1..close];
                match args.iter().find(|(n, _)| *n == name) {
                    Some((_, value)) => out.push_str(tr_value(lang, value)),
                    // A placeholder a translation drops (or a caller does not
                    // supply) stays visible as-is — the existing contract.
                    None => out.push_str(&after[..=close]),
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Lazily materialise the English→Chinese lookup from the flat [`ZH_ENTRIES`]
/// slice (built once, on the first `Zh` translation).
fn zh_map() -> &'static HashMap<&'static str, &'static str> {
    static ZH: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    ZH.get_or_init(|| ZH_ENTRIES.iter().copied().collect())
}

/// THE single translation catalogue — "language version control" lives here.
/// English skeleton key → Chinese value, grouped by UI region. Add a pair when a
/// new English string is introduced; a key with no pair falls back to English.
/// Keys MUST match the English literal at the `tr`/`trf` call site byte-for-byte
/// (placeholders included), or the lookup silently misses.
#[rustfmt::skip]
static ZH_ENTRIES: &[(&str, &str)] = &[
    (" [look reference: finished photo {stem} from the photographer's look library; tags: {tags}]", " [外观参考：来自你的成片外观库的照片 {stem}；标签：{tags}]"),
    (" [finished look photo {stem} also went to the vision model as IMAGE 2]", " [成片 {stem} 同时作为 IMAGE 2 发给了视觉模型]"),
    (" [look library unavailable for this develop ({n} finished photos): style embedding was off or no query vector was produced]", " [本次显影用不到外观库（库中有 {n} 张成片）：风格嵌入未开启，或没有生成查询向量]"),
    (" [look descriptions: {n} of {total} exemplars carry a local prose description]", " [外观描述：{total} 条样本中有 {n} 条带本地生成的文字描述]"),
    (" [direction adherence tier: {tier}]", " [方向遵循档位：{tier}]"),
    (" White balance withheld: it would paint hues the target does not contain.", " 白平衡已保留：目标没有这些色相。"),
    (" White balance was clamped into the strength budget (gain ratio {from} to {to}, rotated share {rotated_share} over {coverage} of the frame); the requested cast exceeded the honest range.", " 白平衡已限制在强度预算内（增益比 {from} 至 {to}，色相旋转份额 {rotated_share}，覆盖画面 {coverage}）；请求色偏超出范围。"),
    (" White balance withheld: rotated share {rotated_share} over {coverage} of the frame exceeded the strength budget.", " 白平衡已保留：色相旋转份额 {rotated_share}（覆盖画面 {coverage}）超出强度预算。"),
    (" White balance search reached the {k} K domain bound; the requested colour temperature may lie beyond the fitted range.", " 白平衡达到 {k} K 的域边界；请求的色温可能超出拟合范围。"),
    (" Colour-cast curves were admitted by the strength budget (measured ratio {ratio}, budget {budget}).", " 色偏曲线已获强度预算准入（测得比值 {ratio}，预算 {budget}）。"),
    (" Colour-cast curves were admitted: the look error with the curves is \
      {ratio} of the error without them (the strength budget's bound is \
      {bound}), and they re-hued {rehued} of the frame past the rotation \
      budget.",
        " 色偏曲线已采用：加曲线后的观感误差与不加曲线时的比值为 {ratio}（强度预算的上限为 {bound}），并使画面的 {rehued} 超出色相旋转预算。"),
    (" They created {foreign} of the frame in hues the target does not \
      contain.",
        " 它们在目标不含的色相上新增了画面的 {foreign}。"),
    (" The foreign-hue share was not measurable: the target carries too \
      little colour to say which hues are foreign to it.",
        " 目标外色相份额无法测量：目标的色彩太少，无法判定色相是否为它所不含。"),
    (" They changed the widest hue class's hue spread across luminance by \
      {fan} degrees, against a limit of {limit} (a negative value means \
      they narrowed it).",
        " 它们把占比最大的色相类内沿亮度的色相跨度改变了 {fan} 度，上限为 {limit}（负值表示跨度变小）。"),
    (" The hue fan was not measurable: no hue class holds enough of the \
      frame across two luma slices for a fan to have a value.",
        " 色相沿亮度的跨度无法测量：任何色相类都没有在两个亮度切片上占到足够的画面。"),
    (" Colour-cast curves were withheld: they would have fanned one hue \
      class apart across luminance — that class holds {share} of the \
      frame's measurable colour, and the three channel curves would have \
      spread its dark and bright slices {fan} degrees apart (limit \
      {limit}). Shrinking them — first toward the one shape all three \
      channels share, then toward no curves at all — was tried first: \
      either no point on that path cleared the limit, or the \
      best-paying point that did bought less than the fit's own \
      quantisation budget, so none shipped. A single-hued region sorted \
      into a hue fan by brightness is not something any develop control \
      can put back.",
        " 色偏曲线已被扣留：它们会把同一个色相类沿亮度拉散——该色相类占画面可测色彩的 {share}，而三条通道曲线会把它的暗部与亮部切片拉开 {fan} 度（上限 {limit}）。先尝试过把它们收缩——先收向三个通道共有的那一个形状，再收向完全不加曲线：这条路上没有任何一点进入上限之内，或者进入上限之内改进最大的那一点带来的改进也不足拟合自身的量化预算，于是一条也没有采用。单一色相的区域被按明暗分选成多个色相，任何显影控件都无法还原。"),
    (" Colour-cast curves were shrunk toward the shape all three channels \
      share: as fitted they would have opened a {fan_before} degree hue \
      fan in a class holding {share} of the frame's measurable colour \
      (limit {limit}), so they were taken back to t = {t} of the fitted \
      cast (1 = as fitted, 0.5 = one curve shared by all three channels, \
      0 = none). The look error with the shrunk curves is {ratio} of the \
      error without them (the strength budget's bound is {bound}), and \
      they re-hued {rehued} of the frame past the rotation budget.",
        " 色偏曲线已向三个通道共有的形状收缩：按拟合原样，它们会在占画面可测色彩 {share} 的色相类内拉开 {fan_before} 度的色相跨度（上限 {limit}），因此把它们收缩到拟合值的 t = {t}（1 = 原样，0.5 = 三个通道共用一条曲线，0 = 不加曲线）。加上收缩后曲线的观感误差与不加曲线时的比值为 {ratio}（强度预算的上限为 {bound}），并使画面的 {rehued} 超出色相旋转预算。"),
    (" The projected curves change that class's hue spread across \
      luminance by {fan_after} degrees, inside the {target} degree target \
      a projection has to reach.",
        " 收缩后的曲线把该色相类沿亮度的色相跨度改变了 {fan_after} 度，在收缩需要达到的 {target} 度目标之内。"),
    (" The projected curves' hue fan was not measurable: no hue class \
      holds enough of the frame across two luma slices for a fan to have \
      a value.",
        " 收缩后曲线的色相沿亮度跨度无法测量：任何色相类都没有在两个亮度切片上占到足够的画面。"),
    (" The finished recipe was read once more against the untouched photo: \
      it sorted one hue class apart across luminance — that class holds \
      {share} of the frame's measurable colour and its dark and bright \
      slices ended {fan} degrees apart, against a limit of {limit} — so \
      the colour-cast curves were taken back out, which brings that class \
      inside the limit.",
        " 成品配方再对照未处理的原片读了一次：它把同一个色相类沿亮度分选开了——该色相类占画面可测色彩的 {share}，其暗部与亮部切片最终相差 {fan} 度，上限为 {limit}——因此把色偏曲线重新取了出来，这使该色相类回到上限之内。"),
    (" The finished recipe was read once more against the untouched photo: \
      it sorted one hue class apart across luminance — that class holds \
      {share} of the frame's measurable colour and its dark and bright \
      slices ended {fan} degrees apart, against a limit of {limit}. Taking \
      the colour-cast curves back out still left {after} degrees, so they \
      were not the cause and they were kept; tone and saturation are what \
      spread that class, and no develop control puts a brightness-sorted \
      hue fan back.",
        " 成品配方再对照未处理的原片读了一次：它把同一个色相类沿亮度分选开了——该色相类占画面可测色彩的 {share}，其暗部与亮部切片最终相差 {fan} 度，上限为 {limit}。把色偏曲线取出来之后仍有 {after} 度，说明起因不是这些曲线，于是把它们保留了下来；把该色相类拉散的是影调与饱和度，而任何显影控件都无法把按明暗分选出来的色相跨度还原。"),
    (" Global colour cast measured from consistent hue rotation across the populated frame (rotation {rotation} degrees, chroma ratio {ratio}); white balance and saturation were read from population evidence.", " 已从画面中各色相带一致的色相旋转测得全局色偏（旋转 {rotation} 度，色度比 {ratio}）；白平衡和饱和度取自整体证据。"),
    (" High-strength fit disclosed unsupported movement in {kind}: {ranges}. The controls were retained, but confidence is capped by the strength budget.", " 高强度拟合显示 {kind} 中没有双侧证据的移动：{ranges}。控制保留，置信度受强度预算限制。"),
    // Enumerated ARGUMENT values (`rationale::values`), substituted through
    // `tr_value` — the `{kind}` above is one of these two.
    ("luma ranges", "明度范围"),
    ("hue bands", "色相带"),
    (" [the proposal exceeded recipe limits — discarded {dropped}]",
        " [该提案超出了配方上限——已丢弃 {dropped}]"),
    // ── Settings ────────────────────────────────────────────────────────────
    ("Language", "语言"),
    ("Reverse-fit", "反推 / Reverse-fit"),
    ("Zoned fit (sky)", "分区反推：天空 / Zoned fit (sky)"),
    ("Up to four semantic regions", "最多四个语义区域 / Up to four semantic regions"),
    ("Opt in to semantic regions beyond the historical sky/land pass; this costs one OneFormer pass per frame and may take longer.", "选择历史天空/地面流程之外的语义区域；每次 OneFormer 推理，耗时可能更长。"),
    ("On reverse-fit, fit globally first. Sky segmentation and native luminance-range fallback stay exclusive; then frozen-evidence spatial tiles are tried automatically on a 4x4 grid with a four-tile cap and zero frame regression. Conservative guided refinement may keep or abstain before fitting semantic/tile masks, and never changes luminance ranges. Bitmap masks stay engine-only with a named XMP loss; native ranges are written to the Lightroom sidecar. Segmentation needs the python dependencies (transformers + torch), and every fallback or abstention is noted in the rationale.",
        "反推时先做全局拟合。天空语义分割与原生亮度范围回退二选一；随后自动在 4x4 网格上尝试冻结证据空间图块，最多四个且不允许画面回归。受限的引导细化会在拟合语义/图块蒙版前选择保留或放弃，绝不改变亮度范围。位图蒙版仅由本机引擎渲染并带具名 XMP 损失；原生亮度范围会写入 Lightroom 边车。分割需要 python 依赖（transformers + torch），每次回退或放弃都会写入理由。"),
    ("Analysis — the verifier", "分析 · 校验器"),
    ("Provider", "提供方"),
    ("Model", "模型"),
    ("Base URL", "基础 URL"),
    ("API Key", "API 密钥"),
    ("key set — blank keeps it", "已设密钥 — 留空则保留"),
    ("no key set", "未设密钥"),
    ("Image — the vision proposer + generative edits", "图像 · 视觉提案 + 生成式编辑"),
    ("OAuth (Codex bridge / ChatGPT sub)", "OAuth (Codex 桥 / ChatGPT 订阅)"),
    ("OAuth (Claude CLI)", "OAuth（Claude CLI）"),
    ("API (OpenAI-compatible)", "API（兼容 OpenAI）"),
    ("fetching…", "拉取中… / fetching…"),
    ("🔄 Fetch models", "🔄 拉取可用模型 / Fetch models"),
    ("List the models this endpoint serves (GET /models) so you can pick instead of guess — and a live reachability check for the bridge/API. Uses the key/token typed below; a saved key is only used at the endpoint it was saved for.",
        "列出该端点提供的模型（GET /models），可挑选而非猜测 —— 并对桥接/API 做一次连通性检查。使用下方输入的密钥/令牌；已保存的密钥只会用于保存它时的那个端点。"),
    ("{chat} chat · {image} image", "{chat} 对话 · {image} 图像"),
    ("Bridge URL", "桥接 URL"),
    ("Vision model", "视觉模型"),
    ("Image-gen model", "生图模型"),
    ("Gate token", "网关令牌"),
    ("set — blank keeps it", "已设 — 留空则保留"),
    ("the bridge's own api-keys token (loopback, not a cloud key)", "桥接自身的 api-keys 令牌（回环地址，不是云端密钥）"),
    ("OAuth rides your ChatGPT subscription via the local Codex bridge — no OpenAI key. Start the bridge first (else edits fail to connect). Generative output is capped at ~1.5 MP by the subscription image tier; for full-resolution edits switch to API mode with a real key.",
        "OAuth 通过本地 Codex 桥使用你的 ChatGPT 订阅 —— 无需 OpenAI 密钥。请先启动桥（否则编辑无法连接）。生成输出受订阅图像档位限制，上限约 1.5 MP；需全分辨率编辑请切换到 API 模式并用真实密钥。"),
    ("Tip: gpt-image-1.5 keeps the photo most faithful (input_fidelity); newer models like gpt-image-2 ignore that lock and edit more freely.",
        "提示：gpt-image-1.5 对照片最忠实（input_fidelity）；gpt-image-2 等较新模型会忽略该锁定、编辑更自由。"),
    ("Save settings", "保存设置"),
    ("saved → {path}", "已保存 → {path}"),
    ("save failed: {err}", "保存失败: {err}"),

    // ── Local adjustments (masks) ────────────────────────────────────────────
    ("Linear", "线性"),
    ("Radial", "径向"),
    ("Bitmap", "位图"),
    // The mask-row kind label for a Lightroom `Mask/Aggregate` group.
    ("Brush", "画笔"),
    ("mask", "蒙版"),
    ("Sky (reverse-fit)", "天空（反推）"),
    ("Land (reverse-fit)", "地景（反推）"),

    // ── Gallery / Library ─────────────────────────────────────────────────────
    ("Library", "图库 · Library"),
    ("Open folder…", "打开文件夹…"),
    ("{dir} · {count} photos", "{dir} · {count} 张照片"),
    ("⎘ Copy recipe", "⎘ 复制配方"),
    ("Copy every develop setting from the current photo", "复制当前照片的全部 develop 参数"),
    ("Recipe copied — Ctrl/⌘+click to pick several, then “Paste to selected”", "配方已复制 — Ctrl/⌘+点击选多张，再「粘贴到选中」"),
    ("⇩ Paste to selected ({n})", "⇩ 粘贴到选中({n})"),
    ("Writes each photo's develop into your develop store (recipe JSON; RAW also gets a Lightroom XMP). Leaves library files untouched, renders nothing.",
        "把每张照片的显影写入显影库（配方 JSON；RAW 另附 Lightroom XMP）。不动库文件、不渲染成品。"),
    ("🖼 Render selected ({n})", "🖼 渲染选中({n})"),
    ("Each renders by its own saved develop from the store (neutral develop if none) → <Destination>/<name>.developed.*, using the current format / long-edge / sharpening / quality; AI Denoise sits out the batch.",
        "每张按它在显影库里保存的显影出图（没有则中性显影）→ <目标位置>/<名>.developed.*，用当前格式/长边/锐化/质量；AI Denoise 不参与批量"),
    ("Clear selection", "清除多选"),
    ("Include crop / straighten when pasting", "粘贴时含裁剪/拉直"),
    ("Off by default — composition rarely transfers between photos", "默认不带几何 — 构图在照片间通常不可复用"),
    ("Open a folder to browse your photos here.", "打开一个文件夹，在此浏览照片。"),
    // Round-12 阶段5 empty-state ladder: empty folder ≠ no folder, and the
    // controls panel / first develop say what happens next instead of
    // dead-ending in a bare label.
    ("No photos in this folder — RAW / JPEG / PNG / TIFF would show up here.",
        "此文件夹里没有可显示的照片——RAW / JPEG / PNG / TIFF 都会出现在这里。"),
    ("Click a thumbnail in the Library, or press Ctrl+O.",
        "点击图库缩略图，或按 Ctrl+O 打开照片。"),
    ("Preparing preview…", "正在准备预览…"),
    ("No masks yet — draw one with the tools above; AI Analyze adds its own too.",
        "还没有蒙版——用上面的工具画一个；AI 分析也会自动添加。"),
    ("✓ selected", "✓ 选中"),
    ("● edited", "● 已编辑"),

    // ── Retouch (reimagine / fill / heal / clone) ─────────────────────────────
    ("Retouch", "修饰 · Retouch"),
    ("Reimagine (whole image)", "整图 AI 生成 · Reimagine"),
    ("✨ Generate image", "✨ AI 生成出片"),
    ("Repaint the whole image with gpt-image, styled by the prompt on the left (empty = a neutral finished develop). Repainted pixels = not faithful; the result is added as an 「AI generated」 variant at the bottom and switched to, so you can keep tweaking without reverting. Models that accept any size (gpt-image-2) reach ~8MP, others ~1.5K. Needs an image API (OPENAI_API_KEY, or the OAuth image bridge in Settings).",
        "用 gpt-image 直接重绘整张图（风格取左侧提示词；留空=中性成片方向）。重绘像素=非保真；生成后自动加入底部「AI 生成」变体并切过去，可继续微调不会变回去。支持任意尺寸的模型（gpt-image-2）可达 ~8MP，其余 ~1.5K。需图像 API（OPENAI_API_KEY，或设置里的 OAuth 图像桥）。"),
    ("style to repaint toward — e.g. golden-hour glow, moody film look",
        "想重绘成的风格——如「金色黄昏氛围」「胶片低饱和」"),
    // R23-6 B: the reverse-fit target is no longer only an app-generated
    // variant, so the empty-state line names BOTH entries.
    ("Pick a reference below, or generate an image and stay on that variant, to reverse-fit a recipe.",
        "在下面选一张参考图，或者「AI 生成出片」并停在该变体上，才能反推配方。"),
    ("🖼 Choose reference…", "🖼 选择参考图…"),
    ("Reverse-fit toward ANY finished version of THIS SAME photo — your own \
      Lightroom/Capture One export, the camera's JPEG, a TIFF, or another RAW \
      (developed neutrally first). The fit solves the develop parameters that \
      reproduce that file's look and leaves your pixels untouched. It must be \
      the same frame: a different picture is warned about, not refused, and \
      its result means nothing.",
        "把本图的任意一张成品当作反推目标——你自己在 Lightroom / Capture One 导出的成片、相机直出 JPEG、TIFF，或另一个 RAW（会先中性显影）。反推求解的是能重现该文件观感的显影参数，不动你的像素。它只能是同一张照片：换成别的画面只会得到警告而不会被拒绝，而那样的结果没有意义。"),
    ("Forget this reference and go back to reverse-fitting the active generated variant",
        "清除这张参考图，回到反推当前 AI 生成变体"),
    ("deep", "深度"),
    ("DEEP REVERSE-FIT: run the review BEFORE saving and let it buy one \
      guided retry — the reviewer's suggestion picks the next ACTION \
      (add the zoned pass, pull the chroma chase back), never the \
      numbers, and the retry is kept only if it re-scores at least as \
      high. COST: up to two paid vision calls instead of one, and the \
      save waits for them; there is NO cancel, exactly as for the \
      review itself. Off = the reviewed fit is saved first and the \
      score is a note (the behaviour of every release since v0.26.0).",
        "深度反推：在保存之前先做复查，并让它换来一次按提示的重试——复查给出的提示只用来挑选下一个动作（加上分区处理、把饱和度拉回来），从不直接写参数；重试只有在复查分数不低于原分时才会被采用。开销：最多两次收费视觉调用而不是一次，保存也要等它们返回；与复查本身一样，没有取消。关闭 = 先保存复查过的反推结果，分数只作为一条注记（v0.26.0 以来每个版本的行为）。"),
    ("Turn on 「AI review」 first — the deep fit is that review, iterated",
        "请先勾选「AI 打分」——深度反推就是把这次复查反复做几轮"),
    ("🎛 Reverse-fit recipe → sliders/XMP", "🎛 反推配方 → 滑杆/XMP"),
    ("Statistical fit: reverse the freshly generated look into editable develop params (local, no API cost). Sliders update (undoable), and for RAW a Lightroom XMP goes into this photo's develop store; hit Export to render the full-resolution result. Uses the panel's Strength control as the reverse-fit honesty budget.",
        "统计拟合：把刚生成的观感反解成可编辑的 develop 参数（本地运算，无 API 费）。滑杆会更新（可 undo），RAW 会在该照片的显影库里生成 Lightroom XMP；再点「导出」可出全分辨率成品。使用面板的 Strength 控制作为反推预算。"),
    ("📝 Extract style prompt", "📝 提取风格提示词"),
    ("Compare the original / generated images and have the vision model write a reusable style prompt: auto-fills the Reimagine prompt (ready to restyle other photos) and saves ./out/<stem>.style.txt.",
        "对比 原图/生成图，让 vision 模型写一段可复用的风格 prompt：自动填入 Reimagine 提示词（可直接给别的照片重绘用）并存 ./out/<stem>.style.txt。"),
    ("AI review", "AI 打分"),
    // R23-6 D: the hint is no longer discarded, so the tooltip says so.
    ("After the fit, show the target and the fitted render to the vision model and have it SCORE the match (0-100) with a short critique — LLM as a judge. One paid vision call per fit (needs the image API key); the fit itself stays local and free. The score, its critique AND its suggestion land in the status line below — nothing is changed for you. No cancel: like the fit itself, the app stays busy until the review returns.",
        "反推后把 目标图/拟合渲染 交给 vision 模型打分：0-100 匹配度 + 说明（LLM as a judge）。每次反推一次 vision API 收费（需图像 API 密钥）；反推本身仍为本地免费。得分、说明和它给出的建议都显示在下方状态行——不会替你改动任何参数。无法取消：与反推本身一样，打分返回前 app 保持忙碌。"),
    ("After generating, use 「Reverse-fit recipe」 to turn the look into sliders + XMP (the full-resolution way).",
        "生成后可「反推配方」把观感变成滑杆+XMP（全分辨率的正道）。"),
    ("Paint mask", "涂抹蒙版"),
    ("Brush over the area; box-select is paused while on. Shared by Fill and Heal.",
        "在区域上涂抹；开启时框选暂停。Fill 与 Heal 共用。"),
    ("Generative Fill", "生成填充 · Generative Fill"),
    ("what belongs there, e.g. remove the trash can, extend the sky",
        "那里该有什么，例如：移除垃圾桶、延展天空"),
    // R22 #16: one 「Full-res」 key served FOUR checkboxes in three panels (fill
    // here, heal + clone below, denoise in Develop · Detail) with three different
    // gates — RAW-only for fill, both source types for the rest. Each label now
    // names its verb, so a single retired key becomes four.
    ("Full-res fill", "全分辨率填充"),
    ("Composite onto the full-sensor develop (slow, RAW only)", "合成到全分辨率显影上（慢，仅 RAW）"),
    // L09#4: heal honours --full-res on baked sources too (since b4c6c30);
    // "RAW only" was fill's semantics, copied and never re-synced. The new
    // text names the omission consequence the old one hid.
    ("Full-res heal", "全分辨率修复"),
    ("Heal at full resolution (slow; without it a baked image is saved at 2048px)",
        "在全分辨率上修复（慢；不勾选时烘焙图会按 2048px 保存为母版）"),
    ("gpt-image render quality — higher looks better and costs more per image",
        "gpt-image 出图质量 — 越高画质越好、单张费用也越高"),
    ("high", "高"),
    ("medium", "中"),
    ("low", "低"),
    ("Remove / Fill", "移除 / 填充"),
    ("Paint the area, write what belongs there, then Remove/Fill. Needs an image API (OPENAI_API_KEY, or the OAuth image bridge in Settings).",
        "涂抹区域，写下那里该有什么，再点 Remove/Fill。需图像 API（OPENAI_API_KEY，或设置里的 OAuth 图像桥）。"),
    ("Heal (pixel)", "去瑕疵 · Heal（像素）"),
    ("🤖 AI heal (auto)", "🤖 AI 去瑕疵 (auto)"),
    ("Heal painted area", "修复涂抹区域"),
    ("AI auto-detects dust / blemishes, or paint a mask and Heal it. Pixel retouch from surrounding pixels; saved to ./out.",
        "AI 自动识别灰尘/瑕疵，或涂抹蒙版后修复。按周围像素做像素级修饰；存 ./out。"),
    ("Clone Stamp", "仿制图章 · Clone Stamp"),
    // ✓ (geometric) since 阶段5 — one finish-glyph family with 「✓ Apply」.
    ("✓ Done", "✓ 完成"),
    ("Stamp: Alt+click to set the source → brush the target area → 「⎘ Clone painted area」",
        "图章：Alt+点击取源点 → 画笔涂目标区 → 「⎘ 克隆已涂区域」"),
    ("⎘ Clone painted area", "⎘ 克隆已涂区域"),
    ("Full-res clone", "全分辨率克隆图章"),
    ("Clone at full resolution (slow; without it a baked image is saved at 2048px)",
        "全分辨率克隆（慢；不开启时烘焙图像按 2048px 保存）"),
    ("Photoshop-style clone stamp: Alt+click to sample a source (cross marker), brush the area to cover, and pixels are carried over as-is from the source (feathered edges, no tone matching). Local compute, saves a ./out pixel master.",
        "Photoshop 的仿制图章：Alt+点击取源（十字标记），画笔涂要覆盖的区域，按源点原样搬运像素（羽化边缘、不做色调匹配）。本地运算，存 ./out 像素母版。"),

    // ── Develop · shared slider helper ───────────────────────────────────────
    // NOT 归零: five sliders reset to a non-zero default (Temp 5500, Blending/
    // Midpoint 50, Lum. high 1.0, Feather 0.1) — matches 重置为默认值 in F1.
    ("double-click / right-click resets · hover + ↑/↓ nudges (Shift ×10)",
        "双击 / 右键恢复默认值 · 悬停按 ↑/↓ 微调（Shift ×10）"),

    // ── Develop · panel + Tone & WB ──────────────────────────────────────────
    ("Develop", "显影 · Develop"),
    ("Tone & WB", "色调 · Tone & WB"),
    ("Custom white balance (off = as-shot)", "自定义白平衡（关=按拍摄值）"),
    ("as shot ≈ {k} K · tint {t}", "拍摄时 ≈ {k} K · 色调 {t}"),
    ("{n} XMP numeric setting(s) unreadable ({list}) — restored as neutral; saving would overwrite the sidecar with those neutrals",
     "XMP 有 {n} 个数值设置无法读取（{list}）——已按中性恢复；保存会用这些中性值覆盖 sidecar"),
    ("a Lightroom sidecar sits beside this photo but could not be read ({why}) — any Lightroom edits in it are NOT reflected",
     "照片旁有一个 Lightroom sidecar 但无法读取（{why}）——其中的 Lightroom 编辑（如有）未生效"),
    ("this RAW carries an embedded XMP develop that could not be read ({why}) — it is NOT reflected",
     "此 RAW 内嵌的 XMP 显影无法读取（{why}）——未生效"),
    ("{n} unreadable item(s) skipped during the folder scan",
     "扫描文件夹时跳过了 {n} 个不可读条目"),
    ("bitmap raster unreadable ({list}) — this mask currently has NO effect",
     "位图蒙版栅格无法读取（{list}）——该蒙版当前无效"),
    ("💧 Click in image…", "💧 点击图中…"),
    ("💧 Eyedropper", "💧 吸管"),
    ("Click a spot in the image that should be neutral grey/white to auto-solve Temp/Tint (same forward model as the engine). Click again to cancel.",
        "点击图中应为中性灰/白的位置，自动解算色温/色调（与引擎同一正向模型）。再次点击取消。"),
    ("WB eyedropper: click a spot that should be neutral grey/white", "白平衡吸管：点击应为中性灰/白的区域"),
    ("Temp (K)", "色温 (K)"),
    ("Tint", "色调"),
    ("Exposure", "曝光"),
    ("Contrast", "对比度"),
    ("Highlights", "高光"),
    ("Shadows", "阴影"),
    ("Whites", "白色"),
    ("Blacks", "黑色"),

    // ── Develop · Presence / Detail ──────────────────────────────────────────
    ("Curves", "曲线 · Curves"),
    ("Presence", "质感 · Presence"),
    ("Clarity", "清晰度"),
    ("Dehaze", "去朦胧"),
    ("Vibrance", "自然饱和度"),
    ("Saturation", "饱和度"),
    ("Detail", "细节 · Detail"),
    ("Sharpening", "锐化"),
    ("Noise Reduction", "降噪"),
    // R25 B3 — the eight CarriedOnly detail axes. Qualified labels for the
    // same reason the Effects block below states: the bare 「Detail」 is
    // already the section title, and 「Amount」 is already mask strength.
    ("Sharpen radius", "锐化半径"),
    ("Sharpen detail", "锐化细节"),
    // 「边缘保护」 would be the natural gloss, but 护 (U+62A4) is outside the
    // embedded font subset — and the Chinese UI must not depend on the
    // machine having a CJK font.
    ("Sharpen masking", "边缘蒙版"),
    ("Noise detail", "降噪细节"),
    ("Noise contrast", "降噪对比"),
    ("Colour noise reduction", "彩色降噪"),
    ("Colour noise detail", "彩噪细节"),
    ("Colour noise smoothness", "彩噪平滑度"),
    // R23-1b: the two new PER-MASK sliders. Distinct labels from the global
    // 「锐化」/「色相」 on purpose — these are the local, signed/rotating forms,
    // and the tooltips carry the difference.
    ("Sharpness", "局部锐化"),
    ("Sharpens inside the mask when positive and SOFTENS when negative (the global 「Sharpening」 has no negative half). Same radius as the global one.",
        "正值在蒙版内锐化，负值柔化（全局「锐化」没有负值一半）。半径与全局一致。"),
    ("Hue shift", "色相旋转"),
    ("Rotates every colour inside the mask (±100 = ±30°) — unlike the global color mixer, which moves one color band across the whole frame.",
        "旋转蒙版内的所有颜色（±100 = ±30°）——不同于全局颜色混合器：那个是把某一色段在整幅画面上移动。"),

    // ── Develop · Color Mixer (HSL) + Grading ────────────────────────────────
    ("Color Mixer (HSL)", "颜色混合器 · HSL"),
    ("↺ reset all", "↺ 全部重置"),
    ("Hue", "色相"),
    ("Luminance", "明度"),
    ("Color Grading", "颜色分级 · Grading"),
    ("Blending", "混合"),
    ("Balance", "平衡"),
    // HSL_BANDS labels (Color Mixer band picker).
    ("Red", "红"),
    ("Orange", "橙"),
    ("Yellow", "黄"),
    ("Green", "绿"),
    ("Aqua", "青"),
    ("Blue", "蓝"),
    ("Purple", "紫"),
    ("Magenta", "洋红"),

    // ── Develop · Crop + Lens ────────────────────────────────────────────────
    ("Crop", "裁剪 · Crop"),
    ("⛶ Enter crop", "⛶ 进入裁剪"),
    ("Straighten (°)", "拉直 (°)"),
    ("Once in (R): drag corner/edge handles to resize, drag inside to move, drag OUTSIDE the box (or the canvas border while the box is full-frame) to rotate-straighten; arrows nudge the box, Enter commits; preview, export and XMP all match. Straighten auto-crops the black corners.",
        "进入后（R）：拖角/边把手调整大小、框内拖动移动、框外（满幅时沿画布边缘）拖动旋转拉直；方向键微移裁剪框，Enter 提交；预览、导出与 XMP 三者一致。拉直会自动裁掉黑边。"),
    ("Lens", "镜头 · Lens"),
    ("Profile corrections (from camera metadata)", "机内镜头校正（相机元数据）"),
    ("Vignetting", "暗角校正"),
    ("Chromatic aberration", "色差校正"),
    ("The camera's own falloff map for this shot, applied in linear light",
        "相机为这张照片记录的暗角衰减曲线，按线性光增益应用"),
    ("The camera's own geometric correction; masks and crop follow the corrected frame",
        "相机自带的几何畸变校正；蒙版与裁剪跟随校正后的画面"),
    ("Per-channel radius correction: removes red/blue colour fringing at edges",
        "分通道半径校正：去除边缘的红/蓝色边"),
    ("No in-camera lens correction data in this file", "此文件不含机内镜头校正数据"),
    // R25 B2: was ("Vignette", "暗角"). The Effects section now carries
    // Lightroom's POST-CROP vignette, so the bare word named two different
    // operators in two sections — a collision, renamed at the source rather
    // than disambiguated in one place and left ambiguous in the other.
    ("Lens vignetting", "镜头暗角"),
    ("Midpoint", "中点"),
    ("Distortion", "畸变"),
    // R25 B3 — the manual CA pair (RENDERED here) and the carried half of the
    // same panel. 「自动横向色差」 would be the literal gloss, but 横 (U+6A2A)
    // is outside the embedded font subset.
    ("Chromatic aberration (manual)", "手动色差"),
    ("Red / cyan", "红 / 青"),
    ("Blue / yellow", "蓝 / 黄"),
    ("Auto lateral CA", "自动色差校正"),
    ("Defringe", "去边"),
    // The six de-fringe controls. 「紫 · …」/「绿 · …」 keeps Adobe's two
    // colour groups visible in one column without a caption per group, and
    // the bare 「强度」 stays reserved for mask strength.
    ("Purple amount", "紫 · 强度"),
    ("Purple hue low", "紫 · 色相下限"),
    ("Purple hue high", "紫 · 色相上限"),
    ("Green amount", "绿 · 强度"),
    ("Green hue low", "绿 · 色相下限"),
    ("Green hue high", "绿 · 色相上限"),
    // R25 B3: the tail of this line used to promise 「De-fringe in a later
    // batch」 / 「去紫边留待后续批次」. The batch is this one — de-fringe now
    // round-trips, so the sentence states what it actually does instead of
    // when it might arrive.
    ("Vignette: positive brightens the corners (compensates falloff), negative darkens; a radial gain in linear light. Distortion: positive fixes barrel (wide-angle bulge), negative fixes pincushion (tele pinch); auto-scales to fill the frame, and masks / brush still position on the corrected image. Preview / export / XMP match. Manual CA renders here too; the auto-CA switch and de-fringe are carried to Lightroom without being rendered.",
        "暗角：正值提亮四角（补偿衰减），负值压暗；在线性光下的径向增益。畸变：正值修桶形（广角外凸），负值修枕形（长焦内缩）；自动缩放填满画幅，蒙版/画笔仍按校正后的图像定位。预览/导出/XMP 一致。手动色差本机也渲染；自动色差校正与去边仅随 XMP 带出，本机不渲染。"),
    // CROP_ASPECTS display names (ratio values are not localized).
    ("Free", "自由"),
    ("Original", "原始"),

    // ── Develop · Effects (R25 B2 — the nine CarriedOnly globals) ────────────
    // Every label is QUALIFIED on purpose: the bare 「Amount」/「Style」/
    // 「Detail」 keys already exist (mask strength, the AI style library, the
    // Detail section title) and reusing one would translate two unrelated
    // controls with one word. 「胶片噪点」/「噪点密度」 rather than 颗粒/粗糙:
    // those four hanzi are outside the embedded font subset, and the Chinese
    // UI must not depend on the machine having a CJK font.
    ("Effects", "效果"),
    ("Post-crop vignetting", "裁剪后暗角"),
    ("Vignette amount", "暗角数量"),
    ("Vignette roundness", "暗角圆度"),
    ("Vignette feather", "暗角羽化"),
    ("Vignette highlights", "暗角高光"),
    ("Vignette style", "暗角样式"),
    ("Grain", "胶片噪点"),
    ("Grain amount", "噪点数量"),
    ("Grain size", "噪点大小"),
    ("Grain roughness", "噪点密度"),
    ("Carried to Lightroom, not rendered here", "本机不渲染，仅随 XMP 带出"),

    // ── Develop · Transform + Calibration (R25 B4 — Tier::PassThrough) ───────
    // A READ-ONLY section: these sixteen crs properties are carried between
    // the sidecar and recipe.json verbatim and never interpreted, so there is
    // nothing here to offer a slider for. The two group captions are
    // QUALIFIED («Perspective correction» / «Camera calibration») while the
    // bare 「Transform」/「Calibration」 name the blocks in the save-time
    // render-gap line — the same word doing the same job in both places.
    ("Transform", "变换"),
    ("Calibration", "校准"),
    ("Perspective correction", "透视校正"),
    ("Camera calibration", "相机校准"),
    // 「相机」, not 「镜头」: this is `crs:CameraProfile`, the colour profile
    // the photographer picked in Lightroom's own profile browser. The
    // 「镜头配置文件」 entry further down is a different thing entirely — the
    // LENS correction profile the engine applies from camera metadata — and
    // the two had the same Chinese name, which pointed the user at the wrong
    // panel to go looking for either.
    ("Camera profile", "相机配置文件"),
    ("Carried through to the sidecar unchanged; AutoShade never interprets these",
        "原样带出到边车文件；AutoShade 从不解读这些值"),

    // ── Develop · Local Masks (add + AI segmentation) ────────────────────────
    ("Local Masks ({n})", "局部蒙版 ({n})"),
    ("＋ Linear gradient", "＋ 线性渐变"),
    ("Drag on the image: start = fully-applied side, end = unaffected side (Shift = horizontal/vertical)",
        "在图上拖拽：起点=完全应用侧，终点=不受影响侧（Shift = 水平/垂直锁定）"),
    ("Drag on the image to draw a linear gradient (start fully applied → end unaffected; Shift = axis lock)",
        "在图上拖拽画线性渐变（起点完全应用 → 终点不受影响；Shift = 锁轴）"),
    ("Drag on the image to draw an elliptical area", "在图上拖拽画一个椭圆区域"),
    ("Drag on the image to draw a radial (elliptical) area", "在图上拖拽画径向（椭圆）区域"),
    ("Drag to reorder", "拖动重新排序"),
    ("🤖 AI select subject", "🤖 AI 选主体"),
    // R29 B4: the subject backend became BiRefNet (user ruling, 2026-08-21).
    // Three facts the photographer acts on BEFORE clicking, so all three ride
    // in the tooltip: the new dependencies, the 444 MB first run, and the
    // fallback — a machine without torchvision still gets a mask, just U²-Net's
    // softer one, and 「it worked, but which model drew it」 is not a question
    // the product may leave unanswered.
    ("BiRefNet salient-subject segmentation → bitmap mask (python sidecar: pip install torchvision timm einops; first run auto-downloads a ~444MB model; without them it falls back to U²-Net / pip install rembg, whose edges are softer)",
        "BiRefNet 显著主体分割 → 位图蒙版（python sidecar：pip install torchvision timm einops；首次运行自动下载约 444MB 模型；未安装这些依赖时回退到 U²-Net / pip install rembg，边缘更模糊）"),
    ("🤖 AI select sky", "🤖 AI 选天空"),
    // R27 Batch-4: the model NAME and its download size both changed with the
    // licence fix (SegFormer-B0's weights are "research or evaluation purposes
    // only"; OneFormer Swin-L is MIT). 14 MB -> 880 MB is a number the user
    // acts on before clicking, so the tooltip has to carry the real one.
    ("OneFormer-ADE20K sky segmentation → bitmap mask (python sidecar: pip install transformers; first run auto-downloads a ~880MB model)",
        "OneFormer-ADE20K 天空分割 → 位图蒙版（python sidecar：pip install transformers；首次运行自动下载约 880MB 模型）"),
    ("this build did not ship the python sidecar — run AutoShade from the project directory, or point AUTOSHADE_SEGMENT_SCRIPT at python/segment.py",
        "本版本未随发布包分发 python 边车——请从项目目录运行 AutoShade，或用 AUTOSHADE_SEGMENT_SCRIPT 指向 python/segment.py"),

    // ── Develop · selected-mask controls ─────────────────────────────────────
    ("Name", "名称"),
    ("↻ Redraw", "↻ 重画"),
    ("Re-drag this mask's area on the image", "在图上重新拖拽这个蒙版的范围"),
    // R22 #16: was 「Overlay」/「叠加」, sitting between 「↻ Redraw」 and the ⬆/⬇
    // order buttons — a row of per-mask verbs, so the label read as per-mask
    // state. The flag is the O key's ONE view switch (canvas.rs
    // refresh_mask_overlay draws whichever mask is hovered-or-selected), and the
    // EN label now matches the F1 sheet's wording for that key.
    ("Show mask overlay", "显示蒙版叠加"),
    ("One view switch shared by every mask (shortcut O): shows the hovered-or-selected mask's actual coverage as a red semi-transparent overlay (geometry × range × strength)",
        "所有蒙版共用的一个显示开关（快捷键 O）：用红色半透明显示当前悬停或选中蒙版的实际作用范围（几何×范围×强度）"),
    ("Move up (renders earlier)", "上移（更早渲染）"),
    ("Move down (renders later)", "下移（更晚渲染）"),
    ("Invert", "反转"),
    ("Edge feather", "边缘羽化"),
    ("Flip", "内外翻转"),
    ("Swap which side of the ellipse the adjustment affects (composes with Invert)",
        "对调椭圆内外的作用侧（与反转 Invert 叠加生效）"),
    ("Range mask", "范围蒙版"),
    ("Luminance range", "亮度范围"),
    ("Color range", "颜色范围"),
    ("Ordered bounds", "有序边界"),
    ("None", "无"),
    ("Color", "颜色"),
    ("Color range: click the color to pick in the image", "颜色范围：点击图中要选取的颜色"),
    ("Lum. low", "亮度下限 Lo"),
    ("Lum. high", "亮度上限 Hi"),
    ("Feather", "羽化 Feather"),
    ("🎯 Click in image…", "🎯 点击图中…"),
    ("🎯 Sample", "🎯 取样"),
    ("Click the color to pick in the image (the same color at other brightnesses is also selected; clicking this button again cancels sampling)",
        "在图上点击要选取的颜色（亮暗不同的同色也会被选中；再点一次此按钮取消取样）"),
    ("Tolerance", "容差 Tolerance"),
    ("Amount", "强度"),
    ("Texture", "纹理"),
    ("Lightroom-style local adjustments: add a gradient to darken the sky, a radial to brighten the subject. AI Analyze also writes to this list.",
        "像 Lightroom 的局部调整：加一个渐变压暗天空、径向提亮主体。AI Analyze 也会写到同一列表。"),
    // R22-5 (#10): the selected mask's sliders in Lightroom's three groups.
    // 「Detail」/「Color」 reuse the panel-section entries above.
    ("Tone", "明暗"),
    // R25 P6: the fourth group under those three — the mask's own point
    // curves. SINGULAR, and deliberately not the 「Curves」 of the global
    // section above (「曲线 · Curves」): two captions reading the same word in
    // one scrolled panel is how a user edits the wrong curve.
    ("Curve", "曲线"),
    ("Select a mask above to edit its adjustments", "选中上面任一蒙版即可编辑它的调整"),
    ("A RELATIVE warm/cool shift (±100) around a fixed 5500 K anchor, not absolute Kelvin: this value renders like ≈ {k} K. The global 「Temp (K)」 is absolute and anchored at this photo's as-shot value — a different axis.",
        "相对冷暖偏移（±100），以固定 5500 K 为基准，不是绝对色温：当前值渲染起来约等于 {k} K。全局「色温 (K)」是绝对值、以本张照片的拍摄色温为基准——两者不是同一条轴。"),
    ("A RELATIVE green/magenta shift (±100) inside the mask — positive goes magenta. Unlike the global 「Tint」 it is not solved against the photo's as-shot tint.",
        "蒙版内的相对绿/洋红偏移（±100），正值偏洋红。它不像全局「色调」那样按拍摄色调解算。"),
    ("carries reverse-fit recolour (not exported to XMP)", "含反推重上色（不写入 XMP）"),
    ("↺ Clear", "↺ 清除"),
    ("Drop this mask's per-channel recolour gains (one Ctrl+Z to undo)",
        "清除这个蒙版的分通道重上色增益（一步 Ctrl+Z 可撤销）"),

    // ── Develop · Versions ───────────────────────────────────────────────────
    ("Versions ({n})", "版本 · Versions ({n})"),
    ("＋ Save as version", "＋ 存为版本"),
    ("Snapshot this card's develop only as a numbered version (v<N>.recipe.json with frozen mask rasters); Ctrl+S saves every card",
        "仅把当前变体的显影保存为编号版本（v<N>.recipe.json，并冻结蒙版栅格）；Ctrl+S 会保存所有变体"),
    ("Load", "载入"),
    ("Replace current parameters (one Ctrl+Z to undo)", "替换当前参数（一步 Ctrl+Z 可撤销）"),
    ("Like LR virtual copies: store multiple parameter sets for one photo (B&W, cropped…) without overwriting.",
        "像 LR 虚拟副本：一张照片存多套参数（黑白版/裁剪版…），互不覆盖。"),
    // R24-2: version names + provenance (.version-meta.json)
    ("Name…", "起名…"),
    ("Name this snapshot", "给这个快照起个名字"),
    ("· from {kind}", "· 来自 {kind}"),
    ("· auto-archived", "· 自动存档"),
    ("Only this variant", "仅当前变体"),
    ("Show only snapshots taken from the variant you are on. Versions with no recorded source (saved before this) are hidden while it is on.",
        "只列出从当前变体存下的快照。没有记录来源的版本（早于此功能）会被隐藏。"),
    ("{n} hidden — saved from another variant", "已隐藏 {n} 个（来自其他变体）"),
    ("A generated variant's look lives in its pixels — a version snapshot would store an almost-empty recipe; run 「Reverse-fit」 first",
        "AI 生成变体的效果在像素里——存版本只会存下一份基本空白的参数；请先跑「反推」"),
    ("Renaming v{n} failed: {err}", "v{n} 改名失败：{err}"),
    // R24-3: loading a snapshot TAKEN on a generated card onto a parametric
    // canvas — the snapshot carries no camera base look, so one is stamped.
    ("this snapshot was taken on a generated variant, whose look lives in its pixels — the photo's own camera base look was applied so it renders on the negative",
        "这个快照存自 AI 生成变体，效果在像素里——已补上本照片自己的相机基础色调，让它在底片上正常渲染"),
    // R24-4: the edit-state list (variant cards above the version rows).
    ("Variants (cards) ({n})", "变体（卡片） · Variants (cards) ({n})"),
    ("Snapshot history", "快照历史 · Snapshot history"),
    ("· current", "· 当前"),
    ("· pixel-state (no XMP)", "· 像素态（不出 XMP）"),
    ("· on baked pixels", "· 基于烘焙像素"),

    // ── Develop · export bar sliders (in update()) ───────────────────────────
    ("Output sharpening", "输出锐化"),
    ("JPEG quality", "JPEG 质量"),

    // ── Develop · tone curve (curve_editor) ──────────────────────────────────
    ("Master", "主"),
    ("Clear the current channel's curve", "清空当前通道曲线"),

    // ── Develop · histogram + clipping triangles (histogram_ui) ──────────────
    ("shadow crush", "阴影死黑"),
    ("highlight clip", "高光溢出"),
    ("{what}: {chan} channel(s) — click to toggle clipping warning (J)",
        "{what}：{chan} 通道 — 点击切换削波警告 (J)"),
    ("{what} indicator (clean) — click to toggle clipping warning (J)",
        "{what}指示（干净）— 点击切换削波警告 (J)"),

    // ── Canvas · mode hints + zoom / clip / preview-edge (after_view) ────────
    ("Before (source) — release B to return to editing", "Before (source) — 松开 B 回到编辑"),
    ("Before (source) — press \\ again (or Esc) to return to editing",
        "Before (source) — 再按一次 \\（或 Esc）回到编辑"),
    ("After — drag a box = local AI · scroll to zoom · space/middle-drag to pan · hold B to compare",
        "After — 拖框=局部AI · 滚轮缩放 · 空格/中键平移 · 按住B对比"),
    ("Preview pixels 1:1 (double-click the image to toggle; key: 1)", "预览像素 1:1（双击图片可切换；快捷键 1）"),
    ("Fit the whole image to the canvas (double-click the image to toggle; key: 0)", "整图适配画布（双击图片可切换；快捷键 0）"),
    ("Fit", "适配"),
    ("Fit ↔ 1:1", "适配 ↔ 1:1"),
    ("Clipping warning (J): red = highlight clip, blue = shadow crush (judged on export pixels)",
        "削波警告 (J)：红 = 高光溢出，蓝 = 阴影死黑（按导出像素判定）"),
    ("1280px · fluid", "1280px 流畅"),
    ("4096px · inspect", "4096px 检查"),
    ("Working preview resolution: 1280 is smoothest on the sliders; 2560/4096 for 1:1 focus/noise checks (slower on every adjustment)",
        "工作预览分辨率：1280 滑杆最流畅；2560/4096 供 1:1 查合焦/噪点（每次调整更慢）"),

    // ── Develop · variant strip (variant_strip) ──────────────────────────────
    // Variants = 变体 (independent renditions), Versions = 版本 (parameter
    // snapshots). One ZH word for both made "删除此版本" ambiguous between
    // two different destructive actions.
    ("Variants", "变体"),
    ("Click to switch to this variant (lossless)", "点击切到此变体（无损）"),
    ("Click to switch to this variant (lossless); Ctrl+S saves every card for this photo",
        "点击切换到此变体（无损）；Ctrl+S 会保存此照片的所有变体"),
    ("Delete this variant", "删除此变体"),
    // R24-3: the card's own name (the strip is where the cards live, so the
    // rename box lives there too) — 「Name」/「Name…」 reuse the mask panel's.
    ("Name this variant", "给这个变体起个名字"),
    // R24-3 (#7) 「apply to Original」. The two concepts stay apart: this
    // overwrites the ▣ Original CARD's parameters, and Ctrl+S is what makes
    // that card's develop the photo's saved develop.
    ("Copy this variant's develop onto the ▣ Original card — its baked pixels and this card both stay. One Ctrl+Z undoes it; Ctrl+S then saves it as this photo's develop",
        "把这个变体的显影参数复制到「▣ 原片」变体——它的烘焙像素和本变体都保留。一步 Ctrl+Z 可撤销；再按 Ctrl+S 才存为本照片的显影"),
    ("A generated variant's look lives in its pixels — there are no develop parameters to copy onto the ▣ Original card; run 「Reverse-fit」 first",
        "AI 生成变体的效果在像素里——没有可复制到「▣ 原片」变体的显影参数；请先跑「反推」"),
    ("this photo's strip holds no ▣ Original card to apply onto",
        "这张照片的变体里没有「▣ 原片」可以覆盖"),
    ("「{name}」 copied onto the ▣ Original card (its pixels are untouched) — Ctrl+Z undoes it; Ctrl+S then saves it as this photo's develop",
        "「{name}」已复制到「▣ 原片」变体（像素未动）——Ctrl+Z 可撤销；再按 Ctrl+S 才存为本照片的显影"),
    // R24-4: the strip's ✕ arms before it fires — a deleted card cannot be
    // brought back, which is exactly what the version registry does NOT say
    // about a deleted number.
    ("Delete variant「{name}」? Click ✕ again to confirm — a deleted variant cannot be brought back (Ctrl+Z does not cross variants)",
        "删除变体「{name}」？再点一次 ✕ 才会删除——删掉的变体无法找回（Ctrl+Z 只在同一变体内有效）"),
    ("Click again to delete this variant — it cannot be brought back (Ctrl+Z does not cross variants)",
        "再点一次就删除此变体——删掉无法找回（Ctrl+Z 只在同一变体内有效）"),

    // ── Toolbar · top row (update()) ─────────────────────────────────────────
    ("Batch {done}/{total}", "批量 {done}/{total}"),
    ("Open photo…", "打开照片…"),
    ("Ctrl+O · or drag a file into the window", "Ctrl+O · 或直接拖拽进窗口"),
    ("AI Refine", "AI 微调"),
    ("Adjust the CURRENT edit instead of proposing from scratch — your sliders are the starting point (enabled once the edit is non-neutral).",
        "在当前编辑基础上微调，而不是从零提议——你的滑杆就是起点（编辑非中性后可用）。"),
    ("Reset", "重置"),
    ("Back to this photo's fresh-open look: sliders neutral on the camera-matched base (one undo brings it back)", "回到本照片刚打开的状态：全部滑杆归中性、保留相机基调（一步撤销可回来）"),
    ("saved before the camera base look — renders as originally tuned; Reset switches to the camera-matched base", "此存档保存于相机基调功能之前——按原样渲染；Reset 可切换到相机基调"),
    ("↶ Undo", "↶ 撤销"),
    ("↷ Redo", "↷ 重做"),

    // ── toolbar · manual 90° rotation (R27) ─────────────────────────────────
    ("⭯ Turn left", "⭯ 左转"),
    ("⭮ Turn right", "⭮ 右转"),
    // The zh side deliberately says 「向左/右转 90 度」 rather than
    // 「逆/顺时针转四分之一圈」: 逆 圈 命 落 are NOT in the shipped font subset
    // (checked with scripts/subset_gui_fonts.py --check), and rewriting the
    // sentence is the standing alternative to regenerating five subsets.
    ("A quarter turn anticlockwise — the crop and every mask turn with it (one undo)",
        "向左转 90 度——裁剪与全部蒙版随之旋转（一步撤销）"),
    ("A quarter turn clockwise — the crop and every mask turn with it (one undo)",
        "向右转 90 度——裁剪与全部蒙版随之旋转（一步撤销）"),
    ("rotation is off while this photo carries baked pixels (retouch / AI rendition) or a tool is armed — its master raster is a file in the frame it was baked in; turn first, retouch after",
        "本照片带有已烘焙像素（修复 / AI 生成）或有工具处于待用状态时不可旋转——它的主图是一张按烘焙时画幅写入磁盘的文件；请先旋转，再修复"),
    ("could not turn this photo: {err} — nothing was changed",
        "无法旋转本照片：{err}——未做任何改动"),
    ("this photo is turned, and pixel repairs still work on the un-turned frame — turn it back to 0 first",
        "本照片已旋转，而像素修复仍在未旋转的画幅上工作——请先把旋转转回 0"),
    ("Style", "风格"),
    ("Personal style strength: how far AI proposals lean toward your past XMP editing habits (0 = ignore). With a Direction written above at Adherence over 40%, the direction leads instead and your habits are sent as background only — whatever this dial says.",
        "个人风格强度：AI 提案向你过往 XMP 编辑习惯靠拢的程度（0 = 不参考）。若上方写了 Direction 且 Adherence 高于 40%，则改由方向主导，你的习惯只作为参照给模型——与本滑杆的数值无关。"),

    // ── AI panel · the grade STRENGTH axis (R23-3, feedback #5) ──────────────
    ("Strength", "强度"),
    ("How hard the AI pushes the grade — a different axis from Style: Style asks how close to your own past edits, Strength asks how committed the result should be. 50% is where every AI guardrail NUMBER was calibrated: the ±50/±35 pair and the soft caps are bit-for-bit the ones earlier releases used, but the restraint WORDING those releases sent is now the 40%-and-below prose, so no single setting brings an old release back whole. From 41% up the AI must decide EACH colour control explicitly instead of leaving it neutral by default; the default 65% (double-click to reset) leans a little further than the calibration point. Above 70% it is additionally told to use the controls it wants at a strength a viewer can see, and the visual review may then run up to 3 rounds — the same ceiling 「Deep thinking」 raises it to, and either one ALONE is enough to make the worst case 17 API calls (10 carrying images). The clipping and white-point safeguards never widen with it.",
        "AI 下手有多重 —— 与「风格」是两条不同的轴：风格问「有多接近你过往的习惯」，强度问「出手多重」。50% 是所有 AI 限制参数的标定点：±50/±35 这一对与配方的两对上限，与此前各版本逐位相同；但那些版本发出的克制用词，如今属于不高于 40% 的那一档，所以没有任何单一档位能把旧版完整带回来。41% 起，AI 要对每一个颜色控件逐一表态，不能默认留白；默认 65%（双击复位到这里）比标定点再进一点。高于 70% 时还会要求它把想用的控件用到观者可见的力度，并把视觉复查上限提到 3 轮 —— 与勾选「Deep thinking」相同的上限，两者只要占其一，最多就是 17 次 API 调用（其中 10 次带图）。高光与黑位的安全边界永不随强度放宽。"),

    // ── AI panel · style reference library (R23-2, feedback #6) ──────────────
    ("⚠ no library", "⚠ 未建库"),
    ("This slider does nothing until a style reference library is built — the section just below builds one.",
        "在建好风格参考库之前，这个滑杆不起作用——下面那一节就是用来建库的。"),
    ("Style reference library", "风格参考库"),
    ("an unrecorded folder", "未记录的文件夹"),
    ("{n} finished photos · from {path} · built {age} ago", "{n} 张成片 · 来自 {path} · {age} 前建库"),
    ("Library file: {path}", "库文件：{path}"),
    ("built {hours}h ago", "{hours} 小时前建库"),
    ("built {days}d ago", "{days} 天前建库"),
    ("The style library could not be read ({err}) — rebuild it below.",
        "风格参考库读取失败（{err}）——请在下面重建。"),
    ("No library built yet — the Style slider above has nothing to lean on. Point this at the folder you edit in Lightroom (each RAW with its .xmp sidecar beside it); AutoShade keeps its own .xmp in the develop store, never beside your RAWs, so its output folder always yields nothing.",
        "还没有建库——上面的风格滑杆没有可依据的记录。请指向你在 Lightroom 里编辑的文件夹（每个 RAW 旁边都有它的 .xmp）；AutoShade 自己的 .xmp 放在显影库里、从不放在 RAW 旁边，所以它的输出目录永远一张也入不了库。"),
    ("reading the style library…", "正在读取风格参考库…"),
    ("🗂 Pick folder…", "🗂 选择文件夹…"),
    ("Choose the folder of your OWN edited RAWs — the ones with a Lightroom .xmp sidecar beside them. Each pair teaches AutoShade one of your finished looks. Indexing starts as soon as you choose.",
        "选择放着你自己编辑过的 RAW 的文件夹——也就是旁边带 Lightroom .xmp 的那些。每一对都是你的一个成片风格样本。选好后立刻开始入库。"),
    ("building…", "构建中…"),
    ("🔄 Build / rebuild", "🔄 构建 / 重建"),
    ("Index every RAW+.xmp pair in that folder (local compute, no API cost). Every RAW is decoded, so a large library takes minutes; the app stays usable and this button re-arms when it finishes. It cannot be cancelled — a build that indexes nothing is refused and leaves your existing library untouched.",
        "把该文件夹里每一对 RAW+.xmp 都入库（本地计算，无 API 费用）。每张 RAW 都要解码，所以大库要几分钟；构建期间 App 仍可用，完成后本按钮重新可用。构建无法取消——一次什么都没入库的构建会被拒绝，你原有的库保持不动。"),
    ("Pick a folder first", "请先选择文件夹"),
    ("🗂 Sidecar folder…", "🗂 选择 .xmp 文件夹…"),
    ("Where your .xmp sidecars live when they are NOT beside the RAWs — an exported catalogue, or a photo volume you cannot write to. AutoShade looks for a mirror of the library's own folder tree first, then a flat folder of sidecars, then beside the RAW as before.",
        "当你的 .xmp 不在 RAW 旁边时（比如导出到别处的目录，或者一个不能写入的照片盘），这里指向它们所在的文件夹。先按库里同名的下级目录去找，再找只有一层的那种目录，最后仍回到 RAW 旁边。"),
    ("Beside the RAWs", "放在 RAW 旁边"),
    ("Forget that folder and pair each RAW with the .xmp beside it, the way every build before this one did.",
        "不再使用那个文件夹，仍旧用每张 RAW 旁边的 .xmp 配对，和以前每一次构建一样。"),
    ("sidecars: {path}", ".xmp：{path}"),
    ("beside each RAW", "每张 RAW 旁边"),
    ("{stage}: {done} / {total} photos", "{stage}：{done} / {total} 张"),
    ("Also give the model a reference photo", "同时给模型一张参考照片"),
    ("WILL UPLOAD TWO IMAGES per analysis call: this photo, plus the ONE most similar shot from your style library, so the model can match your look by eye instead of only by numbers. COST: an analysis with two images is billed for two images instead of one — and a revision round sends both again. The reference is never stored by the provider (store:false), and the rationale names the photo that was used. Off = the numeric style reference only.",
        "打开后每次分析调用会发出两张图：本图，加上风格参考库里最接近的那一张，让模型能按图对齐你的风格，而不是只靠数字。费用：带两张图的分析按两张图计费，而不是一张；修订轮也会再发一次两张。参考图不会被提供方保存（store:false），依据里会写出所用的那一张。关闭 = 只用数字化的风格参考。"),
    ("Build a style library first — there is no reference photo to send",
        "请先建好风格参考库——现在没有参考照片可发"),
    ("Building the style library from {path} … every RAW is decoded, so a big folder takes minutes",
        "正在从 {path} 构建风格参考库…每张 RAW 都要解码，大文件夹要几分钟"),
    ("Building the style library… {stage}: {done} / {total} photos", "正在构建风格参考库…{stage}：{done} / {total} 张"),
    ("Style library built: {n} of your own edits from {path}",
        "风格参考库已建成：来自 {path} 的 {n} 条你自己的编辑"),
    ("Style library built: {n} of your own edits from {path} ({m} of them without a style embedding)",
        "风格参考库已建成：来自 {path} 的 {n} 条你自己的编辑（其中 {m} 条没有嵌入向量）"),
    ("Nothing to index in {path} — no RAW there has its .xmp sidecar beside it (AutoShade keeps its own .xmp in the develop store, never beside the RAW, so point this at the folder you edit in Lightroom). Your existing style library was left untouched.",
        "{path} 里没有可入库的照片——那里没有一个 RAW 旁边带着它的 .xmp（AutoShade 自己的 .xmp 放在显影库里、从不放在 RAW 旁边，所以请指向你在 Lightroom 里编辑的文件夹）。你原有的风格参考库保持不动。"),
    ("Building the style library failed: {err}", "风格参考库构建失败：{err}"),
    ("Before/After side by side", "原图/成片并排"),
    ("⬛ Single", "⬛ 单图"),
    ("The edit fills the canvas; hold B to quickly compare the original", "编辑图占满画布；按住 B 快速对比原图"),
    ("⚙ Settings", "⚙ 设置"),
    ("AI provider / model / API key", "AI 提供方 / 模型 / API 密钥"),
    ("Keyboard shortcuts (F1 / ?)", "快捷键速查（F1 / ?）"),

    // ── Toolbar · export bar (update()) ──────────────────────────────────────
    ("e.g. warmer and moodier, lift the shadows", "例如：更暖更有氛围，提亮阴影"),
    ("16-bit TIFF", "16 位 TIFF"),
    ("Long edge", "长边"),
    ("Original size", "原尺寸"),
    ("Colour space", "色彩空间"),
    ("sRGB (universal)", "sRGB（通用）"),
    ("Display P3 (wide-gamut screens)", "Display P3（广色域屏）"),
    ("Adobe RGB (print)", "Adobe RGB（印刷）"),
    ("AI Denoise", "AI 降噪"), // the export_summary chip, not a control label
    // R22 #16: was 「🤖 AI Denoise」 — byte-identical to the Detail section's
    // active verb once both wore the 🤖 prefix. 「on export」 / 「now」 is the real
    // difference (this one waits for a full-resolution delivery; that one bakes
    // the current variant immediately). ZH deliberately keeps this block's 降噪
    // (shared with the 「Noise Reduction」 slider and the render status line) and
    // the on-canvas block's 去噪 — the two words separate the two timings for a
    // ZH reader exactly as 「on export」/「now」 do for an EN one.
    ("🤖 AI Denoise on export", "🤖 导出时 AI 降噪"),
    // R22-7 retired 「Download…」 and its hover: the button was start_render_to
    // with a dialog-supplied path, which is now the Destination setting's
    // 「Ask every time」 value and the split button's ▾ half.
    ("Destination", "目标位置"),
    // R24-5 M8: the destination used to be the literal 「./out folder」. The
    // folder is a SETTING now (config::delivery_root), so the label names the
    // role and the resolved absolute path is echoed beside it. 「成片」 is the
    // word serve.rs's own status route already uses for this folder (成片库),
    // and every glyph is in the shipped CJK subset (投递 is not).
    ("Delivery folder", "成片文件夹"),
    // The Settings row that MOVES that folder (M8). Every glyph here was
    // checked against the shipped CJK subset before it was written — the
    // whole-chain font gate (`embedded_fonts_cover_every_ui_symbol`) is what
    // catches a 「递」 that would render as tofu.
    ("Where finished files land: exports, AI/retouch pixel masters and the extracted style prompt — for this window, the CLI, the web surface and batch renders alike. Blank = the default ./out beside the working directory. Saved develops are NOT here (see 「Develop store」 above).",
        "成品文件的去处：导出、AI/修饰像素母版、提取出的风格提示词 —— 本窗口、CLI、网页端和批量渲染共用同一个。留空 = 默认的 ./out（相对于启动目录）。已保存的显影不在这里（见上方「显影库」）。"),
    // R24 round-end LOW-3: what choosing a folder INSIDE the photo library
    // costs. 「保护」 cannot be spelled here (护 is not in the shipped CJK
    // subset, and neither are 拦 / 防 / 规), so both sentences say the thing
    // itself — the folder stops being 「只读」 — which is also the plainer
    // wording. Every glyph checked against assets/fonts before writing.
    ("Pointing it inside your photo library removes that folder's read-only protection: AutoShade refuses to write beside your originals, but never into its own delivery folder.",
        "把它指向照片库里的文件夹，就等于让那个文件夹不再「只读」：AutoShade 不会在你的原片旁边写文件，但从不拒绝写进自己的成片文件夹。"),
    ("⚠ This folder and the open photo's folder are inside one another — the photo's folder is no longer protected as read-only, so a render can land beside your originals.",
        "⚠ 这个文件夹和当前打开照片所在的文件夹互相包含 —— 那个文件夹不再「只读」，渲染出来的文件会写在你的原片旁边。"),
    ("Browse…", "浏览…"),
    ("Pick the delivery folder", "选择成片文件夹"),
    // The interpreter row (M1-3). 「解释器」 cannot be spelled in the shipped
    // CJK subset (释 is absent, and so are 探 and 系), so the heading names
    // the thing the field actually holds — a path — and the button says what
    // it does rather than 「探测」. Every glyph checked against the subset the
    // font gate enforces before these lines were written.
    ("Python interpreter", "Python 路径"),
    ("Which Python runs the AI sidecars (segmentation, denoise, style). Blank = the platform default. It can only be set here or by the AUTOSHADE_PYTHON environment variable — never by a file that arrives beside your photos.",
        "运行 AI 侧车（分割、降噪、风格）的 Python。留空表示用平台默认值。只能在这里设置，或用 AUTOSHADE_PYTHON 环境变量；随照片一起到来的文件永远不能指定它。"),
    ("Detect", "自动查找"),
    ("Look in the standard install locations for a working Python 3",
        "在标准安装位置里找一个能用的 Python 3"),
    ("found {bin}", "已找到 {bin}"),
    ("no Python found in the standard install locations — type the full path above",
        "标准安装位置里没有可用的 Python——请在上面填写完整路径"),
    ("Last used folder", "上次用过的文件夹"),
    ("Ask every time", "每次都问"),
    ("a save dialog opens on every export", "每次导出都会打开保存对话框"),
    ("Export to…", "导出到…"),
    ("Export to a one-off path…", "只把这一次导出写到你选的路径…"),
    ("Pick a path for THIS export only — the Destination setting is left as it is",
        "只为「这一次」导出选路径——不改「目标位置」设置"),
    ("Ctrl+S · save this photo's develop (recipe + a Lightroom/ACR XMP for RAW; a baked retouch master is linked so reopening restores it) to your develop store",
        "Ctrl+S · 把这张照片的显影保存到显影库（配方 + RAW 附带 Lightroom/ACR XMP；已烘焙的修饰母版会被关联，重新打开可恢复）"),

    // ── Empty-state landing screen (update()) ────────────────────────────────
    ("AI auto-develop · RAW develop", "AI 自动出片 · RAW develop"),
    ("📷 Open photo…  (Ctrl+O)", "📷 打开照片…  (Ctrl+O)"),
    ("🗂 Open folder…", "🗂 打开文件夹…"),
    ("or drag a RAW / image straight into the window · drag & drop anywhere",
        "或把 RAW / 图片直接拖进窗口 · drag & drop anywhere"),

    // ── Variant strip · kind labels + switch status ──────────────────────────
    ("▣ Original", "▣ 原片"),
    ("✨ AI generated", "✨ AI 生成"),
    ("◭ Reverse-fit", "◭ 反推"),
    ("Switched to variant「{name}」 — variants are independent, switching is lossless",
        "已切到「{name}」变体 — 各变体独立，切换无损"),

    // ── Canvas mask badge + model-picker placeholder ─────────────────────────
    ("▨ Bitmap mask", "▨ 位图蒙版"),
    // R29 Batch-6b. 「本机实测模型」 not 「导入」 on purpose, exactly like the AI
    // badge below: Lightroom's sidecar holds the STROKE and no alpha at all, so
    // what the canvas shows was drawn by our own rasteriser from a measured
    // model of Lightroom's kernel, and the badge has to say so. Every CJK glyph
    // here already occurs elsewhere in the GUI tree — 「实」×8 「测」×3 「模」
    // 「型」「渲」「染」「本」「机」 — so the shipped subset covers it
    // (`python scripts/subset_gui_fonts.py --check`).
    ("▨ Brush mask (drawn from our measured model)", "▨ 画笔蒙版（本机实测模型渲染）"),
    // R27 Batch-5. 「重算」 not 「导入」 on purpose: Lightroom's sidecar holds no
    // raster, so what the canvas shows came from OUR segmenter and the badge
    // has to say so — a photographer looking at a sky selection that looks
    // right must not be left assuming it is Adobe's. Every CJK glyph here is
    // already in the shipped subset (verified against
    // assets/fonts/NotoSansSC-autoshade.ttf): 「本」「机」「重」「算」「非」
    // 「原」「栅」「格」「尚」「由」 all occur elsewhere in this table's font.
    ("▨ AI mask (re-derived locally, not Adobe's raster)",
        "▨ AI 蒙版（本机重算，非 Adobe 原栅格）"),
    ("▨ AI mask (carried, not yet re-derived)", "▨ AI 蒙版（已带走，尚未重算）"),
    ("Select…", "选择… / pick"),
    ("or type a custom id", "或输入自定义模型 id"),

    // ── Versions · save / load snapshots (status) ────────────────────────────
    ("Version v{n} saved → {path}", "版本 v{n} 已存 → {path}"),
    ("Save version failed: {err}", "存版本失败: {err}"),
    ("Loaded version v{n} — Ctrl+Z returns to before the load", "已载入版本 v{n} — Ctrl+Z 可回到载入前"),
    ("camera base look re-estimated — this photo was saved by a version whose preview sampler ran bright, so its stored base look rendered too dark", "相机基调已重估——这张照片由预览采样偏亮的旧版本保存，存档基调渲染过暗"),
    ("this photo's saved crop and masks were rotated to match the RAW's EXIF orientation — earlier versions displayed rotated RAWs sideways, so their coordinates were stored against the sideways frame", "本照片存档的裁剪与蒙版已按 RAW 的 EXIF 方向转正——旧版本没有把旋转过的 RAW 转正显示，这些位置便是按未转正的画面记录的"),
    ("its raster masks are image files, not coordinates, and could NOT be rotated — check them and re-generate if they no longer fit", "其栅格蒙版是图片文件，不是可换算的位置，无法随之转正——请检查，若已对不上请重新生成"),
    ("busy — the preview-resolution switch was not applied; pick it again when the current task finishes", "忙碌中——预览分辨率切换未生效；当前任务结束后请再选一次"),
    ("preview resolution kept — this retouched canvas has no saved master to re-decode at the new size; save the photo, then switch", "预览分辨率保持不变——修饰后的画布尚无已保存母版可按新尺寸重解码；请先保存照片再切换"),
    ("preview resolution kept — this canvas's retouch master is no longer on disk, so it cannot be re-decoded at the new size", "预览分辨率保持不变——本画布的修饰母版已不在磁盘上，无法按新尺寸重解码"),
    ("preview resolution kept — a generated variant's pixels come from its own render; switch to a source-based variant to work at another resolution", "预览分辨率保持不变——生成变体的像素来自它自己的渲染；请切换到基于源的变体以在其他分辨率下工作"),
    ("Switched to variant「{name}」 — its baked pixels stay at {px}px (their own bake); edits and retouches follow that, not the preview preference", "已切换到变体「{name}」——其烘焙像素保持在 {px}px（各自的烘焙分辨率）；编辑与修饰跟随它，而非预览偏好"),
    ("the canvas pixels stay at {px}px (their own bake) — edits and retouches follow that, not the preview preference", "画布像素保持在 {px}px（各自的烘焙分辨率）——编辑与修饰跟随它，而非预览偏好"),
    ("restored the canvas pixels", "已恢复画布像素"),
    ("variant removed", "变体已删除"),
    (" · restored pixels stay at {px}px (their own bake)", " · 恢复的像素保持在 {px}px（各自的烘焙分辨率）"),
    ("Load v{n} failed: {err}", "载入 v{n} 失败: {err}"),

    // ── Tone curve caption (curve_editor) ────────────────────────────────────
    ("Click to add a point · drag to move · drag outside the box to delete — preview and export match (XMP carries the closest Lightroom form)",
        "点击加点 · 拖动移点 · 拖出框外删点 — 预览与导出一致（XMP 只携带最接近的 Lightroom 形式，Lightroom 中可能有细微差异）"),

    // ── AI segmentation · subject / sky (labels + status) ────────────────────
    ("Subject", "主体"),
    ("Sky", "天空"),
    ("AI「{what}」mask added — adjust its sliders (exposure / contrast / saturation…) to take effect",
        "AI「{what}」蒙版已加入 — 调它的滑杆（曝光/对比/饱和…）即刻生效"),
    // WHICH backend drew it. `--target subject` has two tiers and they are not
    // interchangeable, so the run names itself; the fallback also gets its own
    // sentence below rather than sharing this one.
    (" · drawn by {backend}", " · 由 {backend} 绘制"),
    ("「{what}」was drawn by the U^2-Net FALLBACK, not the pinned BiRefNet — its edges are materially softer and it can invent a subject where there is none. Install torchvision + timm + einops matching your torch to get the pinned model.",
        "「{what}」是 U^2-Net 回退层画的，不是固定版本的 BiRefNet —— 它的边缘明显更糊，也可能在没有主体的画面上编出一个。装上与你 torch 配套的 torchvision + timm + einops 才能用上 BiRefNet。"),
    ("AI segmentation failed", "AI 分割失败"),

    // ── Status bar · open / decode / scan / settings / esc ───────────────────
    ("decoding {path} …", "解码 {path} …"),
    ("scanning {path} …", "扫描 {path} …"),
    ("settings saved — applies to the next AI call (Analyze / Fill / Reimagine)", "设置已保存 — 对下一次 AI 调用生效（分析 / 生成填充 / 重绘）"),
    ("preview develop failed", "预览显影失败"),
    ("ready — adjust sliders or run AI Analyze", "就绪 — 拉滑杆或运行 AI 分析"),
    ("ready — restored saved edits ({kind}); Reset returns to neutral",
        "就绪 — 已恢复保存的编辑（{kind}）；「重置」可回中性"),
    ("could not open", "打开失败"),
    ("1 photo — click a thumbnail to open", "1 张照片 — 点击缩略图打开"),
    ("{n} photos — click a thumbnail to open", "{n} 张照片 — 点击缩略图打开"),
    ("scan failed", "扫描失败"),
    ("busy — wait for the current task to finish before opening", "忙 — 等当前任务完成再打开"),
    ("unsupported file type: {path}", "不支持的文件类型: {path}"),
    ("Exited the current tool (Esc)", "已退出当前工具（Esc）"),

    // ── Status bar · AI analyze / render / export / region ───────────────────
    ("refining your current edit with AI…", "AI 微调当前编辑中…"),
    ("analyzing with AI (GPT + Claude)…", "AI 分析中（GPT + Claude）…"),
    ("AI develop applied", "AI 显影已应用"),
    ("analyze failed", "分析失败"),
    ("rendering + AI denoise → {path} … (GPU sidecar, can take minutes)",
        "渲染 + AI 降噪 → {path} …（GPU sidecar，可能数分钟）"),
    ("rendering full-resolution → {path} …", "全分辨率渲染 → {path} …"),
    ("exported → {path}", "已导出 → {path}"),
    ("export failed", "导出失败"),
    ("retouch failed", "修饰失败"),
    ("region {w}×{h}% — type a direction, then AI Analyze (click to clear)",
        "选区 {w}×{h}% — 输入方向语后 AI 分析（点击清除）"),

    // ── Status bar · batch render / paste / preview re-decode ────────────────
    // R22-7: the three batch lines carried a hardcoded ./out, which the
    // Destination setting turned into a lie. Start and landing name the root
    // that was ACTUALLY used ({path}, from the outcome's own fact); the
    // per-photo tick drops it — 500 repetitions of one folder name only pushed
    // the counts out of a truncated status line.
    ("Batch-rendering {n} photos → {path} …", "批量渲染 {n} 张 → {path} …"),
    ("batch {n} done → {path}", "批量 {n} 张完成 → {path}"),
    ("Batch: {ok} succeeded, {fail} failed: {detail}", "批量：{ok} 成功、{fail} 失败：{detail}"),
    (" · same-name photos kept apart: {list}", " · 同名照片已避让：{list}"),
    (" · {n} base look(s) re-estimated (a pre-era save rendered too dark)", " · {n} 张的相机基调已重估（旧版保存的基调渲染过暗）"),
    ("Batch-rendering {done}/{total} …", "批量渲染 {done}/{total} …"),
    ("Pasting recipe to {n} photos…", "粘贴配方到 {n} 张…"),
    ("Recipe pasted to {ok} photos ({xmp} XMP) → develop store", "配方已粘贴到 {ok} 张（{xmp} 个 XMP）→ 显影库"),
    ("{ok} succeeded, {fail} failed: {detail}", "{ok} 成功、{fail} 失败：{detail}"),
    ("batch paste", "批量粘贴"),
    ("Preview resolution {px}px — re-decoded", "预览分辨率 {px}px — 已重解码"),

    // ── Status bar · XMP save ────────────────────────────────────────────────
    // Terminology: sidecar/边车 is reserved for the RAW-adjacent .xmp; the
    // store's recipe.json is "the saved develop / 已保存的显影" (it no longer
    // sits beside anything).
    ("A generated variant's look lives in its pixels — there's no parametric recipe to export; run 「Reverse-fit」 first to get an exportable XMP",
        "生成变体的观感在像素里，没有参数配方可导；先「反推配方」得到可导出的 XMP"),
    ("XMP + recipe saved → {path}", "XMP + 配方已保存 → {path}"),
    ("recipe saved → {path} (XMP applies to RAW only)",
        "配方已保存 → {path}（XMP 仅适用于 RAW）"),
    ("recipe saved — but the Lightroom XMP failed: {err}",
        "配方已保存 — 但 Lightroom XMP 写入失败：{err}"),
    // M6a export-side lossy-projection disclosure (util::xmp_loss_line): the
    // counts come from the writer's own per-mask verdicts, so these are pure
    // labels — an empty list prints nothing at all.
    ("the Lightroom XMP does not carry: {list} (recipe.json keeps all of it)",
        "Lightroom XMP 不会带走：{list}（recipe.json 全部保留）"),
    ("bitmap masks ×{n}", "位图蒙版 ×{n}"),
    ("muted masks ×{n}", "已静音蒙版 ×{n}"),
    ("shape components flattened ×{n}", "形状组件已压平 ×{n}"),
    // R27 Batch-4 (L-08). The odd member of the export bucket: the brush group
    // IS written to the sidecar, complete — so the phrase has to break out of
    // the frame sentence's 「不会带走」 rather than sit inside it. Since R29
    // Batch-6b what it breaks out to say is the AI line's correction, one
    // notch weaker: the brush IS drawn here, from a measurement of Lightroom's
    // own kernel rather than from Adobe's code, so the alpha is ours.
    ("brush masks ×{n} drawn from our measured model, not Adobe's raster",
        "画笔蒙版 ×{n} 由本机实测模型渲染——非 Adobe 原栅格"),
    // The AI mask's line has to carry the OPPOSITE correction to the brush's:
    // the sidecar gets the intent WHOLE (Lightroom will rebuild its own mask
    // from it), so nothing was left out of the XMP — what differs is the alpha
    // this app drew. 「非 Adobe 原栅格」 is the load-bearing half.
    ("AI masks ×{n} re-derived locally — not Adobe's raster",
        "AI 蒙版 ×{n} 由本机重算——非 Adobe 原栅格"),
    // No 「未带走」 on these two: the frame sentence above is already
    // 「…不会带走：」, and "will not carry: radial rotation NOT CARRIED" reads as
    // the opposite of what happened (R22 L5). The English labels are bare nouns
    // for exactly that reason.
    ("radial rotation ×{n}", "径向旋转 ×{n}"),
    // R25 P5: when the writer CAN name the angle it dropped, it does — and it
    // says why, because 「radial rotation ×1」 leaves a photographer unable to
    // tell a deliberate limit from a bug. The bare label above stays as the
    // fallback for a rotation with no nameable angle (`util::rotation_degrees`).
    // v0.32.0 changed the REASON, not the line: `crs:Angle`'s sign and pivot are
    // measured now and the projection carries the tilt, so what is left is a
    // document that declares no frame — the pixel→normalised fold has no aspect
    // to fold with (`xmp::FrameAspect`).
    ("Rotation {a}° not written to XMP (frame size unknown)",
        "旋转 {a}° 未写入 XMP（画幅尺寸未知）"),
    ("recolour gains ×{n}", "重上色增益 ×{n}"),
    // R24-5 M0: the same line NAMES the masks now (「which of my twelve?」 was
    // the half a count could not answer), and grew a GLOBAL bucket beside the
    // mask one — an active control the engine renders that the sidecar has no
    // property for. The two labels below are the only members of that bucket
    // today; the tier registry decides membership, not this list.
    ("+{n} more", "另 {n} 个"),
    ("(unnamed)", "（无名）"),
    ("camera base curve", "相机基础曲线"),
    ("lens profile correction", "镜头配置文件校正"),
    // The import direction of the same fact (workers.rs, the Opened handler).
    ("this Lightroom sidecar carries {n} global setting(s) the engine does not render (a save keeps them untouched): {list}",
        "这份 Lightroom sidecar 里有 {n} 项本引擎不渲染的全局设置（保存时会原样保留）：{list}"),
    ("could not clear the saved edits: {err}", "无法清除已保存的编辑：{err}"),
    ("save postponed: this photo is being changed by another AutoShade process ({err}); your canvas remains unsaved — retry",
        "保存已推迟：另一个 AutoShade 进程正在修改这张照片（{err}）；画布上的编辑尚未保存 — 请稍后重试"),
    ("{n} Lightroom XMP projection(s) failed (those develops ARE saved): {detail}",
        "{n} 个 Lightroom XMP 投影写入失败（显影本身已保存）：{detail}"),
    ("{n} clear(s) could not be marked: {detail} — a sidecar beside the RAW may restore those edits on the next open",
        "{n} 次清除成功但清除标记写入失败：{detail} — RAW 旁的 sidecar 可能在下次打开时恢复这些编辑"),
    ("saved with warnings — the window stays open so they can be read; quit again to close: {detail}",
        "已保存，但有警告 — 窗口保持打开以便阅读；再次退出即可关闭：{detail}"),
    (" — ⚠ {n} XMP projection(s) failed (those pastes ARE saved): {detail}",
        " — ⚠ {n} 个 XMP 投影写入失败（粘贴本身已保存）：{detail}"),
    (" — {n} sidecar(s) regenerated rather than merged (Lightroom-only properties dropped): {detail}",
        " — {n} 个 XMP 文件因无法合并而重建（Lightroom 专有属性已丢失）：{detail}"),
    ("neutral recipe — saved edits cleared (saved files removed)",
        "中性配方 — 已清除保存的编辑（存档文件已删除）"),
    ("neutral recipe — nothing to save", "中性配方 — 无需保存"),
    ("saved edits cleared, but the clear could not be marked ({err}) — a sidecar beside the RAW may restore them",
        "已清除保存的编辑，但清除标记写入失败（{err}）— RAW 旁的 sidecar 可能在重新打开时恢复这些编辑"),
    (" · retouched pixels: master linked — reopening restores them (the Lightroom XMP stays parametric-only)",
        " · 修饰像素：已关联烘焙母版 — 重新打开会恢复（Lightroom XMP 仍只含参数化显影）"),
    ("cancelled — the app is free again and the late result is discarded; a generative call stops at its next checkpoint, while an AI analyze keeps running (and billing) until it finishes or times out",
        "已取消 — 应用已解锁，迟到结果将被丢弃；生成类调用会在下一检查点停止，而 AI 分析会一直跑到完成或超时（仍然计费）"),
    ("✕ Cancel", "✕ 取消"),
    ("the cancelled AI call is still running (and still billed) — this re-arms when it finishes or times out",
        "已取消的 AI 调用仍在运行（仍在计费）— 它结束或超时后此按钮会重新可用"),
    ("Stop waiting: the app unblocks now and the late result is discarded. A generative call halts at its next checkpoint; an AI analyze keeps running (and billing) until it finishes or times out",
        "停止等待：应用立即解锁，迟到结果将被丢弃。生成类调用会在下一检查点停止；AI 分析则会一直跑到完成或超时（仍然计费）"),
    ("over 999 retouch masters for this photo — clean up ./out first",
        "本照片的修饰母版已超过 999 个 — 请先清理 ./out"),
    (" · previous save backed up as v{n}", " · 之前的保存已备份为 v{n}"),
    ("● unsaved", "● 未保存"),
    ("This card differs from its saved develop — Ctrl+S saves every card; switching photos keeps unsaved work for this session only",
        "当前变体与已保存的显影不同——Ctrl+S 会保存所有变体；切换照片只会在本次会话中保留未保存的工作"),
    ("ready — restored this session's unsaved edits (● not saved yet; Ctrl+S)",
        "就绪 — 已恢复本会话未保存的编辑（● 尚未保存；Ctrl+S）"),
    ("recipe limits discarded {n} mask(s), {m} component(s), {c} curve point(s) and {s} string byte(s) on restore — the saved file exceeds the app's caps",
        "恢复时因配方上限丢弃了 {n} 个蒙版、{m} 个组合项、{c} 个曲线点、{s} 个字符字节 — 存档文件超出应用上限"),
    ("not saved — a develop-store write failed: {err}",
        "未保存 — 显影库写入失败：{err}"),
    ("this photo's variant strip (variants.json) cannot be read — background variants stay hidden and saving refuses until the file is fixed or deleted",
        "此照片的变体条（variants.json）无法读取 — 后台变体保持隐藏，保存将被拒绝，直到该文件被修复或删除"),
    ("recipe.json is unreadable ({err}) — edits NOT fully restored; Ctrl+S would overwrite it (the unread save is backed up as a version first)",
        "recipe.json 无法解析（{err}）— 编辑未完整恢复；Ctrl+S 会覆盖它（未能读取的保存会先备份为版本）"),
    ("a saved develop exists but holds no effective edits", "已保存的显影存在但不含有效编辑"),
    ("the current variant holds no edits; this photo's saved edits live in {n} background variant(s)",
        "当前变体未作修改；本照片已保存的编辑在 {n} 个后台变体中"),
    ("AI develop applied — verdict {v}: NOT saved (Ctrl+S keeps it, Ctrl+Z steps back)",
        "AI 显影已应用 — 判词 {v}：未保存（Ctrl+S 保留，Ctrl+Z 回退）"),
    // Verdict decision words (advisor::decision_key) + the verdict line
    // skeleton, rendered at draw time so a language switch re-renders them.
    ("Accept", "接受"),
    ("Revise", "修订"),
    ("Reject", "驳回"),
    ("{decision} — {reasons}", "{decision} — {reasons}"),
    ("AI develop applied · saved to recipe.json", "AI 显影已应用 · 已保存到 recipe.json"),
    ("AI develop applied · saved (previous save backed up as v{n})",
        "AI 显影已应用 · 已保存（之前的保存已备份为 v{n}）"),
    ("AI develop applied — but saving the sidecar failed: {err}",
        "AI 显影已应用 — 但显影保存失败：{err}"),
    ("AI develop applied — but NOT saved: backing up your existing save failed ({err}); Ctrl+S overwrites explicitly",
        "AI 显影已应用 — 但未保存：备份你已有的保存失败（{err}）；Ctrl+S 可显式覆盖"),
    (" · NOT persisted: backing up your existing save failed ({err}) — Ctrl+S to save explicitly",
        " · 未持久化：备份你已有的保存失败（{err}）— Ctrl+S 可显式保存"),
    ("AI develop applied — but NOT saved: this photo is being changed by another AutoShade process ({err}); Ctrl+S retries",
        "AI 显影已应用 — 但未保存：另一个 AutoShade 进程正在修改这张照片（{err}）；Ctrl+S 可重试"),
    (" · NOT persisted: the develop store could not be locked ({err}) — Ctrl+S to save explicitly",
        " · 未持久化：显影库无法加锁（{err}）— Ctrl+S 可显式保存"),
    ("Open a photo, or open a folder to browse your library.",
        "打开一张照片，或打开文件夹浏览图库。"),
    ("busy — variants unlock when the current task finishes",
        "忙碌中 — 当前任务完成后变体才可切换/删除"),
    ("busy — the photo opens when the current task finishes",
        "忙碌中 — 当前任务完成后照片才会打开"),
    (" · {n} develop warning(s): {detail}",
        " · {n} 条显影警告：{detail}"),
    ("busy — undo and redo unlock when the current task finishes",
        "忙碌中 — 当前任务完成后才可撤销/重做"),
    ("opened the first photo — {n} more ignored (drop their folder to browse them all)",
        "已打开第一张 — 其余 {n} 张被忽略（把它们所在的文件夹拖进来可整体浏览）"),
    ("{n} bitmap mask(s) not pasted — their rasters belong to the source photo (re-run AI select on each target)",
        "{n} 个位图蒙版未粘贴 — 栅格属于源照片（请在各目标上重跑 AI 选择）"),
    ("AI segmenting {what}… (first run auto-downloads the model; failures are reported here)",
        "AI 分割{what}中…（首次运行自动下载模型；失败会在此报告）"),
    ("Reset to its default", "重置为默认值"),
    ("Language & Theme apply immediately. The provider sections below persist via 「Save settings」 to autoshade.local.json in your per-user AutoShade folder (never in a repo) and apply to the next AI call (Analyze / Fill / Reimagine).",
        "「语言」与「主题」立即生效。下方的提供商设置经「保存设置」写入你 AutoShade 个人目录下的 autoshade.local.json（不在仓库里），对下一次 AI 调用生效（分析 / 填充 / 重绘）。"),

    // ── Status bar · WB / range pick + manual mask placement ─────────────────
    ("WB eyedropper: {k} K · tint {tint} — fine-tune in the Tone section",
        "WB 吸管：{k} K · tint {tint} — 可在色调区微调"),
    ("Color range: sampled — the 「Tolerance」 slider adjusts the selection width",
        "颜色范围：已取样 — 「容差」滑杆调节选中宽度"),
    ("Manual {n}", "手动 {n}"),
    // ZH must quote the panel's ACTUAL header 「局部蒙版」 (i18n "Local Masks
    // ({n})"), not a panel that doesn't exist.
    ("mask placed — pull its sliders in 「Local Masks」 at left (all 0 now, no visible effect yet)",
        "蒙版已放置 — 在左侧「局部蒙版」里拉滑杆（当前全为 0，无可见效果）"),

    // ── Status bar · generative fill / heal / clone ──────────────────────────
    ("write what should fill the painted area", "写下涂抹区域该填入什么"),
    ("paint the area to remove/fill first (tick Paint mask)", "先涂抹要移除/填充的区域（勾选「涂抹蒙版」）"),
    ("generative fill (full-res render)… (slow, minutes)", "生成填充（全分辨率渲染）…（慢，数分钟）"),
    ("generative fill via gpt-image… (high quality can run minutes — progress in the status bar; ✕ Cancel to stop)",
        "gpt-image 生成填充中…（高质量可能需要数分钟——进度见状态栏；✕ 取消可停止）"),
    ("filled → {path} (updated current variant)", "已填充 → {path}（更新当前变体）"),
    ("tick Paint mask and paint the spots, then Heal painted area",
        "勾选「涂抹蒙版」并涂抹瑕疵，再「修复涂抹区域」"),
    ("healing painted area…", "修复涂抹区域中…"),
    ("AI healing… (~10-30s)", "AI 去瑕疵中…（约 10-30 秒）"),
    ("healed {n} spot(s) → {path}", "已修复 {n} 处 → {path}"),
    ("Clone source sampled — brush the area to cover, then 「⎘ Clone painted area」",
        "克隆源已取样 — 画笔涂要覆盖的区域，然后「⎘ 克隆已涂区域」"),
    ("Alt+click to set the clone source first", "先 Alt+点击取克隆源点"),
    ("Brush the area to clone over first", "先用画笔涂要克隆覆盖的区域"),
    ("Cloning… (local pixel compute)", "克隆中…（本地像素运算）"),
    ("Cloned {n} spot(s) → {path}", "克隆 {n} 处 → {path}"),

    // ── Active AI denoise (on-canvas, Detail section) ────────────────────────
    // 「now」 vs the Export section's 「on export」 (R22 #16): two TIMINGS of one
    // SCUNet denoiser, which the shared 🤖 prefix alone could not tell apart.
    // ZH keeps this block's 去噪 and the export block's 降噪 — see the note at
    // the export-settings entries.
    ("🤖 AI Denoise now", "🤖 立即 AI 去噪"),
    ("Full-res denoise", "全分辨率去噪"),
    ("Run the SCUNet GPU sidecar on this variant's pixels and show the result on canvas (undoable — bakes a clean base into the current variant; the develop sliders keep applying on top; first run downloads the model)",
        "对当前变体的像素跑 SCUNet GPU 边车，结果直接上画布（可撤销——干净基图烘焙进当前变体；显影滑杆继续在其上生效；首次运行会下载模型）"),
    // The capability arm. WORD-FOR-WORD the segmentation buttons' missing-
    // sidecar line (only the env var and the file name differ), because it is
    // word-for-word the same situation — a release package ships neither
    // helper — and two wordings for one condition is how a user concludes they
    // are two different problems.
    ("this build did not ship the python sidecar — run AutoShade from the project directory, or point AUTOSHADE_DENOISE_SCRIPT at python/denoise.py",
        "本版本未随发布包分发 python 边车——请从项目目录运行 AutoShade，或用 AUTOSHADE_DENOISE_SCRIPT 指向 python/denoise.py"),
    ("Denoise at full resolution (the full-sensor develop for a RAW, the image itself for a baked source; slow) — off = a ≤2048px working copy for a quick on-canvas result",
        "全分辨率去噪（RAW 用全画幅显影，烘焙图像用原图；慢）——关闭 = 用 ≤2048px 工作副本快速出画布结果"),
    ("AI denoise (full-res)… (GPU sidecar, can take minutes; first run downloads the model)",
        "AI 去噪（全分辨率）中…（GPU 边车，可能需数分钟；首次运行会下载模型）"),
    ("AI denoise… (GPU sidecar on a ≤2048px working copy; first run downloads the model)",
        "AI 去噪中…（GPU 边车处理 ≤2048px 工作副本；首次运行会下载模型）"),
    ("AI denoised → {path} (updated current variant)",
        "AI 去噪完成 → {path}（已更新当前变体）"),
    ("An operation is still running — wait for it to finish, then close",
        "还有操作在运行——等它完成后再关闭"),
    ("over 999 generated variants for this photo — clean up ./out first",
        "这张照片的生成变体已超过 999 个——请先清理 ./out"),

    // ── Status bar · reimagine / reverse-fit / style prompt ──────────────────
    ("AI generating… (gpt-image; high quality can run minutes — progress in the status bar; ✕ Cancel to stop; hi-res input needs a full-frame develop first)",
        "AI 生成出片中…（gpt-image；高质量可能需要数分钟——进度见状态栏；✕ 取消可停止；高分辨率输入需先全幅显影）"),
    ("「AI generated」variant created → {path} · keep tweaking or 「Reverse-fit」",
        "已生成「AI 生成」变体 → {path} · 可继续微调或「反推配方」"),
    ("Reverse-fitting… (global + semantic/ranges + spatial tiles)",
        "反推配方中…（全局 + 语义/亮度范围 + 空间图块）"),
    ("Reverse-fitting… (statistical fit, local compute)", "反推配方中…（统计拟合，本地运算）"),
    ("Reverse-fit done: look residual {before}→{after} · created a「Reverse-fit」variant (editable / XMP / full-res)",
        "反推完成：look 残差 {before}→{after} · 已建「反推」变体（可编辑/导 XMP/出全分辨率）"),
    (" · includes sky-zone correction (adjustable in the mask panel; XMP carries the global part only)",
        " · 含天空分区校正（蒙版面板可调；XMP 只带全局部分）"),
    (" · includes native range correction (adjustable in the mask panel and written to XMP)",
        " · 含原生范围校正（蒙版面板可调，并会写入 XMP）"),
    (" · then AI review (vision call)", " · 拟合后 AI 打分（vision 调用）"),
    (" · AI review: match {score}/100 — {critique}",
        " · AI 打分：匹配 {score}/100——{critique}"),
    (" · AI review unavailable ({err}) — the fit itself already landed",
        " · AI 打分不可用（{err}）——反推本身已完成"),
    // ── R23-6: the reverse-fit's own status facts (workers::render_fit_note)
    (" · it suggests: {hint} (nothing was changed — tick 「deep」 to let it try)",
        " · 它的建议：{hint}（什么都没有改动——勾选「深度」才会让它去试）"),
    (" · ⚠ THE REVERSE-FIT WAS DISCARDED: every version of it rendered farther from the target than your untouched photo, so the recipe was reset to neutral — this is the same as not having reverse-fitted at all",
        " · ⚠ 反推结果已被丢弃：它的每一个版本渲染出来都比你未处理的原图离目标更远，配方已重置为中性——这与没有做反推是一样的"),
    (" · ⚠ the reference's proportions do not match this photo — it was cropped, or it is not the same frame; either way the two distributions are not comparable",
        " · ⚠ 参考图与本图的比例不一致——它被裁切过，或者不是同一张；两者都会让两边的分布无法相比"),
    (" · deep: AI review BEFORE saving, up to one guided retry",
        " · 深度：保存之前先做 AI 复查，最多一次按提示的重试"),
    (" · deep: the review found nothing this app can act on — the plain fit stands",
        " · 深度：复查没有给出本应用做得了的动作——保留原始反推结果"),
    (" · deep: the review asked for {action}, which could not be carried out — the plain fit stands",
        " · 深度：复查要求「{action}」，但它没能生效——保留原始反推结果"),
    (" · deep: tried {action} on the review's suggestion and kept it (it re-scored at least as high)",
        " · 深度：按复查提示试了「{action}」，已采用（复查分数不低于原分）"),
    (" · deep: tried {action} on the review's suggestion and discarded it (it re-scored lower)",
        " · 深度：按复查提示试了「{action}」，已丢弃（复查分数更低）"),
    ("Reverse-fit failed", "反推失败"),
    ("Style prompt extracted → filled into the Reimagine prompt (also saved ./out/<stem>.style.txt)",
        "风格提示词已提取 → 已填入 Reimagine 提示词（同时存 ./out/<stem>.style.txt）"),
    ("Extracting style prompt… (vision, ~5-20s)", "提取风格提示词中…（vision，约 5-20 秒）"),
    ("Style extraction failed", "风格提取失败"),

    // ── Shortcuts cheat-sheet window (title + both columns) ───────────────────
    ("⌨ Shortcuts", "⌨ 快捷键 · Shortcuts"),
    ("Open photo", "打开照片"),
    ("Undo / Redo", "撤销 / 重做"),
    ("Copy recipe / paste to selected", "复制配方 / 粘贴到选中"),
    ("Step through the library", "图库走图"),
    ("Step through the library (outside the controls panel)", "图库走图（指针不在控制面板上时）"),
    ("Zoom in / out / fit / 1:1", "放大 / 缩小 / 适配 / 1:1"),
    ("Enter / exit crop", "进入 / 退出裁剪"),
    ("Brush size (paint / clone armed)", "笔刷大小（画笔 / 克隆已启用时）"),
    ("Hide / show the side panels", "隐藏 / 显示侧栏"),
    ("Hover a slider + ↑/↓", "悬停滑杆 + ↑/↓"),
    ("Nudge its value (Shift ×10)", "微调数值（Shift ×10）"),
    ("B (hold)", "B（按住）"),
    ("Compare original", "对比原图"),
    ("Toggle mask overlay (crop: cycle grid)", "蒙版覆盖层开关（裁剪中：切换网格线）"),
    ("Commit the crop (exit the tool)", "提交裁剪（退出工具）"),
    ("WB eyedropper", "白平衡吸管"),
    ("Retouch stamp", "修饰图章"),
    ("Paint mask brush", "画笔蒙版"),
    ("Linear / radial gradient", "线性 / 径向渐变"),
    ("Shift (while drawing a gradient)", "Shift（画渐变时）"),
    ("Lock to horizontal / vertical", "锁定水平 / 垂直"),
    ("Before / after (toggle)", "原图 / 效果切换（锁定）"),
    ("Side-by-side ↔ single view", "并排 ↔ 单图视图"),
    ("Toggle clipping warning", "削波警告开关"),
    ("Exit tool / close this window", "退出当前工具 / 关闭本窗"),
    ("This cheat-sheet", "本速查表"),
    ("Scroll", "滚轮"),
    ("Zoom (toward cursor)", "缩放（指向光标）"),
    ("Double-click canvas", "双击画布"),
    ("Space+drag / middle-drag", "空格+拖 / 中键拖"),
    ("Pan", "平移"),
    ("Drag when zoomed", "放大后直接拖"),
    ("Pan (Ctrl+drag = box-select)", "平移（Ctrl+拖 = 框选）"),
    ("Alt+click", "Alt+点击"),
    ("Sample clone source", "克隆取源点"),
    ("Slider double-click / right-click", "滑杆双击 / 右键"),
    ("Curve: click / drag / drag-out", "曲线：点击/拖/拖出框"),
    ("Add / move / delete point", "加点 / 移点 / 删点"),
    ("Drag a mask handle", "蒙版手柄拖拽"),
    ("Reshape / move the selected mask", "改形 / 移动选中蒙版"),

    // ── Drag & drop overlay ──────────────────────────────────────────────────
    ("Drop to open", "松开打开 · Drop to open"),

    // ── Round-2 polish batch (quit guard / develop store /
    //    versions delete / settings statuses) ─────────────────────────────────
    // ("AI verdict") retired in round 10: the section title has been the bare
    // "AI" key since the UX batch — the old key matched no call site.
    ("No photo open.", "未打开照片。"),
    ("Before (source)", "原图（源）· Before"),
    ("Delete this snapshot (its frozen mask rasters go with it)",
        "删除该快照（连同其冻结的蒙版栅格）"),
    ("Version v{n} deleted", "版本 v{n} 已删除"),
    ("Delete v{n} failed: {err}", "删除 v{n} 失败：{err}"),
    ("{n} photo(s) have edits that were never saved:", "{n} 张照片的编辑尚未保存："),
    ("Save all & quit", "全部保存并退出"),
    ("Discard & quit", "放弃并退出"),
    ("Cancel", "取消"),
    ("Develop store", "显影库 · Develop store"),
    // R25: the read-only segmentation-sidecar row (closing R22-1 as the
    // security ruling it is — see panels/settings.rs).
    ("Segmentation sidecar", "分割边车"),
    ("This path can only be set by environment variable, because it is executed",
        "此路径只能用环境变量设置，因为它会被当作程序启动"),
    ("Where saved develops live: recipes, Lightroom XMP, version snapshots and mask rasters — one folder per photo, keyed by its absolute path. Override the location with the AUTOSHADE_DATA_DIR environment variable.",
        "已保存显影的存放地：配方、Lightroom XMP、版本快照与蒙版栅格 — 每张照片一个文件夹，按其绝对路径键控。可用 AUTOSHADE_DATA_DIR 环境变量改存放位置。"),
    // R22-8 · SF8-C: the row shows the OPEN photo's hash subdirectory, so it
    // needs a way in — the folder name is a hash nobody can retype.
    ("🗂 Show in file manager", "🗂 在文件管理器中打开"),
    ("Open this folder in your file manager", "在文件管理器中打开这个文件夹"),
    ("Nothing saved for this photo yet — the folder appears with the first save",
        "这张照片还没有任何保存内容——第一次保存后文件夹才会出现"),
    ("could not open the folder: {err}", "无法打开文件夹：{err}"),
    ("Import develops from an old ./out folder…", "从旧 ./out 文件夹导入显影…"),
    ("Saves made before v0.13 lived in a ./out folder next to wherever the app was launched. If your old edits are missing, point this at that folder — its recipes / XMP / versions migrate into the develop store.",
        "v0.13 之前的保存放在启动目录旁的 ./out 里。如果旧编辑不见了，把这里指向那个文件夹 — 其中的配方/XMP/版本会迁入显影库。"),
    ("Open a folder first — import migrates the photos currently in the gallery",
        "请先打开文件夹 — 导入只迁移当前图库里的照片"),
    ("Importing develops from {path} …", "正在从 {path} 导入显影…"),
    ("Imported saved develops for {n} photo(s) from {path}",
        "已从 {path} 导入 {n} 张照片的已保存显影"),
    ("import failed", "导入失败"),
    ("fetching models…", "正在拉取模型列表…"),
    ("fetched {n} models ({chat} chat · {img} image)",
        "已拉取 {n} 个模型（{chat} 对话 · {img} 图像）"),
    ("fetch failed: {err}", "拉取失败：{err}"),
    ("model list discarded — settings changed while it was being fetched",
        "已丢弃模型列表 —— 拉取期间设置已变更"),
    ("{chat} chat", "{chat} 个对话模型"),
    ("List the models THIS endpoint serves (GET /models). The analysis role has its own endpoint and key, so it gets its own list.",
        "列出「这个」端点提供的模型（GET /models）。分析角色有自己的端点和密钥，因此有自己的列表。"),
    ("Reasoning effort", "推理等级"),
    ("provider default", "由供应商决定"),
    ("or type a tier", "或直接输入等级"),
    ("How hard the model is asked to think. Higher tiers cost more and take longer; blank leaves the choice to the provider. An endpoint that does not know the tier is retried without it.",
        "要求模型思考的深度。等级越高越贵越慢；留空则交由供应商决定。端点若不认识该等级，会自动去掉它重试。"),
    ("saved, but the key was not accepted — it contains characters that cannot appear in an HTTP header (a stray space or newline from a copy/paste?). Re-copy it and save again.",
        "已保存，但密钥未被接受——其中含有 HTTP 头不允许的字符（复制粘贴时多带了空格或换行？）。请重新复制后再保存。"),

    // ── UX batch (toolbar slim-down · AI section · Export section · tools) ──
    ("AI", "AI"),
    ("AI Analyze", "AI 分析"),
    ("AI proposes a recipe from scratch (GPT proposal + validation + a visual review: the result is RENDERED and judged by the vision model, which may buy one guided revision), written into the sliders — undoable. Uses the Direction above; Style and Strength steer it. COST, worst case: 11 API calls, 6 of them carrying images (8 high-detail frames). Ticking 「Deep thinking」 below OR pushing Strength above 70% raises that ceiling — either one alone does it; the Deep thinking tooltip has the numbers.",
        "AI 从零提案配方（GPT 提案+验证+视觉复查：结果渲染后交视觉模型打分，低分会多跑一轮提示修订），直接写入滑杆——可撤销。读上方「方向」文本；风格与强度两个滑杆一同生效。费用最多为 11 次 API 调用，其中 6 次带图（8 张高清图）。勾选下方的「Deep thinking」或把强度调到 70% 以上，都会把这个上限提高——两者占其一即可；具体数字见它自己的说明。"),
    // R23-4 · feedback #13: the thinking-mode switch and its cost disclosure.
    ("Deep thinking", "深度思考"),
    ("Make the AI show its work and let it iterate. The proposal must first name what it sees, decide EACH tool family (tone / white balance / presence / HSL / colour grading / curves / detail / framing / masks) with a reason, state the look it is going for, and end by critiquing its own answer — those three sentences land in the rationale above. It also asks the image model for one step more reasoning effort (only when a tier other than 「provider default」 is set in Settings), and lets the visual judge keep going until it scores well enough: 2 rounds at a balanced Strength, 3 above 70%. COST: a normal analyze is at worst 11 API calls (6 with images, 8 high-detail frames); with this box ticked OR Strength above 70% — either one alone is enough — it is at worst 17 calls (10 with images, 14 high-detail), plus roughly 10-20% more output tokens per proposal. Batch and the eval harness never do this.",
        "让 AI 把思考过程写出来，并允许它反复改进。提案要先说明它读到的画面，对每一类工具（影调 / 白平衡 / 清晰与饱和 / HSL / 调色 / 曲线 / 细节 / 裁切 / 蒙版）逐一给出用或不用的理由，说明想要的成片效果，最后再对自己的结果做一次自查——这三段话会出现在上方的说明里。它还会把图像模型的推理档位本次提高一级（仅在设置里选了「provider default」以外的档位时有效），并让视觉复查继续跑到分数达标为止：强度中等时 2 轮，高于 70% 时 3 轮。费用：不开本项时一次分析最多 11 次 API 调用（6 次带图，8 张高清图）；只要勾选本项，或把强度调到 70% 以上（两者占其一即可），最多 17 次调用（10 次带图，14 张高清图），每次提案还多约 10-20% 的输出 token。批处理与 eval 从不启用它。"),
    ("Direction", "方向"),
    ("Free-text direction for AI Analyze — e.g. warmer and moodier",
        "给 AI 分析的自由文字方向——如「更暖、更有氛围」"),
    ("Ctrl+Z · undo the last edit", "Ctrl+Z · 撤销上一步编辑"),
    ("Ctrl+Y · redo the undone edit", "Ctrl+Y · 重做撤销的编辑"),
    ("◫ Compare", "◫ 对比"),
    ("Export", "导出 · Export"),
    ("Export to the Destination (Destination + settings in the Export section)",
        "导出到「目标位置」（目标位置与设置都在 Export 节）"),
    ("Format", "格式"),
    ("Save develop", "保存显影"),
    ("Save develop (recipe + XMP for RAW)", "保存显影（recipe + RAW 的 XMP）"),
    ("Ctrl+Shift+E (or Ctrl+E) · full-resolution render to the Destination below (follows the current variant's pixels); Destination + settings live in the Export section",
        "Ctrl+Shift+E（或 Ctrl+E）· 全分辨率渲染到下方「目标位置」（跟随当前变体的像素）；目标位置与其余设置都在 Export 节"),
    ("Applied by 「Export」 in the toolbar (Ctrl+Shift+E, or Ctrl+E) and by 「Render selected」 in the library. The ▾ beside Export delivers one file to a path you pick without touching the Destination.",
        "由工具栏的「Export」（Ctrl+Shift+E，或 Ctrl+E）和图库的「Render selected」使用。Export 旁的 ▾ 只把这一次的文件写到你选的路径，不改「目标位置」。"),
    // R22-8 · SF8-A: hand the stored projection to Lightroom (which only ever
    // looks beside the photo). 边车 is this table's word for the RAW-adjacent
    // .xmp — see the terminology note in the XMP-save block.
    ("Export .xmp beside the photo", "导出 .xmp 到照片旁"),
    ("⚠ Overwrite the .xmp already there", "⚠ 覆盖已存在的 .xmp"),
    ("Copy this photo's stored Lightroom/ACR sidecar into the photo's own folder, where Lightroom reads it. Save the develop first — this delivers what is stored, not what is unsaved on the canvas.",
        "把这张照片在显影库里的 Lightroom/ACR 边车复制到照片自己的文件夹，Lightroom 就是在那里读它。请先保存显影——这里给出的是已保存的内容，不是画布上未保存的改动。"),
    ("RAW only — a baked PNG/TIFF has no Lightroom sidecar convention, so its neighbouring .xmp belongs to another program",
        "仅 RAW —— 烘焙好的 PNG/TIFF 没有 Lightroom 边车约定，旁边的 .xmp 属于别的程序"),
    ("A .xmp already sits beside this photo (Lightroom's own, or an earlier copy) — clicking again replaces it",
        "这张照片旁已有 .xmp（Lightroom 自己的，或早先复制的）—— 再点一次会替换它"),
    ("Lightroom sidecar delivered → {path}", "Lightroom 边车已写到照片旁 → {path}"),
    ("a .xmp already sits beside this photo ({path}) — click again to replace it",
        "这张照片旁已有 .xmp（{path}）—— 再点一次会替换它"),
    ("the .xmp could not be delivered: {err}", "无法写出 .xmp：{err}"),
    ("SCUNet AI denoise before developing — high-ISO / astro (slow, GPU; needs the python sidecar). Batch render skips it.",
        "显影前 SCUNet AI 降噪——高 ISO/星空（慢，GPU；需 python 边车）。批量渲染不含此项。"),
    ("All regions", "全部区域"),
    ("Midtones", "中间调"),
    ("Global", "全局"),
    ("Temp shift", "色温偏移"),
    ("Tint shift", "色调偏移"),
    ("Brush size", "笔刷大小"),
    ("Clear crop", "清除裁剪"),
    ("Clear brush", "清除画笔"),
    ("Wipe the painted area (shared by Fill, Heal and Stamp)", "清空涂抹区（填充/修复/图章共用）"),
    ("＋ Radial gradient", "＋ 径向渐变"),
    ("Delete this mask (its stack order shifts the ones below)", "删除此蒙版（其后的蒙版层序会前移）"),
    ("Crop — drag corners/edges to resize, inside to move, outside to rotate · Esc to exit",
        "裁剪 — 拖角/边把手调整，框内拖动移动，框外拖动旋转拉直 · Esc 退出"),
    ("Linear gradient — drag from the fully-applied side to the unaffected side (Shift = axis lock) · Esc to exit",
        "线性渐变 — 从完全应用的一侧拖到不受影响的一侧（Shift = 锁轴）· Esc 退出"),
    ("Radial gradient — drag to draw an elliptical area · Esc to exit",
        "径向渐变 — 拖拽画出椭圆区域 · Esc 退出"),
    ("WB eyedropper — click a spot that should be neutral grey/white · Esc to exit",
        "WB 吸管 — 点击应为中性灰/白的位置 · Esc 退出"),
    ("Color range — click the color to pick in the image · Esc to exit",
        "颜色范围 — 点击图中要选取的颜色 · Esc 退出"),
    ("Stamp — Alt+click to set the source · drag to brush the area to cover · Esc to exit",
        "图章 — Alt+点击取源点 · 拖动涂要覆盖的区域 · Esc 退出"),
    ("Brush — paint over the area to fill / heal · Esc to exit",
        "画笔 — 涂抹要填充/修复的区域 · Esc 退出"),
    ("⎘ Enter stamp", "⎘ 进入图章"),
    ("Arm the stamp: Alt+click samples a source, the brush paints the target; your painted mask survives",
        "启用图章：Alt+点击取源，画笔涂目标区；已涂的画笔蒙版会保留"),
    ("Copy the sampled source over the brushed area verbatim (feathered edges, no tone matching) — local compute",
        "把取样源原样盖到涂抹区（羽化边缘，不做色调匹配）——本地计算"),
    ("Regenerate ONLY the painted area from your prompt (gpt-image API call — costs per image); the rest keeps the engine's own develop",
        "只按提示词重生成涂抹区（gpt-image API 调用，按图计费）；其余保持引擎自己的显影"),
    ("A vision model finds small dust spots / blemishes (API call), then each is healed from surrounding REAL pixels — never generated",
        "视觉模型自动找出小灰尘/瑕疵（API 调用），逐个用周围真实像素修复——绝不生成"),
    ("Heal the brushed area from surrounding real pixels — local compute, no API",
        "用周围真实像素修复涂抹区——本地计算，无 API"),
    ("● Unsaved edits", "● 未保存的编辑"),
    ("Enter · save every listed develop, then quit", "Enter · 保存列出的全部显影后退出"),
    ("{n} other variant(s) hold edits — 「Save all」 saves each photo's whole variant strip along with its develop.",
        "还有 {n} 个变体存有编辑——「全部保存」会连同每张照片的整条变体带一起保存。"),
    ("Quit WITHOUT saving — these edits are gone for good", "不保存直接退出——这些编辑将永久丢失"),

    // ── Round-3 audit batch (new / reworded user-facing strings) ─────────────
    ("no photos found in this folder", "此文件夹中没有找到照片"),
    ("AI「{what}」mask refreshed — the existing mask now uses the new selection (its sliders still apply)",
        "AI「{what}」蒙版已刷新——原有蒙版改用新的选区（其滑杆设置仍然生效）"),
    ("the saved retouch master could not be loaded — opened the un-retouched source (Ctrl+S would overwrite the master link)",
        "已保存的修饰母版无法加载——打开的是未修饰原图（Ctrl+S 会覆盖母版链接）"),
    ("loading this variant's retouched master… (showing the source develop meanwhile)",
        "正在载入该变体的修饰母版…（期间显示原片显影）"),
    ("this variant's saved master could not be loaded ({err}) — showing the un-retouched source develop instead",
        "该变体已保存的母版无法加载（{err}）——改为显示未修饰的原图显影"),
    ("saved, but {n} variant(s) still count as unsaved — the window stays open; please report this",
        "已保存，但仍有 {n} 个变体计为未保存——窗口保持打开；请反馈此问题"),
    ("mask area redrawn — its existing adjustments now apply to the new area",
        "蒙版区域已重画——原有调整现作用于新区域"),
    (" · NOT persisted: saving the develop failed ({err}) — Ctrl+S to save explicitly",
        " · 未持久化：保存显影失败（{err}）——用 Ctrl+S 显式保存"),
    ("edits saved under an older spelling of this photo's path were adopted ({path})",
        "已收编本照片旧路径拼写下保存的显影（{path}）"),
    ("a second saved develop exists at {path} (an older spelling of this photo's path) — it was NOT merged; showing the develop under the photo's true path",
        "检测到另一份保存的显影：{path}（来自本照片路径的旧拼写）——未合并；当前显示的是照片真实路径下的存档"),
    ("Style prompt extracted → filled into the Reimagine prompt (saving ./out/<stem>.style.txt failed: {err})",
        "风格提示词已提取 → 已填入重绘提示词（保存 ./out/<stem>.style.txt 失败：{err}）"),
    ("Style prompt extracted → filled into the Reimagine prompt",
        "风格提示词已提取 → 已填入重绘提示词"),
    ("opened the first folder — {n} more dropped item(s) ignored",
        "已打开第一个文件夹——其余 {n} 个拖入项已忽略"),
    ("unsupported file type: {path} — {n} more dropped item(s) ignored",
        "不支持的文件类型：{path}——其余 {n} 个拖入项已忽略"),

    // ── Round-9 batch: mask components / brush / raster tools / eye toggle ──
    ("Show/mute this mask without losing its settings",
        "显示/静音此蒙版——设置全部保留"),
    ("Duplicate this mask (bitmap rasters are copied, so the copies stay independent)",
        "复制此蒙版（位图栅格也会复制，两份互不影响）"),
    ("mask limit reached (64) — delete one first", "蒙版已达上限（64）——请先删除一个"),
    ("Angle", "角度"),
    ("Shapes", "形状"),
    ("＋ Add", "＋ 增加"),
    ("－ Subtract", "－ 排除"),
    ("∩ Intersect", "∩ 交叉"),
    ("Add", "增加"),
    ("Subtract", "排除"),
    ("Intersect", "交叉"),
    ("▭ Linear", "▭ 线性"),
    ("◯ Radial", "◯ 径向"),
    ("Drag on the image to add a linear shape to THIS mask",
        "在图上拖拽，为「当前蒙版」添加一个线性形状"),
    ("Drag on the image to add an elliptical shape to THIS mask",
        "在图上拖拽，为「当前蒙版」添加一个椭圆形状"),
    ("Drag on the image: the new shape composes onto this mask",
        "在图上拖拽：新形状将合成到当前蒙版上"),
    ("Select to drag this shape's knobs on the image (the base mask's knobs come back when deselected)",
        "选中后可在图上拖拽此形状的手柄（取消选中则回到基础蒙版的手柄）"),
    ("Shapes compose in order onto the base mask. In-app render + export only — the Lightroom XMP carries the base shape alone.",
        "形状按列表顺序合成到基础蒙版上。仅本机渲染与导出生效——Lightroom XMP 只携带基础形状。"),
    ("shape added to this mask — drag its knobs to adjust; the shape list is under the mask's row",
        "形状已加入此蒙版——拖拽手柄可调整；形状列表在蒙版行下方"),
    ("🖌 Brush", "🖌 笔刷"),
    ("Paint a free-form mask (drag the 「Brush size」 slider, or press [ / ]); 「Apply」 bakes it into a new mask",
        "涂抹绘制自由形状蒙版（拖动下方「笔刷大小」滑杆，或按 [ / ]）；「应用」后生成新蒙版"),
    ("⌫ Erase", "⌫ 擦除"),
    ("Strokes remove from the selection instead of adding",
        "笔画从选区中移除而非添加"),
    // R22-3 (#9): the mask-brush session grew its own Brush size slider, and
    // both sliders drive the one `self.brush` — the tooltip says so.
    ("One radius for every brush: the mask brush and the Fill / Heal / Stamp brush are the same size ([ / ] move it too)",
        "所有画笔共用一个半径：蒙版画笔与填充 / 修复 / 图章的笔刷是同一个大小（[ / ] 也能调）"),
    ("✓ Apply", "✓ 应用"),
    ("🖌 Edit raster", "🖌 编辑栅格"),
    ("Brush-edit this mask: paint adds, 「Erase」 removes, 「Apply」 bakes",
        "笔刷编辑此蒙版：涂抹添加，「擦除」移除，「应用」固化"),
    ("◌ Feather", "◌ 羽化"),
    ("Soften the mask boundary one step (bakes a new raster; repeat for more)",
        "柔化蒙版边界一档（生成新栅格；可重复叠加）"),
    ("⊕ Expand", "⊕ 扩展"),
    ("Grow the selection one step (bakes a new raster)",
        "选区外扩一档（生成新栅格）"),
    ("⊖ Contract", "⊖ 收缩"),
    ("Shrink the selection one step (bakes a new raster)",
        "选区内收一档（生成新栅格）"),
    ("⇱ Full-res refine", "⇱ 全分辨率精修"),
    ("Re-cut this mask against the FULL-resolution source (guided filter). Preview-res AI masks smear their boundary at export — this snaps it to real edges. Decodes the full-size source; takes a few seconds.",
        "以「全分辨率」原图重新切割此蒙版（引导滤波）。预览分辨率的 AI 蒙版在导出时边界会发糊——此操作让边界贴合真实边缘。需解码全尺寸原图，耗时数秒。"),
    ("could not load this mask's raster ({err}) — starting from an empty brush canvas",
        "无法加载此蒙版的栅格（{err}）——从空白画布开始"),
    ("Brush mask — paint to select; 「Erase」 removes; 「Apply」 bakes it · Esc cancels",
        "笔刷蒙版——涂抹选取；「擦除」移除；「应用」固化 · Esc 取消"),
    ("nothing painted yet — drag on the image first", "还没有涂抹任何区域——请先在图上拖拽"),
    ("mask raster updated — its adjustments now apply to the edited area",
        "蒙版栅格已更新——其调整现作用于编辑后的区域"),
    ("brush mask created — pull its sliders in 「Local Masks」 (all 0 now, no visible effect yet)",
        "笔刷蒙版已创建——在「局部蒙版」中拉动其滑杆（当前全为 0，尚无可见效果）"),
    ("Brush {n}", "笔刷 {n}"),
    ("could not save the brush mask ({err})", "笔刷蒙版保存失败（{err}）"),
    ("could not edit this mask's raster ({err})", "蒙版栅格编辑失败（{err}）"),
    ("the raster-edit brush session ended — you selected another mask; its unbaked strokes were discarded",
        "栅格编辑笔刷会话已结束——你选中了另一个蒙版，未固化的涂抹已丢弃"),
    ("the mask-brush session ended — the canvas pixels underneath were replaced, so its strokes no longer line up",
        "蒙版笔刷会话已结束——其下方的画布像素已被替换，涂抹不再对齐"),
    ("the AI result took the selection — a brush session on another mask ended; its unbaked strokes were discarded",
        "AI 结果选中了它的蒙版——另一蒙版上的笔刷会话已结束，未固化的涂抹已丢弃"),
    ("Refining the mask to full resolution (decoding the full-size source) …",
        "正在将蒙版精修到全分辨率（解码全尺寸原图）……"),
    ("this source is {w}×{h} — a full-resolution refined mask would exceed the mask budget and could never be read back",
        "该原图为 {w}×{h}——全分辨率精修蒙版会超出蒙版预算，之后无法读回"),
    ("mask refined to full resolution — boundaries now follow the source's own edges",
        "蒙版已精修到全分辨率——边界现贴合原图自身边缘"),
    ("the mask changed while refining — the result was saved at {path} but not applied",
        "精修期间蒙版发生了变化——结果已保存在 {path}，但未应用"),
    ("mask refine failed", "蒙版精修失败"),
    ("Mask brush — paint to select · 「Erase」 removes · 「Apply」 bakes · Esc cancels",
        "蒙版笔刷——涂抹选取 · 「擦除」移除 · 「应用」固化 · Esc 取消"),
    // ── R25 P1: the import-side loss line (`util::xmp_import_line`) ──────────
    // Replaces 「{n} Lightroom mask(s) (brush/AI/depth) have no engine
    // equivalent…」, which counted refusals back when EVERY Lightroom mask was
    // one. The masks import now; these name what did not come with them.
    // Every glyph below was checked against the shipped CJK subset before it
    // was written — 「旋」「混」「域」「附」「杆」 all resolve; the whole-chain gate
    // (`embedded_fonts_cover_every_ui_symbol`) is what catches a tofu.
    ("Imported {n} Lightroom mask(s), {m} feature(s) not modelled",
        "已导入 {n} 个 Lightroom 蒙版，{m} 项未建模"),
    ("Rotation angle", "旋转角度"),
    // R25 P5, the import twin of the export line's angle-naming head. Same
    // fallback rule: the bare label above serves a rotation whose angle we
    // could not read (`util::rotation_degrees`).
    ("Rotation {a}° read as 0 (frame size unknown)",
        "旋转 {a}° 按 0 读入（画幅尺寸未知）"),
    ("Blend mode", "混合模式"),
    ("Extra shapes", "附加形状"),
    // R27 Batch-4 (L-08) / R29 Batch-6b. Import twin of the export line above —
    // the strokes, their dab streams and the group's blend mode all arrive and
    // round-trip, and the alpha kernel that used to be missing was MEASURED, so
    // the mask is drawn. What the label must still refuse to imply is that the
    // alpha came from Lightroom.
    ("Brush mask (drawn from our measured model)", "画笔蒙版（本机实测模型渲染）"),
    // Two sentences, not one, because they are two different states: the mask
    // rendered from OUR alpha, or it rendered nothing at all. Collapsing them
    // would let a failed model run read as a successful approximation.
    ("AI mask (re-derived locally, not Adobe's raster)",
        "AI 蒙版（本机重算，非 Adobe 原栅格）"),
    ("AI mask (carried, not yet re-derived)", "AI 蒙版（已带走，尚未重算）"),
    ("Range mask (foreign)", "值域蒙版（外部）"),
    // R25 P6: the four local point curves are modelled now, so this verdict
    // no longer means "not supported" — it means the sidecar's own curve did
    // not parse. The key moved with the meaning rather than staying a phrase
    // that reads as a missing feature.
    ("Local point curve (unreadable)", "局部点曲线（无法读取）"),
    ("Unmodelled slider", "未建模的滑杆"),
    ("AI / brush masks cannot be imported — Lightroom recomputes them from a digest",
        "AI / 画笔蒙版无法导入 —— Lightroom 会自行重新计算"),
    // NOT in the design's eight: the reader keeps TWO drop verdicts, and
    // labelling 「values we can read but that are past the model」 as an
    // AI/brush mask would be a sentence that lies. A subtract-only component
    // (`crs:MaskValue="0"`) and an exposure past ±5 EV both land here.
    ("Beyond this engine's model", "超出本引擎的模型"),

    // ── Round-10 batch: theme picker (Settings) ──────────────────────────────
    ("Theme", "主题"),
    ("Dark", "深色"),
    ("Light", "浅色"),

    // ── Round-12 L12#2B: deterministic-rationale note templates ──────────────
    // Each en key is BYTE-IDENTICAL to its `autoshade::rationale::keys` const
    // (the audit extracts that module and this table must cover it). The zh
    // value must keep the exact {placeholder} multiset — the placeholder gate
    // checks it.
    ("Reverse-fit from a target rendition (statistical match; the target is not \
      pixel-aligned, so local masks and per-band hue rotation are not recovered): luma-CDF \
      → tone sliders + residual tone curve, chroma → saturation, per-band colour mixer, \
      per-channel cast curves. Residual look error {err_before} → {err_after}.",
        "从目标成品反推（统计匹配；目标未像素对齐，故局部蒙版与分色相的色相旋转无法恢复）：亮度 CDF → 影调滑杆 + 残差影调曲线，色度 → 饱和度，逐色带颜色混合器，逐通道色偏曲线。剩余观感误差 {err_before} → {err_after}。"),
    ("Reverse-fit from a target rendition (statistical match; the target is not \
      pixel-aligned, so local masks and per-band hue rotation are not recovered): luma-CDF \
      → tone sliders (no residual curve), chroma → saturation, per-band colour mixer, \
      per-channel cast curves. Residual look error {err_before} → {err_after}.",
        "从目标成品反推（统计匹配；目标未像素对齐，故局部蒙版与分色相的色相旋转无法恢复）：亮度 CDF → 影调滑杆（无残差曲线），色度 → 饱和度，逐色带颜色混合器，逐通道色偏曲线。剩余观感误差 {err_before} → {err_after}。"),
    ("Reverse-fit from a target rendition (paired robust match on corresponding \
      pixels; local masks and per-band hue rotation are still not solved): robust paired \
      luma regression → tone sliders + residual tone curve, chroma → saturation, \
      per-band colour mixer, per-channel cast curves. Residual look error {err_before} → {err_after}.",
        "从目标成品反推（对应像素稳健配对匹配；局部蒙版与分色相的色相旋转仍不解算）：稳健配对亮度回归 → 影调滑杆 + 残差影调曲线，色度 → 饱和度，逐色带颜色混合器，逐通道色偏曲线。剩余观感误差 {err_before} → {err_after}。"),
    ("Reverse-fit from a target rendition (paired robust match on corresponding \
      pixels; local masks and per-band hue rotation are still not solved): robust paired \
      luma regression → tone sliders (no residual curve), chroma → saturation, \
      per-band colour mixer, per-channel cast curves. Residual look error {err_before} → {err_after}.",
        "从目标成品反推（对应像素稳健配对匹配；局部蒙版与分色相的色相旋转仍不解算）：稳健配对亮度回归 → 影调滑杆（无残差曲线），色度 → 饱和度，逐色带颜色混合器，逐通道色偏曲线。剩余观感误差 {err_before} → {err_after}。"),
    (" Paired convergence carried movement through one-sided hue bands \
      [{bands}]: each moved pixel was individually vouched (robust \
      weight, hue-coherent with the global edit) and moved toward its own \
      paired target pixel; unvouched pixels kept the veto.",
        " 配对收敛携带移动穿过单侧色相带 [{bands}]：每个被移动的像素都逐一获得担保（稳健权重、与全局编辑色相相干），并向其自身的配对目标像素靠近；未获担保的像素维持否决。"),
    (" Paired robust fit: {pct}% of the comparable pixels disagreed with any \
      single global develop of this source (concentrated in [{ranges}]) and \
      were down-weighted before the controls were solved.",
        " 配对稳健拟合：{pct}% 的可比像素与本源图的任何单一全局显影都不一致（集中在 [{ranges}]），已在求解控件前降权。"),
    (" Zoned {label} robust fit down-weighted {pct}% of the overlapping pixels \
      as content the two zones do not share (concentrated in [{ranges}]).",
        " 分区 {label} 稳健拟合将 {pct}% 的重叠像素降权：两区并不共有这部分内容（集中在 [{ranges}]）。"),
    ("Reverse-fit Atmosphere mode (structural divergence D={d}): the target's \
      structure cannot be reconstructed by develop controls, so only its atmosphere \
      and overall tone/colour were matched with bounded robust controls. Residual look \
      error {err_before} → {err_after}.",
        "反推氛围模式（结构差异 D={d}）：目标结构无法通过显影控制恢复，因此仅以有界的可靠控制匹配其氛围和整体影调/色调。剩余观感误差 {err_before} → {err_after}。"),
    (" NOTE: the fitted recipe still renders far from the target \
      (residual {err_after}) — this look exceeds what global \
      sliders can express; consider the AI variant itself or a zoned \
      edit.",
        " 注意：拟合配方的渲染结果仍与目标相距较远（残差 {err_after}）——此风格超出全局滑杆的表达范围；可考虑直接使用 AI 变体或分区编辑。"),
    (" Saturation demand exceeded the model cap (±60).",
        " 饱和度需求超出模型上限（±60）。"),
    (" Atmosphere-mode saturation demand exceeded its conservative cap (±{cap}).",
        " 氛围模式的饱和度需求超出其更低上限（±{cap}）。"),
    (" Atmosphere-mode confidence is capped at {cap} because develop controls cannot \
      recreate the divergent structure.",
        " 由于显影控制无法重建差异结构，氛围模式的置信度上限为 {cap}。"),
    (" Atmosphere controls were read on population evidence; structural withholding \
      of luma ranges [{luma_ranges}] and hue bands [{hue_bands}] applies to the \
      residual, zone and detail fits, not to the bounded atmosphere controls.",
        " 氛围控制依据整体证据读取；结构性保留的明度范围 [{luma_ranges}] 和色相带 [{hue_bands}] 仅适用于残差、区域和细节拟合，不适用于有界氛围控制。"),
    (" Reverse-fit used panel Strength {pct}% to derive its honesty budget.",
        " 反推使用面板 Strength {pct}% 来推导预算。"),
    (" Fit refused: the source or target frame has no tonal variation (blank or single-tone), so a statistical match would produce a constant tone map — no recipe was fitted.",
        " 拟合已拒绝：源图或目标图没有影调变化（空白或单色画面），统计匹配只会产生恒定影调映射——未生成拟合配方。"),
    (" The full fit rendered farther from the target than the untouched \
      source at every saturation level — returning a NEUTRAL recipe \
      (do-no-harm terminal case); this look is outside the global \
      model's reach.",
        " 全强度拟合在每个饱和度档位上都比未处理原图离目标更远——返回中性配方（不伤害原则的终止情形）；此风格在全局模型能力之外。"),
    (" Saturation was pulled back from the chroma-matched {sat_fitted} \
      to {sat_now} after the full-strength fit rendered farther from the \
      target than the untouched source (do-no-harm check).",
        " 饱和度已从色度匹配的 {sat_fitted} 回撤至 {sat_now}：全强度拟合的渲染结果比未处理原图离目标更远（不伤害检查）。"),
    (" Colour-cast curves were withheld: they would have re-hued a \
      region of the frame (pixel-aligned hue-damage gates).",
        " 色偏曲线已被扣留：它们会使画面某区域变色（像素对齐的色相损伤门）。"),
    (" Deep reverse-fit: the visual reviewer scored the plain solve \
      {score1}/100, so one guided retry was bought ({action}); it \
      re-scored {score2}/100 and was kept (a lower score would have been \
      discarded).",
        " 深度反推：视觉复查给未加提示的解打了 {score1}/100，于是换来一次按提示的重试（{action}）；重试复查得 {score2}/100，已采用（分数更低会被丢弃）。"),
    // ── R23-6: the reverse-fit honesty batch + the joint value-range family ──
    (" The colour stage produced NOTHING: the per-channel cast curves it \
      fitted did not improve the look by a clear enough margin to earn \
      the risk of dragging every region, so they were rejected and this \
      fit carries tone and saturation only.",
        " 色彩环节什么也没有产出：它拟合出的逐通道色偏曲线对观感的改进幅度不明显，不值得让整幅画面的每个区域都被拖动，因此被拒绝——本次反推只有影调与饱和度。"),
    (" Joint distribution check (luminance × chroma value ranges, {n} of \
      them carried evidence on both sides): area-weighted mismatch \
      {weighted}, worst range {worst} in {label}. These are VALUE ranges, \
      not areas of the picture — their pixels are spread over the whole \
      frame.",
        " 联合分布检查（亮度 × 色度值域桶，其中 {n} 个在两侧都有充分证据）：按覆盖比例计算的失配 {weighted}，最差值域 {worst}（{label}）。这些是数值区间而不是画面上的区域——桶内像素分布在整幅画面各处。"),
    (" The fit tried the supported controls but did not reach the target: \
      the two images still differ inside matching value ranges, which the \
      single residual number above cannot show; treat this fit as a starting \
      point, not a match.",
        " 拟合尝试了有证据支持的控制项，但仍未达到目标：两张图在相同的数值区间内仍有差异，而上面的单一残差数字显示不出这一点；请把本次反推当作起点，而不是匹配结果。"),
    (" The fit withheld supported movement because the evidence was one-sided, \
      so this result is deliberately farther from the target; this is a \
      refusal, not a miss.",
        " 拟合暂不移动有证据支持的控制项，因为证据只在一侧，所以结果有意更远离目标；这是拒绝，不是未命中。"),
    (" (the refusal came from the joint distribution check, not the \
      residual: the fitted recipe pushed the value ranges further apart \
      than leaving the photo alone)",
        " （这次拒绝来自联合分布检查而非残差：拟合配方让各数值区间比不做任何处理时相距更远）"),
    (" (the joint distribution check found no value range with enough \
      evidence on both sides, so it has no opinion on this pair; confidence \
      remains capped by the pair's shared-evidence identifiability)",
        " （联合分布检查没有找到两侧都有足够证据的数值区间，因此它对本配对没有意见；置信度仍受配对共享证据可识别性上限约束）"),
    (" Fit refused because the evidence contradicted the proposed correction: the measurable value ranges moved farther apart, so the recipe was reset to the untouched calibration.",
        " 证据与拟议修正相矛盾：可测量的数值区间变得更加分离，因此配方已重置为未处理的调校基础。"),
    (" Fit withheld because nothing measurable was available here: no shared value range had enough evidence on both sides, so no correction was fitted.",
        " 未找到可测量的共享证据：没有任何数值区间在两侧拥有足够证据，因此未拟合任何修正。"),
    (" Detail evidence fitted clarity {clarity} and texture {texture} within the +/-20 budget; their high-frequency reading used only identifiable pixels.",
        " 细节证据在 +/-20 限额内拟合了清晰度 {clarity} 和纹理 {texture}；高频测量仅使用可识别像素。"),
    (" Detail controls were withheld: two-sided structural and luma-range evidence did not support a safe global detail move, so clarity and texture were not moved.",
        " 细节控制暂不调整：两侧结构与亮度范围的证据不足，因此未移动清晰度与纹理。"),
    (" Per-band colour mixer, solved from each band's own population: [{moved}]. Hue rotation is never solved, so every band's hue stays 0. Bands left neutral for want of two-sided population evidence: [{refused}].",
        " 逐带颜色混合器按各色带自身的人口统计求解：[{moved}]。色相旋转从不求解，因此每个色带的色相保持 0。因两侧人口证据不足而保持中性的色带：[{refused}]。"),
    (" The per-band colour move was given back: applying it did not leave the frame closer to the target, so every band returned to neutral.",
        " 逐带颜色调整已交还：应用后画面并未更接近目标，因此所有色带恢复中性。"),
    (" The per-band colour move was given back: it would have carried pixels through hue bands no two-sided evidence covers, and blind movement is vetoed rather than shipped.",
        " 逐带颜色调整已交还：它会让像素穿过没有两侧证据覆盖的色相带，盲目移动会被否决而不会交付。"),
    (" Evidence gating withheld luma ranges [{luma_ranges}] and hue bands [{hue_bands}]. One-sided [{one_sided}] is UNMEASURABLE, not equal, so it vetoed movement. Sparse on both sides [{sparse}] was excluded from estimation but did not veto a move. Structurally divergent [{divergent}] also vetoed movement.",
        " 证据门控暂不调整亮度 [{luma_ranges}] 和色相 [{hue_bands}]，单侧范围 [{one_sided}] 无法测量，两侧证据稀疏 [{sparse}]，结构差异范围 [{divergent}] 也被阻止。"),
    (" Zoned {label} colour controls withheld: they would move zero-evidence hue bands [{hue_bands}]. Those bands were not adjusted blindly.",
        " 分区 {label} 的色彩控件暂不调整：它们会移动零证据的色相 [{hue_bands}]，因此未盲目调整。"),
    (" Zoned {label} tone controls withheld: they would move zero-evidence luma ranges [{luma_ranges}]. Those ranges were not adjusted blindly.",
        " 分区 {label} 的影调控件暂不调整：它们会移动零证据的亮度范围 [{luma_ranges}]，因此未盲目调整。"),
    (" No zoned correction attached: the source and target zone shares differ by more than 2:1, so neither population is a comparable measurement of the same subject.",
        " 未附加任何分区修正：源图与目标的分区占比之比超过 2:1，两侧不是对同一主体的可比测量。"),
    (" This target's look appears to use {controls}, which the reverse-fit \
      never solves for (its solution space is exposure/contrast/\
      highlights/shadows/whites/blacks, a tone curve, one global \
      saturation, clarity/texture, an evidence-gated per-band colour mixer that never \
      rotates hue, and the three channel curves) — that part of the look \
      cannot arrive through this route.",
        "此目标的风格似乎使用了 {controls}；反推不会求解这些控制项，因此这部分风格无法通过此路径得到。"),
    (" This target's remaining look appears to need {controls}; Atmosphere mode only \
      solves bounded exposure, white balance, a robust five-point tone curve, \
      saturation, an evidence-gated per-band colour mixer that never rotates hue, \
      and evidence-gated clarity/texture, so that part cannot arrive through this route.",
        "此目标剩余的观感可能需要 {controls}；氛围模式只求解有界曝光、白平衡、稳健五点曲线、饱和度、不旋转色相的证据门控逐带颜色混合器，以及证据门控的细节，因此这部分无法通过此路径得到。"),
    (" WARNING: the reference's proportions do not match this photo's — it \
      was CROPPED, or it is not the same frame. Either way the two \
      distributions are not comparable, because a crop changes which pixels \
      the statistics are taken over. The fit matched them anyway, as it was \
      asked to — treat the result as unreliable.",
        " 警告：参考图与本图的比例不一致——它被裁切过，或者不是同一张。两者都会让两边的分布无法相比，因为裁切会改变统计所覆盖的像素。反推仍按要求做了匹配——结果请视为不可靠。"),
    (" Zoned sky fit unavailable ({e}) — trying the automatic luminance-range fallback.",
        " 分区天空拟合不可用（{e}）——正在尝试自动亮度范围回退。"),
    (" Multi-region semantic corrections refused after the final comparison: the multi-region frame residual {multi} was no better than the seeded two-region residual {two}, so the byte-identical two-region result was kept. Trialled regions: {regions}.",
        " 多区域语义校正在最终比较后被拒绝：多区域残差 {multi} 不优于双区域残差 {two}，因此保留字节相同的双区域结果。试验区域：{regions}。"),
    (" Multi-region semantic segmentation unavailable ({e}) — the historical sky/land pass was used instead.",
        " 多区域语义分割不可用（{e}）——改用历史的天空/地面流程。"),
    (" No semantic region cleared the shared support floor on both frames (up to {n} requested) — the historical sky/land pass was used instead.",
        " 没有语义区域在两侧同时达到共享的支持下限（最多请求 {n} 个）——改用历史的天空/地面流程。"),
    (" The {label} region was refused by its boundary-continuity gate ({why}): candidate rim {before} luma against budget {max} ({transitions} measured transitions).",
        " {label} 区域被其边界连续性门拒绝（{why}）：候选边缘 {before} luma，预算 {max}（测得 {transitions} 处过渡）。"),
    (" Zoned fit skipped: no usable sky partition (sky covers {s}% \
      of the source frame, {t}% of the target's).",
        " 分区拟合已跳过：没有可用的天空分割（天空占原图 {s}%、目标图 {t}%）。"),
    (" The {label} zone covers too little of the frame (source {s}%, \
      target {t}%) — skipped.",
        " {label} 区占画面比例过小（原图 {s}%、目标 {t}%）——已跳过。"),
    (" The {label} structural divergence is D={d}; Full zone fit selected.",
        " {label} 区的结构差异为 D={d}；已选择完整分区拟合。"),
    (" The {label} structure cannot be reconstructed by develop controls; \
      matching atmosphere only (D={d}).",
        " {label} 结构无法通过配方恢复，只匹配氛围（D={d}）。"),
    (" Local quality gate passed for {label}: texture ratio {texture}, clipped \
      share {clip_before}% → {clip_after}%.",
        " {label} 区已通过局部质量门：纹理比 {texture}，裁切占比 {clip_before}% → {clip_after}%。"),
    (" Zoned {label} correction dropped by the local-quality texture gate: \
      ratio {ratio} is outside [{min}, {max}].",
        " {label} 区校正被局部质量纹理门弃用：比值 {ratio} 超出 [{min}, {max}]。"),
    (" Zoned {label} correction dropped by the local-quality clipping gate: \
      clipped share {before}% → {after}% (allowed growth {growth} percentage point).",
        " {label} 区校正被局部质量裁切门弃用：裁切占比 {before}% → {after}%（允许增加 {growth}%）。"),
    (" Zoned {label} correction attached ({label}-to-{label} moments → \
      local exposure {ev} EV, colour gains [{g0} {g1} {g2}], \
      saturation {sat}): zone residual {before} → {after}. The correction \
      is a BITMAP mask — rendered in-app; the Lightroom sidecar \
      carries the global fit only (classic XMP cannot hold raster \
      masks).",
        " 已附加 {label} 区校正（{label} 对 {label} 矩 → 局部曝光 {ev} EV、色彩增益 [{g0} {g1} {g2}]、饱和度 {sat}）：区残差 {before} → {after}。该校正是位图蒙版——仅应用内渲染；Lightroom 边车只携带全局拟合（经典 XMP 无法承载栅格蒙版）。"),
    (" Note: the {label} zone covers {s}% of the source frame \
      but {t}% of the target's — the compositions differ, so the \
      overall distribution residual stays where the global fit \
      left it.",
        " 注：{label} 区占原图 {s}% 而占目标图 {t}%——两者构图不同，整体分布残差停留在全局拟合的水平。"),
    (" Zoned {label} correction dropped: zone residual {before} → {after} \
      (needs ≤ {ratio}% of the original, or ≤ {floor} with a ≥ {gain}% \
      gain) with frame-global drift {drift} (tolerance {tol}).",
        " 已弃用 {label} 区校正：区残差 {before} → {after}（需 ≤ 原值的 {ratio}%，或 ≤ {floor} 同时增益 ≥ {gain}%），全画面漂移 {drift}（容差 {tol}）。"),
    (" Zoned {label} atmosphere correction dropped: zone residual {before} → \
      {after} did not satisfy do-no-harm, or frame-global drift {drift} exceeded \
      tolerance {tol}.",
        " 已弃用 {label} 区氛围校正：区残差 {before} → {after} 未通过不伤害原则，或全画面漂移 {drift} 超出容差 {tol}。"),
    (" The {label} zone already matches the target (zone residual \
      {before}) — no correction needed.",
        " {label} 区已与目标匹配（区残差 {before}）——无需校正。"),
    (" Confidence for this fit comes from the {n} zone correction(s) that \
      were actually accepted (worst zone residual {worst}), not from the \
      frame-wide residual {frame} — a frame-wide distribution cannot \
      judge a zone whose share of the two frames differs.",
        " 本次拟合的置信度来自实际被接受的 {n} 个分区校正（最差区残差 {worst}），而不是来自全画面残差 {frame}——当某个区在两张图中所占比例不同时，全画面分布无法判断它。"),
    (" {label} attached for luminance [{lo}, {hi}] (local exposure {ev} EV, \
      colour gains [{g0} {g1} {g2}], saturation {sat}): band residual \
      {before} → {after}. The sentinel-hosted luminance range is native in \
      the Lightroom sidecar.",
        " 已附加 {label}，亮度 [{lo}, {hi}]（局部曝光 {ev} EV、色彩增益 [{g0} {g1} {g2}]、饱和度 {sat}）：范围残差 {before} → {after}。由全画面 LINEAR 承载的亮度范围会原生写入 Lightroom 边车。"),
    (" Luminance range [{lo}, {hi}] abstained: {reason}.",
        " 亮度范围 [{lo}, {hi}] 已放弃：{reason}。"),
    (" Luminance range [{lo}, {hi}] merged into [{into_lo}, {into_hi}] \
      {why}; both runs have sign {sign}.",
        " 亮度范围 [{lo}, {hi}] 已合并到 [{into_lo}, {into_hi}]（{why}）；两段符号相同，为 {sign}。"),
    (" Range boundary-continuity gate kept {n} correction(s): signed \
      transition rim {before} to {after} luma after shared \
      direction-preserving shrink k={k} (budget {max}, {transitions} \
      measured crossings); the delivered tone order falls back by at most \
      {reversal} luma against the {rev_max} allowed.",
        " 范围边界连续性门保留了 {n} 个校正：共享保方向收缩 k={k} 后，有符号过渡边缘亮度由 {before} 变为 {after}（预算 {max}，测得 {transitions} 个交叉）；交付的色调顺序最大回落 {reversal} 亮度（允许 {rev_max}）。"),
    (" Range corrections refused by the boundary-continuity gate: candidate \
      rim {before} luma, and even zero differential left {after} (budget \
      {max}, {transitions} measured crossings); the delivered tone order \
      falls back by at most {reversal} luma against the {rev_max} allowed.",
        " 范围校正被边界连续性门拒绝：候选边缘亮度为 {before}，即使差分归零仍为 {after}（预算 {max}，测得 {transitions} 个交叉）；交付的色调顺序最大回落 {reversal} 亮度（允许 {rev_max}）。"),
    (" Range corrections refused after the final boundary pass: the \
      composed frame residual {after} exceeded the global-only residual \
      {global} plus tolerance {tol}, so all {n} range correction(s) were removed.",
        " 最终边界检查后已拒绝范围校正：合成全画面残差 {after} 超过仅全局残差 {global} 加容差 {tol}，因此移除了全部 {n} 个范围校正。"),
    (" Confidence for this fit includes the {n} accepted luminance-range \
      correction(s) (worst band residual {worst}); the final frame residual \
      is {frame}.",
        " 本次拟合的置信度包含 {n} 个已接受的亮度范围校正（最差范围残差 {worst}）；最终全画面残差为 {frame}。"),
    (" Local-field ceiling: global {global}, ceiling {ceiling}, realized \
      {realized}, saturated vertices {saturated}, CG iterations {iterations}.",
        " 局部场上限：全局 {global}，上限 {ceiling}，已实现 {realized}，饱和顶点 {saturated}，CG 迭代 {iterations}。"),
    (" Local-field shape: R2 tiles {r2_tiles}, R2 linear {r2_linear}, verdict \
      {shape}, effective tile cap {cap}, structured bins [{structured}].",
        " 局部场形状：分块 R2 {r2_tiles}，线性 R2 {r2_linear}，判定 {shape}，有效分块上限 {cap}，结构化桶 [{structured}]。"),
    (" Local-field band skipped: bin {bin}, dispersion {dispersion}/255, \
      maximum {max}/255.",
        " 局部场亮度段已跳过：桶 {bin}，离散度 {dispersion}/255，上限 {max}/255。"),
    (" Local-field realized after {producer}: frame {err_after}, ceiling \
      {ceiling}, share {realized}.",
        " 局部场在 {producer} 后的实现量：全画面 {err_after}，上限 {ceiling}，比例 {realized}。"),
    (" Local-field stop after {producer}: skipped [{skipped}], margin {margin}.",
        " 局部场在 {producer} 后停止：跳过 [{skipped}]，余量 {margin}。"),
    (" Field mask {n} proposed: {sign} m={mass} s={share_src}/{share_tgt} D={d} p={pixels}.",
        " 自由形状场蒙版 {n} 已提出：符号 {sign}，质量 {mass}，证据占比为源图 {share_src}、目标图 {share_tgt}，D={d}，{pixels} 个像素。"),
    (" Field mask {n} attached: {err_before}->{err_after}, cross-boundary step \
      {step}, context-charged {charged} (bitmap/XMP loss).",
        " 自由形状场蒙版 {n} 已作为引擎位图附加：合成全画面 {err_before} -> {err_after}，跨边界跳变 {step}，语境计费读数 {charged}。经典 XMP 会以已命名的位图蒙版损失不包含此校正。"),
    (" Field mask component(s) {n} refused: {why}.",
        " 自由形状场蒙版分量 {n} 已拒绝：{why}。"),
    (" No tone move was solved for {label}: its own population covers \
      {knots} of the 8 tone knots, and a slider solve needs at least 2.",
        " 未为 {label} 求解色调移动：它自身的人口只覆盖 8 个色调节点中的 {knots} 个，而滑块求解至少需要 2 个。"),
    (" Field mask candidate {n} was not offered: an accepted spatial tile \
      already covers it ({filter}), p={pixels} s={share_src}/{share_tgt}.",
        " 自由形状场蒙版候选 {n} 未提出：已被接受的空间图块覆盖（{filter}），{pixels} 个像素，证据占比为源图 {share_src}、目标图 {share_tgt}。"),
    (" No field mask qualified: {why}.",
        " 没有符合条件的自由形状场蒙版：{why}。"),
    (" [revision round {round} failed ({e}) — keeping the previous verified proposal]",
        " [第 {round} 轮修订失败（{e}）——保留上一轮已验证的提案]"),
    (" [verification of revision round {round} failed ({e}) — keeping the previous \
      verified proposal]",
        " [第 {round} 轮修订的验证失败（{e}）——保留上一轮已验证的提案]"),
    // ── R23-1: the vision proposer's own HSL-length repair (openai.rs) ───────
    (" [the proposal's 8-band colour mixer arrived with the wrong number of values \
      ({axes}) — the missing bands were read as neutral 0 and any extra ones dropped, \
      so the rest of the proposal was kept]",
        " [提案的 8 段色彩混合器数值个数不对（{axes}）——少掉的色段按中性 0 读取，多出的已丢弃，提案其余部分保留]"),
    // ── R20 visual judge closed loop (pipeline.rs) ───────────────────────────
    (" [AI visual review: {score}/100 — {critique}]",
        " [AI 视觉复查：{score}/100——{critique}]"),
    (" [AI visual review: {score1}/100 first; a guided revision re-scored \
      {score2}/100 and was adopted — {critique}]",
        " [AI 视觉复查：首次 {score1}/100；按其提示修订后复查 {score2}/100，已采用——{critique}]"),
    (" [AI visual review: {score1}/100 — {critique}; the guided revision \
      re-scored lower ({score2}/100) and was discarded (do-no-harm)]",
        " [AI 视觉复查：{score1}/100——{critique}；提示修订后得分更低（{score2}/100），已弃用（do-no-harm）]"),
    (" [AI visual review: {score}/100 — {critique}; the guided revision \
      returned the same recipe — keeping it]",
        " [AI 视觉复查：{score}/100——{critique}；提示修订返回了相同配方，已保留]"),
    (" [AI visual review: {score}/100 — {critique}; the guided revision \
      round failed ({e}) — keeping the reviewed develop]",
        " [AI 视觉复查：{score}/100——{critique}；提示修订轮失败（{e}）——保留已复查的提案]"),
    (" [AI visual review: {score}/100 — {critique}; the guided revision \
      could not be re-judged ({e}) and was discarded (do-no-harm)]",
        " [AI 视觉复查：{score}/100——{critique}；修订结果无法复查（{e}），已弃用（do-no-harm）]"),
    (" [AI visual review unavailable ({e}) — the develop was not visually \
      checked]",
        " [AI 视觉复查不可用（{e}）——本次显影未经视觉检查]"),
    // ── R23-4: one intermediate round of the multi-round convergence loop ───
    (" [AI visual review round {round}: {score1}/100 → a guided revision \
      re-scored {score2}/100 and was adopted; still under the {target}/100 \
      target, so another round was bought]",
        " [AI 视觉复查第 {round} 轮：{score1}/100 → 按提示修订后复查 {score2}/100，已采用；仍低于 {target}/100 的目标，因此继续下一轮]"),
    // ── R23-4: deep thinking — the three single-sentence fields ─────────────
    (" [deep thinking — what it saw: {scene}]",
        " [深度思考——它读到的画面：{scene}]"),
    (" [deep thinking — the look it aimed for: {look}]",
        " [深度思考——它想要的成片效果：{look}]"),
    (" [deep thinking — its own critique against your strength target: {critique}]",
        " [深度思考——它对照你的强度目标做的自查：{critique}]"),
    // ── R23-1b: the pixel-tool suggestions (advice only, nothing was run) ────
    (" [it also suggests the pixel tools (nothing was run — these are for you to \
      choose): {tools}]",
        " [它还建议用这些像素工具（未运行任何操作——是否使用由你决定）：{tools}]"),
    (" [style distillation then pulled this recipe toward this user's past \
      edits (effective strength {pct}%; moved: {fields}) — final values can \
      differ from the derivation above]",
        " [风格蒸馏随后将本配方拉向该用户的历史编辑（有效强度 {pct}%；被拉动的项：{fields}）——最终数值可能与上文推导不同]"),
    (" [your style library was kept as BACKGROUND for this develop — the direction \
      leads at adherence tier {tier}, so no style-distillation pull was applied to \
      these numbers; lower the Adherence dial to 40% or below to hand the library \
      back the lead]",
        " [本次显影只把你的风格库当作参照——指令在遵循度档位 {tier} 上主导，因此没有对这些数值做风格蒸馏拉动；把遵循度（Adherence）滑杆调到 40% 或以下即可把主导权交回风格库]"),
    (" [re-verification after style distillation failed ({e}) — the verdict \
      above describes the PRE-distillation recipe]",
        " [风格蒸馏后的复验失败（{e}）——上方判词描述的是蒸馏前的配方]"),
    (" [style reference unavailable ({e}) — the Style slider had no effect on this \
      develop; rebuild it with: autoshade style-index <folder>]",
        " [风格参考不可用（{e}）——本次显影中风格滑杆未起作用；用 autoshade style-index <文件夹> 重建]"),
    (" [no style reference was available for this photo — the Style slider ({pct}%) had \
      no effect on this develop. Build your style library in the AI panel → Style \
      reference library: a folder of your own RAWs with their Lightroom .xmp sidecars \
      beside them]",
        " [本片没有可用的风格参考——风格滑杆（{pct}%）在这次显影中未起作用。请在 AI 面板 → 风格参考库里建库：一个放着你自己的 RAW、每个旁边带 Lightroom .xmp 的文件夹]"),
    (" [style reference: your own edits on {files} — the {n} most similar shots in your \
      style library]",
        " [风格参考：你自己对 {files} 的编辑——风格参考库里最接近的 {n} 张]"),
    (" [{file} also went to the vision model as a reference IMAGE — one extra image on \
      each call of this analysis]",
        " [{file} 还作为参考图一并给了视觉模型——本次分析的每次调用都多一张图]"),
    (" [the reference photo could not be prepared ({e}) — this develop used the text \
      reference only]",
        " [参考图未能准备好（{e}）——本次显影只用了文字参考]"),
    ("\n⚠ the response did not preserve mask identities (a mask was renamed or \
      duplicated) — your masks were kept unchanged and the model's mask edits were \
      discarded",
        "\n⚠ 响应未保留蒙版身份（有蒙版被改名或复制）——你的蒙版保持原样，模型的蒙版编辑已丢弃"),
    ("Heuristic baseline (AI vision unavailable (untrusted provider diagnostic): {e}). \
      mean_luma={mean}/255, clip black/white={clip_b}%/{clip_w}% → exposure {ev}EV, \
      highlights {hl}, shadows {sh}.",
        "启发式基线（AI 视觉不可用（不受信提供方诊断）：{e}）。mean_luma={mean}/255，黑/白裁剪={clip_b}%/{clip_w}% → 曝光 {ev}EV、高光 {hl}、阴影 {sh}。"),
    ("Heuristic baseline (no AI vision; OPENAI_API_KEY unset). \
      mean_luma={mean}/255, clip black/white={clip_b}%/{clip_w}% → exposure {ev}EV, \
      highlights {hl}, shadows {sh}.",
        "启发式基线（无 AI 视觉；OPENAI_API_KEY 未设置）。mean_luma={mean}/255，黑/白裁剪={clip_b}%/{clip_w}% → 曝光 {ev}EV、高光 {hl}、阴影 {sh}。"),
    ("AI spot-detection failed ({e}); healed the painted mask only.",
        "AI 斑点检测失败（{e}）；仅修复了手绘蒙版。"),
    ("; ", "；"),
    ("healed {n} of {total} painted region(s) — the rest exceeded the retouch budget \
      ({max_spots} regions / {max_bbox}x bbox / {max_disk}x \
      heal coverage) and were left UNTOUCHED; paint fewer or smaller regions",
        "已修复 {n}/{total} 个手绘区域——其余超出修饰预算（{max_spots} 区 / {max_bbox}x 包围盒 / {max_disk}x 修复覆盖），保持未动；请少画或画小一些"),

    // ── Round-12 L12#4: reverse-fit landing facts rendered at landing ────────
    (" · XMP → {path}", " · XMP → {path}"),
    (" · ⚠ {note}", " · ⚠ {note}"),

    // ── Round-12 L12#3: tofu disclosure for uncovered writing scripts ────────
    ("some file names use characters no installed font can draw ({sample}) — they show as boxes",
        "部分文件名使用了已安装字体无法绘制的字符（{sample}）——它们会显示为方块"),

    // ── Round-12 阶段4: export format + depth (ExportFormat::label) ──────────
    // "16-bit TIFF" already exists above; "JPEG" is allow-listed jargon.
    ("8-bit TIFF", "8 位 TIFF"),
    ("16-bit PNG", "16 位 PNG"),
    ("8-bit PNG", "8 位 PNG"),
    // Identical on purpose: format jargon (also in the bypass allow-list),
    // but the label() extractor demands the pair exist.
    ("JPEG", "JPEG"),

    // ── Round-13 easter eggs (user request: 讽刺 Adobe 的拉跨技术和昂贵定价) ──
    ("skymanbp's AS — the “As” stands for AutoShade, not an Adobe subscription. Rent paid to date: $0.00.",
        "skymanbp's AS——「As」是 AutoShade，不是 Adobe 订阅。迄今已缴月租：$0.00。"),
    ("Fun fact: this empty state finished loading while Photoshop's splash screen would still be painting its clouds.",
        "冷知识：这个空页面加载完的时候，Photoshop 的启动画面还没画完那朵云。"),
    ("Every shortcut above ships free — no Creative tier, no Cloud, no monthly ransom.",
        "以上快捷键全部免费——没有 Creative 档位，没有 Cloud，没有每月赎金。"),
    // ── R22 #4/#14b: AI panel (analysis / whole-image / reverse-fit) ──────────
    //    + the develop panel's group captions
    (" Boundary-continuity gate kept {n} zoned correction(s): introduced transition \
      rim {before} to {after} luma after shared differential shrink k={k} \
      (budget {max}, {transitions} measured transitions).",
        " 分区边界连续性门保留了 {n} 个分区校正：共享差异收缩 k={k} 后，引入的过渡边缘亮度由 {before} 变为 {after}（预算 {max}，测量了 {transitions} 个过渡）。"),
    (" Zoned corrections dropped by the boundary-continuity gate: candidate \
      rim {before} luma, and even shared shrink k=0 left {after} \
      (budget {max}, {transitions} measured transitions).",
        " 分区校正被边界连续性门弃用：候选边缘亮度为 {before}，且共享收缩 k=0 时仍为 {after}（预算 {max}，测量了 {transitions} 个过渡）。"),
    // v1.2.2 shipped these four English sentences with no zh entry, so the
    // GUI fell back to English mid-paragraph for a Chinese reader. The
    // "kept" line above is the same note whose wording moved from "signed"
    // to "introduced" in that batch, which orphaned its old translation.
    (" Zoned {n} correction(s) dropped by the boundary-continuity gate: \
      candidate introduced rim {before} luma, and the largest shrink inside \
      budget {max} was k={k}, whose render is byte-identical to the frame \
      without it — reading {after} over {transitions} measured transitions. \
      An inert attachment would occupy the correction budget and disclose a \
      change it did not make.",
        " {n} 个分区校正被边界连续性门弃用：候选引入的边缘亮度为 {before}，而预算 {max} 之内最大的收缩为 k={k}，其渲染结果与不含该校正的画面逐字节相同——读数为 {after}，测量了 {transitions} 个过渡。无效的附加会占用校正预算，并报告一项它其实没有做出的改动。"),
    (" No zoned {label} correction attached: every control that survived \
      the evidence and quality gates solved to neutral, so the zone \
      residual {before} is left uncorrected.",
        " 未附加 {label} 分区校正：通过证据门与质量门的每一项控制都解算为中性，因此 {before} 的分区残差保持未校正。"),
    (" The residual above was measured on this camera's embedded rendition, \
      which is the frame the fit and the review both saw. The delivered \
      render additionally applies this photo's own calibration ({what}), so \
      it is a different frame — closer to the target in luma, and differing \
      in chroma, because the camera curve is matched on luma alone.",
        " 上述残差是在本相机的嵌入渲染上测得的，那正是拟合与复查共同依据的画面。交付渲染还会额外应用这张照片自己的校准（{what}），因此那是另一幅画面——在亮度上更接近目标，在色度上则有所不同，因为相机曲线只按亮度匹配。"),
    ("Analysis", "分析"),
    ("Pixel-level AI tools stay at their tools: select subject / select sky in Local Masks, denoise in Detail, heal and fill in Retouch.",
        "像素级 AI 工具就近放置：选主体 / 选天空在局部蒙版，降噪在细节，修复填充在修饰。"),
    ("More AI features are in the AI area at the top of this panel",
        "更多 AI 功能都在本面板上方的 AI 区"),
    ("Tone & Colour", "影调与色彩"),
    ("Detail & Lens", "细节与镜头"),
    ("Local & Pixel", "局部与像素"),
    ("Versions & Export", "版本与导出"),
    ("Brush (shared)", "画笔（共用）"),

    // ── Reimagine · generation-side fidelity reading (step 6) ────────────────
    ("auto-retry once if the result diverges", "结果结构偏离时自动重试一次"),
    ("After generating, the structural divergence D vs the original is measured. \
     If D ≥ {limit} (the reverse-fit's atmosphere threshold), buy ONE more \
     generation — a second paid image — and keep the closer result. \
     Off = never spend extra.",
        "生成后会测量结果与原图的结构差异 D。若 D ≥ {limit}（反推的氛围模式阈值），\
         则再买一张——第二次付费生成——并保留更贴合的一张。关闭 = 绝不额外花钱。"),
    (" · ⚠ structure diverged from the original (D={d} ≥ {limit}) — a reverse-fit will fall back to atmosphere mode",
        " · ⚠ 结构偏离原图（D={d} ≥ {limit}）——反推将退回氛围模式"),
    (" · faithful to the frame (structural divergence D={d})",
        " · 贴合原图（结构差异 D={d}）"),
    (" · the opt-in retry kept the closer of two results (discarded D={d0})",
        " · 自选重试保留了两张中更贴合的一张（弃用 D={d0}）"),

    // ── Reverse-fit · cross-image correspondence (step 7b) ──────────────────
    (" Spatial tile {id} eligible in derivation {generation}: frozen evidence \
      shares source {s}, target {t}, original D={d}, signed residual {residual} \
      (95% CI +/-{ci}, parent {parent}).",
        " 空间图块 {id} 在第 {generation} 次推导中符合条件：冻结证据占比为源图 {s}、目标图 {t}，原始 D={d}，有符号残差 {residual}（95% 置信区间 +/-{ci}，上一层 {parent}）。"),
    (" Spatial tile {id} attached as an engine bitmap: local residual \
      {before} -> {after}, composed frame {frame_before} -> {frame_after}, \
      boundary {boundary}. Classic XMP omits this correction with the named \
      bitmap-mask loss.",
        " 空间图块 {id} 已作为引擎位图附加：局部残差 {before} -> {after}，合成画面 {frame_before} -> {frame_after}，边界 {boundary}。经典 XMP 会以具名位图蒙版损失跳过此校正。"),
    (" Spatial tile {id} abstained in derivation {generation} ({reason}): \
      frozen evidence shares source \
      {s}, target {t}, original D={d}, signed residual {residual} (95% CI \
      +/-{ci}, parent {parent}).",
        " 空间图块 {id} 在第 {generation} 次推导中放弃（{reason}）：冻结证据占比为源图 {s}、目标图 {t}，原始 D={d}，有符号残差 {residual}（95% 置信区间 +/-{ci}，上一层 {parent}）。"),
    (" Spatial sweep {generation}: eligible parent nodes {eligible}; \
      abstentions by source share {s}, target share {t}, structural \
      divergence {d}, confidence interval {ci}, parent proximity {parent}, \
      other {other}.",
        " 空间扫描第 {generation} 代：符合条件的父节点 {eligible}；放弃——源占比 {s}、目标占比 {t}、结构分歧 {d}、置信区间 {ci}、贴近上层 {parent}、其他 {other}。"),
    (" Spatial traversal stopped at depth {depth} with a {cap}-tile attachment \
      cap; {attached} tile(s) attached.",
        " 空间推导在深度 {depth} 停止，附加上限为 {cap} 个图块；实际附加 {attached} 个。"),
    (" Spatial tile {id} passed the boundary gate: cross-boundary step \
      {before} -> {after} luma after direction-preserving shrink k={k}, \
      context-charged {charged} (ceiling {max}, {transitions} measured crossings).",
        " 空间图块 {id} 通过边界门控：保持方向缩减 k={k} 后，跨边界跳变由 {before} -> {after} 亮度，语境计费读数 {charged}（上限 {max}，测得 {transitions} 处穿越）。"),
    (" Spatial tile {id} refused by its boundary/composed-frame gate: \
      candidate step {before}, final reading {after}, context-charged \
      {charged}, ceiling {max} ({transitions} measured crossings, k={k}).",
        " 空间图块 {id} 被边界/合成画面门控拒绝：候选跳变 {before}，最终读数 {after}，语境计费读数 {charged}，上限 {max}（测得 {transitions} 处穿越，k={k}）。"),
    (" Guided mask refinement kept for {label}: coverage delta {coverage}, \
      guide-edge alignment {before} -> {after}, core pixels changed {core}.",
        " {label} 的引导蒙版细化已保留：覆盖变化 {coverage}，引导边缘对齐 {before} -> {after}，内部像素变化 {core}。"),
    (" Guided mask refinement abstained for {label}: coverage delta {coverage}, \
      guide-edge alignment {before} -> {after}, core pixels changed {core}; \
      the original mask bytes were retained.",
        " {label} 的引导蒙版细化已放弃：覆盖变化 {coverage}，引导边缘对齐 {before} -> {after}，内部像素变化 {core}；保留原始蒙版字节。"),
    (" · includes {n} spatial tile correction(s) (adjustable in the mask panel; omitted from classic XMP with a named bitmap loss)",
        " · 包含 {n} 个空间图块校正（可在蒙版面板调整；经典 XMP 会以具名位图损失跳过）"),
    (" · guided mask refinement: {kept} kept, {abstained} abstained",
        " · 引导蒙版细化：保留 {kept} 个，放弃 {abstained} 个"),

    (" Atmosphere white balance and exposure were solved from WHOLE-FRAME per-channel \
      weighted medians of both sides. That pairs the two frames as distributions, which \
      assumes both describe the same content — the assumption this mode is selected \
      precisely because it does not hold.",
        " 氛围模式的白平衡与曝光解自两侧的整帧逐通道加权中位数。这是把两张画面当作分布来配对，\
         其前提是两侧描述同一批内容——而正是因为该前提不成立，才选用了这个模式。"),
    (" Of that reference population, {share}% of the target has no confident counterpart \
      in the source (cross-image correspondence below {tau}, read on the sidecar's {grid} \
      cell grid, so this is a coarse share) — and defined those two controls all the same.",
        " 在该参照人口中，目标画面有 {share}% 在源图中没有可信对应（跨图对应低于 {tau}，\
         读自侧车的 {grid} 网格，故为粗粒度占比）——它们同样参与了上面两项控件的定义。"),
    (" Of the whole-frame reference population, {share}% of the target has no confident \
      counterpart in the source (cross-image correspondence below {tau}, read on the \
      sidecar's {grid} cell grid, so this is a coarse share) — and was EXCLUDED from \
      those two controls rather than left to define them.",
        " 在整帧参照人口中，目标画面有 {share}% 在源图中没有可信对应（跨图对应低于 {tau}，\
         读自侧车的 {grid} 网格，故为粗粒度占比）——这部分已被排除在上面两项控件之外，\
         而不是任其参与定义。"),
    (" Atmosphere white balance and exposure were solved from the SHARED-CONTENT \
      population of the two frames rather than from the whole frame: target pixels no \
      confident source pixel answers for are generated content that is not a rendition \
      of this frame, and source pixels whose content the target replaced have nothing \
      left to be compared with (cross-image correspondence below {tau} on either side), \
      so both were dropped before the per-channel weighted medians were read. That left \
      {src}% of the source's evidence mass and {tgt}% of the target's, and the two \
      distributions being paired now describe the same content.",
        " 氛围模式的白平衡与曝光解自两帧的「共有内容」人口，而非整帧：目标中没有任何可信源\
         像素为之作答的像素是生成内容，并非本帧的一种呈现；源中被目标替换掉的像素则已无物\
         可比（任一侧跨图对应低于 {tau}）——两者都在读取逐通道加权中位数之前被剔除。余下源\
         证据质量的 {src}% 与目标证据质量的 {tgt}%，被配对的这两个分布现在描述的是同一批\
         内容。"),
    (" The shared-content population that would have replaced it retains only {src}% of \
      the source's evidence mass and {tgt}% of the target's, under the {floor}% a \
      population must keep to be read as one — so the whole-frame medians above stand, \
      and so does their pairing assumption.",
        " 本可取而代之的共有内容人口只保留了源证据质量的 {src}% 与目标证据质量的 {tgt}%，\
         低于一个人口要被当作人口来读所需的 {floor}%——因此上面的整帧中位数继续成立，\
         其配对假设亦然。"),
    (" How much of that reference population has no counterpart in the source was not \
      measured: no cross-image correspondence field was available for this pair.",
        " 该参照人口中有多少在源图中没有对应，本次未测量：这一对没有可用的跨图对应场。"),
    (" Zoned {label} correction kept by the strictly-better arm: zone residual {before} \
      → {after}, an absolute gain past the {gain} floor that the halve-or-land arms \
      would have dropped, at no cost to the frame ({frame_before} → {frame_after}). \
      Local quality read texture ratio {texture}; that texture floor is calibrated but \
      known not to separate every case, so this correction rests on the clipping gate, \
      the zero-regression frame reading and the boundary gate.",
        " {label} 区校正由「严格更优」臂保留：区内残差 {before} → {after}，绝对收益越过 {gain} 下限，\
         而「减半或落位」两臂本会丢弃它；帧上没有付出代价（{frame_before} → {frame_after}）。\
         局部质量读数为纹理比 {texture}；该纹理下限虽经标定，但已知无法区分所有情形，\
         故此校正实际依托的是裁切门、零回归的帧读数与边界门。"),
    (" Cross-image correspondence measured (DIFT): {cov}% of the frame has a confident counterpart in the target (median confidence {med}); full zone fits weight pairs by it and read shifted content at its corresponded position.",
        " 跨图对应已测量（DIFT）：画面 {cov}% 在目标中有可信对应（中位置信 {med}）；完整区拟合按其为像素对加权，并在对应位置读取被移动的内容。"),
    (" Cross-image correspondence unavailable ({e}) — the content-divergent estimators ran without it.",
        " 跨图对应不可用（{e}）——内容分歧估计器已在没有它的情况下运行。"),
    ("Adherence", "遵循度"),
    ("How closely the AI follows your direction; disabled until Direction has text: <=40% Hint, 40-70% Direct, above 70% Brief. Prompt intent only - it never moves a render limit. Direct and Brief also decide WHO LEADS: your style library becomes background and its distillation pull is skipped, so a direction can take a photo somewhere your past edits never went. Hint leaves the library in the lead.",
        "AI 对你所写方向的遵循程度；Direction 为空时不可用：≤ 40% Hint，40-70% Direct，高于 70% Brief。仅影响提示词意图，不会改变任何渲染限制。Direct 与 Brief 还决定由方向还是风格库主导：你的风格库退为参照，其风格蒸馏拉动被跳过，方向因此可以把一张照片带到你过往编辑从未去过的地方。Hint 则让风格库继续主导。"),
    ("a system font was found but not loaded — {font}. File names in the scripts it covers may show as boxes.",
        // Deliberately says "字体" and not "系统字体": 系 (U+7CFB) is in no
        // other translated string, so embedding it would mean regenerating
        // all five font subsets for one glyph. The path in {font} says which
        // font it is far better than the adjective would.
        "字体已找到但未加载——{font}。用到这类文字的文件名可能显示为方块。"),
    ("Look library", "外观库"),
    ("Use look library", "使用外观库"),
    ("Pick look folder…", "选择成片文件夹…"),
    ("Build look library", "构建外观库"),
    ("Use SigLIP 2 look embedding (downloads 1.5 GB once; index builds and analyses take longer)", "使用 SigLIP 2 外观嵌入（首次下载 1.5 GB；建库与分析都会变慢）"),
    ("Embedding is optional and local. The environment override wins when set; rebuild the index after changing this switch.", "嵌入是可选的本地功能。环境变量一经设置即优先于此开关；改动后请重建索引。"),
    ("Describe looks with the local vision model (downloads 4.3 GB once; slower builds)", "用本地视觉模型描述外观（首次下载 4.3 GB；构建更慢）"),
    ("Writes ONE short sentence per photo about its GRADE — white balance, tonality, contrast, colour, finishing — with a local model (Qwen3-VL-2B). Nothing leaves this machine and nothing is billed. Descriptions are cached by frame content, so a rebuild only describes what changed. Off = the fixed attribute tags alone.", "为每张照片写一句关于其调色的话——白平衡、影调、对比、色彩、后期质感——由本地模型（Qwen3-VL-2B）生成。数据不出本机，也不产生任何费用。描述按帧内容缓存，重建时只描述变化过的照片。关闭 = 只用固定的属性标签。"),
    ("Turn on the look embedding first — the description pass runs over the same frames", "请先开启外观嵌入——描述会在同一批帧上运行"),
    ("decoding", "解码"),
    ("embedding", "嵌入"),
    ("describing", "描述"),
    ("text vectors", "文本向量"),
    ("Building the look library from {path}", "正在从 {path} 构建外观库"),
    ("Look library built: {n} finished photos from {path}", "外观库构建完成：来自 {path} 的 {n} 张成片"),
    ("{n} of your own edits · from {path} · embeddings {with_embedding}/{total} · looks {looks}", "{n} 条你自己的编辑 · 来自 {path} · 嵌入 {with_embedding}/{total} · 外观 {looks}"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// macOS says Cmd, and this app's bindings always meant Cmd there.
    ///
    /// Every shortcut is `egui::Modifiers::COMMAND`, which egui maps to Command
    /// on a Mac and Ctrl everywhere else, so the hint strings were naming a key
    /// the app does not listen for on exactly one platform.
    #[test]
    fn the_modifier_label_follows_the_platform_the_bindings_already_use() {
        let hint = tr(Lang::En, "Ctrl+O · or drag a file into the window");
        if cfg!(target_os = "macos") {
            assert_eq!(hint, "Cmd+O · or drag a file into the window");
        } else {
            assert_eq!(
                hint, "Ctrl+O · or drag a file into the window",
                "no Windows or Linux string may move for this"
            );
        }
        // The Chinese side goes through the same rewrite: the translations
        // spell the modifier in ASCII too.
        assert_eq!(
            tr(Lang::Zh, "Ctrl+O · or drag a file into the window").contains("Ctrl+"),
            !cfg!(target_os = "macos")
        );
        // A string naming no modifier comes back untouched on every platform —
        // and as the SAME bytes, not an interned copy.
        assert_eq!(tr(Lang::En, "Cancel"), "Cancel");
        assert!(std::ptr::eq(tr(Lang::En, "Cancel"), "Cancel"));
    }

    #[test]
    fn a_value_containing_placeholder_syntax_is_not_reinterpreted() {
        // The old sequential replace expanded {dir} first, then the {count}
        // pass rewrote the placeholder-looking text INSIDE the directory name.
        let s = trf(
            Lang::En,
            "{dir} · {count} photos",
            &[("dir", "D:/packs/{count}"), ("count", "3")],
        );
        assert_eq!(s, "D:/packs/{count} · 3 photos");
    }

    #[test]
    fn a_dropped_or_unknown_placeholder_stays_visible() {
        let s = trf(Lang::En, "saved → {path}", &[]);
        assert_eq!(s, "saved → {path}");
        let s = trf(Lang::En, "brace {", &[]);
        assert_eq!(s, "brace {");
    }

    #[test]
    fn repeated_placeholders_all_expand() {
        let s = trf(Lang::En, "{n} of {n}", &[("n", "2")]);
        assert_eq!(s, "2 of 2");
    }

    /// Every Chinese value is real Chinese, in the character classes this file
    /// already uses.
    ///
    /// S1 shipped 16 values that were UTF-8 Chinese bytes DECODED AS GBK: the
    /// text was still valid UTF-8 (so nothing failed to compile, and nothing
    /// failed to render) but it read as unrelated hanzi, two values carried
    /// U+FFFD where a byte pair had no GBK mapping at all, and the shipped SC
    /// font subset then grew 47 KB carrying the garbage glyphs. A console
    /// encoding accident is invisible to every other gate in this repo, so it
    /// gets its own.
    ///
    /// The allowed set is DERIVED from the catalogue, not hand-listed: CJK, plus
    /// exactly the non-ASCII symbols the ENGLISH keys already carry. So a
    /// translation may introduce hanzi and nothing else, and the list cannot
    /// drift away from what the UI actually renders.
    ///
    /// MUTATION: put any single mojibake value back and this fails, naming it.
    #[test]
    fn every_chinese_value_is_real_chinese_and_not_a_console_encoding_accident() {
        // U+FFFD is the unambiguous half: a byte sequence that had no mapping
        // at all in the codepage it was mangled through.
        for (en, zh) in ZH_ENTRIES {
            assert!(
                !zh.contains('\u{FFFD}'),
                "the Chinese for {en:?} carries U+FFFD - it was written through a non-UTF-8 \
                 console; rewrite it with an explicit UTF-8 write"
            );
        }
        // ...and the rest by character class, DERIVED from the catalogue rather
        // than hand-listed: a Chinese value may introduce CJK, and every other
        // non-ASCII character in it must already appear on the ENGLISH side.
        // The English keys are authored in Rust source that no console
        // re-encoded, so they are the trustworthy half, and every symbol the UI
        // renders in a translation (the middot, the ellipsis, the arrows, the
        // toolbar emoji) reaches it from there.
        let symbols: std::collections::BTreeSet<char> =
            ZH_ENTRIES.iter().flat_map(|(en, _)| en.chars()).filter(|c| !c.is_ascii()).collect();
        // The same ranges `scripts/subset_gui_fonts.py` embeds.
        let is_cjk = |c: char| matches!(c as u32, 0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF);
        for (en, zh) in ZH_ENTRIES {
            if let Some(bad) =
                zh.chars().find(|c| !c.is_ascii() && !is_cjk(*c) && !symbols.contains(c))
            {
                panic!(
                    "the Chinese for {en:?} contains U+{:04X} {bad:?}, which is neither CJK nor \
                     a symbol any English key uses",
                    bad as u32
                );
            }
        }
        // DELIBERATE NON-CHECK: an all-ASCII value is not an error. "AI" is
        // "AI" in the Chinese UI, and a rule demanding a hanzi in every value
        // would force a worse translation of an acronym.
    }
}
