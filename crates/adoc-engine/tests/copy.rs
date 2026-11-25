//! Copying rendered text through the engine (#794).

use std::{fs, path::Path};

use adoc_engine::{
    commands::ids, CommandArgs, CommandOutcome, CopyFormat, CopySource, DocumentId, EditorMode,
    Engine, EngineError, Settings, TextPlace, VisualFlow,
};

fn engine() -> Engine {
    Engine::new(Settings::default().with_git_opt_in())
}

/// A page shown visually, beside the files given.
fn visual(directory: &Path, text: &str, files: &[(&str, &str)]) -> (Engine, DocumentId) {
    for (name, text) in files {
        fs::write(directory.join(name), text).unwrap();
    }
    let page = directory.join("page.adoc");
    fs::write(&page, text).unwrap();
    let mut engine = engine();
    let id = engine.open(&page).unwrap();
    engine
        .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(id))
        .unwrap();
    assert_eq!(engine.editor_mode(id), EditorMode::Visual);
    (engine, id)
}

/// The place of `needle` in the page as the visual editor shows it: a source offset in its own
/// text, and a leaf where the text is drawn from the view tree.
fn place(engine: &Engine, id: DocumentId, needle: &str, end: bool) -> TextPlace {
    let document = engine.visual(id).unwrap();
    let flow = VisualFlow::new(&document);
    let (leaf, at) = flow
        .flow()
        .leaves()
        .iter()
        .enumerate()
        .find_map(|(index, leaf)| leaf.text.find(needle).map(|at| (index, at)))
        .unwrap_or_else(|| panic!("{needle:?} is not shown"));
    let offset = if end { at + needle.len() } else { at };
    match flow.block(leaf) {
        Some(path) => {
            let text = document.model.node(path).unwrap().text.as_ref().unwrap();
            TextPlace::Source(
                text.source_offset(offset, text.marks_at(offset))
                    .unwrap_or_else(|| panic!("{needle:?}")),
            )
        }
        None => TextPlace::Drawn {
            leaf: flow.flow().leaves()[leaf].key.clone(),
            offset,
        },
    }
}

fn source(engine: &Engine, id: DocumentId, from: &str, to: &str) -> CopySource {
    CopySource::Visual {
        id,
        version: engine.document(id).unwrap().version,
        from: place(engine, id, from, false),
        to: place(engine, id, to, true),
    }
}

fn copied(engine: &mut Engine, id: DocumentId, from: &str, to: &str, format: CopyFormat) -> String {
    let source = source(engine, id, from, to);
    let before = engine.document(id).unwrap().version;
    let copied = engine.copy_text(&source, format).unwrap();
    assert_eq!(
        engine.document(id).unwrap().version,
        before,
        "copying changes nothing"
    );
    copied.text
}

const FILES: &[(&str, &str)] = &[
    ("query.sql", "SELECT 1;\n  SELECT 2;\n"),
    ("part.adoc", "Included *text* here.\n\n- in a list\n"),
];

const PAGE: &str = "= Page\n\nBefore *bold words* end.\n\n[source,sql]\n----\ninclude::query.sql[]\n----\n\n\
                    include::part.adoc[]\n\n[source,sql]\n----\nSELECT a; -- <1>\n----\n<1> The callout.\n\nAfter it all.\n";

#[test]
fn a_selection_copies_what_it_passes_over() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, id) = visual(directory.path(), PAGE, FILES);
    assert_eq!(
        copied(&mut engine, id, "words", "After it", CopyFormat::Plain),
        "words end.\n\nSELECT 1;\n  SELECT 2;\n\nIncluded text here.\n\n- in a list\n\n\
         SELECT a; -- <1>\n\n<1> The callout.\n\nAfter it"
    );
    // Either way round.
    let CopySource::Visual {
        id,
        version,
        from,
        to,
    } = source(&engine, id, "words", "After it")
    else {
        unreachable!()
    };
    let backwards = CopySource::Visual {
        id,
        version,
        from: to,
        to: from,
    };
    let text = engine
        .copy_text(&backwards, CopyFormat::Plain)
        .unwrap()
        .text;
    assert!(
        text.starts_with("words end.") && text.ends_with("After it"),
        "{text}"
    );
}

#[test]
fn a_selection_can_lie_in_read_only_text() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, id) = visual(directory.path(), PAGE, FILES);
    let plain = CopyFormat::Plain;
    assert_eq!(
        copied(&mut engine, id, "  SELECT 2;", "  SELECT 2;", plain),
        "  SELECT 2;"
    );
    assert_eq!(copied(&mut engine, id, "text", "here", plain), "text here");
    assert_eq!(
        copied(&mut engine, id, "callout", "callout", plain),
        "callout"
    );
    assert_eq!(
        copied(&mut engine, id, "The callout.", "The callout.", plain),
        "<1> The callout."
    );
}

#[test]
fn text_drawn_as_the_parser_read_it_is_copied_as_drawn() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, id) = visual(
        directory.path(),
        "One.\n\n[subs=none]\nSome *literal stars* here.\n\nTwo.\n",
        &[],
    );
    assert_eq!(
        copied(&mut engine, id, "One.", "Two.", CopyFormat::Plain),
        "One.\n\nSome *literal stars* here.\n\nTwo."
    );
}

#[test]
fn editable_text_is_copied_as_it_is_shown() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, id) = visual(
        directory.path(),
        "Uses kbd:[Ctrl+C] and\na note.footnote:[Said.]\n\n. one\n. two\n. three\n",
        &[],
    );
    let plain = CopyFormat::Plain;
    assert_eq!(
        copied(&mut engine, id, "Uses", "note.", plain),
        "Uses kbd:[Ctrl+C] and a note."
    );
    assert_eq!(
        copied(&mut engine, id, "two", "three", plain),
        "2. two\n3. three"
    );
}

#[test]
fn asciidoc_is_the_source_and_never_an_include() {
    let directory = tempfile::tempdir().unwrap();
    let text = "= Page\n:name: World\n\nHello {name}, *bold*.\n\n\
                include::part.adoc[]\n\n[source,sql]\n----\ninclude::query.sql[]\n----\n\n\
                .A table\n[%header,cols=2*]\n|===\n|A |B\n\n|c |d\n|===\n\nLast.\n";
    let (mut engine, id) = visual(directory.path(), text, FILES);
    assert_eq!(
        copied(&mut engine, id, "Hello", "Last.", CopyFormat::AsciiDoc),
        "Hello {name}, *bold*.\n\nIncluded *text* here.\n\n- in a list\n\n\
         [source,sql]\n----\nSELECT 1;\n  SELECT 2;\n----\n\n\
         .A table\n[%header,cols=2*]\n|===\n|A |B\n\n|c |d\n|===\n\nLast."
    );
    // Marks a selection cuts are closed.
    assert_eq!(
        copied(&mut engine, id, "old", "Included", CopyFormat::AsciiDoc),
        "*old*.\n\nIncluded"
    );
}

#[test]
fn an_offset_in_shown_text_is_the_same_place_in_the_page() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("other.adoc"), "= The Other Page\n").unwrap();
    let (mut engine, id) = visual(
        directory.path(),
        "See xref:other.adoc[] then press kbd:[Ctrl+C] or\nkeep +\non *going* now.\n",
        &[],
    );
    assert_eq!(
        copied(&mut engine, id, "then", "press", CopyFormat::Markdown),
        "then press"
    );
    assert_eq!(
        copied(&mut engine, id, "keep", "oing", CopyFormat::Markdown),
        "keep\\\non **going**"
    );
}

#[test]
fn markdown_and_html_keep_structure() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, id) = visual(directory.path(), PAGE, FILES);
    let markdown = copied(
        &mut engine,
        id,
        "Page",
        "After it all.",
        CopyFormat::Markdown,
    );
    assert!(
        markdown.starts_with("# Page\n\nBefore **bold words** end."),
        "{markdown}"
    );
    assert!(
        markdown.contains("```sql\nSELECT 1;\n  SELECT 2;\n```"),
        "{markdown}"
    );
    let source = source(&engine, id, "Page", "After it all.");
    let formatted = engine.copy_text(&source, CopyFormat::Formatted).unwrap();
    let html = formatted.html.unwrap();
    assert!(html.contains("<strong>bold words</strong>"), "{html}");
    assert!(html.contains("<ul><li>in a list</li></ul>"), "{html}");
    let name = directory.path().file_name().unwrap().to_str().unwrap();
    assert!(!html.contains(name), "{html}");
    assert!(
        formatted.text.starts_with("Page\n\nBefore"),
        "{}",
        formatted.text
    );
}

#[test]
fn copying_settles_the_model_and_refuses_an_old_selection() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, id) = visual(directory.path(), "One.\n\nTwo.\n", &[]);
    let old = source(&engine, id, "One.", "Two.");
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: adoc_core::SourceRange::new(0, 0),
                text: "Zero. ".to_owned(),
            },
        )
        .unwrap();
    assert!(matches!(
        engine.copy_text(&old, CopyFormat::Plain),
        Err(EngineError::Refused(_))
    ));
    let now = CopySource::Visual {
        id,
        version: engine.document(id).unwrap().version,
        from: TextPlace::Source(0),
        to: TextPlace::Source(5),
    };
    assert_eq!(
        engine.copy_text(&now, CopyFormat::AsciiDoc).unwrap().text,
        "Zero."
    );
}

#[test]
fn the_commands_give_the_text_for_the_clipboard() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, id) = visual(directory.path(), "Some *bold*.\n", &[]);
    for (command, expected) in [
        (ids::EDIT_COPY_PLAIN, "Some bold."),
        (ids::EDIT_COPY_MARKDOWN, "Some **bold**."),
        (ids::EDIT_COPY_ASCIIDOC, "Some *bold*."),
    ] {
        let args = CommandArgs::Copy(source(&engine, id, "Some", "bold."));
        match engine.invoke(command, args).unwrap() {
            CommandOutcome::Copied(copied) => assert_eq!(copied.text, expected),
            other => panic!("{other:?}"),
        }
    }
}

/// Every text block the editor draws itself is a leaf of the page, each a different one, so no
/// text is left out of a copy or copied twice.
#[test]
fn every_text_block_is_a_leaf() {
    fn pages(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pages(&path, out);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "adoc")
            {
                out.push(path);
            }
        }
    }
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut paths = Vec::new();
    pages(&repository.join("tests/fixtures"), &mut paths);
    pages(&repository.join("src/docs/modules/ROOT"), &mut paths);
    assert!(paths.len() > 100);
    for path in paths {
        if fs::read_to_string(&path).is_err() {
            continue;
        }
        let mut engine = engine();
        let id = engine.open(&path).unwrap();
        engine
            .invoke(ids::VIEW_SHOW_VISUAL, CommandArgs::Document(id))
            .unwrap();
        let document = engine.visual(id).unwrap();
        let flow = VisualFlow::new(&document);
        let mut seen = std::collections::HashSet::new();
        for (block, node, text) in document.model.text_blocks() {
            if text.display.is_empty() || node.drawn_from_view().is_some() {
                continue;
            }
            let found =
                (0..flow.flow().leaves().len()).find(|leaf| flow.block(*leaf) == Some(&block));
            let found = found.unwrap_or_else(|| {
                panic!(
                    "{}: {:?} {:?} is no leaf",
                    path.display(),
                    node.kind,
                    text.display
                )
            });
            assert!(seen.insert(found), "{}: {:?}", path.display(), text.display);
        }
    }
}
