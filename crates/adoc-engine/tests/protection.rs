//! Read-only exports, READONLY 2/5 (`docs/prd/PRD-AdocFoundry_READONLY.md` §10, tests 5 to
//! 13): one decision per export from the pages' own headers, checked against the format before
//! anything is written. Word carries a whole-document restriction (READONLY 3/5); until
//! READONLY 4/5 gives the PDF its own, every PDF a page asks to protect is refused, and a book
//! mixing pages that ask with pages that do not is refused in both.

#![cfg(all(feature = "typst", feature = "docx"))]

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    commands::ids, CommandArgs, DocxExportArgs, Engine, EngineError, ExportArgs, ExportFormat,
    ExportProtection, ExportTarget, PdfStandard, ProtectionRefusal, Settings,
};

const WAIT: Duration = Duration::from_secs(120);

/// A component `docs` 1.0 whose pages `a`, `b` and `c` (in its navigation's order) have the
/// headers given, with `settings` as the workspace's.
fn workspace(headers: [&str; 3], settings: &str) -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(directory.path());
    let pages = root.join("docs/modules/ROOT/pages");
    fs::create_dir_all(&pages).unwrap();
    fs::write(
        root.join("docs/antora.yml"),
        "name: docs\nversion: '1.0'\nnav:\n- modules/ROOT/nav.adoc\n",
    )
    .unwrap();
    fs::write(
        root.join("docs/modules/ROOT/nav.adoc"),
        "* xref:a.adoc[]\n* xref:b.adoc[]\n* xref:c.adoc[]\n",
    )
    .unwrap();
    for (name, header) in ["a", "b", "c"].into_iter().zip(headers) {
        let title = name.to_uppercase();
        fs::write(
            pages.join(format!("{name}.adoc")),
            format!("= Page {title}\n{header}\nThe text of page {title}.\n"),
        )
        .unwrap();
    }
    fs::write(root.join(".adocfoundry.toml"), settings).unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (directory, root, engine)
}

const ON: &str = ":export-read-only: true\n";
const OFF: &str = ":export-read-only: false\n";
const NONE: &str = "";

fn page(root: &Path, name: &str) -> PathBuf {
    root.join(format!("docs/modules/ROOT/pages/{name}.adoc"))
}

fn book() -> ExportTarget {
    ExportTarget::Component {
        name: "docs".to_owned(),
        version: Some("1.0".to_owned()),
    }
}

fn docx(target: ExportTarget, output: &Path) -> DocxExportArgs {
    DocxExportArgs {
        target,
        output: output.to_path_buf(),
        attributes: Vec::new(),
        paper: None,
        landscape: None,
        look: None,
    }
}

fn pdf(target: ExportTarget, output: &Path) -> ExportArgs {
    ExportArgs {
        target,
        output: output.to_path_buf(),
        theme: None,
        standards: Vec::new(),
        fonts_dir: None,
        typst: None,
        attributes: Vec::new(),
    }
}

/// The refusal an export ended with.
fn refusal(result: Result<adoc_engine::ExportSummary, EngineError>) -> ProtectionRefusal {
    match result {
        Err(EngineError::ExportProtection(refusal)) => *refusal,
        other => panic!("not refused for protection: {other:?}"),
    }
}

#[test]
fn protection_article_enabled_disabled() {
    let (_directory, root, mut engine) = workspace([ON, OFF, NONE], "");
    let article = |engine: &mut Engine, name: &str| {
        ExportTarget::Page(engine.open(&page(&root, name)).unwrap())
    };
    // Disabled and unspecified pages export as they always did.
    for name in ["b", "c"] {
        let target = article(&mut engine, name);
        for format in [ExportFormat::Docx, ExportFormat::Pdf, ExportFormat::Html] {
            assert_eq!(
                engine.export_protection(&target, format).unwrap(),
                ExportProtection::Off,
                "{name} {format:?}"
            );
        }
    }
    // An enabled page asks for the whole document, named at its line: Word gives it, PDF and
    // HTML refuse.
    let target = article(&mut engine, "a");
    match engine
        .export_protection(&target, ExportFormat::Docx)
        .unwrap()
    {
        ExportProtection::WholeDocument { requested_by } => {
            assert_eq!(requested_by.len(), 1);
            assert_eq!(requested_by[0].path, page(&root, "a"));
            assert_eq!(requested_by[0].line, 2);
        }
        other => panic!("{other:?}"),
    }
    for format in [ExportFormat::Pdf, ExportFormat::Html] {
        match engine.export_protection(&target, format) {
            Err(EngineError::ExportProtection(refusal)) => match *refusal {
                ProtectionRefusal::UnsupportedFormat { requested_by, .. } => {
                    assert_eq!(requested_by.len(), 1);
                    assert_eq!(requested_by[0].path, page(&root, "a"));
                    assert_eq!(requested_by[0].line, 2);
                }
                other => panic!("{format:?}: {other:?}"),
            },
            other => panic!("{format:?}: {other:?}"),
        }
    }
    // An invalid value names the page and line, whatever the format.
    // A page never opened, so its file is what an export reads.
    fs::write(
        page(&root, "d"),
        "= Page D\n:export-read-only: yes\n\nText.\n",
    )
    .unwrap();
    let target = ExportTarget::File(page(&root, "d"));
    match engine.export_protection(&target, ExportFormat::Docx) {
        Err(EngineError::ExportProtection(refusal)) => {
            assert!(matches!(*refusal, ProtectionRefusal::InvalidHeader(_)));
            assert!(refusal.to_string().contains("d.adoc:2: "), "{refusal}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn protection_book_all_enabled() {
    // Every page asks: one request for the whole book, naming each page, which Word carries
    // and PDF refuses.
    let (directory, root, mut engine) = workspace([ON, ON, ON], "");
    let output = directory.path().join("book.docx");
    let summary = engine
        .export_docx_blocking(docx(book(), &output), WAIT)
        .unwrap();
    assert!(summary.protected);
    let entries = adoc_docx::testing::assert_valid_package(&fs::read(&output).unwrap());
    assert!(
        adoc_docx::testing::text(&entries, "word/settings.xml").contains("w:documentProtection")
    );
    let output = directory.path().join("book.pdf");
    match refusal(engine.export_blocking(pdf(book(), &output), WAIT)) {
        ProtectionRefusal::UnsupportedFormat { requested_by, .. } => {
            let pages: Vec<&Path> = requested_by
                .iter()
                .map(|source| source.path.as_path())
                .collect();
            assert_eq!(
                pages,
                [page(&root, "a"), page(&root, "b"), page(&root, "c")]
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(!output.exists());
    // One page not asking makes it a mixed book, not a whole-document request.
    let (_directory, _root, engine) = workspace([ON, ON, NONE], "");
    assert!(matches!(
        engine.export_protection(&book(), ExportFormat::Docx),
        Err(EngineError::ExportProtection(refusal))
            if matches!(*refusal, ProtectionRefusal::MixedDocument { .. })
    ));
    // No page asking: as before.
    let (_directory, _root, engine) = workspace([NONE, OFF, NONE], "");
    assert_eq!(
        engine
            .export_protection(&book(), ExportFormat::Docx)
            .unwrap(),
        ExportProtection::Off
    );
}

#[test]
fn protection_pdf_book_mixed_fails() {
    let (directory, root, mut engine) = workspace([ON, NONE, OFF], "");
    let output = directory.path().join("book.pdf");
    let refused = refusal(engine.export_blocking(pdf(book(), &output), WAIT));
    match &refused {
        ProtectionRefusal::MixedDocument {
            format,
            requested_by,
            editable,
        } => {
            assert_eq!(*format, ExportFormat::Pdf);
            assert_eq!(requested_by[0].path, page(&root, "a"));
            assert_eq!(editable, &[page(&root, "b"), page(&root, "c")]);
        }
        other => panic!("{other:?}"),
    }
    let message = refused.to_string();
    assert!(
        message.contains("a.adoc:2") && message.contains("b.adoc") && message.contains("c.adoc"),
        "{message}"
    );
    assert!(!output.exists());
}

#[test]
fn protection_docx_mixed_fails_until_supported() {
    let (directory, root, mut engine) = workspace([NONE, ON, NONE], "");
    let output = directory.path().join("book.docx");
    match refusal(engine.export_docx_blocking(docx(book(), &output), WAIT)) {
        ProtectionRefusal::MixedDocument {
            format,
            requested_by,
            editable,
        } => {
            assert_eq!(format, ExportFormat::Docx);
            assert_eq!(requested_by[0].path, page(&root, "b"));
            assert_eq!(editable, [page(&root, "a"), page(&root, "c")]);
        }
        other => panic!("{other:?}"),
    }
    assert!(!output.exists());
}

#[test]
fn protection_unsupported_format_fails() {
    let (directory, root, mut engine) = workspace([ON, NONE, NONE], "");
    let output = directory.path().join("a.html");
    fs::write(&output, "kept").unwrap();
    let id = engine.open(&page(&root, "a")).unwrap();
    let refused = engine.invoke(
        ids::EXPORT_HTML_PAGE,
        CommandArgs::Export(pdf(ExportTarget::Page(id), &output)),
    );
    match refused {
        Err(EngineError::ExportProtection(refusal)) => {
            assert!(matches!(
                *refusal,
                ProtectionRefusal::UnsupportedFormat {
                    format: ExportFormat::Html,
                    ..
                }
            ));
            assert!(refusal
                .to_string()
                .contains("HTML has no editing restriction"));
        }
        other => panic!("{other:?}"),
    }
    assert!(engine.wait_for_workers(WAIT));
    assert_eq!(fs::read_to_string(&output).unwrap(), "kept");
    // An editable page still exports to HTML.
    let id = engine.open(&page(&root, "b")).unwrap();
    engine
        .invoke(
            ids::EXPORT_HTML_PAGE,
            CommandArgs::Export(pdf(
                ExportTarget::Page(id),
                &directory.path().join("b.html"),
            )),
        )
        .unwrap();
}

#[test]
fn protection_profile_attributes_cannot_override() {
    let settings = "[docx.profiles.locking]\npage = \"docs/modules/ROOT/pages/b.adoc\"\noutput = \"b.docx\"\n\
                    [docx.profiles.locking.attributes]\nexport-read-only = \"true\"\n\n\
                    [docx.profiles.unlocking]\npage = \"docs/modules/ROOT/pages/a.adoc\"\noutput = \"a.docx\"\n\
                    [docx.profiles.unlocking.attributes]\nexport-read-only = \"false\"\n\n\
                    [pdf.profiles.unlocking]\npage = \"docs/modules/ROOT/pages/a.adoc\"\noutput = \"a.pdf\"\n\
                    [pdf.profiles.unlocking.attributes]\nexport-read-only = \"false\"\n";
    let (_directory, root, mut engine) = workspace([ON, NONE, NONE], settings);
    // A profile's attribute does not lock an editable page: it exports unprotected.
    let args = engine.docx_profile_export_args("locking").unwrap();
    let summary = engine.export_docx_blocking(args, WAIT).unwrap();
    assert!(!summary.protected);
    assert!(root.join("b.docx").is_file());
    // Nor unlock a protected one: the Word document is read-only, the PDF refused.
    let args = engine.docx_profile_export_args("unlocking").unwrap();
    assert!(engine.export_docx_blocking(args, WAIT).unwrap().protected);
    let args = engine.profile_export_args("unlocking").unwrap();
    assert!(matches!(
        refusal(engine.export_blocking(args, WAIT)),
        ProtectionRefusal::UnsupportedFormat { .. }
    ));
    // Nor the export's own attributes.
    let mut args = pdf(ExportTarget::File(page(&root, "a")), &root.join("own.pdf"));
    args.attributes = vec![("export-read-only".to_owned(), "false".to_owned())];
    assert!(matches!(
        refusal(engine.export_blocking(args, WAIT)),
        ProtectionRefusal::UnsupportedFormat { .. }
    ));
}

#[test]
fn protection_pdfa_is_rejected() {
    let (directory, root, mut engine) = workspace([ON, ON, ON], "");
    for standard in [PdfStandard::A1b, PdfStandard::A1a, PdfStandard::A2a] {
        let mut args = pdf(
            ExportTarget::File(page(&root, "a")),
            &directory.path().join("a.pdf"),
        );
        args.standards = vec![standard];
        match refusal(engine.export_blocking(args, WAIT)) {
            ProtectionRefusal::IncompatibleStandard {
                standard: name,
                requested_by,
            } => {
                assert!(name.starts_with("PDF/A-"), "{name}");
                assert_eq!(requested_by[0].path, page(&root, "a"));
            }
            other => panic!("{standard:?}: {other:?}"),
        }
    }
    // From the settings and from a profile too, a book included.
    let settings = "[pdf]\nstandards = [\"pdf-a-2a\"]\n\n[pdf.profiles.archive]\ncomponent = \"1.0@docs\"\noutput = \"book.pdf\"\nstandards = [\"pdf-a-1b\"]\n";
    let (_directory, root, mut engine) = workspace([ON, ON, ON], settings);
    let args = pdf(ExportTarget::File(page(&root, "a")), &root.join("a.pdf"));
    assert!(matches!(
        refusal(engine.export_blocking(args, WAIT)),
        ProtectionRefusal::IncompatibleStandard { .. }
    ));
    let args = engine.profile_export_args("archive").unwrap();
    assert!(matches!(
        refusal(engine.export_blocking(args, WAIT)),
        ProtectionRefusal::IncompatibleStandard { .. }
    ));
    assert!(!root.join("a.pdf").exists() && !root.join("book.pdf").exists());
}

#[test]
fn protection_failed_export_preserves_existing_file() {
    let (directory, root, mut engine) = workspace([ON, NONE, NONE], "");
    let docx_output = directory.path().join("book.docx");
    let pdf_output = directory.path().join("a.pdf");
    fs::write(&docx_output, b"an earlier document").unwrap();
    fs::write(&pdf_output, b"an earlier PDF").unwrap();
    // A mixed Word book, and a read-only PDF, are refused.
    let _ = refusal(engine.export_docx_blocking(docx(book(), &docx_output), WAIT));
    let target = ExportTarget::File(page(&root, "a"));
    let _ = refusal(engine.export_blocking(pdf(target, &pdf_output), WAIT));
    // Through the command, as the app exports.
    assert!(engine
        .invoke(
            ids::EXPORT_DOCX_COMPONENT,
            CommandArgs::ExportDocx(docx(book(), &docx_output)),
        )
        .is_err());
    assert!(engine.wait_for_workers(WAIT));
    assert_eq!(fs::read(&docx_output).unwrap(), b"an earlier document");
    assert_eq!(fs::read(&pdf_output).unwrap(), b"an earlier PDF");
    // A batch of pages lists the protected one as failed, with why, and writes the others.
    let folder = directory.path().join("pages");
    fs::create_dir_all(&folder).unwrap();
    let events = engine.subscribe();
    engine
        .invoke(
            ids::EXPORT_PDF_PAGES,
            CommandArgs::ExportPages {
                pages: vec![page(&root, "a"), page(&root, "b")],
                folder: folder.clone(),
            },
        )
        .unwrap();
    assert!(engine.wait_for_workers(WAIT));
    let (written, failed) = events
        .try_iter()
        .find_map(|event| match event {
            adoc_engine::EngineEvent::PagesExported {
                written, failed, ..
            } => Some((written, failed)),
            _ => None,
        })
        .expect("the batch finished");
    assert_eq!(written.len(), 1, "{written:?}");
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, page(&root, "a"));
    assert!(failed[0].1.contains("read-only"), "{failed:?}");
    assert!(!folder.join("a.pdf").exists());
}

#[test]
fn unprotected_export_unchanged() {
    // The same page with the attribute absent, false and unset exports the same bytes.
    let (directory, root, mut engine) = workspace([NONE, OFF, ":export-read-only!:\n"], "");
    let text = |name: &str| fs::read_to_string(page(&root, name)).unwrap();
    // Give the three pages the same title and text, so only their headers differ.
    for (name, header) in [("a", NONE), ("b", OFF), ("c", ":export-read-only!:\n")] {
        fs::write(page(&root, name), format!("= Page\n{header}\nThe text.\n")).unwrap();
        assert!(text(name).contains("The text."));
    }
    let mut written = Vec::new();
    for name in ["a", "b", "c"] {
        let target = ExportTarget::File(page(&root, name));
        let word = directory.path().join(format!("{name}.docx"));
        let summary = engine
            .export_docx_blocking(docx(target.clone(), &word), WAIT)
            .unwrap();
        assert!(!summary.protected);
        let portable = directory.path().join(format!("{name}.pdf"));
        let summary = engine
            .export_blocking(pdf(target, &portable), WAIT)
            .unwrap();
        assert!(!summary.protected);
        written.push((fs::read(word).unwrap(), fs::read(portable).unwrap()));
    }
    assert!(
        written[0].0 == written[1].0 && written[1].0 == written[2].0,
        "Word bytes differ"
    );
    assert!(
        written[0].1 == written[1].1 && written[1].1 == written[2].1,
        "PDF bytes differ"
    );
}
