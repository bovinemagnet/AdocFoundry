//! The app's impact inspector (Impact PRD §§4 and 9, #311): one kept analysis, worked out on a
//! worker from a copy of the live workspace, never on the front end's thread or while it draws.
//! `impact.analyse_changes` compares `HEAD` with the working tree and its unsaved buffers,
//! `impact.compare_revisions` two refs, and `impact.inspect` a page or partial as if it were
//! removed (#100: what references it, includes it, lists it and publishes it, before it is
//! moved or deleted). A result is kept with the workspace revision it was asked at, so a front
//! end can say it is out of date; a newer request supersedes one still running. Nothing here
//! changes a file.
//!
//! [`Engine::impact_trail`] turns an impact's dependency path into graph steps for the graph's
//! path display, the links between each pair asked of the graph, never searched for: it is the
//! path the analysis followed, not a shortest path.

use std::{path::PathBuf, sync::Arc};

use adoc_graph::{GraphKey, PathStep};
use adoc_impact::{Impact, ImpactReport, Limits};

use crate::{
    commands::{ids, register, spec},
    impact_analysis::{compute_owned, OwnedInputs},
    workspace::WorkerMessage,
    ChangeSource, CommandArgs, CommandCategory, CommandContext, CommandOutcome, CommandRegistry,
    Engine, EngineError, EngineEvent, NavigationPosition,
};

/// The kept analysis and what it was asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct ImpactInspection {
    pub source: ChangeSource,
    pub state: InspectionState,
    /// The workspace revision it was asked at.
    revision: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InspectionState {
    Running,
    Ready(Box<ImpactReport>),
    /// Why no analysis could be made: no repository, an unknown ref, a path outside the roots.
    Failed(String),
}

pub(crate) fn register_impact_inspection(registry: &mut CommandRegistry) {
    register(
        registry,
        spec(
            ids::IMPACT_ANALYSE_CHANGES,
            "Analyse Impact of Changes",
            CommandCategory::Workspace,
            None,
            CommandContext::Antora,
        ),
        |engine, _| {
            engine.inspect_impact(ChangeSource::WorkingTree);
            Ok(CommandOutcome::Done)
        },
    );
    register(
        registry,
        spec(
            ids::IMPACT_COMPARE_REVISIONS,
            "Compare Revisions…",
            CommandCategory::Workspace,
            None,
            CommandContext::Antora,
        ),
        |engine, args| {
            let CommandArgs::Revisions { base, head } = args else {
                return Err(EngineError::InvalidArguments {
                    command: ids::IMPACT_COMPARE_REVISIONS,
                    expected: "a base and a head revision",
                });
            };
            engine.inspect_impact(ChangeSource::GitRefs { base, head });
            Ok(CommandOutcome::Done)
        },
    );
    register(
        registry,
        spec(
            ids::IMPACT_INSPECT,
            "Inspect Impact",
            CommandCategory::Workspace,
            None,
            CommandContext::Antora,
        ),
        |engine, args| {
            let path = match args {
                CommandArgs::Path(path) => path,
                CommandArgs::Document(id) => engine
                    .document(id)
                    .ok_or(EngineError::UnknownDocument(id))?
                    .path
                    .clone(),
                CommandArgs::None => engine
                    .active()
                    .and_then(|id| engine.document(id))
                    .map(|document| document.path.clone())
                    .ok_or_else(|| {
                        EngineError::NotFound("no page is open to inspect".to_owned())
                    })?,
                _ => {
                    return Err(EngineError::InvalidArguments {
                        command: ids::IMPACT_INSPECT,
                        expected: "a file, a document or nothing (the active page)",
                    })
                }
            };
            engine.inspect_impact(ChangeSource::Removal(vec![path]));
            Ok(CommandOutcome::Done)
        },
    );
}

impl Engine {
    /// The kept analysis, running or finished.
    #[must_use]
    pub fn impact_inspection(&self) -> Option<&ImpactInspection> {
        self.impact_inspection.as_ref()
    }

    /// Whether the kept analysis was made before the workspace last changed (an edit, a file
    /// changed on disk, a new catalog), so it may no longer hold.
    #[must_use]
    pub fn impact_stale(&self) -> bool {
        self.impact_inspection.as_ref().is_some_and(|inspection| {
            matches!(inspection.state, InspectionState::Ready(_))
                && inspection.revision != self.workspace_revision()
        })
    }

    /// Analyses `source` on a worker, keeping the result for [`Engine::impact_inspection`];
    /// any analysis still running is superseded.
    pub fn inspect_impact(&mut self, source: ChangeSource) {
        self.impact_generation += 1;
        let generation = self.impact_generation;
        let inputs = OwnedInputs {
            source: source.clone(),
            roots: self.roots().to_vec(),
            publications: self.impact_publications(),
            limits: Limits::default(),
            live: Arc::new(self.workspace.clone()),
            open: self.open_documents(),
            ignore: self.settings.workspace.ignore.clone(),
            cached: self.impact_cached(),
        };
        self.impact_inspection = Some(ImpactInspection {
            source,
            state: InspectionState::Running,
            revision: self.workspace_revision(),
        });
        self.emit(EngineEvent::ImpactChanged);
        let sender = self.worker_sender.clone();
        self.workers_running += 1;
        let crashed = move |error| WorkerMessage::ImpactInspected {
            generation,
            result: Err(format!("the impact analysis crashed: {error}")),
            built: Vec::new(),
        };
        crate::worker::spawn(sender, crashed, move |sender| {
            let (result, built) = compute_owned(&inputs);
            let _ = sender.send(WorkerMessage::ImpactInspected {
                generation,
                result: result.map(Box::new).map_err(|error| error.to_string()),
                built,
            });
        });
    }

    pub(crate) fn impact_inspected(
        &mut self,
        generation: u64,
        result: Result<Box<ImpactReport>, String>,
        built: Vec<(String, Arc<adoc_lang::WorkspaceState>)>,
    ) {
        self.remember_snapshots(built);
        if generation != self.impact_generation {
            return;
        }
        if let Some(inspection) = &mut self.impact_inspection {
            inspection.state = match result {
                Ok(report) => InspectionState::Ready(report),
                Err(error) => InspectionState::Failed(error),
            };
            self.emit(EngineEvent::ImpactChanged);
        }
    }

    /// The file a report's root-relative `path` names: under the root that holds it, else the
    /// first.
    #[must_use]
    pub fn impact_path(&self, path: &str) -> Option<PathBuf> {
        let candidates: Vec<PathBuf> = self
            .roots()
            .iter()
            .map(|root| adoc_index::normalize_path(&root.join(path)))
            .collect();
        candidates
            .iter()
            .find(|candidate| {
                self.workspace.index.file(candidate).is_some()
                    || self.workspace.antora.resource_for_path(candidate).is_some()
                    || candidate.exists()
            })
            .or(candidates.first())
            .cloned()
    }

    /// Where to open an impact: the reference or entry that links it, else the affected file.
    #[must_use]
    pub fn impact_place(&self, impact: &Impact) -> Option<NavigationPosition> {
        match &impact.source {
            Some(source) => Some(NavigationPosition {
                path: self.impact_path(&source.path)?,
                offset: source.range.start,
            }),
            None => Some(NavigationPosition {
                path: self.impact_path(&impact.path)?,
                offset: 0,
            }),
        }
    }

    /// An impact's dependency path as graph steps, from the changed file to the affected one:
    /// each step with the links between it and the one before, in the graph's direction
    /// (`backwards` when the link runs from it to the one before, as an include from a page to
    /// its partial does). A pair the live graph has no link between (base-side evidence) has
    /// none.
    #[must_use]
    pub fn impact_trail(&self, impact: &Impact) -> Vec<PathStep> {
        let keys: Vec<GraphKey> = impact
            .via
            .iter()
            .filter_map(|path| self.impact_path(path))
            .map(|path| GraphKey::File(Arc::from(path)))
            .collect();
        let store = self.graph.store();
        let kinds = |from: &GraphKey, to: &GraphKey| {
            let mut kinds: Vec<_> = store
                .links_from(from)
                .filter(|link| link.to == *to)
                .map(|link| link.kind)
                .collect();
            kinds.sort();
            kinds.dedup();
            kinds
        };
        let mut steps: Vec<PathStep> = Vec::new();
        for (number, key) in keys.iter().enumerate() {
            let Some(before) = number.checked_sub(1).map(|before| &keys[before]) else {
                steps.push(PathStep {
                    key: key.clone(),
                    kinds: Vec::new(),
                    backwards: false,
                });
                continue;
            };
            let forward = kinds(before, key);
            let (kinds, backwards) = if forward.is_empty() {
                let back = kinds(key, before);
                let backwards = !back.is_empty();
                (back, backwards)
            } else {
                (forward, false)
            };
            steps.push(PathStep {
                key: key.clone(),
                kinds,
                backwards,
            });
        }
        steps
    }
}
