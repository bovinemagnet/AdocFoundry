//! Word templates (the Word looks PRD §5.3): what an export takes from one, and what it refuses
//! or leaves out of one, read as untrusted input.

mod common;

use adoc_docx::{
    testing::{relationships, template, TINY_PNG},
    DocxOptions, Margins, Template, TemplateError, TemplatePage, WordLook,
};
use common::{assert_valid_package, paragraphs, text};

const PAGE: &str = "= Release Notes\n\nThe first paragraph.\n\n== Installing\n\n. One\n. Two\n\n\
                    [source,rust]\n----\nfn main() {}\n----\n";

fn export(
    template: Template,
    options: DocxOptions,
) -> (
    std::collections::BTreeMap<String, Vec<u8>>,
    adoc_docx::DocxReport,
) {
    let options = DocxOptions {
        look: WordLook::from_template(template),
        ..options
    };
    let mut bytes = Vec::new();
    let report = adoc_docx::write_docx(&common::article(PAGE), &options, &mut bytes).unwrap();
    (assert_valid_package(&bytes), report)
}

#[test]
fn a_template_is_read_part_by_part() {
    let read = Template::read(&template(|_| {})).unwrap();
    assert_eq!(read.warnings, Vec::<String>::new());
    assert_eq!(
        read.page(),
        Some(TemplatePage {
            width: 12240,
            height: 15840,
            landscape: false,
            margins: Margins {
                top: 1000,
                right: 1100,
                bottom: 1200,
                left: 1300,
                header: 500,
                footer: 600,
            },
        })
    );
    assert!(read.has_running(true) && read.has_running(false));
}

#[test]
fn an_export_takes_the_templates_styles_numbering_theme_and_running_content() {
    let read = Template::read(&template(|_| {})).unwrap();
    let (entries, report) = export(read, DocxOptions::default());
    assert_eq!(report.diagnostics, Vec::new());

    // Nothing of the template's body.
    let document = text(&entries, "word/document.xml");
    assert!(!document.contains("TEMPLATE BODY TEXT"));
    assert_eq!(
        paragraphs(document)[0],
        (Some("Title".to_owned()), "Release Notes".to_owned())
    );

    // Its styles as it defines them; the export's added where it has none, its own kept once.
    let styles = text(&entries, "word/styles.xml");
    assert!(
        styles.contains(r#"<w:color w:val="1F4E79"/><w:sz w:val="36"/>"#),
        "{styles}"
    );
    assert!(styles.contains(r#"w:styleId="CorporateNote""#));
    assert!(styles.contains(r#"w:styleId="SourceCode""#));
    assert_eq!(styles.matches(r#"w:styleId="Heading1""#).count(), 1);
    assert_eq!(styles.matches(r#"w:styleId="Normal""#).count(), 1);
    assert_eq!(styles.matches(r#"w:default="1""#).count(), 1);

    // Its numbering first, the export's lists after it.
    let numbering = text(&entries, "word/numbering.xml");
    assert!(numbering.contains(r#"<w:lvlText w:val="%1)"/>"#));
    assert!(
        numbering.contains(r#"<w:num w:numId="2"><w:abstractNumId w:val="1"/>"#),
        "{numbering}"
    );
    assert!(document.contains(r#"<w:numId w:val="2"/>"#));

    // Its theme and font table.
    assert!(text(&entries, "word/theme/theme1.xml").contains(r#"name="Corporate""#));
    assert!(text(&entries, "word/fontTable.xml").contains(r#"w:name="Arial""#));

    // Its header and footer, as they are, with the picture and the link they hold.
    let header = text(&entries, "word/header1.xml");
    assert!(header.contains("CORPORATE HEADER"));
    let header_rels = text(&entries, "word/_rels/header1.xml.rels");
    assert!(
        header_rels.contains(r#"Id="rIdLogo""#)
            && header_rels.contains(r#"Target="media/template1.png""#)
    );
    assert_eq!(entries["word/media/template1.png"], TINY_PNG);
    assert!(text(&entries, "word/footer1.xml").contains("GE </w:instrText>"));
    assert!(text(&entries, "word/_rels/footer1.xml.rels").contains("https://example.com/"));
    assert!(document.contains(r#"<w:headerReference w:type="default""#));
    assert!(document.contains(r#"<w:footerReference w:type="default""#));
}

#[test]
fn written_styles_and_running_content_apply_over_the_template() {
    let mut read = Template::read(&template(|_| {})).unwrap();
    read.without_running(false);
    let options = DocxOptions {
        styles: adoc_docx::StyleOverrides {
            heading_colour: Some("C00000".to_owned()),
            body_size: Some(24),
            ..adoc_docx::StyleOverrides::default()
        },
        ..DocxOptions::default()
    };
    let (entries, _) = export(read, options);
    let styles = text(&entries, "word/styles.xml");
    // The template's own heading, its colour changed and the rest kept.
    assert!(
        styles.contains(r#"<w:rPr><w:b/><w:color w:val="C00000"/><w:sz w:val="36"/></w:rPr>"#),
        "{styles}"
    );
    assert!(
        styles.contains(r#"<w:rFonts w:ascii="Arial" w:hAnsi="Arial"/><w:sz w:val="24"/>"#),
        "{styles}"
    );
    // The footer is the export's: the page number, not the template's link.
    let footers: Vec<&String> = entries
        .keys()
        .filter(|name| name.starts_with("word/footer"))
        .collect();
    assert!(!footers.is_empty());
    for footer in footers {
        assert!(!text(&entries, footer).contains("example.com"));
    }
    assert!(text(&entries, "word/header1.xml").contains("CORPORATE HEADER"));
}

#[test]
fn a_localised_templates_styles_are_used_by_their_names() {
    let read = Template::read(&template(|entries| {
        let styles = String::from_utf8(entries["word/styles.xml"].clone())
            .unwrap()
            .replace(r#"w:styleId="Heading1""#, r#"w:styleId="berschrift1""#)
            .replace(r#"w:styleId="Normal""#, r#"w:styleId="Standard""#)
            .replace(
                r#"<w:basedOn w:val="Normal"/>"#,
                r#"<w:basedOn w:val="Standard"/>"#,
            )
            .replace(
                r#"<w:next w:val="Normal"/>"#,
                r#"<w:next w:val="Standard"/>"#,
            );
        entries.insert("word/styles.xml".to_owned(), styles.into_bytes());
    }))
    .unwrap();
    let (entries, _) = export(read, DocxOptions::default());
    let document = text(&entries, "word/document.xml");
    assert!(document.contains(r#"<w:pStyle w:val="berschrift1"/>"#));
    assert!(!document.contains(r#"<w:pStyle w:val="Heading1"/>"#));
    let styles = text(&entries, "word/styles.xml");
    assert!(!styles.contains(r#"w:styleId="Heading1""#));
    assert!(!styles.contains(r#"w:styleId="Normal""#));
}

fn read_with(
    change: impl FnOnce(&mut std::collections::BTreeMap<String, Vec<u8>>),
) -> Result<Template, TemplateError> {
    Template::read(&template(change))
}

#[test]
fn templates_that_can_hold_macros_are_refused() {
    let macro_enabled = read_with(|entries| {
        let types = String::from_utf8(entries["[Content_Types].xml"].clone())
            .unwrap()
            .replace(
                "officedocument.wordprocessingml.template.main+xml",
                "ms-word.template.macroEnabledTemplate.main+xml",
            );
        entries.insert("[Content_Types].xml".to_owned(), types.into_bytes());
    });
    assert_eq!(macro_enabled, Err(TemplateError::Macros));
    // A macro project, however it is named and whatever the package says it is.
    let project = read_with(|entries| {
        entries.insert("word/VBAProject.bin".to_owned(), vec![0; 16]);
    });
    assert_eq!(project, Err(TemplateError::Macros));
}

#[test]
fn templates_too_large_damaged_or_not_word_are_refused() {
    assert!(matches!(
        Template::read(&vec![0; adoc_docx::MAX_TEMPLATE_BYTES + 1]),
        Err(TemplateError::TooLarge(_))
    ));
    assert!(matches!(
        Template::read(b"PK nonsense"),
        Err(TemplateError::Unreadable(_))
    ));
    let malformed = read_with(|entries| {
        entries.insert(
            "word/styles.xml".to_owned(),
            b"<w:styles><unclosed>".to_vec(),
        );
    });
    assert!(
        matches!(malformed, Err(TemplateError::Malformed { part, .. }) if part == "word/styles.xml")
    );
    // A DTD, as an entity expansion would need, is not XML a template's parts may hold.
    let dtd = read_with(|entries| {
        let styles = String::from_utf8(entries["word/styles.xml"].clone())
            .unwrap()
            .replace(
                r#"<?xml version="1.0"?>"#,
                r#"<?xml version="1.0"?><!DOCTYPE w:styles [<!ENTITY a "aaaaaaaaaa">]>"#,
            );
        entries.insert("word/styles.xml".to_owned(), styles.into_bytes());
    });
    assert!(matches!(dtd, Err(TemplateError::Malformed { .. })));
    let spreadsheet = read_with(|entries| {
        let types = String::from_utf8(entries["[Content_Types].xml"].clone())
            .unwrap()
            .replace(
                "application/vnd.openxmlformats-officedocument.wordprocessingml.template.main+xml",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
            );
        entries.insert("[Content_Types].xml".to_owned(), types.into_bytes());
    });
    assert!(matches!(spreadsheet, Err(TemplateError::NotWord(_))));
}

#[test]
fn headers_and_footers_that_could_reach_outside_are_left_out() {
    // A linked picture, fetched on opening.
    let linked = read_with(|entries| {
        entries.insert(
            "word/_rels/header1.xml.rels".to_owned(),
            relationships(&[(
                "rIdLogo",
                "image",
                "https://tracker.example/pixel.png",
                true,
            )])
            .into_bytes(),
        );
    })
    .unwrap();
    assert!(!linked.has_running(true));
    assert!(linked.has_running(false));
    assert!(
        linked.warnings[0].contains("https://tracker.example/pixel.png"),
        "{:?}",
        linked.warnings
    );

    // A field that includes a file, its instruction split across runs.
    let including = read_with(|entries| {
        let footer = String::from_utf8(entries["word/footer1.xml"].clone())
            .unwrap()
            .replace("> PA<", "> INCLUDEPIC<")
            .replace(">GE <", ">TURE \"C:\\\\secret.png\" <");
        entries.insert("word/footer1.xml".to_owned(), footer.into_bytes());
    })
    .unwrap();
    assert!(!including.has_running(false));
    assert!(
        including.warnings[0].contains("INCLUDEPICTURE"),
        "{:?}",
        including.warnings
    );

    // A simple field that runs another program.
    let dde = read_with(|entries| {
        let header = String::from_utf8(entries["word/header1.xml"].clone())
            .unwrap()
            .replace(
                "<w:t>CORPORATE HEADER</w:t></w:r>",
                r#"<w:t>CORPORATE HEADER</w:t></w:r><w:fldSimple w:instr=" DDEAUTO cmd /c calc "/>"#,
            );
        entries.insert("word/header1.xml".to_owned(), header.into_bytes());
    })
    .unwrap();
    assert!(!dde.has_running(true));
    assert!(dde.warnings[0].contains("DDEAUTO"));

    // An altChunk, or anything else but pictures and links.
    let chunk = read_with(|entries| {
        entries.insert(
            "word/_rels/header1.xml.rels".to_owned(),
            relationships(&[
                ("rIdLogo", "image", "media/logo.png", false),
                ("rIdChunk", "aFChunk", "chunk.html", false),
            ])
            .into_bytes(),
        );
        entries.insert("word/chunk.html".to_owned(), b"<p>hi</p>".to_vec());
    })
    .unwrap();
    assert!(!chunk.has_running(true));

    // A theme that refers to other parts.
    let theme = read_with(|entries| {
        entries.insert(
            "word/theme/_rels/theme1.xml.rels".to_owned(),
            relationships(&[("rId1", "image", "../media/logo.png", false)]).into_bytes(),
        );
    })
    .unwrap();
    let (entries, report) = export(theme, DocxOptions::default());
    assert!(!entries.contains_key("word/theme/theme1.xml"));
    // What is left out is reported with the export.
    assert!(
        report.diagnostics[0].message.contains("theme"),
        "{:?}",
        report.diagnostics
    );

    // What is left out never reaches the export.
    let (entries, _) = export(linked, DocxOptions::default());
    for (name, data) in &entries {
        let data = String::from_utf8_lossy(data);
        assert!(!data.contains("tracker.example"), "{name}");
    }
}

fn replaced(
    entries: &mut std::collections::BTreeMap<String, Vec<u8>>,
    name: &str,
    from: &str,
    to: &str,
) {
    let text = String::from_utf8(entries[name].clone()).unwrap();
    assert!(text.contains(from), "{name} has no {from}");
    entries.insert(name.to_owned(), text.replace(from, to).into_bytes());
}

#[test]
fn a_templates_header_and_the_exports_footer_share_the_pages() {
    // The template's header (one part, for every page) with the export's footer, which
    // differs on odd and even pages: the header must show on even pages too.
    let mut read = Template::read(&template(|_| {})).unwrap();
    read.without_running(false);
    let (entries, _) = export(read, DocxOptions::default());
    let document = text(&entries, "word/document.xml");
    assert!(text(&entries, "word/settings.xml").contains("<w:evenAndOddHeaders/>"));
    let header = |kind: &str| {
        document
            .split("<w:headerReference ")
            .skip(1)
            .find(|reference| reference.starts_with(&format!(r#"w:type="{kind}""#)))
            .map(|reference| reference.split('"').nth(3).unwrap().to_owned())
    };
    assert_eq!(header("even"), header("default"), "{document}");
    assert!(header("even").is_some());
}

#[test]
fn a_header_using_a_link_to_fetch_or_naming_an_address_is_left_out() {
    // A link's id used as a VML picture's source.
    let fetching = read_with(|entries| {
        entries.insert(
            "word/_rels/header1.xml.rels".to_owned(),
            relationships(&[
                ("rIdLogo", "image", "media/logo.png", false),
                (
                    "rIdTrack",
                    "hyperlink",
                    "https://tracker.example/pixel.png",
                    true,
                ),
            ])
            .into_bytes(),
        );
        replaced(
            entries,
            "word/header1.xml",
            r#"r:id="rIdLogo""#,
            r#"r:id="rIdLogo" r:href="rIdTrack""#,
        );
    })
    .unwrap();
    assert!(!fetching.has_running(true));
    assert!(
        fetching.warnings[0].contains("rIdTrack"),
        "{:?}",
        fetching.warnings
    );
    // An address written straight into an attribute.
    let direct = read_with(|entries| {
        replaced(
            entries,
            "word/header1.xml",
            r#"o:title="""#,
            r#"o:title="" src="https://tracker.example/x.png""#,
        );
    })
    .unwrap();
    assert!(!direct.has_running(true));
    // A link used as a link is kept.
    assert!(Template::read(&template(|_| {}))
        .unwrap()
        .has_running(false));
}

#[test]
fn svg_pictures_are_kept_only_when_they_load_nothing() {
    let with_svg = |svg: &str| {
        let svg = svg.to_owned();
        read_with(move |entries| {
            replaced(entries, "[Content_Types].xml", r#"<Default Extension="png" ContentType="image/png"/>"#, r#"<Default Extension="png" ContentType="image/png"/><Default Extension="svg" ContentType="image/svg+xml"/>"#);
            entries.insert(
                "word/_rels/header1.xml.rels".to_owned(),
                relationships(&[("rIdLogo", "image", "media/logo.svg", false)]).into_bytes(),
            );
            entries.insert("word/media/logo.svg".to_owned(), svg.into_bytes());
        })
        .unwrap()
    };
    let closed = with_svg(
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><defs><circle id="c" r="2"/></defs><use xlink:href="#c"/></svg>"##,
    );
    assert!(closed.has_running(true), "{:?}", closed.warnings);
    let (entries, _) = export(closed, DocxOptions::default());
    assert!(entries.contains_key("word/media/template1.svg"));
    for open in [
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><image xlink:href="https://tracker.example/x.png"/></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><style>@import url(https://tracker.example/a.css);</style></svg>"#,
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect fill="url(https://tracker.example/#p)"/></svg>"#,
    ] {
        assert!(!with_svg(open).has_running(true), "{open}");
    }
}

#[test]
fn picture_bullets_are_kept_with_their_pictures() {
    let bullets = |target: &str, external: bool| {
        let target = target.to_owned();
        read_with(move |entries| {
            replaced(
                entries,
                "word/numbering.xml",
                r#"<w:abstractNum w:abstractNumId="0">"#,
                r#"<w:numPicBullet w:numPicBulletId="0"><w:pict><v:shape style="width:9pt;height:9pt"><v:imagedata r:id="rIdBullet"/></v:shape></w:pict></w:numPicBullet><w:abstractNum w:abstractNumId="0">"#,
            );
            entries.insert(
                "word/_rels/numbering.xml.rels".to_owned(),
                relationships(&[("rIdBullet", "image", &target, external)]).into_bytes(),
            );
        })
        .unwrap()
    };
    let (entries, report) = export(bullets("media/logo.png", false), DocxOptions::default());
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert!(text(&entries, "word/_rels/numbering.xml.rels").contains("media/template1.png"));
    assert_eq!(entries["word/media/template1.png"], TINY_PNG);
    // A bullet fetched from outside: the template's numbering is left out, the export's kept.
    let fetched = bullets("https://tracker.example/b.png", true);
    assert!(
        fetched.warnings[0].contains("numbering"),
        "{:?}",
        fetched.warnings
    );
    let (entries, _) = export(fetched, DocxOptions::default());
    let numbering = text(&entries, "word/numbering.xml");
    assert!(!numbering.contains("numPicBullet") && !numbering.contains("%1)"));
    assert!(numbering.contains(r#"<w:num w:numId="1">"#));
}

#[test]
fn headers_inherited_from_an_earlier_section_are_found() {
    let read = read_with(|entries| {
        replaced(
            entries,
            "word/document.xml",
            r#"<w:p><w:r><w:t>TEMPLATE BODY TEXT</w:t></w:r></w:p><w:sectPr><w:headerReference w:type="default" r:id="rId6"/><w:footerReference w:type="default" r:id="rId7"/>"#,
            r#"<w:p><w:pPr><w:sectPr><w:headerReference w:type="default" r:id="rId6"/><w:footerReference w:type="default" r:id="rId7"/></w:sectPr></w:pPr><w:r><w:t>Cover</w:t></w:r></w:p><w:sectPr>"#,
        );
    })
    .unwrap();
    assert!(read.has_running(true) && read.has_running(false));
}

#[test]
fn naming_fonts_only_leaves_a_templates_embedded_fonts_out() {
    let read = read_with(|entries| {
        replaced(
            entries,
            "word/fontTable.xml",
            r#"<w:font w:name="Arial"/>"#,
            r#"<w:font w:name="Brand"><w:embedRegular r:id="rIdF1" w:fontKey="{00000000-0000-0000-0000-000000000000}"/></w:font>"#,
        );
        entries.insert(
            "word/_rels/fontTable.xml.rels".to_owned(),
            relationships(&[("rIdF1", "font", "fonts/font1.odttf", false)]).into_bytes(),
        );
        entries.insert("word/fonts/font1.odttf".to_owned(), vec![0; 64]);
    })
    .unwrap();
    let (entries, _) = export(
        read,
        DocxOptions {
            fonts_named_only: true,
            ..DocxOptions::default()
        },
    );
    let table = text(&entries, "word/fontTable.xml");
    assert!(
        table.contains(r#"<w:font w:name="Brand"></w:font>"#),
        "{table}"
    );
    assert!(!entries.keys().any(|name| name.starts_with("word/fonts/")));
    assert!(!text(&entries, "word/settings.xml").contains("embedTrueTypeFonts"));
}

#[test]
fn the_same_template_gives_the_same_bytes() {
    let write = || {
        let options = DocxOptions {
            look: WordLook::from_template(Template::read(&template(|_| {})).unwrap()),
            ..DocxOptions::default()
        };
        let mut bytes = Vec::new();
        adoc_docx::write_docx(&common::article(PAGE), &options, &mut bytes).unwrap();
        bytes
    };
    assert_eq!(write(), write());
}
