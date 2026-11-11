//! Tools that wait for a job of their own wait for it alone (#425): a site build running in
//! the background holds none of them up.

#![cfg(unix)]

mod common;

use std::{
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt as _,
    time::{Duration, Instant},
};

use common::Client;
use serde_json::json;

#[test]
fn mcp_search_answers_while_a_site_build_runs() {
    let directory = tempfile::tempdir().unwrap();
    let root = adoc_engine::normalize_path(&directory.path().join("demo"));
    let pages = root.join("docs/modules/ROOT/pages");
    fs::create_dir_all(&pages).unwrap();
    fs::write(
        root.join("antora-playbook.yml"),
        "site:\n  title: Demo\ncontent:\n  sources:\n  - url: .\n    start_path: docs\n",
    )
    .unwrap();
    fs::write(root.join("docs/antora.yml"), "name: demo\nversion: ~\n").unwrap();
    fs::write(
        pages.join("index.adoc"),
        "= Demo\n\nA needle in the text.\n",
    )
    .unwrap();

    // A stand-in for npm, found on the `PATH`, that runs until `hold` is removed.
    let tools = directory.path().join("tools");
    fs::create_dir(&tools).unwrap();
    let hold = directory.path().join("hold");
    fs::write(&hold, "").unwrap();
    let npm = tools.join("npm");
    fs::write(
        &npm,
        format!(
            "#!/bin/sh\necho \"npm $*\"\nwhile [ -f '{}' ]; do sleep 0.1; done\n",
            hold.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&npm, fs::Permissions::from_mode(0o755)).unwrap();
    let mut path = OsString::from(&tools);
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    // No user settings file, so no `[tools] node` of the developer's is used.
    let home = directory.path().join("home");
    fs::create_dir(&home).unwrap();
    let env = [
        ("PATH", Some(path.as_os_str())),
        ("HOME", Some(home.as_os_str())),
        ("XDG_CONFIG_HOME", None),
    ];
    let (mut client, _) = Client::start_with_env(&root, &["--allow-build"], "2025-06-18", &env);

    let running = client
        .call("build_site", json!({ "wait_seconds": 0 }))
        .unwrap();
    assert_eq!(running["outcome"], "running", "{running}");

    let asked = Instant::now();
    let found = client.call("search", json!({ "query": "needle" })).unwrap();
    let took = asked.elapsed();
    assert_eq!(found["matches"].as_array().unwrap().len(), 1, "{found}");
    assert!(took < Duration::from_secs(5), "answered after {took:?}");

    fs::remove_file(&hold).unwrap();
    client.stop();
}
