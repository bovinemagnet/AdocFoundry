//! Handling of `workspace/executeCommand`.
//!
//! Editor-agnostic: this operates on a URI and returns the artefact that was produced,
//! leaving all `lsp-types` mapping to `protocol.rs`.

use std::path::{Path, PathBuf};

use adoc_render::{RenderRequest, RenderSafeMode, Renderer};

use crate::{
    handlers::render_source::source_for_render,
    preview::{scratch_directory, PreviewError, PreviewSink},
    state::{document_path, ServerState},
};

/// Render the document at `uri` and deliver it through `sink`.
///
/// The open buffer is rendered rather than the file on disk, so an unsaved document
/// previews exactly what the author sees.
pub fn render_preview(
    state: &ServerState,
    renderer: &dyn Renderer,
    sink: &dyn PreviewSink,
    uri: &str,
) -> Result<PathBuf, PreviewError> {
    let open = state
        .documents
        .get(uri)
        .ok_or_else(|| PreviewError::NotOpen(uri.to_owned()))?;

    let source = document_path(uri);
    let text = source_for_render(&open.document, &state.antora, &source, &scratch_directory());
    let mut request = RenderRequest::from_source(source.clone(), text);
    request.attributes.extend(antora_attributes(state, &source));
    request.base_dir = antora_component_root(state, &source);
    // Rewritten copies of included files sit in a scratch directory outside the
    // component, which any jailed safe mode refuses to read.
    request.safe_mode = RenderSafeMode::Unsafe;

    let output = renderer.render(&request)?;
    sink.deliver(&output, &source)
}

/// Root of the Antora component owning `source`, if any.
///
/// Asciidoctor's safe mode refuses includes outside its base directory. An Antora page
/// includes partials from sibling directories, so the jail has to be the component root
/// rather than the page's own directory.
fn antora_component_root(state: &ServerState, source: &Path) -> Option<PathBuf> {
    let context = state.antora.context_for_path(source)?;
    let component = state
        .antora
        .component(&context.component, context.version.as_deref())?;
    Some(component.root.clone())
}

/// Antora page attributes for `source`, empty when the file is not in an Antora module.
///
/// `adoc-ls` knows the component and module; a bare Asciidoctor invocation would not.
fn antora_attributes(
    state: &ServerState,
    source: &Path,
) -> impl Iterator<Item = (String, String)> + use<> {
    let context = state.antora.context_for_path(source);
    let module_root = context.as_ref().and_then(|context| {
        state
            .antora
            .module(
                &context.component,
                context.version.as_deref(),
                &context.module,
            )
            .map(|module| module.root.clone())
    });

    // Antora sets these for every page; a document written against them fails under a
    // stock Asciidoctor otherwise. Absolute, so a preview rendered elsewhere still finds
    // partials, examples, and images.
    let families = module_root.into_iter().flat_map(|root| {
        [
            ("partialsdir", "partials"),
            ("examplesdir", "examples"),
            ("attachmentsdir", "attachments"),
            ("imagesdir", "images"),
        ]
        .into_iter()
        .map(move |(attribute, family)| {
            (
                attribute.to_owned(),
                root.join(family).display().to_string(),
            )
        })
    });

    let page = context.into_iter().flat_map(|context| {
        let mut attributes = vec![
            ("page-component-name".to_owned(), context.component),
            ("page-module".to_owned(), context.module),
        ];
        if let Some(version) = context.version {
            attributes.push(("page-component-version".to_owned(), version));
        }
        attributes
    });

    page.chain(families)
}

#[cfg(test)]
mod tests {
    use super::*;
    use adoc_render::MockRenderer;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingSink {
        delivered: Mutex<Vec<(String, PathBuf)>>,
    }

    impl PreviewSink for RecordingSink {
        fn deliver(
            &self,
            output: &adoc_render::RenderOutput,
            source: &Path,
        ) -> Result<PathBuf, PreviewError> {
            self.delivered
                .lock()
                .unwrap()
                .push((output.html.clone(), source.to_path_buf()));
            Ok(source.with_extension("html"))
        }
    }

    fn state_with(uri: &str, text: &str) -> ServerState {
        let mut state = ServerState::default();
        state.open(uri, text, 1);
        state
    }

    #[test]
    fn renders_the_unsaved_buffer_rather_than_the_file_on_disk() {
        let uri = "file:///docs/guide.adoc";
        let state = state_with(uri, "= Unsaved Title\n");
        let renderer = RecordingRenderer::default();
        let sink = RecordingSink::default();

        render_preview(&state, &renderer, &sink, uri).expect("render");

        let request = renderer.last_request().expect("a request was made");
        assert_eq!(request.source_text.as_deref(), Some("= Unsaved Title\n"));
    }

    #[test]
    fn delivers_rendered_html_for_the_document_path() {
        let uri = "file:///docs/guide.adoc";
        let state = state_with(uri, "= Title\n");
        let sink = RecordingSink::default();

        render_preview(&state, &MockRenderer::new("<h1>Title</h1>"), &sink, uri).expect("render");

        let delivered = sink.delivered.lock().unwrap();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].0, "<h1>Title</h1>");
        assert_eq!(delivered[0].1, PathBuf::from("/docs/guide.adoc"));
    }

    #[test]
    fn reports_a_document_that_is_not_open() {
        let state = ServerState::default();
        let sink = RecordingSink::default();

        let error = render_preview(
            &state,
            &MockRenderer::new("<p/>"),
            &sink,
            "file:///gone.adoc",
        )
        .expect_err("closed documents cannot be previewed");

        assert!(matches!(error, PreviewError::NotOpen(_)), "{error:?}");
    }

    #[test]
    fn returns_the_artefact_path_from_the_sink() {
        let uri = "file:///docs/guide.adoc";
        let state = state_with(uri, "= Title\n");

        let artefact = render_preview(
            &state,
            &MockRenderer::new("<h1>Title</h1>"),
            &RecordingSink::default(),
            uri,
        )
        .expect("render");

        assert_eq!(artefact, PathBuf::from("/docs/guide.html"));
    }

    #[test]
    fn merges_antora_page_attributes_into_the_request() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let page = root.join("modules/security/pages/authentication.adoc");
        let uri = format!("file://{}", page.display());

        let mut state = ServerState::default();
        state.index_workspace(vec![root]).expect("index fixture");
        state.open(&uri, "= Authentication\n", 1);

        let renderer = RecordingRenderer::default();
        render_preview(&state, &renderer, &RecordingSink::default(), &uri).expect("render");

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
    fn a_document_outside_an_antora_module_contributes_no_page_attributes() {
        let uri = "file:///elsewhere/guide.adoc";
        let state = state_with(uri, "= Title\n");

        let renderer = RecordingRenderer::default();
        render_preview(&state, &renderer, &RecordingSink::default(), uri).expect("render");

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

        let mut state = ServerState::default();
        state.index_workspace(vec![root]).expect("index fixture");
        state.open(
            &uri,
            "= Authentication\n\ninclude::partial$token-note.adoc[]\n",
            1,
        );

        let renderer = RecordingRenderer::default();
        render_preview(&state, &renderer, &RecordingSink::default(), &uri).expect("render");

        let source = renderer
            .last_request()
            .expect("a request")
            .source_text
            .expect("source text");
        assert!(!source.contains("partial$"), "{source}");
        assert!(
            source.contains("modules/security/partials/token-note.adoc"),
            "{source}"
        );
    }

    #[test]
    fn widens_the_safe_mode_jail_to_the_antora_component_root() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/antora-single-component");
        let page = root.join("modules/security/pages/authentication.adoc");
        let uri = format!("file://{}", page.display());

        let mut state = ServerState::default();
        state.index_workspace(vec![root]).expect("index fixture");
        state.open(&uri, "= Authentication\n", 1);

        let renderer = RecordingRenderer::default();
        render_preview(&state, &renderer, &RecordingSink::default(), &uri).expect("render");

        let base_dir = renderer
            .last_request()
            .expect("a request")
            .base_dir
            .expect("an Antora page needs a widened jail");
        assert!(
            base_dir.join("antora.yml").is_file(),
            "{}",
            base_dir.display()
        );
    }

    #[test]
    fn a_document_outside_an_antora_module_keeps_the_default_jail() {
        let uri = "file:///elsewhere/guide.adoc";
        let state = state_with(uri, "= Title\n");

        let renderer = RecordingRenderer::default();
        render_preview(&state, &renderer, &RecordingSink::default(), uri).expect("render");

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

        let mut state = ServerState::default();
        state.index_workspace(vec![root]).expect("index fixture");
        state.open(&uri, "= Composed\n", 1);

        let renderer = RecordingRenderer::default();
        render_preview(&state, &renderer, &RecordingSink::default(), &uri).expect("render");

        let attributes = renderer.last_request().expect("a request").attributes;
        for (name, family) in [
            ("partialsdir", "partials"),
            ("examplesdir", "examples"),
            ("attachmentsdir", "attachments"),
            ("imagesdir", "images"),
        ] {
            let value = attributes
                .get(name)
                .unwrap_or_else(|| panic!("{name} is not set: {attributes:?}"));
            assert!(
                value.ends_with(&format!("modules/ROOT/{family}")),
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
        render_preview(&state, &renderer, &RecordingSink::default(), uri).expect("render");

        assert_eq!(
            renderer.last_request().expect("a request").safe_mode,
            adoc_render::RenderSafeMode::Unsafe
        );
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
