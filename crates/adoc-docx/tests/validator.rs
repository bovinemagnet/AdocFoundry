//! The package checks themselves: each kind of damage a reader would trip on, made to a valid
//! package, is caught. Without these, a check that passed everything would pass silently.

mod common;

use std::{collections::BTreeMap, fs, panic, path::Path};

use adoc_docx::{write_docx, DocxDocument, DocxOptions, DocxPage, Line, Slots};
use common::{assert_valid_package, entries, package, view};

/// A document with a picture, a footnote, a bookmark and a link to it, a list, a table, and a
/// header with fields: something of everything the checks look at.
fn valid() -> (BTreeMap<String, Vec<u8>>, tempfile::TempDir) {
    let folder = tempfile::tempdir().unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/pdf/reference/images/wide.png"),
        folder.path().join("wide.png"),
    )
    .unwrap();
    let page = folder.path().join("page.adoc");
    let text = "= Page\n\n[#start]\n== Start\n\nText.footnote:[A note.]\n\n* one\n* two\n\n|===\n|a |b\n|===\n\nimage::wide.png[Wide]\n\nSee <<start>>.\n";
    let document = DocxDocument::Article(DocxPage {
        key: "p0".to_owned(),
        view: view(page.to_str().unwrap(), text),
        depth: 0,
    });
    let mut options = DocxOptions::default();
    options.running.header = Slots {
        odd: Line {
            left: String::new(),
            centre: "{document-title}".to_owned(),
            right: "{page-number} of {page-count}".to_owned(),
        },
        even: None,
        first: None,
    };
    let mut bytes = Vec::new();
    let report = write_docx(&document, &options, &mut bytes).unwrap();
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    let entries = assert_valid_package(&bytes);
    assert!(entries.keys().any(|name| name.starts_with("word/media/")));
    assert!(entries.keys().any(|name| name.starts_with("word/header")));
    (entries, folder)
}

/// `entries` with `change` made to the part `name` (removed when it gives `None`), zipped.
fn changed(
    entries: &BTreeMap<String, Vec<u8>>,
    name: &str,
    change: impl Fn(&str) -> Option<String>,
) -> Vec<u8> {
    let mut changed: Vec<(String, Vec<u8>)> = Vec::new();
    let mut found = false;
    for (entry, data) in entries {
        if entry == name {
            found = true;
            if let Some(text) = change(std::str::from_utf8(data).unwrap_or_default()) {
                changed.push((entry.clone(), text.into_bytes()));
            }
        } else {
            changed.push((entry.clone(), data.clone()));
        }
    }
    assert!(found, "no {name}");
    package(
        changed
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_slice())),
    )
}

/// The part whose name starts with `prefix`.
fn part(entries: &BTreeMap<String, Vec<u8>>, prefix: &str) -> String {
    entries
        .keys()
        .find(|name| name.starts_with(prefix))
        .unwrap()
        .clone()
}

fn caught(bytes: &[u8]) -> String {
    let hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let result = panic::catch_unwind(|| {
        assert_valid_package(bytes);
    });
    panic::set_hook(hook);
    let error = result.expect_err("the damage was not caught");
    error
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| error.downcast_ref::<&str>().map(|text| (*text).to_owned()))
        .unwrap_or_default()
}

#[test]
fn the_package_checks_catch_damage() {
    let (entries, _folder) = valid();
    // The same entries, zipped again, are still valid: what follows is caught for its damage.
    assert_valid_package(&package(
        entries
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_slice())),
    ));
    let media = part(&entries, "word/media/");
    let header = part(&entries, "word/header");
    let replace = |from: &'static str, to: &'static str| {
        move |text: &str| {
            assert!(text.contains(from), "{from} is not in the part");
            Some(text.replacen(from, to, 1))
        }
    };
    let damage: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "a picture's part missing",
            changed(&entries, &media, |_| None),
            "not in the package",
        ),
        (
            "malformed XML",
            changed(&entries, "word/document.xml", |text| {
                Some(text[..text.len() / 2].to_owned())
            }),
            "not well formed",
        ),
        (
            "an id with no relationship",
            changed(
                &entries,
                "word/document.xml",
                replace("r:embed=\"", "r:embed=\"missing"),
            ),
            "has no relationship",
        ),
        (
            "an undefined style",
            changed(
                &entries,
                "word/document.xml",
                replace("w:pStyle w:val=\"", "w:pStyle w:val=\"Missing"),
            ),
            "used and not defined",
        ),
        (
            "a field left open",
            changed(
                &entries,
                &header,
                replace("<w:fldChar w:fldCharType=\"end\"/>", ""),
            ),
            "left open",
        ),
        (
            "a bookmark that never ends",
            changed(
                &entries,
                "word/document.xml",
                replace("<w:bookmarkEnd ", "<w:proofErr "),
            ),
            "every bookmark ends",
        ),
        (
            "a link to no bookmark",
            changed(
                &entries,
                "word/document.xml",
                replace("w:anchor=\"", "w:anchor=\"missing"),
            ),
            "no bookmark",
        ),
        (
            "a footnote not written",
            changed(
                &entries,
                "word/document.xml",
                replace(
                    "w:footnoteReference w:id=\"",
                    "w:footnoteReference w:id=\"9",
                ),
            ),
            "not written",
        ),
        (
            "a part with no content type",
            {
                let mut extra: Vec<(&str, &[u8])> = entries
                    .iter()
                    .map(|(name, data)| (name.as_str(), data.as_slice()))
                    .collect();
                extra.push(("word/extra.dat", b"<x/>"));
                package(extra)
            },
            "has no content type",
        ),
    ];
    for (what, bytes, expected) in damage {
        let message = caught(&bytes);
        assert!(message.contains(expected), "{what}: {message}");
    }

    // A changed byte of a part is caught by its CRC, and a cut package by its end record.
    let mut bytes = package(
        entries
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_slice())),
    );
    let at = bytes
        .windows(4)
        .position(|window| window == b"PK\x03\x04")
        .unwrap()
        + 40;
    bytes[at] ^= 0xff;
    assert!(!caught(&bytes).is_empty());
    let whole = package(
        entries
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_slice())),
    );
    assert!(panic::catch_unwind(|| entries_of(&whole[..whole.len() - 1])).is_err());
}

fn entries_of(bytes: &[u8]) {
    entries(bytes);
}
