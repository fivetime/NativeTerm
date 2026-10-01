# Bundled faces

- Inter 4.1 (Regular, Medium, SemiBold), from
  `https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip`
  (`extras/ttf`).
- JetBrains Mono 2.304 (Regular), from
  `https://github.com/JetBrains/JetBrainsMono/releases/download/v2.304/JetBrainsMono-2.304.zip`
  (`fonts/ttf`).

Both under the SIL Open Font License 1.1 (the texts beside them); neither
declares a Reserved Font Name.

**Modified:** the private use area (U+E000–U+F8FF) is taken out of each
font's character map; every glyph and layout feature stays. Inter maps 745
code points there (U+E000–U+F6C3), which are the icon fonts' (Phosphor's
U+E000–U+EE83, Segoe Fluent Icons' U+E700–U+F8CC): put before the icons,
it drew its own glyphs for them. Made with fontTools:

```
python -m fontTools.subset <font>.ttf --unicodes="U+0000-DFFF,U+F900-10FFFF" \
  --layout-features='*' --name-IDs='*' --name-languages='*' --name-legacy \
  --notdef-outline --glyph-names --legacy-kern --hinting \
  --no-prune-unicode-ranges --output-file=<font>.ttf
```
