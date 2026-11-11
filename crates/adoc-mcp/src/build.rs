//! `build_site` (#334, MCP PRD §7.7): the app's Build Site, the engine's `project.build_site`,
//! for an agent, listed and callable only with `--allow-build`, which `--allow-write` never
//! implies. The engine runs the build on a worker, bounded by `[tools] build_timeout` and its
//! output cap, one build at a time; the tool waits for it without holding the engine, so other
//! requests and sessions are answered meanwhile, and answers `running` when its own wait ends
//! first. Calling it again while a build runs waits for that build. There is no sandbox: npm
//! and Antora run with the user's rights, outside the server's root confinement, so
//! `--allow-build` is for trusted projects only.

use std::time::{Duration, Instant};

use adoc_engine::{commands::ids, CommandArgs, SiteBuildBlocker, SiteBuildState};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    responses::{answer, ErrorCode, Failure},
    schemas,
    server::{Answer, Server, State},
};

const SCHEMA_VERSION: u32 = 1;

/// How long the tool waits for a build unless asked otherwise, and at most.
const DEFAULT_WAIT: u64 = 300;
const MAX_WAIT: u64 = 3600;

/// How often a waiting call looks at the build.
const POLL: Duration = Duration::from_millis(100);

/// At most this many bytes of the log's end are returned.
const LOG_BYTES: usize = 64 * 1024;

#[derive(Deserialize, JsonSchema)]
pub(crate) struct BuildSiteArgs {
    /// Seconds to wait for the build to finish before answering `running`: 300 by default, at
    /// most 3600. Call again to go on waiting for the same build.
    wait_seconds: Option<u64>,
}

#[tool_router(router = build_router, vis = "pub(crate)")]
impl Server {
    #[tool(
        description = "Build the Antora site from the workspace's playbook, as the app's Build Site does: in the playbook's folder, `npm install` while `node_modules` is missing, then the `package.json` `build` script, else Antora itself, installing nothing. Waits for the build (`wait_seconds`) and answers with its outcome, the end of its log and the output paths; a build already running is waited for, not started again. It runs the project's own code with the user's rights and network access, outside the server's root confinement: offered only with --allow-build. No Node.js is `unavailable`, never installed.",
        output_schema = schemas::build_site(),
        annotations(read_only_hint = false, destructive_hint = false, idempotent_hint = false, open_world_hint = true)
    )]
    pub(crate) async fn build_site(&self, Parameters(args): Parameters<BuildSiteArgs>) -> Answer {
        let wait = Duration::from_secs(args.wait_seconds.unwrap_or(DEFAULT_WAIT).min(MAX_WAIT));
        let started = match self.blocking(start).await.and_then(|started| started) {
            Ok(started) => started,
            Err(failure) => return Ok(answer(Err(failure))),
        };
        let deadline = Instant::now() + wait;
        loop {
            let (report, finished) = match self.blocking(move |state| report(state, started)).await
            {
                Ok(Ok(report)) => report,
                Ok(Err(failure)) | Err(failure) => return Ok(answer(Err(failure))),
            };
            if finished || Instant::now() >= deadline {
                return Ok(answer(Ok(report)));
            }
            tokio::time::sleep(POLL).await;
        }
    }
}

/// Starts a build, or finds one running (`false`); refused without a playbook or Node.js.
fn start(state: &mut State) -> Result<bool, Failure> {
    match state.engine.site_build_blocker() {
        Some(SiteBuildBlocker::Running) => return Ok(false),
        Some(blocker @ SiteBuildBlocker::NoPlaybook) => {
            return Err(Failure::new(ErrorCode::NotFound, blocker.to_string()));
        }
        Some(blocker @ SiteBuildBlocker::NoNode { .. }) => {
            return Err(Failure::new(ErrorCode::Unavailable, blocker.to_string()));
        }
        None => {}
    }
    let playbook = state
        .engine
        .workspace()
        .antora
        .playbook()
        .map(|playbook| playbook.path.clone())
        .ok_or_else(|| Failure::new(ErrorCode::NotFound, "the workspace has no playbook"))?;
    state.confine(&playbook.to_string_lossy())?;
    state
        .engine
        .invoke(ids::PROJECT_BUILD_SITE, CommandArgs::None)
        .map_err(Failure::from)?;
    Ok(true)
}

/// The build as it stands, and whether it has finished.
fn report(state: &State, started: bool) -> Result<(Value, bool), Failure> {
    let build = state
        .engine
        .site_build()
        .ok_or_else(|| Failure::internal("the site build is gone"))?;
    let (outcome, site, error) = match &build.state {
        SiteBuildState::Running => ("running", None, None),
        SiteBuildState::Built { site } => ("built", site.as_deref(), None),
        SiteBuildState::Failed(error) => ("failed", None, Some(error.as_str())),
    };
    let folder = build.playbook.parent().unwrap_or(&build.playbook);
    let (log, cut) = log_end(&build.log);
    let report = json!({
        "schema_version": SCHEMA_VERSION,
        "started": started,
        "outcome": outcome,
        "playbook": state.shown(&build.playbook),
        "output_dir": state.shown(&folder.join("build/site")),
        "site": site.map(|site| state.shown(site)),
        "error": error,
        "log": log,
        "log_lines": build.log.len(),
        "log_truncated": cut,
    });
    Ok((report, outcome != "running"))
}

/// The log's last lines, at most [`LOG_BYTES`] of them, and whether any were left out.
fn log_end(log: &[String]) -> (Vec<&str>, bool) {
    let mut bytes = 0;
    let kept = log
        .iter()
        .rev()
        .take_while(|line| {
            bytes += line.len() + 1;
            bytes <= LOG_BYTES
        })
        .count();
    let start = log.len() - kept;
    (log[start..].iter().map(String::as_str).collect(), start > 0)
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _, path::Path};

    use adoc_engine::{Engine, NewProject, Settings};

    use super::*;
    use crate::server::Permissions;

    /// A server with `--allow-build` on a project `project.new` made, whose npm is a stand-in
    /// that waits far longer than `[tools] build_timeout` (2 s) while `slow` exists.
    fn server(directory: &Path) -> (Server, std::path::PathBuf) {
        let tools = directory.join("tools");
        fs::create_dir(&tools).unwrap();
        let (node, npm) = (tools.join("node"), tools.join("npm"));
        fs::write(&node, "#!/bin/sh\nexit 0\n").unwrap();
        fs::write(
            &npm,
            "#!/bin/sh\n\
             echo \"npm $*\"\n\
             mkdir -p node_modules\n\
             if [ \"$1\" = run ]; then\n\
               if [ -f slow ]; then echo waiting; sleep 30; fi\n\
               mkdir -p build/site && echo '<html></html>' > build/site/index.html\n\
             fi\n",
        )
        .unwrap();
        for tool in [&node, &npm] {
            fs::set_permissions(tool, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let user = directory.join("settings.toml");
        fs::write(
            &user,
            format!(
                "[tools]\nnode = {:?}\nbuild_timeout = 2\n",
                node.display().to_string()
            ),
        )
        .unwrap();
        let root = adoc_engine::normalize_path(&directory.join("demo"));
        fs::create_dir(&root).unwrap();
        let mut engine = Engine::new(Settings::load(Some(&user), None).unwrap().with_git_opt_in());
        engine.open_workspace(vec![root.clone()]).unwrap();
        assert!(engine.wait_for_workers(Duration::from_secs(30)));
        let request = NewProject {
            git_init: false,
            ..NewProject::new(root.clone(), "demo")
        };
        engine
            .invoke(ids::PROJECT_NEW, CommandArgs::NewProject(request))
            .unwrap();
        let permissions = Permissions {
            write: false,
            build: true,
        };
        (Server::new(engine, vec![root.clone()], permissions), root)
    }

    fn structured(answer: &rmcp::model::CallToolResult) -> Value {
        answer.structured_content.clone().unwrap()
    }

    #[test]
    fn mcp_build_site_returns_logs_and_output_paths() {
        let directory = tempfile::tempdir().unwrap();
        let (server, _root) = server(directory.path());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let built = runtime
            .block_on(server.build_site(Parameters(BuildSiteArgs { wait_seconds: None })))
            .unwrap();
        let built = structured(&built);
        assert_eq!(built["outcome"], "built", "{built}");
        assert_eq!(built["started"], true);
        assert_eq!(built["playbook"], "antora-playbook.yml");
        assert_eq!(built["output_dir"], "build/site");
        assert_eq!(built["site"], "build/site/index.html");
        assert_eq!(
            built["log"],
            json!([
                "$ npm install",
                "npm install",
                "$ npm run build",
                "npm run build"
            ])
        );
        assert_eq!(built["log_truncated"], false);
    }

    /// A stalled build answers `running` when the call's wait ends, other requests are answered
    /// while it runs, a second call waits for it rather than starting another, and the engine
    /// stops it at `[tools] build_timeout`.
    #[test]
    fn mcp_stalled_build_times_out_without_blocking_other_clients() {
        let directory = tempfile::tempdir().unwrap();
        let (server, root) = server(directory.path());
        fs::create_dir(root.join("node_modules")).unwrap();
        fs::write(root.join("slow"), "").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let running = server
                .build_site(Parameters(BuildSiteArgs {
                    wait_seconds: Some(0),
                }))
                .await
                .unwrap();
            let running = structured(&running);
            assert_eq!(running["outcome"], "running", "{running}");
            assert_eq!(running["started"], true);

            let waiting = server.clone();
            let second = tokio::spawn(async move {
                waiting
                    .build_site(Parameters(BuildSiteArgs { wait_seconds: None }))
                    .await
            });
            let asked = Instant::now();
            let revision = server
                .blocking(|state| state.engine.workspace_revision())
                .await;
            assert!(revision.is_ok());
            assert!(
                asked.elapsed() < Duration::from_secs(1),
                "answered meanwhile"
            );

            let stopped = structured(&second.await.unwrap().unwrap());
            assert_eq!(stopped["started"], false, "the same build: {stopped}");
            assert_eq!(stopped["outcome"], "failed");
            assert!(
                stopped["error"].as_str().unwrap().contains("2 s"),
                "{stopped}"
            );
            assert!(stopped["log"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line == "waiting"));
        });
    }

    #[test]
    fn mcp_build_site_without_node_is_unavailable() {
        let directory = tempfile::tempdir().unwrap();
        let (server, _root) = server(directory.path());
        fs::remove_file(directory.path().join("tools/npm")).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let refused = runtime
            .block_on(server.build_site(Parameters(BuildSiteArgs { wait_seconds: None })))
            .unwrap();
        assert_eq!(refused.is_error, Some(true));
        assert_eq!(
            refused.meta.as_ref().unwrap().0["error"]["code"],
            "unavailable"
        );
    }

    #[test]
    fn the_log_s_end_is_kept() {
        let long = "x".repeat(LOG_BYTES / 2);
        let log = vec![
            "first".to_owned(),
            long.clone(),
            long.clone(),
            "last".to_owned(),
        ];
        let (kept, cut) = log_end(&log);
        assert!(cut);
        assert_eq!(kept.last(), Some(&"last"));
        assert!(!kept.contains(&"first"));
        assert_eq!(log_end(&log[..1]), (vec!["first"], false));
    }
}
