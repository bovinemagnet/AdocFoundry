//! Read-only exports, READONLY 1/5 (`docs/prd/PRD-AdocFoundry_READONLY.md` §10, tests 1 to
//! 4): the policy a page's own main header sets for its exports, read from the text an export
//! renders, and located for diagnostics.

#![cfg(feature = "export")]

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_core::SourceRange;
use adoc_engine::{commands::ids, CommandArgs, Engine, ExportReadOnly, Settings};

const WAIT: Duration = Duration::from_secs(60);

/// An engine over a folder holding `files`.
fn workspace(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(directory.path());
    for (name, text) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (directory, root, engine)
}

fn policy(engine: &Engine, path: &Path) -> ExportReadOnly {
    engine
        .export_read_only(path)
        .expect("the page reads")
        .unwrap_or_else(|error| panic!("{error}"))
        .policy
}

#[test]
fn readonly_header_values() {
    let pages = [
        (
            "absent.adoc",
            "= Page\n\nText.\n",
            Some(ExportReadOnly::Unspecified),
        ),
        (
            "true.adoc",
            "= Page\n:export-read-only: true\n\nText.\n",
            Some(ExportReadOnly::Enabled),
        ),
        (
            "flag.adoc",
            "= Page\n:export-read-only:\n\nText.\n",
            Some(ExportReadOnly::Enabled),
        ),
        (
            "false.adoc",
            "= Page\n:export-read-only: false\n\nText.\n",
            Some(ExportReadOnly::Disabled),
        ),
        (
            "unset.adoc",
            "= Page\n:export-read-only!:\n\nText.\n",
            Some(ExportReadOnly::Disabled),
        ),
        (
            "unset-before.adoc",
            "= Page\n:!export-read-only:\n\nText.\n",
            Some(ExportReadOnly::Disabled),
        ),
        (
            "spaced.adoc",
            "= Page\n:export-read-only:\t true \t\n",
            Some(ExportReadOnly::Enabled),
        ),
        (
            "crlf.adoc",
            "= Page\r\n:export-read-only: true\r\n\r\nText.\r\n",
            Some(ExportReadOnly::Enabled),
        ),
        ("capital.adoc", "= Page\n:export-read-only: TRUE\n", None),
        (
            "yes.adoc",
            "= Page\n\n// no header attribute here\n",
            Some(ExportReadOnly::Unspecified),
        ),
        (
            "invalid.adoc",
            "= Page\nPaul Snow\n:export-read-only: yes\n",
            None,
        ),
    ];
    let files: Vec<(&str, &str)> = pages.iter().map(|(name, text, _)| (*name, *text)).collect();
    let (_directory, root, engine) = workspace(&files);
    for (name, _, expected) in pages {
        let result = engine.export_read_only(&root.join(name)).unwrap();
        match expected {
            Some(expected) => {
                assert_eq!(result.unwrap().policy, expected, "{name}");
            }
            None => {
                let error = result.unwrap_err();
                assert_eq!(error.source.path, root.join(name), "{name}");
                assert!(
                    error.to_string().contains("must be `true` or `false`"),
                    "{error}"
                );
            }
        }
    }
    // The error names the page and the declaration's line.
    let error = engine
        .export_read_only(&root.join("invalid.adoc"))
        .unwrap()
        .unwrap_err();
    assert_eq!(error.source.line, 3);
    assert!(
        error
            .to_string()
            .starts_with(&format!("{}:3: ", root.join("invalid.adoc").display())),
        "{error}"
    );
    // A declaration's source is kept for a policy too.
    let found = engine
        .export_read_only(&root.join("crlf.adoc"))
        .unwrap()
        .unwrap();
    let source = found.source.unwrap();
    assert_eq!(source.line, 2);
    assert_eq!(source.range, SourceRange::new(8, 31));
    assert_eq!(
        engine
            .export_read_only(&root.join("absent.adoc"))
            .unwrap()
            .unwrap()
            .source,
        None
    );
}

#[test]
fn readonly_header_duplicate_and_unset() {
    let (_directory, root, engine) = workspace(&[
        (
            "twice.adoc",
            "= Page\n:export-read-only: false\n:export-read-only: true\n",
        ),
        (
            "then-unset.adoc",
            "= Page\n:export-read-only: true\n:export-read-only!:\n",
        ),
        (
            "unset-then-set.adoc",
            "= Page\n:!export-read-only:\n:export-read-only:\n",
        ),
    ]);
    assert_eq!(
        policy(&engine, &root.join("twice.adoc")),
        ExportReadOnly::Enabled
    );
    let disabled = engine
        .export_read_only(&root.join("then-unset.adoc"))
        .unwrap()
        .unwrap();
    assert_eq!(disabled.policy, ExportReadOnly::Disabled);
    // Disabled, by the last declaration: told apart from no declaration.
    assert_eq!(disabled.source.unwrap().line, 3);
    assert_eq!(
        policy(&engine, &root.join("unset-then-set.adoc")),
        ExportReadOnly::Enabled
    );
}

#[test]
fn readonly_ignores_body_and_includes() {
    let (_directory, root, engine) = workspace(&[
        (
            "antora-playbook.yml",
            "site:\n  title: Docs\ncontent:\n  sources:\n  - url: .\nasciidoc:\n  attributes:\n    export-read-only: 'true'\n",
        ),
        (
            "docs/antora.yml",
            "name: docs\nversion: '1.0'\nasciidoc:\n  attributes:\n    export-read-only: 'true'\n",
        ),
        (
            "docs/modules/ROOT/partials/protected.adoc",
            "= Partial\n:export-read-only: true\n\n:export-read-only: true\n",
        ),
        (
            "docs/modules/ROOT/partials/editable.adoc",
            "= Partial\n:export-read-only: false\n\nText.\n",
        ),
        (
            "docs/modules/ROOT/pages/body.adoc",
            "= Body\n\nText.\n\n:export-read-only: true\n",
        ),
        (
            "docs/modules/ROOT/pages/includes.adoc",
            "= Includes\n\ninclude::partial$protected.adoc[]\n",
        ),
        (
            "docs/modules/ROOT/pages/header-include.adoc",
            "= Header include\ninclude::partial$protected.adoc[]\n\nText.\n",
        ),
        (
            "docs/modules/ROOT/pages/protected.adoc",
            "= Protected\n:export-read-only:\n\ninclude::partial$editable.adoc[]\n",
        ),
        ("docs/modules/ROOT/pages/plain.adoc", "= Plain\n\nText.\n"),
    ]);
    let page = |name: &str| root.join("docs/modules/ROOT/pages").join(name);
    // The playbook's and the component's attributes are not the page's.
    assert_eq!(
        policy(&engine, &page("plain.adoc")),
        ExportReadOnly::Unspecified
    );
    // Nor is a declaration in the body.
    assert_eq!(
        policy(&engine, &page("body.adoc")),
        ExportReadOnly::Unspecified
    );
    // Nor an included file's, in the body or in the header.
    assert_eq!(
        policy(&engine, &page("includes.adoc")),
        ExportReadOnly::Unspecified
    );
    assert_eq!(
        policy(&engine, &page("header-include.adoc")),
        ExportReadOnly::Unspecified
    );
    // A protected page stays protected whatever its partial says.
    assert_eq!(
        policy(&engine, &page("protected.adoc")),
        ExportReadOnly::Enabled
    );
}

#[test]
fn readonly_uses_unsaved_snapshot() {
    let text = "= Page\n:export-read-only: false\n\nText.\n";
    let (_directory, root, mut engine) = workspace(&[("page.adoc", text)]);
    let path = root.join("page.adoc");
    let id = engine.open(&path).unwrap();
    let at = text.find("false").unwrap();
    engine
        .invoke(
            ids::EDIT_REPLACE_RANGE,
            CommandArgs::Text {
                id,
                range: SourceRange::new(at, at + "false".len()),
                text: "true".to_owned(),
            },
        )
        .unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("false"));
    // An unsaved `true` is honoured over the `false` on disk.
    assert_eq!(policy(&engine, &path), ExportReadOnly::Enabled);
}

/// The export's own parser is the oracle for where a page's main header ends: whenever the
/// header scan finds the declaration, the DOM's header attributes hold it, and not otherwise.
#[test]
fn the_header_boundary_agrees_with_the_export_parser() {
    use adoc_dom::{AcdcParser, DocumentParser, ParseInput};

    let shapes = [
        "= Page\n:export-read-only: true\n\nText.\n",
        "= Page\n\n:export-read-only: true\n\nText.\n",
        ":export-read-only: true\n\nText.\n",
        ":a: b\n:export-read-only: true\n\n= Title\n",
        "// comment\n= Page\n:export-read-only: true\n",
        "\n\n= Page\n:export-read-only: true\n",
        "= Page\nPaul Snow <paul@example.com>\n:export-read-only: true\n",
        "= Page\nPaul Snow\nv1.0, 2026-11-17: Draft\n:export-read-only: true\n",
        "= Page\n////\nnotes\n////\n:export-read-only: true\n",
        "= Page\n// note\n:export-read-only: true\n",
        "= Page\n:description: one \\\n  two\n:export-read-only: true\n",
        "= Page\r\n:export-read-only: true\r\n\r\nText.\r\n",
        "= Page\n== Section\n:export-read-only: true\n",
        "Text.\n:export-read-only: true\n",
        "= Page\nText one\nText two\nText three\n:export-read-only: true\n",
        "= Page\n[abstract]\n:export-read-only: true\n",
    ];
    let parser = AcdcParser;
    for text in shapes {
        let path = Path::new("page.adoc");
        let dom = parser.parse(&ParseInput::new(path, text));
        let in_dom = dom.attributes.contains_key("export-read-only");
        let scanned = adoc_parser::export_read_only(text)
            .map(|header| header.policy == ExportReadOnly::Enabled)
            .unwrap_or(false);
        assert_eq!(
            scanned, in_dom,
            "{text:?}: scanned {scanned}, the parser's header {in_dom}"
        );
    }
}

/// The same oracle over every header shape up to three lines of these kinds, with the
/// declaration at every position, a byte-order mark or none. The direction that matters most:
/// whenever the export's parser reads the declaration as part of the header, the scan finds it
/// (or refuses the page), so a page that asks is never exported editable; and the scan never
/// finds one the parser does not.
#[test]
fn every_short_header_agrees_with_the_export_parser() {
    use adoc_dom::{AcdcParser, DocumentParser, ParseInput};

    const KINDS: [&str; 13] = [
        "= Page",
        ":a: b",
        "",
        "// comment",
        "Paul Snow",
        "[role]",
        "[[id]]",
        ".Title",
        "== Section",
        "# Title",
        "Page\n====",
        "Text.",
        "////\nnotes\n////",
    ];
    const DECLARATION: &str = ":export-read-only: true";
    let parser = AcdcParser;
    let mut shapes = vec![Vec::new()];
    for _ in 0..3 {
        let longer: Vec<Vec<&str>> = shapes
            .iter()
            .flat_map(|shape: &Vec<&str>| {
                KINDS.iter().map(move |kind| {
                    let mut shape = shape.clone();
                    shape.push(*kind);
                    shape
                })
            })
            .collect();
        shapes.extend(longer);
    }
    shapes.sort();
    shapes.dedup();
    let mut disagreements = Vec::new();
    for shape in &shapes {
        for at in 0..=shape.len() {
            let mut lines = shape.clone();
            lines.insert(at, DECLARATION);
            for bom in ["", "\u{feff}"] {
                let text = format!("{bom}{}\n\nBody.\n", lines.join("\n"));
                let dom = parser.parse(&ParseInput::new(Path::new("page.adoc"), &text));
                let in_header = dom.attributes.contains_key("export-read-only");
                let scanned = adoc_parser::export_read_only(&text);
                let ok = match &scanned {
                    // A page the parser reads as asking must never pass as editable.
                    Ok(header) if header.policy == ExportReadOnly::Enabled => in_header,
                    Ok(_) => !in_header,
                    Err(_) => true,
                };
                if !ok {
                    disagreements.push(format!("{text:?}: parser {in_header}, scan {scanned:?}"));
                }
            }
        }
    }
    assert!(
        disagreements.is_empty(),
        "{} of the shapes disagree, for example:\n{}",
        disagreements.len(),
        disagreements
            .iter()
            .take(25)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
