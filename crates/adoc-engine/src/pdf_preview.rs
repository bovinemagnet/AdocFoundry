//! The live PDF preview: while the preview shows PDF pages (`PreviewLayout::pdf`), each new
//! view of the preview target is typeset on a worker as the PDF export would typeset the page,
//! with the theme and fonts chosen as the export chooses them, and published as SVG pages with
//! the source of each line ([`EngineEvent::PdfPreviewReady`]). One typesetting runs at a time;
//! a view that arrives meanwhile waits, and only the newest is typeset next.

use std::{collections::HashMap, path::PathBuf, sync::Arc, thread};

use adoc_pdf::{Document, PageView, PreviewPage};

use crate::{workspace::WorkerMessage, DocumentId, Engine, EngineEvent, ExportArgs, ExportTarget};

/// A page typeset for the PDF preview, or why it could not be.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfPreview {
    pub document: DocumentId,
    /// The document version the pages show.
    pub version: u64,
    pub path: PathBuf,
    pub pages: Result<Vec<PreviewPage>, String>,
}

#[derive(Default)]
pub(crate) struct PdfPreviewState {
    /// A typesetting is under way.
    running: bool,
    /// The document to typeset once it finishes.
    waiting: Option<DocumentId>,
    published: HashMap<DocumentId, Arc<PdfPreview>>,
}

impl Engine {
    /// The latest PDF preview of `id`, once one has been typeset.
    #[must_use]
    pub fn pdf_preview(&self, id: DocumentId) -> Option<Arc<PdfPreview>> {
        self.pdf_preview.published.get(&id).cloned()
    }

    /// Whether the PDF preview is typesetting.
    #[must_use]
    pub fn is_typesetting_preview(&self) -> bool {
        self.pdf_preview.running
    }

    /// Typesets the preview target's latest view, now or after the one being typeset, when the
    /// preview shows PDF pages.
    pub(crate) fn request_pdf_preview(&mut self, id: DocumentId) {
        let layout = self.preview_layout();
        if !(layout.visible && layout.pdf) || self.preview_target() != Some(id) {
            return;
        }
        if self.pdf_preview.running {
            self.pdf_preview.waiting = Some(id);
            return;
        }
        let Some(preview) = self.rendered_preview(id) else {
            return;
        };
        let args = ExportArgs {
            target: ExportTarget::Page(id),
            output: PathBuf::new(),
            theme: None,
            standards: Vec::new(),
            fonts_dir: None,
            typst: None,
        };
        let attributes = preview.view.attributes.clone();
        let source = self.theme_source(None, &attributes, Some(&preview.path));
        let mut options = self.export_options(&args, &attributes, Some(&preview.path));
        // Tagging and standards change nothing on screen.
        options.standards.clear();
        let access = self.file_access.clone();
        let sender = self.worker_sender.clone();
        let version = preview.version;
        self.pdf_preview.running = true;
        self.workers_running += 1;
        thread::spawn(move || {
            let pages = adoc_pdf::load_theme_within(&source, &access)
                .map_err(|error| error.to_string())
                .and_then(|theme| {
                    let document = Document::Article(PageView {
                        key: "p0".to_owned(),
                        view: preview.view.clone(),
                        depth: 0,
                    });
                    let typst = adoc_pdf::to_typst(&document, &theme, &options);
                    adoc_pdf::preview_pages(&typst, &theme, &options)
                        .map_err(|error| error.to_string())
                });
            let _ = sender.send(WorkerMessage::PdfPreviewTypeset {
                preview: Box::new(PdfPreview {
                    document: id,
                    version,
                    path: preview.path.clone(),
                    pages,
                }),
            });
        });
    }

    pub(crate) fn pdf_preview_typeset(&mut self, preview: PdfPreview) {
        self.pdf_preview.running = false;
        let (id, version) = (preview.document, preview.version);
        if self.documents.contains_key(&id) {
            self.pdf_preview.published.insert(id, Arc::new(preview));
            self.emit(EngineEvent::PdfPreviewReady { id, version });
        }
        if let Some(next) = self.pdf_preview.waiting.take() {
            self.request_pdf_preview(next);
        }
    }

    pub(crate) fn forget_pdf_preview(&mut self, id: DocumentId) {
        self.pdf_preview.published.remove(&id);
        if self.pdf_preview.waiting == Some(id) {
            self.pdf_preview.waiting = None;
        }
    }
}
