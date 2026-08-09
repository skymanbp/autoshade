# Embedded GUI symbol fonts

Subsets of four OFL-licensed Noto faces, embedded into the GUI binary so
every toolbar/panel symbol renders identically on every machine (egui's
bundled fonts lack most of them, and system fonts vary — stock Windows
shows tofu boxes for ⧉ ⊖ ◭ ▭ ◯ ◌ ✓ ✕ 🖌 without these).

| file | upstream (github.com/google/fonts, `ofl/` tree) | role |
|---|---|---|
| `NotoSansSymbols2-autoshop.ttf` | `notosanssymbols2/NotoSansSymbols2-Regular.ttf` | geometric shapes, technical, dingbats |
| `NotoSansSymbols-autoshop.ttf` | `notosanssymbols/NotoSansSymbols[wght].ttf` (instanced wght=400) | enclosed alphanumerics, ⎘ |
| `NotoSansMath-autoshop.ttf` | `notosansmath/NotoSansMath-Regular.ttf` | math operators, curved/paired arrows |
| `NotoEmoji-autoshop.ttf` | `notoemoji/NotoEmoji[wght].ttf` (instanced wght=400) | monochrome emoji missing from egui's subset |

All four are licensed under the SIL Open Font License 1.1 — full texts in
the `OFL-*.txt` files here. No Reserved Font Names are declared by the
upstream copyright notices, so the subsets keep their original family names.

Regenerate with `python scripts/subset_gui_fonts.py --fonts-dir <donors>`;
the needed-glyph list is extracted from the GUI sources automatically, and
the GUI test `embedded_fonts_cover_every_ui_symbol` fails if any string
literal ever uses a symbol this chain cannot render.
