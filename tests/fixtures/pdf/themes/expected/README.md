The key maps asciidoctor-pdf 2.3.15's `ThemeLoader.load_theme` produces for the bundled themes and
for `acme-theme.yml`, as `[key, value]` pairs in definition order. Colours are written as
`{"color": "RRGGBB"}`, `{"cmyk": [c, m, y, k]}` or `{"transparent": true}`; font paths start with
`GEM_FONTS_DIR` (the bundled fonts) or `THEME_DIR` (this directory). `base` is loaded as is, so its
colours are plain strings. Regenerate with Ruby and the asciidoctor-pdf gem when the bundled themes
are updated.
