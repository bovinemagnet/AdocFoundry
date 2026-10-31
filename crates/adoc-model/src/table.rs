//! Tables and images: rows and columns of regular tables, and block image macros.

use adoc_core::SourceRange;

use crate::{
    step::{StepError, StepResult},
    tree::{Model, Node, NodeKind, RowSection},
    SourceEdit,
};

fn unsupported(why: &'static str) -> StepError {
    StepError::Unsupported(why)
}

/// Where a cell sits: its table, row and column.
struct CellAt<'m> {
    table: &'m Node,
    row: usize,
    column: usize,
}

/// A table's `cols` value: a number of equal columns, or one spec per column.
#[derive(Debug, PartialEq)]
enum Columns {
    Count(usize),
    Specs(Vec<String>),
}

impl Columns {
    /// As Asciidoctor reads it: a whole value that is a number is that many columns of equal
    /// width, `cols=3` three of them; anything else is a spec per column.
    fn read(value: &str) -> Self {
        match value.trim().parse::<usize>() {
            Ok(count) => Self::Count(count),
            Err(_) => Self::Specs(
                value
                    .split([',', ';'])
                    .map(|spec| spec.trim().to_owned())
                    .collect(),
            ),
        }
    }

    /// The value with a column of width 1 after `column`.
    fn inserted_after(self, column: usize) -> String {
        match self {
            Self::Count(count) => (count + 1).to_string(),
            Self::Specs(mut specs) => {
                specs.insert((column + 1).min(specs.len()), "1".to_owned());
                specs.join(",")
            }
        }
    }

    /// The value without `column`. A single width is written `1`, since a lone number would be
    /// a count of columns.
    fn deleted(self, column: usize) -> String {
        match self {
            Self::Count(count) => count.saturating_sub(1).max(1).to_string(),
            Self::Specs(mut specs) => {
                if column < specs.len() {
                    specs.remove(column);
                }
                if specs.len() <= 1 {
                    "1".to_owned()
                } else {
                    specs.join(",")
                }
            }
        }
    }
}

impl Model {
    /// The regular table cell holding `at`.
    fn cell_at(&self, at: usize) -> Result<CellAt<'_>, StepError> {
        let (path, node, _) = self.text_block_at(at).ok_or(StepError::NotEditable)?;
        if node.kind != NodeKind::Cell || path.len() < 3 {
            return Err(unsupported("put the caret in a table cell"));
        }
        let table = self
            .node(&path[..path.len() - 2])
            .ok_or(StepError::NotEditable)?;
        let NodeKind::Table { regular, .. } = table.kind else {
            return Err(StepError::NotEditable);
        };
        if !regular || !table.is_editable() {
            return Err(unsupported(
                "this table's rows and columns are edited in the source",
            ));
        }
        Ok(CellAt {
            table,
            row: path[path.len() - 2],
            column: path[path.len() - 1],
        })
    }

    fn rows(table: &Node) -> impl Iterator<Item = &Node> {
        table
            .children
            .iter()
            .filter(|row| matches!(row.kind, NodeKind::Row { .. }))
    }

    fn line_start_of(&self, offset: usize) -> usize {
        self.source[..offset]
            .rfind('\n')
            .map_or(0, |index| index + 1)
    }

    fn rebuilt(
        &self,
        mut edits: Vec<SourceEdit>,
        selection: SourceRange,
    ) -> Result<StepResult, StepError> {
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        Ok(StepResult {
            edits,
            selection,
            model: None,
        })
    }

    /// An empty row for a table of `columns` columns: `| |` for two.
    pub(crate) fn empty_row(columns: usize) -> String {
        let mut row = "|".to_owned();
        for _ in 1..columns {
            row.push_str(" |");
        }
        row
    }

    /// Adds an empty row below the caret's (below the header, before the first body row).
    pub(crate) fn insert_table_row(&self, at: usize) -> Result<StepResult, StepError> {
        let cell = self.cell_at(at)?;
        let rows: Vec<&Node> = Self::rows(cell.table).collect();
        let row = rows.get(cell.row).ok_or(StepError::NotEditable)?;
        let columns = row.children.len();
        let new = Self::empty_row(columns);
        let header = matches!(
            row.kind,
            NodeKind::Row {
                section: RowSection::Header
            }
        );
        let (edit, first_cell) = match rows.get(cell.row + 1).filter(|_| header) {
            Some(body) => {
                let start = self.line_start_of(body.range.start);
                (SourceEdit::insert(start, format!("{new}\n")), start + 1)
            }
            None if header => (
                SourceEdit::insert(row.range.end, format!("\n\n{new}")),
                row.range.end + 3,
            ),
            None => (
                SourceEdit::insert(row.range.end, format!("\n{new}")),
                row.range.end + 2,
            ),
        };
        self.rebuilt(vec![edit], SourceRange::new(first_cell, first_cell))
    }

    /// Deletes the caret's row, which must not be the header.
    pub(crate) fn delete_table_row(&self, at: usize) -> Result<StepResult, StepError> {
        let cell = self.cell_at(at)?;
        let rows: Vec<&Node> = Self::rows(cell.table).collect();
        let row = rows.get(cell.row).ok_or(StepError::NotEditable)?;
        if matches!(
            row.kind,
            NodeKind::Row {
                section: RowSection::Header
            }
        ) {
            return Err(unsupported("the header row is deleted in the source"));
        }
        let start = self.line_start_of(row.range.start);
        let end = self.source[row.range.end..]
            .find('\n')
            .map_or(self.source.len(), |index| row.range.end + index + 1);
        let caret = start;
        self.rebuilt(
            vec![SourceEdit::delete(SourceRange::new(start, end))],
            SourceRange::new(caret, caret),
        )
    }

    /// The `cols` attribute of a table: the range of its value and the column specs.
    fn column_specs(&self, table: &Node) -> Result<Option<(SourceRange, Columns)>, StepError> {
        let head_end = self.source[table.range.start..table.range.end]
            .find("|===")
            .map_or(table.range.start, |index| table.range.start + index);
        let head = &self.source[table.range.start..head_end];
        let Some(found) = head.find("cols=") else {
            return Ok(None);
        };
        let rest = &head[found + 5..];
        let (start, end) = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                let close = rest[1..].find(quote).ok_or(StepError::NotEditable)?;
                (found + 6, found + 6 + close)
            }
            _ => {
                let close = rest.find([',', ']']).unwrap_or(rest.len());
                (found + 5, found + 5 + close)
            }
        };
        let value = &head[start..end];
        if value.contains('*') {
            return Err(unsupported(
                "the column widths repeat with `*`; edit them in the source",
            ));
        }
        let specs = Columns::read(value);
        Ok(Some((
            SourceRange::new(table.range.start + start, table.range.start + end),
            specs,
        )))
    }

    /// Adds an empty column after the caret's.
    pub(crate) fn insert_table_column(&self, at: usize) -> Result<StepResult, StepError> {
        let cell = self.cell_at(at)?;
        let mut edits = Vec::new();
        let mut caret = at;
        for (index, row) in Self::rows(cell.table).enumerate() {
            let target = row
                .children
                .get(cell.column)
                .ok_or(StepError::NotEditable)?;
            edits.push(SourceEdit::insert(target.range.end, " |"));
            if index == cell.row {
                caret = target.range.end + 2;
            }
        }
        if let Some((range, columns)) = self.column_specs(cell.table)? {
            edits.push(SourceEdit::new(range, columns.inserted_after(cell.column)));
        }
        let selection = crate::Mapping::new(&{
            let mut sorted = edits.clone();
            sorted.sort_by_key(|edit| edit.range.start);
            sorted
        })
        .map(caret, crate::Assoc::Before);
        self.rebuilt(edits, SourceRange::new(selection, selection))
    }

    /// Deletes the caret's column; a table keeps at least one.
    pub(crate) fn delete_table_column(&self, at: usize) -> Result<StepResult, StepError> {
        let cell = self.cell_at(at)?;
        let mut edits = Vec::new();
        for row in Self::rows(cell.table) {
            if row.children.len() < 2 {
                return Err(unsupported("a table keeps at least one column"));
            }
            let target = row
                .children
                .get(cell.column)
                .ok_or(StepError::NotEditable)?;
            let range = match row.children.get(cell.column + 1) {
                Some(next) => SourceRange::new(target.range.start, next.range.start),
                None => {
                    let previous = &row.children[cell.column - 1];
                    let end = previous
                        .text
                        .as_ref()
                        .map_or(previous.range.end, |text| text.range.end);
                    SourceRange::new(end, target.range.end)
                }
            };
            edits.push(SourceEdit::delete(range));
        }
        if let Some((range, columns)) = self.column_specs(cell.table)? {
            edits.push(SourceEdit::new(range, columns.deleted(cell.column)));
        }
        let start = self.line_start_of(cell.table.range.start);
        self.rebuilt(edits, SourceRange::new(start, start))
    }

    /// The innermost image block whose range holds `at`.
    fn image_at(&self, at: usize) -> Option<&Node> {
        self.walk()
            .into_iter()
            .filter(|(_, node)| {
                matches!(node.kind, NodeKind::Image { .. })
                    && node.range.start <= at
                    && at <= node.range.end
            })
            .map(|(_, node)| node)
            .next_back()
    }

    /// Changes a block image's target and alt text, leaving its other attributes as written.
    pub(crate) fn set_image(
        &self,
        at: usize,
        target: &str,
        alt: &str,
    ) -> Result<StepResult, StepError> {
        let target = target.trim();
        if target.is_empty() || target.contains(char::is_whitespace) {
            return Err(unsupported("an image target has no spaces"));
        }
        let image = self
            .image_at(at)
            .ok_or(unsupported("there is no image here"))?;
        if !image.is_editable() {
            return Err(StepError::NotEditable);
        }
        let text = &self.source[image.range.start..image.range.end];
        let macro_start = text.find("image::").ok_or(StepError::NotEditable)?;
        let line_end = text[macro_start..]
            .find('\n')
            .map_or(text.len(), |index| macro_start + index);
        let line = &text[macro_start..line_end];
        let open = line.find('[').ok_or(StepError::NotEditable)?;
        let close = line.rfind(']').ok_or(StepError::NotEditable)?;
        let base = image.range.start + macro_start;
        let mut edits = Vec::new();
        let old_target = &line["image::".len()..open];
        if old_target != target {
            edits.push(SourceEdit::new(
                SourceRange::new(base + "image::".len(), base + open),
                target,
            ));
        }
        let attributes = &line[open + 1..close];
        let alt = alt.replace(']', "\\]");
        let quoted = if alt.contains(',') {
            format!("\"{}\"", alt.replace('"', "\\\""))
        } else {
            alt.clone()
        };
        let parts = split_attributes(attributes);
        let attributes_start = base + open + 1;
        let named = parts
            .iter()
            .find(|(part, _)| part.trim_start().starts_with("alt="));
        match (named, parts.first()) {
            (Some((part, offset)), _) => {
                let value_start = part.find('=').map_or(0, |index| index + 1);
                let value = &part[value_start..];
                let (start, end, replacement) =
                    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
                        (value_start + 1, part.len() - 1, alt.replace('"', "\\\""))
                    } else {
                        (value_start, part.len(), quoted.clone())
                    };
                edits.push(SourceEdit::new(
                    SourceRange::new(
                        attributes_start + offset + start,
                        attributes_start + offset + end,
                    ),
                    replacement,
                ));
            }
            (None, Some((first, offset))) if !first.contains('=') => {
                if first.trim() != quoted {
                    edits.push(SourceEdit::new(
                        SourceRange::new(
                            attributes_start + offset,
                            attributes_start + offset + first.len(),
                        ),
                        quoted,
                    ));
                }
            }
            (None, None) => {
                if !alt.is_empty() {
                    edits.push(SourceEdit::insert(attributes_start, quoted));
                }
            }
            (None, Some(_)) => {
                edits.push(SourceEdit::insert(attributes_start, format!("{quoted},")))
            }
        }
        let caret = image.range.start;
        self.rebuilt(edits, SourceRange::new(caret, caret))
    }
}

/// An attribute list split at commas outside quotes, each part with its offset.
fn split_attributes(attributes: &str) -> Vec<(&str, usize)> {
    if attributes.is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (index, character) in attributes.char_indices() {
        match character {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                parts.push((&attributes[start..index], start));
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push((&attributes[start..], start));
    parts
}

#[cfg(test)]
mod tests {
    use super::Columns;

    #[test]
    fn insert_column_on_integer_cols() {
        assert_eq!(Columns::read("3"), Columns::Count(3));
        assert_eq!(Columns::read(" 3 ").inserted_after(1), "4");
    }

    #[test]
    fn delete_column_on_integer_cols() {
        assert_eq!(Columns::read("3").deleted(1), "2");
        assert_eq!(Columns::read("1").deleted(0), "1");
    }

    #[test]
    fn delete_to_one_column_writes_one() {
        assert_eq!(Columns::read("2,3").deleted(0), "1");
        assert_eq!(Columns::read("1,2,3").deleted(1), "1,3");
        assert_eq!(Columns::read("1,2").inserted_after(0), "1,1,2");
        assert_eq!(Columns::read("<1;>2").deleted(1), "1");
    }
}
