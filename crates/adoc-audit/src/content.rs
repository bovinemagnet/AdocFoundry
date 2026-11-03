//! Page content: `quality.page.missing-title` and `quality.image.missing-alt`.
//!
//! Titles are checked on published pages (an `.adoc` file whose name does not begin with `_` or
//! `.`). An empty `=` line is reported; no title line at all
//! is reported unless a `doctitle` attribute gives one, and skipped when an include before the
//! first section may supply it.
//!
//! Alternative text is checked on the block image macros (`image::`) of published pages and of
//! partials, which pages include. Inline images are not checked: the parser records block
//! images only. Text counts as the author's whenever it is written: a first positional
//! attribute (`image::a.png[What it shows]`), a named `alt`, an attribute reference, or text
//! explicitly quoted empty (`[""]`), the author's statement that an image is decorative. Only a
//! macro with no text at all, whose alternative text Asciidoctor would make from the file name,
//! is reported. A decorative image without quotes, or a generated one, is left to an exclusion.

use adoc_antora::ResourceFamily;
use adoc_core::{Document, ImageDirective};

use std::path::Path;

use crate::{
    workspace::published_page, workspace::Context, FileAudit, Finding, FindingCode, RuleId, Skip,
};

/// The content rules' findings and skips for the indexed file at `path`.
pub(crate) fn file(context: &Context<'_>, path: &Path, document: &Document, part: &mut FileAudit) {
    let Some(resource) = context.workspace().antora.resource_for_path(path) else {
        return;
    };
    let coordinate = &resource.coordinate;
    match coordinate.family {
        ResourceFamily::Page if published_page(&coordinate.relative_path) => {
            title(context, path, document, part);
            images(context, path, document, part);
        }
        ResourceFamily::Partial => images(context, path, document, part),
        ResourceFamily::Page
        | ResourceFamily::Example
        | ResourceFamily::Image
        | ResourceFamily::Attachment => {}
    }
}

fn title(context: &Context<'_>, path: &Path, document: &Document, part: &mut FileAudit) {
    let finding = |message: &str, range| {
        let mut finding = Finding::new(
            FindingCode::Rule(RuleId::MISSING_TITLE),
            RuleId::MISSING_TITLE.spec().default_severity,
            message,
            context.location(path, range),
        );
        finding.coordinate = context.coordinate(path);
        finding.remediation =
            Some("Give the page a title on its first line: `= What the page is about`".to_owned());
        finding
    };
    if let Some(title) = &document.title {
        if title.text.trim().is_empty() {
            part.findings.push(finding(
                "The page's document title is empty",
                Some(title.range),
            ));
        }
        return;
    }
    let titled_by_attribute = document.attributes.iter().any(|attribute| {
        attribute.name == "doctitle"
            && attribute
                .value
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    });
    if titled_by_attribute {
        return;
    }
    let first_section = document.sections.first().map(|section| section.range.start);
    let header_include = document
        .includes
        .iter()
        .find(|include| first_section.is_none_or(|start| include.range.start < start));
    match header_include {
        Some(include) => part.skips.push(Skip {
            location: context.location(path, Some(include.range)),
            code: FindingCode::Rule(RuleId::MISSING_TITLE),
            reason: format!("The title may come from `{}`", include.target),
        }),
        None => part
            .findings
            .push(finding("The page has no document title", None)),
    }
}

fn images(context: &Context<'_>, path: &Path, document: &Document, part: &mut FileAudit) {
    for image in &document.images {
        if has_alt(image) {
            continue;
        }
        let mut finding = Finding::new(
            FindingCode::Rule(RuleId::MISSING_ALT),
            RuleId::MISSING_ALT.spec().default_severity,
            format!("The image `{}` has no alternative text", image.target),
            context.location(path, Some(image.range)),
        );
        finding.coordinate = context.coordinate(path);
        finding.remediation = Some(format!(
            "Describe the image in the macro's first attribute: `image::{}[What it shows]`",
            image.target
        ));
        part.findings.push(finding);
    }
}

/// Whether the macro's attributes give alternative text, in any form the author wrote.
fn has_alt(image: &ImageDirective) -> bool {
    let attributes = image.attributes.as_deref().unwrap_or("");
    let items = split_attributes(attributes);
    let positional = items
        .first()
        .is_some_and(|first| first.starts_with('"') || (!first.is_empty() && !is_named(first)));
    let named = items.iter().any(|item| {
        item.split_once('=').is_some_and(|(name, value)| {
            name.trim() == "alt" && {
                let value = value.trim();
                value.starts_with('"') || !value.is_empty()
            }
        })
    });
    positional || named
}

/// Whether an attribute list item is `name=value` rather than a positional value.
fn is_named(item: &str) -> bool {
    item.split_once('=').is_some_and(|(name, _)| {
        let name = name.trim();
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    })
}

/// The comma-separated items of an attribute list, trimmed, with commas inside double quotes
/// kept in their item.
fn split_attributes(attributes: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    for (offset, character) in attributes.char_indices() {
        match character {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                items.push(attributes[start..offset].trim());
                start = offset + 1;
            }
            _ => {}
        }
    }
    items.push(attributes[start..].trim());
    items
}

#[cfg(test)]
mod tests {
    use adoc_core::SourceRange;

    use super::*;

    fn image(attributes: Option<&str>) -> ImageDirective {
        ImageDirective {
            target: "a.png".to_owned(),
            attributes: attributes.map(str::to_owned),
            range: SourceRange::new(0, 0),
        }
    }

    #[test]
    fn alternative_text_in_every_written_form_counts() {
        for written in [
            "A sunset",
            "A sunset, 300, 200",
            "\"A sunset, over the sea\", 300",
            "{sunset-alt}",
            "\"\"",
            "alt=\"Described by name\"",
            "width=300, alt=Described",
            "alt=\"\"",
        ] {
            assert!(has_alt(&image(Some(written))), "{written:?}");
        }
    }

    #[test]
    fn a_macro_with_no_text_has_none() {
        for unwritten in [
            None,
            Some(""),
            Some("  "),
            Some("width=20"),
            Some("width=20,alt="),
        ] {
            assert!(!has_alt(&image(unwritten)), "{unwritten:?}");
        }
    }
}
