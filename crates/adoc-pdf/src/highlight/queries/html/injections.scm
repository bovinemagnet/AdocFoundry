; From gpui-component 0.8.0 (Apache-2.0), src/highlighter/languages/html/injections.scm, the
; query the preview highlights with.

((script_element
  (raw_text) @injection.content)
 (#set! injection.language "javascript"))

((style_element
  (raw_text) @injection.content)
 (#set! injection.language "css"))