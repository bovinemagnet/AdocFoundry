//! The impact tool (MCP PRD §7.4): `analyse_impact`, the engine's documentation impact
//! analysis as `adocfoundry-cli impact` makes it, with its impacts filtered and paged. The rules
//! stay in `adoc-impact` and the snapshots in the engine; this adapter only reads the source
//! selector, filters by scope and pages, keeping the whole report's completeness and counts on
//! every page.

use adoc_engine::{impact::Limits, ChangeSource, ImpactError};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    responses::{page, ErrorCode, Failure},
    schemas,
    server::{Answer, Server},
};

/// What to compare: exactly one kind.
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Source {
    /// `HEAD` against the working tree, the server's unsaved buffers included.
    WorkingTree,
    /// Two local revisions, by any name Git reads (`main`, `HEAD~1`, a commit id); nothing is
    /// fetched, checked out, or read from the working tree.
    GitRefs { base: String, head: String },
    /// Files named, against the working tree, with no base revision.
    Paths { paths: Vec<String> },
}

#[derive(Deserialize, JsonSchema)]
struct ImpactArgs {
    source: Source,
    /// Only impacts in this component.
    component: Option<String>,
    /// Only impacts in this version of `component` (`~` for the versionless one).
    version: Option<String>,
    /// Give each impact's dependency path (`via`) and source span; true by default.
    include_provenance: Option<bool>,
    /// How many includes deep the analysis follows from a changed file; 32 by default.
    max_depth: Option<usize>,
    /// The `next_cursor` of the page before; the first page without it.
    cursor: Option<String>,
    /// At most this many impacts in the page, up to 500 (the default).
    limit: Option<usize>,
}

fn failure(error: ImpactError) -> Failure {
    match error {
        ImpactError::NoRepository(message) => Failure::new(ErrorCode::Unavailable, message),
        ImpactError::UnknownRevision(message) => Failure::not_found(message),
        ImpactError::Git(message) => Failure::internal(message),
        ImpactError::OutsideRoots(path) => Failure::new(
            ErrorCode::OutsideWorkspace,
            format!("{} is outside the workspace roots", path.display()),
        ),
    }
}

#[tool_router(router = impact_router, vis = "pub(crate)")]
impl Server {
    #[tool(
        description = "What a change to the documentation may affect, as `adocfoundry-cli impact` reports it: for two local Git revisions (from both revisions' own files, never the working tree), `HEAD` against the working tree, or paths named. Impacts are rendered content (through includes), references to review (moved or removed pages, removed anchors), navigation and publications, each with its evidence side, dependency path and source span. A page's prose change never reaches the pages that merely link to it. Reads Git objects only: never checks out, fetches or changes the repository.",
        output_schema = schemas::analyse_impact(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn analyse_impact(&self, Parameters(args): Parameters<ImpactArgs>) -> Answer {
        self.run(move |state| {
            state.indexed()?;
            if args.version.is_some() && args.component.is_none() {
                return Err(Failure::invalid(
                    "a `version` needs its `component`, so no other is taken",
                ));
            }
            let (source, query_source) = match &args.source {
                Source::WorkingTree => (ChangeSource::WorkingTree, "working_tree".to_owned()),
                Source::GitRefs { base, head } => (
                    ChangeSource::GitRefs {
                        base: base.clone(),
                        head: head.clone(),
                    },
                    format!("git_refs\u{1e}{base}\u{1e}{head}"),
                ),
                Source::Paths { paths } => {
                    if paths.is_empty() {
                        return Err(Failure::invalid("give at least one path"));
                    }
                    let mut confined = Vec::new();
                    for path in paths {
                        confined.push(state.confine(path)?);
                    }
                    let key = paths.join("\u{1e}");
                    (ChangeSource::Paths(confined), format!("paths\u{1e}{key}"))
                }
            };
            let limits = Limits {
                depth: args.max_depth.unwrap_or(Limits::default().depth).clamp(1, 256),
                impacts: usize::MAX,
            };
            let revision = state.engine.workspace_revision();
            let report = state
                .engine
                .analyse_impact(&source, limits)
                .map_err(failure)?;
            let version = args
                .version
                .as_deref()
                .map(|version| (version != "~").then(|| version.to_owned()));
            let provenance = args.include_provenance.unwrap_or(true);
            let mut value =
                serde_json::to_value(&report).map_err(|error| Failure::internal(error.to_string()))?;
            let impacts: Vec<Value> = report
                .impacts
                .iter()
                .zip(value["impacts"].as_array().cloned().unwrap_or_default())
                .filter(|(impact, _)| {
                    args.component
                        .as_ref()
                        .is_none_or(|component| impact.component.as_ref() == Some(component))
                        && version
                            .as_ref()
                            .is_none_or(|version| impact.version == *version)
                })
                .map(|(_, mut json)| {
                    if !provenance {
                        if let Some(object) = json.as_object_mut() {
                            object.remove("via");
                            object.remove("source");
                        }
                    }
                    json
                })
                .collect();
            let query = format!(
                "analyse_impact\u{1f}{query_source}\u{1f}{:?}\u{1f}{version:?}\u{1f}{provenance}\u{1f}{}",
                args.component, limits.depth
            );
            let page = page(impacts, &query, revision, args.cursor.as_deref(), args.limit)?;
            value["impacts"] = json!(page.items);
            value["impacts_total"] = json!(page.total);
            value["next_cursor"] = json!(page.next_cursor);
            if page.next_cursor.is_some() {
                value["truncated"] = json!({ "shown": page.items.len(), "total": page.total });
            }
            value["workspace_revision"] = json!(revision);
            Ok(value)
        })
        .await
    }
}
