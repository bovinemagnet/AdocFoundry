// The template every exported document is typeset with. It follows asciidoctor-pdf's
// converter: the theme's keys arrive as `t` (asciidoctor-pdf's flattened names, already
// evaluated: lengths in points, colours as colours), the document's facts as `doc`, and the
// generated body calls the `adoc-` functions below. Nothing in a document is ever Typst code.

// --- theme access -----------------------------------------------------------------------------

// The first of `keys` the theme sets, else `default`.
#let pick(..keys, default: none) = {
  for key in keys.pos() {
    let value = t.at(key, default: none)
    if value != none { return value }
  }
  default
}

#let base-size = pick("base_font_size", default: 12pt)
#let base-color = pick("base_font_color", default: black)
#let base-line-height = pick("base_line_height", default: 1.15)

// Leading between lines for a line height ratio, as Prawn spaces lines: the font's own height
// plus (ratio - 1) times the size.
#let leading(size, ratio) = calc.max((ratio - 1) * size, 0pt)

// A theme size: points, or `em` and `%` relative to `relative`.
#let resolve-size(value, relative) = {
  if type(value) == length { value.abs + value.em * relative }
  else if type(value) == ratio { value * relative }
  else if type(value) == relative { value.length.abs + value.length.em * relative + value.ratio * relative }
  else if type(value) == float or type(value) == int { value * 1pt }
  else { relative }
}

// Families: the theme's family first, then the catalogue's fallbacks.
#let families(family) = {
  let names = if family == none { () } else { (family,) }
  names + doc.fallbacks
}

// Weight and style for an asciidoctor-pdf font style.
#let font-style(style) = {
  if style == "bold" { (weight: "bold", style: "normal") }
  else if style == "italic" or style == "normal_italic" { (weight: "regular", style: "italic") }
  else if style == "bold_italic" { (weight: "bold", style: "italic") }
  else { (weight: "regular", style: "normal") }
}

// Applies `<prefix>_text_transform`.
#let transform(value, body) = {
  if value == "uppercase" { upper(body) }
  else if value == "lowercase" { lower(body) }
  else { body }
}

// Text set in the font of the first category that sets each property, as asciidoctor-pdf's
// `theme_font` cascade does: `styled(("heading_h2", "heading"), body)`.
#let styled(prefixes, body, size: auto) = {
  let get(name) = pick(..prefixes.map(prefix => prefix + "_" + name))
  let family = get("font_family")
  let color = get("font_color")
  let font-size = if size == auto { get("font_size") } else { size }
  let style = get("font_style")
  let args = (:)
  if family != none { args.font = families(family) }
  if color != none { args.fill = color }
  if font-size != none { args.size = resolve-size(font-size, base-size) }
  if style != none {
    let resolved = font-style(style)
    args.weight = resolved.weight
    args.style = resolved.style
  }
  let body = transform(get("text_transform"), body)
  text(..args, body)
}

// Expands a CSS-style shorthand (one, two, three or four values) to (top, right, bottom, left).
#let sides(value, default: 0pt) = {
  if value == none { return (default, default, default, default) }
  if type(value) != array { return (value, value, value, value) }
  let v = value.map(x => if x == none { default } else { x })
  if v.len() == 1 { (v.at(0), v.at(0), v.at(0), v.at(0)) }
  else if v.len() == 2 { (v.at(0), v.at(1), v.at(0), v.at(1)) }
  else if v.len() == 3 { (v.at(0), v.at(1), v.at(2), v.at(1)) }
  else { (v.at(0), v.at(1), v.at(2), v.at(3)) }
}

#let inset(value) = {
  let (top, right, bottom, left) = sides(value)
  (top: top, right: right, bottom: bottom, left: left)
}

// A stroke from a width and a colour, or none.
#let border(width, color) = {
  if width == none or color == none or width == 0pt { none } else { width + color }
}

// The line gap of the font in use, as a fraction of its size: Prawn's line height includes it.
#let line-gap() = {
  let font = text.font
  let family = if type(font) == array { font.first() } else { font }
  doc.line-gaps.at(lower(family), default: 0)
}

#let block-bottom = pick("block_margin_bottom", default: 12pt)
#let prose-bottom = pick("prose_margin_bottom", default: block-bottom)

// A heading styled by its caller (the document title, the title page's title): still a level 1
// heading in the PDF's structure, as PDF/UA asks of the first one. `body` carries its own
// styling, which wins over Typst's heading styles.
#let plain-heading(body) = [#heading(level: 1, outlined: false, bookmarked: false, body) <adoc-plain-heading>]

// --- inlines ----------------------------------------------------------------------------------

#let adoc-codespan(body) = {
  let background = pick("codespan_background_color")
  let content = styled(("codespan",), body)
  if background != none { highlight(fill: background, extent: 1pt, content) } else { content }
}

#let adoc-mark(body) = highlight(fill: pick("mark_background_color", default: yellow), extent: pick("mark_border_offset", default: 1pt), styled(("mark",), body))

#let adoc-role(name, body) = {
  if name == "underline" { return underline(body) }
  if name == "line-through" { return strike(body) }
  let prefix = "role_" + name
  let keys = t.keys().filter(key => key.starts-with(prefix + "_"))
  if keys.len() == 0 { return body }
  let decoration = pick(prefix + "_text_decoration")
  let body = styled((prefix,), body)
  let background = pick(prefix + "_background_color")
  let body = if background != none { highlight(fill: background, body) } else { body }
  if decoration == "underline" { underline(body) }
  else if decoration == "line-through" { strike(body) }
  else { body }
}

#let adoc-kbd(keys) = {
  let one(key) = box(
    fill: pick("kbd_background_color"),
    stroke: border(pick("kbd_border_width", default: 0.5pt), pick("kbd_border_color")),
    radius: pick("kbd_border_radius", default: 0pt),
    // The offset below the text adds to the line's height, as Prawn lays the box out.
    inset: (x: pick("kbd_border_offset", default: 2pt), bottom: pick("kbd_border_offset", default: 2pt)),
    outset: (top: pick("kbd_border_offset", default: 2pt)),
    styled(("kbd", "codespan"), key),
  )
  let separator = pick("kbd_separator", default: "+")
  keys.map(one).join(styled(("kbd",), separator))
}

#let adoc-button(label) = {
  let content = pick("button_content", default: "[%s]")
  box(styled(("button",), content.replace("%s", label)))
}

#let adoc-menu(items) = {
  let caret = text(fill: pick("menu_caret_color", default: rgb("#B12146")), [ › ])
  styled(("menu",), items.join(caret))
}

#let adoc-link(dest, body) = {
  let body = styled(("link",), body)
  let decoration = pick("link_text_decoration")
  link(dest, if decoration == "underline" { underline(body) } else { body })
}

// Superscript and subscript leave the line's height alone, as Prawn sets them.
#let adoc-super(body) = box(height: 0pt, super(body))
#let adoc-sub(body) = box(height: 0pt, sub(body))

#let adoc-footnote-ref(number, dest) = adoc-super(adoc-link(dest, [\[#number\]]))

// --- blocks -----------------------------------------------------------------------------------
//
// Spacing follows Prawn's model, not Typst's: blocks have no spacing of their own, the body puts
// each block's bottom margin after it (`#v(block-bottom)`, `#v(prose-bottom)`), and a heading
// adds its top margin as weak spacing, which Typst drops at the top of a page as asciidoctor-pdf
// does. Margins therefore add up, negative ones included, exactly as a theme expects.

#let list-gap = pick("list_item_spacing", default: 6pt)

// A block asciidoctor-pdf keeps on one page when it fits on one: moved whole to the next page
// rather than split, and split only when it is taller than a page.
//
// Measured at the page's content width: `layout` would give the exact width, but links laid out
// inside it lose their place in a tagged PDF's structure.
#let keep(body, fits: false) = if fits {
  // The emitter could tell from the block's text that it fits on a page: no need to measure.
  block(breakable: false, width: 100%, body)
} else {
  context {
    let margin = doc.page.margin
    let height = measure(body, width: doc.page.width - margin.left - margin.right).height
    block(breakable: height > doc.page.height - margin.top - margin.bottom, width: 100%, body)
  }
}

#let alignment(name, default: left) = {
  if name == "center" { center } else if name == "right" { right } else if name == "left" { left } else { default }
}

// A block with its caption ("Table 1. Title") above or below it, `caption_margin_inside`
// apart.
#let adoc-captioned(caption, body, below: false, fits: false) = {
  let inside = pick("caption_margin_inside", default: 4pt)
  let outside = pick("caption_margin_outside", default: 0pt)
  // A paragraph, so it is spaced as Prawn spaces lines of text.
  let caption = align(alignment(pick("caption_text_align", "caption_align", default: "left")),
    par(styled(("caption",), caption)))
  if not below { v(outside, weak: true) }
  keep(fits: fits, grid(
    columns: 1fr,
    row-gutter: inside,
    ..if below { (body, caption) } else { (caption, body) },
  ))
  if below { v(outside) }
}

#let adoc-paragraph(roles, body) = {
  let body = body
  for role in roles { body = adoc-role(role, body) }
  if roles.contains("lead") and pick("role_lead_font_size") != none {
    let size = resolve-size(pick("role_lead_font_size"), base-size)
    set par(leading: leading(size, base-line-height))
    text(size: size, body)
  } else if roles.contains("text-center") {
    align(center, body)
  } else if roles.contains("text-right") {
    align(right, body)
  } else { body }
}

// The document title of an article, on its first page.
#let adoc-doctitle(body) = {
  let size = resolve-size(pick("heading_h1_font_size", default: 24pt), base-size)
  set par(leading: leading(size, pick("heading_h1_line_height", "heading_line_height", default: 1.15)), justify: false)
  block(width: 100%, align(alignment(pick("heading_h1_text_align", default: "center"), default: center),
    plain-heading(styled(("heading_h1", "heading"), body))))
  v(pick("heading_h1_margin_bottom", "heading_margin_bottom", default: 12pt))
}

// Typst's own spacing, size and weight for headings and outline entries give way to the
// theme's.
#show heading: set block(above: 0pt, below: 0pt, sticky: false)
#show heading: set text(size: 1em, weight: "regular")
#show outline.entry: set block(above: 0pt, below: 0pt)

#show heading: it => {
  if it.at("label", default: none) == <adoc-plain-heading> { return it.body }
  let level = it.level
  let prefixes = ("heading_h" + str(level), "heading")
  let size = resolve-size(pick("heading_h" + str(level) + "_font_size", "heading_font_size", default: base-size), base-size)
  let top = pick("heading_h" + str(level) + "_margin_top", "heading_margin_top", default: 0pt)
  let bottom = pick("heading_h" + str(level) + "_margin_bottom", "heading_margin_bottom", default: 0pt)
  let ratio = pick("heading_h" + str(level) + "_line_height", "heading_line_height", default: 1.15)
  set par(leading: leading(size, ratio), justify: false)
  v(top, weak: true)
  let heading = block(
    width: 100%,
    inset: (bottom: calc.max(bottom, 0pt)),
    align(alignment(pick("heading_h" + str(level) + "_text_align", "heading_text_align", default: "left")),
      styled(prefixes, it.body, size: size)),
  )
  // As asciidoctor-pdf: the heading moves to the next page unless `heading_min_height_after`
  // fits below it, or stays with all of what follows when that is `auto`.
  let after = pick("heading_min_height_after", default: 0pt)
  if after == "auto" {
    block(sticky: true, heading)
  } else if type(after) == length and after > 0pt {
    block(breakable: false, { heading; v(after) })
    v(-after)
  } else {
    heading
  }
  if bottom < 0pt { v(bottom) }
}

// A list: markers in a column as wide as the indent, so the text of every item lines up. It is
// Typst's enum with each item's own marker as its number, so the PDF tags it as a list.
#let adoc-list(markers, items) = context {
  let indent = pick("list_indent", default: 30pt)
  // asciidoctor-pdf leaves the width of an "x" between a marker and its text.
  let gap = measure(text("x")).width
  let marker-color = pick("list_marker_font_color")
  enum(
    numbering: number => box(width: indent - gap, align(right, {
      let marker = markers.at(number - 1)
      if marker-color != none { text(fill: marker-color, marker) } else { marker }
    })),
    full: false,
    indent: 0pt,
    body-indent: gap,
    spacing: list-gap,
    ..items.enumerate().map(((index, item)) => enum.item(index + 1, item)),
  )
}

// A description list: each term on its own line, its description indented below. It is
// Typst's terms, so the PDF tags it as a list, with the hanging indent putting the term at the
// margin. Terms are not made bold by Typst: the theme's term style decides.
#let adoc-dlist(items) = {
  let indent = pick("description_list_description_indent", default: 15pt)
  let spacing = pick("description_list_term_spacing", default: 4pt)
  set strong(delta: 0)
  terms(
    tight: false,
    indent: 0pt,
    hanging-indent: indent,
    spacing: prose-bottom,
    separator: [],
    ..items.map(((term, description)) => terms.item(
      styled(("description_list_term",), term),
      if description == none { [] } else {
        v(spacing)
        set strong(delta: 300)
        description
      },
    )),
  )
}

#let adoc-hlist(items) = grid(
  columns: (auto, 1fr),
  column-gutter: pick("description_list_description_indent", default: 15pt),
  row-gutter: list-gap,
  ..items.map(((term, description)) => (styled(("description_list_term",), term), if description == none { [] } else { description })).flatten(),
)

#let adoc-admonition(kind, label, body, fits: false) = {
  let padding = inset(pick("admonition_padding", default: (4pt, 12pt, 4pt, 12pt)))
  let label-padding = inset(pick("admonition_label_padding", default: pick("admonition_padding", default: (4pt, 12pt, 4pt, 12pt))))
  let rule = border(pick("admonition_column_rule_width", default: 0.5pt), pick("admonition_column_rule_color", "base_border_color"))
  let label = styled(("admonition_label_" + kind, "admonition_label"), transform(pick("admonition_label_" + kind + "_text_transform", "admonition_label_text_transform"), label))
  let label-align = alignment(pick("admonition_label_text_align", default: "center"), default: center)
  let valign = pick("admonition_label_vertical_align", default: "middle")
  keep(fits: fits, block(
    width: 100%,
    fill: pick("admonition_background_color"),
    stroke: border(pick("admonition_border_width"), pick("admonition_border_color")),
    radius: pick("admonition_border_radius", default: 0pt),
    breakable: true,
    grid(
      columns: (auto, 1fr),
      align: (x, y) => if x == 0 {
        label-align + (if valign == "top" { top } else if valign == "bottom" { bottom } else { horizon })
      } else { left + top },
      grid.cell(inset: (left: label-padding.left, right: label-padding.right), label),
      grid.cell(stroke: (left: rule), inset: (top: padding.top, bottom: padding.bottom, left: padding.left), body),
    ),
  ))
}

// A listing, source or literal block.
#let adoc-code(body, fits: false) = {
  let size = resolve-size(pick("code_font_size", default: base-size), base-size)
  let ratio = pick("code_line_height", default: 1.2)
  let conum-color = pick("conum_font_color", "codespan_font_color", default: base-color)
  keep(fits: fits, block(
    width: 100%,
    fill: pick("code_background_color"),
    stroke: border(pick("code_border_width", default: 0.5pt), pick("code_border_color")),
    radius: pick("code_border_radius", default: 0pt),
    inset: {
      // Prawn pads a block of lines with half its leading above and below, and its line
      // height includes the font's line gap.
      let gap = doc.line-gaps.at(lower(pick("code_font_family", "codespan_font_family", default: "")), default: 0) * size
      let padding = inset(pick("code_padding", default: 9pt))
      padding.top += leading(size, ratio) / 2 + gap
      padding.bottom += leading(size, ratio) / 2
      padding
    },
    breakable: true,
    {
      let gap = doc.line-gaps.at(lower(pick("code_font_family", "codespan_font_family", default: "")), default: 0) * size
      set par(leading: leading(size, ratio) + gap, justify: false)
      set text(size: size)
      show raw: set text(font: families(pick("code_font_family", "codespan_font_family")), size: size)
      show raw: it => {
        show regex("[\u{2460}-\u{2473}]"): set text(fill: conum-color, font: families(pick("conum_font_family", "codespan_font_family")))
        it
      }
      if pick("code_font_color") != none { text(fill: pick("code_font_color"), body) } else { body }
    },
  ))
}

// A listing highlighted as the preview highlights it: each line of the `raw` block drawn as its
// runs, `(text, fill, bold, italic)`, so the block stays a listing, in the code font, with its
// callouts coloured. A run without a fill keeps the code's colour.
#let adoc-highlighted(lines, body) = {
  show raw.line: it => {
    let runs = lines.at(it.number - 1, default: none)
    if runs == none { return it }
    for (content, fill, bold, italic) in runs {
      let args = (:)
      if fill != none { args.fill = fill }
      if bold { args.weight = "bold" }
      if italic { args.style = "italic" }
      text(..args, content)
    }
  }
  body
}

#let adoc-callouts(items) = context {
  let color = pick("conum_font_color", "codespan_font_color", default: base-color)
  let markers = items.map(((number, _)) => text(fill: color, font: families(pick("conum_font_family", "codespan_font_family")), str.from-unicode(0x245F + number)))
  // Closer to the listing above than a block margin, as the theme asks.
  v(pick("callout_list_margin_top_after_code", default: 0pt))
  grid(
    columns: (auto, 1fr),
    column-gutter: measure(text("x")).width,
    row-gutter: list-gap,
    ..items.zip(markers).map((((_, body), marker)) => (par(marker), body)).flatten(),
  )
}

#let adoc-attribution(attribution, citation, prefix) = {
  if attribution == none and citation == none { return }
  let parts = (attribution, citation).filter(part => part != none)
  v(pick("vertical_rhythm", default: 12pt))
  par(styled((prefix + "_cite",), [— ] + parts.join(", ")))
}

#let adoc-quote(body, attribution, citation, prefix: "quote", fits: false) = {
  let size = resolve-size(pick(prefix + "_font_size", default: base-size), base-size)
  keep(fits: fits, block(
    width: 100%,
    stroke: (left: border(pick(prefix + "_border_left_width", default: 4pt), pick(prefix + "_border_color", "base_border_color"))),
    inset: inset(pick(prefix + "_padding", default: (3pt, 12pt, 3pt, 14pt))),
    breakable: true,
    {
      set par(leading: leading(size, base-line-height))
      styled((prefix,), body, size: size)
      adoc-attribution(attribution, citation, prefix)
    },
  ))
}

// A sidebar or example: a framed container with an optional title inside (sidebar) or a
// caption above (example, emitted by the caller).
#let adoc-container(prefix, title, body, fits: false) = keep(fits: fits, block(
  width: 100%,
  fill: pick(prefix + "_background_color"),
  stroke: border(pick(prefix + "_border_width", default: 0.5pt), pick(prefix + "_border_color", "base_border_color")),
  radius: pick(prefix + "_border_radius", default: 0pt),
  inset: inset(pick(prefix + "_padding", default: 12pt)),
  breakable: true,
  {
    if title != none {
      let size = resolve-size(pick(prefix + "_title_font_size", "heading_h4_font_size", default: base-size), base-size)
      set par(leading: leading(size, pick(prefix + "_title_line_height", "heading_line_height", default: 1.15)))
      block(
        sticky: true,
        width: 100%,
        inset: (bottom: pick("heading_margin_bottom", default: 12pt)),
        align(alignment(pick(prefix + "_title_text_align", default: "center"), default: center),
          par(styled((prefix + "_title", "heading"), title, size: size))),
      )
    }
    body
  },
))

#let adoc-open(body) = block(width: 100%, body)

#let adoc-break() = {
  let padding = inset(pick("thematic_break_padding", default: 0pt))
  v(padding.top)
  line(length: 100%, stroke: border(pick("thematic_break_border_width", default: 0.5pt), pick("thematic_break_border_color", "base_border_color", default: luma(200))))
  v(padding.bottom)
}

// A block image; the body passes the `image` itself, so an error loading it points at the
// AsciiDoc that named it.
#let adoc-image(image, align-to: none) = block(
  width: 100%,
  align(alignment(if align-to != none { align-to } else { pick("image_align", default: "left") }), image),
)

#let adoc-missing-image(alt) = block(text(fill: base-color.lighten(40%), [\[#alt\]]))

// A table: frame and grid from the theme, header row emphasised, optional stripes.
#let adoc-table(columns, header, rows, footer, stripes: none) = {
  let border-color = pick("table_border_color", "base_border_color", default: luma(200))
  let grid-color = pick("table_grid_color", "table_border_color", "base_border_color", default: luma(200))
  let border-width = pick("table_border_width", default: 0.5pt)
  let grid-width = pick("table_grid_width", "table_border_width", default: 0.5pt)
  let head-bottom = pick("table_head_border_bottom_width", default: grid-width)
  let stripe = pick("table_body_stripe_background_color")
  let head-rows = if header == none { 0 } else { 1 }
  let body-fill(y) = {
    let row = y - head-rows
    if y < head-rows { pick("table_head_background_color") }
    else if stripe != none and stripes == "even" and calc.rem(row, 2) == 1 { stripe }
    else if stripe != none and stripes == "odd" and calc.rem(row, 2) == 0 { stripe }
    else if stripe != none and stripes == "all" { stripe }
    else { pick("table_background_color") }
  }
  block(width: 100%, {
    set par(justify: false)
    set text(size: resolve-size(pick("table_font_size", default: base-size), base-size))
    table(
      columns: columns,
      inset: inset(pick("table_cell_padding", default: 2pt)),
      fill: (_, y) => body-fill(y),
      stroke: (x, y) => (
        top: if y == 0 { border(border-width, border-color) } else if y == head-rows and head-rows > 0 { border(head-bottom, grid-color) } else { border(grid-width, grid-color) },
        bottom: border(grid-width, grid-color),
        left: border(if x == 0 { border-width } else { grid-width }, if x == 0 { border-color } else { grid-color }),
        right: border(border-width, border-color),
      ),
      ..if header != none { (table.header(..header.map(cell => [#styled(("table_head",), cell)])),) } else { () },
      ..rows,
      ..if footer != none { (table.footer(..footer.map(cell => [#styled(("table_foot",), cell)])),) } else { () },
    )
  })
}

#let adoc-unsupported(source) = adoc-code(raw(source, block: true))

// Footnotes, gathered at the end of the document as asciidoctor-pdf places them. Every item has
// the item spacing below it, the last one too, and it is that margin that meets the page's
// bottom margin, as asciidoctor-pdf aligns the block.
#let adoc-footnotes(items) = {
  if items.len() == 0 { return }
  let spacing = pick("footnotes_item_spacing", default: 3pt)
  place(bottom, float: true, clearance: block-bottom, {
    set text(size: resolve-size(pick("footnotes_font_size", default: base-size * 0.75), base-size), fill: pick("footnotes_font_color", default: base-color))
    set par(justify: false)
    stack(spacing: spacing, ..items.map(((number, dest, body)) => [#adoc-link(dest, [\[#number\]]) #body]))
    v(spacing)
  })
}

// The index: every term the document marks, grouped by first letter, each with the body pages
// it is on as links, in `index_column_count` columns, as asciidoctor-pdf lays it out.
#let adoc-index() = context {
  let entries = query(<adoc-index-term>).map(marker => (
    terms: marker.value,
    page: counter(page).at(marker.location()).first(),
    location: marker.location(),
  ))
  if entries.len() == 0 { return }
  let indent = pick("index_term_indent", "list_indent", default: 18pt)
  let entry(level, name, pages) = {
    set par(hanging-indent: indent, justify: false)
    pad(left: indent * level, par({
      name
      if pages.len() > 0 {
        [, ]
        pages.map(((number, location)) => link(location, str(number))).join([, ])
      }
    }))
  }
  // The terms beneath `prefix` at `level`, sorted, each followed by its own subterms.
  let terms-at(level, prefix) = {
    let beneath = entries.filter(e => e.terms.len() > level and e.terms.slice(0, level) == prefix)
    let names = beneath.map(e => e.terms.at(level)).dedup().sorted(key: name => lower(name))
    for name in names {
      let own = beneath.filter(e => e.terms.at(level) == name and e.terms.len() == level + 1)
      let pages = ()
      for e in own.sorted(key: e => e.page) {
        if pages.len() == 0 or pages.last().at(0) != e.page { pages.push((e.page, e.location)) }
      }
      entry(level, name, pages)
      terms-at(level + 1, prefix + (name,))
    }
  }
  let letters = entries.map(e => upper(e.terms.first().first())).dedup().sorted()
  columns(pick("index_column_count", default: 2), gutter: pick("index_column_gap", default: 12pt), {
    for letter in letters {
      block(sticky: true, above: prose-bottom, below: list-gap, styled(("index_category", "heading_h4", "heading"), strong(letter)))
      let beneath = entries.filter(e => upper(e.terms.first().first()) == letter)
      let names = beneath.map(e => e.terms.first()).dedup().sorted(key: name => lower(name))
      for name in names {
        let own = beneath.filter(e => e.terms.first() == name and e.terms.len() == 1)
        let pages = ()
        for e in own.sorted(key: e => e.page) {
          if pages.len() == 0 or pages.last().at(0) != e.page { pages.push((e.page, e.location)) }
        }
        entry(0, name, pages)
        terms-at(1, (name,))
      }
    }
  })
}

// A part of a book: its title alone at the top of a new page, as asciidoctor-pdf sets it; the
// chapter that follows starts the next page.
#let adoc-part(title) = {
  pagebreak(weak: true)
  heading(level: 1, outlined: true, bookmarked: true, title)
}

// The table of contents: the title as a level 2 heading, then an entry per outlined heading
// with dot leaders and its page number.
#let adoc-toc(min-level: 2) = {
  heading(level: 2, outlined: false, bookmarked: false, doc.toc-title)
  let size = resolve-size(pick("toc_font_size", default: base-size), base-size)
  let ratio = pick("toc_line_height", default: base-line-height)
  let indent = pick("toc_indent", default: 15pt)
  let dots = pick("toc_dot_leader_content", default: ". ")
  let dot-color = pick("toc_dot_leader_font_color", default: base-color)
  let space = leading(size, ratio)
  show outline.entry: it => {
    let level = it.level
    let prefixes = ("toc_h" + str(level), "toc")
    block(inset: (y: space / 2), link(it.element.location(), pad(left: indent * (level - min-level), {
      set par(justify: false, leading: space)
      styled(prefixes, it.element.body, size: size)
      [ ]
      box(width: 1fr, repeat(text(size: size, fill: dot-color, dots)))
      [ ]
      styled(prefixes, it.page(), size: size)
    })))
  }
  outline(title: none, target: heading.where(outlined: true))
}

// --- title page and running content ----------------------------------------------------------

// A theme `top` (a length, or a percentage of the area inside the page margins) from the top
// of the page.
#let from-top(value) = {
  let margin = doc.page.margin
  let area = doc.page.height - margin.top - margin.bottom
  margin.top + if type(value) == ratio { value * area } else if value == none { 0pt } else { value }
}

#let adoc-title-page(title: none, subtitle: none, authors: none, revision: none, logo: none) = {
  let a = pick("title_page_text_align", "base_text_align", default: "left")
  let alignment = if a == "center" { center } else if a == "right" { right } else { left }
  let margin = doc.page.margin
  let width = doc.page.width - margin.left - margin.right
  let part(prefix, body, heading: false) = {
    let size = resolve-size(pick(prefix + "_font_size", default: base-size), base-size)
    let ratio = pick(prefix + "_line_height", default: base-line-height)
    let body = styled((prefix, "title_page"), body, size: size)
    block(
      above: pick(prefix + "_margin_top", default: 0pt),
      below: pick(prefix + "_margin_bottom", default: 0pt),
      width: 100%,
      {
        set par(leading: leading(size, ratio), justify: false)
        align(alignment, if heading { plain-heading(body) } else { body })
      },
    )
  }
  // Labelled `i` in a PDF viewer, as asciidoctor-pdf labels its front matter.
  page(
    numbering: "i",
    fill: pick("title_page_background_color", "page_background_color"),
    {
      if logo != none and pick("title_page_logo_display") != "none" {
        let la = pick("title_page_logo_align", default: a)
        place(top + left, dy: from-top(pick("title_page_logo_top", default: 10%)) - margin.top,
          block(width: width, align(if la == "center" { center } else if la == "right" { right } else { left }, logo)))
      }
      place(top + left, dy: from-top(pick("title_page_title_top", default: 40%)) - margin.top, block(width: width, {
        if pick("title_page_title_display") != "none" { part("title_page_title", title, heading: true) }
        if subtitle != none and pick("title_page_subtitle_display") != "none" { part("title_page_subtitle", subtitle) }
        if authors != none and pick("title_page_authors_display") != "none" { part("title_page_authors", authors) }
        if revision != none and pick("title_page_revision_display") != "none" { part("title_page_revision", revision) }
      }))
    },
  )
}

// The headings a running title can name at `levels`, each with the page it starts on, in
// document order. It does not depend on the page asking, so Typst works it out once for every
// page instead of filtering the document's headings again on each.
#let running-headings(levels) = (
  query(heading)
    .filter(h => h.level >= levels.at(0) and h.level <= levels.at(1) and h.bookmarked == true)
    .map(h => (h.location().page(), h.body))
)

// The heading that a running title names on the page being drawn: the first that starts on
// it, else the last before it, as asciidoctor-pdf indexes sections by the page they start on.
#let running-title(levels) = {
  if levels == none { return [] }
  let page-number = here().page()
  let found = running-headings(levels)
  // The first heading starting on this page or after it.
  let (low, high) = (0, found.len())
  while low < high {
    let middle = calc.div-euclid(low + high, 2)
    if found.at(middle).at(0) < page-number { low = middle + 1 } else { high = middle }
  }
  if low < found.len() and found.at(low).at(0) == page-number { return found.at(low).at(1) }
  if low > 0 { found.at(low - 1).at(1) } else { [] }
}

// The header or footer of the page being drawn, from the theme's `<periphery>_*` keys and the
// generated `running` content.
#let adoc-periphery(periphery, contents) = context {
  let height = pick(periphery + "_height")
  if height == none or contents == none { return }
  let starts = query(<adoc-running-start>)
  if starts.len() == 0 or here().page() < starts.first().location().page() { return }
  // Counted from the page the body starts on: a counter update there comes after this page's
  // background is laid out.
  let number = here().page() - starts.first().location().page() + 1
  if number < doc.running-from { return }
  let side = if calc.odd(number) { "recto" } else { "verso" }
  let columns = contents.at(side, default: (:))
  let section = running-title(doc.running.section)
  let chapter = running-title(doc.running.chapter)
  let ctx = (
    "page-number": str(number),
    "page-count": str(counter(page).final().first()),
    "part-title": running-title(doc.running.part),
    "chapter-title": chapter,
    "section-title": section,
    "section-or-chapter-title": if section != [] { section } else { chapter },
  )
  let margin = doc.page.margin
  let width = doc.page.width - margin.left - margin.right
  let padding = inset(pick(periphery + "_padding", default: 0pt))
  let rule = pick(periphery + "_border_width", default: 0pt)
  let rule-color = pick(periphery + "_border_color", "base_border_color", default: luma(200))
  let valign = pick(periphery + "_vertical_align", default: "middle")
  let size = resolve-size(pick(periphery + "_font_size", default: base-size), base-size)
  let ratio = pick(periphery + "_line_height", default: base-line-height)
  let offset = if periphery == "footer" { rule / 2 } else { 0pt }
  let inner-width = width - padding.left - padding.right
  let inner-height = height - padding.top - padding.bottom - rule / 2
  place(top + left, dx: margin.left, dy: if periphery == "header" { 0pt } else { doc.page.height - height },
    block(width: width, height: height, fill: pick(periphery + "_background_color"), {
      if rule > 0pt {
        place(if periphery == "header" { bottom } else { top } + left, line(length: 100%, stroke: rule + rule-color))
      }
      place(top + left, dx: padding.left, dy: padding.top + offset, block(width: inner-width, height: inner-height, {
        set par(leading: leading(size, ratio), justify: false)
        for position in ("left", "center", "right") {
          let column = columns.at(position, default: none)
          if column == none { continue }
          let h-align = alignment(column.align)
          let v-align = if valign == "top" { top } else if valign == "bottom" { bottom } else { horizon }
          place(top + left, dx: column.x * inner-width, block(width: column.width * inner-width, height: inner-height,
            align(h-align + v-align, styled((periphery + "_" + side + "_" + position, periphery), (column.body)(ctx), size: size))))
        }
      }))
    }))
}

// --- page -------------------------------------------------------------------------------------

#let adoc-page-setup(body, running: (:)) = {
  set page(
    width: doc.page.width,
    height: doc.page.height,
    margin: doc.page.margin,
    fill: pick("page_background_color"),
    numbering: "1",
    header: none,
    footer: none,
    background: {
      adoc-periphery("header", running.at("header", default: none))
      adoc-periphery("footer", running.at("footer", default: none))
    },
  )
  set text(
    font: families(pick("base_font_family")),
    size: base-size,
    fill: base-color,
    top-edge: "ascender",
    bottom-edge: "descender",
    hyphenate: doc.hyphenate,
    lang: doc.lang,
    region: doc.region,
  )
  let justify = pick("base_text_align", default: "left") == "justify"
  // Lines fill greedily and spaces only stretch, as Prawn justifies.
  set par(
    justify: justify,
    leading: leading(base-size, base-line-height),
    spacing: 0pt,
    linebreaks: "simple",
    justification-limits: (spacing: (min: 100%, max: 1000%)),
  )
  let a = pick("base_text_align", default: "left")
  set align(if a == "center" { center } else if a == "right" { right } else { left })
  set block(spacing: 0pt)
  // Prawn gives every paragraph half its leading above and below; Typst only puts leading
  // between lines.
  // (As spacing around the paragraph: wrapping it in a block here would cut its links off
  // from their place in a tagged PDF's structure.)
  show par: it => context {
    let gap = line-gap() * text.size
    let half = par.leading / 2
    set par(leading: par.leading + gap)
    v(half + gap)
    it
    v(half)
  }
  set smartquote(enabled: false)
  body
}
