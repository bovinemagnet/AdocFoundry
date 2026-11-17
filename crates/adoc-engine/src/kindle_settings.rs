//! `[kindle]`: Kindle export's defaults, and `[kindle.profiles.<name>]`, publications saved
//! under a name (Kindle PRD §6.6). Built without the `kindle` feature too, so a settings file
//! naming them reads the same in every build. Nothing here is read from `[pdf]` or `[docx]`,
//! and a profile repeats its content (`page`, `component` or `nav`) rather than naming another
//! format's profile, as a Word profile does.

use std::path::Path;

use crate::profiles::{profile_attributes, profile_content, profile_target, profile_text};
use crate::{Engine, EngineError, FileAccess, KindleExportArgs, KindleFormat, ProfileContent};

/// A Kindle publication saved under a name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KindleProfile {
    pub name: String,
    pub content: ProfileContent,
    /// Its own format, else `[kindle] format`'s.
    pub format: Option<KindleFormat>,
    /// Where the book is written, relative to the first workspace root and inside it, its
    /// extension the format's; `<name>.<format>` there when unset.
    pub output: Option<String>,
    /// Attributes every page is read with before its own header.
    pub attributes: Vec<(String, String)>,
    pub title: Option<String>,
    pub creator: Option<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    /// The cover picture, relative to the first workspace root.
    pub cover: Option<String>,
}

/// The `[kindle]` table.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KindleSettings {
    /// The format an export takes unless it or its profile names one: AZW3 unless set.
    pub format: KindleFormat,
    pub title: Option<String>,
    pub creator: Option<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    pub publisher: Option<String>,
    /// The cover picture, relative to the first workspace root.
    pub cover: Option<String>,
    pub profiles: Vec<KindleProfile>,
}

const PROFILE_KEYS: &[&str] = &[
    "page",
    "component",
    "nav",
    "format",
    "output",
    "attributes",
    "title",
    "creator",
    "language",
    "identifier",
    "cover",
];

fn format(value: &toml::Value) -> Result<KindleFormat, String> {
    value
        .as_str()
        .and_then(KindleFormat::from_name)
        .ok_or_else(|| "expected \"azw3\" or \"kfx\"".to_owned())
}

fn text(value: &toml::Value) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_owned)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| "expected a string".to_owned())
}

/// A profile from its `[kindle.profiles.<name>]` table.
pub(crate) fn parse_profile(name: &str, table: &toml::Table) -> Result<KindleProfile, String> {
    if let Some(unknown) = table
        .keys()
        .find(|key| !PROFILE_KEYS.contains(&key.as_str()))
    {
        return Err(format!("unknown key `{unknown}`"));
    }
    let profile_format = table
        .get("format")
        .map(format)
        .transpose()
        .map_err(|problem| format!("`format`: {problem}"))?;
    let output = profile_text(table, "output")?;
    if let Some(output) = &output {
        let named = Path::new(output)
            .extension()
            .and_then(|extension| extension.to_str())
            .and_then(KindleFormat::from_name);
        match (named, profile_format) {
            (None, _) => return Err(format!("`output`: {output} is not a .azw3 or .kfx file")),
            (Some(named), Some(asked)) if named != asked => {
                return Err(format!(
                    "`output`: {output} is not a .{} file, as `format` asks",
                    asked.extension()
                ));
            }
            _ => {}
        }
    }
    Ok(KindleProfile {
        name: name.to_owned(),
        content: profile_content(table)?,
        format: profile_format,
        output,
        attributes: profile_attributes(table)?,
        title: profile_text(table, "title")?,
        creator: profile_text(table, "creator")?,
        language: profile_text(table, "language")?,
        identifier: profile_text(table, "identifier")?,
        cover: profile_text(table, "cover")?,
    })
}

/// Applies `[kindle] <key> = value`, returning the problems as `(setting, problem)`.
pub(crate) fn apply(
    settings: &mut KindleSettings,
    key: &str,
    value: &toml::Value,
) -> Vec<(String, String)> {
    let mut problems = Vec::new();
    let name = format!("kindle.{key}");
    let mut set = |result: Result<(), String>| {
        if let Err(problem) = result {
            problems.push((name.clone(), problem));
        }
    };
    match key {
        "format" => set(format(value).map(|format| settings.format = format)),
        "title" => set(text(value).map(|title| settings.title = Some(title))),
        "creator" => set(text(value).map(|creator| settings.creator = Some(creator))),
        "language" => set(text(value).map(|language| settings.language = Some(language))),
        "identifier" => set(text(value).map(|identifier| settings.identifier = Some(identifier))),
        "publisher" => set(text(value).map(|publisher| settings.publisher = Some(publisher))),
        "cover" => set(text(value).map(|cover| settings.cover = Some(cover))),
        "profiles" => match value.as_table() {
            None => set(Err("expected a table of profiles".to_owned())),
            Some(profiles) => {
                for (profile_name, table) in profiles {
                    let parsed = table
                        .as_table()
                        .ok_or_else(|| "expected a table".to_owned())
                        .and_then(|table| parse_profile(profile_name, table));
                    match parsed {
                        Ok(profile) => {
                            settings.profiles.retain(|known| known.name != profile.name);
                            settings.profiles.push(profile);
                            settings
                                .profiles
                                .sort_by(|left, right| left.name.cmp(&right.name));
                        }
                        Err(problem) => {
                            problems.push((format!("kindle.profiles.{profile_name}"), problem));
                        }
                    }
                }
            }
        },
        _ => set(Err("unknown setting".to_owned())),
    }
    problems
}

impl Engine {
    /// The Kindle export a `[kindle.profiles.<name>]` profile describes, every path resolved
    /// against the first workspace root, its output inside the workspace.
    pub fn kindle_profile_export_args(&self, name: &str) -> Result<KindleExportArgs, EngineError> {
        let profile = self
            .settings
            .kindle
            .profiles
            .iter()
            .find(|profile| profile.name == name)
            .ok_or_else(|| EngineError::NotFound(format!("no Kindle profile `{name}`")))?;
        let root = self
            .roots
            .first()
            .ok_or_else(|| EngineError::Refused("no workspace is open".to_owned()))?;
        let format = profile.format.unwrap_or(self.settings.kindle.format);
        let output = crate::normalize_path(
            &root.join(
                profile
                    .output
                    .clone()
                    .unwrap_or_else(|| format!("{}.{}", profile.name, format.extension())),
            ),
        );
        let format = profile
            .output
            .as_deref()
            .and_then(|output| Path::new(output).extension())
            .and_then(|extension| extension.to_str())
            .and_then(KindleFormat::from_name)
            .unwrap_or(format);
        // As a PDF or Word profile's: the workspace's own files write only inside it.
        if !FileAccess::within(vec![root.clone()]).permits(&output) {
            return Err(EngineError::Refused(format!(
                "Kindle profile `{name}` writes {}, which is outside the workspace",
                output.display()
            )));
        }
        Ok(KindleExportArgs {
            target: profile_target(&profile.content, root),
            format,
            output,
            attributes: profile.attributes.clone(),
            title: profile.title.clone(),
            creator: profile.creator.clone(),
            language: profile.language.clone(),
            identifier: profile.identifier.clone(),
            cover: profile
                .cover
                .as_deref()
                .map(|cover| crate::normalize_path(&root.join(cover))),
        })
    }

    /// What would stop the Kindle profile `name` from exporting, found without exporting: an
    /// unknown profile, an output outside the workspace, or content that is not in it.
    #[must_use]
    pub fn kindle_profile_problems(&self, name: &str) -> Vec<String> {
        match self.kindle_profile_export_args(name) {
            Ok(args) => {
                let mut problems = self.target_problems(&args.target);
                if let Some(cover) = &args.cover {
                    if !cover.is_file() {
                        problems.push(format!("its cover {} is missing", cover.display()));
                    }
                }
                problems
            }
            Err(error) => vec![error.to_string()],
        }
    }

    /// The names of the Kindle profiles, in order.
    #[must_use]
    pub fn kindle_profiles(&self) -> Vec<String> {
        self.settings
            .kindle
            .profiles
            .iter()
            .map(|profile| profile.name.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> toml::Table {
        text.parse().unwrap()
    }

    #[test]
    fn profiles_name_their_content_once_and_an_output_of_their_format() {
        let profile = parse_profile(
            "handbook",
            &table("component = \"1.0@guide\"\nformat = \"kfx\"\noutput = \"build/hb.kfx\"\ntitle = \"Handbook\""),
        )
        .unwrap();
        assert_eq!(profile.format, Some(KindleFormat::Kfx));
        assert_eq!(profile.title.as_deref(), Some("Handbook"));
        assert!(parse_profile("x", &table("format = \"kfx\"")).is_err());
        assert!(parse_profile("x", &table("page = \"a.adoc\"\ncomponent = \"g\"")).is_err());
        let wrong = parse_profile(
            "x",
            &table("page = \"a.adoc\"\nformat = \"kfx\"\noutput = \"a.azw3\""),
        );
        assert!(wrong.unwrap_err().contains("not a .kfx file"));
        assert!(parse_profile("x", &table("page = \"a.adoc\"\noutput = \"a.epub\"")).is_err());
        assert!(parse_profile("x", &table("page = \"a.adoc\"\nformat = \"mobi\"")).is_err());
        assert!(parse_profile("x", &table("page = \"a.adoc\"\ntheme = \"t\"")).is_err());
    }
}
