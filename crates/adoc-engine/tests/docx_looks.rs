//! A Word export's look: the PDF theme, a Word template or none, chosen by the export, a
//! profile or the settings. The Word looks PRD (`docs/prd/PRD-AdocFoundry_DOCX_Themes.md`),
//! §9 tests 2 (`docx_look_follows_precedence`), 3 (`docx_settings_apply_over_the_look`),
//! 8 (`docx_template_is_used`) and 9 (`docx_template_is_untrusted`).

#![cfg(feature = "docx")]

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_engine::{
    DocxExportArgs, Engine, ExportSummary, ExportTarget, LookChoice, LookKind, Settings,
};

const WAIT: Duration = Duration::from_secs(120);

/// A theme with a page of its own: US Letter, an inch at the top and bottom, half an inch at
/// the sides.
const NARROW: &str = "extends: default\npage:\n  size: Letter\n  margin: [1in, 0.5in]\n";

/// A workspace of one page, `page.adoc` holding `header` after its title, with `settings` as
/// its `.adocfoundry.toml`, a theme `narrow` and a template `corporate.dotx` in `themes/`.
fn workspace(settings: &str, header: &str) -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(directory.path());
    fs::create_dir_all(root.join("themes")).unwrap();
    fs::write(root.join("themes/narrow-theme.yml"), NARROW).unwrap();
    fs::write(
        root.join("themes/wide-theme.yml"),
        "extends: default\npage:\n  layout: landscape\n",
    )
    .unwrap();
    fs::write(
        root.join("themes/corporate.dotx"),
        adoc_docx::testing::template(|_| {}),
    )
    .unwrap();
    fs::write(root.join("page.adoc"), format!("= Page\n{header}\nText.\n")).unwrap();
    fs::write(root.join(".adocfoundry.toml"), settings).unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (directory, root, engine)
}

/// Exports the page with `look` as its own choice.
fn export(
    engine: &mut Engine,
    root: &Path,
    look: Option<LookChoice>,
) -> Result<(ExportSummary, String), String> {
    let id = engine.open(&root.join("page.adoc")).unwrap();
    let output = root.join("page.docx");
    let summary = engine
        .export_docx_blocking(
            DocxExportArgs {
                target: ExportTarget::Page(id),
                output: output.clone(),
                attributes: Vec::new(),
                paper: None,
                landscape: None,
                look,
            },
            WAIT,
        )
        .map_err(|error| error.to_string())?;
    let entries = adoc_docx::testing::assert_valid_package(&fs::read(&output).unwrap());
    let document = adoc_docx::testing::text(&entries, "word/document.xml").to_owned();
    Ok((summary, document))
}

/// The look an export used, as (kind, name).
fn look(summary: &ExportSummary) -> (LookKind, String) {
    let look = summary
        .look
        .as_ref()
        .expect("a Word export reports its look");
    (look.kind, look.name.clone())
}

const A4: &str = r#"<w:pgSz w:w="11906" w:h="16838"/>"#;
const LETTER: &str = r#"<w:pgSz w:w="12240" w:h="15840"/>"#;

#[test]
fn docx_look_follows_precedence() {
    // No theme named anywhere: no theme, today's page, whatever `[docx] look` defaults to.
    let (_directory, root, mut engine) = workspace("", "");
    let (summary, document) = export(&mut engine, &root, None).unwrap();
    assert_eq!(look(&summary), (LookKind::None, "none".to_owned()));
    assert!(document.contains(A4), "{document}");
    assert!(document.contains(r#"w:top="1440""#));

    // `[pdf] theme` written: Word follows it, its page included.
    let settings = "[pdf]\ntheme = \"narrow\"\nthemes_dir = \"themes\"\n";
    let (_directory, root, mut engine) = workspace(settings, "");
    let (summary, document) = export(&mut engine, &root, None).unwrap();
    assert_eq!(look(&summary), (LookKind::PdfTheme, "narrow".to_owned()));
    assert_eq!(
        summary.look.as_ref().unwrap().path.as_deref(),
        Some(root.join("themes/narrow-theme.yml").as_path())
    );
    assert!(document.contains(LETTER), "{document}");
    assert!(document.contains(r#"w:top="1440" w:right="720" w:bottom="1440" w:left="720""#));

    // The page's `pdf-theme` wins over `[pdf] theme`, and its `pdf-page-size` over the theme's.
    let (_directory, root, mut engine) =
        workspace(settings, ":pdf-theme: wide\n:pdf-page-size: A4\n");
    let (summary, document) = export(&mut engine, &root, None).unwrap();
    assert_eq!(look(&summary), (LookKind::PdfTheme, "wide".to_owned()));
    assert!(document.contains(r#"<w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/>"#));

    // The export's own choice wins over the settings and the page.
    let (summary, document) = export(&mut engine, &root, Some(LookChoice::None)).unwrap();
    assert_eq!(look(&summary).0, LookKind::None);
    assert!(document.contains(A4));
    let (summary, document) = export(
        &mut engine,
        &root,
        Some(LookChoice::PdfTheme("narrow".to_owned())),
    )
    .unwrap();
    assert_eq!(look(&summary), (LookKind::PdfTheme, "narrow".to_owned()));
    // The page's own size still wins over the theme's.
    assert!(document.contains(A4), "{document}");
    let (summary, _) = export(
        &mut engine,
        &root,
        Some(LookChoice::Template("corporate".to_owned())),
    )
    .unwrap();
    assert_eq!(look(&summary), (LookKind::Template, "corporate".to_owned()));
    assert_eq!(
        summary.look.as_ref().unwrap().path.as_deref(),
        Some(root.join("themes/corporate.dotx").as_path())
    );
    // `--theme default` names the bundled default theme.
    let (summary, _) = export(
        &mut engine,
        &root,
        Some(LookChoice::PdfTheme("default".to_owned())),
    )
    .unwrap();
    assert_eq!(look(&summary), (LookKind::PdfTheme, "default".to_owned()));

    // `[docx] look` sets the default: none, or a template it names.
    let (_directory, root, mut engine) =
        workspace(&format!("{settings}\n[docx]\nlook = \"none\"\n"), "");
    assert_eq!(
        look(&export(&mut engine, &root, None).unwrap().0).0,
        LookKind::None
    );
    let (_directory, root, mut engine) = workspace(
        &format!("{settings}\n[docx]\nlook = \"template\"\ntemplate = \"corporate\"\n"),
        "",
    );
    let (summary, _) = export(&mut engine, &root, None).unwrap();
    assert_eq!(look(&summary), (LookKind::Template, "corporate".to_owned()));

    // A profile's choice is its own, and an export's wins over it.
    let profiles = format!(
        "{settings}\n[docx.profiles.plain]\npage = \"page.adoc\"\nlook = \"none\"\n\n\
         [docx.profiles.themed]\npage = \"page.adoc\"\ntheme = \"wide\"\n\n\
         [docx.profiles.both]\npage = \"page.adoc\"\ntheme = \"wide\"\ntemplate = \"corporate\"\n"
    );
    let (_directory, _root, mut engine) = workspace(&profiles, "");
    let mut args = engine.docx_profile_export_args("plain").unwrap();
    assert_eq!(args.look, Some(LookChoice::None));
    assert_eq!(
        look(&engine.export_docx_blocking(args.clone(), WAIT).unwrap()).0,
        LookKind::None
    );
    args.look = Some(LookChoice::PdfTheme("narrow".to_owned()));
    assert_eq!(
        look(&engine.export_docx_blocking(args, WAIT).unwrap()).1,
        "narrow"
    );
    assert_eq!(
        engine.docx_profile_export_args("themed").unwrap().look,
        Some(LookChoice::PdfTheme("wide".to_owned()))
    );
    // A profile names at most one look.
    assert!(engine.docx_profile_export_args("both").is_err());
    assert!(
        engine
            .settings()
            .warnings
            .iter()
            .any(|warning| warning.contains("both") && warning.contains("at most one")),
        "{:?}",
        engine.settings().warnings
    );

    // A theme or template that is not there fails the export, naming where it was looked for.
    let (_directory, root, mut engine) = workspace(settings, "");
    let error = export(
        &mut engine,
        &root,
        Some(LookChoice::PdfTheme("missing".to_owned())),
    )
    .unwrap_err();
    assert!(error.contains("missing-theme.yml"), "{error}");
    let error = export(
        &mut engine,
        &root,
        Some(LookChoice::Template("missing".to_owned())),
    )
    .unwrap_err();
    assert!(error.contains("missing.dotx"), "{error}");
    let (_directory, root, mut engine) = workspace("[docx]\nlook = \"template\"\n", "");
    let error = export(&mut engine, &root, None).unwrap_err();
    assert!(error.contains("no template"), "{error}");
}

#[test]
fn docx_settings_apply_over_the_look() {
    // The theme gives Letter and its margins; the settings write the paper and the top margin.
    let settings = "[pdf]\ntheme = \"narrow\"\nthemes_dir = \"themes\"\n\n\
                    [docx]\npaper = \"a4\"\n\n[docx.margins]\ntop = \"30mm\"\n";
    let (_directory, root, mut engine) = workspace(settings, "");
    let (summary, document) = export(&mut engine, &root, None).unwrap();
    assert_eq!(look(&summary).0, LookKind::PdfTheme);
    assert!(document.contains(A4), "the written paper wins: {document}");
    // The written top margin wins; the sides and bottom are the theme's.
    assert!(
        document.contains(r#"w:top="1701" w:right="720" w:bottom="1440" w:left="720""#),
        "{document}"
    );

    // Orientation written in the settings wins over a landscape theme.
    let settings =
        "[pdf]\ntheme = \"wide\"\nthemes_dir = \"themes\"\n\n[docx]\norientation = \"portrait\"\n";
    let (_directory, root, mut engine) = workspace(settings, "");
    let (_, document) = export(&mut engine, &root, None).unwrap();
    assert!(document.contains(A4), "{document}");

    // Nothing written: the theme's page, every side of it.
    let (_directory, root, mut engine) =
        workspace("[pdf]\ntheme = \"narrow\"\nthemes_dir = \"themes\"\n", "");
    let (_, document) = export(&mut engine, &root, None).unwrap();
    assert!(document.contains(LETTER));

    // A template's page and footer give way to what the settings wrote, key by key.
    let settings = "[pdf]\nthemes_dir = \"themes\"\n\n[docx]\nlook = \"template\"\n\
                    template = \"corporate\"\n\n[docx.margins]\ntop = \"30mm\"\n\n\
                    [docx.footer]\ncenter = \"Written {page-number}\"\n";
    let (_directory, root, mut engine) = workspace(settings, "");
    let (summary, document) = export(&mut engine, &root, None).unwrap();
    assert_eq!(look(&summary).0, LookKind::Template);
    assert!(
        document.contains(LETTER),
        "the template's paper: {document}"
    );
    assert!(
        document.contains(
            r#"w:top="1701" w:right="1100" w:bottom="1200" w:left="1300" w:header="500" w:footer="600""#
        ),
        "{document}"
    );
    let entries =
        adoc_docx::testing::assert_valid_package(&fs::read(root.join("page.docx")).unwrap());
    let footers: Vec<&str> = entries
        .keys()
        .filter(|name| name.starts_with("word/footer"))
        .map(|name| adoc_docx::testing::text(&entries, name))
        .collect();
    assert!(footers.iter().any(|footer| footer.contains("Written")));
    assert!(footers.iter().all(|footer| !footer.contains("example.com")));
    // The header nobody wrote is still the template's.
    assert!(adoc_docx::testing::text(&entries, "word/header1.xml").contains("CORPORATE HEADER"));
}

#[test]
fn docx_template_is_used() {
    let settings = "[pdf]\nthemes_dir = \"themes\"\n";
    let (_directory, root, mut engine) = workspace(settings, "");
    fs::write(
        root.join("page.adoc"),
        "= Page\n\nText.\n\n== Steps\n\n. One\n. Two\n\n----\ncode\n----\n",
    )
    .unwrap();
    let (summary, document) = export(
        &mut engine,
        &root,
        Some(LookChoice::Template("corporate".to_owned())),
    )
    .unwrap();
    assert_eq!(look(&summary), (LookKind::Template, "corporate".to_owned()));
    assert_eq!(
        summary.look.as_ref().unwrap().path.as_deref(),
        Some(root.join("themes/corporate.dotx").as_path())
    );
    assert!(summary.diagnostics.is_empty(), "{:?}", summary.diagnostics);
    let entries =
        adoc_docx::testing::assert_valid_package(&fs::read(root.join("page.docx")).unwrap());
    let text = |name: &str| adoc_docx::testing::text(&entries, name);

    // Its styles, with the export's added where it has none; its numbering, then the export's.
    let styles = text("word/styles.xml");
    assert!(styles.contains(r#"<w:color w:val="1F4E79"/><w:sz w:val="36"/>"#));
    assert!(styles.contains(r#"w:styleId="CorporateNote""#));
    assert!(styles.contains(r#"w:styleId="SourceCode""#));
    assert!(text("word/numbering.xml").contains(r#"<w:num w:numId="2">"#));
    assert!(document.contains(r#"<w:numId w:val="2"/>"#));
    // Its page, headers and footers, theme and font table; nothing of its body.
    assert!(document.contains(LETTER), "{document}");
    assert!(!document.contains("TEMPLATE BODY TEXT"));
    assert!(text("word/header1.xml").contains("CORPORATE HEADER"));
    assert!(entries.contains_key("word/media/template1.png"));
    assert!(entries.contains_key("word/theme/theme1.xml"));
    assert!(text("word/fontTable.xml").contains("Arial"));
}

#[test]
fn docx_template_is_untrusted() {
    let settings = "[pdf]\nthemes_dir = \"themes\"\n";
    let (_directory, root, mut engine) = workspace(settings, "");
    let write =
        |name: &str, bytes: Vec<u8>| fs::write(root.join("themes").join(name), bytes).unwrap();
    let edited = |name: &'static str, from: &'static str, to: &'static str| {
        adoc_docx::testing::template(move |entries| {
            let text = String::from_utf8(entries[name].clone())
                .unwrap()
                .replace(from, to);
            entries.insert(name.to_owned(), text.into_bytes());
        })
    };
    let failure = |engine: &mut Engine, name: &str| {
        export(engine, &root, Some(LookChoice::Template(name.to_owned()))).unwrap_err()
    };

    // Refused, failing the export with the file and the reason.
    write(
        "macros.dotx",
        edited(
            "[Content_Types].xml",
            "officedocument.wordprocessingml.template.main+xml",
            "ms-word.template.macroEnabledTemplate.main+xml",
        ),
    );
    let error = failure(&mut engine, "macros");
    assert!(
        error.contains("macros.dotx") && error.contains("macros"),
        "{error}"
    );
    write(
        "project.dotx",
        adoc_docx::testing::template(|entries| {
            entries.insert("word/vbaProject.bin".to_owned(), vec![1; 8]);
        }),
    );
    assert!(failure(&mut engine, "project").contains("macros"));
    write("huge.dotx", vec![0; adoc_docx::MAX_TEMPLATE_BYTES + 1]);
    assert!(failure(&mut engine, "huge").contains("too large"));
    write(
        "broken.dotx",
        edited("word/styles.xml", "</w:styles>", "</w:style>"),
    );
    let error = failure(&mut engine, "broken");
    assert!(
        error.contains("word/styles.xml") && error.contains("not well formed"),
        "{error}"
    );

    // Left out with a warning: a picture linked from outside, an altChunk, a field that reads
    // a file. Nothing of them reaches the export.
    write(
        "linked.dotx",
        adoc_docx::testing::template(|entries| {
            entries.insert(
                "word/_rels/header1.xml.rels".to_owned(),
                adoc_docx::testing::relationships(&[(
                    "rIdLogo",
                    "image",
                    "https://tracker.example/pixel.png",
                    true,
                )])
                .into_bytes(),
            );
            entries.insert(
                "word/_rels/footer1.xml.rels".to_owned(),
                adoc_docx::testing::relationships(&[
                    ("rIdSite", "hyperlink", "https://example.com/", true),
                    ("rIdChunk", "aFChunk", "chunk.html", false),
                ])
                .into_bytes(),
            );
            entries.insert("word/chunk.html".to_owned(), b"<p>chunk</p>".to_vec());
        }),
    );
    let (summary, _) = export(
        &mut engine,
        &root,
        Some(LookChoice::Template("linked".to_owned())),
    )
    .unwrap();
    let messages: Vec<&str> = summary
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("tracker.example")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("chunk.html")),
        "{messages:?}"
    );
    let bytes = fs::read(root.join("page.docx")).unwrap();
    let entries = adoc_docx::testing::assert_valid_package(&bytes);
    for (name, data) in &entries {
        let data = String::from_utf8_lossy(data);
        assert!(
            !data.contains("tracker.example") && !data.contains("chunk"),
            "{name}"
        );
    }
    assert!(!entries.contains_key("word/media/template1.png"));

    // A link's id used as a picture to fetch.
    write(
        "fetching.dotx",
        adoc_docx::testing::template(|entries| {
            entries.insert(
                "word/_rels/header1.xml.rels".to_owned(),
                adoc_docx::testing::relationships(&[
                    ("rIdLogo", "image", "media/logo.png", false),
                    (
                        "rIdTrack",
                        "hyperlink",
                        "https://tracker.example/p.png",
                        true,
                    ),
                ])
                .into_bytes(),
            );
            let header = String::from_utf8(entries["word/header1.xml"].clone())
                .unwrap()
                .replace(r#"r:id="rIdLogo""#, r#"r:id="rIdLogo" r:href="rIdTrack""#);
            entries.insert("word/header1.xml".to_owned(), header.into_bytes());
        }),
    );
    let (summary, _) = export(
        &mut engine,
        &root,
        Some(LookChoice::Template("fetching".to_owned())),
    )
    .unwrap();
    assert!(summary
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("rIdTrack")));
    let entries =
        adoc_docx::testing::assert_valid_package(&fs::read(root.join("page.docx")).unwrap());
    assert!(entries
        .values()
        .all(|data| !String::from_utf8_lossy(data).contains("tracker.example")));

    write(
        "including.dotx",
        edited(
            "word/footer1.xml",
            "> PA<",
            "> INCLUDETEXT \"C:/secret.txt\" <",
        ),
    );
    let (summary, _) = export(
        &mut engine,
        &root,
        Some(LookChoice::Template("including".to_owned())),
    )
    .unwrap();
    assert!(summary
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.message.contains("INCLUDETEXT")));
    let entries =
        adoc_docx::testing::assert_valid_package(&fs::read(root.join("page.docx")).unwrap());
    assert!(entries
        .values()
        .all(|data| !String::from_utf8_lossy(data).contains("secret")));
}

/// A book of two pages in `brand`'s look, from `tests/fixtures/docx/themes`, with `settings`
/// after the theme's own.
fn brand_book(settings: &str) -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(directory.path());
    let themes = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/docx/themes");
    fs::create_dir_all(root.join("themes")).unwrap();
    for entry in fs::read_dir(themes).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), root.join("themes").join(entry.file_name())).unwrap();
    }
    let pages = root.join("guide/modules/ROOT/pages");
    fs::create_dir_all(&pages).unwrap();
    fs::write(
        root.join("guide/antora.yml"),
        "name: guide\ntitle: The Guide\nversion: '1.0'\nnav:\n- modules/ROOT/nav.adoc\n\
         asciidoc:\n  attributes:\n    author: Paul Snow\n",
    )
    .unwrap();
    fs::write(
        root.join("guide/modules/ROOT/nav.adoc"),
        "* xref:index.adoc[]\n* xref:more.adoc[]\n",
    )
    .unwrap();
    fs::write(pages.join("index.adoc"), "= Start\n\nThe start.\n").unwrap();
    fs::write(
        pages.join("more.adoc"),
        "= More\n\n== A section\n\nSee https://example.com[a link] and `code`.\n\n\
         .A listing\n[source,sh]\n----\necho hi\n----\n\n\
         ____\nA quotation.\n____\n\nNOTE: A note.\n\n.A sidebar\n****\nAside.\n****\n\n\
         .An example\n====\nExample.\n====\n\n.A table\n|===\n|Head |Head\n|a |b\n|c |d\n|===\n",
    )
    .unwrap();
    fs::write(
        root.join(".adocfoundry.toml"),
        format!("[pdf]\ntheme = \"brand\"\nthemes_dir = \"themes\"\n{settings}"),
    )
    .unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (directory, root, engine)
}

/// Exports `brand_book`'s component and gives the package's parts.
fn export_book(engine: &mut Engine, root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let output = root.join("guide.docx");
    let summary = engine
        .export_docx_blocking(
            DocxExportArgs {
                target: ExportTarget::Component {
                    name: "guide".to_owned(),
                    version: Some("1.0".to_owned()),
                },
                output: output.clone(),
                attributes: Vec::new(),
                paper: None,
                landscape: None,
                look: None,
            },
            WAIT,
        )
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(look(&summary), (LookKind::PdfTheme, "brand".to_owned()));
    adoc_docx::testing::assert_valid_package(&fs::read(&output).unwrap())
}

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// The value of `attribute` on the first `element` inside the style `id` (or the document's
/// defaults, for `id` empty), as `styles.xml` gives it.
fn style_value(styles: &str, id: &str, element: &str, attribute: &str) -> Option<String> {
    let xml = roxmltree::Document::parse(styles).unwrap();
    let scope = if id.is_empty() {
        xml.descendants()
            .find(|node| node.has_tag_name((W, "docDefaults")))?
    } else {
        xml.descendants().find(|node| {
            node.has_tag_name((W, "style")) && node.attribute((W, "styleId")) == Some(id)
        })?
    };
    let found = scope
        .descendants()
        .find(|node| node.has_tag_name((W, element)))?;
    Some(found.attribute((W, attribute)).unwrap_or("").to_owned())
}

/// The Word looks PRD, §9 test 4: a PDF theme's body, headings, links, code, captions,
/// quotes and boxes become the Word styles they style, each still a style a reader can change.
#[test]
fn docx_pdf_theme_styles() {
    let (_directory, root, mut engine) = brand_book("");
    let entries = export_book(&mut engine, &root);
    let styles = adoc_docx::testing::text(&entries, "word/styles.xml");
    let value =
        |id: &str, element: &str, attribute: &str| style_value(styles, id, element, attribute);
    let has = |id: &str, element: &str| value(id, element, "val").is_some();

    // The body: the document's defaults and Normal.
    assert_eq!(value("", "rFonts", "ascii").as_deref(), Some("Noto Sans"));
    assert_eq!(value("", "sz", "val").as_deref(), Some("20"));
    assert_eq!(value("", "color", "val").as_deref(), Some("333333"));
    assert_eq!(value("", "spacing", "line").as_deref(), Some("360"));
    assert_eq!(value("Normal", "jc", "val").as_deref(), Some("both"));
    // Headings: the shared keys, each level's own over them.
    assert_eq!(
        value("Heading2", "rFonts", "ascii").as_deref(),
        Some("M+ 1mn")
    );
    assert_eq!(value("Heading2", "sz", "val").as_deref(), Some("40"));
    assert_eq!(value("Heading2", "color", "val").as_deref(), Some("1F4E79"));
    assert_eq!(value("Heading3", "color", "val").as_deref(), Some("7030A0"));
    assert!(has("Heading2", "i") && has("Heading2", "caps"));
    assert_eq!(
        value("Heading2", "spacing", "before").as_deref(),
        Some("240")
    );
    assert_eq!(
        value("Heading2", "spacing", "after").as_deref(),
        Some("120")
    );
    // Links, code and code spans.
    assert_eq!(
        value("Hyperlink", "color", "val").as_deref(),
        Some("C00000")
    );
    assert_eq!(value("Hyperlink", "u", "val").as_deref(), Some("none"));
    assert_eq!(
        value("SourceCode", "rFonts", "ascii").as_deref(),
        Some("M+ 1mn")
    );
    assert_eq!(value("SourceCode", "sz", "val").as_deref(), Some("18"));
    assert_eq!(
        value("SourceCode", "color", "val").as_deref(),
        Some("006400")
    );
    assert_eq!(
        value("SourceCode", "shd", "fill").as_deref(),
        Some("FFFFE0")
    );
    assert_eq!(
        value("SourceCode", "top", "color").as_deref(),
        Some("999999")
    );
    assert_eq!(value("SourceCode", "top", "sz").as_deref(), Some("6"));
    assert_eq!(value("CodeChar", "color", "val").as_deref(), Some("8B0000"));
    assert_eq!(value("CodeChar", "shd", "fill").as_deref(), Some("EEEEEE"));
    // Captions and quotes.
    assert_eq!(value("Caption", "color", "val").as_deref(), Some("555555"));
    assert_eq!(value("Caption", "jc", "val").as_deref(), Some("center"));
    assert!(has("Caption", "b"));
    assert_eq!(value("Quote", "color", "val").as_deref(), Some("444444"));
    assert_eq!(value("Quote", "left", "color").as_deref(), Some("0070C0"));
    assert_eq!(value("Quote", "left", "sz").as_deref(), Some("32"));
    // Boxes.
    assert_eq!(
        value("AdocAdmonition", "left", "color").as_deref(),
        Some("FF8C00")
    );
    assert_eq!(value("AdocAdmonition", "left", "sz").as_deref(), Some("16"));
    assert_eq!(
        value("AdocAdmonition", "shd", "fill").as_deref(),
        Some("F0F8FF")
    );
    assert_eq!(
        value("AdocSidebar", "top", "color").as_deref(),
        Some("DB7093")
    );
    assert_eq!(
        value("AdocSidebar", "shd", "fill").as_deref(),
        Some("FFF0F5")
    );
    assert_eq!(
        value("AdocExample", "top", "color").as_deref(),
        Some("2E8B57")
    );
    assert_eq!(
        value("AdocExample", "shd", "fill").as_deref(),
        Some("F5FFFA")
    );
    assert_eq!(
        value("AdocTable", "insideH", "color").as_deref(),
        Some("00008B")
    );
    assert_eq!(value("AdocTable", "insideH", "sz").as_deref(), Some("8"));
    let xml = roxmltree::Document::parse(styles).unwrap();
    let table = xml
        .descendants()
        .find(|node| node.attribute((W, "styleId")) == Some("AdocTable"))
        .unwrap();
    let conditional = |kind: &str| {
        table.descendants().find(|node| {
            node.has_tag_name((W, "tblStylePr")) && node.attribute((W, "type")) == Some(kind)
        })
    };
    let fill = |node: roxmltree::Node| {
        node.descendants()
            .find(|node| node.has_tag_name((W, "shd")))
            .and_then(|node| node.attribute((W, "fill")))
            .map(str::to_owned)
    };
    assert_eq!(
        fill(conditional("firstRow").unwrap()).as_deref(),
        Some("ADD8E6")
    );
    assert_eq!(
        fill(conditional("band2Horz").unwrap()).as_deref(),
        Some("F0F0F0")
    );

    // What `[docx.styles]` writes wins over the theme; the rest stays the theme's.
    let (_directory, root, mut engine) =
        brand_book("\n[docx.styles]\nheading_colour = \"112233\"\nbody_font = \"Georgia\"\n");
    let entries = export_book(&mut engine, &root);
    let styles = adoc_docx::testing::text(&entries, "word/styles.xml");
    assert_eq!(
        style_value(styles, "Heading2", "color", "val").as_deref(),
        Some("112233")
    );
    assert_eq!(
        style_value(styles, "", "rFonts", "ascii").as_deref(),
        Some("Georgia")
    );
    assert_eq!(
        style_value(styles, "Heading2", "sz", "val").as_deref(),
        Some("40")
    );
}

/// The Word looks PRD, §9 test 5: the theme's page, title page and covers.
#[test]
fn docx_pdf_theme_page_and_title_page() {
    let (_directory, root, mut engine) = brand_book("");
    let entries = export_book(&mut engine, &root);
    let document = adoc_docx::testing::text(&entries, "word/document.xml");
    // A5 landscape, three-quarters of an inch top and bottom, an inch at the sides.
    assert!(
        document.contains(r#"<w:pgSz w:w="11906" w:h="8391" w:orient="landscape"/>"#),
        "{document}"
    );
    assert!(document.contains(r#"w:top="1080" w:right="1440" w:bottom="1080" w:left="1440""#));
    // The front cover is the theme's, the title page left-aligned with its logo at the right.
    assert!(
        document.contains(r#"descr="Front cover""#),
        "the theme's cover"
    );
    assert!(document.contains(r#"descr="Logo""#), "the theme's logo");
    let paragraphs = adoc_docx::testing::paragraphs(document);
    assert!(paragraphs
        .iter()
        .any(|(style, text)| style.as_deref() == Some("Title") && text == "The Guide"));
    let xml = roxmltree::Document::parse(document).unwrap();
    let alignment = |predicate: &dyn Fn(roxmltree::Node) -> bool| {
        xml.descendants()
            .filter(|node| node.has_tag_name((W, "p")))
            .find(|node| predicate(*node))
            .and_then(|node| node.descendants().find(|node| node.has_tag_name((W, "jc"))))
            .and_then(|node| node.attribute((W, "val")))
            .map(str::to_owned)
    };
    let styled = |id: &'static str| {
        move |node: roxmltree::Node| {
            node.descendants().any(|child| {
                child.has_tag_name((W, "pStyle")) && child.attribute((W, "val")) == Some(id)
            })
        }
    };
    assert_eq!(alignment(&styled("Title")).as_deref(), Some("left"));
    let logo = |node: roxmltree::Node| {
        node.descendants().any(|child| {
            child.tag_name().name() == "docPr" && child.attribute("descr") == Some("Logo")
        })
    };
    assert_eq!(alignment(&logo).as_deref(), Some("right"));
    let styles = adoc_docx::testing::text(&entries, "word/styles.xml");
    assert_eq!(
        style_value(styles, "Title", "sz", "val").as_deref(),
        Some("60")
    );
    assert_eq!(
        style_value(styles, "Title", "color", "val").as_deref(),
        Some("8B008B")
    );
    assert_eq!(
        style_value(styles, "TitlePageDetail", "color", "val").as_deref(),
        Some("2F4F4F")
    );
}

/// A theme's logo and covers are read as every picture is, through the engine's file access: a
/// confined engine (the MCP server's) leaves out one the theme names outside the roots.
#[test]
fn theme_images_outside_the_roots_are_not_embedded() {
    let directory = tempfile::tempdir().unwrap();
    let outside = adoc_engine::normalize_path(directory.path());
    let root = outside.join("docs");
    fs::create_dir_all(root.join("themes")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/docx/themes/brand-cover.png"),
        outside.join("secret.png"),
    )
    .unwrap();
    fs::write(
        root.join("themes/leak-theme.yml"),
        "extends: default\ncover:\n  front:\n    image: ../../secret.png\ntitle_page:\n  logo:\n    image: image:../../secret.png[]\n",
    )
    .unwrap();
    fs::write(root.join("page.adoc"), "= Page\n:title-page:\n\nText.\n").unwrap();
    fs::write(
        root.join(".adocfoundry.toml"),
        "[pdf]\ntheme = \"leak\"\nthemes_dir = \"themes\"\n",
    )
    .unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.set_file_access(adoc_engine::FileAccess::within([root.clone()]));
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    let (summary, document) = export(&mut engine, &root, None).unwrap();
    assert_eq!(look(&summary).0, LookKind::PdfTheme);
    assert!(
        !document.contains("<wp:inline"),
        "nothing from outside is drawn: {document}"
    );
    assert!(
        summary
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("outside")),
        "{:?}",
        summary.diagnostics
    );
}

/// The Word looks PRD, §9 test 6: the theme's headers and footers, odd (recto) and even
/// (verso), with their fields, attributes, image, font and rule; written `[docx]` lines win.
#[test]
fn docx_pdf_theme_running_content() {
    let (_directory, root, mut engine) = brand_book("");
    let entries = export_book(&mut engine, &root);
    let document = adoc_docx::testing::text(&entries, "word/document.xml");
    let settings = adoc_docx::testing::text(&entries, "word/settings.xml");
    assert!(settings.contains("<w:evenAndOddHeaders/>"));
    // Each reference of the body's section, by kind, to its part's text.
    let rels = adoc_docx::testing::text(&entries, "word/_rels/document.xml.rels");
    let part = |id: &str| {
        let at = rels.find(&format!("Id=\"{id}\"")).unwrap();
        let target = &rels[at..];
        let start = target.find("Target=\"").unwrap() + 8;
        let end = target[start..].find('"').unwrap();
        adoc_docx::testing::text(&entries, &format!("word/{}", &target[start..start + end]))
            .to_owned()
    };
    let xml = roxmltree::Document::parse(document).unwrap();
    let body_section = xml
        .descendants()
        .filter(|node| node.has_tag_name((W, "sectPr")))
        .find(|node| {
            node.descendants()
                .any(|child| child.has_tag_name((W, "pgNumType")))
        })
        .expect("the body's section");
    let reference = |element: &str, kind: &str| {
        body_section
            .children()
            .find(|node| {
                node.has_tag_name((W, element)) && node.attribute((W, "type")) == Some(kind)
            })
            .and_then(|node| {
                node.attribute((
                    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
                    "id",
                ))
            })
            .map(part)
            .unwrap_or_else(|| panic!("no {kind} {element}"))
    };
    let text = |part: &str| {
        adoc_docx::testing::paragraphs(part)
            .into_iter()
            .map(|(_, text)| text)
            .collect::<String>()
    };
    let odd_header = reference("headerReference", "default");
    assert!(text(&odd_header).contains("The Guide"), "{odd_header}");
    assert!(
        odd_header.contains("STYLEREF \"heading 1\""),
        "{odd_header}"
    );
    let even_header = reference("headerReference", "even");
    assert!(
        even_header.contains("STYLEREF \"heading 2\""),
        "{even_header}"
    );
    assert!(
        even_header.contains("<w:drawing>"),
        "the theme's image: {even_header}"
    );
    // An inch wide.
    assert!(even_header.contains(r#"cx="914400""#), "{even_header}");
    let odd_footer = reference("footerReference", "default");
    assert!(
        odd_footer.contains(" PAGE ") && odd_footer.contains(" NUMPAGES "),
        "{odd_footer}"
    );
    let footer_text = text(&odd_footer);
    assert!(footer_text.contains("of"), "{footer_text}");
    assert!(
        footer_text.contains("· v1.0"),
        "the version and the character: {footer_text}"
    );
    assert!(
        !footer_text.contains('*'),
        "bold marks left out: {footer_text}"
    );
    assert!(
        !footer_text.contains("Draft"),
        "a line naming an unset attribute is dropped: {footer_text}"
    );
    let even_footer = reference("footerReference", "even");
    assert!(
        even_footer.contains(" PAGE ") && !text(&even_footer).contains('_'),
        "{even_footer}"
    );

    // The Header and Footer styles: the theme's font colour and size, the header's rule.
    let styles = adoc_docx::testing::text(&entries, "word/styles.xml");
    assert_eq!(
        style_value(styles, "Header", "color", "val").as_deref(),
        Some("595959")
    );
    assert_eq!(
        style_value(styles, "Header", "sz", "val").as_deref(),
        Some("16")
    );
    assert_eq!(
        style_value(styles, "Header", "bottom", "color").as_deref(),
        Some("CC0000")
    );
    assert_eq!(
        style_value(styles, "Header", "bottom", "sz").as_deref(),
        Some("4")
    );
    assert_eq!(
        style_value(styles, "Footer", "top", "color"),
        None,
        "no rule of width 0"
    );
    // The header sits in the middle of its 0.6 inch box: (43.2 - 8 * 1.2) / 2 points.
    assert!(document.contains(r#"w:header="336""#), "{document}");

    // A footer written in the settings wins over the theme's; the header stays the theme's.
    let (_directory, root, mut engine) = brand_book("\n[docx.footer]\nright = \"Written\"\n");
    let entries = export_book(&mut engine, &root);
    let parts: Vec<String> = entries
        .keys()
        .filter(|name| name.starts_with("word/footer"))
        .map(|name| adoc_docx::testing::text(&entries, name).to_owned())
        .collect();
    assert!(
        parts.iter().any(|part| part.contains("Written")),
        "{parts:?}"
    );
    assert!(
        parts.iter().all(|part| !part.contains("NUMPAGES")),
        "{parts:?}"
    );
    assert!(entries
        .keys()
        .filter(|name| name.starts_with("word/header"))
        .any(|name| adoc_docx::testing::text(&entries, name).contains("The Guide")));
}

/// `font` with its `fsType` set to `value`, found through the font's table directory.
fn with_fs_type(font: &[u8], value: u16) -> Vec<u8> {
    let at = |offset: usize| u32::from_be_bytes(font[offset..offset + 4].try_into().unwrap());
    let tables = usize::from(u16::from_be_bytes([font[4], font[5]]));
    let record = (0..tables)
        .map(|index| 12 + index * 16)
        .find(|record| &font[*record..*record + 4] == b"OS/2")
        .expect("an OS/2 table");
    let os2 = at(record + 8) as usize;
    let mut font = font.to_vec();
    font[os2 + 8..os2 + 10].copy_from_slice(&value.to_be_bytes());
    font
}

/// A workspace whose theme names its fonts under a family name of its own: the regular face
/// a file of its own, the bold face bundled, the italic one a file whose licence forbids
/// embedding, the bold italic one missing; and the bundled monospace font for code.
fn fonts_workspace(settings: &str) -> (tempfile::TempDir, PathBuf, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(directory.path());
    let regular = adoc_theme::bundled_font("notoserif-regular-subset.ttf").unwrap();
    fs::create_dir_all(root.join("themes/fonts")).unwrap();
    fs::write(root.join("themes/fonts/brand-regular.ttf"), regular).unwrap();
    fs::write(
        root.join("themes/fonts/brand-italic.ttf"),
        with_fs_type(
            adoc_theme::bundled_font("notoserif-italic-subset.ttf").unwrap(),
            0x0002,
        ),
    )
    .unwrap();
    fs::write(
        root.join("themes/brand-theme.yml"),
        "extends: default\nfont:\n  catalog:\n    Brand Serif:\n      normal: fonts/brand-regular.ttf\n\
         \x20     bold: GEM_FONTS_DIR/notoserif-bold-subset.ttf\n      italic: fonts/brand-italic.ttf\n\
         \x20     bold_italic: fonts/missing.ttf\n    M+ 1mn:\n\
         \x20     normal: GEM_FONTS_DIR/mplus1mn-regular-subset.ttf\nbase:\n  font_family: Brand Serif\n",
    )
    .unwrap();
    fs::write(root.join("page.adoc"), "= Page\n\nText and `code`.\n").unwrap();
    fs::write(
        root.join(".adocfoundry.toml"),
        format!("[pdf]\ntheme = \"brand\"\nthemes_dir = \"themes\"\n{settings}"),
    )
    .unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    (directory, root, engine)
}

/// The font table's families and, for each, the faces embedded in it.
fn font_table(entries: &std::collections::BTreeMap<String, Vec<u8>>) -> Vec<(String, Vec<String>)> {
    let Some(_) = entries.get("word/fontTable.xml") else {
        return Vec::new();
    };
    let table = adoc_docx::testing::text(entries, "word/fontTable.xml");
    let xml = roxmltree::Document::parse(table).unwrap();
    xml.descendants()
        .filter(|node| node.has_tag_name((W, "font")))
        .map(|font| {
            (
                font.attribute((W, "name")).unwrap().to_owned(),
                font.children()
                    .filter(|node| node.is_element())
                    .map(|node| node.tag_name().name().to_owned())
                    .collect(),
            )
        })
        .collect()
}

/// The Word looks PRD, §9 test 7: the fonts the look's styles name are embedded, obfuscated
/// and read back through the font table, under the names the fonts give themselves; a face
/// whose licence forbids it is named and not embedded, with a warning, as is a missing one;
/// `embed_fonts = false` names the fonts only; no theme names none; the same export gives the
/// same bytes.
#[test]
fn docx_fonts_are_embedded() {
    let (_directory, root, mut engine) = fonts_workspace("");
    let (summary, _) = export(&mut engine, &root, None).unwrap();
    assert_eq!(look(&summary), (LookKind::PdfTheme, "brand".to_owned()));
    let bytes = fs::read(root.join("page.docx")).unwrap();
    let entries = adoc_docx::testing::assert_valid_package(&bytes);
    let table = font_table(&entries);
    // The theme's family is written under the font's own name, which the styles use too.
    let names: Vec<&str> = table.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["Noto Serif", "M+ 1mn"]);
    let styles = adoc_docx::testing::text(&entries, "word/styles.xml");
    assert_eq!(
        style_value(styles, "", "rFonts", "ascii").as_deref(),
        Some("Noto Serif")
    );
    assert!(!styles.contains("Brand Serif"));
    // Regular and bold embedded; the italic face's licence forbids it; bold italic is missing.
    let faces = &table
        .iter()
        .find(|(name, _)| name == "Noto Serif")
        .unwrap()
        .1;
    assert_eq!(faces, &["embedRegular", "embedBold"]);
    let fonts = entries
        .keys()
        .filter(|name| name.starts_with("word/fonts/"))
        .count();
    assert_eq!(fonts, 3);
    let warnings: Vec<&str> = summary
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("italic face") && warning.contains("licence")),
        "{warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("bold italic face") && warning.contains("missing.ttf")),
        "{warnings:?}"
    );
    assert!(
        adoc_docx::testing::text(&entries, "word/settings.xml").contains("w:embedTrueTypeFonts")
    );
    // The same export gives the same bytes.
    export(&mut engine, &root, None).unwrap();
    assert_eq!(fs::read(root.join("page.docx")).unwrap(), bytes);

    // Not embedding names the fonts only.
    let (_directory, root, mut engine) = fonts_workspace("[docx]\nembed_fonts = false\n");
    export(&mut engine, &root, None).unwrap();
    let entries =
        adoc_docx::testing::assert_valid_package(&fs::read(root.join("page.docx")).unwrap());
    let table = font_table(&entries);
    assert_eq!(table.len(), 2, "{table:?}");
    assert!(table.iter().all(|(_, faces)| faces.is_empty()), "{table:?}");
    assert!(!entries.keys().any(|name| name.starts_with("word/fonts/")));

    // No theme, no fonts.
    export(&mut engine, &root, Some(LookChoice::None)).unwrap();
    let entries =
        adoc_docx::testing::assert_valid_package(&fs::read(root.join("page.docx")).unwrap());
    assert!(!entries.contains_key("word/fontTable.xml"));
}

#[test]
fn theme_fonts_outside_the_roots_are_not_embedded() {
    let directory = tempfile::tempdir().unwrap();
    let outside = adoc_engine::normalize_path(directory.path());
    let root = outside.join("docs");
    fs::create_dir_all(root.join("themes")).unwrap();
    fs::write(
        outside.join("secret.ttf"),
        adoc_theme::bundled_font("notoserif-regular-subset.ttf").unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("themes/leak-theme.yml"),
        "extends: default\nfont:\n  catalog:\n    Leak:\n      normal: ../../secret.ttf\n\
         base:\n  font_family: Leak\n",
    )
    .unwrap();
    fs::write(root.join("page.adoc"), "= Page\n\nText.\n").unwrap();
    fs::write(
        root.join(".adocfoundry.toml"),
        "[pdf]\ntheme = \"leak\"\nthemes_dir = \"themes\"\n",
    )
    .unwrap();
    let mut settings = Settings::load(None, Some(&root)).unwrap();
    settings.git.enabled = false;
    let mut engine = Engine::new(settings);
    engine.set_file_access(adoc_engine::FileAccess::within([root.clone()]));
    engine.open_workspace(vec![root.clone()]).unwrap();
    assert!(engine.wait_for_workers(WAIT));
    let (summary, _) = export(&mut engine, &root, None).unwrap();
    let entries =
        adoc_docx::testing::assert_valid_package(&fs::read(root.join("page.docx")).unwrap());
    // Named, as the theme names it, and not embedded.
    assert_eq!(font_table(&entries), [("Leak".to_owned(), Vec::new())]);
    assert!(!entries.keys().any(|name| name.starts_with("word/fonts/")));
    assert!(
        summary
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("secret.ttf")),
        "{:?}",
        summary.diagnostics
    );
}
