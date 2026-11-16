//! Tables: AsciiDoc's as real Word tables, with header rows, column widths, alignment and
//! cells spanning columns (`gridSpan`) and rows (`vMerge`), and admonitions, sidebars and
//! examples as tables of one row, so their blocks stay editable inside them.

use adoc_render::{Align, CellStyle, ColumnWidth, VAlign, ViewBlock, ViewCell, ViewTable};

use crate::body::{Format, Writer};

/// The space a cell's margins take from its width, in twentieths of a point.
const CELL_MARGINS: u32 = 216;

/// A cell's content as written: what spans it and how it looks.
struct Cell<'a> {
    width: u32,
    span: usize,
    /// `Some(true)` starts a vertical merge, `Some(false)` continues one.
    merge: Option<bool>,
    valign: Option<&'static str>,
    content: Option<(&'a [ViewBlock], Format, Option<&'static str>)>,
}

impl Writer<'_> {
    /// Opens a table of `style` filling the width, with its columns' widths.
    fn open_table(&mut self, style: &str, widths: &[u32]) {
        // Two tables in a row would run together into one.
        self.close_tables();
        self.xml.open("w:tbl", &[]);
        self.xml.open("w:tblPr", &[]);
        let style = self.style_id(style);
        self.xml.empty("w:tblStyle", &[("w:val", &style)]);
        self.xml
            .empty("w:tblW", &[("w:w", "5000"), ("w:type", "pct")]);
        self.xml.empty(
            "w:tblLook",
            &[
                ("w:val", "04A0"),
                ("w:firstRow", "1"),
                ("w:lastRow", "0"),
                ("w:firstColumn", "0"),
                ("w:lastColumn", "0"),
                ("w:noHBand", "1"),
                ("w:noVBand", "1"),
            ],
        );
        self.xml.close("w:tblPr");
        self.xml.open("w:tblGrid", &[]);
        for width in widths {
            self.xml.empty("w:gridCol", &[("w:w", &width.to_string())]);
        }
        self.xml.close("w:tblGrid");
    }

    fn close_table(&mut self) {
        self.xml.close("w:tbl");
        self.after_table = true;
    }

    /// A cell: its properties, then its blocks in its own context, ending with a paragraph.
    fn cell(&mut self, cell: &Cell<'_>, depth: u8) {
        self.xml.open("w:tc", &[]);
        self.xml.open("w:tcPr", &[]);
        self.xml.empty(
            "w:tcW",
            &[("w:w", &cell.width.to_string()), ("w:type", "dxa")],
        );
        if cell.span > 1 {
            self.xml
                .empty("w:gridSpan", &[("w:val", &cell.span.to_string())]);
        }
        match cell.merge {
            Some(true) => self.xml.empty("w:vMerge", &[("w:val", "restart")]),
            Some(false) => self.xml.empty("w:vMerge", &[]),
            None => {}
        }
        if let Some(valign) = cell.valign {
            self.xml.empty("w:vAlign", &[("w:val", valign)]);
        }
        self.xml.close("w:tcPr");
        let context = (
            self.indent,
            self.quote,
            self.base,
            self.align,
            self.width,
            self.after_table,
        );
        let mut wrote = false;
        if let Some((blocks, base, align)) = cell.content {
            self.indent = 0;
            self.quote = false;
            self.base = base;
            self.align = align;
            self.width = cell.width.saturating_sub(CELL_MARGINS).max(720);
            self.after_table = false;
            let before = self.xml_len();
            self.blocks(blocks, depth);
            wrote = self.xml_len() > before;
        }
        // A cell ends with a paragraph: after a table, or when it has nothing else.
        if !wrote || self.after_table {
            self.after_table = false;
            self.xml.open("w:p", &[]);
            for name in std::mem::take(&mut self.pending) {
                self.bookmark(&name);
            }
            self.xml.close("w:p");
        }
        (
            self.indent,
            self.quote,
            self.base,
            self.align,
            self.width,
            self.after_table,
        ) = context;
        self.xml.close("w:tc");
    }

    /// An admonition, sidebar or example: a table of one row, the label (if any) in a cell of
    /// its own beside the blocks.
    pub(crate) fn container(
        &mut self,
        style: &str,
        label: Option<&str>,
        blocks: &[ViewBlock],
        depth: u8,
    ) {
        let label_width = label.map_or(0, |_| 1440.min(self.width / 4));
        let content_width = self.width - label_width;
        let widths: Vec<u32> = label
            .map(|_| label_width)
            .into_iter()
            .chain([content_width])
            .collect();
        self.open_table(style, &widths);
        self.xml.open("w:tr", &[]);
        if let Some(label) = label {
            let labelled = [label_block(label)];
            self.cell(
                &Cell {
                    width: label_width,
                    span: 1,
                    merge: None,
                    valign: None,
                    content: Some((
                        &labelled,
                        Format {
                            bold: true,
                            ..Format::default()
                        },
                        None,
                    )),
                },
                depth,
            );
        }
        self.cell(
            &Cell {
                width: content_width,
                span: 1,
                merge: None,
                valign: None,
                content: Some((blocks, Format::default(), None)),
            },
            depth,
        );
        self.xml.close("w:tr");
        self.close_table();
    }

    /// An AsciiDoc table.
    pub(crate) fn table(&mut self, block: &ViewBlock, table: &ViewTable, depth: u8) {
        let rows: Vec<(&Vec<ViewCell>, bool)> = table
            .header
            .iter()
            .map(|row| (row, true))
            .chain(table.rows.iter().map(|row| (row, false)))
            .chain(table.footer.iter().map(|row| (row, false)))
            .collect();
        let columns = if table.columns.is_empty() {
            rows.first().map_or(1, |(row, _)| {
                row.iter().map(|cell| cell.colspan.max(1)).sum()
            })
        } else {
            table.columns.len()
        }
        .max(1);
        let widths = column_widths(table, columns, self.width);
        self.open_table("AdocTable", &widths);
        // How many more rows each column is taken by a cell from above, and that cell's span.
        let mut taken: Vec<(usize, usize)> = vec![(0, 1); columns];
        let mut dropped = false;
        for (row, header) in rows {
            self.xml.open("w:tr", &[]);
            if header {
                self.xml.open("w:trPr", &[]);
                self.xml.empty("w:tblHeader", &[]);
                self.xml.close("w:trPr");
            }
            let mut cells = row.iter();
            let mut column = 0;
            while column < columns {
                let (rows_left, span) = taken[column];
                if rows_left > 0 {
                    let span = span.min(columns - column);
                    taken[column].0 -= 1;
                    self.cell(
                        &Cell {
                            width: widths[column..column + span].iter().sum(),
                            span,
                            merge: Some(false),
                            valign: None,
                            content: None,
                        },
                        depth,
                    );
                    column += span;
                    continue;
                }
                let Some(cell) = cells.next() else {
                    // A short row: the rest of the grid is empty cells.
                    self.cell(
                        &Cell {
                            width: widths[column],
                            span: 1,
                            merge: None,
                            valign: None,
                            content: None,
                        },
                        depth,
                    );
                    column += 1;
                    continue;
                };
                let span = cell.colspan.max(1).min(columns - column);
                if cell.rowspan > 1 {
                    taken[column] = (cell.rowspan - 1, span);
                }
                let base = cell_format(cell.style, header);
                let align = match cell.align {
                    Align::Start => None,
                    Align::Centre => Some("center"),
                    Align::End => Some("right"),
                };
                let valign = match cell.valign {
                    VAlign::Top => None,
                    VAlign::Middle => Some("center"),
                    VAlign::Bottom => Some("bottom"),
                };
                self.cell(
                    &Cell {
                        width: widths[column..column + span].iter().sum(),
                        span,
                        merge: (cell.rowspan > 1).then_some(true),
                        valign,
                        content: Some((&cell.blocks, base, align)),
                    },
                    depth,
                );
                column += span;
            }
            dropped |= cells.next().is_some();
            self.xml.close("w:tr");
        }
        self.close_table();
        if dropped {
            self.warn(
                Some(&block.span),
                "a table row has more cells than the table has columns; the rest are left out"
                    .to_owned(),
            );
        }
    }

    /// The length of the part written so far, to tell whether a block wrote anything.
    fn xml_len(&self) -> usize {
        self.xml.len()
    }
}

/// A one-line paragraph block holding `text`, for an admonition's label.
fn label_block(text: &str) -> ViewBlock {
    ViewBlock {
        span: adoc_render::SourceSpan {
            path: std::path::PathBuf::new(),
            range: Default::default(),
            line: 0,
            include_chain: Vec::new(),
        },
        id: None,
        roles: Vec::new(),
        style: None,
        title: Vec::new(),
        caption: None,
        attributes: Default::default(),
        options: Default::default(),
        kind: adoc_render::ViewKind::Paragraph(vec![adoc_render::ViewInline::Text(
            text.to_owned(),
        )]),
    }
}

/// How a cell's text looks: its column's or its own style, a header's in bold.
fn cell_format(style: CellStyle, header: bool) -> Format {
    let mut format = Format::default();
    match style {
        CellStyle::Header | CellStyle::Strong => format.bold = true,
        CellStyle::Emphasis => format.italic = true,
        CellStyle::Literal | CellStyle::Monospace => format.style = Some("CodeChar"),
        CellStyle::AsciiDoc | CellStyle::Default => {}
    }
    format.bold |= header;
    format
}

/// Each column's width in twentieths of a point, sharing `width` as `cols` asks: proportions,
/// percentages, or equal shares for automatic widths.
fn column_widths(table: &ViewTable, columns: usize, width: u32) -> Vec<u32> {
    let weights: Vec<u32> = (0..columns)
        .map(
            |index| match table.columns.get(index).map(|column| column.width) {
                Some(ColumnWidth::Proportional(weight) | ColumnWidth::Percentage(weight)) => {
                    weight.max(1)
                }
                Some(ColumnWidth::Auto) | None => 1,
            },
        )
        .collect();
    let total: u64 = weights.iter().map(|weight| u64::from(*weight)).sum();
    weights
        .iter()
        .map(|weight| {
            u32::try_from(u64::from(width) * u64::from(*weight) / total.max(1))
                .unwrap_or(width)
                .max(1)
        })
        .collect()
}
