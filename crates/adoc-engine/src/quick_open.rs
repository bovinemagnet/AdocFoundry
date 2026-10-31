//! Quick Open: the workspace's indexed files ranked by how well a typed query matches their
//! file name, title, Antora resource id or path, the files opened most recently first. Only
//! the index is read, never the disk.

use std::{
    cmp::Reverse,
    collections::HashMap,
    path::{Path, PathBuf},
};

use adoc_antora::{AntoraResource, AntoraResourceId, ResourceFamily};
use adoc_index::FileEntry;
use adoc_lang::WorkspaceState;

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

/// Every indexed file as Quick Open lists it, built once for an index, a catalog and the roots
/// and kept until one of them changes, so a keystroke allocates only for what it shows.
#[derive(Debug, Default)]
pub(crate) struct QuickOpenCache {
    /// The index's and catalog's revisions and the roots the items were built from.
    built_from: Option<(u64, u64, Vec<PathBuf>)>,
    items: Vec<QuickOpenItem>,
}

impl QuickOpenCache {
    /// The items of every file in `workspace`, below `roots`: built by the index worker with
    /// the index, so the first Quick Open after indexing does not build them.
    pub(crate) fn new(workspace: &WorkspaceState, roots: &[PathBuf]) -> Self {
        let mut resources: HashMap<&Path, &AntoraResource> = HashMap::new();
        for resource in workspace.antora.resources() {
            resources
                .entry(resource.source_path.as_path())
                .or_insert(resource);
        }
        let items = workspace
            .index
            .files()
            .map(|entry| {
                let resource = resources.get(entry.path.as_path()).copied();
                item(roots, entry, resource)
            })
            .collect();
        Self {
            built_from: Some(built_from(workspace, roots)),
            items,
        }
    }

    /// Brings the items up to date with `workspace` and `roots`: as they are when nothing
    /// changed, walking them beside the index when only files did, else from the start.
    fn update(&mut self, workspace: &WorkspaceState, roots: &[PathBuf]) {
        let now = built_from(workspace, roots);
        match &self.built_from {
            Some(then) if *then == now => {}
            // Only files changed: the names, paths and resource ids of the others stand.
            Some((_, catalog, then_roots)) if *catalog == now.1 && then_roots == roots => {
                let items = std::mem::take(&mut self.items);
                self.items = updated(items, workspace, roots);
                self.built_from = Some(now);
            }
            _ => *self = Self::new(workspace, roots),
        }
    }
}

fn built_from(workspace: &WorkspaceState, roots: &[PathBuf]) -> (u64, u64, Vec<PathBuf>) {
    (
        workspace.index.revision(),
        workspace.antora.revision(),
        roots.to_vec(),
    )
}

/// `items`, built from an earlier index of the same catalog and roots, brought up to date:
/// both are in path order, so they are walked side by side, keeping each item whose file is
/// still indexed and making only those of files added since.
fn updated(
    items: Vec<QuickOpenItem>,
    workspace: &WorkspaceState,
    roots: &[PathBuf],
) -> Vec<QuickOpenItem> {
    let mut old = items.into_iter().peekable();
    let mut updated = Vec::with_capacity(old.len());
    for entry in workspace.index.files() {
        while old.next_if(|item| item.path < entry.path).is_some() {}
        let title = entry
            .document
            .title
            .as_ref()
            .map(|title| title.text.as_str());
        match old.next_if(|item| item.path == entry.path) {
            Some(mut item) => {
                if item.title.as_deref() != title {
                    item.title = title.map(str::to_owned);
                }
                updated.push(item);
            }
            None => {
                let resource = workspace.antora.resource_for_path(&entry.path);
                updated.push(item(roots, entry, resource));
            }
        }
    }
    updated
}

/// The indexed file `entry`, read from the catalog as `resource`, as Quick Open lists it.
fn item(roots: &[PathBuf], entry: &FileEntry, resource: Option<&AntoraResource>) -> QuickOpenItem {
    adoc_core::probe::hit(adoc_core::probe::Probe::QuickOpenItem);
    QuickOpenItem {
        path: entry.path.clone(),
        relative_path: relative_path(roots, &entry.path),
        title: entry
            .document
            .title
            .as_ref()
            .map(|title| title.text.clone()),
        resource_id: resource.map(|resource| {
            let coordinate = &resource.coordinate;
            resource_id_string(&AntoraResourceId {
                version: coordinate.version.clone(),
                component: Some(coordinate.component.clone()),
                module: Some(coordinate.module.clone()),
                family: (coordinate.family != ResourceFamily::Page).then_some(coordinate.family),
                path: coordinate
                    .relative_path
                    .to_string_lossy()
                    .replace('\\', "/"),
            })
        }),
    }
}

/// `path` below the deepest of `roots` holding it, written with `/`.
fn relative_path(roots: &[PathBuf], path: &Path) -> String {
    let relative = roots
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

impl Engine {
    /// The indexed files matching `query`, at most `limit` of them. Each word of the query must
    /// match, its letters in order, the file name, title, resource id or path; matches in the
    /// name and title count most. Files opened recently come first, most recent first, then the
    /// rest by how well they match. An empty query lists every file, the recent first.
    #[must_use]
    pub fn quick_open(&self, query: &str, limit: usize) -> Vec<QuickOpenItem> {
        let words: Vec<Vec<char>> = query.split_whitespace().map(lower).collect();
        let recent: HashMap<&Path, usize> = self
            .recent
            .0
            .iter()
            .enumerate()
            .map(|(position, path)| (path.as_path(), position))
            .collect();
        let mut cache = self.quick_open.borrow_mut();
        cache.update(&self.workspace, &self.roots);

        // Recent files by recency, then the rest by score, then by path.
        let mut matches: Vec<(usize, Reverse<u32>, &QuickOpenItem)> = cache
            .items
            .iter()
            .filter_map(|item| {
                let score = item_score(&words, item)?;
                let rank = recent.get(item.path.as_path()).copied();
                Some((rank.unwrap_or(usize::MAX), Reverse(score), item))
            })
            .collect();
        let order = |left: &(usize, Reverse<u32>, &QuickOpenItem),
                     right: &(usize, Reverse<u32>, &QuickOpenItem)| {
            (left.0, left.1)
                .cmp(&(right.0, right.1))
                .then_with(|| left.2.relative_path.cmp(&right.2.relative_path))
        };
        if matches.len() > limit && limit > 0 {
            matches.select_nth_unstable_by(limit - 1, order);
        }
        matches.truncate(limit);
        matches.sort_by(order);
        matches
            .into_iter()
            .map(|(_, _, item)| item.clone())
            .collect()
    }
}

fn lower(text: &str) -> Vec<char> {
    text.chars().map(lower_char).collect()
}

/// One character for one, so offsets in the lowered text are offsets in the original.
fn lower_char(character: char) -> char {
    if character.is_ascii() {
        return character.to_ascii_lowercase();
    }
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
    let stem = name.split('.').next().unwrap_or(name);
    let mut total = 0;
    for word in words {
        let mut best = None;
        let mut consider = |score: Option<u32>, weight: u32| {
            if let Some(score) = score {
                best = best.max(Some(score * weight));
            }
        };
        let name_score = fuzzy_score(word, name).map(|score| {
            let starts_with = stem
                .chars()
                .map(lower_char)
                .take(word.len())
                .eq(word.iter().copied());
            score
                + if starts_with && stem.chars().count() == word.len() {
                    50
                } else if starts_with {
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
/// score less. `None` if the characters are not all there in order. Nothing is allocated, as
/// it runs for every file on every keystroke.
fn fuzzy_score(query: &[char], candidate: &str) -> Option<u32> {
    let Some(&first) = query.first() else {
        return Some(0);
    };
    // Try each place the first character matches, then take the rest greedily. Once the rest
    // is not all there after one place, it is not after any later one either.
    let mut best = None;
    let mut before = None;
    for (index, (offset, character)) in candidate.char_indices().enumerate() {
        let prior = before.replace(character);
        if lower_char(character) != first {
            continue;
        }
        match score_from(query, &candidate[offset..], index, prior) {
            Some(score) => best = best.max(Some(score)),
            None => break,
        }
    }
    best
}

/// The score of `query` matched greedily in `rest`, which is the candidate from its character
/// `index` on, after the character `before`.
fn score_from(
    query: &[char],
    rest: &str,
    mut index: usize,
    mut before: Option<char>,
) -> Option<u32> {
    let mut characters = rest.chars();
    let mut score: i64 = 0;
    let mut previous: Option<usize> = None;
    for &wanted in query {
        let (found, starts_word) = loop {
            let character = characters.next()?;
            let at = index;
            index += 1;
            let prior = before.replace(character);
            if lower_char(character) == wanted {
                break (at, starts_word(prior, character));
            }
        };
        score += 1;
        if found == 0 {
            score += 10;
        }
        if starts_word {
            score += 8;
        }
        match previous {
            Some(previous) if found == previous + 1 => score += 6,
            Some(previous) => score -= (found - previous - 1).min(3) as i64,
            None => {}
        }
        previous = Some(found);
    }
    Some(score.max(1) as u32)
}

/// A word starts at the beginning, after anything but a letter or digit, and at a capital
/// after a lower-case letter.
fn starts_word(before: Option<char>, character: char) -> bool {
    before.is_none_or(|before| {
        !before.is_alphanumeric() || (character.is_uppercase() && before.is_lowercase())
    })
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
