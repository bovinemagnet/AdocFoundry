//! The effective value of an attribute at a place in a page, and every place that sets it.
//!
//! Antora's rules: the playbook, then the component version's `antora.yml`, then the page. A
//! value in either YAML file is hard set unless it ends in `@` (soft set); `false` soft unsets
//! and `~` hard unsets. A scope may change an attribute only when what reaches it from above is
//! soft, or absent. Within the page, declarations apply in document order, an include's at the
//! place of its `include::` line, and the value at a place is the last that applies before it.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use adoc_antora::{AntoraCatalog, AntoraContext, AttributeEntry};
use adoc_core::{Document, Location, SourceRange};
use adoc_index::{normalize_path, WorkspaceIndex};

use crate::{
    attributes::{antora_attributes, BUILT_IN_ATTRIBUTES},
    hover::attribute_reference_at,
    includes::{resolve_in_page, MAX_INCLUDE_DEPTH},
    references::includes_of,
    WorkspaceState,
};

/// Where a step sets or unsets the attribute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Origin {
    /// Asciidoctor's default, which any scope may change.
    BuiltIn,
    /// `asciidoc.attributes` in the site's `antora-playbook.yml`.
    Playbook,
    /// `asciidoc.attributes` in the component version's `antora.yml`.
    Component,
    /// An attribute Antora computes for every page.
    Antora,
    /// An attribute entry (`:name: value`) in the page or in a file it includes.
    Declaration,
}

/// What became of a step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// It gives the value at the place asked about.
    Wins,
    /// A later step changed it; `by` indexes [`EffectiveAttribute::steps`].
    Overridden { by: usize },
    /// It has no effect, because the step `by` hard sets or hard unsets the attribute.
    Ignored { by: usize },
    /// It comes after the place asked about.
    After,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Step {
    pub origin: Origin,
    /// Where it is written: the YAML key, or the attribute entry. None for a built-in or an
    /// attribute Antora computes.
    pub location: Option<Location>,
    /// The value it sets, `@` removed; None when it unsets the attribute. For a built-in, what
    /// the attribute stands for.
    pub value: Option<String>,
    /// For the playbook and `antora.yml`: soft set or soft unset, so a later scope may change it.
    pub soft: bool,
    /// For a declaration in an included file: the `include::` in the page it comes through.
    pub via: Option<Location>,
    pub outcome: Outcome,
}

/// The value of an attribute at one place.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Resolution {
    /// The steps in the order they apply: built-in, playbook, component, Antora, then the
    /// declarations in document order.
    pub steps: Vec<Step>,
    /// The step that wins, if any sets or unsets the attribute before the place.
    pub winner: Option<usize>,
}

impl Resolution {
    /// The value at the place: None when the attribute is unset or never set.
    #[must_use]
    pub fn value(&self) -> Option<&str> {
        self.winner
            .and_then(|winner| self.steps[winner].value.as_deref())
    }

    #[must_use]
    pub fn winning_step(&self) -> Option<&Step> {
        self.winner.map(|winner| &self.steps[winner])
    }
}

/// The value at the place asked about in one page that includes the file inspected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncluderValue {
    /// The page, which nothing includes.
    pub page: PathBuf,
    pub resolution: Resolution,
}

/// The attribute at a place: its name, the steps that set it and, in a partial, its value in
/// each page that includes the partial.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveAttribute {
    pub name: String,
    /// The name, reference or entry at the place asked about.
    pub range: SourceRange,
    /// Read with the file as a page in its own right.
    pub resolution: Resolution,
    /// For a file other pages include: the value in each, by page path.
    pub includers: Vec<IncluderValue>,
}

/// The attribute named at `offset` in `text`: a `{name}` reference, an attribute entry
/// (`:name: value`, `:name!:`, `:!name:`), or a name in `ifdef::` or `ifndef::`.
#[must_use]
pub fn attribute_name_at(text: &str, offset: usize) -> Option<(&str, SourceRange)> {
    if let Some(found) = attribute_reference_at(text, offset) {
        return Some(found);
    }
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let line_start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
    let line_end = text[offset..]
        .find('\n')
        .map_or(text.len(), |newline| offset + newline);
    let line = text[line_start..line_end].trim_end_matches('\r');

    // An attribute entry: the caret anywhere on its line.
    if let Some(rest) = line.strip_prefix(':') {
        let end = rest.find(':')?;
        let written = &rest[..end];
        let name = written.trim_start_matches('!').trim_end_matches('!');
        if is_name(name) {
            let start = line_start + 1 + (written.len() - written.trim_start_matches('!').len());
            return Some((name, SourceRange::new(start, start + name.len())));
        }
        return None;
    }

    // A name in `ifdef::a,b[]` or `ifndef::a+b[]`.
    let directive = ["ifdef::", "ifndef::"]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix).map(|_| prefix.len()))?;
    let names_end = line.find('[')?;
    let mut start = line_start + directive;
    for name in line[directive..names_end].split([',', '+']) {
        let end = start + name.len();
        if (start..=end).contains(&offset) && is_name(name) {
            return Some((name, SourceRange::new(start, end)));
        }
        start = end + 1;
    }
    None
}

fn is_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|character| character.is_alphanumeric() || character == '_' || character == '-')
}

/// The attribute at `offset` in the file at `path` (`document` its current parse): its value
/// there and every step, and for a file other pages include, its value in each of them.
#[must_use]
pub fn effective_attribute(
    ws: &WorkspaceState,
    path: &Path,
    document: &Document,
    offset: usize,
) -> Option<EffectiveAttribute> {
    let (name, range) = attribute_name_at(&document.text, offset)?;
    let resolution = resolve(&ws.index, &ws.antora, path, document, name, offset);
    let includers = top_pages(ws, path)
        .into_iter()
        .filter_map(|page| {
            let file = ws.index.file(&page)?;
            let resolution = resolve_through(
                &ws.index,
                &ws.antora,
                &page,
                &file.document,
                name,
                (path, offset),
            )?;
            Some(IncluderValue { page, resolution })
        })
        .collect();
    Some(EffectiveAttribute {
        name: name.to_owned(),
        range,
        resolution,
        includers,
    })
}

/// The attribute `name` at `offset` in the page at `path`, `document` its current parse.
#[must_use]
pub fn resolve(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    path: &Path,
    document: &Document,
    name: &str,
    offset: usize,
) -> Resolution {
    resolve_through(index, antora, path, document, name, (path, offset)).unwrap_or_default()
}

/// The attribute `name` at the place `stop` (a file and an offset in it) as the page at `path`
/// composes it; None when composing the page never reaches the file.
pub(crate) fn resolve_through(
    index: &WorkspaceIndex,
    antora: &AntoraCatalog,
    path: &Path,
    document: &Document,
    name: &str,
    stop: (&Path, usize),
) -> Option<Resolution> {
    let context = antora.context_for_path(path);
    let mut chain = Chain::default();

    if let Some((_, meaning)) = BUILT_IN_ATTRIBUTES.iter().find(|(built, _)| *built == name) {
        chain.apply(Step::new(
            Origin::BuiltIn,
            None,
            Some((*meaning).to_owned()),
            true,
        ));
    }
    if let Some(context) = &context {
        if let Some(playbook) = antora.playbook() {
            if let Some(entry) = playbook.attribute_entries.get(name) {
                chain.apply(Step::from_entry(Origin::Playbook, &playbook.path, entry));
            }
        }
        if let Some(component) = antora.component(&context.component, context.version.as_deref()) {
            if let Some(entry) = component.attribute_entries.get(name) {
                let descriptor = component.root.join("antora.yml");
                chain.apply(Step::from_entry(Origin::Component, &descriptor, entry));
            }
        }
    }
    if let Some((_, value)) = antora_attributes(antora, path).find(|(computed, _)| computed == name)
    {
        chain.apply(Step::new(Origin::Antora, None, Some(value), false));
    }

    let mut walk = Walk {
        index,
        antora,
        page: context.as_ref(),
        name,
        stop: (normalize_path(stop.0), stop.1),
        reached: false,
        chain,
    };
    walk.file(&normalize_path(path), document, None, 0);
    walk.reached.then(|| walk.chain.finish())
}

/// The steps so far, and the one in effect with whether it is hard.
#[derive(Default)]
struct Chain {
    steps: Vec<Step>,
    current: Option<(usize, bool)>,
}

impl Chain {
    fn apply(&mut self, mut step: Step) {
        let hard = !step.soft;
        let index = self.steps.len();
        match self.current {
            Some((by, true)) => step.outcome = Outcome::Ignored { by },
            current => {
                if let Some((previous, _)) = current {
                    self.steps[previous].outcome = Outcome::Overridden { by: index };
                }
                self.current = Some((index, hard));
            }
        }
        self.steps.push(step);
    }

    fn after(&mut self, mut step: Step) {
        step.outcome = Outcome::After;
        self.steps.push(step);
    }

    fn finish(self) -> Resolution {
        Resolution {
            winner: self.current.map(|(winner, _)| winner),
            steps: self.steps,
        }
    }
}

impl Step {
    fn new(origin: Origin, location: Option<Location>, value: Option<String>, soft: bool) -> Self {
        Self {
            origin,
            location,
            value,
            soft,
            via: None,
            outcome: Outcome::Wins,
        }
    }

    fn from_entry(origin: Origin, path: &Path, entry: &AttributeEntry) -> Self {
        let location = Location {
            path: normalize_path(path),
            range: entry.range,
        };
        Self::new(origin, Some(location), entry.value.clone(), entry.soft)
    }
}

/// Composes a page in document order, following includes, until the place asked about.
struct Walk<'a> {
    index: &'a WorkspaceIndex,
    antora: &'a AntoraCatalog,
    page: Option<&'a AntoraContext>,
    name: &'a str,
    stop: (PathBuf, usize),
    reached: bool,
    chain: Chain,
}

impl Walk<'_> {
    fn file(&mut self, path: &Path, document: &Document, via: Option<&Location>, depth: usize) {
        if depth > MAX_INCLUDE_DEPTH {
            return;
        }
        let is_stop = path == self.stop.0;
        let mut events: Vec<(usize, Event<'_>)> = document
            .attributes
            .iter()
            .filter_map(|attribute| {
                let (name, unset) = declared_name(&attribute.name);
                (name == self.name).then(|| {
                    let value = if unset {
                        None
                    } else {
                        Some(attribute.value.clone().unwrap_or_default())
                    };
                    (
                        attribute.range.start,
                        Event::Declares(attribute.range, value),
                    )
                })
            })
            .chain(
                document
                    .includes
                    .iter()
                    .map(|include| (include.range.start, Event::Includes(include))),
            )
            .collect();
        events.sort_by_key(|(start, _)| *start);

        for (start, event) in events {
            if is_stop && !self.reached && start >= self.stop.1 {
                self.reached = true;
            }
            match event {
                Event::Declares(range, value) => {
                    let mut step = Step::new(
                        Origin::Declaration,
                        Some(Location {
                            path: path.to_path_buf(),
                            range,
                        }),
                        value,
                        true,
                    );
                    step.via = via.cloned();
                    if self.reached {
                        self.chain.after(step);
                    } else {
                        self.chain.apply(step);
                    }
                }
                Event::Includes(include) => {
                    let Some(target) = resolve_in_page(
                        self.index,
                        self.antora,
                        self.page,
                        path,
                        document,
                        &include.target,
                    ) else {
                        continue;
                    };
                    let target = normalize_path(&target);
                    let Some(file) = self.index.file(&target) else {
                        continue;
                    };
                    let here = Location {
                        path: path.to_path_buf(),
                        range: include.range,
                    };
                    let via = via.cloned().unwrap_or(here);
                    self.file(&target, &file.document, Some(&via), depth + 1);
                }
            }
        }
        if is_stop {
            self.reached = true;
        }
    }
}

enum Event<'a> {
    Declares(SourceRange, Option<String>),
    Includes(&'a adoc_core::IncludeDirective),
}

/// The name an attribute entry declares, and whether it unsets it (`name!` or `!name`).
fn declared_name(written: &str) -> (&str, bool) {
    if let Some(name) = written.strip_suffix('!') {
        (name, true)
    } else if let Some(name) = written.strip_prefix('!') {
        (name, true)
    } else {
        (written, false)
    }
}

/// The pages that include the file at `path`, directly or through other files, which nothing
/// includes in turn.
pub(crate) fn top_pages(ws: &WorkspaceState, path: &Path) -> Vec<PathBuf> {
    let start = normalize_path(path);
    let mut seen = BTreeSet::from([start.clone()]);
    let mut frontier = vec![start];
    let mut pages = BTreeSet::new();
    for _ in 0..=MAX_INCLUDE_DEPTH {
        let mut next = Vec::new();
        for file in &frontier {
            for location in includes_of(ws, file) {
                let includer = normalize_path(&location.path);
                if !seen.insert(includer.clone()) {
                    continue;
                }
                if includes_of(ws, &includer).is_empty() {
                    pages.insert(includer);
                } else {
                    next.push(includer);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    pages.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use adoc_core::Document;
    use adoc_index::normalize_path;

    use super::{attribute_name_at, effective_attribute, EffectiveAttribute, Origin, Outcome};
    use crate::WorkspaceState;

    fn fixture() -> PathBuf {
        normalize_path(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/effective-attributes"),
        )
    }

    fn workspace() -> WorkspaceState {
        let mut ws = WorkspaceState::new();
        ws.index_roots(&[fixture()]).unwrap();
        ws
    }

    fn page(name: &str) -> PathBuf {
        fixture().join("docs/modules/ROOT/pages").join(name)
    }

    fn partial(name: &str) -> PathBuf {
        fixture().join("docs/modules/ROOT/partials").join(name)
    }

    /// The attribute at the `nth` occurrence of `needle` in the file at `path`, one byte in.
    fn inspect(ws: &WorkspaceState, path: &Path, needle: &str, nth: usize) -> EffectiveAttribute {
        let text = std::fs::read_to_string(path).unwrap();
        let document: Document = adoc_parser::parse("file:///page.adoc", text.as_str()).document;
        let offset = text
            .match_indices(needle)
            .nth(nth)
            .unwrap_or_else(|| panic!("{needle} not found"))
            .0
            + 1;
        effective_attribute(ws, path, &document, offset).unwrap()
    }

    fn written(location: &adoc_core::Location) -> String {
        let text = std::fs::read_to_string(&location.path).unwrap();
        text[location.range.start..location.range.end].to_owned()
    }

    #[test]
    fn inspector_shows_component_and_page_header_and_which_wins() {
        let ws = workspace();
        let index = page("index.adoc");
        let found = inspect(&ws, &index, "{edition}", 0);
        assert_eq!(found.name, "edition");
        assert_eq!(found.resolution.value(), Some("Enterprise"));

        let steps = &found.resolution.steps;
        let component = steps
            .iter()
            .position(|step| step.origin == Origin::Component)
            .unwrap();
        let header = steps
            .iter()
            .position(|step| step.origin == Origin::Declaration)
            .unwrap();
        assert_eq!(found.resolution.winner, Some(header));
        assert_eq!(steps[header].outcome, Outcome::Wins);
        assert_eq!(steps[component].outcome, Outcome::Overridden { by: header });
        assert_eq!(steps[component].value.as_deref(), Some("Standard"));
        assert!(steps[component].soft);

        // Each links to where it is written.
        let at_component = steps[component].location.as_ref().unwrap();
        assert!(at_component.path.ends_with("docs/antora.yml"));
        assert_eq!(written(at_component), "edition");
        let at_header = steps[header].location.as_ref().unwrap();
        assert_eq!(at_header.path, index);
        assert_eq!(written(at_header), ":edition: Enterprise");

        // The declaration later in the page does not apply yet.
        let later = steps.last().unwrap();
        assert_eq!(later.value.as_deref(), Some("Later"));
        assert_eq!(later.outcome, Outcome::After);
    }

    #[test]
    fn a_later_declaration_wins_after_it() {
        let ws = workspace();
        let found = inspect(&ws, &page("index.adoc"), "{edition}", 1);
        assert_eq!(found.resolution.value(), Some("Later"));
        let steps = &found.resolution.steps;
        assert!(steps.iter().all(|step| step.outcome != Outcome::After));
    }

    #[test]
    fn a_hard_set_component_attribute_ignores_the_page() {
        let ws = workspace();
        let found = inspect(&ws, &page("index.adoc"), "{product}", 0);
        assert_eq!(found.resolution.value(), Some("Guide"));
        let steps = &found.resolution.steps;
        let origins: Vec<_> = steps.iter().map(|step| step.origin.clone()).collect();
        assert_eq!(
            origins,
            [Origin::Playbook, Origin::Component, Origin::Declaration]
        );
        assert_eq!(steps[0].outcome, Outcome::Overridden { by: 1 });
        assert_eq!(steps[1].outcome, Outcome::Wins);
        assert_eq!(steps[2].outcome, Outcome::Ignored { by: 1 });
    }

    #[test]
    fn a_hard_set_playbook_attribute_ignores_the_component_and_page() {
        let ws = workspace();
        let index = page("index.adoc");
        let company = inspect(&ws, &index, "{company}", 0);
        assert_eq!(company.resolution.value(), Some("Foundry Ltd"));
        assert_eq!(
            company.resolution.steps[1].outcome,
            Outcome::Ignored { by: 0 }
        );

        let locked = inspect(&ws, &index, "{locked}", 0);
        assert_eq!(locked.resolution.value(), Some("site"));
        assert_eq!(locked.resolution.steps[1].origin, Origin::Component);
        assert_eq!(
            locked.resolution.steps[1].outcome,
            Outcome::Ignored { by: 0 }
        );
    }

    #[test]
    fn a_component_sets_what_the_playbook_soft_unsets() {
        let ws = workspace();
        let found = inspect(&ws, &page("index.adoc"), "{support}", 0);
        assert_eq!(found.resolution.steps[0].value, None);
        assert!(found.resolution.steps[0].soft);
        assert_eq!(found.resolution.value(), Some("help desk"));
    }

    #[test]
    fn a_partial_gives_its_value_in_each_page_that_includes_it() {
        let ws = workspace();
        let terms = partial("terms.adoc");
        let found = inspect(&ws, &terms, "{edition}", 0);
        // On its own, the partial sees its component's value.
        assert_eq!(found.resolution.value(), Some("Standard"));

        let values: Vec<_> = found
            .includers
            .iter()
            .map(|includer| {
                (
                    includer
                        .page
                        .file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    includer.resolution.value().map(str::to_owned),
                )
            })
            .collect();
        assert_eq!(
            values,
            [
                ("index.adoc".to_owned(), Some("Enterprise".to_owned())),
                ("plain.adoc".to_owned(), Some("Standard".to_owned())),
            ]
        );
    }

    #[test]
    fn a_declaration_in_an_included_file_comes_through_its_include() {
        let ws = workspace();
        let mut text = std::fs::read_to_string(page("plain.adoc")).unwrap();
        text.push_str("\nSee {support-line}.\n");
        let path = page("plain.adoc");
        let document = adoc_parser::parse("file:///plain.adoc", text.as_str()).document;
        let offset = text.find("{support-line}").unwrap() + 1;
        let found = effective_attribute(&ws, &path, &document, offset).unwrap();
        assert_eq!(found.resolution.value(), Some("call us"));
        let step = found.resolution.winning_step().unwrap();
        assert_eq!(step.location.as_ref().unwrap().path, partial("terms.adoc"));
        let via = step.via.as_ref().unwrap();
        assert_eq!(via.path, path);
        assert_eq!(
            &text[via.range.start..via.range.end],
            "include::partial$terms.adoc[]"
        );
    }

    #[test]
    fn names_an_attribute_in_an_entry_and_in_ifdef() {
        let text = ":edition: Pro\n:!draft:\nifdef::alpha,beta+gamma[]\n";
        let at = |needle: &str| text.find(needle).unwrap() + 1;
        assert_eq!(attribute_name_at(text, at("Pro")).unwrap().0, "edition");
        assert_eq!(attribute_name_at(text, at("draft")).unwrap().0, "draft");
        assert_eq!(attribute_name_at(text, at("alpha")).unwrap().0, "alpha");
        assert_eq!(attribute_name_at(text, at("beta")).unwrap().0, "beta");
        assert_eq!(attribute_name_at(text, at("gamma")).unwrap().0, "gamma");
        assert_eq!(attribute_name_at("Plain text.\n", 3), None);
    }

    #[test]
    fn an_attribute_never_set_has_no_value_and_no_steps() {
        let ws = WorkspaceState::new();
        let text = "Use {missing} here.\n";
        let document = adoc_parser::parse("file:///page.adoc", text).document;
        let found = effective_attribute(&ws, Path::new("/page.adoc"), &document, 6).unwrap();
        assert_eq!(found.name, "missing");
        assert_eq!(found.resolution.winner, None);
        assert!(found.resolution.steps.is_empty());
        assert!(found.includers.is_empty());
    }

    #[test]
    fn an_unset_in_the_page_wins_over_a_soft_component_value() {
        let ws = workspace();
        let path = page("plain.adoc");
        let text = "= Plain\n:edition!:\n\n{edition}\n";
        let document = adoc_parser::parse("file:///plain.adoc", text).document;
        let offset = text.rfind("{edition}").unwrap() + 1;
        let found = effective_attribute(&ws, &path, &document, offset).unwrap();
        assert_eq!(found.resolution.value(), None);
        let winner = found.resolution.winning_step().unwrap();
        assert_eq!(winner.origin, Origin::Declaration);
        assert_eq!(winner.value, None);
    }

    #[test]
    fn half_typed_input_does_not_panic() {
        let ws = WorkspaceState::new();
        for text in [
            "{",
            ":",
            "::",
            ":!:",
            "ifdef::",
            "ifdef::[",
            "ifndef::a,,b[]",
            "{a",
        ] {
            let document = adoc_parser::parse("file:///page.adoc", text).document;
            for offset in 0..=text.len() {
                let _ = effective_attribute(&ws, Path::new("/page.adoc"), &document, offset);
            }
        }
    }
}
