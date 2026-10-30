; From gpui-component 0.8.0 (Apache-2.0), src/highlighter/languages/json/highlights.scm, the
; query the preview highlights with.

(comment) @comment

(string) @string
(escape_sequence) @string.escape

(number) @number

(pair key: (string) @property)

[
  (true)
  (false)
] @boolean

(null) @constant.builtin

[
  ","
  ":"
  "{"
  "}"
  "["
  "]"
] @punctuation
