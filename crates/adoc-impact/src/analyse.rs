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
    source: Option<Source>,
    anchor: Option<String>,
    publication: Option<String>,
}

/// The reference or include an impact rests on. Its target as written and its ordinal among
/// those with that target identify it on both sides; its range is the side's that found it,
/// head's whenever head has it.
#[derive(Clone, Debug)]
struct Source {
    path: PathBuf,
    range: adoc_core::SourceRange,
    side: Side,
    written: String,
    ordinal: usize,
}

impl Source {
    fn of(referrer: &Referrer, side: Side) -> Self {
        Self {
            path: referrer.path.clone(),
            range: referrer.range,
            side,
            written: referrer.written.clone(),
            ordinal: referrer.ordinal,
        }
    }
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
    Option<(PathBuf, String, usize)>,
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
                .map(|source| (source.path.clone(), source.written.clone(), source.ordinal)),
        );
        match self.found.get_mut(&key) {
            Some(known) => {
                known.sides.extend(found.sides);
                let head_source = found
                    .source
                    .as_ref()
                    .is_some_and(|source| source.side == Side::Head);
                let base_source = known
                    .source
                    .as_ref()
                    .is_some_and(|source| source.side == Side::Base);
                if head_source && base_source {
                    known.source = found.source;
                }
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
                if let Some(base) = base {
                    lost_referrers(&mut gathered, base, head, path, path);
                }
                removed_anchors(&mut gathered, base, head, path);
                if let Some(base) = base {
                    descriptor(&mut gathered, base, head, path);
                }
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
    publications_of(&mut gathered, base, head, publications);
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
                impact.source = Some(Source::of(&includer, side));
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
    impact.source = Some(Source::of(referrer, side));
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
            .then(|| head.still_writes(&referrer.path, &referrer.written, referrer.ordinal))
            .flatten();
        let (status, side, range) = match still {
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
        impact.source = Some(Source {
            range,
            side,
            ..Source::of(&referrer, side)
        });
        gathered.add(impact);
    }
}

/// The references that led to the file at `path` in base and no longer do in head, though the
/// file is still there (a dropped `page-aliases` entry, say): broken when head still writes
/// them, resolved by the change when it does not. A reference that would still lead there is
/// not one to review, whether or not head still writes it.
fn lost_referrers(
    gathered: &mut Gathered,
    base: &DependencySnapshot<'_>,
    head: &DependencySnapshot<'_>,
    path: &Path,
    origin: &Path,
) {
    for referrer in base.referrers(path) {
        if referrer.path == path
            || head.resolves(&referrer.path, &referrer.written).as_deref() == Some(path)
        {
            continue;
        }
        let still = head
            .has(&referrer.path)
            .then(|| head.still_writes(&referrer.path, &referrer.written, referrer.ordinal))
            .flatten();
        let (status, side, range) = match still {
            Some(range) => (ImpactStatus::Broken, Side::Head, range),
            None => (ImpactStatus::ResolvedByChange, Side::Base, referrer.range),
        };
        let mut impact = referrer_impact(base, &referrer, path, origin, Side::Base, status);
        impact.sides.insert(side);
        impact.source = Some(Source {
            range,
            side,
            ..Source::of(&referrer, side)
        });
        gathered.add(impact);
    }
}

/// A modified `antora.yml`, compared between the sides beyond its navigation files' listings:
/// a new name or version moves every page of the version, so the references that led to them
/// by the old coordinate are reviewed; a new title, version, start page or navigation list is
/// a navigation impact on the version; changed attributes may change any page that reads
/// them, which the index cannot say, so they are a gap.
fn descriptor(
    gathered: &mut Gathered,
    base: &DependencySnapshot<'_>,
    head: &DependencySnapshot<'_>,
    path: &Path,
) {
    let (Some(before), Some(after)) = (base.descriptor(path), head.descriptor(path)) else {
        return;
    };
    let moved = (&before.name, &before.version) != (&after.name, &after.version);
    if moved {
        for page in base.version_pages(&before.name, before.version.as_deref()) {
            lost_referrers(gathered, base, head, &page, path);
        }
    }
    let navigation_changed = moved
        || before.title != after.title
        || before.display_version != after.display_version
        || before.start_page != after.start_page
        || before.nav != after.nav
        || before.prerelease != after.prerelease;
    if navigation_changed {
        gathered.add(Found {
            sides: BTreeSet::from([Side::Base, Side::Head]),
            ..found(
                ImpactKind::Navigation,
                path,
                path,
                Side::Head,
                vec![path.to_path_buf()],
            )
        });
    }
    let attributes = |descriptor: &adoc_antora::ComponentDescriptor| {
        descriptor
            .attribute_entries
            .iter()
            .map(|(name, entry)| (name.clone(), entry.value.clone(), entry.soft))
            .collect::<Vec<_>>()
    };
    if before.asciidoc_attributes != after.asciidoc_attributes
        || attributes(before) != attributes(after)
    {
        gathered.gaps.push(Gap {
            reason: "attribute-consumers-not-analysed",
            detail: format!(
                "{} changes the version's AsciiDoc attributes; which pages read them is not \
                 traced",
                head.display(path)
            ),
            affected: vec![head.display(path)],
        });
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
                    .then(|| head.still_writes(&referrer.path, &referrer.written, referrer.ordinal))
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
    if is_nav {
        // The file changed, so the navigation it gives (order, hierarchy, labels) may have,
        // whatever its membership does.
        let mut sides = BTreeSet::new();
        if base.is_some_and(|base| base.is_listed_nav(path)) {
            sides.insert(Side::Base);
        }
        if head.is_listed_nav(path) {
            sides.insert(Side::Head);
        }
        gathered.add(Found {
            sides,
            ..found(
                ImpactKind::Navigation,
                path,
                path,
                Side::Head,
                vec![path.to_path_buf()],
            )
        });
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

/// What a publication selects on one side, worked out once for every affected page.
struct Selection<'s> {
    snapshot: &'s DependencySnapshot<'s>,
    /// Whether the side holds everything the selection names.
    known: bool,
    /// For a navigation profile, the pages its files list on this side.
    listed: BTreeSet<PathBuf>,
}

impl<'s> Selection<'s> {
    fn new(snapshot: &'s DependencySnapshot<'s>, selection: &PublicationSelection) -> Self {
        let antora = &snapshot.state().antora;
        let (known, listed) = match selection {
            PublicationSelection::Page(_) => (true, BTreeSet::new()),
            PublicationSelection::Component { name, version } => (
                match version {
                    Some(version) => antora.component(name, Some(version)).is_some(),
                    None => !antora.versions_of(name).is_empty(),
                },
                BTreeSet::new(),
            ),
            PublicationSelection::Navigation(files) => (
                files.iter().all(|file| snapshot.has(file)),
                files
                    .iter()
                    .flat_map(|file| snapshot.listed_by(file))
                    .collect(),
            ),
        };
        Self {
            snapshot,
            known,
            listed,
        }
    }

    /// Whether the selection holds `page` on this side; `None` when it names content the side
    /// does not hold, so it may.
    fn holds(&self, selection: &PublicationSelection, page: &Path) -> Option<bool> {
        match selection {
            PublicationSelection::Page(selected) => Some(selected == page),
            PublicationSelection::Component { name, version } => self.known.then(|| {
                self.snapshot
                    .component_version(page)
                    .is_some_and(|(component, page_version)| {
                        component == *name
                            && version
                                .as_ref()
                                .is_none_or(|version| page_version.as_ref() == Some(version))
                    })
            }),
            PublicationSelection::Navigation(files) => {
                // A changed navigation file of the profile is itself an impact on it.
                let listed = files.iter().any(|file| file == page) || self.listed.contains(page);
                if self.known || listed {
                    Some(listed)
                } else {
                    None
                }
            }
        }
    }
}

/// Each publication whose selection holds a page with a render or navigation impact, asked of
/// the side (or sides) whose evidence shows the impact.
fn publications_of(
    gathered: &mut Gathered,
    base: Option<&DependencySnapshot<'_>>,
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
        let selection = &publication.selection;
        let on_head = Selection::new(head, selection);
        let on_base = base.map(|base| Selection::new(base, selection));
        for (page, origin, sides) in &affected {
            let answers: Vec<Option<bool>> = sides
                .iter()
                .map(|side| match (side, &on_base) {
                    (Side::Base, Some(on_base)) => on_base.holds(selection, page),
                    _ => on_head.holds(selection, page),
                })
                .collect();
            let confidence = if answers.contains(&Some(true)) {
                Confidence::Confirmed
            } else if answers.contains(&None) {
                // The selection names content this revision does not hold: it may select it.
                Confidence::Potential
            } else {
                continue;
            };
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
                source: found.source.map(|source| SourceSpan {
                    path: display(&source.path),
                    range: source.range.into(),
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
        source: None,
        completeness,
        changes: reported,
        impacts,
        counts,
        truncated,
    }
}
