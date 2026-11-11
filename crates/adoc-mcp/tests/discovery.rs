//! The discovery tools (MCP PRD §§7.2 and 11.1, tests 5–7), driven through the binary over
//! stdio on a copy of `tests/fixtures/discovery`: a versionless and a numbered version of
//! `product`, and `guide`, which links to it.

mod common;

use common::{scratch, Client};
use serde_json::{json, Value};

fn ids(answer: &Value) -> Vec<String> {
    answer["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["resource_id"].as_str().unwrap().to_owned())
        .collect()
}

fn code(client: &mut Client, tool: &str, arguments: Value) -> String {
    let result = client.call_result(tool, arguments);
    assert_eq!(result["isError"], true, "{result}");
    result["_meta"]["error"]["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn mcp_list_pages_honours_component_version_and_family() {
    let (_directory, root) = scratch("discovery");
    let mut client = Client::start(&root, false);

    let every = client.call("list_pages", json!({})).unwrap();
    assert_eq!(every["schema_version"], 1);
    assert_eq!(
        ids(&every),
        [
            "1.0@guide:ROOT:page$index.adoc",
            "product:ROOT:page$draft.adoc",
            "product:ROOT:page$index.adoc",
            "product:ROOT:page$install.adoc",
            "3.0@product:ROOT:page$index.adoc",
            "3.0@product:ROOT:page$install.adoc",
            "3.0@product:admin:page$users.adoc",
        ]
    );
    assert!(every["next_cursor"].is_null());
    let install = &every["items"][3];
    assert_eq!(
        install["path"],
        "product/main/modules/ROOT/pages/install.adoc"
    );
    assert_eq!(install["title"], "Installing");
    assert_eq!(install["version"], Value::Null);
    assert_eq!(install["in_navigation"], true);
    assert_eq!(
        every["items"][5]["in_navigation"], false,
        "3.0's navigation lists no install page, whatever the versionless one lists"
    );

    // The versionless version and a numbered one are never conflated.
    let versionless = client
        .call(
            "list_pages",
            json!({ "component": "product", "version": "~" }),
        )
        .unwrap();
    assert_eq!(
        ids(&versionless),
        [
            "product:ROOT:page$draft.adoc",
            "product:ROOT:page$index.adoc",
            "product:ROOT:page$install.adoc",
        ]
    );
    let numbered = client
        .call(
            "list_pages",
            json!({ "component": "product", "version": "3.0" }),
        )
        .unwrap();
    assert!(ids(&numbered)
        .iter()
        .all(|id| id.starts_with("3.0@product:")));
    assert_eq!(ids(&numbered).len(), 3);
    let admin = client
        .call(
            "list_pages",
            json!({ "component": "product", "version": "3.0", "module": "admin" }),
        )
        .unwrap();
    assert_eq!(ids(&admin), ["3.0@product:admin:page$users.adoc"]);
    let partials = client
        .call("list_pages", json!({ "family": "partial" }))
        .unwrap();
    assert_eq!(ids(&partials), ["product:ROOT:partial$note.adoc"]);
    assert_eq!(partials["items"][0]["in_navigation"], Value::Null);
    let rooted = client
        .call("list_pages", json!({ "root": "guide" }))
        .unwrap();
    assert_eq!(ids(&rooted), ["1.0@guide:ROOT:page$index.adoc"]);

    // A page at a time, resumed where the last stopped, each page once.
    let mut paged = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut arguments = json!({ "limit": 2 });
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }
        let answer = client.call("list_pages", arguments).unwrap();
        assert!(answer["items"].as_array().unwrap().len() <= 2);
        paged.extend(ids(&answer));
        match answer["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    assert_eq!(paged, ids(&every));

    // Ambiguous or unknown selections are refused, never guessed.
    let first = client.call("list_pages", json!({ "limit": 1 })).unwrap();
    let cursor = first["next_cursor"].as_str().unwrap();
    for (arguments, expected) in [
        (json!({ "version": "3.0" }), "invalid_argument"),
        (json!({ "module": "ROOT" }), "invalid_argument"),
        (json!({ "family": "chapter" }), "invalid_argument"),
        (json!({ "component": "nope" }), "not_found"),
        (
            json!({ "component": "product", "version": "9.9" }),
            "not_found",
        ),
        (json!({ "root": "../elsewhere" }), "outside_workspace"),
        (
            json!({ "component": "guide", "cursor": cursor }),
            "invalid_argument",
        ),
    ] {
        assert_eq!(
            code(&mut client, "list_pages", arguments.clone()),
            expected,
            "{arguments}"
        );
    }
    client.stop();
}

/// The text a reference's range covers, read from the copy on disk.
fn spanned(root: &std::path::Path, item: &Value) -> String {
    let text = std::fs::read_to_string(root.join(item["path"].as_str().unwrap())).unwrap();
    let (start, end) = (
        item["range"]["start"].as_u64().unwrap() as usize,
        item["range"]["end"].as_u64().unwrap() as usize,
    );
    text[start..end].to_owned()
}

#[test]
fn mcp_find_references_returns_exact_spans() {
    let (_directory, root) = scratch("discovery");
    let mut client = Client::start(&root, false);

    let to_page = client
        .call(
            "find_references",
            json!({ "resource_id": "product:ROOT:page$install.adoc" }),
        )
        .unwrap();
    assert_eq!(to_page["total"], 3);
    assert_eq!(
        to_page["target"]["path"],
        "product/main/modules/ROOT/pages/install.adoc"
    );
    let described: Vec<(String, String, String, u64, u64)> = to_page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["kind"].as_str().unwrap().to_owned(),
                item["path"].as_str().unwrap().to_owned(),
                spanned(&root, item),
                item["line"].as_u64().unwrap(),
                item["column"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        described,
        [
            (
                "xref".to_owned(),
                "guide/modules/ROOT/pages/index.adoc".to_owned(),
                "xref:product::install.adoc#prerequisites[the product prerequisites]".to_owned(),
                3,
                6
            ),
            (
                "nav".to_owned(),
                "product/main/modules/ROOT/nav.adoc".to_owned(),
                "xref:install.adoc[Installing]".to_owned(),
                2,
                3
            ),
            (
                "xref".to_owned(),
                "product/main/modules/ROOT/pages/index.adoc".to_owned(),
                "xref:install.adoc#prerequisites[the prerequisites]".to_owned(),
                5,
                14
            ),
        ]
    );
    assert_eq!(to_page["items"][1]["target"], "install.adoc");
    assert_eq!(
        to_page["items"][0]["resource_id"],
        "1.0@guide:ROOT:page$index.adoc"
    );

    let to_anchor = client
        .call(
            "find_references",
            json!({
                "path": "product/main/modules/ROOT/pages/install.adoc",
                "anchor": "prerequisites",
            }),
        )
        .unwrap();
    let spans: Vec<String> = to_anchor["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            assert_eq!(item["kind"], "anchor");
            spanned(&root, item)
        })
        .collect();
    assert_eq!(
        spans,
        [
            "xref:product::install.adoc#prerequisites[the product prerequisites]",
            "xref:install.adoc#prerequisites[the prerequisites]",
            "<<prerequisites>>",
        ]
    );

    let nav_only = client
        .call(
            "find_references",
            json!({ "resource_id": "product:ROOT:page$install.adoc", "kinds": ["nav"] }),
        )
        .unwrap();
    assert_eq!(nav_only["total"], 1);

    let includes = client
        .call(
            "find_references",
            json!({ "resource_id": "product:ROOT:partial$note.adoc", "limit": 2 }),
        )
        .unwrap();
    assert_eq!(includes["total"], 3);
    assert_eq!(includes["items"].as_array().unwrap().len(), 2);
    let rest = client
        .call(
            "find_references",
            json!({
                "resource_id": "product:ROOT:partial$note.adoc",
                "limit": 2,
                "cursor": includes["next_cursor"],
            }),
        )
        .unwrap();
    assert_eq!(rest["items"].as_array().unwrap().len(), 1);
    for item in includes["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(rest["items"].as_array().unwrap())
    {
        assert_eq!(item["kind"], "include");
        assert!(spanned(&root, item).starts_with("include::"));
    }

    // Nothing reaches the numbered version's install page.
    let none = client
        .call(
            "find_references",
            json!({ "resource_id": "3.0@product:ROOT:page$install.adoc" }),
        )
        .unwrap();
    assert_eq!(none["total"], 0);
    assert_eq!(
        code(
            &mut client,
            "find_references",
            json!({ "path": "product/main/modules/ROOT/pages/install.adoc", "resource_id": "product:ROOT:page$install.adoc" })
        ),
        "invalid_argument",
        "two targets are ambiguous"
    );
    assert_eq!(
        code(
            &mut client,
            "find_references",
            json!({ "resource_id": "product:ROOT:page$install.adoc", "kinds": ["link"] })
        ),
        "invalid_argument"
    );
    client.stop();
}

fn declared<'a>(context: &'a Value, name: &str) -> &'a Value {
    context["attributes"]["declared"]
        .as_array()
        .unwrap()
        .iter()
        .find(|attribute| attribute["name"] == name)
        .unwrap_or_else(|| panic!("no attribute {name}: {context}"))
}

#[test]
fn mcp_page_context_distinguishes_known_and_unknown_attributes() {
    let (_directory, root) = scratch("discovery");
    let mut client = Client::start(&root, false);
    let context = client
        .call(
            "get_page_context",
            json!({ "resource_id": "product:ROOT:page$index.adoc" }),
        )
        .unwrap();
    assert_eq!(context["schema_version"], 1);
    assert_eq!(
        context["path"],
        "product/main/modules/ROOT/pages/index.adoc"
    );
    assert_eq!(context["identity"]["component"], "product");
    assert_eq!(context["identity"]["version"], Value::Null);
    assert_eq!(context["identity"]["module"], "ROOT");
    assert_eq!(context["identity"]["family"], "page");
    assert_eq!(context["title"], "Product");
    assert!(context.get("text").is_none(), "no text unless asked");

    // What is declared is given as written; what a build sees at the end of the header is
    // traced separately.
    assert_eq!(context["attributes"]["effective_values"], "at_header_end");
    let effective = |name: &str| -> Value {
        context["attributes"]["effective"]
            .as_array()
            .unwrap()
            .iter()
            .find(|attribute| attribute["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no effective {name}: {context}"))
    };
    assert_eq!(effective("product-name")["value"], "Widget");
    assert_eq!(effective("product-name")["source"], "page");
    assert_eq!(
        effective("product-name")["defined_in"],
        "product/main/modules/ROOT/pages/index.adoc"
    );
    assert_eq!(effective("company")["value"], "Acme");
    assert_eq!(effective("company")["source"], "component");
    assert_eq!(
        effective("company")["defined_in"],
        "product/main/antora.yml"
    );
    assert!(effective("company")["range"]["start"].as_u64().is_some());
    assert_eq!(effective("edition")["value"], "{company} Cloud");
    assert_eq!(effective("edition")["dynamic"], true);
    let product = declared(&context, "product-name");
    assert_eq!(product["value"], "Widget");
    assert_eq!(product["source"], "page");
    assert_eq!(product["dynamic"], false);
    let full = declared(&context, "full-name");
    assert_eq!(full["value"], "{product-name} Suite");
    assert_eq!(full["dynamic"], true);
    let company = declared(&context, "company");
    assert_eq!(company["source"], "component");
    assert_eq!(company["defined_in"], "product/main/antora.yml");
    assert_eq!(declared(&context, "edition")["dynamic"], true);
    for attribute in context["attributes"]["declared"].as_array().unwrap() {
        assert!(
            attribute.get("effective").is_none() && attribute.get("effective_value").is_none(),
            "no effective value is invented: {attribute}"
        );
    }

    assert_eq!(context["navigation"]["in_navigation"], true);
    assert_eq!(
        context["navigation"]["places"][0]["nav_file"],
        "product/main/modules/ROOT/nav.adoc"
    );
    let include = &context["includes"][0];
    assert_eq!(include["target"], "partial$note.adoc");
    assert_eq!(include["resolved"], true);
    assert_eq!(
        include["resolves_to"],
        "product/main/modules/ROOT/partials/note.adoc"
    );
    assert_eq!(include["resource_id"], "product:ROOT:partial$note.adoc");
    // guide's xref:product::index.adoc[] and the navigation entry; 3.0's admin page names its
    // own version's index.
    assert_eq!(
        context["incoming"],
        json!({ "xref": 1, "nav": 1, "include": 0 })
    );

    let with_text = client
        .call(
            "get_page_context",
            json!({
                "path": "product/main/modules/ROOT/pages/index.adoc",
                "text": true,
                "attributes": false,
                "navigation": false,
            }),
        )
        .unwrap();
    assert!(with_text["text"].as_str().unwrap().starts_with("= Product"));
    assert!(with_text.get("attributes").is_none());
    assert!(with_text.get("navigation").is_none());
    client.stop();
}

#[test]
fn get_outline_gives_sections_with_ids_and_ranges() {
    let (_directory, root) = scratch("discovery");
    let mut client = Client::start(&root, false);
    let outline = client
        .call(
            "get_outline",
            json!({ "resource_id": "product:ROOT:page$index.adoc" }),
        )
        .unwrap();
    assert_eq!(outline["kind"], "page");
    let title = &outline["outline"][0];
    assert_eq!(title["kind"], "document");
    assert_eq!(title["title"], "Product");
    let sections: Vec<(&str, &str)> = title["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|section| {
            (
                section["title"].as_str().unwrap(),
                section["id"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(sections, [("Overview", "overview"), ("Details", "details")]);
    let text =
        std::fs::read_to_string(root.join("product/main/modules/ROOT/pages/index.adoc")).unwrap();
    let heading = &title["children"][0]["title_range"];
    let (start, end) = (
        heading["start"].as_u64().unwrap() as usize,
        heading["end"].as_u64().unwrap() as usize,
    );
    assert!(text[start..end].contains("Overview"));
    let anchors: Vec<&str> = outline["anchors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|anchor| anchor["id"].as_str().unwrap())
        .collect();
    assert_eq!(anchors, ["details"]);

    let nav = client
        .call(
            "get_outline",
            json!({ "path": "product/main/modules/ROOT/nav.adoc", "anchors": false }),
        )
        .unwrap();
    assert_eq!(nav["kind"], "navigation");
    assert_eq!(nav["outline"][1]["target"], "install.adoc");
    assert_eq!(nav["anchors"], json!([]));
    client.stop();
}
