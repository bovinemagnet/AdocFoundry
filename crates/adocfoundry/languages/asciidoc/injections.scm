; The injected language is looked up by the name it is registered under in
; `src/editor/language.rs`; any other spelling resolves to no language and the injection is
; silently dropped.
;
; gpui-component parses each injection on its own and stops after a fixed number per
; document, so text is injected only when it holds a character that can begin inline markup
; (`[`*_#^~+\[\]{}<>@]`, `((`, `(C)`, `(R)`, `(TM)`, `...`, `->`, `=>`, a URL or `www.`).
; Every `#match?` below must stay a superset of what `asciidoc_inline/highlights.scm`
; captures; `prose_with_markup_is_injected_and_plain_prose_is_not` checks one of each.
((block_macro
  (target) @injection.content)
  (#match? @injection.content "[`*_#^~+\\\\\\[\\]{}<>@]|\\(\\(|\\((C|R|TM)\\)|\\.\\.\\.|->|=>|://|(?i:www\\.)")
  (#set! injection.language "asciidoc_inline"))

((table_cell
  (table_cell_content) @injection.content)
  (#match? @injection.content "[`*_#^~+\\\\\\[\\]{}<>@]|\\(\\(|\\((C|R|TM)\\)|\\.\\.\\.|->|=>|://|(?i:www\\.)")
  (#set! injection.language "asciidoc_inline"))

; Lines outside paragraphs contain inline AsciiDoc too (titles, list items, etc.).
([
  (admonition (line) @injection.content)
  (block_title (line) @injection.content)
  (callout_list_item (line) @injection.content)
  (checked_list_item (line) @injection.content)
  (description_list_item (line) @injection.content)
  (document_attr (line) @injection.content)
  (document_title (line) @injection.content)
  (ordered_list_item (line) @injection.content)
  (quoted_block (line) @injection.content)
  (quoted_md_block (line) @injection.content)
  (title1 (line) @injection.content)
  (title2 (line) @injection.content)
  (title3 (line) @injection.content)
  (title4 (line) @injection.content)
  (title5 (line) @injection.content)
  (unordered_list_item (line) @injection.content)
]
  (#match? @injection.content "[`*_#^~+\\\\\\[\\]{}<>@]|\\(\\(|\\((C|R|TM)\\)|\\.\\.\\.|->|=>|://|(?i:www\\.)")
  (#set! injection.include-children)
  (#set! injection.language "asciidoc_inline"))

; Plain paragraphs use the inline grammar; a paragraph's lines are not injected again.
; Source-styled paragraphs are code, not prose.
((section_block
  (paragraph) @injection.content) @_block
  (#not-match? @_block "(^|\\n)\\[\\s*source\\s*(?:[,#.%\\]])")
  (#match? @injection.content "[`*_#^~+\\\\\\[\\]{}<>@]|\\(\\(|\\((C|R|TM)\\)|\\.\\.\\.|->|=>|://|(?i:www\\.)")
  (#set! injection.include-children)
  (#set! injection.language "asciidoc_inline"))

((section_block
  (element_attr
    (positional_attr
      (block_style))
    (positional_attr) @injection.language)
  (listing_block
    (listing_block_body) @injection.content)))
