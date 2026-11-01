//! HTML export: a page as a standalone HTML file, rendered from the same view tree as the
//! preview and `adocfoundry-cli render`, with its images and diagrams embedded so the file
//! opens anywhere. The page is rendered as the preview renders it, from its open buffer; once
//! that render is current and no diagram is pending, the file is written on a worker and the
//! result arrives as [`EngineEvent::ExportFinished`].

use std::{path::PathBuf, sync::Arc};

use adoc_render::HtmlOptions;

use crate::{
    commands::{ids, register, spec},
    workspace::WorkerMessage,
    CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry, DocumentId,
    Engine, EngineError, EngineEvent, ExportArgs, ExportSummary, ExportTarget, RenderedPreview,
};

/// Pages waiting for a current render before they are written.
#[derive(Default)]
pub(crate) struct HtmlExports {
    waiting: Vec<(DocumentId, PathBuf)>,
}

pub(crate) fn register_html_export(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::EXPORT_HTML_PAGE,
            "Export Page to HTML…",
            CommandCategory::Document,
            None,
            CommandContext::ActiveDocument,
        ),
        export_html_command,
    );
}

/// Takes `CommandArgs::Export` with a page target; the PDF options are not used.
fn export_html_command(
    engine: &mut Engine,
    args: CommandArgs,
) -> Result<CommandOutcome, EngineError> {
    let CommandArgs::Export(ExportArgs {
        target: ExportTarget::Page(id),
        output,
        ..
    }) = args
    else {
        return Err(EngineError::InvalidArguments {
            command: ids::EXPORT_HTML_PAGE,
            expected: "a page and an output path",
        });
    };
    engine
        .document(id)
        .ok_or(EngineError::UnknownDocument(id))?;
    engine.render_now(id);
    engine.html_exports.waiting.push((id, output));
    Ok(CommandOutcome::Done)
}

impl Engine {
    /// Writes each waiting page whose render is current and complete. Returns the number
    /// started.
    pub(crate) fn write_due_html_exports(&mut self) -> usize {
        let mut started = 0;
        let mut index = 0;
        while index < self.html_exports.waiting.len() {
            let id = self.html_exports.waiting[index].0;
            let Some(preview) = self.finished_preview(id) else {
                index += 1;
                continue;
            };
            let (_, output) = self.html_exports.waiting.remove(index);
            self.release_preview(id);
            self.write_html(preview, output);
            started += 1;
        }
        started
    }

    pub(crate) fn has_due_html_export(&self) -> bool {
        self.html_exports
            .waiting
            .iter()
            .any(|(id, _)| self.finished_preview(*id).is_some())
    }

    fn write_html(&mut self, preview: Arc<RenderedPreview>, output: PathBuf) {
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let written = output.clone();
        let crashed = move |error| WorkerMessage::HtmlExported {
            output: written,
            result: Err(format!("writing the HTML crashed: {error}")),
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let html = adoc_render::embed_local_files(&adoc_render::to_html(
                &preview.view,
                &HtmlOptions { standalone: true },
            ));
            let result = std::fs::write(&output, html)
                .map_err(|error| format!("{}: {error}", output.display()));
            let _ = sender.send(WorkerMessage::HtmlExported { output, result });
        });
    }

    pub(crate) fn html_exported(&mut self, output: PathBuf, result: Result<(), String>) {
        let result = result.map(|()| ExportSummary {
            output: output.clone(),
            pages: 1,
            diagnostics: Vec::new(),
        });
        self.emit(EngineEvent::ExportFinished { output, result });
    }

    /// Reports the exports of a page closed before it was written.
    pub(crate) fn forget_html_exports(&mut self, id: DocumentId) {
        let (closed, waiting) = std::mem::take(&mut self.html_exports.waiting)
            .into_iter()
            .partition(|(waiting, _)| *waiting == id);
        self.html_exports.waiting = waiting;
        for (_, output) in closed {
            self.emit(EngineEvent::ExportFinished {
                output,
                result: Err("the page was closed before it was exported".to_owned()),
            });
        }
    }
}
