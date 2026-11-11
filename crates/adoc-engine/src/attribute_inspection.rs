//! Inspect Attribute (issue #99): where the value of the attribute at the caret comes from,
//! as rows a front end lists as they are, worded here so every front end says the same.

use std::{fs, path::Path};

use adoc_core::LineIndex;
use adoc_lang::{
    effective::{EffectiveAttribute, Origin, Outcome, Resolution, Step},
    Location,
};

use crate::{report::display_path, DocumentId, Engine};

/// The attribute at a place, as Inspect Attribute shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeInspection {
    pub name: String,
    /// Its value there: `edition is “Enterprise” here`.
    pub summary: String,
    /// The steps in the order they apply, then, for a file pages include, a row per page.
    pub rows: Vec<InspectionRow>,
    /// The resolution the rows were worded from.
    pub detail: EffectiveAttribute,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectionRowKind {
    /// A step that sets or unsets the attribute, and whether it gives the value.
    Step { wins: bool },
    /// The value in a page that includes the file.
    Includer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectionRow {
    pub kind: InspectionRowKind,
    /// What sets it: `Playbook`, `antora.yml`, `This page`, an included file's path, or for
    /// an includer the page's path.
    pub source: String,
    /// The value, `unset`, or `set, empty`.
    pub value: String,
    /// `wins`, `soft set, overridden by this page`, `ignored: antora.yml hard sets it`, …
    pub status: String,
    /// Where it is written, as `path:line` relative to its root.
    pub place: Option<String>,
    /// Where opening the row leads.
    pub location: Option<Location>,
}

/// An attribute's value at one place, and the step that gives it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveValue {
    pub name: String,
    /// None when the attribute is unset there.
    pub value: Option<String>,
    pub origin: Origin,
    /// Where the winning step is written, if in a file.
    pub location: Option<Location>,
}

impl InspectionRow {
    #[must_use]
    pub fn wins(&self) -> bool {
        self.kind == InspectionRowKind::Step { wins: true }
    }
}

impl Engine {
    /// The attribute at `offset` in document `id`: a `{name}` reference, an attribute entry, or
    /// a name in `ifdef::`/`ifndef::`; None when there is none there.
    #[must_use]
    pub fn inspect_attribute(&self, id: DocumentId, offset: usize) -> Option<AttributeInspection> {
        let document = self.document(id)?;
        let path = document.path.clone();
        let detail = adoc_lang::effective::effective_attribute(
            &self.workspace,
            &path,
            document.parsed(),
            offset,
        )?;
        let name = detail.name.clone();
        let summary = match (&detail.resolution.winner, detail.resolution.value()) {
            (None, _) => format!("{name} is not set here"),
            (Some(_), None) => format!("{name} is unset here"),
            (Some(_), Some("")) => format!("{name} is set, with no value, here"),
            (Some(_), Some(value)) => format!("{name} is “{value}” here"),
        };
        let mut rows = self.step_rows(&path, &detail.resolution);
        for includer in &detail.includers {
            let winner = includer.resolution.winning_step();
            rows.push(InspectionRow {
                kind: InspectionRowKind::Includer,
                source: self.shown_path(&includer.page),
                value: winner.map_or_else(|| "not set".to_owned(), value_text),
                status: match winner {
                    Some(step) => match self.place(step) {
                        Some(place) => format!("from {place}"),
                        None => format!("from {}", self.source_label(&includer.page, step)),
                    },
                    None => "not set".to_owned(),
                },
                place: Some(self.shown_path(&includer.page)),
                location: Some(
                    winner
                        .and_then(|step| step.location.clone())
                        .unwrap_or(Location {
                            path: includer.page.clone(),
                            range: adoc_core::SourceRange::new(0, 0),
                        }),
                ),
            });
        }
        Some(AttributeInspection {
            name,
            summary,
            rows,
            detail,
        })
    }

    /// The value at the end of the header of the page at `path` (its first blank line) of
    /// every attribute the playbook, its component or the page itself sets or unsets, by name.
    /// Attributes that are never set by then are left out.
    #[must_use]
    pub fn effective_values(&self, path: &Path) -> Vec<EffectiveValue> {
        let path = adoc_index::normalize_path(path);
        let open = self
            .document_for_path(&path)
            .and_then(|id| self.document(id));
        let Some(document) = open
            .map(|open| open.parsed())
            .or_else(|| self.workspace.index.file(&path).map(|file| &file.document))
        else {
            return Vec::new();
        };
        let Some(text) = self.text_at(&path) else {
            return Vec::new();
        };
        let header_end = text.find("\n\n").map_or(text.len(), |blank| blank + 1);

        let antora = &self.workspace.antora;
        let mut names: std::collections::BTreeSet<String> = document
            .attributes
            .iter()
            .map(|attribute| {
                attribute
                    .name
                    .trim_start_matches('!')
                    .trim_end_matches('!')
                    .to_owned()
            })
            .collect();
        if let Some(context) = antora.context_for_path(&path) {
            names.extend(
                antora
                    .playbook()
                    .into_iter()
                    .flat_map(|playbook| playbook.attribute_entries.keys().cloned()),
            );
            names.extend(
                antora
                    .component(&context.component, context.version.as_deref())
                    .into_iter()
                    .flat_map(|component| component.attribute_entries.keys().cloned()),
            );
        }
        names
            .into_iter()
            .filter_map(|name| {
                let resolution = adoc_lang::effective::resolve(
                    &self.workspace.index,
                    antora,
                    &path,
                    document,
                    &name,
                    header_end,
                );
                let step = resolution.winning_step()?;
                Some(EffectiveValue {
                    value: step.value.clone(),
                    origin: step.origin.clone(),
                    location: step.location.clone(),
                    name,
                })
            })
            .collect()
    }

    fn step_rows(&self, page: &Path, resolution: &Resolution) -> Vec<InspectionRow> {
        resolution
            .steps
            .iter()
            .map(|step| {
                let outcome = match step.outcome {
                    Outcome::Wins => "wins".to_owned(),
                    Outcome::Overridden { by } => format!(
                        "overridden by {}",
                        self.source_label(page, &resolution.steps[by])
                    ),
                    Outcome::Ignored { by } => {
                        let by = &resolution.steps[by];
                        let does = if by.value.is_some() {
                            "hard sets"
                        } else {
                            "hard unsets"
                        };
                        format!("ignored: {} {does} it", self.source_label(page, by))
                    }
                    Outcome::After => "after this point".to_owned(),
                };
                let status = match strength(step) {
                    Some(strength) => format!("{strength}, {outcome}"),
                    None => outcome,
                };
                InspectionRow {
                    kind: InspectionRowKind::Step {
                        wins: step.outcome == Outcome::Wins,
                    },
                    source: match self.source_label(page, step) {
                        label if label == "this page" => "This page".to_owned(),
                        label => label,
                    },
                    value: value_text(step),
                    status,
                    place: self.place(step),
                    location: step.location.clone(),
                }
            })
            .collect()
    }

    /// What sets the step, in lower case where it is a phrase: `this page`, `antora.yml`.
    fn source_label(&self, page: &Path, step: &Step) -> String {
        match step.origin {
            Origin::BuiltIn => "Asciidoctor's default".to_owned(),
            Origin::Playbook => "Playbook".to_owned(),
            Origin::Component => "antora.yml".to_owned(),
            Origin::Antora => "Antora, for this page".to_owned(),
            Origin::Declaration => match &step.location {
                Some(location) if location.path != page => self.shown_path(&location.path),
                _ => "this page".to_owned(),
            },
        }
    }

    /// `path:line` for a step written in a file.
    fn place(&self, step: &Step) -> Option<String> {
        let location = step.location.as_ref()?;
        let text = self.text_at(&location.path)?;
        let (line, _) = LineIndex::new(&text).line_column(location.range.start);
        Some(format!("{}:{}", self.shown_path(&location.path), line + 1))
    }

    /// The text of the file at `path`: its open buffer, else the disk.
    fn text_at(&self, path: &Path) -> Option<String> {
        match self.document_for_path(path) {
            Some(open) => Some(self.document(open)?.text_string()),
            None => fs::read_to_string(path).ok(),
        }
    }

    /// `path` relative to the root it is beneath, with `/` separators.
    fn shown_path(&self, path: &Path) -> String {
        let root = self
            .roots()
            .iter()
            .find(|root| path.starts_with(root))
            .map_or(Path::new(""), |root| root.as_path());
        display_path(path, root)
    }
}

/// `soft set`, `hard unset` and so on, for a step from the playbook or `antora.yml`.
fn strength(step: &Step) -> Option<&'static str> {
    matches!(step.origin, Origin::Playbook | Origin::Component).then(|| {
        match (step.soft, step.value.is_some()) {
            (true, true) => "soft set",
            (true, false) => "soft unset",
            (false, true) => "hard set",
            (false, false) => "hard unset",
        }
    })
}

fn value_text(step: &Step) -> String {
    match step.value.as_deref() {
        None => "unset".to_owned(),
        Some("") => "set, empty".to_owned(),
        Some(value) => value.to_owned(),
    }
}
