use crate::SourceRange;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Section {
    pub level: u8,
    pub title: String,
    pub id: Option<String>,
    pub range: SourceRange,
    pub selection_range: SourceRange,
}

impl Section {
    /// Ids a reference may use to reach this section when it carries no explicit anchor:
    /// the title itself (AsciiDoc natural cross reference), the `idseparator=-` form Antora
    /// generates by default, and Asciidoctor's default `_`-prefixed form.
    #[must_use]
    pub fn implicit_ids(&self) -> Vec<String> {
        let mut ids = vec![
            self.title.clone(),
            canonical_id(&self.title),
            generated_id(&self.title, "_", '_'),
            alphanumeric_id(&self.title),
        ];
        ids.retain(|id| !id.is_empty());
        ids.dedup();
        ids
    }
}

/// A punctuation-insensitive form of a reference id.
///
/// Asciidoctor keeps some punctuation when it generates a section id and drops the rest, and the
/// exact rules shift between versions and `idseparator` settings. Comparing both sides in this
/// form resolves a reference whenever it names the section, whatever the surrounding punctuation.
#[must_use]
pub fn canonical_id(text: &str) -> String {
    generated_id(text, "", '-')
}

/// The id plain Asciidoctor generates for a heading with no explicit anchor.
///
/// Antora's default UI sets `idprefix: ''` and `idseparator: '-'`, which is what
/// `canonical_id` produces; Asciidoctor's own defaults, used whenever a document is
/// rendered outside an Antora module, are `idprefix=_` and `idseparator=_`. Both forms are
/// needed because which one a reference must match depends on how the document is built.
#[must_use]
pub fn asciidoctor_id(text: &str) -> String {
    generated_id(text, "_", '_')
}

/// The last-resort comparison form: letters and digits only.
///
/// Asciidoctor drops some punctuation outright and turns the rest into the separator, so
/// `== Version 0.6.0 (superseded -- room in the key)` becomes
/// `version-0-6-0-supersededroom-in-the-key`. Stripping punctuation from both sides matches the
/// section however its title was punctuated.
#[must_use]
pub fn alphanumeric_id(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn generated_id(title: &str, prefix: &str, separator: char) -> String {
    let mut id = String::from(prefix);
    for character in title.chars() {
        if character.is_alphanumeric() {
            id.extend(character.to_lowercase());
        } else if !id.ends_with(separator) {
            id.push(separator);
        }
    }
    id.trim_end_matches(separator).to_owned()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Anchor {
    pub id: String,
    pub range: SourceRange,
}
