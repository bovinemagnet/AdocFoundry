//! Bold, italic, code and highlight toggled in source text, as the source editor's shortcuts do.

use adoc_core::SourceRange;
use adoc_model::{apply_edits, toggle_mark_in_source, Mark};

/// `marked` holds `[` and `]` around the selection; the result is the text after the toggle,
/// with the selection after it marked the same way.
fn toggle(marked: &str, mark: Mark) -> String {
    let start = marked.find('[').unwrap();
    let end = marked.find(']').unwrap() - 1;
    let text = marked.replacen('[', "", 1).replacen(']', "", 1);
    let toggled = toggle_mark_in_source(&text, SourceRange::new(start, end), mark);
    let after = apply_edits(&text, &toggled.edits);
    let selection = toggled.selection;
    format!(
        "{}[{}]{}",
        &after[..selection.start],
        &after[selection.start..selection.end],
        &after[selection.end..]
    )
}

#[test]
fn a_whole_word_is_wrapped_in_the_constrained_form() {
    assert_eq!(toggle("A [bold] word.", Mark::Strong), "A *[bold]* word.");
    assert_eq!(
        toggle("A [slanted] word.", Mark::Emphasis),
        "A _[slanted]_ word."
    );
    assert_eq!(toggle("Run [make] now.", Mark::Code), "Run `[make]` now.");
    assert_eq!(
        toggle("A [key] point.", Mark::Highlight),
        "A #[key]# point."
    );
}

#[test]
fn part_of_a_word_is_wrapped_in_the_unconstrained_form() {
    assert_eq!(toggle("un[break]able", Mark::Strong), "un**[break]**able");
    assert_eq!(toggle("[Foundry]'s", Mark::Emphasis), "_[Foundry]_'s");
}

#[test]
fn spaces_at_the_edges_of_the_selection_stay_outside_the_mark() {
    assert_eq!(toggle("A[ bold ]word.", Mark::Strong), "A *[bold]* word.");
    assert_eq!(toggle("A [bold ]word.", Mark::Strong), "A *[bold]* word.");
}

#[test]
fn a_marked_selection_is_unmarked() {
    // The delimiters just outside the selection, or the selection's own.
    assert_eq!(toggle("A *[bold]* word.", Mark::Strong), "A [bold] word.");
    assert_eq!(toggle("un**[break]**able", Mark::Strong), "un[break]able");
    assert_eq!(toggle("A [*bold*] word.", Mark::Strong), "A [bold] word.");
    assert_eq!(toggle("A [`code`] word.", Mark::Code), "A [code] word.");
    // Another mark's delimiters do not count; next to `_`, a word character, only the
    // unconstrained form is bold.
    assert_eq!(
        toggle("A _[slanted]_ word.", Mark::Strong),
        "A _**[slanted]**_ word."
    );
}

#[test]
fn toggling_twice_restores_the_text() {
    for (marked, mark) in [
        ("A [bold] word.", Mark::Strong),
        ("un[break]able", Mark::Emphasis),
        ("Lines [one\ntwo] here.", Mark::Code),
    ] {
        let once = toggle(marked, mark);
        assert_eq!(toggle(&once, mark), marked, "{marked}");
    }
}

#[test]
fn each_line_of_a_selection_is_marked_on_its_own() {
    // A mark does not run across lines that might be list items or block delimiters.
    assert_eq!(
        toggle("[First line\nsecond line]\n", Mark::Strong),
        "[*First line*\n*second line*]\n"
    );
    assert_eq!(
        toggle("[*First line*\n\nsecond line]\n", Mark::Strong),
        "[*First line*\n\n*second line*]\n",
        "a line already marked is left as it is while the others are marked"
    );
    assert_eq!(
        toggle("[*First line*\n*second line*]\n", Mark::Strong),
        "[First line\nsecond line]\n"
    );
}

#[test]
fn a_caret_gets_an_empty_mark_to_type_into() {
    assert_eq!(toggle("Type [] here.", Mark::Strong), "Type *[]* here.");
    assert_eq!(toggle("in[]side", Mark::Strong), "in**[]**side");
}
