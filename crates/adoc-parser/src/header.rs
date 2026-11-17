//! What a page's own main header asks of its exports: `:export-read-only:` (the read-only
//! exports PRD, `docs/prd/PRD-AdocFoundry_READONLY.md`, §5.1).
//!
//! Only the page's own text is read, so a declaration in an included file, `antora.yml`, the
//! playbook or a profile's attributes never counts, by construction. The main header is the
//! document title, its author and revision lines, attribute entries, comments and preprocessor
//! directives at the top of the page, up to the first blank line once it has begun, or the
//! first line that is none of those. Where the header cannot be read without running the
//! preprocessor (a declaration inside a conditional, or after an `include::` that could change
//! what follows), the declaration is an error rather than a guess.

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
}

impl HeaderReadOnlyError {
    /// The declaration at fault.
    #[must_use]
    pub fn range(&self) -> SourceRange {
        match self {
            Self::InvalidValue { range, .. }
            | Self::Conditional { range }
            | Self::AfterInclude { range } => *range,
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

/// Where the page's main header ends, as a byte offset: past its last line, or 0 when the
/// page has none.
#[must_use]
pub fn main_header_end(text: &str) -> usize {
    scan(text, |_, _, _| {})
}

/// The policy a page's own main header sets for its exports, or the declaration in it that
/// gives none.
pub fn export_read_only(text: &str) -> Result<HeaderReadOnly, HeaderReadOnlyError> {
    let mut found: Result<HeaderReadOnly, HeaderReadOnlyError> = Ok(HeaderReadOnly::default());
    scan(text, |declaration, in_conditional, after_include| {
        let Some(unset) = read_only_name(&declaration.name) else {
            return;
        };
        // The first error stands: a later declaration does not excuse it.
        if found.is_err() {
            return;
        }
        let range = declaration.range;
        found = if in_conditional {
            Err(HeaderReadOnlyError::Conditional { range })
        } else if after_include {
            Err(HeaderReadOnlyError::AfterInclude { range })
        } else {
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
        };
    });
    found
}

/// Walks the main header, calling `entry` for each attribute entry in it with whether it is
/// inside a conditional and whether an `include::` came before it. Returns where the header
/// ends.
fn scan(text: &str, mut entry: impl FnMut(&adoc_core::AttributeDeclaration, bool, bool)) -> usize {
    let mut offset = 0;
    let mut end = 0;
    let mut started = false;
    // After the title, its author and revision lines may follow.
    let mut text_lines_allowed = 0;
    let mut comment_block: Option<usize> = None;
    let mut depth = 0usize;
    let mut after_include = false;
    let mut continued = false;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let body = content(line);
        if let Some(length) = comment_block {
            if comment_delimiter(body) == Some(length) {
                comment_block = None;
            }
            continue;
        }
        if continued {
            // The rest of a value continued from the line before.
            continued = body.trim_end().ends_with(" \\");
            end = offset;
            continue;
        }
        if let Some(length) = comment_delimiter(body) {
            comment_block = Some(length);
            continue;
        }
        if body.trim().is_empty() {
            if started {
                break;
            }
            continue;
        }
        if body.starts_with("//") {
            continue;
        }
        if let Some(block) = conditional(body) {
            if block {
                depth += 1;
            } else if let Some(rest) = body.find('[').map(|open| &body[open + 1..]) {
                // A one-line conditional's text: an entry written there is conditional.
                if let Some(declaration) = parse_attribute(
                    rest.trim_end_matches(']'),
                    start + body.find('[').unwrap_or(0) + 1,
                ) {
                    let declaration = adoc_core::AttributeDeclaration {
                        range: SourceRange::new(start, start + body.len()),
                        ..declaration
                    };
                    entry(&declaration, true, after_include);
                }
            }
            end = offset;
            continue;
        }
        if body.trim_end().starts_with("endif::") {
            depth = depth.saturating_sub(1);
            end = offset;
            continue;
        }
        if body.starts_with("include::") {
            after_include = true;
            started = true;
            text_lines_allowed = 0;
            end = offset;
            continue;
        }
        if !started && (body.starts_with("= ") || body == "=") {
            started = true;
            text_lines_allowed = 2;
            end = offset;
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
            entry(&declaration, depth > 0, after_include);
            started = true;
            text_lines_allowed = 0;
            end = offset;
            continue;
        }
        // The two lines after the title, whatever they hold, are its author and revision
        // lines, as Asciidoctor and the export's parser read them.
        if text_lines_allowed > 0 {
            // The author line, then the revision line.
            text_lines_allowed -= 1;
            end = offset;
            continue;
        }
        break;
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
    fn only_the_main_header_counts() {
        // The body, past the first blank line or a section.
        assert_eq!(
            policy("= Page\n\n:export-read-only: true\n"),
            ExportReadOnly::Unspecified
        );
        // A section title straight after the title is read as the author line.
        assert_eq!(
            policy("= Page\n== Section\n:export-read-only:\n"),
            ExportReadOnly::Enabled
        );
        assert_eq!(
            policy("= Page\n\n== Section\n:export-read-only:\n"),
            ExportReadOnly::Unspecified
        );
        // Text before any header: there is none.
        assert_eq!(
            policy("Text.\n:export-read-only:\n"),
            ExportReadOnly::Unspecified
        );
        // Author and revision lines, comments and a comment block are part of the header.
        assert_eq!(
            policy("= Page\nPaul Snow <paul@example.com>\nv1.0, 2026-11-17\n// a note\n////\n:export-read-only: false\n////\n:export-read-only: true\n"),
            ExportReadOnly::Enabled
        );
        // A declaration inside a comment block is a comment.
        assert_eq!(
            policy("= Page\n////\n:export-read-only: true\n////\n"),
            ExportReadOnly::Unspecified
        );
        // Leading blank lines and comments come before the header; entries with no title are
        // still the header.
        assert_eq!(
            policy("\n// comment\n= Page\n:export-read-only:\n"),
            ExportReadOnly::Enabled
        );
        assert_eq!(
            policy(":export-read-only: true\n\nText.\n"),
            ExportReadOnly::Enabled
        );
        // A third text line after the title is the body.
        assert_eq!(
            policy("= Page\nAuthor\nv1\nText.\n:export-read-only:\n"),
            ExportReadOnly::Unspecified
        );
        assert_eq!(main_header_end("= Page\n:a: b\n\nText.\n"), 13);
        assert_eq!(main_header_end("Text.\n"), 0);
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
        let text = "= Page\n:export-read-only: tr\u{e9}\nifdef::[\n////\n:\n::\n:!:\n";
        for end in 0..=text.len() {
            if text.is_char_boundary(end) {
                let _ = export_read_only(&text[..end]);
                let _ = main_header_end(&text[..end]);
            }
        }
    }
}
