//! Navigation coverage: `quality.navigation.unlisted-page`, each component version's
//! [`NavCoverage`], and the navigation's own `adoc.antora.nav-unresolved` diagnostics, which no
//! file's diagnostics report.
//!
//! A page is listed when an entry in one of its own component version's navigation files
//! reaches it, through a page alias or not. The start page is listed, being linked from the
//! component's title. A file Antora does not publish as a page (not `.adoc`, or a name beginning
//! with `_` or `.`) is neither counted nor reported. When a version's navigation includes another file, has an entry naming an
//! attribute, or cannot be read, what it lists is not known: its unlisted pages are skipped, not
//! reported. A version with no navigation files has nothing to be listed in, and its pages are
//! not reported.

use std::{collections::BTreeSet, path::Path};

use adoc_antora::{
    nav_tree, parse_nav, parse_resource_id, ComponentDescriptor, NavItem, NavTree, ResourceFamily,
};
use adoc_core::{DiagnosticCode, SourceRange};

use crate::{
    portable_path, workspace::published_page, workspace::Context, AuditBuilder, Evidence, Finding,
    FindingCode, NavCoverage, RuleId, Skip,
};

pub(crate) fn check(context: &Context<'_>, builder: &mut AuditBuilder) {
    for descriptor in context.workspace().antora.components() {
        if context.includes(&descriptor.name, descriptor.version.as_deref()) {
            component_version(context, descriptor, builder);
        }
    }
}

/// Why a version's navigation cannot be relied on to list a page, if it cannot.
struct Unknown {
    reason: String,
}

fn component_version(
    context: &Context<'_>,
    descriptor: &ComponentDescriptor,
    builder: &mut AuditBuilder,
) {
    let catalog = &context.workspace().antora;
    let version = descriptor.version.as_deref();
    let (trees, mut unknown) =
        match nav_tree(catalog, &descriptor.name, version, context.input.read) {
            Ok(trees) => (trees, None),
            Err(error) => (
                Vec::new(),
                Some(Unknown {
                    reason: format!("Its navigation could not be read: {error}"),
                }),
            ),
        };

    for tree in &trees {
        let dynamic = dynamic_entries(context, tree);
        if let Some(first) = dynamic.first() {
            unknown.get_or_insert(Unknown {
                reason: format!(
                    "{} {}, so what it lists is not known",
                    context.location(&tree.path, None).path,
                    first.1
                ),
            });
        }
        for diagnostic in &tree.diagnostics {
            if dynamic.iter().any(|(range, _)| *range == diagnostic.range) {
                builder.skip(Skip {
                    location: context.location(&tree.path, Some(diagnostic.range)),
                    code: FindingCode::Diagnostic(DiagnosticCode::AntoraNavUnresolved),
                    reason: "The entry's target names an attribute".to_owned(),
                });
            } else {
                builder.finding(context.finding_for(&tree.path, diagnostic));
            }
        }
    }

    let mut listed = BTreeSet::new();
    for tree in &trees {
        collect_listed(&tree.items, descriptor, &mut listed);
    }
    let start = start_page(descriptor);

    let mut coverage = NavCoverage {
        component: descriptor.name.clone(),
        version: descriptor.version.clone(),
        navigation_files: descriptor.nav.len(),
        pages: 0,
        listed: 0,
        undetermined: 0,
    };
    for module in catalog.modules_of(&descriptor.name, version) {
        for page in catalog.resources_in(
            &descriptor.name,
            version,
            &module.name,
            ResourceFamily::Page,
        ) {
            let relative = &page.coordinate.relative_path;
            if !published_page(relative) {
                continue;
            }
            coverage.pages += 1;
            let key = (module.name.clone(), portable_path(relative));
            if listed.contains(&key) || start.as_ref() == Some(&key) {
                coverage.listed += 1;
                continue;
            }
            if descriptor.nav.is_empty() {
                continue;
            }
            let location =
                context.location(&page.source_path, title_range(context, &page.source_path));
            if let Some(unknown) = &unknown {
                coverage.undetermined += 1;
                builder.skip(Skip {
                    location,
                    code: FindingCode::Rule(RuleId::UNLISTED_PAGE),
                    reason: unknown.reason.clone(),
                });
                continue;
            }
            let mut finding = Finding::new(
                FindingCode::Rule(RuleId::UNLISTED_PAGE),
                RuleId::UNLISTED_PAGE.spec().default_severity,
                format!(
                    "`{}:{}` is in no navigation file of {}",
                    key.0,
                    key.1,
                    label(descriptor)
                ),
                location,
            );
            finding.coordinate = Some((&page.coordinate).into());
            finding.evidence = trees
                .iter()
                .map(|tree| {
                    let nav = context.location(&tree.path, None);
                    Evidence {
                        message: format!("Not listed in {}", nav.path),
                        location: Some(nav),
                    }
                })
                .collect();
            finding.remediation = Some(
                "Add an entry for it to a navigation file, or exclude it under [quality] if it is \
                 meant to be unlisted"
                    .to_owned(),
            );
            builder.finding(finding);
        }
    }
    builder.coverage(coverage);
}

/// The entries of `tree` whose targets cannot be known without the build: an `include::`
/// line, which the navigation parser does not follow, or an entry naming an attribute. Each is
/// its line's range and a description.
fn dynamic_entries(context: &Context<'_>, tree: &NavTree) -> Vec<(SourceRange, &'static str)> {
    let Ok(text) = (context.input.read)(&tree.path) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        if content.trim_start().starts_with("include::") {
            found.push((
                SourceRange::new(offset, offset + content.len()),
                "includes another file",
            ));
        }
        offset += line.len();
    }
    for entry in parse_nav(&text) {
        if entry
            .target
            .as_deref()
            .is_some_and(|target| target.contains('{'))
        {
            found.push((
                SourceRange::new(entry.start, entry.end),
                "has an entry naming an attribute",
            ));
        }
    }
    found.sort_by_key(|(range, _)| *range);
    found
}

/// Every `(module, path)` of this component version an entry of `items` reaches.
fn collect_listed(
    items: &[NavItem],
    descriptor: &ComponentDescriptor,
    listed: &mut BTreeSet<(String, String)>,
) {
    for item in items {
        if let Some(target) = &item.target {
            if target.component.as_deref() == Some(descriptor.name.as_str())
                && target.version == descriptor.version
            {
                if let Some(module) = &target.module {
                    listed.insert((module.clone(), target.path.clone()));
                }
            }
        }
        collect_listed(&item.children, descriptor, listed);
    }
}

/// The component version's start page as `(module, path)`: its `start_page`, else
/// `ROOT:index.adoc`.
fn start_page(descriptor: &ComponentDescriptor) -> Option<(String, String)> {
    let id = parse_resource_id(
        descriptor
            .start_page
            .as_deref()
            .unwrap_or("ROOT:index.adoc"),
    )
    .ok()?;
    Some((id.module.unwrap_or_else(|| "ROOT".to_owned()), id.path))
}

/// The range of the page's title, to point a finding at, if the index has one.
fn title_range(context: &Context<'_>, path: &Path) -> Option<SourceRange> {
    context
        .workspace()
        .index
        .file(path)?
        .document
        .title
        .as_ref()
        .map(|title| title.range)
}

/// `product 3.0`, or `guide` for a versionless component.
fn label(descriptor: &ComponentDescriptor) -> String {
    match &descriptor.version {
        Some(version) => format!("{} {version}", descriptor.name),
        None => descriptor.name.clone(),
    }
}
