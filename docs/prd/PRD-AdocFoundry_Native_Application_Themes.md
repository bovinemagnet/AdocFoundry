# PRD: AdocFoundry — Native Application Themes

| Field | Value |
| --- | --- |
| Status | Implemented (#357), on `gpui-component` 0.9.0; departures in §14 |
| Date | 2026-11-04 |
| Scope | Named native `gpui-component` application themes, light/dark preference pairs, selection UI, user theme files and cross-window application |
| Repository | [bovinemagnet/AdocFoundry](https://github.com/bovinemagnet/AdocFoundry) |
| Related design | `docs/design/DESIGN-AdocFoundry_Architecture.md`; `AGENTS.md`; `cargo xtask layers` |
| Existing code | `crates/adoc-engine/src/settings.rs`; `crates/adocfoundry/src/main.rs`; `crates/adocfoundry/src/workspace_view.rs`; editor, preview, graph and visual views |
| Dependencies | `gpui-component = 0.9.0` (0.8.0 when written), `gpui`, settings loader, command palette and GPUI test platform |

## 1. Document purpose

AdocFoundry already supports system, light and dark application appearance via `ThemePreference::{Auto, Light, Dark}` and `gpui-component::Theme`. UI views mostly use semantic colours from `cx.theme()`. However, users cannot yet select named colour schemes, maintain preferred light/dark theme pairs, load their own native theme files or preview theme changes in a searchable selector.

This PRD adds **native `gpui-component` application themes** without importing, translating or emulating Zed theme files. It is limited to AdocFoundry's desktop appearance: the source editor, visual editor, previews' chrome, trees, panels, dialogs and auxiliary windows.

## 2. Decisions

| Question | Decision | Consequence |
| --- | --- | --- |
| Theme format | Use the JSON theme-set format supported by the **pinned** `gpui-component` version | No custom theme schema or Zed theme adapter |
| Theme storage | Application-level user preferences and an optional user theme directory | Opening a new Antora workspace does not change app appearance |
| Theme registry | Use `gpui-component::ThemeRegistry` and `Theme` for loading/applying themes | Do not create a second palette/registry |
| Appearance mode | `auto`, `light` or `dark`, with a named theme for each mode | System changes switch between the user's chosen variants |
| Source highlighting | Use the registry's syntax/highlight palette through the existing GPUI highlighter | Do not add a parallel AsciiDoc token-colour engine |
| Theme selection | Searchable palette with transient live preview and explicit commit/cancel | Preview must never persist accidentally |
| Compatibility | Preserve legacy `[editor] theme` values | Existing user files continue to work |
| Publication styles | Remain separate: `[pdf] theme` and Antora UI bundles are unaffected | Theme switching never changes exported content |
| Implementation | Extend the app and existing settings code; no new crate | Preserve the layer boundaries |

## 3. Goals

1. Select any named theme discovered in the native theme registry, including built-in defaults and valid user theme packs.
2. Support `auto`, `light` and `dark` modes, retaining separate preferred light and dark theme names.
3. Provide a searchable `Theme: Select` command with immediate live preview, Enter to commit and Escape to restore the prior appearance.
4. Apply theme changes consistently across all open application windows without restarting.
5. Persist selection and mode in user-level settings and restore them on restart.
6. Honour syntax highlighting in the source editor and code blocks, and ensure visual editor and graph UI remain legible.
7. Handle invalid, missing, duplicate or deleted user themes without crashing or leaving unreadable colours.
8. Preserve existing appearance settings and the headless engine's independence from GPUI.

## 4. Explicit non-goals

- Zed JSON theme imports, Zed extensions, a Zed themes marketplace or compatibility layer.
- A built-in graphical theme designer, individual colour picker or colour-token editor.
- Remote theme downloads, extension installation, executing scripts from themes or theme synchronisation between machines.
- Styling the published Antora site or altering HTML/PDF output, PDF themes or author-defined document attributes.
- Per-workspace UI themes in the initial release.
- Introducing GPUI into `adoc-engine`, CLI, MCP, LSP or lower-layer crates.
- Shipping every third-party theme bundled with `gpui-component`; ship only what is already available and/or deliberately licensed and tested.

## 5. User workflows

### 5.1 Select a theme

1. Invoke `Theme: Select` through the existing command palette or application menu.
2. A searchable list shows available themes, their light/dark mode and a clear indication of the active theme.
3. Navigating to an entry temporarily previews it across the application. Filtering must not itself change the theme unless a result becomes the active selection.
4. Enter commits the highlighted theme. The chosen name is stored for that theme's light/dark mode; explicit selection switches the current appearance to that mode unless the user deliberately chooses to retain `auto` in a mode-aware control.
5. Escape, dismiss or closing the picker without selection restores the **entire pre-preview theme state**, including mode and named choices; no preference file is written.

### 5.2 Follow system appearance

The user selects `auto` and chooses a light theme and a dark theme. When the OS switches appearance, AdocFoundry uses the corresponding saved name. If the named theme is unavailable or mismatches the required mode, it uses the native default for that mode and gives a non-blocking warning. The saved preference is retained so it can become active if the theme reappears.

### 5.3 Install a custom native theme

The user places a valid `gpui-component` JSON theme pack in a user-level theme directory. The theme becomes selectable after discovery/reload. Invalid files are skipped with a useful diagnostic and do not prevent other themes or the application from loading. File watching / hot reload may be used when supported by the pinned dependency; manual reload is an acceptable fallback if watch reliability cannot be established on all desktop platforms.

### 5.4 Reopen AdocFoundry

The app restores the saved mode and named themes, or safely resolves to native defaults. Existing `[editor] theme = "auto" | "light" | "dark"` settings continue to determine appearance unless superseded by the new settings.

## 6. Functional requirements

### 6.1 Registry, discovery and identity

- **THEME-01:** Initialise the native theme registry during desktop startup, after `gpui_component::init(cx)` and before selecting a non-default named theme.
- **THEME-02:** Discover built-in registry themes and supported user JSON packs; derive display names and light/dark classification from the native `ThemeConfig`, not filenames.
- **THEME-03:** Sort themes deterministically by mode and case-insensitive name. Show valid built-in and user themes without duplicates in the selector.
- **THEME-04:** Detect duplicate display names: honour the registry's documented native precedence, surface a warning that identifies the conflicting source and never silently select an arbitrary file.
- **THEME-05:** A malformed file or theme with an incompatible mode must not replace an active working theme or crash startup; fall back to the matching native default.
- **THEME-06:** User theme files are **data**, never executable content. Do not perform network access, invoke processes or resolve external resources through theme loading.
- **THEME-07:** Use a stable default user directory under AdocFoundry's platform-specific configuration location; user configuration may override it with an absolute or configuration-directory-relative path. Do not search the current working directory implicitly, because opening untrusted project folders must not change appearance.

### 6.2 Preference resolution and backward compatibility

Proposed user settings:

```toml
[appearance]
mode = "auto"                       # auto | light | dark
light_theme = "Default Light"        # actual native registry name
dark_theme = "Default Dark"         # actual native registry name
# themes_dir = "themes"             # relative to the user config directory
```

- **THEME-08:** Use `AppearanceSettings { mode, light_theme, dark_theme, themes_dir }` (or equivalent) in the existing settings model. Keep appearance independent of `[pdf]` and Antora content settings.
- **THEME-09:** Backward compatibility: if `[appearance].mode` is absent, use the existing `[editor].theme` value. If both are supplied, `[appearance].mode` wins and the loader warns once about the conflicting legacy entry. Continue accepting legacy settings; do not rewrite the file just by loading it.
- **THEME-10:** New appearance preferences must be user-level only for v1. Workspace `.adocfoundry.toml` must not override them; disallowed workspace settings receive the existing settings warning behaviour.
- **THEME-11:** Invalid mode, unknown theme name or unreadable directory must not fail application startup. Validate syntax as usual, report a warning and use the matching default; preserve the configured name when it is merely unavailable.
- **THEME-12:** Persist a confirmed selection using the existing settings management conventions, changing only the necessary user TOML keys and preserving unrelated settings and comments where feasible. Failed writes leave the in-memory selection active for the session, with an explicit warning.
- **THEME-13:** Changes to settings files through the existing Reload Settings command reconcile the current appearance. Do not allow a reload occurring during a temporary preview to overwrite the user's persistent settings unintentionally.

### 6.3 Theme application

- **THEME-14:** Resolve the effective mode from user preference and system appearance, then resolve the named theme of that mode. Apply through the native `Theme` update/apply mechanism **appropriate to pinned `gpui-component` 0.8.0** so UI colours, syntax highlights and every open window update together.
- **THEME-15:** Existing `Toggle Theme` remains available and changes the **saved** explicit mode, using the corresponding selected theme. In `auto`, toggling switches to the opposite explicit mode; the user can restore `auto` using the mode control. A temporary preview never changes saved preferences.
- **THEME-16:** Avoid global theme modifications that only repaint the current window. All application windows (workspace, graph, mirror/detached preview, dialogs) must be updated consistently.
- **THEME-17:** Keep cursor, selection, gutter, diagnostics, diff markers, links, tree selections, code highlighting and graph labels visually distinguishable. Prefer semantic `cx.theme()` tokens; eliminate hard-coded theme-dependent colours where practical.
- **THEME-18:** Theme selection must not alter document text, undo/redo stacks, export profiles, PDF rendering settings, Antora theme/UI bundles or cached publication output.

### 6.4 Selector and accessibility

- **THEME-19:** Add a `Theme: Select` action in the existing action/menu/command-palette flow, without introducing a separate top-level window.
- **THEME-20:** List themes with name, mode label and active indicator; filter names case-insensitively. Use the existing palette controls and keyboard navigation model.
- **THEME-21:** A highlighted candidate previews immediately; Enter commits; Escape/cancel restores exactly the prior effective theme and preference mode, even after several preview changes.
- **THEME-22:** A selection that cannot be applied remains on the prior valid theme and reports why; no corrupt setting is written.
- **THEME-23:** Keyboard-only operation is sufficient for all selector actions, with visible focus and selected states. Avoid relying on colour alone to convey current state.
- **THEME-24:** Users can select `auto`/`light`/`dark` independently of choosing a named theme. The picker must communicate whether selecting a theme changes an `auto` preference.

### 6.5 User theme reload

- **THEME-25:** Provide a documented reload path for valid user theme packs. Native file watching may refresh the registry automatically where reliable, but is not necessary to meet v1 acceptance if manual reload is implemented.
- **THEME-26:** When a selected theme file changes, reapply it without losing the corresponding saved name; on invalid replacement or deletion, use the matching native fallback and report a recoverable warning.
- **THEME-27:** Theme reload must not block the UI event loop with large synchronous directory scans. Bound and validate file sizes or guard the native loader against unreasonable resource consumption.

## 7. Architecture and code ownership

| Component | Required change |
| --- | --- |
| `crates/adoc-engine/src/settings.rs` (L3) | `AppearanceSettings`, parsing, user/workspace precedence, legacy compatibility, warnings and persistence integration; **no GPUI types** |
| `crates/adocfoundry/src/main.rs` (L4) | Initialise theme discovery after `gpui_component::init`, before opening windows |
| `crates/adocfoundry/src/workspace_view.rs` (L4) | Replace the mode-only `apply_theme` path, wire theme selection and adapt `Toggle Theme` |
| `crates/adocfoundry/src/appearance.rs` or `theme.rs` (L4, proposed module) | Theme selection state, registry access, applying selected names, preview rollback, reload and fallback orchestration |
| `crates/adocfoundry/src/menus.rs` and palette actions (L4) | Expose `Theme: Select` and appearance mode choice in existing menus/commands |
| `crates/adocfoundry/src/editor`, `visual`, `preview`, `graph_view.rs` (L4) | Verify semantic palette and highlighter propagation; replace local fixed colours only where needed |
| CLI / LSP / MCP / PDF / Antora layers | **No new dependencies or behaviour** |

Keep theme registry objects and GPUI app-global state solely at L4. The engine owns serialisable preferences, not theme loading. Do not add a crate or a theme-related MCP tool.

**Native API verification gate:** Before implementation, inspect the exact source/API of pinned `gpui-component` 0.8.0. The available documentation describes `ThemeRegistry` and `Theme::apply_config`, but APIs on `main`/`latest` may differ. Use the pinned crate's supported update path, not a method copied from an unpinned release; make its syntax/highlighter and window refresh behaviour an integration test.

## 8. Proposed state model

Three states must remain distinct:

1. **Saved preference** — `mode`, `light_theme`, `dark_theme`, `themes_dir` in user settings.
2. **Resolved effective theme** — current OS-aware mode and matching loaded theme or a native fallback.
3. **Transient selector preview** — candidate theme plus a complete snapshot of pre-preview effective/saved state, with no disk write until commit.

Proposed app-side API (illustrative):

```rust
pub struct ThemeSelection {
    pub mode: AppearanceMode,
    pub light_theme: String,
    pub dark_theme: String,
}

pub enum ThemePickerOutcome {
    Preview { name: String },
    Commit { name: String },
    Cancel,
}

// L4 only: actual native types and method names must match pinned gpui-component.
fn apply_theme_selection(selection: &ThemeSelection, app: &mut gpui::App);
```

The app must use one authoritative selection state shared across windows. Preview has no side effects on persistent settings, and any asynchronous registry reload must not commit a stale preview.

## 9. Performance and failure handling

- Theme selection and live preview must remain responsive and never force a document parse, reindex, re-render of the source content or PDF export.
- Cache discovered theme metadata; refresh only on explicit reload, native watch notification or settings reload.
- Handle invalid JSON, unknown colour tokens, duplicate names, missing fonts and missing theme files without panics; respect the native library's parser and error contracts.
- UI rerendering after a theme change is expected, but avoid excessive repeated updates on every search keystroke.
- No telemetry, no network, no runtime Ruby/Node/Java dependency and no additional unsafe code.
- Avoid logging entire theme contents or exposing unrelated local paths in user-facing errors.

## 10. Testing strategy and acceptance criteria

| Test ID | Acceptance test |
| --- | --- |
| `appearance_legacy_theme_preserved` | Existing `[editor] theme = "auto"`, `"light"`, `"dark"` selects matching appearance with no migration writes |
| `appearance_precedence_and_user_only` | New user setting wins over legacy; workspace override is ignored with warning |
| `appearance_invalid_theme_falls_back` | Missing/invalid named theme uses same-mode native default and preserves preference |
| `theme_registry_lists_named_entries` | Built-in and valid user packs appear once and sort deterministically |
| `theme_picker_previews_without_save` | Cycling through themes updates visible palette but does not write preferences |
| `theme_picker_cancel_restores` | Escape/dismiss reverts mode, colours and highlight palette after multiple previews |
| `theme_picker_commit_persists` | Enter stores only confirmed selection and it restores after a simulated relaunch |
| `theme_auto_follows_system` | System light/dark change selects configured pair; explicit mode ignores subsequent OS changes |
| `theme_toggle_is_persistent` | Existing Toggle Theme action uses named paired theme and saves the new explicit mode |
| `theme_multi_window_refresh` | Workspace, graph, detached preview and mirror windows adopt one active theme |
| `theme_editor_syntax_switches` | Both AsciiDoc and injected source-code highlights change with the selected syntax palette |
| `theme_rendering_isolated` | Changing UI theme leaves AsciiDoc text, diagnostics, HTML/PDF publication settings and content unchanged |
| `theme_invalid_file_does_not_crash` | Malformed JSON, duplicate names, theme removal and reload remain recoverable |
| `theme_reloads_without_busy_loop` | Reload updates a changed valid theme and does not cause repeated update storms |
| `theme_selection_accessible` | Keyboard navigation, active indicator, focus and non-colour cues work in GPUI headless tests |

Use app headless GPUI tests and settings unit tests. Add a small set of visual regression screenshots covering light/dark palettes and source/visual/graph views. Manual QA on macOS, Linux and Windows must include OS appearance changes, configuration directory permissions, high-DPI displays and a detached preview window.

Run at minimum:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask layers
cargo deny check
```

## 11. Work packages

| WP | Deliverable | Dependencies | Definition of done |
| --- | --- | --- | --- |
| **T1** | Native registry and user theme discovery | None | Built-ins and valid user packs discovered; invalid files produce warnings; mode classification is correct |
| **T2** | Appearance settings, fallback and legacy compatibility | T1 | New `[appearance]` values load; `[editor].theme` works; setting saves and reloads without changing PDF behaviour |
| **T3** | Searchable theme selector, preview/commit/cancel and toggle integration | T1, T2 | End-to-end picker behaviour works, persisted selection survives restart, Escape restores |
| **T4** | Cross-window, highlighting, reload, documentation and QA | T1–T3 | All acceptance tests pass across supported platforms; user instructions and a sample theme pack are documented |

Prefer one focused PR per work package. Commit messages and PR titles should follow existing repository conventions, and implementation deviations must be reflected in this PRD.

## 12. Definition of done

A user can select from built-in or user-provided **native `gpui-component` JSON** theme packs, preview and commit a theme through a searchable picker, configure light and dark names in auto mode, and restart the application without losing their preferences. Theme application updates all open desktop surfaces and source highlights, invalid files fail safely, legacy settings still work, and document output is unchanged. The implementation introduces no new crate or Zed compatibility layer.

## 13. Decisions made

The four open decisions were settled when the work was approved:

1. **Settings namespace:** the `[appearance]` table, with `[editor] theme` kept as the legacy source of the mode.
2. **Built-in selection:** a curated set of licensed themes besides the native defaults: Catppuccin (Latte, Frappe, Macchiato, Mocha), Tokyo Night (Night, Storm, Moon), Solarized, Everforest and Flexoki (Light and Dark each), as ported by GPUI Kit (Apache-2.0) from originals under MIT or Apache-2.0. Later the rest of GPUI Kit's themes were checked at their sources, and those whose originals state MIT or Apache-2.0 were added: Gruvbox (MIT in its README and `package.json`; the repository has no licence file), Hybrid, macOS Classic and Mellifluous (Light and Dark each), Alduin, Asciinema (asciinema-player's theme), Jellybeans and Spaceduck (dark), and GPUI Kit's own Aurora Light. Still not bundled: Adventure, Adventure Time and Kibble (no licence known), Fahrenheit and Harper (CC-BY-SA-4.0), Molokai (its licence keeps Monokai's copyright apart), Twilight (none stated) and Ayu (taken from Zed's repository).
3. **Theme directory reload:** an explicit Reload Themes command; the folder is not watched.
4. **Auto-mode theme selection:** choosing a theme while following the system keeps `auto` and makes it that mode's half of the pair; with an explicit mode, choosing a theme of the other mode switches to it.

## 14. Implementation notes

Implemented against `gpui-component` 0.9.0 (#210 upgraded from 0.8.0 before the work began). Where it departs from the specification above:

- **No second registry, but a catalog.** The pinned `ThemeRegistry` can add a theme (`load_themes_from_str`, first name wins) but neither remove nor replace one, and its only reload (`watch_dir`) also watches the folder and drops themes loaded from strings, the bundled ones included. Reload Themes must take away a deleted file's theme and pick up a changed one, so the app keeps `ThemeCatalog` (`crates/adocfoundry/src/appearance.rs`): the registry's native defaults, the bundled sets (`crates/adocfoundry/themes/`, embedded), then the user's files, a name already listed kept as first found with a warning. The registry still supplies the defaults; the chosen themes are applied by setting `Theme::light_theme` and `dark_theme` and calling `Theme::change` (or `Theme::sync_system_appearance` for `auto`), which repaints every window and switches the highlight theme with the colours. THEME-04's "native precedence" is therefore the catalog's: native defaults, bundled, then user files in file-name order.
- **Settings.** `adoc_engine::AppearanceSettings` (`mode`, `light_theme`, `dark_theme`, `themes_dir`) in `Settings::appearance`; `[appearance]` keys are refused from a workspace file with the existing "can only be set in the user settings file" warning. Without `[appearance] mode`, `[editor] theme` gives it; with both, the mode wins and `editor.theme` is reported once as ignored. `EditorSettings::theme` is kept in step with the resolved mode for older readers. `themes_dir` defaults to `themes` beside the user settings file, relative paths resolve against that file's folder and `~/` against the home folder, and the current folder is never searched.
- **Saving.** `appearance.save` (`CommandArgs::Appearance(AppearanceChoice)`, `Engine::save_appearance`) rewrites only `[appearance]`'s `mode`, `light_theme` and `dark_theme` with `toml_edit`, keeping every other line and the comments above existing keys, takes out `[editor] theme` (so the two never conflict after a choice in the app), writes through a temporary file renamed over the settings file, and reloads the settings, whose `SettingsChanged` shows the choice. Without a user settings file, or if the write fails, the choice is shown for the session with a warning (THEME-12).
- **The picker** is gpui-component's `Command` palette in a dialog: highlighting (`on_select`) previews, Enter (`on_confirm`) saves, Escape (`on_cancel`) or any other close (`Dialog::on_close`) restores what was shown, whatever was previewed. While it is open, `ThemeCatalog::previewing` stops a settings reload or a system appearance change from replacing the preview. Each theme's row names its mode, and the chosen ones are marked with a check and the word "chosen".
- **Mode commands.** Appearance: Follow System, Appearance: Light and Appearance: Dark save the mode; Toggle Light/Dark Theme saves the explicit mode opposite the one shown (THEME-15); all are in View and the palette. In `auto` the workspace window observes the system appearance (`observe_window_appearance`), which it did not before.
- **Limits.** User theme files are read with no link followed, at most 64 files of at most 1 MiB, on the UI thread when the catalog is (re)loaded; a file that is not a theme set, too large or unreadable is skipped with a warning, once per message.
- **Tests.** `appearance_legacy_theme_preserved`, `appearance_precedence_and_user_only`, `appearance_invalid_values_warn_and_keep_defaults`, `appearance_saves_only_its_keys` and `appearance_cannot_be_saved_without_a_user_file` (engine settings); `saving_the_appearance_writes_the_user_file_and_reloads` (engine); `theme_registry_lists_named_entries`, `theme_invalid_file_does_not_crash` and `a_chosen_theme_keeps_auto_and_sets_an_explicit_mode` (catalog); `theme_picker_previews_without_save`, `theme_picker_cancel_restores`, `theme_picker_commit_persists`, `theme_toggle_is_persistent`, `theme_editor_syntax_switches`, `appearance_invalid_theme_falls_back` and `theme_reloads_without_busy_loop` (app). Not automated, left to manual QA: `theme_auto_follows_system` (the test platform cannot change the system appearance), `theme_multi_window_refresh` (the theme is one global that `Theme::change` repaints every window from), `theme_selection_accessible` beyond the keyboard tests, and visual regression screenshots. `theme_rendering_isolated` holds by construction: appearance is applied only in the app crate and never reaches the engine's documents, exports or `[pdf]` settings.
