; From gpui-component 0.8.0 (Apache-2.0), src/highlighter/languages/rust/injections.scm, the
; query the preview highlights with.

((macro_invocation
  (token_tree) @injection.content)
 (#set! injection.language "rust")
 (#set! injection.include-children))

((macro_rule
  (token_tree) @injection.content)
 (#set! injection.language "rust")
 (#set! injection.include-children))