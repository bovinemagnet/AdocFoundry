//! Help › About AdocFoundry: the engine's `About`, named for the app and given the third-party
//! notices its build script embedded, shown as the program's details over a list of every
//! component beside the chosen one's licence texts, with a button that copies them.

use adoc_engine::{About, Notices};
use gpui::{
    div, prelude::FluentBuilder as _, px, uniform_list, App, AppContext as _, ClipboardItem,
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, WindowExt as _,
};

/// The notices of every component the release build is built from, from `xtask/src/notices.rs`.
const NOTICES: &str = include_str!(concat!(env!("OUT_DIR"), "/notices.json"));

/// Names the app in `about`, gives it the embedded notices and shows it.
pub fn show(about: About, window: &mut Window, cx: &mut App) {
    // The build script writes the notices, so they always read; an empty list says otherwise.
    let notices = Notices::from_json(NOTICES).unwrap_or_default();
    let about = about
        .program(
            "AdocFoundry",
            env!("CARGO_PKG_VERSION"),
            Some(env!("ADOCFOUNDRY_BUILD")),
        )
        .notices(notices);
    let title = format!("About {}", about.name);
    let view = cx.new(|_| AboutView { about, selected: 0 });
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title(title.clone()).w(px(880.)).child(view.clone())
    });
}

struct AboutView {
    about: About,
    /// The row chosen: 0 is the program itself, then each component in turn.
    selected: usize,
}

impl AboutView {
    /// Row `index`'s name, version and licence.
    fn entry(&self, index: usize) -> (String, &str) {
        match index.checked_sub(1) {
            None => (
                format!("{} {}", self.about.name, self.about.version),
                self.about.licence,
            ),
            Some(index) => {
                let notice = &self.about.notices.components[index];
                (
                    format!("{} {}", notice.name, notice.version),
                    notice.licence.as_str(),
                )
            }
        }
    }

    /// Row `index`'s licence texts as one text, to read or copy.
    fn text(&self, index: usize) -> String {
        match index.checked_sub(1) {
            None => format!(
                "{} {}\nLicence: {}\n\nLICENSE\n\n{}",
                self.about.name, self.about.version, self.about.licence, self.about.licence_text
            ),
            Some(index) => self
                .about
                .notices
                .text(&self.about.notices.components[index]),
        }
    }
}

impl Render for AboutView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let about = &self.about;
        let mut details = format!("Licence {}", about.licence);
        if let Some(build) = &about.build {
            details = format!("Build {build} · {details}");
        }
        let count = about.notices.components.len();
        let rows = uniform_list(
            "about-components",
            count + 1,
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                let theme = cx.theme();
                range
                    .map(|index| {
                        let (name, licence) = this.entry(index);
                        div()
                            .id(("about-component", index))
                            .debug_selector(move || format!("about-component-{index}"))
                            .px_2()
                            .py_1()
                            .rounded(theme.radius)
                            .cursor_pointer()
                            .when(index == this.selected, |row| row.bg(theme.accent))
                            .child(div().text_sm().truncate().child(name))
                            .child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .text_color(theme.muted_foreground)
                                    .child(licence.to_owned()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = index;
                                cx.notify();
                            }))
                    })
                    .collect()
            }),
        )
        .h_full();
        let text = self.text(self.selected);
        // Blank lines keep their height.
        let lines = text.lines().map(|line| {
            div().child(SharedString::from(if line.is_empty() {
                " ".to_owned()
            } else {
                line.to_owned()
            }))
        });
        let repository = about.repository;
        let documentation = about.documentation;
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .items_start()
                    .child(
                        v_flex()
                            .child(
                                div()
                                    .text_lg()
                                    .child(format!("{} {}", about.name, about.version)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .debug_selector(|| "about-details".to_owned())
                                    .child(details),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("about-repository")
                                    .label("Repository")
                                    .on_click(move |_, _, cx| cx.open_url(repository)),
                            )
                            .child(
                                Button::new("about-documentation")
                                    .label("Documentation")
                                    .on_click(move |_, _, cx| cx.open_url(documentation)),
                            ),
                    ),
            )
            .child(format!(
                "{} is built from the {count} open source components below. Choose one to read \
                 its licence.",
                about.name
            ))
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(div().w(px(260.)).h(px(360.)).child(rows))
                    .child(
                        div()
                            .id("about-licence")
                            .debug_selector(|| "about-licence".to_owned())
                            .flex_1()
                            .min_w_0()
                            .h(px(360.))
                            .overflow_y_scroll()
                            .p_2()
                            .bg(theme.muted)
                            .rounded(theme.radius)
                            .font_family(theme.mono_font_family.clone())
                            .text_xs()
                            .children(lines),
                    ),
            )
            .child(
                h_flex().justify_end().child(
                    Button::new("about-copy")
                        .primary()
                        .label("Copy Licence")
                        .debug_selector(|| "about-copy".to_owned())
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                            window.push_notification("Licence copied.", cx);
                        }),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_embeds_every_components_notice() {
        let notices = Notices::from_json(NOTICES).unwrap();
        for name in ["gpui-pre", "gpui-component", "typst", "Noto Serif (subset)"] {
            let notice = notices
                .components
                .iter()
                .find(|notice| notice.name == name)
                .unwrap_or_else(|| panic!("{name} has no notice"));
            assert!(notices.texts(notice).next().is_some(), "{name}");
        }
    }
}
