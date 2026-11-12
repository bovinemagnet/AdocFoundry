//! The discovery tools (MCP PRD §7.2): `list_pages`, `get_outline`, `get_page_context` and
//! `find_references`, so a client finds its way around the workspace without reading every
//! file. Each is an adapter over the engine's catalog, index and graph queries
//! (`adoc_engine::discovery`), adding paging, root-relative paths and the stable error codes.

use std::path::{Path, PathBuf};

use adoc_core::{LineIndex, SourceRange};
use adoc_engine::{
    AntoraCoordinate, AttributeOrigin, DefinitionTarget, DocumentSymbol, DocumentSymbolKind,
    ReferenceKind, ResourceFamily, ResourceQuery,
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    responses::{cursor, page, page_size, paged, resume, Failure, Page},
    schemas,
    server::{limited_text, Answer, Server, State, MAX_TEXT},
};

/// The version of the discovery tools' answers.
const SCHEMA_VERSION: u32 = 1;

#[derive(Deserialize, JsonSchema)]
struct ListPagesArgs {
    /// Only files beneath this directory, absolute or relative to the first root.
    root: Option<String>,
    /// The component's name.
    component: Option<String>,
    /// One version of `component`: `~` for the versionless one, else its name, such as `3.0`.
    /// Without it every version is listed; it needs a `component`.
    version: Option<String>,
    /// One module of `component`; it needs a `component`.
    module: Option<String>,
    /// `page` (the default), `partial`, `example`, `image`, `attachment`, or `any`.
    family: Option<String>,
    /// The `next_cursor` of the page before, to list the next; the first page without it.
    cursor: Option<String>,
    /// At most this many items in the page, up to 500 (the default).
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
struct TargetArgs {
    /// The file's path, absolute or relative to the first root.
    path: Option<String>,
    /// Or its Antora resource id, `version@component:module:family$path`.
    resource_id: Option<String>,
    /// The page a partial resource id is read from.
    from: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct OutlineArgs {
    #[serde(flatten)]
    target: TargetArgs,
    /// Give the sections (and a navigation file's entries); true by default.
    headings: Option<bool>,
    /// Give the anchors the page declares; true by default.
    anchors: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct PageContextArgs {
    #[serde(flatten)]
    target: TargetArgs,
    /// Where the page sits in its version's navigation; true by default.
    navigation: Option<bool>,
    /// The includes the page makes and what each resolves to; true by default.
    includes: Option<bool>,
    /// How many references of each kind reach the page; true by default.
    incoming: Option<bool>,
    /// The attributes declared for the page, with what is known of their values; true by
    /// default.
    attributes: Option<bool>,
    /// The page's text (the unsaved buffer if it has one); false by default.
    text: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct FindReferencesArgs {
    #[serde(flatten)]
    target: TargetArgs,
    /// An anchor of the page: the references to it rather than to the page.
    anchor: Option<String>,
    /// Only these kinds: `xref`, `include`, `nav`, `anchor`.
    kinds: Option<Vec<String>>,
    /// The `next_cursor` of the page before, to list the next; the first page without it.
    cursor: Option<String>,
    /// At most this many references in the page, up to 500 (the default).
    limit: Option<usize>,
}

fn range(range: SourceRange) -> Value {
    json!({ "start": range.start, "end": range.end })
}

/// A coordinate as a listing's resume key.
fn key(coordinate: &AntoraCoordinate) -> String {
    json!([
        coordinate.component,
        coordinate.version,
        coordinate.module,
        coordinate.family.to_string(),
        coordinate
            .relative_path
            .to_string_lossy()
            .replace('\\', "/"),
    ])
    .to_string()
}

fn coordinate_of(key: &str) -> Option<AntoraCoordinate> {
    let parts: (String, Option<String>, String, String, String) = serde_json::from_str(key).ok()?;
    Some(AntoraCoordinate {
        component: parts.0,
        version: parts.1,
        module: parts.2,
        family: parts.3.parse().ok()?,
        relative_path: PathBuf::from(parts.4),
    })
}

/// The Antora identity of the file at `path`, or `None` outside every component.
fn identity(state: &State, path: &Path) -> Value {
    let Some(resource) = state.engine.workspace().antora.resource_for_path(path) else {
        return Value::Null;
    };
    let coordinate = &resource.coordinate;
    json!({
        "component": coordinate.component,
        "version": coordinate.version,
        "module": coordinate.module,
        "family": coordinate.family.to_string(),
        "relative_path": coordinate.relative_path.to_string_lossy().replace('\\', "/"),
        "aliases": resource
            .aliases
            .iter()
            .map(adoc_engine::resource_id_string)
            .collect::<Vec<_>>(),
    })
}

/// An outline symbol as JSON, a section given the id references reach it by.
fn symbol(symbol: &DocumentSymbol, ids: &[(SourceRange, String)]) -> Value {
    let kind = match symbol.kind {
        DocumentSymbolKind::Document => "document",
        DocumentSymbolKind::Section => "section",
        DocumentSymbolKind::Page => "page",
        DocumentSymbolKind::Category => "category",
    };
    let id = ids
        .iter()
        .find(|(range, _)| *range == symbol.range)
        .map(|(_, id)| id.clone())
        .filter(|id| !id.is_empty());
    json!({
        "kind": kind,
        "title": symbol.name,
        "level": symbol.level,
        "id": id,
        "target": symbol.detail,
        "range": range(symbol.range),
        "title_range": range(symbol.selection_range),
        "children": symbol.children.iter().map(|child| self::symbol(child, ids)).collect::<Vec<_>>(),
    })
}

/// A declared attribute's name and whether the declaration unsets it (`:name!:`, `:!name:`).
fn attribute_name(name: &str) -> (&str, bool) {
    if let Some(name) = name.strip_suffix('!') {
        return (name, true);
    }
    if let Some(name) = name.strip_prefix('!') {
        return (name, true);
    }
    (name, false)
}

fn kind_named(name: &str) -> Result<ReferenceKind, Failure> {
    ReferenceKind::ALL
        .into_iter()
        .find(|kind| kind.name() == name)
        .ok_or_else(|| {
            Failure::invalid(format!(
                "unknown reference kind `{name}`; use `xref`, `include`, `nav` or `anchor`"
            ))
        })
}

#[tool_router(router = discovery_router, vis = "pub(crate)")]
impl Server {
    #[tool(
        description = "List the workspace's Antora resources (pages by default) a page at a time, in resource-id order, filtered by root, component, version (`~` for the versionless one), module and family: each one's resource id, path, title and whether its version's navigation lists it.",
        output_schema = schemas::list_pages(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_pages(&self, Parameters(args): Parameters<ListPagesArgs>) -> Answer {
        self.run(move |state| {
            state.indexed()?;
            if args.component.is_none() && (args.version.is_some() || args.module.is_some()) {
                return Err(Failure::invalid(
                    "a `version` or `module` needs its `component`, so no other is taken",
                ));
            }
            let family = match args.family.as_deref().unwrap_or("page") {
                "any" => None,
                name => Some(name.parse::<ResourceFamily>().map_err(|_| {
                    Failure::invalid(format!(
                        "unknown family `{name}`; use page, partial, example, image, attachment or any"
                    ))
                })?),
            };
            let version = args
                .version
                .as_deref()
                .map(|version| (version != "~").then(|| version.to_owned()));
            let catalog = &state.engine.workspace().antora;
            if let Some(component) = &args.component {
                let known = match &version {
                    Some(version) => catalog.component(component, version.as_deref()).is_some(),
                    None => !catalog.versions_of(component).is_empty(),
                };
                if !known {
                    return Err(Failure::not_found(format!(
                        "no component `{component}`{} in the workspace",
                        args.version
                            .as_deref()
                            .map(|version| format!(" at version `{version}`"))
                            .unwrap_or_default()
                    )));
                }
            }
            let query = ResourceQuery {
                root: args.root.as_deref().map(|root| state.confine(root)).transpose()?,
                component: args.component.clone(),
                version,
                module: args.module.clone(),
                family,
            };
            let query_key = format!(
                "list_pages\u{1f}{}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}\u{1f}{:?}",
                query.root.as_deref().map(|root| state.shown(root)).unwrap_or_default(),
                query.component,
                query.version,
                query.module,
                query.family,
            );
            let revision = state.engine.workspace_revision();
            let after = resume(args.cursor.as_deref(), &query_key, revision)?
                .map(|position| {
                    coordinate_of(&position)
                        .ok_or_else(|| Failure::invalid("the cursor is not one this server made"))
                })
                .transpose()?;
            let (entries, more) =
                state
                    .engine
                    .resources_after(&query, after.as_ref(), page_size(args.limit));
            let next_cursor = more
                .then(|| entries.last())
                .flatten()
                .map(|last| cursor(&query_key, revision, &key(&last.coordinate)));
            let items: Vec<Value> = entries
                .iter()
                .map(|entry| {
                    json!({
                        "resource_id": entry.resource_id,
                        "component": entry.coordinate.component,
                        "version": entry.coordinate.version,
                        "module": entry.coordinate.module,
                        "family": entry.coordinate.family.to_string(),
                        "relative_path": entry.coordinate.relative_path.to_string_lossy().replace('\\', "/"),
                        "path": state.shown(&entry.path),
                        "title": entry.title,
                        "in_navigation": entry.in_navigation,
                    })
                })
                .collect();
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "items": items,
                "next_cursor": next_cursor,
                "workspace_revision": revision,
            }))
        })
        .await
    }

    #[tool(
        description = "A page's outline: its title and sections nested by level (or a navigation file's entries), each with the id references reach it by and its byte range, and the anchors it declares.",
        output_schema = schemas::get_outline(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_outline(&self, Parameters(args): Parameters<OutlineArgs>) -> Answer {
        self.run(move |state| {
            let path = state.target(
                args.target.path.as_deref(),
                args.target.resource_id.as_deref(),
                args.target.from.as_deref(),
            )?;
            let engine = &state.engine;
            let outline = engine.outline_of(&path).ok_or_else(|| {
                Failure::not_found(format!("{} is not an indexed file", state.shown(&path)))
            })?;
            let document = &engine
                .workspace()
                .index
                .file(&path)
                .expect("an outline is of an indexed file")
                .document;
            let ids: Vec<(SourceRange, String)> = document
                .sections
                .iter()
                .map(|section| section.range)
                .zip(engine.section_ids(&path))
                .collect();
            let nav = engine.workspace().antora.is_nav_file(&path);
            let headings = args.headings.unwrap_or(true);
            let anchors: Vec<Value> = if args.anchors.unwrap_or(true) {
                document
                    .anchors
                    .iter()
                    .map(|anchor| {
                        json!({
                            "id": anchor.id,
                            "range": range(anchor.range),
                            "conditional": anchor.conditional,
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            };
            Ok(json!({
                "schema_version": SCHEMA_VERSION,
                "path": state.shown(&path),
                "resource_id": engine.resource_id(&path),
                "kind": if nav { "navigation" } else { "page" },
                "outline": if headings {
                    outline.iter().map(|entry| symbol(entry, &ids)).collect::<Vec<_>>()
                } else {
                    Vec::new()
                },
                "anchors": anchors,
                "workspace_revision": engine.workspace_revision(),
            }))
        })
        .await
    }

    #[tool(
        description = "A page's context without its text: its Antora identity, where its version's navigation lists it, the includes it makes and what they resolve to, how many references of each kind reach it, the attributes declared for it as written, and each attribute's effective value at the end of the page's header with where it comes from (playbook, component or page), following Antora's hard and soft sets and the order of declarations.",
        output_schema = schemas::get_page_context(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_page_context(&self, Parameters(args): Parameters<PageContextArgs>) -> Answer {
        self.run(move |state| {
            let path = state.target(
                args.target.path.as_deref(),
                args.target.resource_id.as_deref(),
                args.target.from.as_deref(),
            )?;
            let engine = &state.engine;
            let file = engine.workspace().index.file(&path).ok_or_else(|| {
                Failure::not_found(format!("{} is not an indexed page", state.shown(&path)))
            })?;
            let document = &file.document;
            let mut value = json!({
                "schema_version": SCHEMA_VERSION,
                "path": state.shown(&path),
                "resource_id": engine.resource_id(&path),
                "identity": identity(state, &path),
                "title": document.title.as_ref().map(|title| title.text.clone()),
                "workspace_revision": engine.workspace_revision(),
            });
            if args.navigation.unwrap_or(true) {
                let places: Vec<Value> = engine
                    .nav_places(&path)
                    .iter()
                    .map(|place| {
                        json!({
                            "nav_file": state.shown(&place.nav),
                            "title": place.page.title,
                            "ancestors": place.ancestors.iter().map(|crumb| crumb.title.clone()).collect::<Vec<_>>(),
                            "children": place.children.iter().map(|crumb| crumb.title.clone()).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                value["navigation"] = json!({
                    "in_navigation": engine.in_navigation(&path),
                    "places": places,
                });
            }
            if args.includes.unwrap_or(true) {
                let includes: Vec<Value> = document
                    .includes
                    .iter()
                    .map(|include| {
                        let resolved = match engine
                            .resolve_target(&path, &format!("include::{}[]", include.target))
                        {
                            Some(DefinitionTarget::Location(location)) => Some(location.path),
                            Some(DefinitionTarget::External(path)) => Some(path),
                            None => None,
                        };
                        json!({
                            "target": include.target,
                            "range": range(include.range),
                            "resolved": resolved.is_some(),
                            "resolves_to": resolved.as_deref().map(|path| state.shown(path)),
                            "resource_id": resolved.as_deref().and_then(|path| engine.resource_id(path)),
                        })
                    })
                    .collect();
                value["includes"] = json!(includes);
            }
            if args.incoming.unwrap_or(true) {
                let found = engine.incoming_references(&path, None);
                let count = |kind: ReferenceKind| found.iter().filter(|r| r.kind == kind).count();
                value["incoming"] = json!({
                    "xref": count(ReferenceKind::Xref),
                    "nav": count(ReferenceKind::Nav),
                    "include": count(ReferenceKind::Include),
                });
            }
            if args.attributes.unwrap_or(true) {
                let mut declared: Vec<Value> = Vec::new();
                if let Some(resource) = engine.workspace().antora.resource_for_path(&path) {
                    let coordinate = &resource.coordinate;
                    if let Some(descriptor) = engine
                        .workspace()
                        .antora
                        .component(&coordinate.component, coordinate.version.as_deref())
                    {
                        let descriptor_file = descriptor.root.join("antora.yml");
                        for (name, written) in &descriptor.asciidoc_attributes {
                            let (name, unset) = attribute_name(name);
                            declared.push(json!({
                                "name": name,
                                "value": (!unset).then(|| written.clone()),
                                "unset": unset,
                                "dynamic": written.contains('{'),
                                "source": "component",
                                "defined_in": state.shown(&descriptor_file),
                                "range": Value::Null,
                            }));
                        }
                    }
                }
                for attribute in &document.attributes {
                    let (name, unset) = attribute_name(&attribute.name);
                    let written = attribute.value.clone().unwrap_or_default();
                    declared.push(json!({
                        "name": name,
                        "value": (!unset).then_some(written.clone()),
                        "unset": unset,
                        "dynamic": written.contains('{'),
                        "source": "page",
                        "defined_in": state.shown(&path),
                        "range": range(attribute.range),
                    }));
                }
                let effective: Vec<Value> = engine
                    .effective_values(&path)
                    .into_iter()
                    .map(|found| {
                        json!({
                            "name": found.name,
                            "value": found.value,
                            "unset": found.value.is_none(),
                            "dynamic": found.value.as_deref().is_some_and(|value| value.contains('{')),
                            "source": match found.origin {
                                AttributeOrigin::BuiltIn => "built_in",
                                AttributeOrigin::Playbook => "playbook",
                                AttributeOrigin::Component => "component",
                                AttributeOrigin::Antora => "antora",
                                AttributeOrigin::Declaration => "page",
                            },
                            "defined_in": found.location.as_ref().map(|location| state.shown(&location.path)),
                            "range": found.location.as_ref().map_or(Value::Null, |location| range(location.range)),
                        })
                    })
                    .collect();
                value["attributes"] = json!({
                    "effective_values": "at_header_end",
                    "note": "`declared` lists each declaration as written. `effective` gives each \
                             attribute the playbook, the component or the page sets, with its value \
                             at the end of the page's header and where that value comes from, \
                             following Antora's hard and soft sets and the order of declarations. \
                             Not traced: the command line, conditionals around a declaration, \
                             include tag filters, and references inside a value, which is given \
                             as written.",
                    "declared": declared,
                    "effective": effective,
                });
            }
            if args.text.unwrap_or(false) {
                let text = state.text(&path)?;
                let (shown, total) = limited_text(&text);
                value["text"] = json!(shown);
                if let Some(total) = total {
                    value["truncated"] = json!({ "shown_bytes": MAX_TEXT, "total_bytes": total });
                }
            }
            Ok(value)
        })
        .await
    }

    #[tool(
        description = "The exact references that reach a page, or one of its anchors, a page at a time: xrefs, navigation entries and includes (or, with `anchor`, the references to that anchor), each with the file it is written in, its byte range, line and column, and its target as written. From the index's references, resolved one by one; a dynamic target that cannot be resolved is not listed.",
        output_schema = schemas::find_references(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn find_references(&self, Parameters(args): Parameters<FindReferencesArgs>) -> Answer {
        self.run(move |state| {
            state.indexed()?;
            let path = state.target(
                args.target.path.as_deref(),
                args.target.resource_id.as_deref(),
                args.target.from.as_deref(),
            )?;
            let engine = &state.engine;
            if engine.workspace().index.file(&path).is_none() {
                return Err(Failure::not_found(format!(
                    "{} is not an indexed file",
                    state.shown(&path)
                )));
            }
            let kinds = args
                .kinds
                .iter()
                .flatten()
                .map(|name| kind_named(name))
                .collect::<Result<Vec<_>, _>>()?;
            let revision = engine.workspace_revision();
            let mut kind_names: Vec<&str> = kinds.iter().map(|kind| kind.name()).collect();
            kind_names.sort_unstable();
            let query = format!(
                "find_references\u{1f}{}\u{1f}{}\u{1f}{}",
                state.shown(&path),
                args.anchor.as_deref().unwrap_or_default(),
                kind_names.join(","),
            );
            let found: Vec<_> = engine
                .incoming_references(&path, args.anchor.as_deref())
                .into_iter()
                .filter(|reference| kinds.is_empty() || kinds.contains(&reference.kind))
                .collect();
            let page = page(found, &query, revision, args.cursor.as_deref(), args.limit)?;
            let mut lines: Option<(PathBuf, String, LineIndex)> = None;
            let items: Vec<Value> = page
                .items
                .iter()
                .map(|reference| {
                    if lines
                        .as_ref()
                        .is_none_or(|(file, _, _)| *file != reference.path)
                    {
                        let text = state.text(&reference.path).unwrap_or_default();
                        let index = LineIndex::new(&text);
                        lines = Some((reference.path.clone(), text, index));
                    }
                    let (_, text, index) = lines.as_ref().expect("set above");
                    let (line, column) = index.line_character(text, reference.range.start);
                    json!({
                        "kind": reference.kind.name(),
                        "path": state.shown(&reference.path),
                        "resource_id": engine.resource_id(&reference.path),
                        "range": range(reference.range),
                        "line": line + 1,
                        "column": column + 1,
                        "target": reference.target,
                        "confidence": "resolved",
                    })
                })
                .collect();
            let page = Page {
                items,
                next_cursor: page.next_cursor,
                total: page.total,
            };
            let mut value = paged("items", page, revision);
            value["schema_version"] = json!(SCHEMA_VERSION);
            value["target"] = json!({
                "path": state.shown(&path),
                "resource_id": engine.resource_id(&path),
                "anchor": args.anchor,
            });
            Ok(value)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::responses::ErrorCode;

    #[test]
    fn resume_keys_read_back_as_their_coordinates() {
        for version in [None, Some("3.0".to_owned())] {
            let coordinate = AntoraCoordinate {
                component: "product".into(),
                version,
                module: "ROOT".into(),
                family: ResourceFamily::Partial,
                relative_path: PathBuf::from("a/b:c.adoc"),
            };
            assert_eq!(coordinate_of(&key(&coordinate)), Some(coordinate));
        }
        assert_eq!(coordinate_of("[1,2]"), None);
    }

    #[test]
    fn unset_attributes_are_told_apart() {
        assert_eq!(attribute_name("name!"), ("name", true));
        assert_eq!(attribute_name("!name"), ("name", true));
        assert_eq!(attribute_name("name"), ("name", false));
        assert!(kind_named("xref").is_ok());
        assert_eq!(
            kind_named("link").unwrap_err().code,
            ErrorCode::InvalidArgument
        );
    }
}
