; The `=` of a document title is an external token outside the `titleN` wrapper that carries
; the marker for every other level, so it needs capturing in its own right.
(document_title
  (title_h0_marker) @title)

(document_title
  (line) @title)

[
  (title1)
  (title2)
  (title3)
  (title4)
  (title5)
] @title

; Document header. The author and revision lines are plain text to the grammar until their
; component parts are captured individually, so each one is listed here.
[
  (firstname)
  (middlename)
  (lastname)
] @variable

(email) @link_uri

(revnumber) @number
(revdate) @string.special
(revremark) @string

(author_line
  ";" @punctuation.delimiter)

(revision_line
  "," @punctuation.delimiter
  ":" @punctuation.delimiter)

[
  (line_comment)
  (block_comment)
] @comment

(document_attr
  (attr_name) @property)

[
  (document_attr_marker)
  (element_attr_marker)
] @punctuation.delimiter

(block_title
  (block_title_marker) @punctuation.special) @attribute

(block_style) @type
(positional_attr) @attribute
(id) @label
(role) @attribute
(option) @attribute

(block_macro
  (block_macro_name) @keyword
  "::" @punctuation.delimiter
  (target)? @link_uri
  "[" @punctuation.bracket
  "]" @punctuation.bracket)

(attribute_name) @attribute
(attribute_value) @variable.parameter

[
  (admonition_note)
  (admonition_tip)
  (admonition_important)
  (admonition_caution)
  (admonition_warning)
] @keyword

[
  (list_marker_star)
  (list_marker_hyphen)
  (list_marker_dot)
  (list_marker_digit)
  (list_marker_geek)
  (list_marker_alpha)
  (description_marker)
] @punctuation.list_marker

; Checklist boxes sit inside the ordinary unordered marker, so they need capturing after it
; to win the last-match-wins contest and colour the `[x]` rather than the bullet.
[
  (checked_list_marker_unchecked)
  (checked_list_marker_checked)
] @constant

; The term of a description list is the part a reader scans for.
(description_list_item
  (term) @emphasis.strong)

; The `+` that attaches a block to the preceding list item.
(list_continuation) @punctuation.special

; Per-cell specifiers such as `h|`, `m|` and `2+|`.
(table_cell_attr) @attribute

; The header row AsciiDoc gives a table whose first line is followed by a blank one.
(table_header_row
  (table_cell
    (table_cell_content) @emphasis.strong))

; Cell and record separators for the psv, csv and dsv table forms.
(table_cell
  "|" @punctuation.special)

(ntable_cell
  "!" @punctuation.special)

(csv_record
  "," @punctuation.special)

(dsv_record
  ":" @punctuation.special)

; Every block delimiter, plus the thematic break and the trailing `+` of a hard line break.
[
  (table_block_marker)
  (csv_table_block_marker)
  (dsv_table_block_marker)
  (ntable_block_marker)
  (listing_block_start_marker)
  (listing_block_end_marker)
  (literal_block_marker)
  (passthrough_block_marker)
  (sidebar_block_start_marker)
  (sidebar_block_end_marker)
  (quoted_block_start_marker)
  (quoted_block_end_marker)
  (quoted_block_md_marker)
  (quoted_paragraph_marker)
  (open_block_marker)
  (delimited_block_start_marker)
  (delimited_block_end_marker)
  (ident_marker)
  ; The `breaks` wrapper spans the trailing newline, so capture the `'''` marker itself.
  (breaks_marker)
  (hard_wrap)
] @punctuation.special

; Callout numbers in the block body and the list entries that explain them.
[
  (callout_marker)
  (callout_list_marker)
] @punctuation.special

; Verbatim bodies. `literal_block_body` covers `....` blocks, matching `----` listing blocks.
[
  (listing_block_body)
  (literal_block_body)
  (ident_block)
] @text.literal
