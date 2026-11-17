//! Read-only PDFs, READONLY 4/5 (`docs/prd/PRD-AdocFoundry_READONLY.md` §10, tests 19 and 20):
//! the Standard Security Handler written in process, AES-256 with modification denied, read
//! back by an independent reader with no password and nothing of the document lost.

mod common;

use adoc_pdf::{PdfOptions, PdfProtection};

/// The PDF reference article, with and without the restriction.
fn both() -> (Vec<u8>, Vec<u8>) {
    let view = common::view_of(&common::fixtures().join("pdf/reference/article.adoc"));
    let theme = common::default_theme();
    let export = |protection| {
        let output = adoc_pdf::export(
            &common::article(view.clone()),
            &theme,
            &PdfOptions {
                protection,
                ..PdfOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            output.protected,
            protection == PdfProtection::RestrictModification
        );
        output.bytes
    };
    (
        export(PdfProtection::None),
        export(PdfProtection::RestrictModification),
    )
}

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle)
}

#[test]
fn pdf_protected_has_encrypt_dictionary() {
    let (plain, protected) = both();
    assert!(!adoc_pdf::restricts_modification(&plain));
    assert!(adoc_pdf::restricts_modification(&protected));
    // The dictionary itself, as written: AES-256, revision 6, modification denied, the
    // extension level AES-256 needs in a PDF 1.7 file, and the trailer naming it.
    for entry in [
        &b"/Filter/Standard"[..],
        b"/V 5",
        b"/R 6",
        b"/Length 256",
        b"/P -1324",
        b"/CFM/AESV3",
        b"/StmF/StdCF",
        b"/StrF/StdCF",
        b"/EncryptMetadata true",
    ] {
        assert!(
            contains(&protected, entry),
            "{}",
            String::from_utf8_lossy(entry)
        );
    }
    assert!(
        contains(&protected, b"/Encrypt "),
        "the trailer names the dictionary"
    );
    // Nothing readable is left: the title and a link's address are in the plain PDF only.
    let title = b"Exporting Documentation to PDF";
    assert!(contains(&plain, title), "the marker is there to be hidden");
    assert!(!contains(&protected, title));
    assert!(contains(&plain, b"https://") && !contains(&protected, b"https://"));
    // Unprotected output is as it always was: no encryption, the same bytes twice.
    assert!(!contains(&plain, b"/Encrypt"));
    assert_eq!(plain, both().0);
}

#[test]
fn pdf_protected_preserves_links_images_fonts() {
    let (plain, protected) = both();
    // An independent reader opens it with no password and reads every page as written.
    let read = |bytes: &[u8]| {
        let pdf = hayro_syntax::Pdf::new(bytes.to_vec()).expect("opens with no password");
        let pages: Vec<Vec<u8>> = pdf
            .pages()
            .iter()
            .map(|page| page.page_stream().unwrap_or_default().to_vec())
            .collect();
        (pages, pdf.metadata().title.clone())
    };
    let (plain_pages, plain_title) = read(&plain);
    let (pages, title) = read(&protected);
    assert!(!pages.is_empty());
    assert_eq!(pages, plain_pages, "every page's content survives");
    assert_eq!(title, plain_title);
    // The structure: the catalog's tags, outline, names and page labels, each page's links,
    // fonts and images.
    let structure = |bytes: &[u8]| {
        let document = lopdf::Document::load_mem(bytes).unwrap();
        let mut catalog: Vec<String> = document
            .catalog()
            .unwrap()
            .iter()
            .map(|(key, _)| String::from_utf8_lossy(key).into_owned())
            .filter(|key| key != "Extensions")
            .collect();
        catalog.sort();
        let pages: Vec<(usize, usize, usize)> = document
            .get_pages()
            .values()
            .map(|id| {
                let annotations = document
                    .get_page_annotations(*id)
                    .map_or(0, |list| list.len());
                let fonts = document.get_page_fonts(*id).map_or(0, |fonts| fonts.len());
                let images = document
                    .get_page_images(*id)
                    .map_or(0, |images| images.len());
                (annotations, fonts, images)
            })
            .collect();
        (catalog, pages)
    };
    let (plain_catalog, plain_pages) = structure(&plain);
    let (catalog, pages) = structure(&protected);
    assert_eq!(catalog, plain_catalog);
    assert!(
        catalog.contains(&"StructTreeRoot".to_owned()) && catalog.contains(&"Outlines".to_owned())
    );
    assert_eq!(pages, plain_pages);
    assert!(pages.iter().any(|(links, _, _)| *links > 0), "{pages:?}");
}
