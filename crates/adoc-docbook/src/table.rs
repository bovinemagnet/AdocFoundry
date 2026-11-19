//! Tables as CALS tables, the table model DocBook's processors know best (PRD §5.2): `table`
//! with its title (`informaltable` without one), the frame and rules, one `tgroup` with a
//! `colspec` per column (its width, proportional, and alignment), then `thead`, `tfoot` and
//! `tbody`, in the order CALS gives them. Cells are placed on the grid as the table lays them
//! out, so a cell spanning columns names its first and last column (`namest`, `nameend`) and
//! one spanning rows says how many more it covers (`morerows`). A span past the table's edge
//! is cut back to it, with a warning, and a row every cell of which a span above covers, which
//! CALS cannot hold, is folded into the cells over it.

use adoc_dom::{Align, ColumnWidth, VAlign};
use adoc_render::{ViewBlock, ViewCell, ViewKind, ViewTable};

use crate::document::Writer;

/// One cell placed on the grid.
struct Placed<'a> {
    cell: &'a ViewCell,
    /// Its first column, from 0.
    column: usize,
    /// How many columns it covers, at least 1.
    columns: usize,
    /// How many rows after its own it covers.
    more_rows: usize,
}

/// A table's cells placed on its grid: its width, and its header, footer and body rows.
struct Grid<'a> {
    columns: usize,
    head: Vec<Vec<Placed<'a>>>,
    foot: Vec<Vec<Placed<'a>>>,
    body: Vec<Vec<Placed<'a>>>,
}

impl Writer<'_> {
    /// `table`'s rows placed on its grid, or `None`, with a warning, when it has none.
    fn grid<'a>(&mut self, block: &ViewBlock, table: &'a ViewTable) -> Option<Grid<'a>> {
        let header = table.header.as_ref().filter(|row| !row.is_empty());
        let footer = table.footer.as_ref().filter(|row| !row.is_empty());
        let mut body: Vec<&[ViewCell]> = table.rows.iter().map(Vec::as_slice).collect();
        let mut head: Vec<&[ViewCell]> = header.map(Vec::as_slice).into_iter().collect();
        let foot: Vec<&[ViewCell]> = footer.map(Vec::as_slice).into_iter().collect();
        // CALS needs a body row: a table of only a header row has it as its body.
        if body.is_empty() {
            body = std::mem::take(&mut head);
        }
        if body.is_empty() {
            self.warn("a table with no rows is left out", Some(&block.span));
            return None;
        }
        let widest = head
            .iter()
            .chain(&body)
            .chain(&foot)
            .map(|row| row.iter().map(|cell| cell.colspan.max(1)).sum::<usize>())
            .max()
            .unwrap_or(1);
        let columns = table.columns.len().max(widest).max(1);
        let mut clipped = false;
        let grid = Grid {
            columns,
            head: place(&head, columns, &mut clipped),
            foot: place(&foot, columns, &mut clipped),
            body: place(&body, columns, &mut clipped),
        };
        if clipped {
            self.warn(
                "a table cell spans past the edge of its table, so its span is cut back to it",
                Some(&block.span),
            );
        }
        Some(grid)
    }

    pub(crate) fn table(&mut self, block: &ViewBlock, table: &ViewTable, role: &[(&str, &str)]) {
        // In a table's cell, a table is the cell's `entrytbl` when it is all the cell holds
        // (`entry`); anywhere else there it may not be, so its rows are written as text.
        if self.forbidden("informaltable") {
            self.table_as_text(block, table);
            return;
        }
        let Some(grid) = self.grid(block, table) else {
            return;
        };
        let titled = !block.title.is_empty();
        let formal = titled && !self.forbidden("table");
        let mut attributes = role.to_vec();
        let (frame, grid_lines) = (
            block.attributes.get("frame").map(String::as_str),
            block.attributes.get("grid").map(String::as_str),
        );
        attributes.push((
            "frame",
            match frame {
                Some("ends" | "topbot") => "topbot",
                Some("sides") => "sides",
                Some("none") => "none",
                _ => "all",
            },
        ));
        let (rowsep, colsep) = match grid_lines {
            Some("rows") => ("1", "0"),
            Some("cols") => ("0", "1"),
            Some("none") => ("0", "0"),
            _ => ("1", "1"),
        };
        attributes.push(("rowsep", rowsep));
        attributes.push(("colsep", colsep));
        let name = if formal { "table" } else { "informaltable" };
        if titled && !formal {
            // A titled table where a `table` may not be (in an example, a figure or an
            // equation) keeps its title before it.
            self.title_para(&block.title);
        }
        self.xml.open(name, &attributes);
        if formal {
            self.xml.open("title", &[]);
            self.inlines(&block.title);
            self.xml.close("title");
        }
        let count = grid.columns.to_string();
        self.xml.open("tgroup", &[("cols", &count)]);
        let write = |writer: &mut Self| {
            writer.colspecs(table, grid.columns);
            for (group, rows) in [
                ("thead", &grid.head),
                ("tfoot", &grid.foot),
                ("tbody", &grid.body),
            ] {
                writer.rows(group, rows);
            }
        };
        if formal {
            self.within("table", write);
        } else {
            write(self);
        }
        self.xml.close("tgroup");
        self.xml.close(name);
    }

    fn colspecs(&mut self, table: &ViewTable, columns: usize) {
        for index in 0..columns {
            let column = table.columns.get(index);
            let name = column_name(index);
            let width = column.and_then(|column| match column.width {
                ColumnWidth::Proportional(width) | ColumnWidth::Percentage(width) if width > 0 => {
                    Some(format!("{width}*"))
                }
                _ => None,
            });
            let mut spec = vec![("colname", name.as_str())];
            if let Some(width) = &width {
                spec.push(("colwidth", width.as_str()));
            }
            if let Some(column) = column {
                spec.push(("align", align(column.align)));
            }
            self.xml.empty("colspec", &spec);
        }
    }

    /// A row group (`thead`, `tfoot` or `tbody`) of `rows`, unless there are none.
    fn rows(&mut self, group: &str, rows: &[Vec<Placed<'_>>]) {
        if rows.is_empty() {
            return;
        }
        self.xml.open(group, &[]);
        for row in rows {
            self.xml.open("row", &[]);
            for placed in row {
                self.entry(placed);
            }
            self.xml.close("row");
        }
        self.xml.close(group);
    }

    /// The attributes every cell has: its alignments and, spanning columns, its first and last.
    fn cell_attributes<'a>(
        placed: &Placed<'_>,
        start: &'a str,
        end: &'a str,
    ) -> Vec<(&'a str, &'a str)> {
        let mut attributes = vec![
            ("align", align(placed.cell.align)),
            (
                "valign",
                match placed.cell.valign {
                    VAlign::Top => "top",
                    VAlign::Middle => "middle",
                    VAlign::Bottom => "bottom",
                },
            ),
        ];
        if placed.columns > 1 {
            attributes.push(("namest", start));
            attributes.push(("nameend", end));
        }
        attributes
    }

    fn entry(&mut self, placed: &Placed<'_>) {
        let start = column_name(placed.column);
        let end = column_name(placed.column + placed.columns - 1);
        // A table that is all its cell holds is the cell's own `entrytbl`, the table CALS
        // allows in a cell; one that spans rows cannot be, having no `morerows`.
        if let [only] = placed.cell.blocks.as_slice() {
            if let ViewKind::Table(inner) = &only.kind {
                if placed.more_rows == 0 && only.title.is_empty() {
                    if let Some(grid) = self.grid(only, inner) {
                        let count = grid.columns.to_string();
                        let mut attributes = Self::cell_attributes(placed, &start, &end);
                        attributes.retain(|(name, _)| *name != "valign");
                        attributes.insert(0, ("cols", &count));
                        self.xml.open("entrytbl", &attributes);
                        self.within("entry", |writer| {
                            writer.colspecs(inner, grid.columns);
                            writer.rows("thead", &grid.head);
                            // `entrytbl` has no footer group: its footer rows end its body.
                            let mut body = grid.body;
                            body.extend(grid.foot);
                            writer.rows("tbody", &body);
                        });
                        self.xml.close("entrytbl");
                        return;
                    }
                }
            }
        }
        let more = placed.more_rows.to_string();
        let mut attributes = Self::cell_attributes(placed, &start, &end);
        if placed.more_rows > 0 {
            attributes.push(("morerows", &more));
        }
        self.xml.open("entry", &attributes);
        self.within("entry", |writer| writer.blocks(&placed.cell.blocks));
        self.xml.close("entry");
    }

    /// A table where DocBook allows none, in a table's cell beside other content: its title
    /// and each row as a paragraph of its cells' text, with a warning.
    fn table_as_text(&mut self, block: &ViewBlock, table: &ViewTable) {
        self.warn(
            "a table inside a table's cell, with more beside it, is written as its rows of text, \
             since DocBook allows a table there only on its own",
            Some(&block.span),
        );
        self.title_para(&block.title);
        let rows = table
            .header
            .iter()
            .chain(&table.rows)
            .chain(table.footer.iter());
        for row in rows {
            let cells: Vec<String> = row
                .iter()
                .map(|cell| {
                    cell.blocks
                        .iter()
                        .map(crate::blocks::block_text)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect();
            self.xml
                .element("para", &[("role", "row")], &cells.join(" | "));
        }
    }
}

/// The CALS name of column `index`, from 0: `col_1`, `col_2`, ….
fn column_name(index: usize) -> String {
    format!("col_{}", index + 1)
}

fn align(align: Align) -> &'static str {
    match align {
        Align::Start => "left",
        Align::Centre => "center",
        Align::End => "right",
    }
}

/// `rows` placed on a grid `columns` wide, each cell in the first column its row has free,
/// spans cut back to the grid; rows every cell of which a span covers are folded away.
fn place<'a>(rows: &[&'a [ViewCell]], columns: usize, clipped: &mut bool) -> Vec<Vec<Placed<'a>>> {
    let mut taken: Vec<Vec<bool>> = vec![vec![false; columns]; rows.len()];
    let mut placed: Vec<Vec<Placed<'a>>> = Vec::with_capacity(rows.len());
    for (row, cells) in rows.iter().enumerate() {
        let mut out = Vec::new();
        let mut column = 0;
        for cell in *cells {
            while column < columns && taken[row][column] {
                column += 1;
            }
            if column >= columns {
                *clipped = true;
                break;
            }
            let wanted = cell.colspan.max(1);
            let mut span = 0;
            while span < wanted && column + span < columns && !taken[row][column + span] {
                span += 1;
            }
            let wanted_rows = cell.rowspan.max(1);
            let span_rows = wanted_rows.min(rows.len() - row);
            if span < wanted || span_rows < wanted_rows {
                *clipped = true;
            }
            for covered in taken.iter_mut().skip(row).take(span_rows) {
                for slot in covered.iter_mut().skip(column).take(span) {
                    *slot = true;
                }
            }
            out.push(Placed {
                cell,
                column,
                columns: span,
                more_rows: span_rows - 1,
            });
            column += span;
        }
        placed.push(out);
    }
    // A row with no entry of its own: the cells spanning it end a row sooner.
    let empty: Vec<usize> = (0..placed.len())
        .rev()
        .filter(|&row| placed[row].is_empty())
        .collect();
    for empty in empty {
        for (row, cells) in placed.iter_mut().enumerate().take(empty) {
            for cell in cells.iter_mut() {
                if row + cell.more_rows >= empty {
                    cell.more_rows -= 1;
                }
            }
        }
        placed.remove(empty);
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(colspan: usize, rowspan: usize) -> ViewCell {
        ViewCell {
            blocks: Vec::new(),
            colspan,
            rowspan,
            align: Align::Start,
            valign: VAlign::Top,
            style: adoc_dom::CellStyle::Default,
        }
    }

    /// Each row's cells as (first column, columns, more rows).
    fn shape(placed: &[Vec<Placed<'_>>]) -> Vec<Vec<(usize, usize, usize)>> {
        placed
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| (cell.column, cell.columns, cell.more_rows))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn cells_are_placed_around_spans() {
        let rows = [vec![cell(1, 2), cell(2, 1)], vec![cell(1, 1), cell(1, 1)]];
        let rows: Vec<&[ViewCell]> = rows.iter().map(Vec::as_slice).collect();
        let mut clipped = false;
        let placed = place(&rows, 3, &mut clipped);
        assert_eq!(
            shape(&placed),
            [vec![(0, 1, 1), (1, 2, 0)], vec![(1, 1, 0), (2, 1, 0)]]
        );
        assert!(!clipped);
    }

    #[test]
    fn spans_past_the_edge_are_cut_back() {
        let rows = [vec![cell(1, 1), cell(3, 3)], vec![cell(1, 1), cell(1, 1)]];
        let rows: Vec<&[ViewCell]> = rows.iter().map(Vec::as_slice).collect();
        let mut clipped = false;
        let placed = place(&rows, 2, &mut clipped);
        assert!(clipped);
        assert_eq!(shape(&placed)[0], [(0, 1, 0), (1, 1, 1)]);
        // The second row's second cell has no room left beside the span.
        assert_eq!(shape(&placed)[1], [(0, 1, 0)]);
    }

    #[test]
    fn a_row_with_no_cells_of_its_own_is_folded_away() {
        let rows = [
            vec![cell(1, 3), cell(1, 3)],
            vec![],
            vec![],
            vec![cell(1, 1), cell(1, 1)],
        ];
        let rows: Vec<&[ViewCell]> = rows.iter().map(Vec::as_slice).collect();
        let mut clipped = false;
        let placed = place(&rows, 2, &mut clipped);
        assert_eq!(
            shape(&placed),
            [vec![(0, 1, 0), (1, 1, 0)], vec![(0, 1, 0), (1, 1, 0)]]
        );
    }
}
