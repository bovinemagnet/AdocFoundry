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
    /// the title itself (AsciiDoc natural cross reference), the ids Antora and plain
    /// Asciidoctor generate for it, and forgiving forms of both that any punctuation matches.
    #[must_use]
    pub fn implicit_ids(&self) -> Vec<String> {
        self.implicit_ids_from(
            IdStyle::Antora.generate(&self.title),
            IdStyle::Asciidoctor.generate(&self.title),
        )
    }

    /// [`Section::implicit_ids`] given the ids each style generates for the title.
    fn implicit_ids_from(
        &self,
        antora: Option<String>,
        asciidoctor: Option<String>,
    ) -> Vec<String> {
        let mut ids = vec![self.title.clone()];
        ids.extend(antora);
        ids.extend(asciidoctor);
        ids.extend([
            canonical_id(&self.title),
            generated_id(&self.title, "_", '_'),
            alphanumeric_id(&self.title),
        ]);
        let mut seen = HashSet::new();
        ids.retain(|id| !id.is_empty() && seen.insert(id.clone()));
        ids
    }
}

/// A punctuation-insensitive form of a reference id, to compare a reference with an id in;
/// `IdStyle::generate` gives the id a build generates.
///
/// Asciidoctor keeps some punctuation when it generates a section id and drops the rest, and the
/// exact rules shift between versions and `idseparator` settings. Comparing both sides in this
/// form resolves a reference whenever it names the section, whatever the surrounding punctuation.
#[must_use]
pub fn canonical_id(text: &str) -> String {
    generated_id(text, "", '-')
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
    /// The id a build in this style generates for a section titled `title` with no anchor,
    /// following Asciidoctor's `Section.generate_id` on the title as its substitutions leave
    /// it: inline anchors, formatting marks, character replacements (`'`, `--`, `...`, arrows,
    /// `(C)`) and characters that are neither word characters, spaces, `-` nor `.` are dropped;
    /// runs of space, `.` and `-` (and `_` in Asciidoctor's style) become one separator; and
    /// Antora's empty prefix drops a leading one. `None` when only rendering could tell: the
    /// title holds an attribute reference or a macro, or leaves nothing.
    #[must_use]
    pub fn generate(self, title: &str) -> Option<String> {
        crate::probe::hit(crate::probe::Probe::SectionId);
        if title.contains('{') || title.contains("<<") || holds_a_macro(title) {
            return None;
        }
        let (prefix, separator) = match self {
            Self::Antora => ("", '-'),
            Self::Asciidoctor => ("_", '_'),
        };
        let separates = |character: char| {
            matches!(character, ' ' | '.' | '-') || (separator == '_' && character == '_')
        };
        let mut id = String::from(prefix);
        // The prefix is squeezed with what follows, as Asciidoctor prepends it first.
        let mut squeezing = prefix.chars().next_back().is_some_and(separates);
        for character in without_emphasis(&replaced(title)).chars() {
            if separates(character) {
                if !squeezing {
                    id.push(separator);
                }
                squeezing = true;
            } else if character.is_alphanumeric() || character == '_' {
                id.extend(character.to_lowercase());
                squeezing = false;
            }
        }
        if id.ends_with(separator) {
            id.pop();
        }
        if prefix.is_empty() && id.starts_with(separator) {
            id.remove(0);
        }
        (!id.is_empty() && id != prefix).then_some(id)
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
/// `== Parameters` is `_parameters_2`, or `parameters-2` in Antora). The id is empty where it
/// cannot be known without rendering the title (`IdStyle::generate`).
///
/// An anchor declared inside `ifdef`, `ifndef` or `ifeval` is not counted: it numbers the id
/// only in a build that includes it, so the id offered is the one without it, and
/// [`section_ids_with_conditionals`] gives the other. [`SectionIds`] gives both, in both
/// styles, generating each title's id once.
#[must_use]
pub fn section_ids(sections: &[Section], anchors: &[Anchor], style: IdStyle) -> Vec<String> {
    numbered_ids(
        sections,
        anchors,
        style,
        &generated_ids(sections, style, false),
        false,
    )
}

/// The id of each section as [`section_ids`] gives it, in a build where every anchor declared
/// inside `ifdef`, `ifndef` or `ifeval` is present.
#[must_use]
pub fn section_ids_with_conditionals(
    sections: &[Section],
    anchors: &[Anchor],
    style: IdStyle,
) -> Vec<String> {
    numbered_ids(
        sections,
        anchors,
        style,
        &generated_ids(sections, style, false),
        true,
    )
}

/// Every section's id in both styles, as [`section_ids`] and [`section_ids_with_conditionals`]
/// give them, with each title's id generated once per style: what the index keeps with a
/// page's entry, so a question about one section's id need not number every section again.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SectionIds {
    antora: Builds,
    asciidoctor: Builds,
}

/// One style's numbered ids, without the conditional anchors and with them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Builds {
    without_conditionals: Vec<String>,
    /// `None` when no anchor is conditional, so both builds number alike.
    with_conditionals: Option<Vec<String>>,
}

impl SectionIds {
    /// The ids of `sections`, whose document declares `anchors`.
    #[must_use]
    pub fn new(sections: &[Section], anchors: &[Anchor]) -> Self {
        Self::generate(sections, anchors, false).0
    }

    /// [`SectionIds::new`], and each section's [`Section::implicit_ids`], from the same
    /// generated ids.
    #[must_use]
    pub fn with_implicit_ids(sections: &[Section], anchors: &[Anchor]) -> (Self, Vec<Vec<String>>) {
        Self::generate(sections, anchors, true)
    }

    fn generate(
        sections: &[Section],
        anchors: &[Anchor],
        implicit: bool,
    ) -> (Self, Vec<Vec<String>>) {
        // A section with an anchor needs its title's ids only for its implicit ids.
        let antora = generated_ids(sections, IdStyle::Antora, implicit);
        let asciidoctor = generated_ids(sections, IdStyle::Asciidoctor, implicit);
        let conditional = anchors.iter().any(|anchor| anchor.conditional);
        let builds = |style: IdStyle, generated: &[Option<String>]| Builds {
            without_conditionals: numbered_ids(sections, anchors, style, generated, false),
            with_conditionals: conditional
                .then(|| numbered_ids(sections, anchors, style, generated, true)),
        };
        let ids = Self {
            antora: builds(IdStyle::Antora, &antora),
            asciidoctor: builds(IdStyle::Asciidoctor, &asciidoctor),
        };
        let implicit = if implicit {
            sections
                .iter()
                .zip(antora.into_iter().zip(asciidoctor))
                .map(|(section, (antora, asciidoctor))| {
                    section.implicit_ids_from(antora, asciidoctor)
                })
                .collect()
        } else {
            Vec::new()
        };
        (ids, implicit)
    }

    fn style(&self, style: IdStyle) -> &Builds {
        match style {
            IdStyle::Antora => &self.antora,
            IdStyle::Asciidoctor => &self.asciidoctor,
        }
    }

    /// Each section's id in `style`, as [`section_ids`] gives it.
    #[must_use]
    pub fn numbered(&self, style: IdStyle) -> &[String] {
        &self.style(style).without_conditionals
    }

    /// Each section's id in `style`, as [`section_ids_with_conditionals`] gives it.
    #[must_use]
    pub fn with_conditionals(&self, style: IdStyle) -> &[String] {
        let builds = self.style(style);
        builds
            .with_conditionals
            .as_deref()
            .unwrap_or(&builds.without_conditionals)
    }

    /// The ids of each build in `style`: [`SectionIds::numbered`], then, when an anchor is
    /// conditional and so may number them differently, [`SectionIds::with_conditionals`].
    pub fn builds(&self, style: IdStyle) -> impl Iterator<Item = &[String]> {
        let builds = self.style(style);
        std::iter::once(builds.without_conditionals.as_slice())
            .chain(builds.with_conditionals.as_deref())
    }
}

/// The id `style` generates for each section without an anchor, or for `every` section;
/// `None` for the rest.
fn generated_ids(sections: &[Section], style: IdStyle, every: bool) -> Vec<Option<String>> {
    sections
        .iter()
        .map(|section| {
            (every || section.id.is_none())
                .then(|| style.generate(&section.title))
                .flatten()
        })
        .collect()
}

/// Each section's id: its anchor, else its `generated` id numbered past those taken before it.
fn numbered_ids(
    sections: &[Section],
    anchors: &[Anchor],
    style: IdStyle,
    generated: &[Option<String>],
    conditionals: bool,
) -> Vec<String> {
    let mut taken = HashSet::new();
    let mut anchors = anchors
        .iter()
        .filter(|anchor| conditionals || !anchor.conditional)
        .peekable();
    let mut ids = Vec::with_capacity(sections.len());
    for (section, generated) in sections.iter().zip(generated) {
        while let Some(anchor) = anchors.next_if(|anchor| anchor.range.start < section.range.start)
        {
            taken.insert(anchor.id.clone());
        }
        let id = match &section.id {
            Some(id) => id.clone(),
            None => {
                let id = generated.clone().unwrap_or_default();
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

/// Whether `title` holds a macro, `name:target[…]` or a URL, whose text only rendering gives.
fn holds_a_macro(title: &str) -> bool {
    title.contains("://")
        || title.match_indices('[').any(|(open, _)| {
            let token = title[..open]
                .rsplit(char::is_whitespace)
                .next()
                .unwrap_or_default();
            token.split_once(':').is_some_and(|(name, _)| {
                name.chars()
                    .next_back()
                    .is_some_and(|character| character.is_ascii_alphabetic())
            })
        })
}

/// `title` without what Asciidoctor's substitutions turn into markup an id leaves out: inline
/// anchors, attribute lists before formatting marks, and the character replacements, which
/// become character references (`'`, `--` with any spaces around it, `...`, arrows, `(C)`,
/// `(R)` and `(TM)`).
fn replaced(title: &str) -> String {
    let mut text = String::with_capacity(title.len());
    let mut rest = title;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("[[") {
            if let Some(close) = after.find("]]") {
                rest = &after[close + 2..];
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('[') {
            if let Some(close) = after.find(']') {
                if after[close + 1..].starts_with(['#', '*', '_', '`', '^', '~']) {
                    rest = &after[close + 1..];
                    continue;
                }
            }
        }
        if rest.starts_with("--") {
            // A spaced dash takes its spaces with it.
            if text.ends_with(' ') && rest[2..].starts_with(' ') {
                text.pop();
                rest = &rest[3..];
            } else {
                rest = &rest[2..];
            }
            continue;
        }
        if let Some(found) = ["...", "->", "=>", "<-", "<=", "(C)", "(R)", "(TM)", "'"]
            .into_iter()
            .find(|replacement| rest.starts_with(replacement))
        {
            rest = &rest[found.len()..];
            continue;
        }
        let character = rest.chars().next().unwrap_or_default();
        text.push(character);
        rest = &rest[character.len_utf8()..];
    }
    text
}

/// `title` as a reader sees it: without the marks of code, strong, emphasis, highlighting and
/// passthrough, constrained or not, nor the attribute list before a mark (`[.role]#text#`).
/// Marks that form nothing stay, as Asciidoctor leaves them; nothing else is changed.
#[must_use]
pub fn plain_title(title: &str) -> String {
    let mut text = String::with_capacity(title.len());
    let mut rest = title;
    while let Some(open) = rest.find('[') {
        text.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find(']') {
            Some(close) if after[close + 1..].starts_with(MARKS) => rest = &after[close + 1..],
            _ => {
                text.push('[');
                rest = after;
            }
        }
    }
    text.push_str(rest);
    MARKS
        .iter()
        .fold(text, |text, &mark| without_marks(&text, mark))
}

/// The formatting marks `plain_title` removes, code first, so passthrough within code goes too.
const MARKS: [char; 5] = ['`', '*', '_', '#', '+'];

/// `text` without the `_` marks of emphasis, `__unconstrained__` and `_constrained_`, which
/// Asciidoctor turns into tags; every other mark is no word character and is dropped anyway.
fn without_emphasis(text: &str) -> String {
    without_marks(text, '_')
}

/// `text` without the pairs of `mark` that Asciidoctor turns into tags, unconstrained (doubled,
/// anywhere) and constrained (single, at word boundaries).
fn without_marks(text: &str, mark: char) -> String {
    let characters: Vec<char> = text.chars().collect();
    let is_word = |character: char| character.is_alphanumeric() || character == '_';
    let mut drop = vec![false; characters.len()];
    let mut at = 0;
    while at < characters.len() {
        if characters[at] != mark {
            at += 1;
            continue;
        }
        if characters.get(at + 1) == Some(&mark) {
            let close = (at + 3..characters.len().saturating_sub(1))
                .find(|&close| characters[close] == mark && characters[close + 1] == mark);
            if let Some(close) = close {
                drop[at..at + 2].fill(true);
                drop[close..close + 2].fill(true);
                at = close + 2;
                continue;
            }
        }
        let opens = at.checked_sub(1).is_none_or(|before| {
            !is_word(characters[before]) && !";:}".contains(characters[before])
        }) && characters
            .get(at + 1)
            .is_some_and(|first| !first.is_whitespace());
        let close = opens
            .then(|| {
                (at + 2..characters.len()).find(|&close| {
                    characters[close] == mark
                        && !characters[close - 1].is_whitespace()
                        && characters
                            .get(close + 1)
                            .is_none_or(|after| !is_word(*after))
                })
            })
            .flatten();
        match close {
            Some(close) => {
                drop[at] = true;
                drop[close] = true;
                at = close + 1;
            }
            None => at += 1,
        }
    }
    characters
        .into_iter()
        .zip(drop)
        .filter_map(|(character, dropped)| (!dropped).then_some(character))
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
    fn plain_titles_lose_their_formatting_marks_only() {
        for (title, plain) in [
            (
                "The one `lsp-types` exception",
                "The one lsp-types exception",
            ),
            (
                "*Bold* and _emphasis_ and #mark#",
                "Bold and emphasis and mark",
            ),
            (
                "**un**constrained __marks__ ``here``",
                "unconstrained marks here",
            ),
            ("A [.role]#marked# word", "A marked word"),
            ("Literal `+{attr}+` text", "Literal {attr} text"),
            // Marks that form nothing stay, as Asciidoctor leaves them.
            (
                "snake_case_name and 2 * 3 * 4",
                "snake_case_name and 2 * 3 * 4",
            ),
            ("C++ is a language", "C++ is a language"),
            ("A lone ` backtick", "A lone ` backtick"),
            (
                "It's a [[anchor]]title -> here",
                "It's a [[anchor]]title -> here",
            ),
        ] {
            assert_eq!(plain_title(title), plain, "{title}");
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

    /// Issue #533: an anchor inside `ifdef` numbers the section only in a build that sets the
    /// attribute, so the id offered is the unnumbered one, and the numbered one is kept as well.
    #[test]
    fn a_conditional_anchor_numbers_the_section_only_with_conditionals() {
        let sections = [
            section("Parameters", None, 20),
            section("Parameters", None, 40),
        ];
        let conditional = Anchor {
            conditional: true,
            ..anchor("parameters", 0)
        };

        assert_eq!(
            section_ids(
                &sections,
                std::slice::from_ref(&conditional),
                IdStyle::Antora
            ),
            ["parameters", "parameters-2"]
        );
        assert_eq!(
            section_ids_with_conditionals(&sections, &[conditional], IdStyle::Antora),
            ["parameters-2", "parameters-3"]
        );
    }

    /// Each title's id as Asciidoctor 2.0.26 generates it, with Antora's `idprefix: ''` and
    /// `idseparator: '-'` and with its own defaults (#529).
    #[test]
    fn generates_the_ids_asciidoctor_generates() {
        let table = [
            ("What's new?", "whats-new", "_whats_new"),
            ("Don't *panic*", "dont-panic", "_dont_panic"),
            ("max_connections", "max_connections", "_max_connections"),
            ("Use `foo_bar` now", "use-foo_bar-now", "_use_foo_bar_now"),
            ("Input/Output", "inputoutput", "_inputoutput"),
            ("Q&A", "qa", "_qa"),
            (
                "(Optional) Settings",
                "optional-settings",
                "_optional_settings",
            ),
            (".NET setup", "net-setup", "_net_setup"),
            ("`code` usage", "code-usage", "_code_usage"),
            ("\"Quoted\" title", "quoted-title", "_quoted_title"),
            ("[[lead]]Title", "title", "_title"),
            ("Step 1: Install", "step-1-install", "_step_1_install"),
            ("Tom & Jerry", "tom-jerry", "_tom_jerry"),
            (
                "1.2.3 Release notes",
                "1-2-3-release-notes",
                "_1_2_3_release_notes",
            ),
            ("a -- b and c--d...e -> f", "ab-and-cde-f", "_ab_and_cde_f"),
            ("_em_ and __un__der", "em-and-under", "_em_and_under"),
            ("Snake_case_ word", "snake_case_-word", "_snake_case_word"),
            ("Ünïcode Straße", "ünïcode-straße", "_ünïcode_straße"),
            ("[.r]#Marked# text", "marked-text", "_marked_text"),
            ("The (C) mark", "the-mark", "_the_mark"),
            ("Parameters", "parameters", "_parameters"),
        ];
        for (title, antora, asciidoctor) in table {
            assert_eq!(
                IdStyle::Antora.generate(title).as_deref(),
                Some(antora),
                "{title}"
            );
            assert_eq!(
                IdStyle::Asciidoctor.generate(title).as_deref(),
                Some(asciidoctor),
                "{title}"
            );
        }
    }

    /// A title whose text only rendering can tell, through an attribute or a macro, has no id
    /// to offer, rather than a wrong one (#529).
    #[test]
    fn generates_no_id_for_a_title_rendering_alone_can_tell() {
        for title in [
            "Using {product}",
            "See xref:other.adoc[Other]",
            "The https://example.com[site]",
            "Press kbd:[Ctrl+S]",
            "About <<intro>>",
            "!!!",
        ] {
            assert_eq!(IdStyle::Antora.generate(title), None, "{title}");
            assert_eq!(IdStyle::Asciidoctor.generate(title), None, "{title}");
        }
        let sections = [
            section("Using {product}", None, 0),
            section("Notes", None, 10),
        ];
        assert_eq!(section_ids(&sections, &[], IdStyle::Antora), ["", "notes"]);
    }

    /// The exact ids are among the forms a reference may name a section by.
    #[test]
    fn implicit_ids_include_the_exact_ids() {
        let ids = section("What's new?", None, 0).implicit_ids();
        assert!(ids.contains(&"whats-new".to_owned()), "{ids:?}");
        assert!(ids.contains(&"_whats_new".to_owned()), "{ids:?}");
    }

    #[test]
    fn implicit_ids_hold_each_form_once() {
        // Issue #533: `dedup` removed neighbours only.
        assert_eq!(
            section("parameters", None, 0).implicit_ids(),
            ["parameters", "_parameters"]
        );
    }

    /// `SectionIds` gives what `section_ids`, `section_ids_with_conditionals` and
    /// `implicit_ids` give, from one generated id per title and style (#533).
    #[test]
    fn section_ids_kept_together_match_each_on_its_own() {
        let sections = [
            section("Parameters", None, 20),
            section("What's new?", Some("news"), 30),
            section("Parameters", None, 40),
            section("Using {product}", None, 50),
        ];
        let anchors = [
            Anchor {
                conditional: true,
                ..anchor("parameters", 0)
            },
            anchor("_parameters", 10),
        ];
        let (ids, implicit) = SectionIds::with_implicit_ids(&sections, &anchors);
        for style in [IdStyle::Antora, IdStyle::Asciidoctor] {
            let numbered = section_ids(&sections, &anchors, style);
            let conditional = section_ids_with_conditionals(&sections, &anchors, style);
            assert_eq!(ids.numbered(style), numbered);
            assert_eq!(ids.with_conditionals(style), conditional);
            assert_eq!(
                ids.builds(style).collect::<Vec<_>>(),
                [numbered.as_slice(), conditional.as_slice()]
            );
        }
        let each: Vec<_> = sections.iter().map(Section::implicit_ids).collect();
        assert_eq!(implicit, each);
        assert_eq!(SectionIds::new(&sections, &anchors), ids);

        let plain = SectionIds::new(&sections, &[]);
        assert_eq!(
            plain.builds(IdStyle::Antora).count(),
            1,
            "no conditional anchor"
        );
        assert_eq!(
            plain.with_conditionals(IdStyle::Antora),
            plain.numbered(IdStyle::Antora)
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
