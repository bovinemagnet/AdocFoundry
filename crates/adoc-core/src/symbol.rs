use std::collections::HashSet;

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
        let mut seen = HashSet::new();
        ids.retain(|id| !id.is_empty() && seen.insert(id.clone()));
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

/// How a build generates section ids: Antora's `idprefix: ''` and `idseparator: '-'`, or
/// plain Asciidoctor's `_` and `_`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdStyle {
    Antora,
    Asciidoctor,
}

impl IdStyle {
    fn id(self, title: &str) -> String {
        match self {
            Self::Antora => canonical_id(title),
            Self::Asciidoctor => asciidoctor_id(title),
        }
    }

    fn separator(self) -> char {
        match self {
            Self::Antora => '-',
            Self::Asciidoctor => '_',
        }
    }
}

/// The id of each section, in order: its anchor, else the id `style` generates, numbered as
/// Asciidoctor numbers a generated id an earlier anchor or section already holds (the second
/// `== Parameters` is `_parameters_2`, or `parameters-2` in Antora).
#[must_use]
pub fn section_ids(sections: &[Section], anchors: &[Anchor], style: IdStyle) -> Vec<String> {
    let mut taken = HashSet::new();
    let mut anchors = anchors.iter().peekable();
    let mut ids = Vec::with_capacity(sections.len());
    for section in sections {
        while let Some(anchor) = anchors.next_if(|anchor| anchor.range.start < section.range.start)
        {
            taken.insert(anchor.id.clone());
        }
        let id = match &section.id {
            Some(id) => id.clone(),
            None => {
                let id = style.id(&section.title);
                if id.is_empty() || !taken.contains(&id) {
                    id
                } else {
                    (2..)
                        .map(|n| format!("{id}{}{n}", style.separator()))
                        .find(|numbered| !taken.contains(numbered))
                        .unwrap_or(id)
                }
            }
        };
        taken.insert(id.clone());
        ids.push(id);
    }
    ids
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
    /// Declared inside an `ifdef`, `ifndef` or `ifeval` block, so it may not be in the
    /// document the build sees.
    pub conditional: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(title: &str, id: Option<&str>, start: usize) -> Section {
        let range = SourceRange::new(start, start + 1);
        Section {
            level: 1,
            title: title.to_owned(),
            id: id.map(str::to_owned),
            range,
            selection_range: range,
        }
    }

    fn anchor(id: &str, start: usize) -> Anchor {
        Anchor {
            id: id.to_owned(),
            range: SourceRange::new(start, start + 1),
            conditional: false,
        }
    }

    #[test]
    fn numbers_repeated_generated_ids() {
        let sections = [
            section("Parameters", None, 0),
            section("Parameters", None, 10),
            section("Parameters", None, 20),
        ];

        assert_eq!(
            section_ids(&sections, &[], IdStyle::Asciidoctor),
            ["_parameters", "_parameters_2", "_parameters_3"]
        );
        assert_eq!(
            section_ids(&sections, &[], IdStyle::Antora),
            ["parameters", "parameters-2", "parameters-3"]
        );
    }

    /// Only ids declared before a section are taken when its id is generated.
    #[test]
    fn counts_anchors_declared_before_the_section() {
        let sections = [
            section("Intro", Some("parameters"), 5),
            section("Parameters", None, 20),
            section("Notes", None, 40),
        ];
        let anchors = [anchor("parameters", 0), anchor("notes", 50)];

        assert_eq!(
            section_ids(&sections, &anchors, IdStyle::Antora),
            ["parameters", "parameters-2", "notes"]
        );
    }

    #[test]
    fn implicit_ids_hold_each_form_once() {
        // Issue #533: `dedup` removed neighbours only.
        assert_eq!(
            section("parameters", None, 0).implicit_ids(),
            ["parameters", "_parameters"]
        );
    }

    #[test]
    fn skips_a_number_already_taken() {
        let sections = [
            section("Parameters", None, 0),
            section("Parameters 2", None, 10),
            section("Parameters", None, 20),
        ];

        assert_eq!(
            section_ids(&sections, &[], IdStyle::Antora),
            ["parameters", "parameters-2", "parameters-3"]
        );
    }
}
