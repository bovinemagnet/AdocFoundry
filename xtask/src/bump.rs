//! `cargo xtask bump <version>`: the version written into every file the release checks
//! (`packaging/release-version.py check`): `Cargo.toml`'s `[workspace.package]`, the
//! documentation's `display_version`, and the changelog's `## Unreleased`, which becomes
//! `## <version> — <date>`. `Cargo.lock` is then brought up to date.

use std::{
    fs,
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

/// Writes `version` into the release's files under `root`, dated today (UTC).
pub fn run(root: &Path, version: &str) -> Result<(), String> {
    if !is_version(version) {
        return Err(format!(
            "`{version}` is not a version like 0.5.0 or 0.5.0-rc.1"
        ));
    }
    let date = today();
    // Every file is read and edited before any is written.
    let edited = |name: &str, edit: &dyn Fn(&str) -> Result<String, String>| {
        let path = root.join(name);
        let text = fs::read_to_string(&path).map_err(|error| format!("{name}: {error}"))?;
        let text = edit(&text).map_err(|error| format!("{name}: {error}"))?;
        Ok::<_, String>((path, text))
    };
    let written = [
        edited("Cargo.toml", &|text| cargo(text, version))?,
        edited("src/docs/antora.yml", &|text| antora(text, version))?,
        edited("CHANGELOG.md", &|text| changelog(text, version, &date))?,
    ];
    for (path, text) in written {
        fs::write(&path, text).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["update", "--workspace", "--offline"])
        .current_dir(root)
        .status()
        .map_err(|error| format!("cargo update: {error}"))?;
    if !status.success() {
        return Err("cargo update --workspace failed; Cargo.lock is not updated".to_owned());
    }
    Ok(())
}

/// `MAJOR.MINOR.PATCH`, optionally followed by `-` and a pre-release.
fn is_version(version: &str) -> bool {
    let (release, pre) = match version.split_once('-') {
        Some((release, pre)) => (release, Some(pre)),
        None => (version, None),
    };
    let numbers: Vec<&str> = release.split('.').collect();
    numbers.len() == 3
        && numbers
            .iter()
            .all(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        && pre.is_none_or(|pre| {
            !pre.is_empty()
                && pre
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
        })
}

/// `Cargo.toml` with `[workspace.package]`'s `version` set to `version`.
fn cargo(text: &str, version: &str) -> Result<String, String> {
    let mut table = String::new();
    let mut done = false;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            table = trimmed[1..trimmed.len() - 1].trim().to_owned();
        } else if !done
            && table == "workspace.package"
            && trimmed.starts_with("version")
            && trimmed["version".len()..].trim_start().starts_with('=')
        {
            let ending = &line[line.trim_end().len()..];
            out.push_str(&format!("version = \"{version}\"{ending}"));
            done = true;
            continue;
        }
        out.push_str(line);
    }
    if done {
        Ok(out)
    } else {
        Err("no `version` in `[workspace.package]`".to_owned())
    }
}

/// The documentation's `antora.yml` with `display_version` set to `version`.
fn antora(text: &str, version: &str) -> Result<String, String> {
    let mut done = false;
    let out: String = text
        .split_inclusive('\n')
        .map(|line| {
            if !done && line.starts_with("display_version:") {
                done = true;
                let ending = &line[line.trim_end().len()..];
                format!("display_version: '{version}'{ending}")
            } else {
                line.to_owned()
            }
        })
        .collect();
    if done {
        Ok(out)
    } else {
        Err("no `display_version`".to_owned())
    }
}

/// The changelog with `## Unreleased` headed `## <version> — <date>` instead.
fn changelog(text: &str, version: &str, date: &str) -> Result<String, String> {
    if text
        .lines()
        .any(|line| line.starts_with(&format!("## {version} ")))
    {
        return Err(format!("there is a `## {version}` section already"));
    }
    let mut done = false;
    let out: String = text
        .split_inclusive('\n')
        .map(|line| {
            if !done && line.trim_end() == "## Unreleased" {
                done = true;
                let ending = &line[line.trim_end().len()..];
                format!("## {version} — {date}{ending}")
            } else {
                line.to_owned()
            }
        })
        .collect();
    if done {
        Ok(out)
    } else {
        Err("no `## Unreleased` section to release".to_owned())
    }
}

/// Today's date in UTC, `YYYY-MM-DD`.
fn today() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_workspace_package_version_changes() {
        let text = "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.package]\nversion = \"0.4.0\"\nedition = \"2021\"\n\n[workspace.dependencies]\nserde = { version = \"=1.0.0\" }\n";
        assert_eq!(
            cargo(text, "0.5.0").unwrap(),
            text.replace("version = \"0.4.0\"", "version = \"0.5.0\"")
        );
        assert!(cargo("[package]\nversion = \"0.4.0\"\n", "0.5.0").is_err());
    }

    #[test]
    fn the_documentation_shows_the_version() {
        let text =
            "name: adocfoundry\nversion: ~\n# The release.\ndisplay_version: '0.4.0'\nnav:\n";
        assert_eq!(
            antora(text, "0.5.0").unwrap(),
            "name: adocfoundry\nversion: ~\n# The release.\ndisplay_version: '0.5.0'\nnav:\n"
        );
        assert!(antora("name: x\n", "0.5.0").is_err());
    }

    #[test]
    fn unreleased_becomes_the_version_and_date() {
        let text = "# Changelog\n\n## Unreleased\n\n- New.\n\n## 0.4.0 — 2026-10-27\n\n- Old.\n";
        assert_eq!(
            changelog(text, "0.5.0", "2026-11-02").unwrap(),
            "# Changelog\n\n## 0.5.0 — 2026-11-02\n\n- New.\n\n## 0.4.0 — 2026-10-27\n\n- Old.\n"
        );
        assert!(
            changelog(text, "0.4.0", "2026-11-02").is_err(),
            "released already"
        );
        assert!(changelog("# Changelog\n", "0.5.0", "2026-11-02").is_err());
    }

    #[test]
    fn versions_are_checked() {
        for good in ["0.5.0", "1.10.0", "0.5.0-rc.1"] {
            assert!(is_version(good), "{good}");
        }
        for bad in ["v0.5.0", "0.5", "0.5.0.1", "0.5.x", "0.5.0-", ""] {
            assert!(!is_version(bad), "{bad}");
        }
    }

    #[test]
    fn dates_are_counted_from_1970() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_757), (2026, 10, 31));
    }
}
