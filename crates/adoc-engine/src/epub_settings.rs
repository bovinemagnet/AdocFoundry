//! `[epub]`: EPUB export's defaults, and `[epub.profiles.<name>]`, publications saved under a
//! name (EPUB PRD §5.7). Built without the `epub` feature too, so a settings file naming them
//! reads the same in every build. Nothing here is read from `[pdf]`, `[docx]` or `[kindle]`,
//! and a profile repeats its content (`page`, `component` or `nav`) rather than naming another
//! format's profile, as a Word or Kindle profile does.

use std::path::{Path, PathBuf};

use crate::profiles::{profile_attributes, profile_content, profile_target, profile_text};
use crate::{Engine, EngineError, EpubExportArgs, FileAccess, ProfileContent};

/// An EPUB publication saved under a name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpubProfile {
    pub name: String,
    pub content: ProfileContent,
    /// Where the book is written, relative to the first workspace root and inside it, an
    /// `.epub` file; `<name>.epub` there when unset.
    pub output: Option<String>,
    /// Attributes every page is read with before its own header.
    pub attributes: Vec<(String, String)>,
    pub title: Option<String>,
    pub creator: Option<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    pub publisher: Option<String>,
    /// The cover picture, relative to the first workspace root.
    pub cover: Option<String>,
    /// A stylesheet of the user's, relative to the first workspace root.
    pub css: Option<String>,
    /// When the book was last modified, in seconds since the Unix epoch.
    pub modified: Option<i64>,
}

/// The `[epub]` table.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EpubSettings {
    pub title: Option<String>,
    pub creator: Option<String>,
    pub language: Option<String>,
    pub identifier: Option<String>,
    pub publisher: Option<String>,
    /// The cover picture, relative to the first workspace root.
    pub cover: Option<String>,
    /// A stylesheet of the user's, relative to the first workspace root, linked after the
    /// default one.
    pub css: Option<String>,
    /// When every book is dated, in seconds since the Unix epoch, for books built again and
    /// again to be the same bytes.
    pub modified: Option<i64>,
    pub profiles: Vec<EpubProfile>,
}

const PROFILE_KEYS: &[&str] = &[
    "page",
    "component",
    "nav",
    "output",
    "attributes",
    "title",
    "creator",
    "language",
    "identifier",
    "publisher",
    "cover",
    "css",
    "modified",
];

fn text(value: &toml::Value) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_owned)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| "expected a string".to_owned())
}

/// The days from 1970-01-01 to the civil date, as `adoc_core::civil_from_days` reverses.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// A `modified` time: a date, `2026-11-19`, at midnight UTC, or a time in UTC,
/// `2026-11-19T10:30:00Z`, as seconds since the Unix epoch. Only a time an EPUB can be dated
/// with, 1980 to 2107, is taken.
pub(crate) fn modified(value: &toml::Value) -> Result<i64, String> {
    let expected = || {
        "expected a date such as \"2026-11-19\" or a UTC time such as \"2026-11-19T10:30:00Z\", \
         from 1980 to 2107"
            .to_owned()
    };
    let text = value.as_str().ok_or_else(expected)?.trim();
    let (date, time) = match text.split_once('T') {
        Some((date, time)) => (date, Some(time.strip_suffix('Z').ok_or_else(expected)?)),
        None => (text, None),
    };
    let number = |part: &str, length: usize| -> Result<i64, String> {
        if part.len() != length || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(expected());
        }
        part.parse().map_err(|_| expected())
    };
    let parts: Vec<&str> = date.split('-').collect();
    let [year, month, day] = parts.as_slice() else {
        return Err(expected());
    };
    let (year, month, day) = (number(year, 4)?, number(month, 2)?, number(day, 2)?);
    let (hour, minute, second) = match time {
        None => (0, 0, 0),
        Some(time) => {
            let parts: Vec<&str> = time.split(':').collect();
            let [hour, minute, second] = parts.as_slice() else {
                return Err(expected());
            };
            (number(hour, 2)?, number(minute, 2)?, number(second, 2)?)
        }
    };
    let days = days_from_civil(year, month, day);
    // A date that does not exist, such as 2026-02-30, is not one written back the same.
    if !(1..=12).contains(&month)
        || adoc_core::civil_from_days(days) != (year, month as u8, day as u8)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(expected());
    }
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
    if !(315_532_800..=4_354_819_199).contains(&seconds) {
        return Err(expected());
    }
    Ok(seconds)
}

/// A profile from its `[epub.profiles.<name>]` table.
pub(crate) fn parse_profile(name: &str, table: &toml::Table) -> Result<EpubProfile, String> {
    if let Some(unknown) = table
        .keys()
        .find(|key| !PROFILE_KEYS.contains(&key.as_str()))
    {
        return Err(format!("unknown key `{unknown}`"));
    }
    let output = profile_text(table, "output")?;
    if let Some(output) = &output {
        let epub = Path::new(output)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("epub"));
        if !epub {
            return Err(format!("`output`: {output} is not an .epub file"));
        }
    }
    let modified = table
        .get("modified")
        .map(modified)
        .transpose()
        .map_err(|problem| format!("`modified`: {problem}"))?;
    Ok(EpubProfile {
        name: name.to_owned(),
        content: profile_content(table)?,
        output,
        attributes: profile_attributes(table)?,
        title: profile_text(table, "title")?,
        creator: profile_text(table, "creator")?,
        language: profile_text(table, "language")?,
        identifier: profile_text(table, "identifier")?,
        publisher: profile_text(table, "publisher")?,
        cover: profile_text(table, "cover")?,
        css: profile_text(table, "css")?,
        modified,
    })
}

/// Applies `[epub] <key> = value`, returning the problems as `(setting, problem)`.
pub(crate) fn apply(
    settings: &mut EpubSettings,
    key: &str,
    value: &toml::Value,
) -> Vec<(String, String)> {
    let mut problems = Vec::new();
    let name = format!("epub.{key}");
    let mut set = |result: Result<(), String>| {
        if let Err(problem) = result {
            problems.push((name.clone(), problem));
        }
    };
    match key {
        // The one style there is, for now; named so a settings file can say so.
        "style" => set(match value.as_str() {
            Some("default") => Ok(()),
            _ => Err("expected \"default\"".to_owned()),
        }),
        "title" => set(text(value).map(|title| settings.title = Some(title))),
        "creator" => set(text(value).map(|creator| settings.creator = Some(creator))),
        "language" => set(text(value).map(|language| settings.language = Some(language))),
        "identifier" => set(text(value).map(|identifier| settings.identifier = Some(identifier))),
        "publisher" => set(text(value).map(|publisher| settings.publisher = Some(publisher))),
        "cover" => set(text(value).map(|cover| settings.cover = Some(cover))),
        "css" => set(text(value).map(|css| settings.css = Some(css))),
        "modified" => set(modified(value).map(|seconds| settings.modified = Some(seconds))),
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
                            problems.push((format!("epub.profiles.{profile_name}"), problem));
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
    /// The EPUB export a `[epub.profiles.<name>]` profile describes, every path resolved
    /// against the first workspace root, its output, cover and stylesheet inside the workspace.
    pub fn epub_profile_export_args(&self, name: &str) -> Result<EpubExportArgs, EngineError> {
        let profile = self
            .settings
            .epub
            .profiles
            .iter()
            .find(|profile| profile.name == name)
            .ok_or_else(|| EngineError::NotFound(format!("no EPUB profile `{name}`")))?;
        let root = self
            .roots
            .first()
            .ok_or_else(|| EngineError::Refused("no workspace is open".to_owned()))?;
        let inside = FileAccess::within(vec![root.clone()]);
        let resolve = |what: &str, relative: &str| -> Result<PathBuf, EngineError> {
            let path = crate::normalize_path(&root.join(relative));
            if inside.permits(&path) {
                Ok(path)
            } else {
                Err(EngineError::Refused(format!(
                    "EPUB profile `{name}` names {what} {}, which is outside the workspace",
                    path.display()
                )))
            }
        };
        let output = resolve(
            "the output",
            &profile
                .output
                .clone()
                .unwrap_or_else(|| format!("{}.epub", profile.name)),
        )?;
        let cover = profile
            .cover
            .as_deref()
            .map(|cover| resolve("the cover", cover))
            .transpose()?;
        let css = profile
            .css
            .as_deref()
            .map(|css| resolve("the stylesheet", css))
            .transpose()?;
        Ok(EpubExportArgs {
            target: profile_target(&profile.content, root),
            output,
            attributes: profile.attributes.clone(),
            title: profile.title.clone(),
            creator: profile.creator.clone(),
            language: profile.language.clone(),
            identifier: profile.identifier.clone(),
            publisher: profile.publisher.clone(),
            cover,
            css,
            modified: profile.modified,
        })
    }

    /// What would stop the EPUB profile `name` from exporting, found without exporting: an
    /// unknown profile, a path outside the workspace, a missing cover or stylesheet, or content
    /// that is not in it.
    #[must_use]
    pub fn epub_profile_problems(&self, name: &str) -> Vec<String> {
        match self.epub_profile_export_args(name) {
            Ok(args) => {
                let mut problems = self.target_problems(&args.target);
                if let Some(cover) = &args.cover {
                    if !cover.is_file() {
                        problems.push(format!("its cover {} is missing", cover.display()));
                    }
                }
                if let Some(css) = &args.css {
                    if !css.is_file() {
                        problems.push(format!("its stylesheet {} is missing", css.display()));
                    }
                }
                problems
            }
            Err(error) => vec![error.to_string()],
        }
    }

    /// The names of the EPUB profiles, in order.
    #[must_use]
    pub fn epub_profiles(&self) -> Vec<String> {
        self.settings
            .epub
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
    fn profiles_name_their_content_once_and_an_epub_output() {
        let profile = parse_profile(
            "guide",
            &table(
                "component = \"1.0@guide\"\noutput = \"build/guide.epub\"\ntitle = \"Guide\"\n\
                 css = \"style/reader.css\"\nmodified = \"2026-11-19\"",
            ),
        )
        .unwrap();
        assert_eq!(profile.title.as_deref(), Some("Guide"));
        assert_eq!(profile.css.as_deref(), Some("style/reader.css"));
        assert_eq!(profile.modified, Some(1_795_046_400));
        assert!(parse_profile("x", &table("output = \"a.epub\"")).is_err());
        assert!(parse_profile("x", &table("page = \"a.adoc\"\ncomponent = \"g\"")).is_err());
        let wrong = parse_profile("x", &table("page = \"a.adoc\"\noutput = \"a.azw3\""));
        assert!(wrong.unwrap_err().contains("not an .epub file"));
        assert!(parse_profile("x", &table("page = \"a.adoc\"\nformat = \"kfx\"")).is_err());
        assert!(parse_profile("x", &table("page = \"a.adoc\"\nmodified = \"soon\"")).is_err());
    }

    #[test]
    fn modified_times_are_dates_or_utc_times_an_epub_can_hold() {
        let value = |text: &str| modified(&toml::Value::String(text.to_owned()));
        assert_eq!(value("2026-11-19"), Ok(1_795_046_400));
        assert_eq!(value("2026-11-19T10:30:00Z"), Ok(1_795_084_200));
        assert_eq!(value("1980-01-01"), Ok(315_532_800));
        for bad in [
            "2026-11-19T10:30:00",
            "2026-02-30",
            "2026-13-01",
            "1979-12-31",
            "2108-01-01",
            "26-11-19",
            "2026-11-19T24:00:00Z",
            "yesterday",
        ] {
            assert!(value(bad).is_err(), "{bad}");
        }
        assert!(modified(&toml::Value::Integer(1)).is_err());
    }
}
