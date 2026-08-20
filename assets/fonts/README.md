# Embedded GUI fonts

Subsets of five OFL-licensed Noto faces, embedded into the GUI binary so the
interface renders identically on every machine (egui's bundled fonts lack
most of the symbols, and system fonts vary — stock Windows shows tofu boxes
for ⧉ ⊖ ◭ ▭ ◯ ◌ ✓ ✕ 🖌 without these).

| file | upstream (github.com/google/fonts, `ofl/` tree) | role |
|---|---|---|
| `NotoSansSymbols2-autoshop.ttf` | `notosanssymbols2/NotoSansSymbols2-Regular.ttf` | geometric shapes, technical, dingbats |
| `NotoSansSymbols-autoshop.ttf` | `notosanssymbols/NotoSansSymbols[wght].ttf` (instanced wght=400) | enclosed alphanumerics, ⎘ |
| `NotoSansMath-autoshop.ttf` | `notosansmath/NotoSansMath-Regular.ttf` | math operators, curved/paired arrows |
| `NotoEmoji-autoshop.ttf` | `notoemoji/NotoEmoji[wght].ttf` (instanced wght=400) | monochrome emoji missing from egui's subset |
| `NotoSansSC-autoshop.ttf` | `notosanssc/NotoSansSC[wght].ttf` (instanced wght=400) | the hanzi the Chinese UI itself renders |

The CJK face carries only the codepoints the translations use — the checker
reports 68 symbols + 735 CJK codepoints, all embedded (`subset_gui_fonts.py
--check`, 2026-08-20), and the shipped SC subset measures 751 glyphs / 192 KB
where a full CJK face is ~16 MB. Before it, choosing 中文 on a machine with no
system CJK font rendered the entire window as tofu. The runtime system-CJK
fallback stays in the chain for text this static extraction cannot know,
such as the user's own file and folder names.

All five are licensed under the SIL Open Font License 1.1 — full texts in
the `OFL-*.txt` files here. Four declare no Reserved Font Name. Noto Sans SC
inherits Adobe's `Source` RFN from Source Han Sans; the subset's family name
is `Noto Sans SC`, which does not contain the reserved name, so it is kept
unchanged. Variable donors are instanced with `updateFontNames=True`, so a
shipped face's name always matches the weight of its outlines (NotoSansSC
defaults to wght=100, and without that flag the subset would ship carrying
Regular outlines while labelled `Thin`).

Regenerate with `python scripts/subset_gui_fonts.py --fonts-dir <donors>`;
the needed-glyph list is extracted from the GUI sources automatically, and
the GUI test `embedded_fonts_cover_every_ui_symbol` fails if any string
literal ever uses a symbol — or a hanzi — this chain cannot render.
