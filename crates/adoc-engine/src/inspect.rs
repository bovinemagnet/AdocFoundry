//! Questions about the workspace that need no open document, for headless front ends such as
//! the MCP server: what a target resolves to from a page, and the Antora resource ids of files.

use std::path::{Path, PathBuf};

use adoc_antora::{parse_resource_id, AntoraContext, AntoraResourceId, ResourceFamily};
use adoc_index::normalize_path;
use adoc_lang::DefinitionTarget;

use crate::{Engine, EngineError};

/// `id` written out in full: `version@component:module:family$path`.
#[must_use]
pub fn resource_id_string(id: &AntoraResourceId) -> String {
    let mut out = String::new();
    if let Some(version) = &id.version {
        out.push_str(version);
        out.push('@');
    }
    if let Some(component) = &id.component {
        out.push_str(component);
        out.push(':');
    }
    if let Some(module) = &id.module {
        out.push_str(module);
        out.push(':');
    }
    if let Some(family) = id.family {
        out.push_str(&format!("{family}$"));
    }
    out.push_str(&id.path);
    out
}

impl Engine {
    /// What `target` resolves to from the page at `from`: an xref, include or image macro as
    /// written (`xref:page.adoc#id[]`, `include::partial$x.adoc[]`, `image::x.png[]`), or a bare
    /// xref target. The page's own attributes apply.
    #[must_use]
    pub fn resolve_target(&self, from: &Path, target: &str) -> Option<DefinitionTarget> {
        let from = normalize_path(from);
        let target = target.trim();
        let line = if ["xref:", "include::", "image:", "<<"]
            .iter()
            .any(|prefix| target.starts_with(prefix))
        {
            if target.ends_with(']') || target.ends_with(">>") {
                target.to_owned()
            } else {
                format!("{target}[]")
            }
        } else {
            format!("xref:{target}[]")
        };
        let page = self
            .document_for_path(&from)
            .and_then(|id| self.document(id))
            .map(|document| document.parsed().text.to_string())
            .or_else(|| {
                self.workspace
                    .index
                    .file(&from)
                    .and_then(|file| file.text().map(std::borrow::Cow::into_owned))
            })
            .unwrap_or_default();
        let text = format!("{page}\n\n{line}\n");
        let offset = page.len() + 2 + line.find(':').map_or(2, |colon| colon + 2);
        adoc_lang::definition_at_offset(&self.workspace, &from, &text, offset)
    }

    /// The fully qualified resource id of the file at `path`, if it is an Antora resource.
    #[must_use]
    pub fn resource_id(&self, path: &Path) -> Option<String> {
        let resource = self.workspace.antora.resource_for_path(path)?;
        let coordinate = &resource.coordinate;
        Some(resource_id_string(&AntoraResourceId {
            version: coordinate.version.clone(),
            component: Some(coordinate.component.clone()),
            module: Some(coordinate.module.clone()),
            family: Some(coordinate.family),
            path: coordinate
                .relative_path
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
        }))
    }

    /// The file a resource id names, read from the page at `from` when given; without one the
    /// id must name its component.
    pub fn resolve_resource_id(
        &self,
        id: &str,
        from: Option<&Path>,
    ) -> Result<PathBuf, EngineError> {
        let parsed = parse_resource_id(id)
            .map_err(|error| EngineError::InvalidRequest(format!("{id}: {error}")))?;
        let context = match from {
            Some(from) => self
                .workspace
                .antora
                .context_for_path(from)
                .ok_or_else(|| {
                    EngineError::InvalidRequest(format!(
                        "{} is not in an Antora component",
                        from.display()
                    ))
                })?,
            None => AntoraContext {
                component: parsed.component.clone().ok_or_else(|| {
                    EngineError::InvalidRequest(format!(
                        "{id} names no component, and there is no page to read it from"
                    ))
                })?,
                version: parsed.version.clone(),
                module: parsed.module.clone().unwrap_or_else(|| "ROOT".to_owned()),
                family: ResourceFamily::Page,
            },
        };
        self.workspace
            .antora
            .resolve_detailed(&parsed, &context)
            .map(|resolved| resolved.resource.source_path.clone())
            .map_err(|error| EngineError::NotFound(format!("{id}: {error}")))
    }
}
