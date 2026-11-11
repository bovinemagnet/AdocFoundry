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

    /// Now, when a waiting export's preview has finished.
    pub(crate) fn due_html_export(&self) -> Option<std::time::Instant> {
        self.html_exports
            .waiting
            .iter()
            .any(|(id, _)| self.finished_preview(*id).is_some())
            .then(|| self.clock.now())
    }

    /// The site's address: `[pdf] site_url`, else the playbook's `site.url`.
    pub(crate) fn site_url(&self) -> Option<String> {
        self.settings.pdf.site_url.clone().or_else(|| {
            self.workspace
                .antora
                .playbook()
                .and_then(|playbook| playbook.site_url.clone())
        })
    }

    /// Options for writing a page as HTML: whole, or its content alone, with xrefs to other
    /// pages leading to their published addresses when the site's address is known, else to
    /// where each page's own export would sit beside its source.
    #[must_use]
    pub fn html_options(&self, standalone: bool) -> HtmlOptions {
        HtmlOptions {
            standalone,
            page_urls: self
                .site_url()
                .as_deref()
                .map(|site| self.page_urls(site))
                .unwrap_or_default(),
        }
    }

    /// The published address of every page in the catalogue, as Antora lays out a site:
    /// `<site>/<component>/<version>/<module>/<page>.html`, without `ROOT` or `~`.
    pub(crate) fn page_urls(&self, site: &str) -> std::collections::HashMap<PathBuf, String> {
        let site = site.trim_end_matches('/');
        self.workspace
            .antora
            .resources()
            .filter(|resource| resource.coordinate.family == crate::ResourceFamily::Page)
            .map(|resource| {
                let coordinate = &resource.coordinate;
                let mut url = format!("{site}/{}", coordinate.component);
                if let Some(version) = coordinate
                    .version
                    .as_deref()
                    .filter(|version| *version != "~")
                {
                    url.push('/');
                    url.push_str(version);
                }
                if coordinate.module != "ROOT" {
                    url.push('/');
                    url.push_str(&coordinate.module);
                }
                let page = coordinate
                    .relative_path
                    .with_extension("html")
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                url.push('/');
                url.push_str(&page);
                (resource.source_path.clone(), url)
            })
            .collect()
    }

    fn write_html(&mut self, preview: Arc<RenderedPreview>, output: PathBuf) {
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let written = output.clone();
        let options = self.html_options(true);
        let crashed = move |error| WorkerMessage::HtmlExported {
            output: written,
            result: Err(format!("writing the HTML crashed: {error}")),
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let html =
                adoc_render::embed_local_files(&adoc_render::to_html(&preview.view, &options));
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
