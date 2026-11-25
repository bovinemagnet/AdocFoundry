//! The settings window (#80): the engine's `setting_specs`, a page for each group of them, each
//! showing what it is now and saved with `settings.save` to the user settings file. Text and
//! numbers are saved once typing pauses, or on Enter or leaving the field; the rest at once. A
//! setting the workspace's file gives is shown and not changed here.

use std::{collections::BTreeMap, time::Duration};

use adoc_engine::{
    commands::ids, setting_specs, CommandArgs, EngineEvent, SettingChange, SettingKind,
    SettingSource, SettingSpec, SettingValue,
};
use gpui::{
    div, prelude::FluentBuilder as _, AnyElement, App, AppContext as _, Context, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Task, WeakEntity, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState, NumberInput},
    menu::{DropdownMenu as _, PopupMenuItem},
    setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings},
    switch::Switch,
    v_flex, ActiveTheme as _, Disableable as _, IconName, Sizable as _, ThemeMode, TitleBar,
};

use crate::app::AppState;

/// How long typing in a field pauses before what it holds is saved.
pub(crate) const COMMIT_DELAY: Duration = Duration::from_millis(500);

pub struct SettingsView {
    app: Entity<AppState>,
    /// What fields hold that is not saved yet.
    pending: BTreeMap<&'static str, SettingValue>,
    /// Why the last change was not saved.
    problem: Option<String>,
    _commit: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = cx.weak_entity();
        // What was typed last is saved as the window closes or the app quits.
        window.on_window_should_close(cx, move |_, cx| {
            let _ = view.update(cx, |view, cx| view.commit(cx));
            true
        });
        let subscriptions = vec![
            cx.subscribe(&app, |_, _, event: &EngineEvent, cx| {
                if *event == EngineEvent::SettingsChanged {
                    cx.notify();
                }
            }),
            cx.on_app_quit(|this, cx| {
                this.commit(cx);
                async {}
            }),
        ];
        Self {
            app,
            pending: BTreeMap::new(),
            problem: None,
            _commit: Task::ready(()),
            _subscriptions: subscriptions,
        }
    }

    /// What `spec` is shown as: what its field holds, else what the settings say.
    fn value(&self, spec: &SettingSpec, cx: &App) -> SettingValue {
        self.pending.get(spec.id).cloned().unwrap_or_else(|| {
            self.app
                .read(cx)
                .engine
                .setting(spec.id)
                .map_or(SettingValue::Text(String::new()), |(value, _)| value)
        })
    }

    fn source(&self, spec: &SettingSpec, cx: &App) -> SettingSource {
        self.app
            .read(cx)
            .engine
            .setting(spec.id)
            .map_or(SettingSource::Default, |(_, source)| source)
    }

    /// Saves `value`, or takes the setting out of the file.
    fn save(&mut self, id: &'static str, value: Option<SettingValue>, cx: &mut Context<Self>) {
        self.pending.remove(id);
        let change = SettingChange {
            id: id.to_owned(),
            value,
        };
        let saved = self.app.update(cx, |app, cx| {
            app.invoke(ids::SETTINGS_SAVE, CommandArgs::Setting(change), cx)
        });
        self.problem = saved.err().map(|error| error.to_string());
        cx.notify();
    }

    /// Saves `value` once nothing has been typed for [`COMMIT_DELAY`].
    fn save_later(&mut self, id: &'static str, value: SettingValue, cx: &mut Context<Self>) {
        self.pending.insert(id, value);
        self._commit = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COMMIT_DELAY).await;
            let _ = this.update(cx, |this, cx| this.commit(cx));
        });
        cx.notify();
    }

    /// Saves what fields hold.
    fn commit(&mut self, cx: &mut Context<Self>) {
        for (id, value) in std::mem::take(&mut self.pending) {
            self.save(id, Some(value), cx);
        }
    }

    fn open_file(&mut self, cx: &mut Context<Self>) {
        self.commit(cx);
        let main = self.app.read(cx).main.clone();
        if let Some((window, workspace)) = main {
            let _ = window.update(cx, |_, window, cx| {
                let _ =
                    workspace.update(cx, |workspace, cx| workspace.open_settings_file(window, cx));
                window.activate_window();
            });
        }
    }

    fn item(&self, spec: &'static SettingSpec, cx: &mut Context<Self>) -> SettingItem {
        let view = cx.weak_entity();
        let source = self.source(spec, cx);
        let description = if source == SettingSource::Workspace {
            format!(
                "{} Set by this workspace's {}.",
                spec.description,
                adoc_engine::WORKSPACE_SETTINGS_FILE
            )
        } else {
            spec.description.to_owned()
        };
        let disabled = source == SettingSource::Workspace;
        let field = SettingField::render(move |_, window, cx| control(spec, &view, window, cx));
        SettingItem::new(spec.title, field)
            .description(SharedString::from(description))
            .keywords([spec.id])
            .disabled(disabled)
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut names: Vec<&'static str> = Vec::new();
        for spec in setting_specs() {
            if !names.contains(&spec.page) {
                names.push(spec.page);
            }
        }
        let pages: Vec<SettingPage> = names
            .into_iter()
            .map(|name| {
                let items: Vec<SettingItem> = setting_specs()
                    .iter()
                    .filter(|spec| spec.page == name)
                    .map(|spec| self.item(spec, cx))
                    .collect();
                SettingPage::new(name)
                    .default_open(true)
                    // Each setting has a button of its own: one for the page would take out
                    // everything on it.
                    .resettable(false)
                    .group(SettingGroup::new().items(items))
            })
            .collect();
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                TitleBar::new().child("Settings").child(
                    Button::new("open-settings-file")
                        .label("Open File")
                        .small()
                        .mr_2()
                        .debug_selector(|| "open-settings-file".to_owned())
                        .on_click(cx.listener(|this, _, _, cx| this.open_file(cx))),
                ),
            )
            .when_some(self.problem.clone(), |this, problem| {
                this.child(
                    div()
                        .debug_selector(|| "settings-problem".to_owned())
                        .px_4()
                        .py_2()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(problem),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Settings::new("settings").pages(pages)),
            )
    }
}

/// What a field shows for `value`.
fn shown(value: &SettingValue) -> String {
    match value {
        SettingValue::Bool(on) => on.to_string(),
        SettingValue::Number(number) => number.to_string(),
        SettingValue::Text(text) => text.clone(),
        SettingValue::List(items) => items.join(", "),
    }
}

/// The least and most a number setting may be, and how far a step goes.
fn bounds(kind: &SettingKind) -> Option<NumberFieldOptions> {
    let (min, max, step) = match kind {
        SettingKind::Number { min, max } => (f64::from(*min), f64::from(*max), 1.),
        SettingKind::Scale { min, max } => (*min, *max, 0.1),
        _ => return None,
    };
    Some(NumberFieldOptions { min, max, step })
}

/// What `text` in `spec`'s field stands for, if it stands for anything the setting may be.
fn read(spec: &SettingSpec, text: &str) -> Option<SettingValue> {
    if let Some(bounds) = bounds(&spec.kind) {
        let number: f64 = text.trim().parse().ok()?;
        let whole = matches!(spec.kind, SettingKind::Number { .. });
        return ((bounds.min..=bounds.max).contains(&number) && !(whole && number.fract() != 0.))
            .then_some(SettingValue::Number(number));
    }
    Some(match spec.kind {
        SettingKind::List => SettingValue::List(
            text.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect(),
        ),
        _ => SettingValue::Text(text.to_owned()),
    })
}

/// The control that shows and changes `spec`.
fn control(
    spec: &'static SettingSpec,
    view: &WeakEntity<SettingsView>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(entity) = view.upgrade() else {
        return div().into_any_element();
    };
    let value = entity.read(cx).value(spec, cx);
    let source = entity.read(cx).source(spec, cx);
    let disabled = source == SettingSource::Workspace;
    let control = match &spec.kind {
        SettingKind::Switch => switch(spec, value == SettingValue::Bool(true), disabled, view),
        SettingKind::Choice(choices) => {
            let choices = choices
                .iter()
                .map(|(value, name)| ((*value).into(), (*name).into()))
                .collect();
            dropdown(spec, choices, &shown(&value), disabled, view)
        }
        SettingKind::LightTheme | SettingKind::DarkTheme => {
            let mode = match spec.kind {
                SettingKind::DarkTheme => ThemeMode::Dark,
                _ => ThemeMode::Light,
            };
            let dir = entity
                .read(cx)
                .app
                .read(cx)
                .engine
                .settings()
                .appearance
                .themes_dir
                .clone();
            let mut choices: Vec<(SharedString, SharedString)> =
                vec![("".into(), "Default".into())];
            choices.extend(
                crate::appearance::theme_names(dir.as_deref(), mode, cx)
                    .into_iter()
                    .map(|name| (name.clone(), name)),
            );
            dropdown(spec, choices, &shown(&value), disabled, view)
        }
        SettingKind::FontSet => {
            let choices = entity
                .read(cx)
                .app
                .read(cx)
                .engine
                .settings()
                .appearance
                .font_sets()
                .into_iter()
                .map(|set| (set.id.into(), set.name.into()))
                .collect();
            dropdown(spec, choices, &shown(&value), disabled, view)
        }
        SettingKind::Text
        | SettingKind::List
        | SettingKind::Number { .. }
        | SettingKind::Scale { .. } => field(spec, &value, disabled, view, window, cx),
    };
    let reset = (source == SettingSource::User).then(|| {
        let view = view.clone();
        div().debug_selector(|| format!("reset-{}", spec.id)).child(
            Button::new(SharedString::from(format!("reset-{}", spec.id)))
                .icon(IconName::Undo)
                .ghost()
                .small()
                .accessibility_label(format!("Reset {} to its default", spec.title))
                .tooltip("Reset to the default")
                .on_click(move |_, _, cx| {
                    let _ = view.update(cx, |view, cx| view.save(spec.id, None, cx));
                }),
        )
    });
    h_flex()
        .gap_1()
        .children(reset)
        .child(
            div()
                .debug_selector(|| format!("setting-{}", spec.id))
                .child(control),
        )
        .into_any_element()
}

fn switch(
    spec: &'static SettingSpec,
    on: bool,
    disabled: bool,
    view: &WeakEntity<SettingsView>,
) -> AnyElement {
    let view = view.clone();
    Switch::new(spec.id)
        .accessibility_label(spec.title)
        .checked(on)
        .disabled(disabled)
        .on_click(move |on, _, cx| {
            let value = SettingValue::Bool(*on);
            let _ = view.update(cx, |view, cx| view.save(spec.id, Some(value), cx));
        })
        .into_any_element()
}

fn dropdown(
    spec: &'static SettingSpec,
    choices: Vec<(SharedString, SharedString)>,
    chosen: &str,
    disabled: bool,
    view: &WeakEntity<SettingsView>,
) -> AnyElement {
    let chosen = SharedString::from(chosen.to_owned());
    let name = choices
        .iter()
        .find(|(value, _)| *value == chosen)
        .map_or_else(|| chosen.clone(), |(_, name)| name.clone());
    let view = view.clone();
    Button::new(spec.id)
        .accessibility_label(format!("{}: {name}", spec.title))
        .label(name)
        .dropdown_caret(true)
        .outline()
        .disabled(disabled)
        .dropdown_menu(move |menu, _, _| {
            choices
                .iter()
                .fold(menu.scrollable(true), |menu, (value, name)| {
                    let (view, value) = (view.clone(), value.clone());
                    menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(value == chosen)
                            .on_click(move |_, _, cx| {
                                let value = SettingValue::Text(value.to_string());
                                let _ =
                                    view.update(cx, |view, cx| view.save(spec.id, Some(value), cx));
                            }),
                    )
                })
        })
        .into_any_element()
}

/// A field's text, kept while its window shows it.
struct Field {
    input: Entity<InputState>,
    /// What the setting was shown as, so that a change of it made elsewhere is seen.
    last: String,
    _subscription: Subscription,
}

fn field(
    spec: &'static SettingSpec,
    value: &SettingValue,
    disabled: bool,
    view: &WeakEntity<SettingsView>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let text = shown(value);
    let bounds = bounds(&spec.kind);
    let state = window.use_keyed_state(spec.id, cx, |window, cx| {
        let input = cx.new(|cx| {
            let input = InputState::new(window, cx).default_value(text.clone());
            match &bounds {
                Some(bounds) => input.step(bounds.step).min(bounds.min).max(bounds.max),
                None => input,
            }
        });
        let view = view.clone();
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |field: &mut Field, input, event, window, cx| {
                let text = input.read(cx).value().to_string();
                let value = read(spec, &text);
                let _ = view.update(cx, |view, cx| match (event, value) {
                    // The field's own text, set to follow the setting, is no change of it.
                    (InputEvent::Change, _) if text == field.last => {}
                    (InputEvent::Change, Some(value)) => {
                        field.last = shown(&value);
                        view.save_later(spec.id, value, cx);
                    }
                    // Half typed, or more than it may be: nothing typed before it is meant.
                    (InputEvent::Change, None) => {
                        view.pending.remove(spec.id);
                        field.last = shown(&view.value(spec, cx));
                    }
                    (InputEvent::PressEnter { .. } | InputEvent::Blur, value) => {
                        view.commit(cx);
                        if value.is_none() {
                            let text = field.last.clone();
                            input.update(cx, |input, cx| input.set_value(text, window, cx));
                        }
                    }
                    (InputEvent::Focus, _) => {}
                });
            },
        );
        Field {
            input,
            last: text.clone(),
            _subscription: subscription,
        }
    });
    let input = state.read(cx).input.clone();
    if state.read(cx).last != text {
        state.update(cx, |field, _| field.last.clone_from(&text));
        input.update(cx, |input, cx| input.set_value(text, window, cx));
    }
    if bounds.is_some() {
        NumberInput::new(&input)
            .accessibility_label(spec.title)
            .disabled(disabled)
            .w_32()
            .into_any_element()
    } else {
        Input::new(&input)
            .aria_label(spec.title)
            .disabled(disabled)
            .w_64()
            .into_any_element()
    }
}
