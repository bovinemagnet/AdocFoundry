[
  (monospace)
  (passthrough)
] @text.literal

(emphasis) @emphasis.strong
(italic) @emphasis
(highlight) @emphasis

[
  (superscript)
  (subscript)
] @emphasis

[
  (link_url)
  (email)
] @link_uri

(uri_label) @link_text

(attribute_reference
  (attribute_name) @constant)

; `{docname}`, `{nbsp}` and friends resolve without a matching document attribute.
(intrinsic_attributes) @constant

(xref
  (id) @link_uri)

(xref
  (reftext) @link_text)

(inline_macro
  (macro_name) @keyword
  (target)? @link_uri
  (attr)? @attribute)

; `footnote:`, `stem:` and `pass:` are separate nodes from `inline_macro`, so the parts they
; share with it need capturing again rather than being picked up by the pattern above.
(footnote
  (macro_name) @keyword
  (target)? @label
  (attr) @attribute)

(stem_macro
  (macro_name) @keyword
  (target)? @label
  (attr)? @text.literal)

(macro_passthrough
  (macro_name) @keyword
  (target)? @label
  (attr)? @text.literal)

; Index terms: `((visible))` and the invisible `(((primary,secondary)))`.
[
  (index_term)
  (index_term2)
] @attribute

; `{counter:name:1}`.
(counter
  (counter_function) @keyword
  (attribute_name) @constant
  (initial_value)? @number)

; The value of a named macro attribute (e.g. `window=_blank`).
(named_attr
  (attribute_value) @string)

; Curly quotes, `(C)`-style replacements and a `\` that escapes one.
[
  (typographic_quote)
  (replacement)
  (super_escape)
] @string.special

(id_assignment) @label
(role) @attribute
(escaped_sequence) @string.escape

; The trailing `+` that forces a line break.
(hard_wrap) @punctuation.special

[
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

; The `<<`/`>>` of a cross-reference get their own capture rather than sharing
; `@punctuation.bracket` with `[]` and `{}`, so a theme can colour the xref
; delimiters on their own without repainting every other bracket.
(xref
  [
    "<<"
    ">>"
  ] @tag)
