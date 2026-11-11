//! The local files a diagram's source names for its tool to read: PlantUML's includes, imports
//! and themes, Graphviz's images and shape files, D2's imports and icons. Found conservatively:
//! a name written with a variable, a URL, PlantUML's standard library or anything else that
//! cannot be read as a plain path is left out, and so is a path outside the page's folder.

use std::path::{Component, Path, PathBuf};

/// The files `source`, a diagram of `kind` on a page in `folder`, names for its tool to read,
/// as paths inside `folder`, in the order named, each once. They need not exist.
pub(crate) fn files_read(kind: &str, source: &str, folder: &Path) -> Vec<PathBuf> {
    let names = match kind {
        "plantuml" | "uml" => plantuml(source),
        "graphviz" | "dot" => graphviz(source),
        "d2" => d2(source),
        _ => Vec::new(),
    };
    let mut files = Vec::new();
    for name in names {
        if let Some(path) = inside(folder, &name) {
            if !files.contains(&path) {
                files.push(path);
            }
        }
    }
    files
}

/// `name` resolved against `folder`, lexically, when it is a plain path that stays inside it.
fn inside(folder: &Path, name: &str) -> Option<PathBuf> {
    let name = name.trim();
    let dynamic = ['$', '%', '<', '>', '{', '}', '\0'];
    if name.is_empty() || name.contains("://") || name.contains(dynamic) {
        return None;
    }
    let mut path = folder.to_path_buf();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            // Leaving the folder, or a path of its own: not the page's to read.
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (path != folder).then_some(path)
}

/// A value as written, without the quotes around it.
fn unquoted(value: &str) -> &str {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

/// `!include`, `!include_many`, `!include_once`, `!includesub` and `!import` with a file, and
/// `!theme <name> from <folder>`.
fn plantuml(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in source.lines() {
        let Some(directive) = line.trim().strip_prefix('!') else {
            continue;
        };
        let (keyword, argument) = directive
            .split_once(char::is_whitespace)
            .unwrap_or((directive, ""));
        let argument = argument.trim();
        match keyword {
            "include" | "include_many" | "include_once" | "includesub" => {
                // `file!1` and `file!PART` name a diagram or part within the file.
                let file = match argument.strip_prefix('"') {
                    Some(quoted) => quoted.split('"').next().unwrap_or_default(),
                    None => argument.split('!').next().unwrap_or_default(),
                };
                names.push(file.to_owned());
            }
            "import" => names.push(unquoted(argument).to_owned()),
            "theme" => {
                if let Some((theme, folder)) = argument.split_once(" from ") {
                    let (theme, folder) = (unquoted(theme), unquoted(folder));
                    if !theme.is_empty() && !folder.is_empty() {
                        names.push(format!("{folder}/puml-theme-{theme}.puml"));
                    }
                }
            }
            _ => {}
        }
    }
    names
}

/// The values of `image` and `shapefile` attributes.
fn graphviz(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for attribute in ["image", "shapefile"] {
        for value in attribute_values(source, attribute, '=') {
            names.push(value);
        }
    }
    names
}

/// The files of `@import`s (`x: @file` and `...@file`, `.d2` when no extension is given) and
/// local `icon` paths.
fn d2(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (at, _) in source.match_indices('@') {
        let before = source[..at].trim_end_matches([' ', '\t']);
        if !(before.ends_with(':') || before.ends_with("...")) {
            continue;
        }
        let rest = &source[at + 1..];
        let end = rest
            .find(|character: char| character.is_whitespace() || ";}],".contains(character))
            .unwrap_or(rest.len());
        let file = unquoted(&rest[..end]);
        if file.is_empty() {
            continue;
        }
        let has_extension = Path::new(file).extension().is_some();
        names.push(if has_extension {
            file.to_owned()
        } else {
            format!("{file}.d2")
        });
    }
    names.extend(attribute_values(source, "icon", ':'));
    names
}

/// The values of `attribute` in `source`, each written `attribute <separator> value`, the value
/// quoted or a run of path characters. The attribute's name must stand alone.
fn attribute_values(source: &str, attribute: &str, separator: char) -> Vec<String> {
    let mut values = Vec::new();
    for (start, _) in source.match_indices(attribute) {
        let joined = |character: char| character.is_alphanumeric() || character == '_';
        if source[..start].chars().next_back().is_some_and(joined) {
            continue;
        }
        let Some(rest) = source[start + attribute.len()..]
            .trim_start()
            .strip_prefix(separator)
        else {
            continue;
        };
        let rest = rest.trim_start_matches([' ', '\t']);
        let value = match rest.strip_prefix('"') {
            Some(quoted) => match quoted.find('"') {
                Some(end) => &quoted[..end],
                None => continue,
            },
            None => {
                let end = rest
                    .find(|character: char| {
                        !(character.is_alphanumeric() || "._-/\\~+:".contains(character))
                    })
                    .unwrap_or(rest.len());
                &rest[..end]
            }
        };
        if !value.is_empty() {
            values.push(value.to_owned());
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_read_from_each_language() {
        assert_eq!(
            graphviz(
                "a [image=\"img/a.png\"]; b [imagepath=x, image = b.svg, shapefile=\"c.png\"]"
            ),
            ["img/a.png", "b.svg", "c.png"]
        );
        assert_eq!(
            d2("a: @shared\n...@more.d2\nb.icon: \"./i.svg\"\nc: { icon: https://x/i.svg }\nd: user@example\n"),
            ["shared.d2", "more.d2", "./i.svg", "https://x/i.svg"]
        );
        // Half-typed input names nothing and does not panic.
        for source in [
            "!include",
            "!theme x from",
            "image=",
            "image=\"",
            "icon:",
            "a: @",
            "@",
        ] {
            for kind in ["plantuml", "graphviz", "d2"] {
                assert_eq!(
                    files_read(kind, source, Path::new("/page")),
                    Vec::<PathBuf>::new()
                );
            }
        }
    }
}
