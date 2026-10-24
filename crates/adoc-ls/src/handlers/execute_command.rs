//! Handling of `workspace/executeCommand`'s preview commands.
//!
//! Editor-agnostic: this operates on a URI and returns the artefact that was produced,
//! leaving all `lsp-types` mapping to `protocol.rs`. A page renders through Asciidoctor when
//! it is configured or on `PATH`, as it always has, and otherwise through the engine's view
//! tree, with diagrams and, in builds with Typst, typeset STEM.

use std::{path::PathBuf, time::Duration};

use adoc_engine::Engine;
use adoc_render::{RenderOutput, RenderRequest, RenderSafeMode, Renderer};

use adoc_lang::{
    attributes::{antora_attributes, component_attributes},
    render_source::source_for_render,
};

use crate::{
    preview::{scratch_directory, PreviewError, PreviewMode, PreviewSink},
    state::document_path,
};

/// How long the engine may take to render a page and its diagrams.
const RENDER_TIMEOUT: Duration = Duration::from_secs(60);

/// Render the document at `uri` with `renderer` and deliver it through `sink`.
///
/// The open buffer is rendered rather than the file on disk, so an unsaved document
/// previews exactly what the author sees.
pub fn render_preview(
    engine: &Engine,
    renderer: &dyn Renderer,
    sink: &dyn PreviewSink,
    uri: &str,
    mode: PreviewMode,
) -> Result<PathBuf, PreviewError> {
    let (source, request) = preview_request(engine, uri)?;
    let output = renderer.render(&request)?;
    sink.deliver(&output, &source, mode)
}

/// Render the document at `uri` through the engine's view tree and deliver it through `sink`.
pub fn render_preview_with_engine(
    engine: &mut Engine,
    sink: &dyn PreviewSink,
    uri: &str,
    mode: PreviewMode,
) -> Result<PathBuf, PreviewError> {
    let (source, output) = engine_output(engine, uri)?;
    sink.deliver(&output, &source, mode)
}

/// The engine's rendering of the document at `uri`, as a standalone page.
fn engine_output(engine: &mut Engine, uri: &str) -> Result<(PathBuf, RenderOutput), PreviewError> {
    let source = document_path(uri);
    let id = engine
        .document_for_path(&source)
        .ok_or_else(|| PreviewError::NotOpen(uri.to_owned()))?;
    let preview = engine
        .render_blocking(id, RENDER_TIMEOUT)
        .ok_or_else(|| PreviewError::NotRendered(source.clone()))?;
    let html = adoc_render::to_html(
        &preview.view,
        &adoc_render::HtmlOptions { standalone: true },
    );
    Ok((
        source,
        RenderOutput {
            html,
            warnings: Vec::new(),
        },
    ))
}

/// The source path and render request for the document at `uri`.
fn preview_request(engine: &Engine, uri: &str) -> Result<(PathBuf, RenderRequest), PreviewError> {
    let source = document_path(uri);
    let open = engine
        .document_for_path(&source)
        .and_then(|id| engine.document(id))
        .ok_or_else(|| PreviewError::NotOpen(uri.to_owned()))?;
    let antora = &engine.workspace().antora;

    let text = source_for_render(&open.parsed, antora, &source, &scratch_directory());
    let mut request = RenderRequest::from_source(source.clone(), text);
    // Component attributes first, so the page and family attributes Antora computes itself
    // win should `antora.yml` name one of them.
    request.attributes.extend(
        component_attributes(antora, &source)
            .map(|(name, value)| (name.to_owned(), value.to_owned())),
    );
    request
        .attributes
        .extend(antora_attributes(antora, &source));
    // Rewritten copies of included files sit in a scratch directory outside the
    // component, which any jailed safe mode refuses to read.
    request.safe_mode = RenderSafeMode::Unsafe;

    Ok((source, request))
}

/// Re-render a live preview in place, without presenting it again, with `renderer` or, when
/// there is none, the engine.
///
/// Called when a document being previewed is saved; the artefact is overwritten so the
/// page picks the change up on its next reload, and focus stays in the editor.
pub fn refresh_preview(
    engine: &mut Engine,
    renderer: Option<&dyn Renderer>,
    sink: &dyn PreviewSink,
    uri: &str,
) -> Result<PathBuf, PreviewError> {
    let (source, output) = match renderer {
        Some(renderer) => {
            let (source, request) = preview_request(engine, uri)?;
            (source, renderer.render(&request)?)
        }
        None => engine_output(engine, uri)?,
    };
    sink.refresh(&output, &source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use adoc_render::MockRenderer;
    use std::{path::Path, sync::Mutex};

    #[derive(Default)]
    struct RecordingSink {
        delivered: Mutex<Vec<(String, PathBuf)>>,
        refreshed: Mutex<Vec<(String, PathBuf)>>,
        mode: Mutex<Option<PreviewMode>>,
    }

    impl PreviewSink for RecordingSink {
        fn deliver(
            &self,
            output: &adoc_render::RenderOutput,
            source: &Path,
            mode: PreviewMode,
        ) -> Result<PathBuf, PreviewError> {
            self.delivered
                .lock()
                .unwrap()
                .push((output.html.clone(), source.to_path_buf()));
            *self.mode.lock().unwrap() = Some(mode);
            Ok(source.with_extension("html"))
        }

        fn refresh(
            &self,
            output: &adoc_render::RenderOutput,
            source: &Path,
        ) -> Result<PathBuf, PreviewError> {
            self.refreshed
                .lock()
                .unwrap()
                .push((output.html.clone(), source.to_path_buf()));
            Ok(source.with_extension("html"))
        }
    }

    /// A page's path and the URI a client on this platform sends for it.
    fn guide() -> (PathBuf, String) {
        let path =
            adoc_index::normalize_path(&std::env::temp_dir().join("docs").join("guide.adoc"));
        let uri = url::Url::from_file_path(&path).unwrap().to_string();
        (path, uri)
    }

    fn state_with(uri: &str, text: &str) -> Engine {
        let mut engine = Engine::new(adoc_engine::Settings::default());
        engine.open_text(&document_path(uri), text);
        engine
    }

    fn workspace_with(root: PathBuf, uri: &str, text: &str) -> Engine {
        let mut engine = Engine::new(adoc_engine::Settings::default());
        engine.open_workspace(vec![root]).expect("index fixture");
        assert!(engine.wait_for_workers(std::time::Duration::from_secs(10)));
        engine.open_text(&document_path(uri), text);
        engine
    }

    #[test]
    fn renders_the_unsaved_buffer_rather_than_the_file_on_disk() {
        let uri = "file:///docs/guide.adoc";
        let state = state_with(uri, "= Unsaved Title\n");
        let renderer = RecordingRenderer::default();
        let sink = RecordingSink::default();

        render_preview(&state, &renderer, &sink, uri, PreviewMode::Static).expect("render");

        let request = renderer.last_request().expect("a request was made");
        assert_eq!(request.source_text.as_deref(), Some("= Unsaved Title\n"));
    }

    #[test]
    fn delivers_rendered_html_for_the_document_path() {
        let (path, uri) = guide();
        let uri = uri.as_str();
        let state = state_with(uri, "= Title\n");
        let sink = RecordingSink::default();

        render_preview(
            &state,
            &MockRenderer::new("<h1>Title</h1>"),
            &sink,
            uri,
            PreviewMode::Static,
        )
        .expect("render");

        let delivered = sink.delivered.lock().unwrap();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, "<h1>Title</h1>");
        assert_eq!(delivered[0].1, path);
    }

    #[test]
    fn reports_a_document_that_is_not_open() {
        let state = Engine::new(adoc_engine::Settings::default());
        let sink = RecordingSink::default();

        let error = render_preview(
            &state,
            &MockRenderer::new("<p/>"),
            &sink,
            "file:///gone.adoc",
            PreviewMode::Static,
        )
        .expect_err("closed documents cannot be previewed");

        assert!(matches!(error, PreviewError::NotOpen(_)), "{error:?}");
    }

    #[test]
    fn returns_the_artefact_path_from_the_sink() {
        let (path, uri) = guide();
        let uri = uri.as_str();
        let state = state_with(uri, "= Title\n");

        let artefact = render_preview(
            &state,
            &MockRenderer::new("<h1>Title</h1>"),
            &RecordingSink::default(),
            uri,
            PreviewMode::Static,
        )
        .expect("render");

        assert_eq!(artefact, path.with_extension("html"));
    }

    #[test]
    fn merges_antora_page_attributes_into_the_request() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let page = root.join("modules/security/pages/authentication.adoc");
        let uri = format!("file://{}", page.display());

        let state = workspace_with(root, &uri, "= Authentication\n");

        let renderer = RecordingRenderer::default();
        render_preview(
            &state,
            &renderer,
            &RecordingSink::default(),
            &uri,
            PreviewMode::Static,
        )
        .expect("render");

        let attributes = renderer.last_request().expect("a request").attributes;
        assert_eq!(
            attributes.get("page-component-name").map(String::as_str),
            Some("demo")
        );
        assert_eq!(
            attributes.get("page-module").map(String::as_str),
            Some("security")
        );
        assert_eq!(
            attributes.get("page-component-version").map(String::as_str),
            Some("latest")
        );
    }

    #[test]
    fn merges_the_component_s_asciidoc_attributes_into_the_request() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let page = root.join("modules/ROOT/pages/index.adoc");
        let uri = format!("file://{}", page.display());

        let state = workspace_with(root, &uri, "= Home\n");

        let renderer = RecordingRenderer::default();
        render_preview(
            &state,
            &renderer,
            &RecordingSink::default(),
            &uri,
            PreviewMode::Static,
        )
        .expect("render");

        let attributes = renderer.last_request().expect("a request").attributes;
        assert_eq!(
            attributes.get("source-highlighter").map(String::as_str),
            Some("highlight.js")
        );
        assert_eq!(attributes.get("sectanchors").map(String::as_str), Some(""));
    }

    #[test]
    fn a_document_outside_an_antora_module_contributes_no_page_attributes() {
        let uri = "file:///elsewhere/guide.adoc";
        let state = state_with(uri, "= Title\n");

        let renderer = RecordingRenderer::default();
        render_preview(
            &state,
            &renderer,
            &RecordingSink::default(),
            uri,
            PreviewMode::Static,
        )
        .expect("render");

        assert!(renderer
            .last_request()
            .expect("a request")
            .attributes
            .is_empty());
    }

    #[test]
    fn resolves_antora_includes_before_rendering() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let page = root.join("modules/security/pages/authentication.adoc");
        let uri = format!("file://{}", page.display());

        let state = workspace_with(
            root,
            &uri,
            "= Authentication\n\ninclude::partial$token-note.adoc[]\n",
        );

        let renderer = RecordingRenderer::default();
        render_preview(
            &state,
            &renderer,
            &RecordingSink::default(),
            &uri,
            PreviewMode::Static,
        )
        .expect("render");

        let source = renderer
            .last_request()
            .expect("a request")
            .source_text
            .expect("source text");
        assert!(!source.contains("partial$"), "{source}");
        // The rewritten include is an absolute path, with this platform's separators.
        assert!(
            source
                .replace('\\', "/")
                .contains("modules/security/partials/token-note.adoc"),
            "{source}"
        );
    }

    #[test]
    fn widens_the_safe_mode_jail_to_the_antora_component_root() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let page = root.join("modules/security/pages/authentication.adoc");
        let uri = format!("file://{}", page.display());

        let state = workspace_with(root, &uri, "= Authentication\n");

        let renderer = RecordingRenderer::default();
        render_preview(
            &state,
            &renderer,
            &RecordingSink::default(),
            &uri,
            PreviewMode::Static,
        )
        .expect("render");

        assert!(renderer
            .last_request()
            .expect("a request")
            .base_dir
            .is_none());
    }

    #[test]
    fn a_document_outside_an_antora_module_keeps_the_default_jail() {
        let uri = "file:///elsewhere/guide.adoc";
        let state = state_with(uri, "= Title\n");

        let renderer = RecordingRenderer::default();
        render_preview(
            &state,
            &renderer,
            &RecordingSink::default(),
            uri,
            PreviewMode::Static,
        )
        .expect("render");

        assert!(renderer
            .last_request()
            .expect("a request")
            .base_dir
            .is_none());
    }

    #[test]
    fn sets_the_antora_family_directory_attributes() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/antora-nested-pages");
        let page = root.join("modules/ROOT/pages/guides/composed.adoc");
        let uri = format!("file://{}", page.display());

        let state = workspace_with(root.clone(), &uri, "= Composed\n");

        let renderer = RecordingRenderer::default();
        render_preview(
            &state,
            &renderer,
            &RecordingSink::default(),
            &uri,
            PreviewMode::Static,
        )
        .expect("render");

        let attributes = renderer.last_request().expect("a request").attributes;
        let moduledir = attributes.get("moduledir").expect("moduledir is not set");
        assert!(
            moduledir.replace('\\', "/").ends_with("modules/ROOT"),
            "moduledir = {moduledir}"
        );
        for (name, family) in [
            ("pagesdir", "pages"),
            ("partialsdir", "partials"),
            ("examplesdir", "examples"),
            ("attachmentsdir", "attachments"),
            ("imagesdir", "images"),
        ] {
            let value = attributes
                .get(name)
                .unwrap_or_else(|| panic!("{name} is not set: {attributes:?}"));
            assert!(
                value
                    .replace('\\', "/")
                    .ends_with(&format!("modules/ROOT/{family}/")),
                "{name} = {value}"
            );
        }
    }

    /// Rewritten copies of included files live outside the component's jail, so any safe
    /// mode above `unsafe` refuses to read them.
    #[test]
    fn renders_unsafe_so_rewritten_includes_are_readable() {
        let uri = "file:///docs/guide.adoc";
        let state = state_with(uri, "= Title\n");

        let renderer = RecordingRenderer::default();
        render_preview(
            &state,
            &renderer,
            &RecordingSink::default(),
            uri,
            PreviewMode::Static,
        )
        .expect("render");

        assert_eq!(
            renderer.last_request().expect("a request").safe_mode,
            adoc_render::RenderSafeMode::Unsafe
        );
    }

    #[test]
    fn a_live_preview_is_delivered_in_live_mode() {
        let uri = "file:///docs/guide.adoc";
        let state = state_with(uri, "= Title\n");
        let sink = RecordingSink::default();

        render_preview(
            &state,
            &MockRenderer::new("<h1>T</h1>"),
            &sink,
            uri,
            PreviewMode::Live,
        )
        .expect("render");

        assert_eq!(*sink.mode.lock().unwrap(), Some(PreviewMode::Live));
    }

    #[test]
    fn refreshing_updates_without_redelivering() {
        let uri = "file:///docs/guide.adoc";
        let mut state = state_with(uri, "= Title\n");
        let sink = RecordingSink::default();

        refresh_preview(
            &mut state,
            Some(&MockRenderer::new("<h1>T</h1>")),
            &sink,
            uri,
        )
        .expect("refresh");

        assert!(sink.delivered.lock().unwrap().is_empty());
        assert_eq!(sink.refreshed.lock().unwrap().len(), 1);
    }

    /// A renderer that captures the request it was given.
    #[derive(Default)]
    struct RecordingRenderer {
        requests: Mutex<Vec<RenderRequest>>,
    }

    impl RecordingRenderer {
        fn last_request(&self) -> Option<RenderRequest> {
            self.requests.lock().unwrap().last().cloned()
        }
    }

    impl adoc_render::Renderer for RecordingRenderer {
        fn render(
            &self,
            request: &RenderRequest,
        ) -> Result<adoc_render::RenderOutput, adoc_render::RenderError> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(adoc_render::RenderOutput {
                html: String::new(),
                warnings: Vec::new(),
            })
        }
    }
}
