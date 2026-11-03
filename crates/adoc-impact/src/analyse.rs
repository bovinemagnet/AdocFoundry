//! The rules (PRD §§5 and 6.1): for each change, the render consumers reached through
//! includes, the reference-integrity reviews of moved and removed targets and removed anchors,
//! the navigation changes of its component version, and the publications that select any of
//! them, each from the side of the comparison whose evidence shows it.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

use crate::{
    report::{
        Change, ChangeKind, Completeness, Confidence, EvidenceSide, Gap, Impact, ImpactCounts,
        ImpactKind, ImpactReport, ImpactStatus, ReportedChange, SourceSpan, Truncated,
        SCHEMA_VERSION,
    },
    snapshot::{DependencySnapshot, Referrer},
};

/// How far the analysis goes and how much it reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Includes followed from a changed file before the walk stops.
    pub depth: usize,
    /// Impacts reported; more are counted and the report says it was cut.
    pub impacts: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            depth: 32,
            impacts: 1_000,
        }
    }
}

/// A publication profile and what it selects, as the head side's settings name it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Publication {
    pub name: String,
    pub selection: PublicationSelection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PublicationSelection {
    /// One page, by its path.
    Page(PathBuf),
    /// A component version; every version when `version` is `None`.
    Component {
        name: String,
        version: Option<String>,
    },
    /// The pages these navigation files list.
    Navigation(Vec<PathBuf>),
}

/// An impact while it is gathered, keyed so that the same one found on both sides merges.
#[derive(Clone, Debug)]
struct Found {
    kind: ImpactKind,
    confidence: Confidence,
    status: Option<ImpactStatus>,
    path: PathBuf,
    origin: PathBuf,
    sides: BTreeSet<Side>,
    via: Vec<PathBuf>,
    source: Option<(PathBuf, adoc_core::SourceRange)>,
    anchor: Option<String>,
    publication: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Side {
    Base,
    Head,
}

type Key = (
    ImpactKind,
    PathBuf,
    PathBuf,
    Option<String>,
    Option<String>,
    Option<(PathBuf, usize)>,
);

#[derive(Default)]
struct Gathered {
    found: BTreeMap<Key, Found>,
    gaps: Vec<Gap>,
}

impl Gathered {
    fn add(&mut self, found: Found) {
        let key = (
            found.kind,
            found.path.clone(),
            found.origin.clone(),
            found.anchor.clone(),
            found.publication.clone(),
            found
                .source
                .as_ref()
                .map(|(path, range)| (path.clone(), range.start)),
        );
        match self.found.get_mut(&key) {
            Some(known) => {
                known.sides.extend(found.sides);
                if found.via.len() < known.via.len() {
                    known.via = found.via;
                }
                if found.confidence == Confidence::Confirmed {
                    known.confidence = Confidence::Confirmed;
                }
            }
            None => {
                self.found.insert(key, found);
            }
        }
    }
}

/// The impact of `changes`, with the evidence of `base` (absent for explicit paths) and `head`
/// (the live workspace or the head revision), the head side's `publications`, within `limits`.
#[must_use]
pub fn analyse(
    changes: &[Change],
    base: Option<&DependencySnapshot<'_>>,
    head: &DependencySnapshot<'_>,
    publications: &[Publication],
    limits: Limits,
) -> ImpactReport {
    let mut gathered = Gathered::default();
    let mut changes: Vec<Change> = changes.to_vec();
    changes.sort();
    changes.dedup();

    for change in &changes {
        let path = &change.path;
        if !head.inside_roots(path) && !base.is_some_and(|base| base.inside_roots(old_path(change)))
        {
            gathered.gaps.push(Gap {
                reason: "outside-every-root",
                detail: format!("{} lies outside the workspace's roots", path.display()),
                affected: vec![head.display(path)],
            });
            continue;
        }
        match &change.kind {
            ChangeKind::Added => {
                render(&mut gathered, head, path, path, Side::Head, limits);
                now_resolving(&mut gathered, base, head, path);
                navigation(&mut gathered, base, head, path);
            }
            ChangeKind::Modified => {
                if let Some(base) = base {
                    render(&mut gathered, base, path, path, Side::Base, limits);
                }
                render(&mut gathered, head, path, path, Side::Head, limits);
                removed_anchors(&mut gathered, base, head, path);
                navigation(&mut gathered, base, head, path);
            }
            ChangeKind::Removed => match base {
                Some(base) => {
                    render(&mut gathered, base, path, path, Side::Base, limits);
                    moved_or_removed(&mut gathered, base, head, path, None);
                    navigation(&mut gathered, Some(base), head, path);
                }
                None => gathered.gaps.push(no_base(head, path, "removed")),
            },
            ChangeKind::Renamed { from } => {
                render(&mut gathered, head, path, path, Side::Head, limits);
                match base {
                    Some(base) => {
                        render(&mut gathered, base, from, from, Side::Base, limits);
                        moved_or_removed(&mut gathered, base, head, from, Some(path));
                        navigation(&mut gathered, Some(base), head, from);
                    }
                    None => gathered.gaps.push(no_base(head, from, "moved")),
                }
                navigation(&mut gathered, base, head, path);
            }
        }
        let known = |snapshot: &DependencySnapshot<'_>, path: &Path| {
            snapshot.resource(path).is_some()
                || snapshot.is_listed_nav(path)
                || snapshot.is_descriptor(path)
        };
        if !known(head, path) && !base.is_some_and(|base| known(base, old_path(change))) {
            gathered.gaps.push(Gap {
                reason: "not-in-catalog",
                detail: format!(
                    "{} belongs to no component version the workspace holds, so what uses it \
                     elsewhere (in a component this workspace does not have) is not known",
                    head.display(path)
                ),
                affected: vec![head.display(path)],
            });
        }
        let asset = head.is_asset(path) || base.is_some_and(|base| base.is_asset(old_path(change)));
        if asset {
            gathered.gaps.push(Gap {
                reason: "asset-consumers-not-analysed",
                detail: format!(
                    "{} is an image or attachment; the pages that show or link it are not \
                     traced yet",
                    head.display(path)
                ),
                affected: vec![head.display(path)],
            });
        }
    }
    publications_of(&mut gathered, head, publications);
    finish(gathered, &changes, base, head, limits)
}

fn old_path(change: &Change) -> &Path {
    match &change.kind {
        ChangeKind::Renamed { from } => from,
        _ => &change.path,
    }
}

fn no_base(head: &DependencySnapshot<'_>, path: &Path, what: &str) -> Gap {
    Gap {
        reason: "no-base-revision",
        detail: format!(
            "{} was {what}, and with no base revision what used it cannot be known",
            head.display(path)
        ),
        affected: vec![head.display(path)],
    }
}

/// The changed file itself, when it is a page, and every page that includes it, transitively,
/// on `side`: rendered content that may change.
fn render(
    gathered: &mut Gathered,
    snapshot: &DependencySnapshot<'_>,
    changed: &Path,
    origin: &Path,
    side: Side,
    limits: Limits,
) {
    if !snapshot.has(changed) {
        return;
    }
    if snapshot.is_page(changed) {
        gathered.add(found(
            ImpactKind::Render,
            changed,
            origin,
            side,
            vec![changed.to_path_buf()],
        ));
    }
    let mut seen: BTreeSet<PathBuf> = BTreeSet::from([changed.to_path_buf()]);
    let mut queue: VecDeque<Vec<PathBuf>> = VecDeque::from([vec![changed.to_path_buf()]]);
    while let Some(via) = queue.pop_front() {
        if via.len() > limits.depth {
            continue;
        }
        let last = via.last().expect("a path holds the changed file");
        for includer in snapshot.includers(last) {
            if !seen.insert(includer.path.clone()) {
                continue;
            }
            let mut next = via.clone();
            next.push(includer.path.clone());
            if snapshot.is_page(&includer.path) {
                let mut impact = found(
                    ImpactKind::Render,
                    &includer.path,
                    origin,
                    side,
                    next.clone(),
                );
                impact.source = Some((includer.path.clone(), includer.range));
                gathered.add(impact);
            }
            queue.push_back(next);
        }
    }
}

fn found(kind: ImpactKind, path: &Path, origin: &Path, side: Side, via: Vec<PathBuf>) -> Found {
    Found {
        kind,
        confidence: Confidence::Confirmed,
        status: None,
        path: path.to_path_buf(),
        origin: origin.to_path_buf(),
        sides: BTreeSet::from([side]),
        via,
        source: None,
        anchor: None,
        publication: None,
    }
}

/// A referrer as an impact: navigation when it is an entry of a navigation file, else a
/// reference to review.
fn referrer_impact(
    snapshot: &DependencySnapshot<'_>,
    referrer: &Referrer,
    target: &Path,
    origin: &Path,
    side: Side,
    status: ImpactStatus,
) -> Found {
    let kind = if snapshot.is_listed_nav(&referrer.path) {
        ImpactKind::Navigation
    } else {
        ImpactKind::ReferenceIntegrity
    };
    let mut impact = found(
        kind,
        &referrer.path,
        origin,
        side,
        vec![target.to_path_buf(), referrer.path.clone()],
    );
    impact.status = Some(status);
    impact.source = Some((referrer.path.clone(), referrer.range));
    impact
}

/// The references to a removed file (`to` is `None`) or a moved one, found in base: broken
/// when head still writes them and they no longer lead there, resolved by an alias when the
/// moved page's `page-aliases` covers them, resolved by the change otherwise.
fn moved_or_removed(
    gathered: &mut Gathered,
    base: &DependencySnapshot<'_>,
    head: &DependencySnapshot<'_>,
    old: &Path,
    to: Option<&Path>,
) {
    for referrer in base.referrers(old) {
        if referrer.path == old {
            continue;
        }
        let still = head
            .has(&referrer.path)
            .then(|| head.still_writes(&referrer.path, &referrer.written))
            .flatten();
        let (status, side, source) = match still {
            None => (ImpactStatus::ResolvedByChange, Side::Base, referrer.range),
            Some(range) => match head.resolves(&referrer.path, &referrer.written) {
                Some(now) if to.is_some_and(|to| to == now) => {
                    (ImpactStatus::ResolvedByAlias, Side::Head, range)
                }
                Some(_) => (ImpactStatus::ResolvedByChange, Side::Head, range),
                None => (ImpactStatus::Broken, Side::Head, range),
            },
        };
        let mut impact = referrer_impact(base, &referrer, old, old, Side::Base, status);
        impact.sides.insert(side);
        impact.source = Some((referrer.path.clone(), source));
        gathered.add(impact);
    }
}

/// A modified page's anchors that base has and head does not: their references, from both
/// sides, to review.
fn removed_anchors(
    gathered: &mut Gathered,
    base: Option<&DependencySnapshot<'_>>,
    head: &DependencySnapshot<'_>,
    path: &Path,
) {
    let Some(base) = base else {
        return;
    };
    let now = head.anchors(path);
    for id in base.anchors(path).difference(&now) {
        for (snapshot, side) in [(base, Side::Base), (head, Side::Head)] {
            for referrer in snapshot.anchor_referrers(path, id) {
                let still = head
                    .has(&referrer.path)
                    .then(|| head.still_writes(&referrer.path, &referrer.written))
                    .flatten()
                    .is_some();
                let status = if still {
                    ImpactStatus::Broken
                } else {
                    ImpactStatus::ResolvedByChange
                };
                let mut impact = referrer_impact(snapshot, &referrer, path, path, side, status);
                impact.kind = ImpactKind::ReferenceIntegrity;
                impact.anchor = Some(id.clone());
                gathered.add(impact);
            }
        }
    }
}

/// An added file's referrers in head that did not resolve in base and now do.
fn now_resolving(
    gathered: &mut Gathered,
    base: Option<&DependencySnapshot<'_>>,
    head: &DependencySnapshot<'_>,
    path: &Path,
) {
    for referrer in head.referrers(path) {
        if referrer.path == path {
            continue;
        }
        let before = base.and_then(|base| base.resolves(&referrer.path, &referrer.written));
        if before.as_deref() == Some(path) {
            continue;
        }
        let mut impact = referrer_impact(
            head,
            &referrer,
            path,
            path,
            Side::Head,
            ImpactStatus::NowResolves,
        );
        if base.is_none() {
            impact.confidence = Confidence::Potential;
        }
        gathered.add(impact);
    }
}

/// A changed navigation file's or descriptor's effect on its component version's navigation:
/// the pages it lists now and no longer, compared between the sides; and a changed page's
/// navigation entries.
fn navigation(
    gathered: &mut Gathered,
    base: Option<&DependencySnapshot<'_>>,
    head: &DependencySnapshot<'_>,
    path: &Path,
) {
    let is_nav = head.is_listed_nav(path) || base.is_some_and(|base| base.is_listed_nav(path));
    let is_descriptor =
        head.is_descriptor(path) || base.is_some_and(|base| base.is_descriptor(path));
    if !is_nav && !is_descriptor {
        return;
    }
    // The navigation files whose listing may have changed: this one, or every one the
    // descriptor's component version lists on either side.
    let mut navs: BTreeSet<PathBuf> = BTreeSet::new();
    if is_nav {
        navs.insert(path.to_path_buf());
    }
    if is_descriptor {
        for snapshot in base.into_iter().chain([head]) {
            if let Some((component, version)) = snapshot.component_version(path) {
                navs.extend(snapshot.version_navs(&component, version.as_deref()));
            }
        }
    }
    let mut listed_before: BTreeSet<PathBuf> = BTreeSet::new();
    let mut listed_now: BTreeSet<PathBuf> = BTreeSet::new();
    for nav in &navs {
        if let Some(base) = base {
            if base.is_listed_nav(nav) {
                listed_before.extend(base.listed_by(nav));
            }
        }
        if head.is_listed_nav(nav) {
            listed_now.extend(head.listed_by(nav));
        }
    }
    let Some(_) = base else {
        // Without a base, every page the navigation lists may have moved in it.
        for page in &listed_now {
            let mut impact = found(
                ImpactKind::Navigation,
                page,
                path,
                Side::Head,
                vec![path.to_path_buf(), page.clone()],
            );
            impact.confidence = Confidence::Potential;
            gathered.add(impact);
        }
        return;
    };
    for page in listed_now.difference(&listed_before) {
        let mut impact = found(
            ImpactKind::Navigation,
            page,
            path,
            Side::Head,
            vec![path.to_path_buf(), page.clone()],
        );
        impact.status = Some(ImpactStatus::Listed);
        gathered.add(impact);
    }
    for page in listed_before.difference(&listed_now) {
        let mut impact = found(
            ImpactKind::Navigation,
            page,
            path,
            Side::Base,
            vec![path.to_path_buf(), page.clone()],
        );
        impact.status = Some(ImpactStatus::Unlisted);
        gathered.add(impact);
    }
}

/// Each publication whose selection holds a page with a render or navigation impact.
fn publications_of(
    gathered: &mut Gathered,
    head: &DependencySnapshot<'_>,
    publications: &[Publication],
) {
    let affected: Vec<(PathBuf, PathBuf, BTreeSet<Side>)> = gathered
        .found
        .values()
        .filter(|impact| matches!(impact.kind, ImpactKind::Render | ImpactKind::Navigation))
        .map(|impact| {
            (
                impact.path.clone(),
                impact.origin.clone(),
                impact.sides.clone(),
            )
        })
        .collect();
    for publication in publications {
        for (page, origin, sides) in &affected {
            let selected = match &publication.selection {
                PublicationSelection::Page(selected) => Some(selected == page),
                PublicationSelection::Component { name, version } => {
                    let known = match version {
                        Some(version) => {
                            head.state().antora.component(name, Some(version)).is_some()
                        }
                        None => !head.state().antora.versions_of(name).is_empty(),
                    };
                    known.then(|| {
                        head.component_version(page)
                            .is_some_and(|(component, page_version)| {
                                component == *name
                                    && version.as_ref().is_none_or(|version| {
                                        page_version.as_ref() == Some(version)
                                    })
                            })
                    })
                }
                PublicationSelection::Navigation(files) => {
                    let known = files.iter().all(|file| head.has(file));
                    let listed = files
                        .iter()
                        .any(|file| file == page || head.listed_by(file).contains(page));
                    if known {
                        Some(listed)
                    } else if listed {
                        Some(true)
                    } else {
                        None
                    }
                }
            };
            let (confidence, matched) = match selected {
                Some(true) => (Confidence::Confirmed, true),
                Some(false) => (Confidence::Confirmed, false),
                // The selection names content this revision does not hold: it may select it.
                None => (Confidence::Potential, true),
            };
            if !matched {
                continue;
            }
            gathered.add(Found {
                kind: ImpactKind::Publication,
                confidence,
                status: None,
                path: page.clone(),
                origin: origin.clone(),
                sides: sides.clone(),
                via: vec![origin.clone(), page.clone()],
                source: None,
                anchor: None,
                publication: Some(publication.name.clone()),
            });
        }
    }
}

fn evidence(sides: &BTreeSet<Side>) -> EvidenceSide {
    match (sides.contains(&Side::Base), sides.contains(&Side::Head)) {
        (true, true) => EvidenceSide::Both,
        (true, false) => EvidenceSide::Base,
        _ => EvidenceSide::Head,
    }
}

fn finish(
    gathered: Gathered,
    changes: &[Change],
    base: Option<&DependencySnapshot<'_>>,
    head: &DependencySnapshot<'_>,
    limits: Limits,
) -> ImpactReport {
    // Resource identities from head, or base for what head no longer has.
    let identity = |path: &Path| {
        head.resource(path)
            .map(|_| (head.resource_id(path), head.component_version(path)))
            .or_else(|| {
                base.and_then(|base| {
                    base.resource(path)
                        .map(|_| (base.resource_id(path), base.component_version(path)))
                })
            })
            .unwrap_or((None, head.component_version(path)))
    };
    let display = |path: &Path| {
        if head.inside_roots(path) {
            head.display(path)
        } else {
            base.map_or_else(|| head.display(path), |base| base.display(path))
        }
    };
    let mut impacts: Vec<Impact> = gathered
        .found
        .into_values()
        .map(|found| {
            let (resource_id, version) = identity(&found.path);
            let (component, version) = match version {
                Some((component, version)) => (Some(component), version),
                None => (None, None),
            };
            Impact {
                kind: found.kind,
                confidence: found.confidence,
                status: found.status,
                path: display(&found.path),
                resource_id,
                component,
                version,
                publication: found.publication,
                origin: display(&found.origin),
                evidence: evidence(&found.sides),
                via: found.via.iter().map(|path| display(path)).collect(),
                source: found.source.map(|(path, range)| SourceSpan {
                    path: display(&path),
                    range: range.into(),
                }),
                anchor: found.anchor,
            }
        })
        .collect();
    impacts.sort_by(|a, b| {
        (
            a.kind,
            &a.path,
            &a.origin,
            &a.publication,
            &a.anchor,
            &a.source,
        )
            .cmp(&(
                b.kind,
                &b.path,
                &b.origin,
                &b.publication,
                &b.anchor,
                &b.source,
            ))
    });
    let mut counts = ImpactCounts::default();
    for impact in &impacts {
        match impact.kind {
            ImpactKind::Render => counts.render += 1,
            ImpactKind::ReferenceIntegrity => counts.reference_integrity += 1,
            ImpactKind::Navigation => counts.navigation += 1,
            ImpactKind::Publication => counts.publication += 1,
        }
    }
    let total = impacts.len();
    let truncated = (total > limits.impacts).then(|| {
        impacts.truncate(limits.impacts);
        Truncated {
            shown: limits.impacts,
            total,
        }
    });
    let mut reported: Vec<ReportedChange> = changes
        .iter()
        .map(|change| {
            let (kind, from) = match &change.kind {
                ChangeKind::Added => ("added", None),
                ChangeKind::Modified => ("modified", None),
                ChangeKind::Removed => ("removed", None),
                ChangeKind::Renamed { from } => ("renamed", Some(display(from))),
            };
            ReportedChange {
                kind,
                path: display(&change.path),
                from,
                resource_id: identity(&change.path).0,
            }
        })
        .collect();
    reported.sort();
    let mut gaps = gathered.gaps;
    gaps.sort();
    gaps.dedup();
    let completeness = if gaps.is_empty() {
        Completeness::Complete
    } else {
        Completeness::Partial { gaps }
    };
    ImpactReport {
        schema_version: SCHEMA_VERSION,
        completeness,
        changes: reported,
        impacts,
        counts,
        truncated,
    }
}
