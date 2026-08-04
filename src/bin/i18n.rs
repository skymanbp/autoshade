//! Zero-dependency i18n for the native GUI (English skeleton · Chinese overlay).
//!
//! English is the SKELETON: every user-facing string literal in `gui.rs` is
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
//! This file is a PRIVATE submodule of `gui.rs` (`mod i18n;`), not a binary —
//! see `autobins = false` in Cargo.toml for why that distinction matters.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The UI language. `En` is both the default and the skeleton (see module docs).
/// Persisted in `Prefs` (eframe storage); a save from an older build that
/// predates this field decodes to `En` via `#[serde(default)]`.
#[derive(Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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
    match lang {
        Lang::En => en,
        Lang::Zh => zh_map().get(en).copied().unwrap_or(en),
    }
}

/// Translate + interpolate. `args` are `(name, value)` pairs; each `{name}`
/// placeholder in the (possibly translated) string is replaced by `value`.
/// Used for every string that was a `format!(…)` before i18n: `format!` needs a
/// compile-time literal, so a translated string is filled by plain replacement.
/// A placeholder a translation happens to drop is simply left as-is (visible),
/// never a panic.
pub fn trf(lang: Lang, en: &'static str, args: &[(&str, &str)]) -> String {
    let mut s = tr(lang, en).to_string();
    for (name, value) in args {
        s = s.replace(&format!("{{{name}}}"), value);
    }
    s
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
    // ── Settings ────────────────────────────────────────────────────────────
    ("Language", "语言"),
    ("Reverse-fit", "反推 / Reverse-fit"),
    ("Zoned fit (sky)", "分区反推：天空 / Zoned fit (sky)"),
    ("On reverse-fit, auto-split the sky on both sides and colour-correct sky↔sky separately (exposure / recolour gains / saturation, bitmap mask). Masks are rendered by the local engine; the LR sidecar carries only the global part. Needs the python segmentation deps (transformers + torch); falls back to pure global reverse-fit when unavailable, noting it in the rationale.",
        "反推时自动分割两侧天空，天空↔天空单独校色（曝光/重着色增益/饱和，位图蒙版）。蒙版由本机引擎渲染；LR sidecar 只携带全局部分。需要 python 分割依赖（transformers + torch）；不可用时自动退回纯全局反推并在理由里说明。"),
    ("Analysis — the verifier", "分析 · 校验器"),
    ("Provider", "提供方"),
    ("Model", "模型"),
    ("Base URL", "基础 URL"),
    ("API Key", "API 密钥"),
    ("key set — blank keeps it", "已设密钥 — 留空则保留"),
    ("no key set", "未设密钥"),
    ("Image — the vision proposer + generative edits", "图像 · 视觉提案 + 生成式编辑"),
    ("OAuth (Codex bridge / ChatGPT sub)", "OAuth (Codex 桥 / ChatGPT 订阅)"),
    ("fetching…", "拉取中… / fetching…"),
    ("🔄 Fetch models", "🔄 拉取可用模型 / Fetch models"),
    ("List the models this endpoint serves (GET /models) so you can pick instead of guess — and a live reachability check for the bridge/API. Uses the key/token typed below, or the saved one if blank.",
        "列出该端点提供的模型（GET /models），可挑选而非猜测 —— 并对桥接/API 做一次连通性检查。使用下方输入的密钥/令牌；留空则用已保存的。"),
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
    ("Each renders by its own saved develop from the store (neutral develop if none) → ./out/<name>.developed.*, using the current format / long-edge / sharpening / quality; AI Denoise sits out the batch.",
        "每张按它在显影库里保存的显影出图（没有则中性显影）→ ./out/<名>.developed.*，用当前格式/长边/锐化/质量；AI Denoise 不参与批量"),
    ("Clear selection", "清除多选"),
    ("Include crop / straighten when pasting", "粘贴时含裁剪/拉直"),
    ("Off by default — composition rarely transfers between photos", "默认不带几何 — 构图在照片间通常不可复用"),
    ("Open a folder to browse your photos here.", "打开一个文件夹，在此浏览照片。"),
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
    ("Generate an image first and stay on that variant to reverse-fit its recipe.",
        "先「AI 生成出片」并停在该变体上，才能反推它的配方。"),
    ("🎛 Reverse-fit recipe → sliders/XMP", "🎛 反推配方 → 滑杆/XMP"),
    ("Statistical fit: reverse the freshly generated look into editable develop params (local, no API cost). Sliders update (undoable), and for RAW a Lightroom XMP goes into this photo's develop store; hit Export to render the full-resolution result.",
        "统计拟合：把刚生成的观感反解成可编辑的 develop 参数（本地运算，无 API 费）。滑杆会更新（可 undo），RAW 会在该照片的显影库里生成 Lightroom XMP；再点「导出」可出全分辨率成品。"),
    ("📝 Extract style prompt", "📝 提取风格提示词"),
    ("Compare the original / generated images and have the vision model write a reusable style prompt: auto-fills the Reimagine prompt (ready to restyle other photos) and saves ./out/<stem>.style.txt.",
        "对比 原图/生成图，让 vision 模型写一段可复用的风格 prompt：自动填入 Reimagine 提示词（可直接给别的照片重绘用）并存 ./out/<stem>.style.txt。"),
    ("After generating, use 「Reverse-fit recipe」 to turn the look into sliders + XMP (the full-resolution way).",
        "生成后可「反推配方」把观感变成滑杆+XMP（全分辨率的正道）。"),
    ("Paint mask", "涂抹蒙版"),
    ("Brush over the area; box-select is paused while on. Shared by Fill and Heal.",
        "在区域上涂抹；开启时框选暂停。Fill 与 Heal 共用。"),
    ("Generative Fill", "生成填充 · Generative Fill"),
    ("what belongs there, e.g. remove the trash can, extend the sky",
        "那里该有什么，例如：移除垃圾桶、延展天空"),
    ("Full-res", "全分辨率"),
    ("Composite onto the full-sensor develop (slow, RAW only)", "合成到全分辨率显影上（慢，仅 RAW）"),
    ("Heal the full-resolution develop (slow, RAW only)", "在全分辨率显影上修复（慢，仅 RAW）"),
    ("gpt-image render quality — higher looks better and costs more per image",
        "gpt-image 出图质量 — 越高画质越好、单张费用也越高"),
    ("high", "高"),
    ("medium", "中"),
    ("low", "低"),
    ("Remove / Fill", "移除 / 填充"),
    ("Paint the area, write what belongs there, then Remove/Fill. Needs an image API (OPENAI_API_KEY, or the OAuth image bridge in Settings).",
        "涂抹区域，写下那里该有什么，再点 Remove/Fill。需图像 API（OPENAI_API_KEY，或设置里的 OAuth 图像桥）。"),
    ("Heal (pixel)", "去瑕疵 · Heal（像素）"),
    ("✦ AI heal (auto)", "✦ AI 去瑕疵 (auto)"),
    ("Heal painted area", "修复涂抹区域"),
    ("AI auto-detects dust / blemishes, or paint a mask and Heal it. Pixel retouch from surrounding pixels; saved to ./out.",
        "AI 自动识别灰尘/瑕疵，或涂抹蒙版后修复。按周围像素做像素级修饰；存 ./out。"),
    ("Clone Stamp", "仿制图章 · Clone Stamp"),
    ("✅ Done", "✅ 完成"),
    ("Stamp: Alt+click to set the source → brush the target area → 「⎘ Clone painted area」",
        "图章：Alt+点击取源点 → 画笔涂目标区 → 「⎘ 克隆已涂区域」"),
    ("⎘ Clone painted area", "⎘ 克隆已涂区域"),
    ("Clone on the full-resolution develop (slow, RAW only)", "在全分辨率显影上克隆（慢，仅 RAW）"),
    ("Photoshop-style clone stamp: Alt+click to sample a source (cross marker), brush the area to cover, and pixels are carried over as-is from the source (feathered edges, no tone matching). Local compute, saves a ./out pixel master.",
        "Photoshop 的仿制图章：Alt+点击取源（十字标记），画笔涂要覆盖的区域，按源点原样搬运像素（羽化边缘、不做色调匹配）。本地运算，存 ./out 像素母版。"),

    // ── Develop · shared slider helper ───────────────────────────────────────
    // NOT 归零: five sliders reset to a non-zero default (Temp 5500, Blending/
    // Midpoint 50, Lum. high 1.0, Feather 0.1) — matches 重置为默认值 in F1.
    ("double-click resets · hover + ↑/↓ nudges (Shift ×10)",
        "双击恢复默认值 · 悬停按 ↑/↓ 微调（Shift ×10）"),

    // ── Develop · panel + Tone & WB ──────────────────────────────────────────
    ("Develop", "显影 · Develop"),
    ("Tone & WB", "色调 · Tone & WB"),
    ("Custom white balance (off = as-shot)", "自定义白平衡（关=按拍摄值）"),
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
    // GRADE_REGIONS labels (Color Grading region picker).

    // ── Develop · Crop + Lens ────────────────────────────────────────────────
    ("Crop", "裁剪 · Crop"),
    ("⛶ Enter crop", "⛶ 进入裁剪"),
    ("Straighten (°)", "拉直 (°)"),
    ("Once in (R): drag corner/edge handles to resize, drag inside to move, drag OUTSIDE the box to rotate-straighten; preview, export and XMP all match. Straighten auto-crops the black corners.",
        "进入后（R）：拖角/边把手调整大小、框内拖动移动、框外拖动旋转拉直；预览、导出与 XMP 三者一致。拉直会自动裁掉黑边。"),
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
    ("Vignette", "暗角"),
    ("Midpoint", "中点"),
    ("Distortion", "畸变"),
    ("Vignette: positive brightens the corners (compensates falloff), negative darkens; a radial gain in linear light. Distortion: positive fixes barrel (wide-angle bulge), negative fixes pincushion (tele pinch); auto-scales to fill the frame, and masks / brush still position on the corrected image. Preview / export / XMP match. De-fringe in a later batch.",
        "暗角：正值提亮四角（补偿衰减），负值压暗；在线性光下的径向增益。畸变：正值修桶形（广角外凸），负值修枕形（长焦内缩）；自动缩放填满画幅，蒙版/画笔仍按校正后的图像定位。预览/导出/XMP 一致。去紫边留待后续批次。"),
    // CROP_ASPECTS display names (ratio values are not localized).
    ("Free", "自由"),
    ("Original", "原始"),

    // ── Develop · Local Masks (add + AI segmentation) ────────────────────────
    ("Local Masks ({n})", "局部蒙版 ({n})"),
    ("＋ Linear gradient", "＋ 线性渐变"),
    ("Drag on the image: start = unaffected side, end = fully-applied side",
        "在图上拖拽：起点=不受影响侧，终点=完全应用侧"),
    ("Drag on the image to draw a linear gradient (start unaffected → end fully applied)",
        "在图上拖拽画线性渐变（起点不受影响 → 终点完全应用）"),
    ("Drag on the image to draw an elliptical area", "在图上拖拽画一个椭圆区域"),
    ("Drag on the image to draw a radial (elliptical) area", "在图上拖拽画径向（椭圆）区域"),
    ("Drag to reorder", "拖动重新排序"),
    ("🤖 AI select subject", "🤖 AI 选主体"),
    ("U²-Net salient-subject segmentation → bitmap mask (python sidecar: pip install rembg; first run auto-downloads the model to ~/.u2net)",
        "U²-Net 显著主体分割 → 位图蒙版（python sidecar：pip install rembg；首次运行自动下载模型到 ~/.u2net）"),
    ("☁ AI select sky", "☁ AI 选天空"),
    ("SegFormer-ADE20K sky segmentation → bitmap mask (python sidecar: pip install transformers; first run auto-downloads a ~14MB model)",
        "SegFormer-ADE20K 天空分割 → 位图蒙版（python sidecar：pip install transformers；首次运行自动下载约 14MB 模型）"),

    // ── Develop · selected-mask controls ─────────────────────────────────────
    ("Name", "名称"),
    ("↻ Redraw", "↻ 重画"),
    ("Re-drag this mask's area on the image", "在图上重新拖拽这个蒙版的范围"),
    ("Overlay", "叠加"),
    ("Show this mask's actual coverage as a red semi-transparent overlay (geometry × range × strength, shortcut O)",
        "用红色半透明显示这个蒙版的实际作用范围（几何×范围×强度，快捷键 O）"),
    ("Move up (renders earlier)", "上移（更早渲染）"),
    ("Move down (renders later)", "下移（更晚渲染）"),
    ("Invert", "反转"),
    ("Edge feather", "边缘羽化"),
    ("Flip", "内外翻转"),
    ("Swap which side of the ellipse the adjustment affects (composes with Invert)",
        "对调椭圆内外的作用侧（与反转 Invert 叠加生效）"),
    ("Range mask", "范围蒙版"),
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
    ("Temp", "色温"),
    ("More (XMP/Lightroom only)", "更多 · More（仅 XMP/Lightroom 生效）"),
    ("Texture", "纹理"),
    ("Lightroom-style local adjustments: add a gradient to darken the sky, a radial to brighten the subject. AI Analyze also writes to this list.",
        "像 Lightroom 的局部调整：加一个渐变压暗天空、径向提亮主体。AI Analyze 也会写到同一列表。"),

    // ── Develop · Versions ───────────────────────────────────────────────────
    ("Versions ({n})", "版本 · Versions ({n})"),
    ("＋ Save as version", "＋ 存为版本"),
    ("Save all current develop parameters as a numbered snapshot (v<N>.recipe.json in this photo's develop store), reloadable anytime",
        "把当前全部 develop 参数存为一个编号快照（此照片显影库中的 v<N>.recipe.json），随时可回"),
    ("Load", "载入"),
    ("Replace current parameters (one Ctrl+Z to undo)", "替换当前参数（一步 Ctrl+Z 可撤销）"),
    ("Like LR virtual copies: store multiple parameter sets for one photo (B&W, cropped…) without overwriting.",
        "像 LR 虚拟副本：一张照片存多套参数（黑白版/裁剪版…），互不覆盖。"),

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
    ("After — drag a box = local AI · scroll to zoom · space/middle-drag to pan · hold B to compare",
        "After — 拖框=局部AI · 滚轮缩放 · 空格/中键平移 · 按住B对比"),
    ("Preview pixels 1:1 (double-click the image to toggle)", "预览像素 1:1（双击图片可切换）"),
    ("Fit the whole image to the canvas (double-click the image to toggle)", "整图适配画布（双击图片可切换）"),
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
    ("Delete this variant", "删除此变体"),

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
    ("Style", "风格"),
    ("Personal style strength: how far AI proposals lean toward your past XMP editing habits (0 = ignore)",
        "个人风格强度：AI 提案向你过往 XMP 编辑习惯靠拢的程度（0 = 不参考）"),
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
    ("AI Denoise", "AI 降噪"),
    ("Download…", "下载…"),
    ("Download… = save the full-resolution export to a path you choose", "下载…＝把全分辨率导出保存到你选的路径"),
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
    ("Select…", "选择… / pick"),
    ("or type a custom id", "或输入自定义模型 id"),

    // ── Versions · save / load snapshots (status) ────────────────────────────
    ("Version v{n} saved → {path}", "版本 v{n} 已存 → {path}"),
    ("Save version failed: {err}", "存版本失败: {err}"),
    ("Loaded version v{n} — Ctrl+Z returns to before the load", "已载入版本 v{n} — Ctrl+Z 可回到载入前"),
    ("Load v{n} failed: {err}", "载入 v{n} 失败: {err}"),

    // ── Tone curve caption (curve_editor) ────────────────────────────────────
    ("Click to add a point · drag to move · drag outside the box to delete — preview and export match (XMP carries the closest Lightroom form)",
        "点击加点 · 拖动移点 · 拖出框外删点 — 预览/导出/XMP 同源生效"),

    // ── AI segmentation · subject / sky (labels + status) ────────────────────
    ("Subject", "主体"),
    ("Sky", "天空"),
    ("AI「{what}」mask added — adjust its sliders (exposure / contrast / saturation…) to take effect",
        "AI「{what}」蒙版已加入 — 调它的滑杆（曝光/对比/饱和…）即刻生效"),
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
    ("Batch-rendering {n} photos → ./out …", "批量渲染 {n} 张 → ./out …"),
    ("./out — batch {n} done", "./out — 批量 {n} 张完成"),
    ("Batch: {ok} succeeded, {fail} failed: {detail}", "批量：{ok} 成功、{fail} 失败：{detail}"),
    (" · same-name photos kept apart: {list}", " · 同名照片已避让：{list}"),
    ("Batch-rendering {done}/{total} → ./out …", "批量渲染 {done}/{total} → ./out …"),
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
    ("could not clear the saved edits: {err}", "无法清除已保存的编辑：{err}"),
    ("neutral recipe — saved edits cleared (saved files removed)",
        "中性配方 — 已清除保存的编辑（存档文件已删除）"),
    ("neutral recipe — nothing to save", "中性配方 — 无需保存"),
    (" · retouched pixels: master linked — reopening restores them (the Lightroom XMP stays parametric-only)",
        " · 修饰像素：已关联烘焙母版 — 重新打开会恢复（Lightroom XMP 仍只含参数化显影）"),
    ("could not record the retouched master ({err}) — reopening shows the un-retouched source; Export keeps the pixels",
        "无法记录修饰母版（{err}）— 重新打开将显示未修饰的原图；「导出」可保留像素"),
    ("cancelled — the app is free again; the running call stops at its next checkpoint and its late result is discarded",
        "已取消 — 应用已解锁；进行中的调用会在下一检查点停止，其迟到结果将被丢弃"),
    ("✕ Cancel", "✕ 取消"),
    ("could not clear the recorded retouched master ({err}) — reopening may resurrect it",
        "无法清除已记录的修饰母版（{err}）— 重新打开可能使其复活"),
    ("Stop this retouch/generation: the app unblocks now, the call halts at its next checkpoint, and its late result is discarded",
        "停止本次修饰/生成：应用立即解锁，调用在下一检查点停止，迟到结果将被丢弃"),
    ("over 999 retouch masters for this photo — clean up ./out first",
        "本照片的修饰母版已超过 999 个 — 请先清理 ./out"),
    (" · previous save backed up as v{n}", " · 之前的保存已备份为 v{n}"),
    ("● unsaved", "● 未保存"),
    ("Edits (or a baked retouch) differ from your saved develop — Ctrl+S saves; switching photos keeps them for this session only",
        "编辑与已保存的显影不同 — Ctrl+S 保存；切换照片仅在本会话内暂存"),
    ("ready — restored this session's unsaved edits (● not saved yet; Ctrl+S)",
        "就绪 — 已恢复本会话未保存的编辑（● 尚未保存；Ctrl+S）"),
    ("recipe.json is unreadable ({err}) — edits NOT fully restored; Ctrl+S would overwrite it",
        "recipe.json 无法解析（{err}）— 编辑未完整恢复；Ctrl+S 会覆盖它"),
    ("a saved develop exists but holds no effective edits", "已保存的显影存在但不含有效编辑"),
    ("AI develop applied — verdict {v}: NOT saved (Ctrl+S keeps it, Ctrl+Z steps back)",
        "AI 显影已应用 — 判词 {v}：未保存（Ctrl+S 保留，Ctrl+Z 回退）"),
    ("AI develop applied · saved to recipe.json", "AI 显影已应用 · 已保存到 recipe.json"),
    ("AI develop applied · saved (previous save backed up as v{n})",
        "AI 显影已应用 · 已保存（之前的保存已备份为 v{n}）"),
    ("AI develop applied — but saving the sidecar failed: {err}",
        "AI 显影已应用 — 但显影保存失败：{err}"),
    ("AI develop applied — but NOT saved: backing up your existing save failed ({err}); Ctrl+S overwrites explicitly",
        "AI 显影已应用 — 但未保存：备份你已有的保存失败（{err}）；Ctrl+S 可显式覆盖"),
    (" · NOT persisted: backing up your existing save failed ({err}) — Ctrl+S to save explicitly",
        " · 未持久化：备份你已有的保存失败（{err}）— Ctrl+S 可显式保存"),
    ("Open a photo, or open a folder to browse your library.",
        "打开一张照片，或打开文件夹浏览图库。"),
    ("busy — variants unlock when the current task finishes",
        "忙碌中 — 当前任务完成后变体才可切换/删除"),
    ("opened the first photo — {n} more ignored (drop their folder to browse them all)",
        "已打开第一张 — 其余 {n} 张被忽略（把它们所在的文件夹拖进来可整体浏览）"),
    ("{n} bitmap mask(s) not pasted — their rasters belong to the source photo (re-run AI select on each target)",
        "{n} 个位图蒙版未粘贴 — 栅格属于源照片（请在各目标上重跑 AI 选择）"),
    ("AI segmenting {what}… (first run auto-downloads the model; failures are reported here)",
        "AI 分割{what}中…（首次运行自动下载模型；失败会在此报告）"),
    ("Reset to its default", "重置为默认值"),
    ("Language & Reverse-fit apply immediately. The provider sections below persist via 「Save settings」 to autoshop.local.json in your per-user Autoshop folder (never in a repo) and apply to the next AI call (Analyze / Fill / Reimagine).",
        "「语言」与「反推」立即生效。下方的提供商设置经「保存设置」写入你 Autoshop 个人目录下的 autoshop.local.json（不在仓库里），对下一次分析生效。"),

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
    ("🤖 AI Denoise now", "🤖 立即 AI 去噪"),
    ("Run the SCUNet GPU sidecar on this variant's pixels and show the result on canvas (undoable — bakes a clean base into the current variant; the develop sliders keep applying on top; first run downloads the model)",
        "对当前变体的像素跑 SCUNet GPU 边车，结果直接上画布（可撤销——干净基图烘焙进当前变体；显影滑杆继续在其上生效；首次运行会下载模型）"),
    ("Denoise the full-sensor develop (slow, minutes on GPU); off = a ≤2048px working copy for a quick on-canvas result",
        "对全画幅显影去噪（慢，GPU 需数分钟）；关闭 = 用 ≤2048px 工作副本快速出画布结果"),
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
    ("Reverse-fitting… (statistical fit + sky segmentation; first run downloads the model)",
        "反推配方中…（统计拟合 + 天空分割，首次分割会下载模型）"),
    ("Reverse-fitting… (statistical fit, local compute)", "反推配方中…（统计拟合，本地运算）"),
    ("Reverse-fit done: look residual {before}→{after} · created a「Reverse-fit」variant (editable / XMP / full-res)",
        "反推完成：look 残差 {before}→{after} · 已建「反推」变体（可编辑/导 XMP/出全分辨率）"),
    (" · includes sky-zone correction (adjustable in the mask panel; XMP carries the global part only)",
        " · 含天空分区校正（蒙版面板可调；XMP 只带全局部分）"),
    ("Reverse-fit failed", "反推失败"),
    ("Style prompt extracted → filled into the Reimagine prompt (also saved ./out/<stem>.style.txt)",
        "风格提示词已提取 → 已填入 Reimagine 提示词（同时存 ./out/<stem>.style.txt）"),
    ("Extracting style prompt… (vision, ~5-20s)", "提取风格提示词中…（vision，约 5-20 秒）"),
    ("Style extraction failed", "风格提取失败"),

    // ── Shortcuts cheat-sheet window (title + both columns) ───────────────────
    ("⌨ Shortcuts", "⌨ 快捷键 · Shortcuts"),
    ("Open photo", "打开照片"),
    ("Undo / Redo", "撤销 / 重做"),
    ("Step through the library", "图库走图"),
    ("Enter / exit crop", "进入 / 退出裁剪"),
    ("Brush size (paint / clone armed)", "笔刷大小（画笔 / 克隆已启用时）"),
    ("Hide / show the side panels", "隐藏 / 显示侧栏"),
    ("Hover a slider + ↑/↓", "悬停滑杆 + ↑/↓"),
    ("Nudge its value (Shift ×10)", "微调数值（Shift ×10）"),
    ("B (hold)", "B（按住）"),
    ("Compare original", "对比原图"),
    ("Toggle mask overlay", "蒙版覆盖层开关"),
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
    ("Slider double-click", "滑杆双击"),
    ("Curve: click / drag / drag-out", "曲线：点击/拖/拖出框"),
    ("Add / move / delete point", "加点 / 移点 / 删点"),
    ("Drag a mask handle", "蒙版手柄拖拽"),
    ("Reshape / move the selected mask", "改形 / 移动选中蒙版"),

    // ── Drag & drop overlay ──────────────────────────────────────────────────
    ("Drop to open", "松开打开 · Drop to open"),

    // ── Round-2 polish batch (AI verdict / quit guard / develop store /
    //    versions delete / settings statuses) ─────────────────────────────────
    ("AI verdict", "AI 判定 · Verdict"),
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
    ("Where saved develops live: recipes, Lightroom XMP, version snapshots and mask rasters — one folder per photo, keyed by its absolute path. Override the location with the AUTOSHOP_DATA_DIR environment variable.",
        "已保存显影的存放地：配方、Lightroom XMP、版本快照与蒙版栅格 — 每张照片一个文件夹，按其绝对路径键控。可用 AUTOSHOP_DATA_DIR 环境变量改存放位置。"),
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
    // (the " · zoned sky pass skipped …" fit note died with the R6 move of
    //  the zoned/fit gate to persist time — key removed with its call site)
    // ── UX batch (toolbar slim-down · AI section · Export section · tools) ──
    ("AI", "AI"),
    ("AI Analyze", "AI 分析"),
    ("AI proposes a recipe from scratch (GPT proposal + validation), written into the sliders — undoable. Uses the Direction above; Style steers it.",
        "AI 从零提案配方（GPT 提案+验证），直接写入滑杆——可撤销。读上方「方向」文本；风格滑杆一同生效。"),
    ("Direction", "方向"),
    ("Free-text direction for AI Analyze — e.g. warmer and moodier",
        "给 AI 分析的自由文字方向——如「更暖、更有氛围」"),
    ("Ctrl+Z · undo the last edit", "Ctrl+Z · 撤销上一步编辑"),
    ("Ctrl+Y · redo the undone edit", "Ctrl+Y · 重做撤销的编辑"),
    ("◫ Compare", "◫ 对比"),
    ("Export", "导出 · Export"),
    ("Export (settings in the Export section)", "导出（设置在 Export 节）"),
    ("Format", "格式"),
    ("Save develop", "保存显影"),
    ("Save develop (recipe + XMP for RAW)", "保存显影（recipe + RAW 的 XMP）"),
    ("Ctrl+E · full-resolution render to ./out (follows the current variant's pixels); settings in the Export section",
        "Ctrl+E · 全分辨率渲染到 ./out（跟随当前变体的像素）；设置在 Export 节"),
    ("Applied by Export / Download… in the toolbar (Ctrl+E). Files land in ./out unless Download picks a path.",
        "由工具栏的 Export / Download…（Ctrl+E）使用。文件写入 ./out，Download 可另选路径。"),
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
    ("Linear gradient — drag from the unaffected side to the fully-applied side · Esc to exit",
        "线性渐变 — 从不受影响的一侧拖到完全应用的一侧 · Esc 退出"),
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
    ("{n} other variant(s) of the open photo hold edits. Only the ACTIVE variant can be saved — Cancel, switch to each one, and save it.",
        "当前照片还有 {n} 个变体存有编辑。只有「活动」变体能被保存——请取消后逐个切换过去保存。"),
    ("Quit WITHOUT saving — these edits are gone for good", "不保存直接退出——这些编辑将永久丢失"),
    // --- round-3 audit batch (new / reworded user-facing strings) ------------
    // ("Save version failed: version namespace exhausted" folded into the
    //  parametrized "Save version failed: {err}" key — stale twin removed)
    ("no photos found in this folder", "此文件夹中没有找到照片"),
    ("AI「{what}」mask refreshed — the existing mask now uses the new selection (its sliders still apply)",
        "AI「{what}」蒙版已刷新——原有蒙版改用新的选区（其滑杆设置仍然生效）"),
    ("the saved retouch master could not be loaded — opened the un-retouched source (Ctrl+S would overwrite the master link)",
        "已保存的修饰母版无法加载——打开的是未修饰原图（Ctrl+S 会覆盖母版链接）"),
    ("mask area redrawn — its existing adjustments now apply to the new area",
        "蒙版区域已重画——原有调整现作用于新区域"),
    (" · NOT persisted: writing recipe.json failed ({err}) — Ctrl+S to save explicitly",
        " · 未持久化：写入 recipe.json 失败（{err}）——用 Ctrl+S 显式保存"),
    (" · could not clear the old retouch-master link ({err}) — reopening may show the fit on retouched pixels; Ctrl+S repairs it",
        " · 无法清除旧的修饰母版链接（{err}）——重开后反推可能落在修饰过的像素上；Ctrl+S 可修复"),
    ("Style prompt extracted → filled into the Reimagine prompt (saving ./out/<stem>.style.txt failed: {err})",
        "风格提示词已提取 → 已填入重绘提示词（保存 ./out/<stem>.style.txt 失败：{err}）"),
    ("Style prompt extracted → filled into the Reimagine prompt",
        "风格提示词已提取 → 已填入重绘提示词"),
    ("opened the first folder — {n} more dropped item(s) ignored",
        "已打开第一个文件夹——其余 {n} 个拖入项已忽略"),
    ("unsupported file type: {path} — {n} more dropped item(s) ignored",
        "不支持的文件类型：{path}——其余 {n} 个拖入项已忽略"),
];
