//! `[docbook]`: DocBook export's defaults, and `[docbook.profiles.<name>]`, publications saved
//! under a name (DocBook PRD §5.5). Built without the `docbook` feature too, so a settings file
//! naming them reads the same in every build. Nothing here is read from `[pdf]` or any other
//! format's table, and a profile repeats its content (`page`, `component` or `nav`) rather than
//! naming another format's profile, as Word, Kindle and EPUB profiles do.

use std::path::{Path, PathBuf};

use crate::profiles::{profile_attributes, profile_content, profile_target, profile_text};
use crate::{DocBookAssets, DocBookExportArgs, Engine, EngineError, FileAccess, ProfileContent};

/// A DocBook publication saved under a name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocBookProfile {
    pub name: String,
    pub content: ProfileContent,
    /// Where the document is written, relative to the first workspace root and inside it, an
    /// `.xml` file; `<name>.xml` there when unset. Its pictures go in the folder beside it
    /// named after it.
    pub output: Option<String>,
    /// Attributes every page is read with before its own header.
    pub attributes: Vec<(String, String)>,
    /// How the document refers to its pictures, in place of `[docbook] assets`.
    pub assets: Option<DocBookAssets>,
}

/// The `[docbook]` table.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocBookSettings {
    /// How documents refer to their pictures: `copy` (the default) or `reference`.
    pub assets: Option<DocBookAssets>,
    pub profiles: Vec<DocBookProfile>,
}

const PROFILE_KEYS: &[&str] = &["page", "component", "nav", "output", "attributes", "assets"];

fn assets(value: &toml::Value) -> Result<DocBookAssets, String> {
    value
        .as_str()
        .and_then(DocBookAssets::from_name)
        .ok_or_else(|| "expected \"copy\" or \"reference\"".to_owned())
}

/// A profile from its `[docbook.profiles.<name>]` table.
pub(crate) fn parse_profile(name: &str, table: &toml::Table) -> Result<DocBookProfile, String> {
    if let Some(unknown) = table
        .keys()
        .find(|key| !PROFILE_KEYS.contains(&key.as_str()))
    {
        return Err(format!("unknown key `{unknown}`"));
    }
    let output = profile_text(table, "output")?;
    if let Some(output) = &output {
        let xml = Path::new(output)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("xml"));
        if !xml {
            return Err(format!("`output`: {output} is not an .xml file"));
        }
    }
    let assets = table
        .get("assets")
        .map(assets)
        .transpose()
        .map_err(|problem| format!("`assets`: {problem}"))?;
    Ok(DocBookProfile {
        name: name.to_owned(),
        content: profile_content(table)?,
        output,
        attributes: profile_attributes(table)?,
        assets,
    })
}

/// Applies `[docbook] <key> = value`, returning the problems as `(setting, problem)`.
pub(crate) fn apply(
    settings: &mut DocBookSettings,
    key: &str,
    value: &toml::Value,
) -> Vec<(String, String)> {
    let mut problems = Vec::new();
    let name = format!("docbook.{key}");
    match key {
        "assets" => match assets(value) {
            Ok(policy) => settings.assets = Some(policy),
            Err(problem) => problems.push((name, problem)),
        },
        "profiles" => match value.as_table() {
            None => problems.push((name, "expected a table of profiles".to_owned())),
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
                            problems.push((format!("docbook.profiles.{profile_name}"), problem));
                        }
                    }
                }
            }
        },
        _ => problems.push((name, "unknown setting".to_owned())),
    }
    problems
}

impl Engine {
    /// The DocBook export a `[docbook.profiles.<name>]` profile describes, every path resolved
    /// against the first workspace root, its output inside the workspace.
    pub fn docbook_profile_export_args(
        &self,
        name: &str,
    ) -> Result<DocBookExportArgs, EngineError> {
        let profile = self
            .settings
            .docbook
            .profiles
            .iter()
            .find(|profile| profile.name == name)
            .ok_or_else(|| EngineError::NotFound(format!("no DocBook profile `{name}`")))?;
        let root = self
            .roots
            .first()
            .ok_or_else(|| EngineError::Refused("no workspace is open".to_owned()))?;
        let inside = FileAccess::within(vec![root.clone()]);
        let output: PathBuf = crate::normalize_path(
            &root.join(
                profile
                    .output
                    .clone()
                    .unwrap_or_else(|| format!("{}.xml", profile.name)),
            ),
        );
        if !inside.permits(&output) {
            return Err(EngineError::Refused(format!(
                "DocBook profile `{name}` names the output {}, which is outside the workspace",
                output.display()
            )));
        }
        Ok(DocBookExportArgs {
            target: profile_target(&profile.content, root),
            output,
            attributes: profile.attributes.clone(),
            assets: profile.assets,
        })
    }

    /// What would stop the DocBook profile `name` from exporting, found without exporting: an
    /// unknown profile, an output outside the workspace, content that is not in it, or an
    /// assets folder that is not the export's own.
    #[must_use]
    pub fn docbook_profile_problems(&self, name: &str) -> Vec<String> {
        match self.docbook_profile_export_args(name) {
            Ok(args) => {
                let problems = self.target_problems(&args.target);
                #[cfg(feature = "docbook")]
                let problems = {
                    let mut problems = problems;
                    if let Err(problem) = crate::export::check_docbook_output(&args.output) {
                        problems.push(problem.to_string());
                    }
                    problems
                };
                problems
            }
            Err(error) => vec![error.to_string()],
        }
    }

    /// The names of the DocBook profiles, in order.
    #[must_use]
    pub fn docbook_profiles(&self) -> Vec<String> {
        self.settings
            .docbook
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
    fn profiles_name_their_content_once_and_an_xml_output() {
        let profile = parse_profile(
            "guide",
            &table(
                "component = \"1.0@guide\"\noutput = \"build/guide.xml\"\nassets = \"Reference\"",
            ),
        )
        .unwrap();
        assert_eq!(profile.output.as_deref(), Some("build/guide.xml"));
        assert_eq!(profile.assets, Some(DocBookAssets::Reference));
        assert!(parse_profile("x", &table("output = \"a.xml\"")).is_err());
        assert!(parse_profile("x", &table("page = \"a.adoc\"\ncomponent = \"g\"")).is_err());
        let wrong = parse_profile("x", &table("page = \"a.adoc\"\noutput = \"a.dbk\""));
        assert!(wrong.unwrap_err().contains("not an .xml file"));
        let wrong = parse_profile("x", &table("page = \"a.adoc\"\nassets = \"link\""));
        assert!(wrong.unwrap_err().contains("\"copy\" or \"reference\""));
        assert!(parse_profile("x", &table("page = \"a.adoc\"\ntheme = \"x\"")).is_err());
    }

    #[test]
    fn the_table_takes_its_keys_only() {
        let mut settings = DocBookSettings::default();
        assert!(apply(&mut settings, "assets", &toml::Value::String("copy".into())).is_empty());
        assert_eq!(settings.assets, Some(DocBookAssets::Copy));
        assert_eq!(
            apply(&mut settings, "assets", &toml::Value::String("move".into()))[0].0,
            "docbook.assets"
        );
        assert_eq!(
            apply(&mut settings, "theme", &toml::Value::String("x".into()))[0].1,
            "unknown setting"
        );
    }
}
