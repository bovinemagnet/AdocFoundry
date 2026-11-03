//! Antora discovery (MCP PRD §7.2): resources listed in coordinate order and resumed, a page's
//! outline, and the exact references to a page or an anchor, on `tests/fixtures/discovery`.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use adoc_core::probe::{self, Probe};
use adoc_engine::{
    normalize_path, Engine, IncomingReference, ReferenceKind, ResourceFamily, ResourceQuery,
    Settings,
};

fn fixture() -> PathBuf {
    normalize_path(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/discovery"))
}

fn engine() -> Engine {
    let mut engine = Engine::new(Settings::load(None, None).unwrap().with_git_opt_in());
    engine.open_workspace(vec![fixture()]).unwrap();
    assert!(engine.wait_for_workers(Duration::from_secs(30)));
    engine.build_graph_blocking();
    engine
}

fn pages(query: &ResourceQuery) -> ResourceQuery {
    ResourceQuery {
        family: Some(ResourceFamily::Page),
        ..query.clone()
    }
}

/// Every resource `query` takes, listed `limit` at a time.
fn all(engine: &Engine, query: &ResourceQuery, limit: usize) -> Vec<String> {
    let mut ids = Vec::new();
    let mut after = None;
    loop {
        let (entries, more) = engine.resources_after(query, after.as_ref(), limit);
        assert!(entries.len() <= limit);
        ids.extend(entries.iter().map(|entry| entry.resource_id.clone()));
        if !more {
            return ids;
        }
        after = entries.last().map(|entry| entry.coordinate.clone());
    }
}

#[test]
fn resources_list_in_coordinate_order_and_resume() {
    let engine = engine();
    let every_page = pages(&ResourceQuery::default());
    let whole = all(&engine, &every_page, 100);
    assert_eq!(
        whole,
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
    assert_eq!(all(&engine, &every_page, 2), whole, "resumed, none twice");
    assert_eq!(all(&engine, &every_page, 1), whole);

    // The versionless version and a numbered one are never mixed up.
    let versionless = pages(&ResourceQuery {
        component: Some("product".into()),
        version: Some(None),
        ..ResourceQuery::default()
    });
    assert_eq!(
        all(&engine, &versionless, 100),
        [
            "product:ROOT:page$draft.adoc",
            "product:ROOT:page$index.adoc",
            "product:ROOT:page$install.adoc",
        ]
    );
    let three = pages(&ResourceQuery {
        component: Some("product".into()),
        version: Some(Some("3.0".into())),
        module: Some("admin".into()),
        ..ResourceQuery::default()
    });
    assert_eq!(
        all(&engine, &three, 100),
        ["3.0@product:admin:page$users.adoc"]
    );
    let partials = ResourceQuery {
        family: Some(ResourceFamily::Partial),
        ..ResourceQuery::default()
    };
    assert_eq!(
        all(&engine, &partials, 100),
        ["product:ROOT:partial$note.adoc"]
    );
    let rooted = pages(&ResourceQuery {
        root: Some(fixture().join("guide")),
        ..ResourceQuery::default()
    });
    assert_eq!(
        all(&engine, &rooted, 100),
        ["1.0@guide:ROOT:page$index.adoc"]
    );
}

#[test]
fn a_page_is_in_navigation_only_through_its_own_version() {
    let engine = engine();
    let (entries, _) = engine.resources_after(&pages(&ResourceQuery::default()), None, 100);
    let listed: Vec<(&str, Option<bool>)> = entries
        .iter()
        .map(|entry| (entry.resource_id.as_str(), entry.in_navigation))
        .collect();
    assert_eq!(
        listed,
        [
            ("1.0@guide:ROOT:page$index.adoc", Some(true)),
            ("product:ROOT:page$draft.adoc", Some(false)),
            ("product:ROOT:page$index.adoc", Some(true)),
            ("product:ROOT:page$install.adoc", Some(true)),
            ("3.0@product:ROOT:page$index.adoc", Some(true)),
            // The versionless version's navigation lists its own install page, not this one.
            ("3.0@product:ROOT:page$install.adoc", Some(false)),
            ("3.0@product:admin:page$users.adoc", Some(false)),
        ]
    );
    assert_eq!(entries[2].title.as_deref(), Some("Product"));
    let (partials, _) = engine.resources_after(
        &ResourceQuery {
            family: Some(ResourceFamily::Partial),
            ..ResourceQuery::default()
        },
        None,
        10,
    );
    assert_eq!(partials[0].in_navigation, None, "only pages are listed");
}

#[test]
fn a_listing_reads_only_what_it_returns() {
    let engine = engine();
    let guide = pages(&ResourceQuery {
        component: Some("guide".into()),
        ..ResourceQuery::default()
    });
    probe::reset();
    let (entries, more) = engine.resources_after(&guide, None, 10);
    assert_eq!(entries.len(), 1);
    assert!(!more);
    // The guide's page, then the first resource of the next component, which ends the walk.
    assert!(
        probe::count(Probe::ListResource) <= 2,
        "{}",
        probe::count(Probe::ListResource)
    );

    // One module of one version: its pages, then the first resource past them.
    let admin = pages(&ResourceQuery {
        component: Some("product".into()),
        version: Some(Some("3.0".into())),
        module: Some("admin".into()),
        ..ResourceQuery::default()
    });
    probe::reset();
    let (entries, _) = engine.resources_after(&admin, None, 10);
    assert_eq!(entries.len(), 1);
    assert!(
        probe::count(Probe::ListResource) <= 2,
        "{}",
        probe::count(Probe::ListResource)
    );

    let every_page = pages(&ResourceQuery::default());
    let (first, _) = engine.resources_after(&every_page, None, 2);
    probe::reset();
    let (next, _) = engine.resources_after(&every_page, first.last().map(|e| &e.coordinate), 2);
    assert_eq!(next.len(), 2);
    assert!(probe::count(Probe::ListResource) <= 4);
}

fn text_at(reference: &IncomingReference) -> String {
    let text = std::fs::read_to_string(&reference.path).unwrap();
    text[reference.range.start..reference.range.end].to_owned()
}

fn described(engine: &Engine, found: &[IncomingReference]) -> Vec<(String, ReferenceKind, String)> {
    let _ = engine;
    found
        .iter()
        .map(|reference| {
            let path = reference
                .path
                .strip_prefix(fixture())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            (path, reference.kind, text_at(reference))
        })
        .collect()
}

#[test]
fn incoming_references_are_exact() {
    let engine = engine();
    let install = fixture().join("product/main/modules/ROOT/pages/install.adoc");
    let found = engine.incoming_references(&install, None);
    assert_eq!(
        described(&engine, &found),
        [
            (
                "guide/modules/ROOT/pages/index.adoc".to_owned(),
                ReferenceKind::Xref,
                "xref:product::install.adoc#prerequisites[the product prerequisites]".to_owned()
            ),
            (
                "product/main/modules/ROOT/nav.adoc".to_owned(),
                ReferenceKind::Nav,
                "xref:install.adoc[Installing]".to_owned()
            ),
            (
                "product/main/modules/ROOT/pages/index.adoc".to_owned(),
                ReferenceKind::Xref,
                "xref:install.adoc#prerequisites[the prerequisites]".to_owned()
            ),
        ]
    );
    assert_eq!(found[1].target, "install.adoc");

    let anchor = engine.incoming_references(&install, Some("prerequisites"));
    let kinds: Vec<(String, ReferenceKind, String)> = described(&engine, &anchor);
    assert_eq!(
        kinds
            .iter()
            .map(|(path, kind, text)| (path.as_str(), *kind, text.as_str()))
            .collect::<Vec<_>>(),
        [
            (
                "guide/modules/ROOT/pages/index.adoc",
                ReferenceKind::Anchor,
                "xref:product::install.adoc#prerequisites[the product prerequisites]"
            ),
            (
                "product/main/modules/ROOT/pages/index.adoc",
                ReferenceKind::Anchor,
                "xref:install.adoc#prerequisites[the prerequisites]"
            ),
            (
                "product/main/modules/ROOT/pages/install.adoc",
                ReferenceKind::Anchor,
                "<<prerequisites>>"
            ),
        ]
    );

    let note = fixture().join("product/main/modules/ROOT/partials/note.adoc");
    let includes = described(&engine, &engine.incoming_references(&note, None));
    assert_eq!(includes.len(), 3, "{includes:?}");
    assert!(includes
        .iter()
        .all(|(_, kind, text)| *kind == ReferenceKind::Include && text.starts_with("include::")));

    // The numbered version's install page has nothing pointing at it.
    let v3 = fixture().join("product/v3/modules/ROOT/pages/install.adoc");
    assert!(engine.incoming_references(&v3, None).is_empty());
}

#[test]
fn a_pages_outline_and_section_ids() {
    let engine = engine();
    let index = fixture().join("product/main/modules/ROOT/pages/index.adoc");
    let outline = engine.outline_of(&index).unwrap();
    assert_eq!(outline.len(), 1);
    assert_eq!(outline[0].name, "Product");
    let sections: Vec<&str> = outline[0]
        .children
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect();
    assert_eq!(sections, ["Overview", "Details"]);
    assert_eq!(engine.section_ids(&index), ["overview", "details"]);

    let nav = fixture().join("product/main/modules/ROOT/nav.adoc");
    let entries = engine.outline_of(&nav).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].detail.as_deref(), Some("install.adoc"));
    assert!(engine.outline_of(&fixture().join("absent.adoc")).is_none());
}
