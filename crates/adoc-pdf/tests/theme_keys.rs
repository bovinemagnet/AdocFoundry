//! Issue #106: every key of asciidoctor-pdf 2.3's Theme Keys Reference is classified as
//! supported, approximated or ignored in `src/theme/compatibility.txt`, the classification agrees
//! with the keys the template and the emitter name, and the documentation's table is generated
//! from it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn manifest() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn reference_file() -> PathBuf {
    manifest().join("../../tests/fixtures/pdf/themes/reference-keys.txt")
}

fn compatibility_file() -> PathBuf {
    manifest().join("src/theme/compatibility.txt")
}

fn partial_file() -> PathBuf {
    manifest().join("../../src/docs/modules/ROOT/partials/theme-keys.adoc")
}

/// The lines of a data file that are not comments or blank.
fn data_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

fn reference_keys() -> Vec<String> {
    data_lines(&reference_file())
}

struct Entry {
    key: String,
    status: String,
    note: String,
}

fn compatibility() -> Vec<Entry> {
    data_lines(&compatibility_file())
        .into_iter()
        .map(|line| {
            let mut fields = line.splitn(3, " | ");
            let key = fields.next().unwrap().to_owned();
            let status = fields.next().unwrap_or_default().to_owned();
            let note = fields.next().unwrap_or_default().to_owned();
            Entry { key, status, note }
        })
        .collect()
}

/// Whether `name` is an instance of the reference key `pattern`, whose `<…>` placeholders
/// stand for one or more name characters.
fn matches(pattern: &str, name: &str) -> bool {
    fn go(pattern: &str, name: &str) -> bool {
        match pattern.find('<') {
            None => pattern == name,
            Some(open) => {
                let close = open + pattern[open..].find('>').unwrap();
                let (literal, rest) = (&pattern[..open], &pattern[close + 1..]);
                let Some(name) = name.strip_prefix(literal) else {
                    return false;
                };
                (1..=name.len()).any(|taken| {
                    name[..taken]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                        && go(rest, &name[taken..])
                })
            }
        }
    }
    go(pattern, name)
}

/// Double-quoted literals made of key characters in `text`.
fn literals(text: &str) -> Vec<&str> {
    text.split('"')
        .skip(1)
        .step_by(2)
        .filter(|literal| {
            !literal.is_empty()
                && literal
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
        .collect()
}

/// The template's source.
fn template() -> String {
    std::fs::read_to_string(manifest().join("src/adoc.typ")).unwrap()
}

/// Every key-like literal in the template and in the library's Rust sources, tests excluded.
/// The theme loader is left out: it sets defaults for keys as asciidoctor-pdf's loader does,
/// whether or not the export reads them.
fn named_in_sources() -> Vec<String> {
    let mut named: Vec<String> = literals(&template())
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut stack = vec![manifest().join("src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if !path.ends_with("theme") {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                let code = text.split("#[cfg(test)]").next().unwrap_or_default();
                named.extend(literals(code).into_iter().map(str::to_owned));
            }
        }
    }
    named
}

/// The keys the template reads by name through `pick`, each once, in order.
fn picked_in_template() -> Vec<String> {
    let template = template();
    let mut picked: Vec<String> = Vec::new();
    for call in template.split("pick(").skip(1) {
        // Up to the parenthesis that closes the call.
        let mut depth = 0;
        let end = call
            .char_indices()
            .find(|&(_, c)| {
                match c {
                    '(' => depth += 1,
                    ')' if depth == 0 => return true,
                    ')' => depth -= 1,
                    _ => {}
                }
                false
            })
            .map_or(call.len(), |(at, _)| at);
        let args = &call[..end];
        // Whole keys only: not `default:` values, nor parts joined with `+`.
        for arg in args.split(',').map(str::trim) {
            let Some(literal) = arg.strip_prefix('"').and_then(|a| a.strip_suffix('"')) else {
                continue;
            };
            if literals(arg) == [literal] && !picked.iter().any(|known| known == literal) {
                picked.push(literal.to_owned());
            }
        }
    }
    picked
}

const STATUSES: &[&str] = &["supported", "approximated", "ignored"];

#[test]
fn every_reference_key_is_classified_once() {
    let reference = reference_keys();
    assert!(reference.len() > 500, "{} keys", reference.len());
    let entries = compatibility();
    let mut seen = BTreeMap::new();
    for entry in &entries {
        assert!(
            STATUSES.contains(&entry.status.as_str()),
            "{}: unknown status {:?}",
            entry.key,
            entry.status
        );
        assert!(
            entry.status == "supported" || !entry.note.is_empty(),
            "{}: an {} key needs a note",
            entry.key,
            entry.status
        );
        assert!(
            seen.insert(entry.key.as_str(), ()).is_none(),
            "{} is classified twice",
            entry.key
        );
    }
    let missing: Vec<&String> = reference
        .iter()
        .filter(|key| !seen.contains_key(key.as_str()))
        .collect();
    assert!(missing.is_empty(), "not classified: {missing:?}");
    let extra: Vec<&str> = entries
        .iter()
        .map(|entry| entry.key.as_str())
        .filter(|key| !reference.iter().any(|known| known == key))
        .collect();
    assert!(extra.is_empty(), "not in the reference: {extra:?}");
}

#[test]
fn keys_the_export_names_are_not_ignored() {
    let named = named_in_sources();
    let wrong: Vec<String> = compatibility()
        .into_iter()
        .filter(|entry| entry.status == "ignored")
        .filter_map(|entry| {
            named
                .iter()
                .find(|name| matches(&entry.key, name))
                .map(|name| format!("{} (named as {name:?})", entry.key))
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "marked ignored but read by the export: {wrong:?}"
    );
}

#[test]
fn placeholders_match_names() {
    assert!(matches("heading_h<n>_font_size", "heading_h2_font_size"));
    assert!(matches(
        "role_<name>_font_color",
        "role_big-blue_font_color"
    ));
    assert!(matches(
        "footer_<side>_<position>_content",
        "footer_recto_left_content"
    ));
    assert!(!matches("heading_h<n>_font_size", "heading_font_size"));
    assert!(!matches("toc_font_size", "toc_font_size_min"));
}

/// The documentation's table: the keys by category, then the keys the template reads that the
/// reference does not list.
fn partial() -> String {
    let entries = compatibility();
    let reference = reference_keys();
    let count = |status: &str| entries.iter().filter(|e| e.status == status).count();
    let mut out = String::new();
    out.push_str(
        "// Generated by `cargo test -p adoc-pdf --test theme_keys` from \
         crates/adoc-pdf/src/theme/compatibility.txt.\n\
         // Run it with UPDATE_THEME_KEYS=1 to regenerate; do not edit by hand.\n\n",
    );
    out.push_str(&format!(
        "Of the {} keys in asciidoctor-pdf 2.3's Theme Keys Reference, {} are supported, {} \
         approximated and {} ignored.\n",
        entries.len(),
        count("supported"),
        count("approximated"),
        count("ignored"),
    ));
    let mut categories: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for entry in &entries {
        categories
            .entry(category(&entry.key))
            .or_default()
            .push(entry);
    }
    for (category, entries) in categories {
        out.push_str(&format!(
            "\n[#key-{}]\n=== {category}\n\n[cols=\"3,1,4\"]\n|===\n|Key |Status |Notes\n",
            category.replace('_', "-")
        ));
        for entry in entries {
            out.push_str(&format!(
                "\n|`{}`\n|{}\n|{}\n",
                entry.key, entry.status, entry.note
            ));
        }
        out.push_str("|===\n");
    }
    let extra: Vec<String> = picked_in_template()
        .into_iter()
        .filter(|name| !reference.iter().any(|key| matches(key, name)))
        .collect();
    if !extra.is_empty() {
        out.push_str(
            "\n[#key-extra]\n=== Keys beyond the reference\n\n\
             The export also reads these keys, which the reference does not list.\n\n",
        );
        for name in extra {
            out.push_str(&format!("* `{name}`\n"));
        }
    }
    out
}

/// The category a key is listed under: the longest category it starts with.
fn category(key: &str) -> &'static str {
    const CATEGORIES: &[&str] = &[
        "abstract",
        "admonition",
        "base",
        "block",
        "button",
        "callout_list",
        "caption",
        "code",
        "codespan",
        "conum",
        "cover",
        "description_list",
        "example",
        "extends",
        "footer",
        "footnotes",
        "font",
        "header",
        "heading",
        "image",
        "index",
        "kbd",
        "link",
        "list",
        "mark",
        "menu",
        "olist",
        "page",
        "prose",
        "quote",
        "quotes",
        "role",
        "running_content",
        "section",
        "sidebar",
        "svg",
        "table",
        "thematic_break",
        "title_page",
        "toc",
        "ulist",
        "verse",
    ];
    CATEGORIES
        .iter()
        .filter(|category| {
            key == **category
                || key
                    .strip_prefix(**category)
                    .is_some_and(|rest| rest.starts_with('_'))
        })
        .max_by_key(|category| category.len())
        .copied()
        .unwrap_or_else(|| panic!("{key} has no category"))
}

#[test]
fn the_documentation_table_is_current() {
    let expected = partial();
    let path = partial_file();
    if std::env::var_os("UPDATE_THEME_KEYS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &expected).unwrap();
        return;
    }
    let actual = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        actual == expected,
        "{} is out of date: run `UPDATE_THEME_KEYS=1 cargo test -p adoc-pdf --test theme_keys`",
        path.display()
    );
}
