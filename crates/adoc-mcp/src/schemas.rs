//! Each tool's output schema: the top level of the JSON it answers with, every field it may
//! give and the ones it always gives. Nested values the engine reports (diagnostics, graph
//! nodes, Git status) are described as objects, not field by field, so the engine can add to
//! them without breaking a client that validates.

use std::sync::Arc;

use rmcp::model::JsonObject;
use serde_json::{json, Value};

fn object(properties: Value, required: &[&str]) -> Arc<JsonObject> {
    let Value::Object(schema) = json!({
        "type": "object",
        "properties": properties,
        "required": required,
    }) else {
        unreachable!("a JSON object literal")
    };
    Arc::new(schema)
}

fn list(items: Value) -> Value {
    json!({ "type": "array", "items": items })
}

fn any_object() -> Value {
    json!({ "type": "object" })
}

fn optional_string() -> Value {
    json!({ "type": ["string", "null"] })
}

/// A list cut short, as the tools report it: how many are shown of how many.
fn truncated() -> Value {
    json!({
        "type": "object",
        "description": "Present when the list was cut: how many items are shown of how many.",
        "properties": { "shown": { "type": "integer" }, "total": { "type": "integer" } },
    })
}

/// Text cut short: how many bytes are shown of how many.
fn truncated_text() -> Value {
    json!({
        "type": "object",
        "description": "Present when the text was cut: how many bytes are shown of how many.",
        "properties": {
            "shown_bytes": { "type": "integer" },
            "total_bytes": { "type": "integer" },
        },
    })
}

fn incomplete() -> Value {
    json!({
        "type": "boolean",
        "description": "True when the list is cut and the rest cannot be paged to.",
    })
}

/// The fields of a paged list besides its items.
fn paging(mut properties: Value) -> Value {
    properties["total"] = json!({ "type": "integer", "description": "Items in the whole list." });
    properties["next_cursor"] = json!({
        "type": ["string", "null"],
        "description": "Pass as `cursor` for the next page; null on the last.",
    });
    properties["workspace_revision"] = json!({
        "type": "integer",
        "description": "The workspace revision the list was made at; a cursor holds to it.",
    });
    properties["truncated"] = truncated();
    properties
}

pub fn list_components() -> Arc<JsonObject> {
    object(
        json!({
            "components": list(json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "version": optional_string(),
                    "title": optional_string(),
                    "display_version": optional_string(),
                    "start_page": optional_string(),
                    "nav": list(json!({ "type": "string" })),
                },
                "required": ["name", "nav"],
            })),
            "truncated": truncated(),
            "incomplete": incomplete(),
        }),
        &["components"],
    )
}

pub fn get_nav() -> Arc<JsonObject> {
    object(
        json!({
            "trees": list(json!({
                "type": "object",
                "properties": {
                    "nav_file": { "type": "string" },
                    "items": list(any_object()),
                },
                "required": ["nav_file", "items"],
            })),
        }),
        &["trees"],
    )
}

pub fn read_page() -> Arc<JsonObject> {
    object(
        json!({
            "path": { "type": "string" },
            "resource_id": optional_string(),
            "title": optional_string(),
            "attributes": list(any_object()),
            "includes": list(any_object()),
            "text": { "type": "string" },
            "truncated": truncated_text(),
        }),
        &["path", "attributes", "includes", "text"],
    )
}

pub fn resolve() -> Arc<JsonObject> {
    object(
        json!({
            "resolves": { "type": "boolean" },
            "path": { "type": "string" },
            "line": { "type": "integer" },
            "resource_id": optional_string(),
            "external": { "type": "boolean" },
            "reason": { "type": "string" },
        }),
        &["resolves"],
    )
}

pub fn check() -> Arc<JsonObject> {
    object(
        paging(json!({
            "diagnostics": list(any_object()),
            "failed": {
                "type": "boolean",
                "description": "Whether any diagnostic, on any page, meets `fail_on`.",
            },
        })),
        &[
            "diagnostics",
            "failed",
            "total",
            "next_cursor",
            "workspace_revision",
        ],
    )
}

pub fn symbols() -> Arc<JsonObject> {
    object(
        paging(json!({ "symbols": list(any_object()) })),
        &["symbols", "total", "next_cursor", "workspace_revision"],
    )
}

pub fn search() -> Arc<JsonObject> {
    object(
        json!({
            "matches": list(json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "line": { "type": "integer" },
                    "text": { "type": "string" },
                },
                "required": ["path", "line", "text"],
            })),
            "truncated": truncated(),
            "incomplete": incomplete(),
        }),
        &["matches"],
    )
}

pub fn render_page() -> Arc<JsonObject> {
    object(
        json!({
            "path": { "type": "string" },
            "content": { "type": "string" },
            "truncated": truncated_text(),
        }),
        &["path", "content"],
    )
}

pub fn link_graph() -> Arc<JsonObject> {
    // The workspace or a page's neighbourhood, a shortest path (null when there is none), or
    // either drawn as SVG.
    object(
        json!({
            "revision": { "type": "integer" },
            "nodes": list(any_object()),
            "edges": list(any_object()),
            "path": { "type": ["array", "object", "null"] },
            "svg": { "type": "string" },
            "truncated": any_object(),
            "incomplete": incomplete(),
        }),
        &[],
    )
}

pub fn git_status() -> Arc<JsonObject> {
    object(json!({ "roots": list(any_object()) }), &["roots"])
}

pub fn edit_page() -> Arc<JsonObject> {
    object(
        json!({
            "path": { "type": "string" },
            "edits": { "type": "integer" },
            "saved": { "type": "boolean" },
            "length": { "type": "integer" },
        }),
        &["path", "edits", "saved", "length"],
    )
}

pub fn rename_page() -> Arc<JsonObject> {
    object(
        json!({
            "from": { "type": "string" },
            "to": { "type": "string" },
            "changed": list(json!({ "type": "string" })),
        }),
        &["from", "to", "changed"],
    )
}

pub fn export_pdf() -> Arc<JsonObject> {
    object(
        json!({
            "output": { "type": "string" },
            "pages": { "type": "integer" },
            "warnings": list(json!({ "type": "string" })),
        }),
        &["output", "pages", "warnings"],
    )
}

fn byte_range() -> Value {
    json!({
        "type": "object",
        "description": "Byte offsets into the file: the first byte and just past the last.",
        "properties": { "start": { "type": "integer" }, "end": { "type": "integer" } },
        "required": ["start", "end"],
    })
}

pub fn list_pages() -> Arc<JsonObject> {
    object(
        json!({
            "schema_version": { "type": "integer" },
            "items": list(json!({
                "type": "object",
                "properties": {
                    "resource_id": { "type": "string" },
                    "component": { "type": "string" },
                    "version": {
                        "type": ["string", "null"],
                        "description": "Null for the versionless (`~`) version.",
                    },
                    "module": { "type": "string" },
                    "family": { "type": "string" },
                    "relative_path": { "type": "string" },
                    "path": { "type": "string" },
                    "title": optional_string(),
                    "in_navigation": { "type": ["boolean", "null"] },
                },
                "required": ["resource_id", "component", "version", "module", "family", "path"],
            })),
            "next_cursor": optional_string(),
            "workspace_revision": { "type": "integer" },
        }),
        &[
            "schema_version",
            "items",
            "next_cursor",
            "workspace_revision",
        ],
    )
}

pub fn get_outline() -> Arc<JsonObject> {
    object(
        json!({
            "schema_version": { "type": "integer" },
            "path": { "type": "string" },
            "resource_id": optional_string(),
            "kind": { "type": "string", "description": "`page` or `navigation`." },
            "outline": list(json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string" },
                    "title": { "type": "string" },
                    "level": { "type": "integer" },
                    "id": optional_string(),
                    "target": optional_string(),
                    "range": byte_range(),
                    "title_range": byte_range(),
                    "children": list(any_object()),
                },
                "required": ["kind", "title", "level", "range", "children"],
            })),
            "anchors": list(json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "range": byte_range(),
                    "conditional": { "type": "boolean" },
                },
                "required": ["id", "range", "conditional"],
            })),
            "workspace_revision": { "type": "integer" },
        }),
        &[
            "schema_version",
            "path",
            "kind",
            "outline",
            "anchors",
            "workspace_revision",
        ],
    )
}

pub fn get_page_context() -> Arc<JsonObject> {
    object(
        json!({
            "schema_version": { "type": "integer" },
            "path": { "type": "string" },
            "resource_id": optional_string(),
            "identity": { "type": ["object", "null"] },
            "title": optional_string(),
            "workspace_revision": { "type": "integer" },
            "navigation": {
                "type": "object",
                "properties": {
                    "in_navigation": { "type": ["boolean", "null"] },
                    "places": list(any_object()),
                },
            },
            "includes": list(any_object()),
            "incoming": {
                "type": "object",
                "properties": {
                    "xref": { "type": "integer" },
                    "nav": { "type": "integer" },
                    "include": { "type": "integer" },
                },
            },
            "attributes": {
                "type": "object",
                "properties": {
                    "effective_values": { "type": "string" },
                    "note": { "type": "string" },
                    "declared": list(any_object()),
                },
                "required": ["effective_values", "declared"],
            },
            "text": { "type": "string" },
            "truncated": truncated_text(),
        }),
        &["schema_version", "path", "identity", "workspace_revision"],
    )
}

pub fn find_references() -> Arc<JsonObject> {
    object(
        paging(json!({
            "schema_version": { "type": "integer" },
            "target": any_object(),
            "items": list(json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string" },
                    "path": { "type": "string" },
                    "resource_id": optional_string(),
                    "range": byte_range(),
                    "line": { "type": "integer" },
                    "column": { "type": "integer", "description": "One-based, in bytes." },
                    "target": { "type": "string" },
                    "confidence": { "type": "string" },
                },
                "required": ["kind", "path", "range", "line", "column", "target"],
            })),
        })),
        &[
            "schema_version",
            "target",
            "items",
            "total",
            "next_cursor",
            "workspace_revision",
        ],
    )
}

pub fn quality_audit() -> Arc<JsonObject> {
    // The report of `adoc-audit` (its own `schema_version`), with the findings paged and the
    // fields below added; nested parts are the report's.
    object(
        json!({
            "schema_version": { "type": "integer", "description": "The audit report's schema." },
            "scope": any_object(),
            "revision": any_object(),
            "roots": list(json!({ "type": "string" })),
            "fail_on": { "type": "string" },
            "require_complete": { "type": "boolean" },
            "rules": list(any_object()),
            "providers": list(any_object()),
            "completeness": {
                "type": "object",
                "properties": {
                    "status": { "type": "string", "description": "`complete`, `partial` or `unavailable`." },
                    "gaps": list(any_object()),
                },
                "required": ["status"],
            },
            "counts": any_object(),
            "coverage": list(any_object()),
            "findings": list(any_object()),
            "skips": list(any_object()),
            "excluded": list(any_object()),
            "findings_total": { "type": "integer", "description": "Findings the filters take, on every page." },
            "next_cursor": optional_string(),
            "truncated": truncated(),
            "verdict": { "type": "string", "description": "`pass`, `fail` or `untrustworthy`, as the policy judges the report." },
            "exit_code": { "type": "integer", "description": "What `adocfoundry-cli audit` would exit with." },
            "outcome": {
                "type": "string",
                "description": "`pass` only for a complete audit; `pass_partial` when nothing failed in what was audited but not everything was; `fail`; `untrustworthy`.",
            },
            "policy_problems": list(json!({ "type": "string" })),
            "workspace_revision": { "type": "integer" },
        }),
        &[
            "schema_version",
            "completeness",
            "counts",
            "findings",
            "findings_total",
            "next_cursor",
            "verdict",
            "exit_code",
            "outcome",
            "workspace_revision",
        ],
    )
}

pub fn preview_changes() -> Arc<JsonObject> {
    object(
        json!({
            "schema_version": { "type": "integer" },
            "preview_id": { "type": "string" },
            "expires_at": { "type": "integer", "description": "Seconds since 1970, UTC." },
            "expires_in_seconds": { "type": "integer" },
            "workspace_revision": { "type": "integer" },
            "files": list(json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "description": "`create`, `edit` or `move`." },
                    "path": { "type": "string" },
                    "to": optional_string(),
                    "resource_id": optional_string(),
                    "before_hash": optional_string(),
                    "after_hash": { "type": "string" },
                    "diff": { "type": "string" },
                    "diagnostics": list(any_object()),
                },
                "required": ["kind", "path", "after_hash", "diff", "diagnostics"],
            })),
            "can_apply": { "type": "boolean" },
            "reason": optional_string(),
        }),
        &[
            "schema_version",
            "preview_id",
            "expires_at",
            "files",
            "can_apply",
            "workspace_revision",
        ],
    )
}

pub fn apply_changes() -> Arc<JsonObject> {
    object(
        json!({
            "schema_version": { "type": "integer" },
            "applied": { "type": "boolean" },
            "saved": { "type": "boolean" },
            "changed": list(json!({ "type": "string" })),
            "resource_ids": list(optional_string()),
            "workspace_revision": { "type": "integer" },
        }),
        &[
            "schema_version",
            "applied",
            "saved",
            "changed",
            "workspace_revision",
        ],
    )
}

pub fn create_page() -> Arc<JsonObject> {
    object(
        json!({
            "schema_version": { "type": "integer" },
            "path": { "type": "string" },
            "resource_id": optional_string(),
            "navigation_file": optional_string(),
            "changed": list(json!({ "type": "string" })),
            "saved": { "type": "boolean" },
            "workspace_revision": { "type": "integer" },
        }),
        &[
            "schema_version",
            "path",
            "resource_id",
            "changed",
            "saved",
            "workspace_revision",
        ],
    )
}

pub fn create_partial() -> Arc<JsonObject> {
    object(
        json!({
            "schema_version": { "type": "integer" },
            "path": { "type": "string" },
            "resource_id": optional_string(),
            "include": {
                "type": "object",
                "properties": {
                    "from_its_module": { "type": "string" },
                    "from_anywhere": optional_string(),
                },
                "required": ["from_its_module"],
            },
            "workspace_revision": { "type": "integer" },
        }),
        &[
            "schema_version",
            "path",
            "resource_id",
            "include",
            "workspace_revision",
        ],
    )
}
