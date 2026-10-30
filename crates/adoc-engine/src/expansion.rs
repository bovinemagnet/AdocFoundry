//! The expansion of the Navigation and Modules trees. A branch opens at first as `[sidebar]`
//! says for its panel, and after that as the user left it: every branch seen is known to the live
//! view, so a rebuilt tree or a new default changes only branches that are new. The branches the
//! user (or a bulk operation) chose are remembered for the workspace in [`EXPANSION_STATE_FILE`],
//! beside the user settings file, when `remember_expansion` is on.
//!
//! The front end builds the trees and hands their branches, by key, to
//! [`Engine::resolve_expansion`]. The bulk operations, commands that need only the panel, act on
//! the tree resolved last and announce the result with [`EngineEvent::ExpansionChanged`].

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::{Duration, Instant},
};

use serde_json::{json, Map, Value};

use crate::{
    commands::{ids, register, spec},
    CommandArgs, CommandCategory, CommandContext, CommandId, CommandOutcome, CommandRegistry,
    Engine, EngineError, EngineEvent, ExpansionPolicy,
};

/// The file the remembered branches are kept in, beside the user settings file.
pub const EXPANSION_STATE_FILE: &str = "sidebar-state.json";

/// How long after the last change the remembered branches are written.
pub const EXPANSION_SAVE_SETTLE: Duration = Duration::from_millis(500);

const FORMAT_VERSION: u64 = 1;

/// A sidebar tree whose branches are remembered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SidebarPanel {
    Navigation,
    Modules,
}

impl SidebarPanel {
    fn index(self) -> usize {
        self as usize
    }

    fn name(self) -> &'static str {
        match self {
            Self::Navigation => "navigation",
            Self::Modules => "modules",
        }
    }
}

const PANELS: [SidebarPanel; 2] = [SidebarPanel::Navigation, SidebarPanel::Modules];

/// A branch of a sidebar tree, by a key that names the same logical branch however the tree is
/// rebuilt, with the branches beneath it. Leaves are left out: they neither open nor close.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Branch {
    pub key: String,
    pub children: Vec<Branch>,
}

#[derive(Clone, Debug, Default)]
struct Panel {
    /// Every branch seen, open or not.
    live: BTreeMap<String, bool>,
    /// The branches whose state was chosen rather than defaulted: what is remembered.
    chosen: BTreeSet<String>,
    /// The tree resolved last, which the bulk operations act on.
    tree: Vec<Branch>,
}

impl Panel {
    fn choose(&mut self, key: &str, expanded: bool) {
        self.live.insert(key.to_owned(), expanded);
        self.chosen.insert(key.to_owned());
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Expansion {
    /// The open workspace, by its roots.
    workspace: Option<String>,
    panels: [Panel; 2],
    save_due: Option<Instant>,
}

/// Calls `visit` with each branch of `tree` and its level, 1 for the top, depth first.
fn walk(tree: &[Branch], level: usize, visit: &mut impl FnMut(&Branch, usize)) {
    for branch in tree {
        visit(branch, level);
        walk(&branch.children, level + 1, visit);
    }
}

impl Engine {
    /// How far `panel` opens when first shown or reset, as the settings say.
    #[must_use]
    pub fn expansion_policy(&self, panel: SidebarPanel) -> ExpansionPolicy {
        match panel {
            SidebarPanel::Navigation => self.settings.sidebar.navigation,
            SidebarPanel::Modules => self.settings.sidebar.modules,
        }
    }

    /// Whether branch `key` of `panel` is open: false until it has been resolved.
    #[must_use]
    pub fn branch_expanded(&self, panel: SidebarPanel, key: &str) -> bool {
        self.expansion.panels[panel.index()]
            .live
            .get(key)
            .copied()
            .unwrap_or(false)
    }

    /// Takes `tree` as `panel`'s tree, giving every branch not seen before the state the panel's
    /// policy gives its level. Branches seen before keep theirs, and branches missing from `tree`
    /// are kept, in case they come back (a tree is empty while the workspace indexes).
    pub fn resolve_expansion(&mut self, panel: SidebarPanel, tree: &[Branch]) {
        let policy = self.expansion_policy(panel);
        let state = &mut self.expansion.panels[panel.index()];
        state.tree = tree.to_vec();
        let live = &mut state.live;
        walk(tree, 1, &mut |branch, level| {
            live.entry(branch.key.clone())
                .or_insert_with(|| policy.expands(level));
        });
    }

    /// Records that the user opened or closed branch `key` of `panel`.
    pub fn set_branch_expanded(&mut self, panel: SidebarPanel, key: &str, expanded: bool) {
        self.expansion.panels[panel.index()].choose(key, expanded);
        self.expansion_changed();
    }

    /// Opens each of `keys`, the branches above a row being revealed, that is closed, recording
    /// those it opens as the user's choices. False when all were open already.
    pub fn open_branches(&mut self, panel: SidebarPanel, keys: &[&str]) -> bool {
        let state = &mut self.expansion.panels[panel.index()];
        let mut opened = false;
        for key in keys {
            if !state.live.get(*key).copied().unwrap_or(false) {
                state.choose(key, true);
                opened = true;
            }
        }
        if opened {
            self.expansion_changed();
        }
        opened
    }

    /// Opens every branch of `panel`'s tree.
    pub fn expand_all(&mut self, panel: SidebarPanel) {
        self.choose_all(panel, true);
    }

    /// Closes every branch of `panel`'s tree, those hidden under closed branches too.
    pub fn collapse_all(&mut self, panel: SidebarPanel) {
        self.choose_all(panel, false);
    }

    fn choose_all(&mut self, panel: SidebarPanel, expanded: bool) {
        let state = &mut self.expansion.panels[panel.index()];
        let tree = std::mem::take(&mut state.tree);
        walk(&tree, 1, &mut |branch, _| {
            state.choose(&branch.key, expanded)
        });
        state.tree = tree;
        self.bulk_changed(panel);
    }

    /// The open branches of `panel`'s tree whose ancestors are all open, with their levels.
    fn visible_open(&self, panel: SidebarPanel) -> Vec<(String, usize)> {
        fn visit(
            live: &BTreeMap<String, bool>,
            tree: &[Branch],
            level: usize,
            out: &mut Vec<(String, usize)>,
        ) {
            for branch in tree {
                if live.get(&branch.key).copied().unwrap_or(false) {
                    out.push((branch.key.clone(), level));
                    visit(live, &branch.children, level + 1, out);
                }
            }
        }
        let state = &self.expansion.panels[panel.index()];
        let mut out = Vec::new();
        visit(&state.live, &state.tree, 1, &mut out);
        out
    }

    /// Whether Collapse One Level has anything to close.
    #[must_use]
    pub fn can_collapse_one_level(&self, panel: SidebarPanel) -> bool {
        !self.visible_open(panel).is_empty()
    }

    /// Closes the deepest level of open branches whose ancestors are open, leaving shallower
    /// branches, and those hidden under closed ones, as they are. False when nothing is open.
    pub fn collapse_one_level(&mut self, panel: SidebarPanel) -> bool {
        let open = self.visible_open(panel);
        let Some(deepest) = open.iter().map(|(_, level)| *level).max() else {
            return false;
        };
        let state = &mut self.expansion.panels[panel.index()];
        for (key, _) in open.iter().filter(|(_, level)| *level == deepest) {
            state.choose(key, false);
        }
        self.bulk_changed(panel);
        true
    }

    /// Forgets `panel`'s choices and opens its tree as the panel's policy says.
    pub fn reset_expansion(&mut self, panel: SidebarPanel) {
        let tree = std::mem::take(&mut self.expansion.panels[panel.index()].tree);
        self.expansion.panels[panel.index()] = Panel::default();
        self.resolve_expansion(panel, &tree);
        self.bulk_changed(panel);
    }

    fn bulk_changed(&mut self, panel: SidebarPanel) {
        self.expansion_changed();
        self.emit(EngineEvent::ExpansionChanged(panel));
    }

    fn expansion_changed(&mut self) {
        if self.settings.sidebar.remember_expansion {
            self.expansion.save_due = Some(self.clock.now() + EXPANSION_SAVE_SETTLE);
        }
    }

    fn expansion_state_file(&self) -> Option<PathBuf> {
        self.settings
            .user_file()
            .and_then(std::path::Path::parent)
            .map(|directory| directory.join(EXPANSION_STATE_FILE))
    }

    /// Starts the expansion of a newly opened workspace: its remembered branches, when
    /// remembering is on, after writing those of the workspace left.
    pub(crate) fn open_expansion(&mut self) {
        self.save_expansion();
        let workspace = self
            .roots
            .iter()
            .map(|root| root.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        self.expansion = Expansion {
            workspace: Some(workspace.clone()),
            ..Expansion::default()
        };
        if !self.settings.sidebar.remember_expansion {
            return;
        }
        let Some(file) = self.expansion_state_file() else {
            return;
        };
        let Some(entry) = read_state(&file).and_then(|mut state| state.remove(&workspace)) else {
            return;
        };
        for panel in PANELS {
            let Some(branches) = entry.get(panel.name()).and_then(Value::as_object) else {
                continue;
            };
            let state = &mut self.expansion.panels[panel.index()];
            for (key, expanded) in branches {
                if let Some(expanded) = expanded.as_bool() {
                    state.choose(key, expanded);
                }
            }
        }
    }

    /// Writes the remembered branches once changes have paused; called from `pump`.
    pub(crate) fn save_due_expansion(&mut self) -> usize {
        match self.expansion.save_due {
            Some(due) if self.clock.now() >= due => {
                self.save_expansion();
                1
            }
            _ => 0,
        }
    }

    /// Writes the open workspace's chosen branches into the state file, keeping the other
    /// workspaces'. A failure costs only the memory of the layout.
    fn save_expansion(&mut self) {
        if self.expansion.save_due.take().is_none() || !self.settings.sidebar.remember_expansion {
            return;
        }
        let (Some(file), Some(workspace)) = (
            self.expansion_state_file(),
            self.expansion.workspace.clone(),
        ) else {
            return;
        };
        let mut workspaces = read_state(&file).unwrap_or_default();
        let mut entry = Map::new();
        for panel in PANELS {
            let state = &self.expansion.panels[panel.index()];
            let branches: Map<String, Value> = state
                .chosen
                .iter()
                .filter_map(|key| Some((key.clone(), Value::Bool(*state.live.get(key)?))))
                .collect();
            entry.insert(panel.name().to_owned(), Value::Object(branches));
        }
        workspaces.insert(workspace, Value::Object(entry));
        let text = json!({ "version": FORMAT_VERSION, "workspaces": workspaces }).to_string();
        let _ = crate::engine::write_replacing(&file, &text);
    }
}

/// The workspaces of the state file, or `None` when it is missing, unreadable or of another
/// format.
fn read_state(file: &std::path::Path) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(file).ok()?;
    let mut state: Value = serde_json::from_str(&text).ok()?;
    if state.get("version").and_then(Value::as_u64) != Some(FORMAT_VERSION) {
        return None;
    }
    match state.get_mut("workspaces")?.take() {
        Value::Object(workspaces) => Some(workspaces),
        _ => None,
    }
}

fn panel_of(command: CommandId, args: &CommandArgs) -> Result<SidebarPanel, EngineError> {
    match args {
        CommandArgs::Panel(panel) => Ok(*panel),
        _ => Err(EngineError::InvalidArguments {
            command,
            expected: "a sidebar panel",
        }),
    }
}

pub(crate) fn register_expansion(registry: &mut CommandRegistry) {
    use CommandCategory::View;
    use CommandContext::Always;
    type Run = fn(&mut Engine, CommandArgs) -> Result<CommandOutcome, EngineError>;
    let commands: [(crate::CommandSpec, Run); 4] = [
        (
            spec(
                ids::SIDEBAR_EXPAND_ALL,
                "Expand All in Sidebar",
                View,
                None,
                Always,
            ),
            |engine, args| {
                engine.expand_all(panel_of(ids::SIDEBAR_EXPAND_ALL, &args)?);
                Ok(CommandOutcome::Done)
            },
        ),
        (
            spec(
                ids::SIDEBAR_COLLAPSE_ALL,
                "Collapse All in Sidebar",
                View,
                None,
                Always,
            ),
            |engine, args| {
                engine.collapse_all(panel_of(ids::SIDEBAR_COLLAPSE_ALL, &args)?);
                Ok(CommandOutcome::Done)
            },
        ),
        (
            spec(
                ids::SIDEBAR_COLLAPSE_ONE_LEVEL,
                "Collapse Sidebar One Level",
                View,
                None,
                Always,
            ),
            |engine, args| {
                engine.collapse_one_level(panel_of(ids::SIDEBAR_COLLAPSE_ONE_LEVEL, &args)?);
                Ok(CommandOutcome::Done)
            },
        ),
        (
            spec(
                ids::SIDEBAR_RESET_EXPANSION,
                "Reset Sidebar to Configured Default",
                View,
                None,
                Always,
            ),
            |engine, args| {
                engine.reset_expansion(panel_of(ids::SIDEBAR_RESET_EXPANSION, &args)?);
                Ok(CommandOutcome::Done)
            },
        ),
    ];
    for (spec, run) in commands {
        register(registry, spec, run);
    }
}

impl Drop for Engine {
    /// Writes remembered branches still waiting for their pause, so quitting loses none.
    fn drop(&mut self) {
        self.save_expansion();
    }
}
