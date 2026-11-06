//! `adoc_parser::parse`, which runs on every keystroke-debounce with no guard around it:
//! it returns for any text, and every range it gives is a range of that text, on character
//! boundaries, so the editor and the language server can slice and convert it.

#![no_main]

use adoc_core::{LineIndex, SourceRange};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let parsed = adoc_parser::parse("fuzz.adoc", &text);
    let document = &parsed.document;
    assert_eq!(&*document.text, &*text);

    let lines = LineIndex::new(&text);
    let check = |what: &str, range: SourceRange| {
        assert!(
            range.start <= range.end
                && range.end <= text.len()
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end),
            "{what} {range:?} is not a range of the {} bytes of text",
            text.len()
        );
        let _ = lines.line_column(range.start);
        let _ = lines.line_column(range.end);
    };

    if let Some(title) = &document.title {
        check("title", title.range);
        check("title selection", title.selection_range);
    }
    for section in &document.sections {
        check("section", section.range);
        check("section selection", section.selection_range);
        let _ = section.implicit_ids();
    }
    for attribute in &document.attributes {
        check("attribute", attribute.range);
    }
    for anchor in &document.anchors {
        check("anchor", anchor.range);
    }
    for reference in &document.references {
        check("reference", reference.range);
    }
    for include in &document.includes {
        check("include", include.range);
    }
    for image in &document.images {
        check("image", image.range);
    }
    for diagnostic in parsed.diagnostics() {
        check("diagnostic", diagnostic.range);
    }
});
