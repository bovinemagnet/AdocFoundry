//! Updates: reading the releases feed, deciding whether a check is due and whether a release is
//! worth announcing, and remembering both between runs. The engine makes no request; the app
//! fetches the feed and hands its text here.

use std::{
    cmp::Ordering,
    fmt,
    path::{Path, PathBuf},
};

/// The latest release, drafts and pre-releases excluded.
pub const FEED: &str = "https://api.github.com/repos/bovinemagnet/AdocFoundry/releases/latest";

/// The file beside the user settings file that remembers the last check.
pub const STATE_FILE: &str = "update-state.toml";

/// How long after a check the next one is due, in seconds.
pub const CHECK_INTERVAL: u64 = 24 * 60 * 60;

/// A release's version: `major.minor.patch`, with an optional pre-release, `v` prefix allowed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Option<String>,
}

impl Version {
    /// `text` as a version, such as `0.3.0`, `v0.3.0` or `0.3.0-beta.1`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        // Build metadata never orders versions.
        let text = text.split('+').next()?;
        let (numbers, pre) = match text.split_once('-') {
            Some((numbers, pre)) if !pre.is_empty() => (numbers, Some(pre.to_owned())),
            Some(_) => return None,
            None => (text, None),
        };
        let mut parts = numbers.split('.').map(|part| part.parse::<u64>().ok());
        let version = Self {
            major: parts.next()??,
            minor: parts.next()??,
            patch: parts.next()??,
            pre,
        };
        parts.next().is_none().then_some(version)
    }

    /// This build's version.
    #[must_use]
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Self {
            major: 0,
            minor: 0,
            patch: 0,
            pre: None,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                // A pre-release comes before its release.
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(a), Some(b)) => compare_pre(a, b),
            })
    }
}

/// Pre-release identifiers compared as Semantic Versioning does: numbers numerically and below
/// words, and a shorter list first when one is a prefix of the other.
fn compare_pre(a: &str, b: &str) -> Ordering {
    let mut a = a.split('.');
    let mut b = b.split('.');
    loop {
        match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// Whether `candidate` is newer than `current`.
#[must_use]
pub fn is_newer(current: &Version, candidate: &Version) -> bool {
    candidate > current
}

/// A file published with a release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// A release from the feed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    /// The release's page, for its notes.
    pub page: String,
    pub assets: Vec<Asset>,
}

/// Why the feed could not be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateError(pub String);

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UpdateError {}

/// The release a GitHub releases feed (`releases/latest`) describes.
///
/// # Errors
///
/// When the text is not such a release, or it is a draft or pre-release.
pub fn parse_release(json: &str) -> Result<Release, UpdateError> {
    #[derive(serde::Deserialize)]
    struct Feed {
        tag_name: String,
        html_url: String,
        #[serde(default)]
        draft: bool,
        #[serde(default)]
        prerelease: bool,
        #[serde(default)]
        assets: Vec<FeedAsset>,
    }
    #[derive(serde::Deserialize)]
    struct FeedAsset {
        name: String,
        browser_download_url: String,
        #[serde(default)]
        size: u64,
    }

    let feed: Feed = serde_json::from_str(json)
        .map_err(|error| UpdateError(format!("the releases feed is not readable: {error}")))?;
    if feed.draft || feed.prerelease {
        return Err(UpdateError(format!(
            "{} is not a final release",
            feed.tag_name
        )));
    }
    let version = Version::parse(&feed.tag_name)
        .ok_or_else(|| UpdateError(format!("{} is not a version", feed.tag_name)))?;
    Ok(Release {
        version,
        tag: feed.tag_name,
        page: feed.html_url,
        assets: feed
            .assets
            .into_iter()
            .map(|asset| Asset {
                name: asset.name,
                url: asset.browser_download_url,
                size: asset.size,
            })
            .collect(),
    })
}

/// What is remembered between runs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateState {
    /// When the feed was last read, in seconds since the Unix epoch.
    pub last_checked: Option<u64>,
    /// The last version the user was told about.
    pub announced: Option<String>,
}

impl UpdateState {
    /// The state in `path`; nothing remembered when it is missing or unreadable.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let Some(table) = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| text.parse::<toml::Table>().ok())
        else {
            return Self::default();
        };
        Self {
            last_checked: table
                .get("last_checked")
                .and_then(toml::Value::as_integer)
                .and_then(|seconds| u64::try_from(seconds).ok()),
            announced: table
                .get("announced")
                .and_then(toml::Value::as_str)
                .map(str::to_owned),
        }
    }

    /// Writes the state to `path`, making its directory if need be.
    ///
    /// # Errors
    ///
    /// When the file cannot be written.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut table = toml::Table::new();
        if let Some(seconds) = self.last_checked.and_then(|s| i64::try_from(s).ok()) {
            table.insert("last_checked".to_owned(), toml::Value::Integer(seconds));
        }
        if let Some(version) = &self.announced {
            table.insert("announced".to_owned(), toml::Value::String(version.clone()));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, table.to_string())
    }
}

/// Whether a check is due at `now` (seconds since the epoch): never checked, a day since, or the
/// clock has gone back past the last check.
#[must_use]
pub fn check_due(state: &UpdateState, now: u64) -> bool {
    state
        .last_checked
        .is_none_or(|last| now < last || now - last >= CHECK_INTERVAL)
}

/// Whether `release` should be announced to a user running `current`: it is newer, and they
/// have not been told about it.
#[must_use]
pub fn should_announce(state: &UpdateState, release: &Release, current: &Version) -> bool {
    is_newer(current, &release.version)
        && state.announced.as_deref() != Some(release.version.to_string().as_str())
}

/// Where the update state is kept: beside the user settings file. Settings loaded from no file
/// (an engine in a test, or embedded) have nowhere to remember a check, so they never check
/// unasked.
#[must_use]
pub fn state_file(settings_file: Option<&Path>) -> Option<PathBuf> {
    settings_file
        .and_then(Path::parent)
        .map(|directory| directory.join(STATE_FILE))
}

/// The file each release publishes with the SHA-256 of every other, as `sha256sum` writes it.
pub const SUMS: &str = "SHA256SUMS";

/// How the running app was installed, which decides how it can be updated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Install {
    /// An app bundle, from the DMG.
    MacApp,
    /// Installed by the MSI, under Program Files.
    WindowsMsi,
    /// Running as an AppImage.
    AppImage,
    /// Anything else: a deb, an rpm, a Flatpak, a portable zip or a build from source, which
    /// their own tools update.
    Other,
}

/// How the app is installed, from its executable's path and the environment.
#[must_use]
pub fn install_kind(executable: &Path, env: &dyn Fn(&str) -> Option<String>) -> Install {
    if env("APPIMAGE").is_some_and(|image| !image.is_empty()) {
        return Install::AppImage;
    }
    let mut ancestors = executable.ancestors().skip(1);
    let (macos, contents, bundle) = (ancestors.next(), ancestors.next(), ancestors.next());
    let named = |path: Option<&Path>, name: &str| {
        path.and_then(Path::file_name)
            .is_some_and(|file| file == name)
    };
    if named(macos, "MacOS")
        && named(contents, "Contents")
        && bundle
            .and_then(Path::extension)
            .is_some_and(|extension| extension == "app")
    {
        return Install::MacApp;
    }
    // The MSI installs per machine under Program Files; a portable zip unpacks anywhere else.
    let under_program_files = ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"]
        .iter()
        .filter_map(|name| env(name))
        .filter(|directory| !directory.is_empty())
        .any(|directory| {
            let directory = directory.to_lowercase();
            executable
                .to_string_lossy()
                .to_lowercase()
                .starts_with(&directory)
        });
    if under_program_files {
        return Install::WindowsMsi;
    }
    Install::Other
}

/// What can be done about a newer release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdatePlan {
    /// Download `asset`, check it against `checksum` (SHA-256, lower-case hex) and install it.
    InPlace { asset: Asset, checksum: String },
    /// Point to AppImageUpdate, which reads the update information the AppImage carries.
    AppImage { page: String },
    /// Point to the release page.
    Notify { page: String },
}

/// The SHA-256 that `sums` (a `SHA256SUMS` file) gives for `name`.
#[must_use]
pub fn checksum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        // `sha256sum` marks files read in binary mode with `*`.
        let file = file.trim_start().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// The package that updates an `install` on `arch` (`std::env::consts::ARCH`), as the release
/// workflow names them.
fn package_name(install: Install, version: &Version, arch: &str) -> Option<String> {
    match install {
        Install::MacApp => Some(format!("AdocFoundry-{version}-macos-universal.dmg")),
        Install::WindowsMsi => Some(format!("adocfoundry-{version}-{arch}.msi")),
        Install::AppImage | Install::Other => None,
    }
}

/// What to do about `release` for an `install` on `arch`, given the release's `SHA256SUMS`
/// text if it has one. Without a checksum for the package, the release is only pointed to.
#[must_use]
pub fn plan(release: &Release, install: Install, arch: &str, sums: Option<&str>) -> UpdatePlan {
    let page = release.page.clone();
    if install == Install::AppImage {
        return UpdatePlan::AppImage { page };
    }
    let in_place = package_name(install, &release.version, arch).and_then(|name| {
        let asset = release.assets.iter().find(|asset| asset.name == name)?;
        let checksum = checksum_for(sums?, &name)?;
        Some(UpdatePlan::InPlace {
            asset: asset.clone(),
            checksum,
        })
    });
    in_place.unwrap_or(UpdatePlan::Notify { page })
}

/// Checks that the file at `path` has the SHA-256 `expected` (hex).
///
/// # Errors
///
/// When it cannot be read or its digest differs.
pub fn verify_checksum(path: &Path, expected: &str) -> Result<(), UpdateError> {
    use sha2::Digest as _;
    let mut file = std::fs::File::open(path)
        .map_err(|error| UpdateError(format!("{}: {error}", path.display())))?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|error| UpdateError(format!("{}: {error}", path.display())))?;
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(UpdateError(format!(
            "{} does not match its published checksum",
            path.display()
        )))
    }
}

/// The Team ID in `codesign --display --verbose=2` output, unless the signature is ad hoc or
/// has none.
#[must_use]
pub fn team_identifier(codesign_output: &str) -> Option<String> {
    codesign_output.lines().find_map(|line| {
        let team = line.trim().strip_prefix("TeamIdentifier=")?;
        (!team.is_empty() && team != "not set").then(|| team.to_owned())
    })
}

impl crate::Engine {
    /// Where the update state is kept for these settings.
    fn update_state_file(&self) -> Option<PathBuf> {
        state_file(self.settings().user_file())
    }

    /// Whether the app should check for a newer release now (`now` in seconds since the
    /// epoch): `[updates] check` is on, there is a user settings file to remember checks
    /// beside, and no check was made in the last day.
    #[must_use]
    pub fn update_check_due(&self, now: u64) -> bool {
        self.settings().updates.check
            && self
                .update_state_file()
                .is_some_and(|path| check_due(&UpdateState::load(&path), now))
    }

    /// Records a check that read `json` from the feed at `now`, and returns the release to
    /// announce, if any. A check the user asked for (`asked`) announces any newer release; one
    /// at start-up announces each release once.
    ///
    /// # Errors
    ///
    /// When the feed cannot be read as a release; nothing is recorded then.
    pub fn record_update_check(
        &mut self,
        json: &str,
        now: u64,
        asked: bool,
    ) -> Result<Option<Release>, UpdateError> {
        let release = parse_release(json)?;
        let path = self.update_state_file();
        let mut state = path.as_deref().map(UpdateState::load).unwrap_or_default();
        state.last_checked = Some(now);
        let current = Version::current();
        let announce = if asked {
            is_newer(&current, &release.version)
        } else {
            should_announce(&state, &release, &current)
        };
        if announce {
            state.announced = Some(release.version.to_string());
        }
        if let Some(path) = path {
            // Not remembering only means checking or announcing again; never worth failing.
            let _ = state.save(&path);
        }
        Ok(announce.then_some(release))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text}"))
    }

    const FEED_JSON: &str = r#"{
        "tag_name": "v0.3.0",
        "html_url": "https://github.com/bovinemagnet/AdocFoundry/releases/tag/v0.3.0",
        "draft": false,
        "prerelease": false,
        "body": "Notes",
        "assets": [
            {"name": "AdocFoundry-0.3.0-macos-universal.dmg",
             "browser_download_url": "https://example.org/a.dmg", "size": 42},
            {"name": "SHA256SUMS", "browser_download_url": "https://example.org/SHA256SUMS"}
        ]
    }"#;

    #[test]
    fn parses_the_latest_release() {
        let release = parse_release(FEED_JSON).unwrap();
        assert_eq!(release.version, version("0.3.0"));
        assert_eq!(release.tag, "v0.3.0");
        assert!(release.page.ends_with("/tag/v0.3.0"));
        assert_eq!(release.assets.len(), 2);
        assert_eq!(release.assets[0].size, 42);
        assert_eq!(release.assets[1].size, 0);

        assert!(parse_release("{}").is_err());
        assert!(parse_release("not json").is_err());
        let draft = FEED_JSON.replace("\"draft\": false", "\"draft\": true");
        assert!(parse_release(&draft).is_err());
        let pre = FEED_JSON.replace("\"prerelease\": false", "\"prerelease\": true");
        assert!(parse_release(&pre).is_err());
        let odd = FEED_JSON.replace(
            "v0.3.0\",\n        \"html_url",
            "nightly\",\n        \"html_url",
        );
        assert!(parse_release(&odd).is_err());
    }

    #[test]
    fn newer_versions_are_detected() {
        assert!(is_newer(&version("0.2.1"), &version("0.3.0")));
        assert!(is_newer(&version("0.2.1"), &version("v0.2.2")));
        assert!(is_newer(&version("0.9.9"), &version("1.0.0")));
        assert!(!is_newer(&version("0.3.0"), &version("0.3.0")));
        assert!(!is_newer(&version("0.3.0"), &version("0.2.9")));
        // A pre-release build is offered its release, and later ones.
        assert!(is_newer(&version("0.3.0-beta.1"), &version("0.3.0")));
        assert!(!is_newer(&version("0.3.0"), &version("0.3.0-beta.1")));
        assert!(version("0.3.0-beta.2") < version("0.3.0-beta.10"));
        assert!(version("0.3.0-alpha") < version("0.3.0-beta"));
        assert_eq!(version("1.2.3+build.5"), version("1.2.3"));
        for bad in ["", "1.2", "1.2.3.4", "1.x.3", "1.2.3-", "v"] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert_eq!(version("v0.3.0-rc.1").to_string(), "0.3.0-rc.1");
    }

    #[test]
    fn checks_at_most_daily() {
        let never = UpdateState::default();
        assert!(check_due(&never, 1_000));
        let checked = UpdateState {
            last_checked: Some(1_000_000),
            announced: None,
        };
        assert!(!check_due(&checked, 1_000_000));
        assert!(!check_due(&checked, 1_000_000 + CHECK_INTERVAL - 1));
        assert!(check_due(&checked, 1_000_000 + CHECK_INTERVAL));
        // A clock set back does not stop checks for good.
        assert!(check_due(&checked, 999_999));
    }

    #[test]
    fn announces_a_release_once() {
        let release = parse_release(FEED_JSON).unwrap();
        let current = version("0.2.1");
        let mut state = UpdateState::default();
        assert!(should_announce(&state, &release, &current));
        state.announced = Some(release.version.to_string());
        assert!(!should_announce(&state, &release, &current));
        // Up to date: nothing to announce.
        assert!(!should_announce(
            &UpdateState::default(),
            &release,
            &version("0.3.0")
        ));

        // Remembered between runs.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config").join(STATE_FILE);
        assert_eq!(UpdateState::load(&path), UpdateState::default());
        state.last_checked = Some(1_700_000_000);
        state.save(&path).unwrap();
        assert_eq!(UpdateState::load(&path), state);
        std::fs::write(&path, "last_checked = \"soon\"\n[[").unwrap();
        assert_eq!(UpdateState::load(&path), UpdateState::default());
    }

    #[test]
    fn the_state_lives_beside_the_settings() {
        assert_eq!(
            state_file(Some(Path::new(
                "/home/me/.config/adocfoundry/settings.toml"
            ))),
            Some(PathBuf::from("/home/me/.config/adocfoundry").join(STATE_FILE))
        );
    }

    fn release_with(names: &[&str]) -> Release {
        Release {
            version: version("0.3.0"),
            tag: "v0.3.0".to_owned(),
            page: "https://example.org/v0.3.0".to_owned(),
            assets: names
                .iter()
                .map(|name| Asset {
                    name: (*name).to_owned(),
                    url: format!("https://example.org/{name}"),
                    size: 1,
                })
                .collect(),
        }
    }

    const HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    #[test]
    fn chooses_the_package_for_the_platform() {
        let names = [
            "AdocFoundry-0.3.0-macos-universal.dmg",
            "adocfoundry-0.3.0-x86_64.msi",
            "adocfoundry-0.3.0-aarch64.msi",
            "AdocFoundry-0.3.0-x86_64.AppImage",
            SUMS,
        ];
        let release = release_with(&names);
        let sums: String = names[..4]
            .iter()
            .map(|name| format!("{HASH}  {name}\n"))
            .collect();
        let in_place = |install, arch| match plan(&release, install, arch, Some(&sums)) {
            UpdatePlan::InPlace { asset, checksum } => {
                assert_eq!(checksum, HASH);
                asset.name
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(in_place(Install::MacApp, "aarch64"), names[0]);
        assert_eq!(in_place(Install::WindowsMsi, "x86_64"), names[1]);
        assert_eq!(in_place(Install::WindowsMsi, "aarch64"), names[2]);
        assert_eq!(
            plan(&release, Install::AppImage, "x86_64", Some(&sums)),
            UpdatePlan::AppImage {
                page: release.page.clone()
            }
        );
        assert_eq!(
            plan(&release, Install::Other, "x86_64", Some(&sums)),
            UpdatePlan::Notify {
                page: release.page.clone()
            }
        );
    }

    #[test]
    fn only_notifies_without_checksums() {
        let release = release_with(&["AdocFoundry-0.3.0-macos-universal.dmg"]);
        let notify = UpdatePlan::Notify {
            page: release.page.clone(),
        };
        assert_eq!(plan(&release, Install::MacApp, "aarch64", None), notify);
        let other = format!("{HASH}  something-else.dmg\n");
        assert_eq!(
            plan(&release, Install::MacApp, "aarch64", Some(&other)),
            notify
        );
        // A package not published at all is only pointed to.
        let sums = format!("{HASH}  adocfoundry-0.3.0-x86_64.msi\n");
        assert_eq!(
            plan(&release, Install::WindowsMsi, "x86_64", Some(&sums)),
            notify
        );

        assert_eq!(
            checksum_for(&format!("{}  *a.dmg\n", HASH.to_uppercase()), "a.dmg"),
            Some(HASH.to_owned())
        );
        assert_eq!(checksum_for("abc  a.dmg\n", "a.dmg"), None);
    }

    #[test]
    fn checksums_are_verified() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("package");
        std::fs::write(&path, "test").unwrap();
        // SHA-256 of `test`.
        assert_eq!(verify_checksum(&path, HASH), Ok(()));
        assert_eq!(verify_checksum(&path, &HASH.to_uppercase()), Ok(()));
        assert!(verify_checksum(&path, &HASH.replace('9', "8")).is_err());
        assert!(verify_checksum(&directory.path().join("missing"), HASH).is_err());
    }

    #[test]
    fn install_kind_follows_the_executable() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        let none = env(&[]);
        assert_eq!(
            install_kind(
                Path::new("/Applications/AdocFoundry.app/Contents/MacOS/adocfoundry"),
                &none
            ),
            Install::MacApp
        );
        assert_eq!(
            install_kind(Path::new("/usr/bin/adocfoundry"), &none),
            Install::Other
        );
        assert_eq!(
            install_kind(
                Path::new("/tmp/.mount_AdocFoXYZ/usr/bin/adocfoundry"),
                &env(&[("APPIMAGE", "/home/me/AdocFoundry.AppImage")])
            ),
            Install::AppImage
        );
        let windows = env(&[("ProgramFiles", r"C:\Program Files")]);
        assert_eq!(
            install_kind(
                Path::new(r"C:\Program Files\AdocFoundry\bin\adocfoundry.exe"),
                &windows
            ),
            Install::WindowsMsi
        );
        assert_eq!(
            install_kind(
                Path::new(r"C:\Users\me\Downloads\adocfoundry.exe"),
                &windows
            ),
            Install::Other
        );
    }

    #[test]
    fn team_identifiers_come_from_codesign() {
        let signed = "Executable=/Applications/AdocFoundry.app/Contents/MacOS/adocfoundry\n\
                      Authority=Developer ID Application: Someone (AB12CD34EF)\n\
                      TeamIdentifier=AB12CD34EF\n";
        assert_eq!(team_identifier(signed), Some("AB12CD34EF".to_owned()));
        assert_eq!(
            team_identifier("Signature=adhoc\nTeamIdentifier=not set\n"),
            None
        );
        assert_eq!(team_identifier("code object is not signed at all"), None);
    }
}
