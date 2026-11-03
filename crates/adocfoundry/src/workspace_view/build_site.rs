//! Build Site (#293): the Build panel, a sidebar tab shown once a build has started, which
//! draws the engine's `SiteBuild` log as it arrives and offers the built site; and the
//! notification each build ends with. The build itself is the engine's `project.build_site`.

use adoc_engine::SiteBuildState;
use gpui::{uniform_list, AnyElement, ScrollStrategy};

use super::*;
use crate::editor::bindings::BuildSite;

impl WorkspaceView {
    /// Builds the site, asking first the first time in this window for a playbook's folder:
    /// `npm install` and the project's `build` script run the project's own code.
    pub(super) fn on_build_site(
        &mut self,
        _: &BuildSite,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((folder, commands)) = self.app.read(cx).engine.site_build_plan() else {
            // Not runnable: the engine says why.
            self.start_site_build(window, cx);
            return;
        };
        if self.trusted_builds.contains(&folder) {
            self.start_site_build(window, cx);
            return;
        }
        let workspace = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let (workspace, folder) = (workspace.clone(), folder.clone());
            let commands = commands.iter().map(|command| div().child(command.clone()));
            dialog
                .title("Run this project's build?")
                .child(
                    v_flex()
                        .gap_2()
                        .debug_selector(|| "build-confirm".to_owned())
                        .child(format!("Build Site runs these in {}:", folder.display()))
                        .child(
                            v_flex()
                                .p_2()
                                .bg(cx.theme().muted)
                                .rounded(cx.theme().radius)
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_xs()
                                .children(commands),
                        )
                        .child(
                            "They run the project's own scripts and its packages' code, which can \
                             do anything you can. Build only projects you trust.",
                        ),
                )
                .confirm()
                .ok_text("Build")
                .on_ok(move |_, window, cx| {
                    let folder = folder.clone();
                    let _ = workspace.update(cx, |this, cx| {
                        this.trusted_builds.insert(folder);
                        this.start_site_build(window, cx);
                    });
                    true
                })
        });
    }

    fn start_site_build(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .invoke(ids::PROJECT_BUILD_SITE, CommandArgs::None, window, cx)
            .is_some()
        {
            self.sidebar_tab = SidebarTab::Build;
            cx.notify();
        }
    }

    /// Keeps the log's last line in view as lines arrive.
    pub(super) fn site_build_output(&self, cx: &App) {
        let lines = self
            .app
            .read(cx)
            .engine
            .site_build()
            .map_or(0, |build| build.log.len());
        if lines > 0 {
            self.build_scroll
                .scroll_to_item(lines - 1, ScrollStrategy::Bottom);
        }
    }

    pub(super) fn site_build_finished(
        result: &Result<Option<PathBuf>, String>,
        window: &mut Window,
        cx: &mut App,
    ) {
        match result {
            Ok(Some(site)) => {
                let site = site.clone();
                window.push_notification(
                    Notification::new()
                        .message("The site is built.")
                        .action(move |_, _, _| open_site_button("build-notification-open", &site)),
                    cx,
                );
            }
            Ok(None) => window.push_notification(
                "The build finished, but wrote no build/site/index.html.",
                cx,
            ),
            Err(error) => window.push_notification(format!("The site build failed: {error}"), cx),
        }
    }

    /// The Build panel: how the build stands, Open Site once it wrote one, and its log.
    pub(super) fn render_build(&self, cx: &mut Context<Self>) -> AnyElement {
        let engine = &self.app.read(cx).engine;
        let Some(build) = engine.site_build() else {
            return div().into_any_element();
        };
        let (status, site) = match &build.state {
            SiteBuildState::Running => ("Building…".to_owned(), None),
            SiteBuildState::Built { site } => ("Built.".to_owned(), site.clone()),
            SiteBuildState::Failed(error) => (format!("Failed: {error}"), None),
        };
        let lines = build.log.len();
        let log = uniform_list(
            "build-log",
            lines,
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                let engine = &this.app.read(cx).engine;
                let log = engine
                    .site_build()
                    .map(|build| &build.log[..])
                    .unwrap_or(&[]);
                range
                    .filter_map(|index| log.get(index).map(|line| (index, line)))
                    .map(|(index, line)| {
                        div()
                            .id(("build-line", index))
                            .debug_selector(move || format!("build-line-{index}"))
                            .px_2()
                            .whitespace_nowrap()
                            // An empty line keeps its height.
                            .child(SharedString::from(if line.is_empty() {
                                " ".to_owned()
                            } else {
                                line.clone()
                            }))
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.build_scroll)
        .flex_1()
        .font_family(cx.theme().mono_font_family.clone())
        .text_xs();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .p_2()
                    .gap_2()
                    .justify_between()
                    .child(
                        div()
                            .debug_selector(|| "build-status".to_owned())
                            .text_sm()
                            .child(status),
                    )
                    .children(site.map(|site| open_site_button("build-open-site", &site))),
            )
            .child(log)
            .into_any_element()
    }
}

/// A button that opens the built site in the system's browser.
fn open_site_button(id: &'static str, site: &Path) -> Button {
    let site = site.to_path_buf();
    Button::new(id)
        .small()
        .label("Open Site")
        .debug_selector(move || id.to_owned())
        .on_click(move |_, _, cx| crate::files_view::open_externally(&site, cx))
}
