//! The source editor's insert commands: a new block's skeleton at the caret, the same source
//! the visual editor writes for it ([`new_block_body`]), with a blank line before and after it
//! where needed; a code block around the selected lines; and a link around the selected text.
//! Like [`crate::toggle_mark_in_source`] this works on any text, since what is edited is
//! plainly the source.

use adoc_core::SourceRange;

use crate::{block::new_block_body, NewBlock, SourceEdit, SourceToggle, StepError};

/// Inserts `new` at the caret (the selection's head), splitting the line it is on, or for a
/// code block with lines selected, wraps those lines in it. The selection after is where
/// typing goes next: inside an empty code block, over the table's first header cell or the
/// admonition's placeholder, at an image's target.
pub fn insert_block_in_source(
    text: &str,
    selection: SourceRange,
    new: &NewBlock,
) -> Result<SourceToggle, StepError> {
    let caret = selection.end.min(text.len());
    if !text.is_char_boundary(caret) {
        return Err(StepError::OutOfRange);
    }
    let (start, end) = (
        selection.start.min(selection.end),
        selection.start.max(selection.end).min(text.len()),
    );
    if *new == NewBlock::Code && start < end {
        return Ok(wrap_lines(text, start, end));
    }
    let (body, select) = match new {
        NewBlock::Image { target, .. } if target.trim().is_empty() => {
            ("image::[]".to_owned(), 7..7)
        }
        new => new_block_body(new)?,
    };
    let before = &text[..caret];
    let after = &text[caret..];
    let leading = if before.is_empty() || before.ends_with("\n\n") {
        ""
    } else if before.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let trailing = if after.starts_with("\n\n") || (after == "\n") {
        ""
    } else if after.starts_with('\n') || after.is_empty() {
        "\n"
    } else {
        "\n\n"
    };
    let base = caret + leading.len();
    Ok(SourceToggle {
        edits: vec![SourceEdit::insert(
            caret,
            format!("{leading}{body}{trailing}"),
        )],
        selection: SourceRange::new(base + select.start, base + select.end),
    })
}

/// Wraps the selection in `link:[…]`, as its text, with the caret at the empty target.
#[must_use]
pub fn insert_link_in_source(text: &str, selection: SourceRange) -> SourceToggle {
    let start = selection.start.min(selection.end).min(text.len());
    let end = selection.start.max(selection.end).min(text.len());
    let label = text[start..end].replace(']', "\\]");
    SourceToggle {
        edits: vec![SourceEdit::new(
            SourceRange::new(start, end),
            format!("link:[{label}]"),
        )],
        selection: SourceRange::new(start + 5, start + 5),
    }
}

/// A listing block around the whole lines from `start` to `end`, with blank lines around it
/// where needed, keeping the lines selected.
fn wrap_lines(text: &str, start: usize, end: usize) -> SourceToggle {
    let first = text[..start].rfind('\n').map_or(0, |line| line + 1);
    // A selection ending at the start of a line does not take that line.
    let end = if end > first && text[..end].ends_with('\n') {
        end - 1
    } else {
        end
    };
    let last = text[end..].find('\n').map_or(text.len(), |line| end + line);
    let before = &text[..first];
    let after = &text[last..];
    let leading = if before.is_empty() || before.ends_with("\n\n") {
        "----\n"
    } else {
        "\n----\n"
    };
    let trailing = if after.starts_with("\n\n") || after == "\n" || after.is_empty() {
        "\n----"
    } else {
        "\n----\n"
    };
    let lines = last - first;
    let selected = first + leading.len();
    SourceToggle {
        edits: vec![
            SourceEdit::insert(first, leading),
            SourceEdit::insert(last, trailing),
        ],
        selection: SourceRange::new(selected, selected + lines),
    }
}

#[cfg(test)]
mod tests {
    use adoc_dom::AdmonitionKind;

    use super::*;
    use crate::apply_edits;

    fn insert(text: &str, selection: (usize, usize), new: &NewBlock) -> (String, String) {
        let done =
            insert_block_in_source(text, SourceRange::new(selection.0, selection.1), new).unwrap();
        let after = apply_edits(text, &done.edits);
        let selected = after[done.selection.start..done.selection.end].to_owned();
        (after, selected)
    }

    fn caret(text: &str) -> usize {
        text.find('|').unwrap()
    }

    #[test]
    fn a_block_goes_at_the_caret_with_blank_lines_around_it() {
        let text = "= Title\n\nBefore.\n";
        let at = text.len() - 1;
        let (after, selected) = insert(
            text,
            (at, at),
            &NewBlock::Table {
                columns: 2,
                rows: 2,
            },
        );
        assert_eq!(
            after,
            "= Title\n\nBefore.\n\n|===\n|Column 1 |Column 2\n\n| |\n| |\n|===\n"
        );
        assert_eq!(selected, "Column 1");

        // On an empty line between paragraphs, no extra blank lines.
        let text = "One.\n\n\n\nTwo.\n";
        let (after, _) = insert(text, (6, 6), &NewBlock::Code);
        assert_eq!(after, "One.\n\n----\n\n----\n\nTwo.\n");

        // In the middle of a line, the line is split.
        let text = "One two.\n";
        let (after, selected) =
            insert(text, (4, 4), &NewBlock::Admonition(AdmonitionKind::Warning));
        assert_eq!(after, "One \n\nWARNING: Warning.\n\ntwo.\n");
        assert_eq!(selected, "Warning.");

        // At the end of a file without a final newline.
        let (after, _) = insert("Last", (4, 4), &NewBlock::Code);
        assert_eq!(after, "Last\n\n----\n\n----\n");
    }

    #[test]
    fn an_image_without_a_target_leaves_the_caret_at_the_target() {
        let image = NewBlock::Image {
            target: String::new(),
            alt: String::new(),
        };
        let done = insert_block_in_source("", SourceRange::new(0, 0), &image).unwrap();
        let after = apply_edits("", &done.edits);
        assert_eq!(after, "image::[]\n");
        assert_eq!(done.selection, SourceRange::new(7, 7));
        let (after, _) = insert(
            "",
            (0, 0),
            &NewBlock::Image {
                target: "flow.png".to_owned(),
                alt: "Flow".to_owned(),
            },
        );
        assert_eq!(after, "image::flow.png[Flow]\n");
    }

    #[test]
    fn a_code_block_goes_around_the_selected_lines() {
        let text = "Intro.\n\nfn main() {\n    run();\n}\n\nOutro.\n";
        let start = text.find("fn main").unwrap() + 3;
        let end = text.find("run();").unwrap() + 2;
        let (after, selected) = insert(text, (start, end), &NewBlock::Code);
        assert_eq!(
            after,
            "Intro.\n\n----\nfn main() {\n    run();\n----\n\n}\n\nOutro.\n"
        );
        assert_eq!(selected, "fn main() {\n    run();");

        // A selection up to the next line's start does not take that line.
        let text = "a\nb\nc\n";
        let (after, selected) = insert(text, (2, 4), &NewBlock::Code);
        assert_eq!(after, "a\n\n----\nb\n----\n\nc\n");
        assert_eq!(selected, "b");
    }

    #[test]
    fn a_link_goes_around_the_selected_text() {
        let text = "See the [guide] here.";
        let start = text.find("the").unwrap();
        let end = text.find(" here").unwrap();
        let done = insert_link_in_source(text, SourceRange::new(end, start));
        let after = apply_edits(text, &done.edits);
        assert_eq!(after, "See link:[the [guide\\]] here.");
        assert_eq!(caret("See link:|"), done.selection.start);
        assert!(done.selection.is_empty());

        let done = insert_link_in_source("", SourceRange::new(0, 0));
        assert_eq!(apply_edits("", &done.edits), "link:[]");
    }
}
