//! AdocFoundry: a desktop editor for AsciiDoc documentation laid out the Antora way.
//!
//! `adocfoundry [folder [file…]]` opens the folder as a workspace and the files in tabs.

mod app;
mod editor;
mod mirror_view;
mod search_view;
mod sidebar;
mod workspace_view;

use std::path::PathBuf;

use adoc_engine::{Engine, Settings};
use gpui::AppContext as _;

fn main() {
    let mut arguments = std::env::args_os().skip(1).map(PathBuf::from);
    let folder = arguments.next();
    let files: Vec<PathBuf> = arguments.collect();
    let settings = match Settings::load(Settings::default_user_file().as_deref(), None) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("adocfoundry: {error}; using default settings");
            Settings::default()
        }
    };
    let engine = Engine::new(settings);

    gpui_platform::application()
        .with_assets(gpui_kit_assets::Assets)
        .run(move |cx| {
            gpui_component::init(cx);
            editor::language::register();
            editor::bindings::bind(&engine, cx);
            let app = cx.new(|cx| app::AppState::new(engine, cx));
            app::open_main_window(app, folder, files, cx);
            cx.activate(true);
        });
}
