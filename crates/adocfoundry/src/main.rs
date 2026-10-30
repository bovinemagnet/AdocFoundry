//! AdocFoundry: a desktop editor for AsciiDoc documentation laid out the Antora way.
//!
//! `adocfoundry [folder [file…]]` opens the folder as a workspace and the files in tabs.

mod app;
mod diagnostics;
mod editor;
mod files_view;
mod history_view;
mod image_decode;
mod image_view;
mod menus;
mod mirror_view;
mod preview;
mod references_view;
mod search_view;
mod sidebar;
mod spelling;
mod typing_bench;
#[cfg(test)]
mod ui_tests;
mod updates;
mod visual;
mod workspace_view;

use std::path::PathBuf;

use adoc_engine::{Engine, Settings};
use gpui::AppContext as _;

fn main() {
    if matches!(std::env::args().nth(1).as_deref(), Some("--version" | "-V")) {
        println!("adocfoundry {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let mut arguments = std::env::args_os().skip(1).map(PathBuf::from);
    let folder = arguments.next();
    let files: Vec<PathBuf> = arguments.collect();
    let settings = match Settings::load(Settings::default_user_file().as_deref(), None) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("adocfoundry: {error}; using default settings");
            Settings::default()
        }
    }
    // Where unsaved text is kept for recovery, and the local history.
    .with_state_dir(Settings::default_state_dir());
    let engine = Engine::new(settings);

    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx| {
            gpui_component::init(cx);
            editor::language::register();
            editor::bindings::bind(&engine, cx);
            let app = cx.new(|cx| app::AppState::new(engine, cx));
            app::open_main_window(app, folder, files, cx);
            cx.activate(true);
        });
}

// The icons the app uses beyond gpui-component's own.
gpui_kit_assets::icon_assets!(AppIcons, [Image]);

/// gpui-component's icons, and the app's.
struct Assets;

impl gpui::AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match AppIcons.load(path)? {
            Some(icon) => Ok(Some(icon)),
            None => gpui_kit_assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<gpui::SharedString>> {
        let mut listed = gpui_kit_assets::Assets.list(path)?;
        listed.extend(AppIcons.list(path)?);
        Ok(listed)
    }
}
