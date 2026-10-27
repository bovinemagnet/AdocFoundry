; The injected language is looked up by the name it is registered under in
; `src/editor/language.rs`; any other spelling resolves to no language and the injection is
; silently dropped.
((block_macro
  (target) @injection.content)
  (#set! injection.language "asciidoc_inline"))

((table_cell
  (table_cell_content) @injection.content)
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
  (quoted_line (line) @injection.content)
  (quoted_md_block (line) @injection.content)
  (title1 (line) @injection.content)
  (title2 (line) @injection.content)
  (title3 (line) @injection.content)
  (title4 (line) @injection.content)
  (title5 (line) @injection.content)
  (unordered_list_item (line) @injection.content)
]
  (#set! injection.include-children)
  (#set! injection.language "asciidoc_inline"))

; Plain paragraphs use the inline grammar; a paragraph's lines are not injected again.
; Source-styled paragraphs are code, not prose.
((section_block
  (paragraph) @injection.content) @_block
  (#not-match? @_block "(^|\\n)\\[\\s*source\\s*(?:[,#.%\\]])")
  (#set! injection.include-children)
  (#set! injection.language "asciidoc_inline"))

((section_block
  (element_attr
    (positional_attr
      (block_style))
    (positional_attr) @injection.language)
  (listing_block
    (listing_block_body) @injection.content)))
