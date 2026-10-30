; From gpui-component 0.8.0 (Apache-2.0), src/highlighter/languages/javascript/injections.scm, the
; query the preview highlights with.

(((comment) @_jsdoc_comment
  (#match? @_jsdoc_comment "(?s)^/[*][*][^*].*[*]/$")) @injection.content
  (#set! injection.language "jsdoc"))

((regex) @injection.content
  (#set! injection.language "regex"))

(call_expression
  function: (identifier) @_name (#eq? @_name "css")
  arguments: (template_string (string_fragment) @injection.content
                              (#set! injection.language "css"))
)

(call_expression
  function: (identifier) @_name (#eq? @_name "html")
  arguments: (template_string) @injection.content
                              (#set! injection.language "html")
)

(call_expression
  function: (identifier) @_name (#eq? @_name "js")
  arguments: (template_string (string_fragment) @injection.content
                              (#set! injection.language "javascript"))
)

(call_expression
  function: (identifier) @_name (#eq? @_name "json")
  arguments: (template_string (string_fragment) @injection.content
                              (#set! injection.language "json"))
)

(call_expression
  function: (identifier) @_name (#eq? @_name "sql")
  arguments: (template_string (string_fragment) @injection.content
                              (#set! injection.language "sql"))
)

(call_expression
  function: (identifier) @_name (#eq? @_name "ts")
  arguments: (template_string (string_fragment) @injection.content
                              (#set! injection.language "typescript"))
)

(call_expression
  function: (identifier) @_name (#match? @_name "^ya?ml$")
  arguments: (template_string (string_fragment) @injection.content
                              (#set! injection.language "yaml"))
)

(call_expression
  function: (identifier) @_name (#match? @_name "^g(raph)?ql$")
  arguments: (template_string (string_fragment) @injection.content
                              (#set! injection.language "graphql"))
)
