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

#let block-bottom = pick("block_margin_bottom", default: 12pt)
#let prose-bottom = pick("prose_margin_bottom", default: block-bottom)

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
    inset: (x: pick("kbd_border_offset", default: 2pt)),
    outset: (y: pick("kbd_border_offset", default: 2pt)),
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

#let adoc-footnote-ref(number, dest) = super(adoc-link(dest, [\[#number\]]))

// --- blocks -----------------------------------------------------------------------------------

// A block with its caption ("Table 1. Title") above or below it, `caption_margin_inside`
// apart.
#let adoc-captioned(caption, body, below: false) = {
  let inside = pick("caption_margin_inside", default: 4pt)
  let outside = pick("caption_margin_outside", default: 0pt)
  let align-to = pick("caption_text_align", "caption_align", default: "left")
  let caption = align(if align-to == "center" { center } else if align-to == "right" { right } else { left },
    styled(("caption",), caption))
  block(
    spacing: block-bottom + outside,
    width: 100%,
    breakable: true,
    grid(
      columns: 1fr,
      row-gutter: inside,
      ..if below { (body, caption) } else { (caption, body) },
    ),
  )
}

#let adoc-paragraph(roles, body) = {
  let body = body
  for role in roles { body = adoc-role(role, body) }
  if roles.contains("lead") {
    let size = resolve-size(pick("role_lead_font_size", default: base-size * 1.25), base-size)
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
  let align-to = pick("heading_h1_text_align", default: "center")
  set par(leading: leading(size, pick("heading_h1_line_height", "heading_line_height", default: 1.15)), justify: false)
  block(
    below: pick("heading_h1_margin_bottom", "heading_margin_bottom", default: 12pt),
    width: 100%,
    align(if align-to == "left" { left } else if align-to == "right" { right } else { center },
      styled(("heading_h1", "heading"), body)),
  )
}

#show heading: it => {
  let level = it.level
  let prefixes = ("heading_h" + str(level), "heading")
  let size = resolve-size(pick("heading_h" + str(level) + "_font_size", "heading_font_size", default: base-size), base-size)
  let top = pick("heading_h" + str(level) + "_margin_top", "heading_margin_top", default: 0pt)
  let bottom = pick("heading_h" + str(level) + "_margin_bottom", "heading_margin_bottom", default: 0pt)
  let align-to = pick("heading_h" + str(level) + "_text_align", "heading_text_align", default: "left")
  let ratio = pick("heading_h" + str(level) + "_line_height", "heading_line_height", default: 1.15)
  set par(leading: leading(size, ratio), justify: false)
  block(
    above: prose-bottom + top,
    below: bottom,
    sticky: true,
    width: 100%,
    align(if align-to == "center" { center } else if align-to == "right" { right } else { left },
      styled(prefixes, it.body, size: size)),
  )
}

// A list: markers in a column as wide as the indent, so the text of every item lines up.
#let adoc-list(markers, items, nested: false) = context {
  let indent = pick("list_indent", default: 30pt)
  let spacing = pick("list_item_spacing", default: 6pt)
  let gap = measure(text(" ")).width
  let marker-color = pick("list_marker_font_color")
  block(
    spacing: if nested { spacing } else { block-bottom },
    grid(
      columns: (indent, 1fr),
      row-gutter: spacing,
      ..items.zip(markers).map(((item, marker)) => (
        align(right, {
          let marker = if marker-color != none { text(fill: marker-color, marker) } else { marker }
          marker
          h(gap)
        }),
        {
          set par(spacing: spacing)
          item
        },
      )).flatten(),
    ),
  )
}

#let adoc-dlist(items) = {
  let indent = pick("description_list_description_indent", default: 15pt)
  let spacing = pick("description_list_term_spacing", default: 4pt)
  let item-spacing = pick("list_item_spacing", default: 6pt)
  for (index, (term, description)) in items.enumerate() {
    block(
      above: if index == 0 { block-bottom } else { item-spacing + spacing },
      below: spacing,
      sticky: true,
      styled(("description_list_term",), term),
    )
    if description != none {
      block(above: spacing, below: block-bottom, pad(left: indent, description))
    }
  }
}

#let adoc-hlist(items) = {
  let spacing = pick("list_item_spacing", default: 6pt)
  block(spacing: block-bottom, grid(
    columns: (auto, 1fr),
    column-gutter: pick("description_list_description_indent", default: 15pt),
    row-gutter: spacing,
    ..items.map(((term, description)) => (styled(("description_list_term",), term), if description == none { [] } else { description })).flatten(),
  ))
}

#let adoc-admonition(kind, label, body) = {
  let padding = inset(pick("admonition_padding", default: (4pt, 12pt, 4pt, 12pt)))
  let label-padding = inset(pick("admonition_label_padding", default: pick("admonition_padding", default: (4pt, 12pt, 4pt, 12pt))))
  let rule = border(pick("admonition_column_rule_width", default: 0.5pt), pick("admonition_column_rule_color", "base_border_color"))
  let label = styled(("admonition_label_" + kind, "admonition_label"), transform(pick("admonition_label_" + kind + "_text_transform", "admonition_label_text_transform"), label))
  let label-align = pick("admonition_label_text_align", default: "center")
  let valign = pick("admonition_label_vertical_align", default: "middle")
  block(
    spacing: block-bottom,
    width: 100%,
    fill: pick("admonition_background_color"),
    stroke: border(pick("admonition_border_width"), pick("admonition_border_color")),
    radius: pick("admonition_border_radius", default: 0pt),
    breakable: true,
    grid(
      columns: (auto, 1fr),
      align: (x, y) => (if x == 0 {
        (if label-align == "left" { left } else if label-align == "right" { right } else { center }) + (if valign == "top" { top } else if valign == "bottom" { bottom } else { horizon })
      } else { left + top }),
      grid.cell(inset: (left: label-padding.left, right: label-padding.right), label),
      grid.cell(
        stroke: (left: rule),
        inset: (top: padding.top, bottom: padding.bottom, left: padding.left),
        {
          set par(spacing: prose-bottom)
          set block(spacing: prose-bottom)
          body
        },
      ),
    ),
  )
}

// A listing, source or literal block.
#let adoc-code(body, conums: false) = {
  let size = resolve-size(pick("code_font_size", default: base-size), base-size)
  let ratio = pick("code_line_height", default: 1.2)
  let conum-color = pick("conum_font_color", "codespan_font_color", default: base-color)
  block(
    spacing: block-bottom,
    width: 100%,
    fill: pick("code_background_color"),
    stroke: border(pick("code_border_width", default: 0.5pt), pick("code_border_color")),
    radius: pick("code_border_radius", default: 0pt),
    inset: inset(pick("code_padding", default: 9pt)),
    breakable: true,
    {
      set par(leading: leading(size, ratio), justify: false)
      set text(size: size)
      show raw: set text(font: families(pick("code_font_family", "codespan_font_family")), size: size)
      show raw: it => {
        show regex("[\u{2460}-\u{2473}]"): set text(fill: conum-color, font: families(pick("conum_font_family", "codespan_font_family")))
        it
      }
      let body = if pick("code_font_color") != none { text(fill: pick("code_font_color"), body) } else { body }
      body
    },
  )
}

#let adoc-callouts(items) = context {
  let color = pick("conum_font_color", "codespan_font_color", default: base-color)
  let markers = items.map(((number, _)) => text(fill: color, font: families(pick("conum_font_family", "codespan_font_family")), str.from-unicode(0x245F + number)))
  let spacing = pick("list_item_spacing", default: 6pt)
  // Closer to the listing above than block spacing allows, as the theme asks.
  v(pick("callout_list_margin_top_after_code", default: 0pt))
  block(
    above: block-bottom,
    below: block-bottom,
    grid(
      columns: (auto, 1fr),
      column-gutter: measure(text(" ")).width,
      row-gutter: spacing,
      ..items.zip(markers).map((((_, body), marker)) => (marker, body)).flatten(),
    ),
  )
}

#let adoc-attribution(attribution, citation, prefix) = {
  if attribution == none and citation == none { return }
  let parts = (attribution, citation).filter(part => part != none)
  block(above: pick("vertical_rhythm", default: 12pt), styled((prefix + "_cite",), [— ] + parts.join(", ")))
}

#let adoc-quote(body, attribution, citation, prefix: "quote") = {
  let padding = inset(pick(prefix + "_padding", default: (3pt, 12pt, 3pt, 14pt)))
  let size = resolve-size(pick(prefix + "_font_size", default: base-size), base-size)
  block(
    spacing: block-bottom,
    width: 100%,
    stroke: (left: border(pick(prefix + "_border_left_width", default: 4pt), pick(prefix + "_border_color", "base_border_color"))),
    inset: padding,
    breakable: true,
    {
      set par(leading: leading(size, base-line-height))
      styled((prefix,), body, size: size)
      adoc-attribution(attribution, citation, prefix)
    },
  )
}

// A sidebar or example: a framed container with an optional title inside (sidebar) or a
// caption above (example, emitted by the caller).
#let adoc-container(prefix, title, body) = {
  let padding = inset(pick(prefix + "_padding", default: 12pt))
  block(
    spacing: block-bottom,
    width: 100%,
    fill: pick(prefix + "_background_color"),
    stroke: border(pick(prefix + "_border_width", default: 0.5pt), pick(prefix + "_border_color", "base_border_color")),
    radius: pick(prefix + "_border_radius", default: 0pt),
    inset: padding,
    breakable: true,
    {
      set block(spacing: prose-bottom)
      if title != none {
        let align-to = pick(prefix + "_title_text_align", default: "center")
        let size = resolve-size(pick(prefix + "_title_font_size", "heading_h4_font_size", default: base-size), base-size)
        block(
          below: pick("heading_margin_bottom", default: 12pt),
          sticky: true,
          width: 100%,
          align(if align-to == "left" { left } else if align-to == "right" { right } else { center },
            styled((prefix + "_title", "heading"), title, size: size)),
        )
      }
      body
    },
  )
}

#let adoc-open(body) = block(spacing: block-bottom, width: 100%, body)

#let adoc-break() = {
  let padding = inset(pick("thematic_break_padding", default: 0pt))
  block(
    above: block-bottom + padding.top,
    below: block-bottom + padding.bottom,
    line(length: 100%, stroke: border(pick("thematic_break_border_width", default: 0.5pt), pick("thematic_break_border_color", "base_border_color", default: luma(200)))),
  )
}

// A block image; the body passes the `image` itself, so an error loading it points at the
// AsciiDoc that named it.
#let adoc-image(image, align-to: none) = {
  let align-to = if align-to != none { align-to } else { pick("image_align", default: "left") }
  block(
    spacing: block-bottom,
    width: 100%,
    align(if align-to == "center" { center } else if align-to == "right" { right } else { left }, image),
  )
}

#let adoc-missing-image(alt) = block(spacing: block-bottom, text(fill: base-color.lighten(40%), [\[#alt\]]))

// A table: frame and grid from the theme, header row emphasised, optional stripes.
#let adoc-table(columns, header, rows, footer, stripes: none, align-cells: ()) = {
  let border-color = pick("table_border_color", "base_border_color", default: luma(200))
  let grid-color = pick("table_grid_color", "table_border_color", "base_border_color", default: luma(200))
  let border-width = pick("table_border_width", default: 0.5pt)
  let grid-width = pick("table_grid_width", "table_border_width", default: 0.5pt)
  let head-bottom = pick("table_head_border_bottom_width", default: grid-width)
  let cell-padding = inset(pick("table_cell_padding", default: 2pt))
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
  block(spacing: block-bottom, width: 100%, {
    set par(justify: false, spacing: prose-bottom)
    set text(size: resolve-size(pick("table_font_size", default: base-size), base-size))
    table(
      columns: columns,
      inset: cell-padding,
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

// Footnotes, gathered at the end of the document as asciidoctor-pdf places them.
#let adoc-footnotes(items) = {
  if items.len() == 0 { return }
  place(bottom, float: true, clearance: block-bottom, {
    set text(size: resolve-size(pick("footnotes_font_size", default: base-size * 0.75), base-size), fill: pick("footnotes_font_color", default: base-color))
    set par(justify: false)
    let spacing = pick("footnotes_item_spacing", default: 3pt)
    stack(spacing: spacing, ..items.map(((number, dest, body)) => [#adoc-link(dest, [\[#number\]]) #body]))
  })
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
    block(above: space, below: space, link(it.element.location(), pad(left: indent * (level - min-level), {
      set par(justify: false, leading: space)
      styled(prefixes, it.element.body, size: size)
      [ ]
      box(width: 1fr, repeat(text(size: size, fill: dot-color, dots)))
      [ ]
      styled(prefixes, it.page(), size: size)
    })))
  }
  block(below: block-bottom, outline(title: none, target: heading.where(outlined: true)))
}

// --- page -------------------------------------------------------------------------------------

#let adoc-page-setup(body) = {
  set page(
    width: doc.page.width,
    height: doc.page.height,
    margin: doc.page.margin,
    fill: pick("page_background_color"),
    numbering: "1",
    header: none,
    footer: none,
  )
  set text(
    font: families(pick("base_font_family")),
    size: base-size,
    fill: base-color,
    top-edge: "ascender",
    bottom-edge: "descender",
    hyphenate: doc.hyphenate,
    lang: doc.lang,
  )
  let justify = pick("base_text_align", default: "left") == "justify"
  // Lines fill greedily and spaces only stretch, as Prawn justifies.
  set par(
    justify: justify,
    leading: leading(base-size, base-line-height),
    spacing: prose-bottom,
    linebreaks: "simple",
    justification-limits: (spacing: (min: 100%, max: 1000%)),
  )
  let a = pick("base_text_align", default: "left")
  set align(if a == "center" { center } else if a == "right" { right } else { left })
  set block(spacing: block-bottom)
  set smartquote(enabled: false)
  body
}
