//! Diagram rendering for the preview: Mermaid in process, other kinds through optional external
//! tools, every result cached on disk by a checksum of what affects it.

mod cache;
mod external;
mod mermaid;

use std::{
    collections::BTreeMap,
    fmt, io,
    path::{Path, PathBuf},
};

pub use cache::DiagramCache;
pub use external::{find_program, is_path, ExternalProvider, ToolPaths};
pub use mermaid::MermaidProvider;

/// One diagram block to render.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagramRequest {
    /// The block style: `mermaid`, `graphviz`, `plantuml`, ….
    pub kind: String,
    pub source: String,
    /// The block's attributes; those that change the image are part of the cache key.
    pub attributes: BTreeMap<String, String>,
}

/// Attributes that place or label the image but never change what is drawn.
const PRESENTATION_ATTRIBUTES: &[&str] = &[
    "align", "caption", "float", "height", "id", "link", "role", "target", "title", "width",
];

/// Changed whenever rendering changes for the same input, so cached images are rendered again.
const RENDER_REVISION: &str = "2";

impl DiagramRequest {
    #[must_use]
    pub fn new(kind: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            source: source.into(),
            attributes: BTreeMap::new(),
        }
    }

    /// The cache key: a SHA-256 of the kind, the source and the attributes that affect output.
    #[must_use]
    pub fn checksum(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        // Lengths separate the fields, so no two requests share an input.
        for part in [RENDER_REVISION, self.kind.as_str(), self.source.as_str()] {
            hasher.update((part.len() as u64).to_le_bytes());
            hasher.update(part.as_bytes());
        }
        for (name, value) in &self.attributes {
            if PRESENTATION_ATTRIBUTES.contains(&name.as_str()) {
                continue;
            }
            for part in [name, value] {
                hasher.update((part.len() as u64).to_le_bytes());
                hasher.update(part.as_bytes());
            }
        }
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

#[derive(Debug)]
pub enum DiagramError {
    /// No provider renders this kind.
    UnsupportedKind(String),
    /// The external tool for this kind is not installed.
    ToolMissing {
        kind: String,
        program: String,
    },
    /// The renderer ran and rejected the diagram.
    Failed(String),
    Io(io::Error),
}

impl fmt::Display for DiagramError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedKind(kind) => write!(formatter, "no renderer for {kind} diagrams"),
            Self::ToolMissing { kind, program } if external::is_path(Path::new(program)) => {
                write!(
                    formatter,
                    "{kind} diagrams need `{program}`, which does not exist"
                )
            }
            Self::ToolMissing { kind, program } => write!(
                formatter,
                "{kind} diagrams need `{program}`, which was not found on the PATH"
            ),
            Self::Failed(message) => formatter.write_str(message),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for DiagramError {}

impl From<io::Error> for DiagramError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Renders one or more diagram kinds to SVG text.
pub trait DiagramProvider: Send + Sync {
    fn kinds(&self) -> &[&str];
    /// # Errors
    ///
    /// When the diagram cannot be rendered; the message is shown in place of the image.
    fn render(&self, request: &DiagramRequest) -> Result<String, DiagramError>;
}

/// The providers and the cache the preview renders diagrams through.
pub struct Diagrams {
    providers: Vec<Box<dyn DiagramProvider>>,
    cache: DiagramCache,
}

impl Diagrams {
    /// Mermaid in process, and Graphviz, PlantUML and D2 when their tools are on the PATH.
    #[must_use]
    pub fn new(cache: DiagramCache) -> Self {
        Self::with_tools(cache, &ToolPaths::default())
    }

    /// Mermaid in process, and Graphviz, PlantUML and D2 run from `tools`.
    #[must_use]
    pub fn with_tools(cache: DiagramCache, tools: &ToolPaths) -> Self {
        let mut providers: Vec<Box<dyn DiagramProvider>> = vec![Box::new(MermaidProvider)];
        providers.extend(
            ExternalProvider::for_tools(tools)
                .into_iter()
                .map(|provider| Box::new(provider) as Box<dyn DiagramProvider>),
        );
        Self::with_providers(cache, providers)
    }

    /// Mermaid only: no external tool is ever run.
    #[must_use]
    pub fn in_process(cache: DiagramCache) -> Self {
        Self::with_providers(cache, vec![Box::new(MermaidProvider)])
    }

    #[must_use]
    pub fn with_providers(cache: DiagramCache, providers: Vec<Box<dyn DiagramProvider>>) -> Self {
        Self { providers, cache }
    }

    /// These providers and one more, for kinds a layer above renders (formulas).
    #[must_use]
    pub fn with_provider(mut self, provider: Box<dyn DiagramProvider>) -> Self {
        self.providers.push(provider);
        self
    }

    #[must_use]
    pub fn supports(&self, kind: &str) -> bool {
        self.provider(kind).is_some()
    }

    fn provider(&self, kind: &str) -> Option<&dyn DiagramProvider> {
        self.providers
            .iter()
            .find(|provider| provider.kinds().contains(&kind))
            .map(AsRef::as_ref)
    }

    /// The cached image for `request`, without rendering.
    #[must_use]
    pub fn cached(&self, request: &DiagramRequest) -> Option<PathBuf> {
        self.cache.get(request)
    }

    /// The image for `request`, rendering and caching it when it is not cached yet.
    ///
    /// # Errors
    ///
    /// When no provider handles the kind, its tool is missing, rendering fails, or the cache
    /// cannot be written.
    pub fn render(&self, request: &DiagramRequest) -> Result<PathBuf, DiagramError> {
        if let Some(path) = self.cached(request) {
            return Ok(path);
        }
        let provider = self
            .provider(&request.kind)
            .ok_or_else(|| DiagramError::UnsupportedKind(request.kind.clone()))?;
        let svg = provider.render(request)?;
        Ok(self.cache.put(request, &svg)?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    #[cfg(unix)]
    use std::sync::{Mutex, MutexGuard};

    use super::*;

    /// Serialises tests that spawn a process, as in `adoc-render`: a concurrent `fork` can
    /// inherit a script's open write descriptor and make its `execve` fail with `ETXTBSY`.
    /// Only the Unix tests write and run scripts.
    #[cfg(unix)]
    static SPAWN: Mutex<()> = Mutex::new(());

    #[cfg(unix)]
    fn spawn_guard() -> MutexGuard<'static, ()> {
        SPAWN.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn mermaid() -> DiagramRequest {
        DiagramRequest::new("mermaid", "graph TD\n  A[Start] --> B[Done]\n")
    }

    #[test]
    fn mermaid_renders_svg_in_process() {
        let svg = MermaidProvider.render(&mermaid()).unwrap();
        assert!(svg.contains("<svg"), "{svg}");
        assert!(svg.contains("Start") && svg.contains("Done"), "{svg}");
        // Labels are SVG text: native SVG renderers do not draw HTML in `foreignObject`.
        assert!(!svg.contains("foreignObject"), "{svg}");

        let error = MermaidProvider
            .render(&DiagramRequest::new("mermaid", "graph TD\n  A -->"))
            .map(|svg| svg.len());
        assert!(
            !matches!(error, Err(DiagramError::Io(_))),
            "a broken diagram is a render failure, not a crash: {error:?}"
        );
    }

    /// Mermaid, counting how often it actually renders.
    struct Counting(Arc<AtomicUsize>);

    impl DiagramProvider for Counting {
        fn kinds(&self) -> &[&str] {
            &["mermaid"]
        }

        fn render(&self, request: &DiagramRequest) -> Result<String, DiagramError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            MermaidProvider.render(request)
        }
    }

    #[test]
    fn cache_hits_by_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiagramCache::for_workspace(dir.path());
        let renders = Arc::new(AtomicUsize::new(0));
        let diagrams = Diagrams::with_providers(cache, vec![Box::new(Counting(renders.clone()))]);

        assert_eq!(diagrams.cached(&mermaid()), None);
        let first = diagrams.render(&mermaid()).unwrap();
        assert!(first.starts_with(dir.path().join(".cache/adocfoundry/diagrams")));
        assert_eq!(
            first.file_name().unwrap().to_str().unwrap(),
            format!("{}.svg", mermaid().checksum())
        );
        assert!(std::fs::read_to_string(&first).unwrap().contains("<svg"));
        let second = diagrams.render(&mermaid()).unwrap();
        assert_eq!(first, second);
        assert_eq!(diagrams.cached(&mermaid()), Some(first));
        assert_eq!(renders.load(Ordering::SeqCst), 1);

        // A different source is a different image.
        let mut edited = mermaid();
        edited.source.push_str("  B --> C\n");
        assert_eq!(diagrams.cached(&edited), None);
        diagrams.render(&edited).unwrap();
        assert_eq!(renders.load(Ordering::SeqCst), 2);
        // Stray temporary files are not left behind.
        let leftovers = std::fs::read_dir(diagrams.cache.dir())
            .unwrap()
            .filter(|entry| {
                !entry
                    .as_ref()
                    .unwrap()
                    .path()
                    .to_string_lossy()
                    .ends_with(".svg")
            })
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn missing_tool_fails_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let provider = ExternalProvider::new(
            vec!["graphviz"],
            "adoc-diagrams-tool-that-does-not-exist",
            Vec::new(),
        );
        let diagrams =
            Diagrams::with_providers(DiagramCache::new(dir.path()), vec![Box::new(provider)]);
        let error = diagrams
            .render(&DiagramRequest::new("graphviz", "digraph { a -> b }"))
            .unwrap_err();
        assert!(
            matches!(&error, DiagramError::ToolMissing { kind, .. } if kind == "graphviz"),
            "{error:?}"
        );
        assert!(error.to_string().contains("not found"), "{error}");

        let error = diagrams
            .render(&DiagramRequest::new("ditaa", "+--+"))
            .unwrap_err();
        assert!(
            matches!(error, DiagramError::UnsupportedKind(_)),
            "{error:?}"
        );
        assert!(!diagrams.supports("ditaa"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_configured_tool_that_does_not_exist_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("bin").join("dot");
        let tools = ToolPaths {
            dot: missing.clone(),
            ..ToolPaths::default()
        };
        let diagrams = Diagrams::with_tools(DiagramCache::new(dir.path().join("cache")), &tools);
        let error = diagrams
            .render(&DiagramRequest::new("graphviz", "digraph { a -> b }"))
            .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains(&missing.display().to_string()),
            "{message}"
        );
        assert!(!message.contains("PATH"), "{message}");
    }

    #[test]
    fn diagram_attributes_change_the_key() {
        let plain = mermaid();
        let mut themed = mermaid();
        themed
            .attributes
            .insert("theme".to_owned(), "dark".to_owned());
        let mut placed = mermaid();
        placed.attributes.insert("id".to_owned(), "flow".to_owned());
        placed
            .attributes
            .insert("width".to_owned(), "400".to_owned());
        placed
            .attributes
            .insert("title".to_owned(), "A flow".to_owned());
        let mut kind = mermaid();
        kind.kind = "graphviz".to_owned();

        assert_ne!(plain.checksum(), themed.checksum());
        assert_eq!(plain.checksum(), placed.checksum());
        assert_ne!(plain.checksum(), kind.checksum());
        assert_eq!(plain.checksum().len(), 64);
        // Field boundaries are part of the key.
        assert_ne!(
            DiagramRequest::new("ab", "c").checksum(),
            DiagramRequest::new("a", "bc").checksum()
        );
    }

    #[cfg(unix)]
    #[test]
    fn external_tools_read_source_and_report_failure() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = spawn_guard();
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("fake-dot");
        std::fs::write(
            &tool,
            "#!/bin/sh\nsource=$(cat)\ncase \"$source\" in\n  *fail*) echo 'syntax error' >&2; exit 1;;\nesac\nprintf '<svg>%s</svg>' \"$source\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let provider = ExternalProvider::new(vec!["graphviz"], &tool, Vec::new());

        let svg = provider
            .render(&DiagramRequest::new("graphviz", "a -> b"))
            .unwrap();
        assert_eq!(svg, "<svg>a -> b</svg>");
        let error = provider
            .render(&DiagramRequest::new("graphviz", "fail"))
            .unwrap_err();
        assert!(
            matches!(&error, DiagramError::Failed(message) if message == "syntax error"),
            "{error:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_configured_tool_renders_from_outside_the_path() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = spawn_guard();
        let dir = tempfile::tempdir().unwrap();
        // Named as the real tool, in a folder that is not on the PATH.
        let tool = dir.path().join("homebrew").join("dot");
        std::fs::create_dir_all(tool.parent().unwrap()).unwrap();
        std::fs::write(&tool, "#!/bin/sh\nprintf '<svg>%s</svg>' \"$(cat)\"\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tools = ToolPaths {
            dot: tool,
            ..ToolPaths::default()
        };
        let diagrams = Diagrams::with_tools(DiagramCache::new(dir.path().join("cache")), &tools);

        let svg = diagrams
            .render(&DiagramRequest::new("graphviz", "a -> b"))
            .unwrap();
        assert_eq!(std::fs::read_to_string(svg).unwrap(), "<svg>a -> b</svg>");
    }
}
