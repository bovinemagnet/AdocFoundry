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
    // Every string, decrypted by the independent reader: link addresses, outline titles, names,
    // the information dictionary; equal to the plain PDF's.
    let (plain_strings, strings) = (strings(&plain), strings(&protected));
    assert!(plain_strings.iter().any(|(key, _)| key == "URI"));
    assert!(plain_strings.iter().any(|(key, _)| key == "Title"));
    assert_eq!(strings, plain_strings);
}

/// Each string in the document under the key that holds it, sorted, as an independent reader
/// (`hayro-syntax`, not the library that encrypted it) decrypts it; the encryption
/// dictionary's own keys left out.
fn strings(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    use hayro_syntax::object::{MaybeRef, Object};
    fn walk(key: &str, object: &Object<'_>, found: &mut Vec<(String, Vec<u8>)>) {
        let entries = |dict: &hayro_syntax::object::Dict<'_>, found: &mut Vec<_>| {
            for (name, value) in dict.entries() {
                let name = String::from_utf8_lossy(name.as_ref()).into_owned();
                if ["O", "U", "OE", "UE", "Perms"].contains(&name.as_str()) {
                    continue;
                }
                if let MaybeRef::NotRef(value) = value {
                    walk(&name, &value, found);
                }
            }
        };
        match object {
            Object::String(text) => found.push((key.to_owned(), text.as_bytes().to_vec())),
            Object::Array(items) => {
                for item in items.raw_iter() {
                    if let MaybeRef::NotRef(item) = item {
                        walk(key, &item, found);
                    }
                }
            }
            Object::Dict(dict) => entries(dict, found),
            Object::Stream(stream) => entries(stream.dict(), found),
            _ => {}
        }
    }
    let pdf = hayro_syntax::Pdf::new(bytes.to_vec()).expect("opens with no password");
    let mut found = Vec::new();
    for object in pdf.objects() {
        walk("", &object, &mut found);
    }
    found.sort();
    found
}

/// PDF/UA-1 lets an encrypted file be conformant when assistive technology may still read it
/// (`/P` bit 10). A protected PDF/UA-1 article is checked by veraPDF when it is on `PATH`
/// (not on CI's test runners, where only the export is checked; CI's veraPDF job is to check a
/// protected article once PDF protection is switched on). veraPDF writes no executable, so the
/// spawn needs no guard.
#[test]
fn pdfua_with_protection_checked() {
    let view = common::view_of(&common::fixtures().join("pdf/reference/article.adoc"));
    let output = adoc_pdf::export(
        &common::article(view),
        &common::default_theme(),
        &PdfOptions {
            standards: vec![adoc_pdf::PdfStandard::Ua1],
            protection: PdfProtection::RestrictModification,
            ..PdfOptions::default()
        },
    )
    .unwrap();
    assert!(output.protected);
    let Ok(found) = std::process::Command::new("verapdf")
        .arg("--version")
        .output()
    else {
        eprintln!("skipped: veraPDF is not on PATH");
        return;
    };
    assert!(found.status.success());
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("article-ua-protected.pdf");
    std::fs::write(&path, &output.bytes).unwrap();
    let result = std::process::Command::new("verapdf")
        .args(["--flavour", "ua1", "--format", "text"])
        .arg(&path)
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&result.stdout);
    assert!(report.starts_with("PASS"), "{report}");
}
