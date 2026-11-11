//! Navigation coverage: `quality.navigation.unlisted-page` and each component version's
//! [`NavCoverage`]. A navigation file's unresolved entries are its own file diagnostics
//! (`adoc.unresolved-xref-file`), aggregated with every file's, so the trees' `nav-unresolved`
//! diagnostics, which restate them, are not added again.
//!
//! A page is listed when an entry in one of its own component version's navigation files
//! reaches it, through a page alias or not. The start page is listed, being linked from the
//! component's title. A file Antora does not publish as a page (not `.adoc`, or a name beginning
//! with `_` or `.`) is neither counted nor reported. When a version's navigation includes another file, has an entry naming an
//! attribute, or cannot be read, what it lists is not known: its unlisted pages are skipped, not
//! reported. A version with no navigation files has nothing to be listed in, and its pages are
//! not reported.

use std::collections::BTreeSet;

use adoc_antora::{
    nav_tree_each, parse_resource_id, ComponentDescriptor, NavError, NavItem, ResourceFamily,
    UnfollowedKind,
};

use crate::{
    portable_path, workspace::published_page, workspace::Context, Evidence, Finding, FindingCode,
    NavCoverage, RuleId, Skip, VersionAudit,
};

/// Why a version's navigation cannot be relied on to list a page, if it cannot.
struct Unknown {
    reason: String,
}

/// The part of the component version `descriptor` declares.
pub(crate) fn version(context: &Context<'_>, descriptor: &ComponentDescriptor) -> VersionAudit {
    let mut findings = Vec::new();
    let mut skips = Vec::new();
    let catalog = &context.workspace().antora;
    let version = descriptor.version.as_deref();
    // Each file is read on its own: one that cannot be read leaves what the others list
    // listed, and only the pages none of them lists unknown.
    let mut trees = Vec::new();
    let mut unknown = None;
    let each = nav_tree_each(catalog, &descriptor.name, version, context.input.read)
        .unwrap_or_else(|error| vec![Err(error)]);
    for tree in each {
        match tree {
            Ok(tree) => trees.push(tree),
            Err(error) => {
                unknown.get_or_insert(Unknown {
                    reason: unreadable(context, &error),
                });
            }
        }
    }

    for tree in &trees {
        // What the navigation parser read past, as it read it: an include in a comment or a
        // listing block is not one.
        if let Some(first) = tree.unfollowed.first() {
            let what = match first.kind {
                UnfollowedKind::Include => "includes another file",
                UnfollowedKind::Attribute => "has an entry naming an attribute",
            };
            unknown.get_or_insert(Unknown {
                reason: format!(
                    "{} {what}, so what it lists is not known",
                    context.location(&tree.path, None).path,
                ),
            });
        }
        // The tree's own `nav-unresolved` diagnostics are left out: a navigation file is
        // diagnosed as any file is, so its unresolved entries are already its
        // `adoc.unresolved-xref-file` findings, and an entry naming an attribute none.
    }

    let mut listed = BTreeSet::new();
    for tree in &trees {
        collect_listed(&tree.items, descriptor, &mut listed);
    }
    let start = start_page(descriptor);
    // A version spread over several `antora.yml` files lists the navigation of each.
    let navigation_files: usize = catalog
        .descriptors_of(&descriptor.name, version)
        .iter()
        .map(|descriptor| descriptor.nav.len())
        .sum();

    // Each unlisted page's finding names every navigation file looked in.
    let evidence: Vec<Evidence> = trees
        .iter()
        .map(|tree| {
            let nav = context.location(&tree.path, None);
            Evidence {
                message: format!("Not listed in {}", nav.path),
                location: Some(nav),
            }
        })
        .collect();
    let mut coverage = NavCoverage {
        component: descriptor.name.clone(),
        version: descriptor.version.clone(),
        navigation_files,
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
            if navigation_files == 0 {
                continue;
            }
            let location = context.location(&page.source_path, None);
            if let Some(unknown) = &unknown {
                coverage.undetermined += 1;
                skips.push(Skip {
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
            finding.evidence.clone_from(&evidence);
            finding.remediation = Some(
                "Add an entry for it to a navigation file, or exclude it under [quality] if it is \
                 meant to be unlisted"
                    .to_owned(),
            );
            findings.push(finding);
        }
    }
    VersionAudit {
        findings,
        skips,
        coverage,
    }
}

/// Why the navigation could not be read, naming files as the report does: relative to their
/// root, never by an absolute path (a [`NavError`]'s own message holds one).
fn unreadable(context: &Context<'_>, error: &NavError) -> String {
    match error {
        // The reader's message is left out: a front end's reader may name the path in it.
        NavError::Read { path, .. } => format!(
            "Its navigation file {} could not be read",
            context.location(path, None).path
        ),
        NavError::OutsideModules { path } => format!(
            "Its navigation file {} is not in a module",
            context.location(path, None).path
        ),
        NavError::UnknownComponent { .. } => "Its navigation could not be read".to_owned(),
    }
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
                if let (Some(module), Some(ResourceFamily::Page)) = (&target.module, target.family)
                {
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

/// `product 3.0`, or `guide` for a versionless component.
fn label(descriptor: &ComponentDescriptor) -> String {
    match &descriptor.version {
        Some(version) => format!("{} {version}", descriptor.name),
        None => descriptor.name.clone(),
    }
}
