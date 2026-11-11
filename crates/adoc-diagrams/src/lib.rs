//! Diagram rendering for the preview: Mermaid in process, other kinds through optional external
//! tools, every result cached on disk by a checksum of what affects it.

mod cache;
mod external;
mod mermaid;
mod reads;

use std::{
    collections::{BTreeMap, HashSet},
    fmt, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub use cache::DiagramCache;
pub use external::{
    find_program, is_path, ExternalProvider, ToolPaths, DEFAULT_TIMEOUT, MAX_OUTPUT,
};
pub use mermaid::MermaidProvider;

/// One diagram block to render.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagramRequest {
    /// The block style: `mermaid`, `graphviz`, `plantuml`, ….
    pub kind: String,
    pub source: String,
    /// The block's attributes; those that change the image are part of the cache key.
    pub attributes: BTreeMap<String, String>,
    /// The folder of the page the diagram is on. External tools run in it, so a relative
    /// `!include` or `image=` resolves against the page, as asciidoctor-diagram resolves it.
    pub directory: Option<PathBuf>,
}

/// Attributes that place or label the image but never change what is drawn.
const PRESENTATION_ATTRIBUTES: &[&str] = &[
    "align", "caption", "float", "height", "id", "link", "role", "target", "title", "width",
];

/// Changed whenever rendering changes for the same input, so cached images are rendered again:
/// among other times, when merman (rendering Mermaid in process) is upgraded.
const RENDER_REVISION: &str = "2";

impl DiagramRequest {
    #[must_use]
    pub fn new(kind: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            source: source.into(),
            attributes: BTreeMap::new(),
            directory: None,
        }
    }

    /// A SHA-256 of the kind, the source and the attributes that affect output: the cache key
    /// for a renderer that nothing else changes the output of. [`Diagrams::checksum`] adds
    /// the renderer's own part.
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
        hex(&hasher.finalize())
    }
}

/// `bytes` in lower-case hexadecimal.
fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0xf)]));
    }
    text
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
    /// The tool ran past its deadline and was stopped.
    TimedOut {
        program: String,
        after: std::time::Duration,
    },
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
            Self::TimedOut { program, after } => write!(
                formatter,
                "`{program}` did not finish within {} seconds and was stopped",
                after.as_secs_f64()
            ),
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
    /// What, besides the request itself, decides the image this provider renders for
    /// `request`, such as which program draws it. Part of the cache key, so an image rendered
    /// by another program, or for another folder, is not reused. Empty by default.
    fn identity(&self, _request: &DiagramRequest) -> String {
        String::new()
    }
    /// Forgets what the provider found out about its tools, so the next render looks again.
    /// Nothing by default.
    fn look_again(&self) {}
    /// The local files the diagram names for the renderer to read, such as a PlantUML
    /// `!include`, whose changes change the image. None by default.
    fn inputs(&self, _request: &DiagramRequest) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// The providers and the cache the preview renders diagrams through.
pub struct Diagrams {
    providers: Vec<Box<dyn DiagramProvider>>,
    cache: DiagramCache,
    /// Checksums whose images are known to be in the cache. Images are named by their
    /// content, so once there an image is never out of date, and the disk is not asked again.
    present: Mutex<HashSet<String>>,
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
        let mut providers: Vec<Box<dyn DiagramProvider>> =
            vec![Box::new(MermaidProvider::with_timeout(tools.timeout))];
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
        Self::with_providers(cache, vec![Box::new(MermaidProvider::default())])
    }

    #[must_use]
    pub fn with_providers(cache: DiagramCache, providers: Vec<Box<dyn DiagramProvider>>) -> Self {
        Self {
            providers,
            cache,
            present: Mutex::default(),
        }
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

    /// The cache key of `request`: its [`DiagramRequest::checksum`], with what its provider
    /// says decides the image ([`DiagramProvider::identity`]).
    #[must_use]
    pub fn checksum(&self, request: &DiagramRequest) -> String {
        let checksum = request.checksum();
        let identity = self
            .provider(&request.kind)
            .map(|provider| provider.identity(request))
            .unwrap_or_default();
        if identity.is_empty() {
            return checksum;
        }
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for part in [checksum.as_str(), identity.as_str()] {
            hasher.update((part.len() as u64).to_le_bytes());
            hasher.update(part.as_bytes());
        }
        hex(&hasher.finalize())
    }

    /// The local files `request` names for its renderer to read ([`DiagramProvider::inputs`]):
    /// a change to one is a reason to show the diagram again.
    #[must_use]
    pub fn inputs(&self, request: &DiagramRequest) -> Vec<PathBuf> {
        self.provider(&request.kind)
            .map(|provider| provider.inputs(request))
            .unwrap_or_default()
    }

    /// Has every provider look for its tools again, so one installed or upgraded since is used.
    pub fn look_for_tools_again(&self) {
        for provider in &self.providers {
            provider.look_again();
        }
    }

    /// The cached image for `request`, without rendering.
    #[must_use]
    pub fn cached(&self, request: &DiagramRequest) -> Option<PathBuf> {
        self.cached_at(&self.checksum(request))
    }

    /// The cached image for the request whose [`DiagramRequest::checksum`] is `checksum`.
    #[must_use]
    pub fn cached_at(&self, checksum: &str) -> Option<PathBuf> {
        let mut present = self
            .present
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if present.contains(checksum) {
            return Some(self.cache.path(checksum));
        }
        let path = self.cache.get(checksum)?;
        present.insert(checksum.to_owned());
        Some(path)
    }

    /// The image for `request`, rendering and caching it when it is not cached yet.
    ///
    /// # Errors
    ///
    /// When no provider handles the kind, its tool is missing, rendering fails, or the cache
    /// cannot be written.
    pub fn render(&self, request: &DiagramRequest) -> Result<PathBuf, DiagramError> {
        self.render_at(&self.checksum(request), request)
    }

    /// [`Diagrams::render`] for a request whose checksum is already known.
    ///
    /// # Errors
    ///
    /// As [`Diagrams::render`].
    pub fn render_at(
        &self,
        checksum: &str,
        request: &DiagramRequest,
    ) -> Result<PathBuf, DiagramError> {
        if let Some(path) = self.cached_at(checksum) {
            return Ok(path);
        }
        let provider = self
            .provider(&request.kind)
            .ok_or_else(|| DiagramError::UnsupportedKind(request.kind.clone()))?;
        let svg = provider.render(request)?;
        let path = self.cache.put(checksum, &svg)?;
        self.present
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(checksum.to_owned());
        Ok(path)
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
        let svg = MermaidProvider::default().render(&mermaid()).unwrap();
        assert!(svg.contains("<svg"), "{svg}");
        assert!(svg.contains("Start") && svg.contains("Done"), "{svg}");
        // Labels are SVG text: native SVG renderers do not draw HTML in `foreignObject`.
        assert!(!svg.contains("foreignObject"), "{svg}");

        let error = MermaidProvider::default()
            .render(&DiagramRequest::new("mermaid", "graph TD\n  A -->"))
            .map(|svg| svg.len());
        assert!(
            !matches!(error, Err(DiagramError::Io(_))),
            "a broken diagram is a render failure, not a crash: {error:?}"
        );
    }

    #[test]
    fn mermaid_stops_at_its_deadline() {
        let error = MermaidProvider::with_timeout(std::time::Duration::from_nanos(1))
            .render(&mermaid())
            .unwrap_err();
        assert!(
            matches!(&error, DiagramError::TimedOut { program, .. } if program == "Mermaid"),
            "{error:?}"
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
            MermaidProvider::default().render(request)
        }
    }

    #[test]
    fn checksums_keep_the_names_images_are_cached_under() {
        assert_eq!(
            DiagramRequest::new("mermaid", "graph TD\n  A --> B\n").checksum(),
            "cdd76dd39e9373fdcca4bb961e0506754e84dba92274cd852168e41664640289"
        );
    }

    #[test]
    fn an_image_known_to_be_cached_is_not_looked_for_again() {
        let dir = tempfile::tempdir().unwrap();
        let diagrams = Diagrams::in_process(DiagramCache::for_workspace(dir.path()));
        let request = mermaid();
        let checksum = request.checksum();
        let path = diagrams.render(&request).unwrap();

        // Images are named by their content, so one found once stays found.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(diagrams.cached_at(&checksum), Some(path.clone()));

        // Another renderer over the same folder looks on disk.
        let fresh = Diagrams::in_process(DiagramCache::for_workspace(dir.path()));
        assert_eq!(fresh.cached_at(&checksum), None);
        std::fs::write(&path, "<svg/>").unwrap();
        assert_eq!(fresh.cached_at(&checksum), Some(path));
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

    #[cfg(unix)]
    #[test]
    fn cache_key_changes_with_the_tool() {
        let _guard = spawn_guard();
        let dir = tempfile::tempdir().unwrap();
        let tool = |folder: &str| {
            let folder = dir.path().join(folder);
            std::fs::create_dir_all(&folder).unwrap();
            script(&folder, "#!/bin/sh\nprintf '<svg/>'\n")
        };
        let diagrams = |tool: PathBuf| {
            let tools = ToolPaths {
                dot: tool,
                ..ToolPaths::default()
            };
            Diagrams::with_tools(DiagramCache::new(dir.path().join("cache")), &tools)
        };
        let (old, new) = (diagrams(tool("old")), diagrams(tool("new")));
        let request = DiagramRequest::new("graphviz", "a -> b");
        // Another program renders it: an image cached from the old one is not reused.
        assert_ne!(old.checksum(&request), new.checksum(&request));
        assert_eq!(old.checksum(&request), old.checksum(&request));
        // Mermaid, rendered in process, keeps the key its cached images are named by.
        assert_eq!(old.checksum(&mermaid()), mermaid().checksum());
    }

    #[cfg(unix)]
    #[test]
    fn a_tool_installed_after_it_was_missed_is_found() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = spawn_guard();
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("bin").join("dot");
        let tools = ToolPaths {
            dot: tool.clone(),
            ..ToolPaths::default()
        };
        let diagrams = Diagrams::with_tools(DiagramCache::new(dir.path().join("cache")), &tools);
        let request = DiagramRequest::new("graphviz", "a -> b");
        let missing = diagrams.checksum(&request);
        assert!(matches!(
            diagrams.render(&request),
            Err(DiagramError::ToolMissing { .. })
        ));

        // Installed: a miss is not remembered beyond a moment, so the key moves on its own.
        std::fs::create_dir_all(tool.parent().unwrap()).unwrap();
        std::fs::write(&tool, "#!/bin/sh\nprintf '<svg>%s</svg>' \"$(cat)\"\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::thread::sleep(external::MISS_RECHECK + std::time::Duration::from_millis(50));
        let found = diagrams.checksum(&request);
        assert_ne!(found, missing);
        let svg = diagrams.render(&request).unwrap();
        assert_eq!(std::fs::read_to_string(svg).unwrap(), "<svg>a -> b</svg>");

        // A tool found is kept until asked to look again, as after an upgrade.
        std::fs::write(&tool, "#!/bin/sh\nprintf '<svg>upgraded</svg>'\n").unwrap();
        assert_eq!(diagrams.checksum(&request), found);
        diagrams.look_for_tools_again();
        assert_ne!(diagrams.checksum(&request), found);
    }

    #[cfg(unix)]
    #[test]
    fn tools_run_in_the_page_directory() {
        let _guard = spawn_guard();
        let dir = tempfile::tempdir().unwrap();
        let tool = script(
            dir.path(),
            "#!/bin/sh\nprintf '<svg>%s</svg>' \"$(pwd -P)\"\n",
        );
        let page_folder = dir.path().join("docs").join("pages");
        std::fs::create_dir_all(&page_folder).unwrap();
        let provider = ExternalProvider::new(vec!["graphviz"], &tool, Vec::new());
        let request = DiagramRequest {
            directory: Some(page_folder.clone()),
            ..DiagramRequest::new("graphviz", "a -> b")
        };
        let svg = provider.render(&request).unwrap();
        // So `!include` and `image=` resolve against the page, as asciidoctor-diagram does.
        let resolved = std::fs::canonicalize(&page_folder).unwrap();
        assert_eq!(svg, format!("<svg>{}</svg>", resolved.display()));
        // The folder is part of the key: the same source elsewhere can include other files.
        let elsewhere = DiagramRequest {
            directory: Some(dir.path().to_path_buf()),
            ..request.clone()
        };
        let tools = ToolPaths {
            dot: tool,
            ..ToolPaths::default()
        };
        let diagrams = Diagrams::with_tools(DiagramCache::new(dir.path().join("cache")), &tools);
        assert_ne!(diagrams.checksum(&request), diagrams.checksum(&elsewhere));
    }

    /// A diagram of `kind` holding `source`, on a page in `folder`, and the renderer of a
    /// provider whose tool is a stub that is never run.
    fn keyed(folder: &Path, kind: &str, source: &str) -> (Diagrams, DiagramRequest) {
        let provider = ExternalProvider::new(
            vec!["graphviz", "plantuml", "d2"],
            "adoc-diagrams-stub-tool",
            Vec::new(),
        );
        let diagrams = Diagrams::with_providers(
            DiagramCache::new(folder.join("cache")),
            vec![Box::new(provider)],
        );
        let request = DiagramRequest {
            directory: Some(folder.to_path_buf()),
            ..DiagramRequest::new(kind, source)
        };
        (diagrams, request)
    }

    #[test]
    fn a_file_a_diagram_includes_is_part_of_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        std::fs::create_dir_all(folder.join("common")).unwrap();
        std::fs::write(folder.join("common/styles.puml"), "skinparam a b\n").unwrap();
        std::fs::write(folder.join("unrelated.puml"), "skinparam c d\n").unwrap();
        let (diagrams, request) = keyed(
            folder,
            "plantuml",
            "@startuml\n!include common/styles.puml\nA -> B\n@enduml\n",
        );
        let before = diagrams.checksum(&request);
        assert_eq!(
            diagrams.inputs(&request),
            [folder.join("common/styles.puml")]
        );

        // Another file changing leaves the key.
        std::fs::write(folder.join("unrelated.puml"), "skinparam c d e f\n").unwrap();
        assert_eq!(diagrams.checksum(&request), before);

        // The included file changing changes it, so the image is drawn again.
        std::fs::write(folder.join("common/styles.puml"), "skinparam a b c\n").unwrap();
        let after = diagrams.checksum(&request);
        assert_ne!(after, before);

        // Taken away, it changes again; the diagram is still keyed, never refused.
        std::fs::remove_file(folder.join("common/styles.puml")).unwrap();
        assert_ne!(diagrams.checksum(&request), after);
    }

    #[test]
    fn graphviz_and_d2_files_are_part_of_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        for name in ["logo.png", "shape.svg", "shared.d2", "icons/db.svg"] {
            let path = folder.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "1").unwrap();
        }
        let cases = [
            (
                "graphviz",
                "digraph { a [image=\"logo.png\"]; b [shapefile=shape.svg] }",
            ),
            ("d2", "x: @shared\ndb: {\n  icon: ./icons/db.svg\n}\n"),
        ];
        for (kind, source) in cases {
            let (diagrams, request) = keyed(folder, kind, source);
            let before = diagrams.checksum(&request);
            for input in diagrams.inputs(&request) {
                std::fs::write(&input, "22").unwrap();
                let after = diagrams.checksum(&request);
                assert_ne!(after, before, "{} in {source}", input.display());
                std::fs::write(&input, "1").unwrap();
            }
            assert_eq!(diagrams.inputs(&request).len(), 2, "{source}");
        }
    }

    #[test]
    fn only_local_files_in_the_page_s_folder_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("pages");
        std::fs::create_dir_all(&folder).unwrap();
        let source = "@startuml\n\
            !include ../outside.puml\n\
            !include /etc/passwd\n\
            !include <C4/C4_Container>\n\
            !include https://example.com/x.puml\n\
            !include $STYLES\n\
            !include %getenv(\"HOME\")/x.puml\n\
            !include\n\
            A -> B\n@enduml\n";
        let (diagrams, request) = keyed(&folder, "plantuml", source);
        assert_eq!(diagrams.inputs(&request), Vec::<PathBuf>::new());
        // Without the page's folder nothing resolves.
        let mut request = request;
        request.source = "!include styles.puml\n".to_owned();
        request.directory = None;
        assert_eq!(diagrams.inputs(&request), Vec::<PathBuf>::new());
    }

    #[test]
    fn plantuml_names_files_in_each_directive() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        let source = "@startuml\n\
            !include a.puml!1\n\
            !include_many b.puml\n\
            !include_once \"c d.puml\"\n\
            !includesub e.puml!PART\n\
            !import f.zip\n\
            !theme spacelab from themes\n\
            !theme plain\n\
            A -> B\n@enduml\n";
        let (diagrams, request) = keyed(folder, "plantuml", source);
        let names: Vec<PathBuf> = ["a.puml", "b.puml", "c d.puml", "e.puml", "f.zip"]
            .iter()
            .map(|name| folder.join(name))
            .chain([folder.join("themes/puml-theme-spacelab.puml")])
            .collect();
        assert_eq!(diagrams.inputs(&request), names);
    }

    /// Writes `script` as an executable tool in `dir`.
    #[cfg(unix)]
    fn script(dir: &Path, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let tool = dir.join("tool");
        std::fs::write(&tool, script).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        tool
    }

    #[cfg(unix)]
    #[test]
    fn a_tool_that_hangs_is_stopped_at_the_deadline() {
        let _guard = spawn_guard();
        let dir = tempfile::tempdir().unwrap();
        let tool = script(dir.path(), "#!/bin/sh\nexec sleep 30\n");
        let provider = ExternalProvider::new(vec!["graphviz"], &tool, Vec::new())
            .with_timeout(std::time::Duration::from_millis(200));

        let started = std::time::Instant::now();
        let error = provider
            .render(&DiagramRequest::new("graphviz", "a -> b"))
            .unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(matches!(error, DiagramError::TimedOut { .. }), "{error:?}");
        assert!(error.to_string().contains("did not finish"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn output_over_the_cap_is_refused() {
        let _guard = spawn_guard();
        let dir = tempfile::tempdir().unwrap();
        let tool = script(
            dir.path(),
            "#!/bin/sh\ncat >/dev/null\nhead -c 10000 /dev/zero | tr '\\0' a\n",
        );
        let provider =
            ExternalProvider::new(vec!["graphviz"], &tool, Vec::new()).with_max_output(1000);

        let error = provider
            .render(&DiagramRequest::new("graphviz", "a -> b"))
            .unwrap_err();
        assert!(
            error.to_string().contains("more than 1000 bytes"),
            "{error}"
        );
    }
}
