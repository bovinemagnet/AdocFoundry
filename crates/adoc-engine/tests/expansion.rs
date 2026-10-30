//! The Navigation and Modules trees' expansion: the configured depth, the branches the user opens
//! and closes, the bulk operations, and remembering them for the workspace.

use std::{fs, path::Path, sync::Arc, time::Duration};

use adoc_engine::{
    Branch, Engine, ExpansionPolicy, ManualClock, Settings, SidebarPanel, EXPANSION_SAVE_SETTLE,
};

use SidebarPanel::{Modules, Navigation};

fn branch(key: &str, children: Vec<Branch>) -> Branch {
    Branch {
        key: key.to_owned(),
        children,
    }
}

/// Component → module → family, as the Modules panel shows them, with an uneven second module.
fn modules_tree() -> Vec<Branch> {
    vec![branch(
        "c@1",
        vec![
            branch(
                "c@1/a",
                vec![branch("c@1/a/page", vec![]), branch("c@1/a/image", vec![])],
            ),
            branch("c@1/b", vec![]),
        ],
    )]
}

fn expanded(engine: &Engine, panel: SidebarPanel, tree: &[Branch]) -> Vec<String> {
    fn walk(engine: &Engine, panel: SidebarPanel, tree: &[Branch], out: &mut Vec<String>) {
        for branch in tree {
            if engine.branch_expanded(panel, &branch.key) {
                out.push(branch.key.clone());
            }
            walk(engine, panel, &branch.children, out);
        }
    }
    let mut out = Vec::new();
    walk(engine, panel, tree, &mut out);
    out
}

fn engine_with(policy: ExpansionPolicy) -> Engine {
    let mut settings = Settings::default();
    settings.sidebar.navigation = policy;
    settings.sidebar.modules = policy;
    Engine::new(settings)
}

#[test]
fn depth_counts_branch_levels_from_the_component() {
    let tree = modules_tree();
    let cases: [(ExpansionPolicy, &[&str]); 5] = [
        (ExpansionPolicy::Depth(0), &[]),
        (ExpansionPolicy::Depth(1), &["c@1"]),
        (ExpansionPolicy::Depth(2), &["c@1", "c@1/a", "c@1/b"]),
        (
            ExpansionPolicy::Depth(3),
            &["c@1", "c@1/a", "c@1/a/page", "c@1/a/image", "c@1/b"],
        ),
        (
            ExpansionPolicy::All,
            &["c@1", "c@1/a", "c@1/a/page", "c@1/a/image", "c@1/b"],
        ),
    ];
    for (policy, want) in cases {
        let mut engine = engine_with(policy);
        engine.resolve_expansion(Modules, &tree);
        assert_eq!(expanded(&engine, Modules, &tree), want, "{policy:?}");
    }
    // Deeper than the tree: every branch, and no more work than the tree.
    let mut engine = engine_with(ExpansionPolicy::Depth(u32::MAX));
    engine.resolve_expansion(Modules, &tree);
    assert_eq!(expanded(&engine, Modules, &tree).len(), 5);
}

#[test]
fn choices_survive_a_rebuilt_tree() {
    let mut engine = engine_with(ExpansionPolicy::All);
    let tree = modules_tree();
    engine.resolve_expansion(Modules, &tree);
    engine.set_branch_expanded(Modules, "c@1/a", false);

    // Rebuilt with a sibling inserted before, and a new branch under the collapsed one.
    let mut rebuilt = modules_tree();
    rebuilt[0].children.insert(0, branch("c@1/0", vec![]));
    rebuilt[0].children[1]
        .children
        .push(branch("c@1/a/attachment", vec![]));
    engine.resolve_expansion(Modules, &rebuilt);
    assert!(!engine.branch_expanded(Modules, "c@1/a"), "kept collapsed");
    assert!(engine.branch_expanded(Modules, "c@1/0"), "new: the default");
    assert!(
        engine.branch_expanded(Modules, "c@1/a/attachment"),
        "new under a collapsed branch: its own default, the parent stays shut"
    );

    // A tree empty while loading forgets nothing.
    engine.resolve_expansion(Modules, &[]);
    engine.resolve_expansion(Modules, &tree);
    assert!(!engine.branch_expanded(Modules, "c@1/a"));
}

#[test]
fn panels_and_component_versions_keep_their_own_state() {
    let mut engine = engine_with(ExpansionPolicy::Depth(1));
    let tree = vec![branch("c@1", vec![]), branch("c@2", vec![])];
    engine.resolve_expansion(Navigation, &tree);
    engine.resolve_expansion(Modules, &tree);
    engine.set_branch_expanded(Navigation, "c@2", false);
    assert!(engine.branch_expanded(Navigation, "c@1"));
    assert!(!engine.branch_expanded(Navigation, "c@2"));
    assert!(engine.branch_expanded(Modules, "c@2"));
}

#[test]
fn a_new_default_applies_to_new_branches_and_a_reset() {
    let directory = tempfile::tempdir().unwrap();
    let user = directory.path().join("settings.toml");
    fs::write(&user, "[sidebar.modules]\ndefault_expansion = 1\n").unwrap();
    let mut engine = Engine::new(Settings::load(Some(&user), None).unwrap());
    let tree = modules_tree();
    engine.resolve_expansion(Modules, &tree);
    assert_eq!(expanded(&engine, Modules, &tree), ["c@1"]);

    fs::write(&user, "[sidebar.modules]\ndefault_expansion = \"all\"\n").unwrap();
    engine.reload_settings().unwrap();
    assert_eq!(engine.expansion_policy(Modules), ExpansionPolicy::All);
    let mut grown = modules_tree();
    grown.push(branch("d@1", vec![branch("d@1/x", vec![])]));
    engine.resolve_expansion(Modules, &grown);
    assert_eq!(
        expanded(&engine, Modules, &grown),
        ["c@1", "d@1", "d@1/x"],
        "the live view stays; new branches follow the new default"
    );
    engine.reset_expansion(Modules);
    assert_eq!(expanded(&engine, Modules, &grown).len(), 7);
}

#[test]
fn bulk_operations_act_on_one_panel() {
    let mut engine = engine_with(ExpansionPolicy::Depth(0));
    let tree = modules_tree();
    engine.resolve_expansion(Modules, &tree);
    engine.resolve_expansion(Navigation, &tree);
    engine.expand_all(Modules);
    assert_eq!(expanded(&engine, Modules, &tree).len(), 5);
    assert!(expanded(&engine, Navigation, &tree).is_empty());

    // An ordinary collapse keeps what is beneath; Collapse All closes that too.
    engine.set_branch_expanded(Modules, "c@1", false);
    assert!(engine.branch_expanded(Modules, "c@1/a"));
    engine.collapse_all(Modules);
    assert!(expanded(&engine, Modules, &tree).is_empty());
}

#[test]
fn collapse_one_level_closes_the_deepest_open_level() {
    let mut engine = engine_with(ExpansionPolicy::All);
    // c@1/a goes two levels deeper than c@1/b.
    let tree = vec![branch(
        "c@1",
        vec![
            branch(
                "c@1/a",
                vec![branch("c@1/a/x", vec![branch("c@1/a/x/y", vec![])])],
            ),
            branch("c@1/b", vec![branch("c@1/b/z", vec![])]),
        ],
    )];
    engine.resolve_expansion(Navigation, &tree);
    let mut steps = Vec::new();
    while engine.can_collapse_one_level(Navigation) {
        assert!(engine.collapse_one_level(Navigation));
        steps.push(expanded(&engine, Navigation, &tree));
    }
    assert_eq!(
        steps,
        [
            vec!["c@1", "c@1/a", "c@1/a/x", "c@1/b", "c@1/b/z"],
            vec!["c@1", "c@1/a", "c@1/b"],
            vec!["c@1"],
            vec![],
        ]
    );
    assert!(!engine.collapse_one_level(Navigation));

    // Branches hidden under a closed parent are not "visible": only the open level goes.
    engine.expand_all(Navigation);
    engine.set_branch_expanded(Navigation, "c@1/a", false);
    engine.collapse_one_level(Navigation);
    assert_eq!(
        expanded(&engine, Navigation, &tree),
        ["c@1", "c@1/a/x", "c@1/a/x/y", "c@1/b"],
        "c@1/b/z, the deepest visible open branch, closed; c@1/a's hidden ones kept"
    );
}

/// An engine whose user settings file, and so its state, are in `directory`.
fn remembering(directory: &Path, remember: bool, clock: &ManualClock) -> Engine {
    let user = directory.join("settings.toml");
    fs::write(
        &user,
        format!("[sidebar]\nremember_expansion = {remember}\n"),
    )
    .unwrap();
    Engine::with_clock(
        Settings::load(Some(&user), None).unwrap(),
        Arc::new(clock.clone()),
    )
}

fn workspace(directory: &Path, name: &str) -> std::path::PathBuf {
    let root = directory.join(name);
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn choices_are_remembered_for_the_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let docs = workspace(directory.path(), "docs");
    let other = workspace(directory.path(), "other");
    let tree = modules_tree();
    let clock = ManualClock::new();

    let mut engine = remembering(directory.path(), true, &clock);
    engine.open_workspace(vec![docs.clone()]).unwrap();
    engine.resolve_expansion(Modules, &tree);
    engine.set_branch_expanded(Modules, "c@1/a", false);
    engine.set_branch_expanded(Navigation, "c@1", false);
    engine.pump();
    clock.advance(EXPANSION_SAVE_SETTLE + Duration::from_millis(1));
    engine.pump();
    drop(engine);
    assert!(
        fs::read_dir(&docs).unwrap().next().is_none(),
        "nothing is written into the workspace"
    );

    let mut engine = remembering(directory.path(), true, &clock);
    engine.open_workspace(vec![docs.clone()]).unwrap();
    engine.resolve_expansion(Modules, &tree);
    engine.resolve_expansion(Navigation, &tree);
    assert!(!engine.branch_expanded(Modules, "c@1/a"), "restored");
    assert!(
        engine.branch_expanded(Modules, "c@1/b"),
        "untouched: the default"
    );
    assert!(!engine.branch_expanded(Navigation, "c@1"));

    // Another workspace has its own.
    engine.open_workspace(vec![other]).unwrap();
    engine.resolve_expansion(Modules, &tree);
    assert!(engine.branch_expanded(Modules, "c@1/a"));
}

#[test]
fn switching_workspaces_saves_the_one_left() {
    let directory = tempfile::tempdir().unwrap();
    let docs = workspace(directory.path(), "docs");
    let other = workspace(directory.path(), "other");
    let clock = ManualClock::new();
    let mut engine = remembering(directory.path(), true, &clock);
    engine.open_workspace(vec![docs.clone()]).unwrap();
    engine.resolve_expansion(Modules, &modules_tree());
    engine.set_branch_expanded(Modules, "c@1/a", false);
    // Before the pause is over.
    engine.open_workspace(vec![other]).unwrap();
    engine.open_workspace(vec![docs]).unwrap();
    engine.resolve_expansion(Modules, &modules_tree());
    assert!(!engine.branch_expanded(Modules, "c@1/a"));
}

#[test]
fn without_remembering_reopening_starts_from_the_default() {
    let directory = tempfile::tempdir().unwrap();
    let docs = workspace(directory.path(), "docs");
    let tree = modules_tree();
    let clock = ManualClock::new();
    let save = |engine: &mut Engine| {
        engine.pump();
        clock.advance(EXPANSION_SAVE_SETTLE + Duration::from_millis(1));
        engine.pump();
    };

    // Remembered once…
    let mut engine = remembering(directory.path(), true, &clock);
    engine.open_workspace(vec![docs.clone()]).unwrap();
    engine.resolve_expansion(Modules, &tree);
    engine.set_branch_expanded(Modules, "c@1/a", false);
    save(&mut engine);

    // …then ignored, and the session's own choices are not written.
    let mut engine = remembering(directory.path(), false, &clock);
    engine.open_workspace(vec![docs.clone()]).unwrap();
    engine.resolve_expansion(Modules, &tree);
    assert!(engine.branch_expanded(Modules, "c@1/a"), "the default");
    engine.set_branch_expanded(Modules, "c@1/b", false);
    engine.resolve_expansion(Modules, &tree);
    assert!(
        !engine.branch_expanded(Modules, "c@1/b"),
        "kept for the session"
    );
    save(&mut engine);

    let mut engine = remembering(directory.path(), true, &clock);
    engine.open_workspace(vec![docs]).unwrap();
    engine.resolve_expansion(Modules, &tree);
    assert!(!engine.branch_expanded(Modules, "c@1/a"), "the old memory");
    assert!(engine.branch_expanded(Modules, "c@1/b"), "not remembered");
}

#[test]
fn unreadable_state_falls_back_to_the_default() {
    let directory = tempfile::tempdir().unwrap();
    let docs = workspace(directory.path(), "docs");
    let clock = ManualClock::new();
    for corrupt in [
        "not json",
        "{\"version\": 99}",
        "[]",
        "{\"version\":1,\"workspaces\":[]}",
    ] {
        fs::write(
            directory.path().join(adoc_engine::EXPANSION_STATE_FILE),
            corrupt,
        )
        .unwrap();
        let mut engine = remembering(directory.path(), true, &clock);
        engine.open_workspace(vec![docs.clone()]).unwrap();
        engine.resolve_expansion(Modules, &modules_tree());
        assert!(engine.branch_expanded(Modules, "c@1/a"), "{corrupt}");
        // And saving over it works.
        engine.set_branch_expanded(Modules, "c@1/a", false);
        engine.pump();
        clock.advance(EXPANSION_SAVE_SETTLE + Duration::from_millis(1));
        engine.pump();
        let saved =
            fs::read_to_string(directory.path().join(adoc_engine::EXPANSION_STATE_FILE)).unwrap();
        assert!(saved.contains("c@1/a"), "{corrupt}: {saved}");
    }
}

#[test]
fn the_bulk_operations_are_commands() {
    use adoc_engine::{commands::ids, CommandArgs, EngineEvent};
    let mut engine = engine_with(ExpansionPolicy::Depth(0));
    let tree = modules_tree();
    engine.resolve_expansion(Modules, &tree);
    let events = engine.subscribe();
    engine
        .invoke(ids::SIDEBAR_EXPAND_ALL, CommandArgs::Panel(Modules))
        .unwrap();
    assert_eq!(expanded(&engine, Modules, &tree).len(), 5);
    assert!(events
        .try_iter()
        .any(|event| event == EngineEvent::ExpansionChanged(Modules)));
    engine
        .invoke(ids::SIDEBAR_COLLAPSE_ONE_LEVEL, CommandArgs::Panel(Modules))
        .unwrap();
    assert_eq!(expanded(&engine, Modules, &tree), ["c@1", "c@1/a", "c@1/b"]);
    engine
        .invoke(ids::SIDEBAR_COLLAPSE_ALL, CommandArgs::Panel(Modules))
        .unwrap();
    assert!(expanded(&engine, Modules, &tree).is_empty());
    engine
        .invoke(ids::SIDEBAR_RESET_EXPANSION, CommandArgs::Panel(Modules))
        .unwrap();
    assert!(
        expanded(&engine, Modules, &tree).is_empty(),
        "depth 0 again"
    );
    assert!(engine
        .invoke(ids::SIDEBAR_EXPAND_ALL, CommandArgs::None)
        .is_err());
}

#[test]
fn closing_the_engine_saves_what_is_pending() {
    let directory = tempfile::tempdir().unwrap();
    let docs = workspace(directory.path(), "docs");
    let clock = ManualClock::new();
    let mut engine = remembering(directory.path(), true, &clock);
    engine.open_workspace(vec![docs.clone()]).unwrap();
    engine.resolve_expansion(Modules, &modules_tree());
    engine.set_branch_expanded(Modules, "c@1/a", false);
    // Quit before the pause is over.
    drop(engine);
    let mut engine = remembering(directory.path(), true, &clock);
    engine.open_workspace(vec![docs]).unwrap();
    engine.resolve_expansion(Modules, &modules_tree());
    assert!(!engine.branch_expanded(Modules, "c@1/a"));
}

#[test]
fn opening_the_way_to_a_row_records_only_the_branches_it_opens() {
    let directory = tempfile::tempdir().unwrap();
    let docs = workspace(directory.path(), "docs");
    let tree = modules_tree();
    let clock = ManualClock::new();

    let mut engine = remembering(directory.path(), true, &clock);
    engine.open_workspace(vec![docs.clone()]).unwrap();
    engine.resolve_expansion(Modules, &tree);
    engine.set_branch_expanded(Modules, "c@1", false);
    assert!(engine.open_branches(Modules, &["c@1", "c@1/a", "c@1/a/page"]));
    assert_eq!(
        expanded(&engine, Modules, &tree),
        ["c@1", "c@1/a", "c@1/a/page", "c@1/b"]
    );
    assert!(
        !engine.open_branches(Modules, &["c@1", "c@1/a"]),
        "nothing left to open"
    );
    engine.pump();
    clock.advance(EXPANSION_SAVE_SETTLE + Duration::from_millis(1));
    engine.pump();
    drop(engine);

    // Under a shallower default, the branches it opened are remembered; c@1/a, open already,
    // was left to the default.
    let user = directory.path().join("settings.toml");
    fs::write(
        &user,
        "[sidebar]\nremember_expansion = true\n\n[sidebar.modules]\ndefault_expansion = 0\n",
    )
    .unwrap();
    let settings = Settings::load(Some(&user), None).unwrap();
    let mut engine = Engine::with_clock(settings, Arc::new(clock.clone()));
    engine.open_workspace(vec![docs]).unwrap();
    engine.resolve_expansion(Modules, &tree);
    assert_eq!(expanded(&engine, Modules, &tree), ["c@1", "c@1/a/page"]);
}
