//! Help › Show Diagnostics: the engine's doctor report, with what only the app knows (its
//! graphics and fonts), shown in a dialog with a button that copies it for an issue.

use adoc_engine::{Doctor, DoctorSection};
use gpui::{
    div, px, App, AppContext as _, ClipboardItem, InteractiveElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, WindowExt as _,
};

/// Completes `doctor` with the app's section, runs it off the main thread (it runs each tool
/// for its version) and shows the report.
pub fn show(doctor: Doctor, editor_font: &str, window: &mut Window, cx: &mut App) {
    let mut doctor = doctor.program("AdocFoundry", env!("CARGO_PKG_VERSION"));
    doctor.add_section(display_section(editor_font, window, cx));
    let task = cx.background_spawn(async move { doctor.run() });
    window
        .spawn(cx, async move |cx| {
            let report = task.await;
            cx.update(|window, cx| open(report.to_text(), window, cx))
        })
        .detach();
}

/// The graphics backend and the fonts the app draws with.
fn display_section(editor_font: &str, window: &Window, cx: &App) -> DoctorSection {
    let backend = if cfg!(target_os = "macos") {
        "Metal"
    } else if cfg!(windows) {
        "DirectX 11"
    } else {
        "wgpu"
    };
    let mut section = DoctorSection::new("Display").line("Graphics", backend);
    if let Some(gpu) = window.gpu_specs() {
        let emulated = if gpu.is_software_emulated {
            ", software emulated"
        } else {
            ""
        };
        section = section.line(
            "GPU",
            format!(
                "{} ({} {}{emulated})",
                gpu.device_name, gpu.driver_name, gpu.driver_info
            ),
        );
    }
    if cfg!(target_os = "linux") {
        let server = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            "Wayland"
        } else if std::env::var_os("DISPLAY").is_some() {
            "X11"
        } else {
            "none found"
        };
        section = section.line("Display server", server);
    }
    section = section.line("Scale factor", window.scale_factor().to_string());
    let families = cx.text_system().all_font_names();
    let font = |family: &str| {
        if families.iter().any(|name| name == family) {
            format!("{family} (found)")
        } else {
            format!("{family} (not installed, so a fallback is used)")
        }
    };
    section
        .line("Editor font", font(editor_font))
        .line("Interface font", font(&cx.theme().font_family))
}

fn open(text: String, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let copy = text.clone();
        // Blank lines keep their height, and spaces their width in the monospaced font.
        let lines = text.lines().map(|line| {
            div().child(SharedString::from(if line.is_empty() {
                " ".to_owned()
            } else {
                line.to_owned()
            }))
        });
        dialog.title("Diagnostics").w(px(720.)).child(
            v_flex()
                .gap_3()
                .child("Paste this into the issue when you report a problem.")
                .child(
                    div()
                        .id("diagnostics-report")
                        .debug_selector(|| "diagnostics-report".to_owned())
                        .max_h(px(420.))
                        .overflow_y_scroll()
                        .p_2()
                        .bg(cx.theme().muted)
                        .rounded(cx.theme().radius)
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .children(lines),
                )
                .child(
                    h_flex().justify_end().child(
                        Button::new("diagnostics-copy")
                            .primary()
                            .label("Copy")
                            .debug_selector(|| "diagnostics-copy".to_owned())
                            .on_click(move |_, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                window.push_notification("Diagnostics copied.", cx);
                            }),
                    ),
                ),
        )
    });
}
