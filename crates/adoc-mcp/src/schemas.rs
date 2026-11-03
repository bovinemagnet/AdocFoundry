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
