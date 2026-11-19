//! A book's metadata: the export's own where it gives them, else the document's attributes.
//! Nothing comes from the clock, the file system or the paths of the pages, so the same input
//! always gives the same metadata.

use std::collections::BTreeMap;

use adoc_render::ViewInline;
use sha2::{Digest, Sha256};

use crate::book::Metadata;
use crate::{Document, Options};

pub(crate) fn metadata(document: &Document, options: &Options) -> Metadata {
    let (attributes, title, subtitle): (&BTreeMap<String, String>, String, Option<String>) =
        match document {
            Document::Article(page) => (
                &page.view.attributes,
                page.view
                    .title
                    .as_deref()
                    .map(ViewInline::plain_text)
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| {
                        page.view
                            .source
                            .file_stem()
                            .map(|stem| stem.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    }),
                None,
            ),
            Document::Book(book) => (&book.attributes, book.title.clone(), book.subtitle.clone()),
        };
    let attribute = |name: &str| {
        attributes
            .get(name)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    let title = options
        .title
        .clone()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(title);
    let title = if title.trim().is_empty() {
        "Untitled".to_owned()
    } else {
        title.trim().to_owned()
    };
    let authors: Vec<String> = match &options.creator {
        Some(creator) if !creator.trim().is_empty() => vec![creator.trim().to_owned()],
        _ => attribute("authors")
            .map(|authors| split_authors(&authors))
            .or_else(|| attribute("author").map(|author| vec![author]))
            .unwrap_or_default(),
    };
    let language = options
        .language
        .clone()
        .or_else(|| attribute("lang"))
        .filter(|language| valid_language(language))
        .unwrap_or_else(|| "en".to_owned());
    let version = attribute("revnumber");
    let identifier = options
        .identifier
        .clone()
        .filter(|identifier| !identifier.trim().is_empty())
        .unwrap_or_else(|| derived_identifier(&title, &authors, version.as_deref()));
    Metadata {
        title,
        subtitle,
        authors,
        language,
        identifier,
        publisher: options.publisher.clone().or_else(|| attribute("publisher")),
        description: attribute("description"),
        date: attribute("revdate"),
        version,
    }
}

/// `authors` as Asciidoctor reads it: names separated by semicolons.
fn split_authors(authors: &str) -> Vec<String> {
    authors
        .split(';')
        .map(str::trim)
        .filter(|author| !author.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether `tag` looks like a BCP 47 language tag: letters, digits and hyphens, beginning with
/// two or three letters.
fn valid_language(tag: &str) -> bool {
    let primary = tag.split('-').next().unwrap_or("");
    (2..=3).contains(&primary.len())
        && primary.chars().all(|c| c.is_ascii_alphabetic())
        && tag.len() <= 35
        && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A `urn:uuid:` from the title, authors and version, in the form of a name-based UUID: the
/// same book always has the same identifier, and another book another.
fn derived_identifier(title: &str, authors: &[String], version: Option<&str>) -> String {
    let mut hash = Sha256::new();
    hash.update(b"adocfoundry-kindle\0");
    hash.update(title.as_bytes());
    for author in authors {
        hash.update(b"\0");
        hash.update(author.as_bytes());
    }
    hash.update(b"\0");
    hash.update(version.unwrap_or("").as_bytes());
    let digest = hash.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Version 5 and the RFC 4122 variant, as a name-based UUID has.
    bytes[6] = (bytes[6] & 0x0F) | 0x50;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_stable_and_distinct() {
        let a = derived_identifier("Guide", &["Ann".to_owned()], Some("1.0"));
        assert_eq!(
            a,
            derived_identifier("Guide", &["Ann".to_owned()], Some("1.0"))
        );
        assert_ne!(
            a,
            derived_identifier("Guide", &["Ann".to_owned()], Some("2.0"))
        );
        assert!(a.starts_with("urn:uuid:"), "{a}");
        assert_eq!(a.len(), "urn:uuid:".len() + 36);
    }

    #[test]
    fn languages_are_checked() {
        assert!(valid_language("en"));
        assert!(valid_language("en-GB"));
        assert!(valid_language("zh-Hant-TW"));
        assert!(!valid_language("english"));
        assert!(!valid_language("e"));
        assert!(!valid_language("en GB"));
    }

    #[test]
    fn authors_split_on_semicolons() {
        assert_eq!(split_authors("Ann; Bo ;"), ["Ann", "Bo"]);
    }
}
