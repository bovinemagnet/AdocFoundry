//! The doctor report: what reproducing a problem on another machine needs, gathered in one
//! place and ready to paste into an issue. That is the build, the platform, each optional tool
//! with its path and version (or why it is missing), the PDF theme's fonts, and the settings
//! files with their warnings.
//!
//! [`Engine::doctor`] collects what the engine knows, on its own thread. [`Doctor::run`] runs
//! the tools and reads the fonts, which can take seconds, so a front end can call it on a
//! worker. Front ends add what only they know (the app's graphics and editor font) with
//! [`Doctor::program`] and [`Doctor::add_section`].

use std::{
    ffi::OsString,
    io::Read as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;

use crate::{
    commands::{ids, register, spec},
    CommandCategory, CommandContext, CommandOutcome, CommandRegistry, Engine,
};

/// How long a tool has to print its version before it is reported as not answering.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// The report, in sections of labelled lines.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct DoctorReport {
    pub sections: Vec<DoctorSection>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct DoctorSection {
    pub title: String,
    pub lines: Vec<DoctorLine>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct DoctorLine {
    pub label: String,
    pub value: String,
}

impl DoctorSection {
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            lines: Vec::new(),
        }
    }

    /// Adds a line. A label may repeat, one line per value.
    #[must_use]
    pub fn line(mut self, label: impl Into<String>, value: impl Into<String>) -> Self {
        self.lines.push(DoctorLine {
            label: label.into(),
            value: value.into(),
        });
        self
    }

    /// The value of the first line labelled `label`.
    #[must_use]
    pub fn value(&self, label: &str) -> Option<&str> {
        self.lines
            .iter()
            .find(|line| line.label == label)
            .map(|line| line.value.as_str())
    }

    /// Every value labelled `label`, in order.
    pub fn values<'a>(&'a self, label: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.lines
            .iter()
            .filter(move |line| line.label == label)
            .map(|line| line.value.as_str())
    }
}

impl DoctorReport {
    #[must_use]
    pub fn section(&self, title: &str) -> Option<&DoctorSection> {
        self.sections.iter().find(|section| section.title == title)
    }

    /// The report as plain text: each section's title, then its lines with the values aligned.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = String::from("AdocFoundry diagnostics\n");
        for section in &self.sections {
            text.push('\n');
            text.push_str(&section.title);
            text.push('\n');
            let width = section
                .lines
                .iter()
                .map(|line| line.label.chars().count())
                .max()
                .unwrap_or(0);
            for line in &section.lines {
                let label = format!("{}:", line.label);
                let value = &line.value;
                text.push_str(&format!("  {label:<pad$} {value}\n", pad = width + 1));
            }
        }
        text
    }
}

/// An optional tool the report looks for.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Tool {
    name: &'static str,
    /// The setting that names it, and its value, when one is set.
    configured: Option<(&'static str, String)>,
    /// The arguments that make it print its version.
    version_args: &'static [&'static str],
}

/// Everything the report needs from the engine, taken on the engine's thread.
#[derive(Clone, Debug, PartialEq)]
pub struct Doctor {
    program: Option<String>,
    sections: Vec<DoctorSection>,
    tools: Vec<Tool>,
    search_path: Option<OsString>,
    settings: DoctorSection,
    spelling: DoctorSection,
    #[cfg(feature = "typst")]
    theme: adoc_pdf::ThemeSource,
    #[cfg(feature = "typst")]
    fonts_dirs: Vec<PathBuf>,
    #[cfg(feature = "typst")]
    access: adoc_core::FileAccess,
}

impl Engine {
    /// What the doctor report needs from the engine; [`Doctor::run`] completes it.
    #[must_use]
    pub fn doctor(&self) -> Doctor {
        let settings = &self.settings;
        let configured = |setting: &'static str, value: &str| {
            (!value.is_empty()).then(|| (setting, value.to_owned()))
        };
        let tools = vec![
            Tool {
                name: "asciidoctor",
                configured: configured("[tools] asciidoctor", &settings.tools.asciidoctor),
                version_args: &["--version"],
            },
            Tool {
                name: "dot",
                configured: configured("[tools] dot", &settings.tools.dot),
                version_args: &["-V"],
            },
            Tool {
                name: "plantuml",
                configured: configured("[tools] plantuml", &settings.tools.plantuml),
                version_args: &["-version"],
            },
            Tool {
                name: "d2",
                configured: configured("[tools] d2", &settings.tools.d2),
                version_args: &["--version"],
            },
            Tool {
                name: "git",
                configured: configured("[tools] git", &settings.tools.git),
                version_args: &["--version"],
            },
        ];

        let file = |path: Option<&Path>| match path {
            Some(path) if path.is_file() => path.display().to_string(),
            Some(path) => format!("{} (not found)", path.display()),
            None => "none".to_owned(),
        };
        let mut section = DoctorSection::new("Settings")
            .line("User settings", file(settings.user_file()))
            .line("Workspace settings", file(settings.workspace_file()));
        for root in &self.roots {
            section = section.line("Workspace root", root.display().to_string());
        }
        let git = if !cfg!(feature = "git") {
            "not built in"
        } else if settings.git.enabled {
            "on"
        } else {
            "off"
        };
        section = section.line("Git", git);
        if settings.warnings.is_empty() {
            section = section.line("Warnings", "none");
        }
        for warning in &settings.warnings {
            section = section.line("Warning", warning.clone());
        }

        let mut spelling = DoctorSection::new("Spelling")
            .line(
                "Spelling",
                if settings.spelling.enabled {
                    "on"
                } else {
                    "off"
                },
            )
            .line("Language", settings.spelling.language.clone());
        let folder = settings.spelling.dictionaries.as_deref().map(Path::new);
        for info in adoc_spell::Dictionaries::new(folder).list() {
            let source = match info.source {
                adoc_spell::DictionarySource::Bundled => "built in".to_owned(),
                adoc_spell::DictionarySource::File(path) => path.display().to_string(),
            };
            spelling = spelling.line("Dictionary", format!("{} ({source})", info.name));
        }

        #[cfg(feature = "typst")]
        let attributes = std::collections::BTreeMap::new();
        Doctor {
            program: None,
            sections: Vec::new(),
            tools,
            search_path: std::env::var_os("PATH"),
            settings: section,
            spelling,
            #[cfg(feature = "typst")]
            theme: self.theme_source(None, &attributes, None),
            #[cfg(feature = "typst")]
            fonts_dirs: self.fonts_dirs(None, &attributes, None),
            #[cfg(feature = "typst")]
            access: self.file_access.clone(),
        }
    }
}

impl Doctor {
    /// Names the front end, `AdocFoundry 0.3.0`, for the report's first line.
    #[must_use]
    pub fn program(mut self, name: &str, version: &str) -> Self {
        self.program = Some(format!("{name} {version}"));
        self
    }

    /// Adds a section only the front end can fill, shown after the system's.
    pub fn add_section(&mut self, section: DoctorSection) {
        self.sections.push(section);
    }

    /// Looks for the tools on `path` instead of the `PATH` the engine started with.
    #[must_use]
    pub fn search_path(mut self, path: impl Into<OsString>) -> Self {
        self.search_path = Some(path.into());
        self
    }

    /// Runs each tool for its version, reads the theme's fonts and writes the report. This can
    /// take seconds, so a front end with a user interface calls it off its main thread.
    #[must_use]
    pub fn run(self) -> DoctorReport {
        let mut features = Vec::new();
        if cfg!(feature = "typst") {
            features.push("typst");
        }
        if cfg!(feature = "git") {
            features.push("git");
        }
        let mut program = DoctorSection::new("Program");
        if let Some(name) = &self.program {
            program = program.line("Version", name.clone());
        }
        program = program
            .line(
                "Engine",
                format!("adoc-engine {}", env!("CARGO_PKG_VERSION")),
            )
            .line(
                "Build",
                if cfg!(debug_assertions) {
                    "debug"
                } else {
                    "release"
                },
            )
            .line(
                "Features",
                if features.is_empty() {
                    "none".to_owned()
                } else {
                    features.join(", ")
                },
            );

        let system = DoctorSection::new("System")
            .line("OS", os_description())
            .line("Architecture", std::env::consts::ARCH);

        let search_path = self.search_path.clone().unwrap_or_default();
        let tools: Vec<DoctorLine> = thread::scope(|scope| {
            let probes: Vec<_> = self
                .tools
                .iter()
                .map(|tool| scope.spawn(|| tool_line(tool, &search_path)))
                .collect();
            probes
                .into_iter()
                .zip(&self.tools)
                .map(|(probe, tool)| DoctorLine {
                    label: tool.name.to_owned(),
                    value: probe
                        .join()
                        .unwrap_or_else(|_| "could not be checked".to_owned()),
                })
                .collect()
        });

        let mut sections = vec![program, system];
        sections.extend(self.sections.iter().cloned());
        sections.push(self.pdf_section());
        sections.push(DoctorSection {
            title: "Tools".to_owned(),
            lines: tools,
        });
        sections.push(self.settings);
        sections.push(self.spelling);
        DoctorReport { sections }
    }

    #[cfg(feature = "typst")]
    fn pdf_section(&self) -> DoctorSection {
        use adoc_pdf::ThemeSource;

        let theme = match &self.theme {
            ThemeSource::Bundled(name) => format!("{name} (bundled)"),
            ThemeSource::File(path) => path.display().to_string(),
        };
        let dirs: Vec<String> = self
            .fonts_dirs
            .iter()
            .filter(|dir| dir.as_os_str() != "GEM_FONTS_DIR")
            .map(|dir| dir.display().to_string())
            .collect();
        let mut section = DoctorSection::new("PDF").line("Theme", theme).line(
            "Fonts directory",
            if dirs.is_empty() {
                "none (the bundled fonts only)".to_owned()
            } else {
                dirs.join(", ")
            },
        );
        match adoc_pdf::load_theme_within(&self.theme, &self.access) {
            Ok(loaded) => {
                let families: Vec<&str> =
                    loaded.fonts.iter().map(|(name, _)| name.as_str()).collect();
                section = section.line("Font families", families.join(", "));
                let missing = adoc_pdf::missing_fonts(&loaded, &self.fonts_dirs, &self.access);
                if missing.is_empty() {
                    section = section.line("Fonts", "all found");
                }
                for font in missing {
                    section = section.line("Missing", font);
                }
                for warning in loaded.warnings {
                    section = section.line("Theme warning", warning.message);
                }
            }
            Err(error) => section = section.line("Error", error.to_string()),
        }
        section
    }

    #[cfg(not(feature = "typst"))]
    fn pdf_section(&self) -> DoctorSection {
        DoctorSection::new("PDF").line("Export", "not built in")
    }
}

/// The tool's path and version, or why it cannot be used.
fn tool_line(tool: &Tool, search_path: &OsString) -> String {
    let program = match &tool.configured {
        Some((setting, value)) => {
            let path = PathBuf::from(value);
            if !path.is_file() {
                return format!("not found: {setting} is {value}, which is not a file");
            }
            path
        }
        None => match find_on(search_path, tool.name) {
            Some(path) => path,
            None => return "not found on PATH".to_owned(),
        },
    };
    match probe(&program, tool.version_args) {
        Ok(version) => format!("{} ({version})", program.display()),
        Err(problem) => format!("{} ({problem})", program.display()),
    }
}

/// The first `name` in the directories of `search_path`, with the extensions `PATHEXT` lists
/// on Windows.
fn find_on(search_path: &OsString, name: &str) -> Option<PathBuf> {
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.BAT;.CMD".to_owned())
            .split(';')
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(search_path).find_map(|directory| {
        extensions.iter().find_map(|extension| {
            let candidate = directory.join(format!("{name}{extension}"));
            candidate.is_file().then_some(candidate)
        })
    })
}

/// Runs `program` with `args` and returns the first line it prints, from standard output or
/// else standard error (`dot -V` writes there).
fn probe(program: &Path, args: &[&str]) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run it: {error}"))?;
    let read = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let stdout = read(child.stdout.take().map(|pipe| Box::new(pipe) as _));
    let stderr = read(child.stderr.take().map(|pipe| Box::new(pipe) as _));
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "did not answer within {} s",
                    PROBE_TIMEOUT.as_secs()
                ));
            }
            Err(error) => return Err(format!("cannot run it: {error}")),
        }
    };
    let output = stdout.join().unwrap_or_default();
    let errors = stderr.join().unwrap_or_default();
    match first_line(&output).or_else(|| first_line(&errors)) {
        Some(line) => Ok(line.to_owned()),
        None if status.success() => Err("printed no version".to_owned()),
        None => Err(format!("exited with {status}")),
    }
}

fn first_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

/// The operating system, with its version where the system says.
fn os_description() -> String {
    let os = std::env::consts::OS;
    let version = if cfg!(target_os = "macos") {
        probe(Path::new("/usr/bin/sw_vers"), &["-productVersion"])
            .ok()
            .map(|version| format!("macOS {version}"))
    } else if cfg!(target_os = "linux") {
        let name = std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find_map(|line| line.strip_prefix("PRETTY_NAME="))
                    .map(|name| name.trim_matches('"').to_owned())
            });
        let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|kernel| kernel.trim().to_owned());
        match (name, kernel) {
            (Some(name), Some(kernel)) => Some(format!("{name}, Linux {kernel}")),
            (Some(name), None) => Some(name),
            (None, Some(kernel)) => Some(format!("Linux {kernel}")),
            (None, None) => None,
        }
    } else if cfg!(windows) {
        probe(Path::new("cmd"), &["/c", "ver"]).ok()
    } else {
        None
    };
    version.unwrap_or_else(|| os.to_owned())
}

pub(crate) fn register_doctor(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::APP_DIAGNOSTICS,
            "Show Diagnostics",
            CommandCategory::Help,
            None,
            CommandContext::Always,
        ),
        |engine, _| Ok(CommandOutcome::Diagnostics(Box::new(engine.doctor()))),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_aligns_each_sections_values() {
        let report = DoctorReport {
            sections: vec![DoctorSection::new("Tools")
                .line("git", "/usr/bin/git (git version 2.47.0)")
                .line("asciidoctor", "not found on PATH")],
        };
        assert_eq!(
            report.to_text(),
            "AdocFoundry diagnostics\n\
             \n\
             Tools\n\
             \x20 git:         /usr/bin/git (git version 2.47.0)\n\
             \x20 asciidoctor: not found on PATH\n"
        );
    }

    #[test]
    fn first_line_skips_blank_lines() {
        assert_eq!(
            first_line("\n  \nPlantUML version 1\nmore"),
            Some("PlantUML version 1")
        );
        assert_eq!(first_line(" \n"), None);
    }

    #[test]
    fn a_tool_missing_from_the_search_path_says_so() {
        let empty = tempfile::tempdir().unwrap();
        let tool = Tool {
            name: "dot",
            configured: None,
            version_args: &["-V"],
        };
        assert_eq!(
            tool_line(&tool, &empty.path().as_os_str().to_owned()),
            "not found on PATH"
        );
    }
}
