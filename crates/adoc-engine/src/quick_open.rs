//! Quick Open: the workspace's indexed files ranked by how well a typed query matches their
//! file name, title, Antora resource id or path, the files opened most recently first. Only
//! the index is read, never the disk.

use std::{
    cmp::Reverse,
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use adoc_antora::{AntoraResourceId, ResourceFamily};

use crate::{
    commands::{ids, register, spec},
    resource_id_string, CommandArgs, CommandCategory, CommandContext, CommandOutcome,
    CommandRegistry, Engine, EngineError,
};

/// The most files `workspace.quick_open` lists.
pub const QUICK_OPEN_LIMIT: usize = 100;

/// How many recently opened files are remembered.
const RECENT_FILES: usize = 50;

/// A file Quick Open lists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuickOpenItem {
    pub path: PathBuf,
    /// The path below the workspace root it lies in, with `/` between its parts.
    pub relative_path: String,
    /// The document title, `= Title`.
    pub title: Option<String>,
    /// A page's resource id (`1.0@component:module:page.adoc`), or another family's with its
    /// family (`component:module:partial$intro.adoc`).
    pub resource_id: Option<String>,
}

/// The files made active, most recent first.
#[derive(Debug, Default)]
pub(crate) struct RecentFiles(Vec<PathBuf>);

impl RecentFiles {
    pub(crate) fn touch(&mut self, path: &Path) {
        self.0.retain(|recent| recent != path);
        self.0.insert(0, path.to_path_buf());
        self.0.truncate(RECENT_FILES);
    }
}

pub(crate) fn register_quick_open(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::WORKSPACE_QUICK_OPEN,
            "Quick Open…",
            CommandCategory::Workspace,
            Some("cmd-p"),
            CommandContext::Always,
        ),
        quick_open_command,
    );
}

/// Takes `CommandArgs::Query`, whose document it ignores, and lists the first
/// [`QUICK_OPEN_LIMIT`] matches.
fn quick_open_command(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Query { query, .. } = args else {
        return Err(EngineError::InvalidArguments {
            command: ids::WORKSPACE_QUICK_OPEN,
            expected: "a query",
        });
    };
    Ok(CommandOutcome::QuickOpen(
        engine.quick_open(&query, QUICK_OPEN_LIMIT),
    ))
}

impl Engine {
    /// The indexed files matching `query`, at most `limit` of them. Each word of the query must
    /// match, its letters in order, the file name, title, resource id or path; matches in the
    /// name and title count most. Files opened recently come first, most recent first, then the
    /// rest by how well they match. An empty query lists every file, the recent first.
    #[must_use]
    pub fn quick_open(&self, query: &str, limit: usize) -> Vec<QuickOpenItem> {
        let words: Vec<Vec<char>> = query.split_whitespace().map(lower).collect();
        let resources: BTreeMap<&Path, AntoraResourceId> = self
            .workspace
            .antora
            .resources()
            .map(|resource| {
                let coordinate = &resource.coordinate;
                let id = AntoraResourceId {
                    version: coordinate.version.clone(),
                    component: Some(coordinate.component.clone()),
                    module: Some(coordinate.module.clone()),
                    family: (coordinate.family != ResourceFamily::Page)
                        .then_some(coordinate.family),
                    path: coordinate
                        .relative_path
                        .to_string_lossy()
                        .replace('\\', "/"),
                };
                (resource.source_path.as_path(), id)
            })
            .collect();
        let recent = |path: &Path| self.recent.0.iter().position(|recent| recent == path);

        let mut matches: Vec<(Option<usize>, u32, QuickOpenItem)> = Vec::new();
        for entry in self.workspace.index.files() {
            let item = QuickOpenItem {
                path: entry.path.clone(),
                relative_path: self.relative_path(&entry.path),
                title: entry
                    .document
                    .title
                    .as_ref()
                    .map(|title| title.text.clone()),
                resource_id: resources.get(entry.path.as_path()).map(resource_id_string),
            };
            let Some(score) = item_score(&words, &item) else {
                continue;
            };
            matches.push((recent(&item.path), score, item));
        }
        // Recent files by recency, then the rest by score, then by path.
        matches.sort_by(
            |(left_recent, left_score, left), (right_recent, right_score, right)| {
                let rank = |recent: &Option<usize>| recent.map_or(usize::MAX, |position| position);
                rank(left_recent)
                    .cmp(&rank(right_recent))
                    .then(Reverse(left_score).cmp(&Reverse(right_score)))
                    .then_with(|| left.relative_path.cmp(&right.relative_path))
            },
        );
        matches
            .into_iter()
            .take(limit)
            .map(|(_, _, item)| item)
            .collect()
    }

    /// `path` below the deepest workspace root holding it, written with `/`.
    fn relative_path(&self, path: &Path) -> String {
        let relative = self
            .roots
            .iter()
            .filter_map(|root| path.strip_prefix(root).ok())
            .min_by_key(|relative| relative.components().count())
            .unwrap_or(path);
        relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }
}

fn lower(text: &str) -> Vec<char> {
    text.chars().map(lower_char).collect()
}

/// One character for one, so offsets in the lowered text are offsets in the original.
fn lower_char(character: char) -> char {
    character.to_lowercase().next().unwrap_or(character)
}

/// The score of `item` for the query's `words`: the sum of each word's best match, weighted by
/// the field it matched in. `None` if a word matches nowhere.
fn item_score(words: &[Vec<char>], item: &QuickOpenItem) -> Option<u32> {
    let name = item
        .relative_path
        .rsplit('/')
        .next()
        .unwrap_or(&item.relative_path);
    let stem: Vec<char> = lower(name.split('.').next().unwrap_or(name));
    let mut total = 0;
    for word in words {
        let mut best = None;
        let mut consider = |score: Option<u32>, weight: u32| {
            if let Some(score) = score {
                best = best.max(Some(score * weight));
            }
        };
        let name_score = fuzzy_score(word, name).map(|score| {
            score
                + if stem == *word {
                    50
                } else if stem.starts_with(word) {
                    20
                } else {
                    0
                }
        });
        consider(name_score, 3);
        consider(
            item.title
                .as_deref()
                .and_then(|title| fuzzy_score(word, title)),
            2,
        );
        consider(
            item.resource_id
                .as_deref()
                .and_then(|id| fuzzy_score(word, id)),
            1,
        );
        consider(fuzzy_score(word, &item.relative_path), 1);
        total += best?;
    }
    Some(total)
}

/// How well `query`, lower case, matches `candidate`: every character in order, ignoring
/// case. Matches at the start of the text or of a word, and runs of matches, score more; gaps
/// score less. `None` if the characters are not all there in order.
fn fuzzy_score(query: &[char], candidate: &str) -> Option<u32> {
    let Some(&first) = query.first() else {
        return Some(0);
    };
    let original: Vec<char> = candidate.chars().collect();
    let text: Vec<char> = original.iter().copied().map(lower_char).collect();
    // A word starts at the beginning, after anything but a letter or digit, and at a capital
    // after a lower-case letter.
    let starts: Vec<bool> = (0..original.len())
        .map(|index| {
            index == 0
                || !original[index - 1].is_alphanumeric()
                || (original[index].is_uppercase() && original[index - 1].is_lowercase())
        })
        .collect();
    // Try each place the first character matches, then take the rest greedily.
    (0..text.len())
        .filter(|&start| text[start] == first)
        .filter_map(|start| {
            let mut score: i64 = 0;
            let mut previous: Option<usize> = None;
            let mut position = start;
            for &character in query {
                let found = position + text[position..].iter().position(|&c| c == character)?;
                score += 1;
                if found == 0 {
                    score += 10;
                }
                if starts[found] {
                    score += 8;
                }
                match previous {
                    Some(previous) if found == previous + 1 => score += 6,
                    Some(previous) => score -= (found - previous - 1).min(3) as i64,
                    None => {}
                }
                previous = Some(found);
                position = found + 1;
            }
            Some(score.max(1) as u32)
        })
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(query: &str, candidate: &str) -> Option<u32> {
        fuzzy_score(&lower(query), candidate)
    }

    #[test]
    fn matches_characters_in_order_ignoring_case() {
        assert!(score("gde", "guide.adoc").is_some());
        assert!(score("GUIDE", "guide.adoc").is_some());
        assert!(score("guide", "GUIDE.adoc").is_some());
        assert_eq!(score("edg", "guide.adoc"), None);
        assert_eq!(score("x", ""), None);
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn prefers_word_starts_and_runs() {
        // Both start words in the first; scattered through one word in the second.
        assert!(score("st", "sea-tuple") > score("st", "setup"));
        assert!(score("st", "SeaTurtle") > score("st", "setup"));
        // A run beats the same letters apart.
        assert!(score("set", "setup") > score("set", "sxexty"));
        // The best of several starting places counts, not the first.
        assert_eq!(score("ab", "xa-ab"), score("ab", "-ab"));
    }

    #[test]
    fn handles_text_that_lower_cases_to_more_than_one_character() {
        // `İ` lower-cases to two characters; offsets must stay in step.
        assert!(score("i", "İstanbul").is_some());
        assert!(score("stanbul", "İstanbul").is_some());
        assert!(score("é", "café").is_some());
    }

    #[test]
    fn recent_files_keep_the_latest_first_without_repeats() {
        let mut recent = RecentFiles::default();
        for name in ["a", "b", "a", "c"] {
            recent.touch(Path::new(name));
        }
        assert_eq!(
            recent.0,
            [PathBuf::from("c"), PathBuf::from("a"), PathBuf::from("b")]
        );
        for index in 0..RECENT_FILES + 10 {
            recent.touch(&PathBuf::from(index.to_string()));
        }
        assert_eq!(recent.0.len(), RECENT_FILES);
    }
}
