; Foldable regions: sections by level, delimited blocks and lists.
;
; gpui-component derives fold ranges from multi-line syntax nodes itself, so this query
; documents the intended regions and is used by tests that check them against the grammar.
[
  (section_block)
  (delimited_block)
  (listing_block)
  (literal_block)
  (open_block)
  (sidebar_block)
  (quoted_block)
  (passthrough_block)
  (block_comment)
  (table_block)
  (ntable_block)
  (csv_table_block)
  (dsv_table_block)
  (unordered_list)
  (ordered_list)
  (checked_list)
  (description_list)
  (callout_list)
] @fold
