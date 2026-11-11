//! Following a link never hands the system a program to run (#495): only a PDF inside the
//! workspace goes to the system's viewer; images and text open in AdocFoundry, and anything
//! else is refused.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    commands::ids, CommandArgs, CommandOutcome, DocumentId, Engine, EngineError, Settings,
};

fn open_workspace(root: &Path) -> Engine {
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    engine.enable_preview_tabs();
    engine.open_workspace(vec![root.to_path_buf()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(10)));
    engine
}

/// An Antora component whose page links to attachments of every kind.
fn component(page: &str, attachments: &[(&str, &[u8])]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("antora.yml"), "name: docs\nversion: ~\n").unwrap();
    let pages = root.join("modules/ROOT/pages");
    let files = root.join("modules/ROOT/attachments");
    fs::create_dir_all(&pages).unwrap();
    fs::create_dir_all(&files).unwrap();
    fs::write(pages.join("index.adoc"), page).unwrap();
    for (name, contents) in attachments {
        fs::write(files.join(name), contents).unwrap();
    }
    directory
}

fn page(engine: &mut Engine, root: &Path) -> DocumentId {
    match engine
        .invoke(
            ids::DOCUMENT_OPEN,
            CommandArgs::Path(root.join("modules/ROOT/pages/index.adoc")),
        )
        .unwrap()
    {
        CommandOutcome::Opened(id) => id,
        other => panic!("opening the page gave {other:?}"),
    }
}

fn follow(
    engine: &mut Engine,
    id: DocumentId,
    needle: &str,
) -> Result<CommandOutcome, EngineError> {
    let offset = engine
        .document(id)
        .unwrap()
        .text_string()
        .find(needle)
        .unwrap()
        + 2;
    engine.invoke(
        ids::NAVIGATE_FOLLOW_LINK,
        CommandArgs::Offset { id, offset },
    )
}

fn real(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

const PAGE: &str = "= Downloads\n\n\
    xref:attachment$guide.pdf[The guide]\n\n\
    xref:attachment$run.bat[The script]\n\n\
    xref:attachment$setup.exe[The installer]\n";

#[test]
fn only_a_pdf_in_the_workspace_is_handed_to_the_system() {
    let directory = component(
        PAGE,
        &[
            ("guide.pdf", b"%PDF-1.7\n"),
            ("run.bat", b"echo hello\r\n"),
            ("setup.exe", b"MZ\x90\x00\x03\x00\x00\x00\xff\xff"),
        ],
    );
    let root = directory.path();
    let mut engine = open_workspace(root);
    let id = page(&mut engine, root);

    let guide = follow(&mut engine, id, "guide.pdf").unwrap();
    assert_eq!(
        guide,
        CommandOutcome::External(real(&root.join("modules/ROOT/attachments/guide.pdf")))
    );
}

#[test]
fn a_script_a_link_names_opens_as_text_and_never_runs() {
    let directory = component(PAGE, &[("run.bat", b"echo hello\r\n")]);
    let root = directory.path();
    let mut engine = open_workspace(root);
    let id = page(&mut engine, root);

    let outcome = follow(&mut engine, id, "run.bat").unwrap();
    let CommandOutcome::Opened(script) = outcome else {
        panic!("following the script gave {outcome:?}");
    };
    assert!(engine
        .document(script)
        .unwrap()
        .path
        .ends_with("attachments/run.bat"));
    assert_eq!(engine.active(), Some(script));
    assert!(engine.can_go_back());
}

#[test]
fn a_program_a_link_names_is_refused() {
    let directory = component(
        PAGE,
        &[("setup.exe", b"MZ\x90\x00\x03\x00\x00\x00\xff\xff")],
    );
    let root = directory.path();
    let mut engine = open_workspace(root);
    let id = page(&mut engine, root);

    let refused = follow(&mut engine, id, "setup.exe");
    assert!(
        matches!(&refused, Err(EngineError::Refused(message)) if message.contains("setup.exe")),
        "following the installer gave {refused:?}"
    );
    assert_eq!(engine.active(), Some(id));
}

#[test]
fn a_pdf_outside_the_workspace_is_not_handed_to_the_system() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("docs");
    fs::create_dir_all(&root).unwrap();
    fs::write(directory.path().join("outside.pdf"), b"%PDF-1.7\n").unwrap();
    fs::write(
        root.join("page.adoc"),
        "= Page\n\nimage::../outside.pdf[Outside]\n",
    )
    .unwrap();
    let mut engine = open_workspace(&root);
    let id = match engine
        .invoke(
            ids::DOCUMENT_OPEN,
            CommandArgs::Path(root.join("page.adoc")),
        )
        .unwrap()
    {
        CommandOutcome::Opened(id) => id,
        other => panic!("opening the page gave {other:?}"),
    };

    let outcome = follow(&mut engine, id, "outside.pdf");
    assert!(
        !matches!(outcome, Ok(CommandOutcome::External(_))),
        "following the PDF outside the workspace gave {outcome:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_pdf_that_is_a_link_to_a_program_is_not_handed_to_the_system() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(
        root.join("page.adoc"),
        "= Downloads\n\nimage::app.pdf[App]\n\nimage::tool.pdf[Tool]\n",
    )
    .unwrap();
    fs::write(root.join("run.sh"), "#!/bin/sh\necho hello\n").unwrap();
    // A link named like a PDF to a macOS application bundle, and one to a script.
    fs::create_dir_all(root.join("Tool.app/Contents/MacOS")).unwrap();
    std::os::unix::fs::symlink(root.join("Tool.app"), root.join("app.pdf")).unwrap();
    std::os::unix::fs::symlink(root.join("run.sh"), root.join("tool.pdf")).unwrap();
    let mut engine = open_workspace(root);
    let id = match engine
        .invoke(
            ids::DOCUMENT_OPEN,
            CommandArgs::Path(root.join("page.adoc")),
        )
        .unwrap()
    {
        CommandOutcome::Opened(id) => id,
        other => panic!("opening the page gave {other:?}"),
    };

    for name in ["app.pdf", "tool.pdf"] {
        assert_eq!(engine.system_viewer_path(&root.join(name)), None, "{name}");
        let outcome = follow(&mut engine, id, name);
        assert!(
            !matches!(outcome, Ok(CommandOutcome::External(_))),
            "following {name} gave {outcome:?}"
        );
    }
}

#[test]
fn only_web_and_mail_addresses_are_handed_to_the_system() {
    for safe in [
        "http://example.com",
        "https://example.com/page?q=1#top",
        "HTTPS://EXAMPLE.COM",
        "mailto:someone@example.com",
    ] {
        assert!(adoc_engine::is_web_address(safe), "{safe} was refused");
    }
    for unsafe_address in [
        "",
        "file:///etc/passwd",
        "FILE:///C:/Windows/System32/calc.exe",
        "javascript:alert(1)",
        "vscode://file/tmp/x",
        "smb://server/share",
        "ms-settings:privacy",
        "x-apple.systempreferences:com.apple.preference",
        "ssh://host",
        "ftp://example.com/file",
        "\\\\server\\share\\run.exe",
        "//example.com/page",
        "other.html",
        "../setup.exe",
        "C:\\Windows\\notepad.exe",
        "/Applications/Calculator.app",
        "http:",
        "https://",
        "http:example.com",
        " https://example.com",
        "https://exa\nmple.com",
        "mailto:",
    ] {
        assert!(
            !adoc_engine::is_web_address(unsafe_address),
            "{unsafe_address:?} was allowed"
        );
    }
}

#[test]
fn only_pdfs_go_to_the_system_viewer_whatever_their_case() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for name in [
        "a.pdf",
        "B.PDF",
        "run.sh",
        "setup.exe",
        "start.command",
        "x.lnk",
        "y.desktop",
    ] {
        fs::write(root.join(name), b"x").unwrap();
    }
    fs::create_dir_all(root.join("folder.pdf")).unwrap();
    let engine = open_workspace(root);
    assert_eq!(
        engine.system_viewer_path(&root.join("a.pdf")),
        Some(real(&root.join("a.pdf")))
    );
    assert_eq!(
        engine.system_viewer_path(&root.join("B.PDF")),
        Some(real(&root.join("B.PDF")))
    );
    for refused in [
        "run.sh",
        "setup.exe",
        "start.command",
        "x.lnk",
        "y.desktop",
        "folder.pdf",
        "missing.pdf",
    ] {
        assert_eq!(
            engine.system_viewer_path(&root.join(refused)),
            None,
            "{refused}"
        );
    }
}
