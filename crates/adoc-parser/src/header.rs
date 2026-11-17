//! What a page's own main header asks of its exports: `:export-read-only:` (the read-only
//! exports PRD, `docs/prd/PRD-AdocFoundry_READONLY.md`, §5.1).
//!
//! Only the page's own text is read, so a declaration in an included file, `antora.yml`, the
//! playbook or a profile's attributes never counts, by construction. The header is read as the
//! export's parser reads it (its oracle tests are the engine's `tests/read_only.rs`):
//!
//! - leading blank lines and comments come first;
//! - then the title (`= Title`, or Markdown's `# Title`), the attribute entries, comments and
//!   preprocessor lines that follow it, the first other line as the author line, and the next
//!   as the revision line when it reads as one (`v1.0`, `1.0, 2026-11-17: Draft`); a blank line
//!   or any other line ends it;
//! - with no title, the leading attribute entries are the header.
//!
//! Where a declaration's place cannot be read for certain, it is an error at its line rather
//! than a guess, so a page that asks is never exported editable: inside a conditional or after
//! an `include::` in the header; between the header's end and the first content (a blank line
//! after the title, say); in a header with anything but comments before its title; and in a
//! page beginning with a byte-order mark, which hides its header from the export's parser.

use adoc_core::SourceRange;

use crate::line_parser::parse_attribute;

/// The attribute's name.
pub const EXPORT_READ_ONLY: &str = "export-read-only";

/// What a page's header asks of its exports.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExportReadOnly {
    /// No declaration: exports as they always were.
    #[default]
    Unspecified,
    /// `:export-read-only: false` or unset (`:export-read-only!:`, `:!export-read-only:`).
    Disabled,
    /// `:export-read-only: true`, or the empty flag `:export-read-only:`.
    Enabled,
}

impl ExportReadOnly {
    /// Whether the exports are to carry an editing restriction.
    #[must_use]
    pub fn enabled(self) -> bool {
        self == Self::Enabled
    }
}

/// A page's export policy and the declaration that set it: the last in its main header.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HeaderReadOnly {
    pub policy: ExportReadOnly,
    /// The declaration's line, without its line ending; `None` when there is none.
    pub range: Option<SourceRange>,
}

/// Why a page's header gives no policy. Each names the declaration at fault.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeaderReadOnlyError {
    /// A value other than `true` or `false`.
    InvalidValue { value: String, range: SourceRange },
    /// Inside `ifdef::`, `ifndef::` or `ifeval::`, whose outcome is not known until rendering.
    Conditional { range: SourceRange },
    /// After an `include::` in the header, which could declare it again or end the header.
    AfterInclude { range: SourceRange },
    /// Where the header has ended or may not be the header: after it but before the first
    /// content, or in a header with something other than comments before its title.
    NotInHeader { range: SourceRange },
    /// In a page that begins with a byte-order mark, which hides its header from the export.
    ByteOrderMark { range: SourceRange },
}

impl HeaderReadOnlyError {
    /// The declaration at fault.
    #[must_use]
    pub fn range(&self) -> SourceRange {
        match self {
            Self::InvalidValue { range, .. }
            | Self::Conditional { range }
            | Self::AfterInclude { range }
            | Self::NotInHeader { range }
            | Self::ByteOrderMark { range } => *range,
        }
    }
}

impl std::fmt::Display for HeaderReadOnlyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidValue { value, .. } => write!(
                formatter,
                "`:{EXPORT_READ_ONLY}:` must be `true` or `false` (or empty for true), not `{value}`"
            ),
            Self::Conditional { .. } => write!(
                formatter,
                "`:{EXPORT_READ_ONLY}:` is inside a conditional; declare it unconditionally in \
                 the page's header"
            ),
            Self::AfterInclude { .. } => write!(
                formatter,
                "`:{EXPORT_READ_ONLY}:` follows an `include::` in the header; declare it before \
                 any include"
            ),
            Self::NotInHeader { .. } => write!(
                formatter,
                "`:{EXPORT_READ_ONLY}:` is not plainly in the page's header; declare it under the \
                 title, with no blank line or other line before it, and nothing but comments \
                 above the title"
            ),
            Self::ByteOrderMark { .. } => write!(
                formatter,
                "the page begins with a byte-order mark, which hides its header and \
                 `:{EXPORT_READ_ONLY}:` from the export; save it without one"
            ),
        }
    }
}

impl std::error::Error for HeaderReadOnlyError {}

/// A line without its line ending.
fn content(line: &str) -> &str {
    line.trim_end_matches(['\n', '\r'])
}

/// A delimiter line of a comment block: four or more slashes and nothing else.
fn comment_delimiter(line: &str) -> Option<usize> {
    let line = line.trim_end();
    (line.len() >= 4 && line.bytes().all(|byte| byte == b'/')).then_some(line.len())
}

/// A preprocessor conditional: `Some(true)` for a block form (`ifdef::name[]`), `Some(false)`
/// for one written on one line (`ifdef::name[text]`).
fn conditional(line: &str) -> Option<bool> {
    let line = line.trim_end();
    let rest = ["ifdef::", "ifndef::", "ifeval::"]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))?;
    let open = rest.find('[')?;
    let inside = rest[open + 1..].strip_suffix(']')?;
    // `ifeval::[…]` always has its expression inside the brackets and is a block.
    Some(inside.is_empty() || line.starts_with("ifeval::"))
}

/// Whether an attribute entry's name is `export-read-only`, set or unset, in any case:
/// Asciidoctor compares attribute names ignoring case.
fn read_only_name(name: &str) -> Option<bool> {
    let unset = name.starts_with('!') || name.ends_with('!');
    let bare = name.trim_start_matches('!').trim_end_matches('!');
    bare.eq_ignore_ascii_case(EXPORT_READ_ONLY).then_some(unset)
}

/// A document title: AsciiDoc's `= Title` or Markdown's `# Title`.
fn title(line: &str) -> bool {
    line.starts_with("= ") || line.starts_with("# ")
}

/// Block metadata that may stand above a title or a block: an anchor or an attribute list.
fn metadata(line: &str) -> bool {
    let line = line.trim_end();
    line.starts_with('[') && line.ends_with(']')
}

/// A revision line as the export's parser reads one: a version (`v1.0`, `1.0`, `2`), then
/// optionally `, <date>` and `: <remark>`.
fn revision(line: &str) -> bool {
    let line = line.trim_end();
    let version = line.split([',', ':']).next().unwrap_or_default();
    let number = version.strip_prefix('v').unwrap_or(version);
    number.starts_with(|first: char| first.is_ascii_digit())
        && !number.contains(char::is_whitespace)
}

/// Where a declaration stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Place {
    Header,
    Conditional,
    AfterInclude,
    NotInHeader,
}

/// Where the page's main header ends, as a byte offset: past its last line, or 0 when the
/// page has none.
#[must_use]
pub fn main_header_end(text: &str) -> usize {
    scan(text, |_, _| {})
}

/// The policy a page's own main header sets for its exports, or the declaration in it that
/// gives none.
pub fn export_read_only(text: &str) -> Result<HeaderReadOnly, HeaderReadOnlyError> {
    let mut found: Result<HeaderReadOnly, HeaderReadOnlyError> = Ok(HeaderReadOnly::default());
    let bom = text.starts_with('\u{feff}');
    scan(text, |declaration, place| {
        let Some(unset) = read_only_name(&declaration.name) else {
            return;
        };
        // The first error stands: a later declaration does not excuse it.
        if found.is_err() {
            return;
        }
        let range = declaration.range;
        found = match place {
            _ if bom => Err(HeaderReadOnlyError::ByteOrderMark { range }),
            Place::Conditional => Err(HeaderReadOnlyError::Conditional { range }),
            Place::AfterInclude => Err(HeaderReadOnlyError::AfterInclude { range }),
            Place::NotInHeader => Err(HeaderReadOnlyError::NotInHeader { range }),
            Place::Header => {
                let policy = match (unset, declaration.value.as_deref()) {
                    (true, _) | (false, Some("false")) => Ok(ExportReadOnly::Disabled),
                    (false, None | Some("true")) => Ok(ExportReadOnly::Enabled),
                    (false, Some(value)) => Err(HeaderReadOnlyError::InvalidValue {
                        value: value.to_owned(),
                        range,
                    }),
                };
                policy.map(|policy| HeaderReadOnly {
                    policy,
                    range: Some(range),
                })
            }
        };
    });
    found
}

/// Where the scan is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    /// Before anything but blank lines and comments.
    Before,
    /// After entries or block metadata with no title yet.
    Untitled,
    /// After the title: `lines` text lines (author, revision) taken so far.
    Titled { lines: u8 },
    /// After the header, before the first content.
    After,
}

/// Walks the top of the page, calling `entry` for each attribute entry up to the first content
/// with where it stands. Returns where the header ends.
fn scan(text: &str, mut entry: impl FnMut(&adoc_core::AttributeDeclaration, Place)) -> usize {
    let text_start = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let mut offset = text_start;
    let mut end = 0;
    let mut state = State::Before;
    // Something other than comments came before the title.
    let mut before_title = false;
    // An anchor or attribute list came with no title: a block, not a header, may follow.
    let mut untitled_metadata = false;
    let mut comment_block: Option<usize> = None;
    let mut depth = 0usize;
    let mut after_include = false;
    let mut continued = false;
    for line in text[text_start..].split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let body = content(line);
        let in_header = matches!(state, State::Untitled | State::Titled { .. });
        if let Some(length) = comment_block {
            if comment_delimiter(body) == Some(length) {
                comment_block = None;
            }
            continue;
        }
        if continued {
            continued = body.trim_end().ends_with(" \\");
            if in_header {
                end = offset;
            }
            continue;
        }
        if let Some(length) = comment_delimiter(body) {
            comment_block = Some(length);
            continue;
        }
        if body.starts_with("//") {
            continue;
        }
        if body.trim().is_empty() {
            if in_header {
                state = State::After;
            }
            continue;
        }
        let place = |state: State, before_title: bool| match state {
            State::After => Place::NotInHeader,
            State::Titled { .. } if before_title => Place::NotInHeader,
            State::Untitled if untitled_metadata => Place::NotInHeader,
            _ if depth > 0 => Place::Conditional,
            _ if after_include => Place::AfterInclude,
            _ => Place::Header,
        };
        if let Some(block) = conditional(body) {
            if block {
                depth += 1;
            } else if let Some(open) = body.find('[') {
                // A one-line conditional's text: an entry written there is conditional.
                let inside = body[open + 1..].trim_end_matches(']');
                if let Some(declaration) = parse_attribute(inside, start + open + 1) {
                    let declaration = adoc_core::AttributeDeclaration {
                        range: SourceRange::new(start, start + body.len()),
                        ..declaration
                    };
                    let place = match place(state, before_title) {
                        Place::Header | Place::AfterInclude => Place::Conditional,
                        other => other,
                    };
                    entry(&declaration, place);
                }
            }
            if in_header {
                end = offset;
            }
            continue;
        }
        if body.trim_end().starts_with("endif::") {
            depth = depth.saturating_sub(1);
            if in_header {
                end = offset;
            }
            continue;
        }
        if body.starts_with("include::") {
            after_include = true;
            if state == State::Before {
                state = State::Untitled;
                before_title = true;
            }
            if matches!(state, State::Untitled | State::Titled { .. }) {
                end = offset;
            }
            continue;
        }
        if let Some(declaration) = parse_attribute(line, start) {
            continued = declaration
                .value
                .as_deref()
                .is_some_and(|value| value.ends_with(" \\") || value == "\\");
            let declaration = adoc_core::AttributeDeclaration {
                range: SourceRange::new(start, start + body.len()),
                ..declaration
            };
            if state == State::Before {
                state = State::Untitled;
                before_title = true;
            }
            entry(&declaration, place(state, before_title));
            if matches!(state, State::Untitled | State::Titled { .. }) {
                end = offset;
            }
            continue;
        }
        match state {
            State::Before | State::Untitled if title(body) => {
                state = State::Titled { lines: 0 };
                end = offset;
            }
            State::Before | State::Untitled if metadata(body) => {
                // An anchor or attribute list above the title, or above a block.
                before_title = true;
                untitled_metadata = true;
                state = State::Untitled;
            }
            State::Titled { lines: 0 } => {
                state = State::Titled { lines: 1 };
                end = offset;
            }
            State::Titled { lines: 1 } if revision(body) => {
                state = State::Titled { lines: 2 };
                end = offset;
            }
            State::After if metadata(body) || body.starts_with('.') => {
                // Block metadata before the first block: still before the content.
            }
            _ => break,
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(text: &str) -> ExportReadOnly {
        export_read_only(text).unwrap().policy
    }

    #[test]
    fn values_give_the_policy() {
        assert_eq!(policy("= Page\n\nText.\n"), ExportReadOnly::Unspecified);
        assert_eq!(
            policy("= Page\n:export-read-only: true\n"),
            ExportReadOnly::Enabled
        );
        assert_eq!(
            policy("# Page\n:export-read-only: true\n"),
            ExportReadOnly::Enabled
        );
        assert_eq!(
            policy("= Page\n:export-read-only:\n"),
            ExportReadOnly::Enabled
        );
        assert_eq!(
            policy("= Page\n:export-read-only: false\n"),
            ExportReadOnly::Disabled
        );
        assert_eq!(
            policy("= Page\n:export-read-only!:\n"),
            ExportReadOnly::Disabled
        );
        assert_eq!(
            policy("= Page\n:!export-read-only:\n"),
            ExportReadOnly::Disabled
        );
        assert_eq!(
            policy("= Page\r\n:export-read-only: true\r\n\r\nText.\r\n"),
            ExportReadOnly::Enabled
        );
        assert_eq!(
            policy("= Page\n:Export-Read-Only: true\n"),
            ExportReadOnly::Enabled
        );
        assert_eq!(
            policy("= Page\n:export-read-only:   true  \n"),
            ExportReadOnly::Enabled
        );
        for value in ["TRUE", "yes", "1", "on", "True"] {
            let error =
                export_read_only(&format!("= Page\n:export-read-only: {value}\n")).unwrap_err();
            assert_eq!(
                error,
                HeaderReadOnlyError::InvalidValue {
                    value: value.to_owned(),
                    range: SourceRange::new(7, 7 + 19 + value.len()),
                }
            );
        }
    }

    #[test]
    fn the_last_declaration_wins_and_its_line_is_kept() {
        let text = "= Page\n:export-read-only: true\n:export-read-only!:\n";
        assert_eq!(
            export_read_only(text).unwrap(),
            HeaderReadOnly {
                policy: ExportReadOnly::Disabled,
                range: Some(SourceRange::new(31, 50)),
            }
        );
        assert_eq!(
            policy("= Page\n:export-read-only!:\n:export-read-only:\n"),
            ExportReadOnly::Enabled
        );
    }

    #[test]
    fn the_header_is_read_as_the_export_reads_it() {
        // The author line is any line after the title; the revision line only one that reads
        // as a revision.
        assert_eq!(
            policy("= Page\n== Section\n:export-read-only:\n"),
            ExportReadOnly::Enabled
        );
        assert_eq!(
            policy("= Page\nPaul Snow <paul@example.com>\nv1.0, 2026-11-17: Draft\n// a note\n////\n:export-read-only: false\n////\n:export-read-only: true\n"),
            ExportReadOnly::Enabled
        );
        assert_eq!(
            policy("= Page\nAuthor\nText.\n:export-read-only:\n"),
            ExportReadOnly::Unspecified
        );
        assert_eq!(
            policy("= Page\nAuthor\nv1\nText.\n:export-read-only:\n"),
            ExportReadOnly::Unspecified
        );
        // A declaration inside a comment block is a comment.
        assert_eq!(
            policy("= Page\n////\n:export-read-only: true\n////\n"),
            ExportReadOnly::Unspecified
        );
        // Leading blank lines and comments come before the header; entries with no title are
        // the header.
        assert_eq!(
            policy("\n// comment\n= Page\n:export-read-only:\n"),
            ExportReadOnly::Enabled
        );
        assert_eq!(
            policy(":export-read-only: true\n\nText.\n"),
            ExportReadOnly::Enabled
        );
        // After content, the body.
        assert_eq!(
            policy("Text.\n:export-read-only:\n"),
            ExportReadOnly::Unspecified
        );
        assert_eq!(
            policy("= Page\n\nText.\n\n:export-read-only: true\n"),
            ExportReadOnly::Unspecified
        );
        assert_eq!(main_header_end("= Page\n:a: b\n\nText.\n"), 13);
        assert_eq!(main_header_end("Text.\n"), 0);
    }

    #[test]
    fn a_declaration_whose_place_is_uncertain_is_an_error() {
        let not_in_header = |text: &str| {
            matches!(
                export_read_only(text),
                Err(HeaderReadOnlyError::NotInHeader { .. })
            )
        };
        // A blank line after the title, then the declaration before any content.
        assert!(not_in_header(
            "= Page\n\n:export-read-only: true\n\nText.\n"
        ));
        assert!(not_in_header("= Page\n\n[abstract]\n:export-read-only:\n"));
        // Anything but comments above the title.
        assert!(not_in_header(":a: b\n= Page\n:export-read-only:\n"));
        assert!(not_in_header("[[id]]\n= Page\n:export-read-only:\n"));
        // A byte-order mark.
        assert!(matches!(
            export_read_only("\u{feff}= Page\n:export-read-only:\n"),
            Err(HeaderReadOnlyError::ByteOrderMark { .. })
        ));
    }

    #[test]
    fn what_the_preprocessor_decides_is_an_error() {
        let error =
            export_read_only("= Page\nifdef::draft[]\n:export-read-only: true\nendif::[]\n")
                .unwrap_err();
        assert!(matches!(error, HeaderReadOnlyError::Conditional { .. }));
        let error =
            export_read_only("= Page\nifdef::draft[:export-read-only: true]\n").unwrap_err();
        assert!(matches!(error, HeaderReadOnlyError::Conditional { .. }));
        let error = export_read_only("= Page\ninclude::attrs.adoc[]\n:export-read-only: true\n")
            .unwrap_err();
        assert!(matches!(error, HeaderReadOnlyError::AfterInclude { .. }));
        // Other entries in a conditional, and an include with no declaration after it, are fine.
        assert_eq!(
            policy(
                "= Page\n:export-read-only:\nifdef::draft[]\n:x: y\nendif::[]\ninclude::a.adoc[]\n"
            ),
            ExportReadOnly::Enabled
        );
        // A later declaration does not excuse an earlier error.
        assert!(
            export_read_only("= Page\n:export-read-only: maybe\n:export-read-only: true\n")
                .is_err()
        );
    }

    #[test]
    fn half_written_headers_never_panic() {
        let text =
            "\u{feff}= Page\n:export-read-only: tr\u{e9}\nifdef::[\n////\n:\n::\n:!:\n# \n[\n";
        for end in 0..=text.len() {
            if text.is_char_boundary(end) {
                let _ = export_read_only(&text[..end]);
                let _ = main_header_end(&text[..end]);
            }
        }
    }
}
