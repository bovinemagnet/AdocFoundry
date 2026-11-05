# Font Sets — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | Font sets where automated tests cannot reach: real font loading and text as drawn on each platform. GPUI's test platform loads no fonts |

Run each item on macOS 13+, Windows 10+ and Ubuntu 22.04+, on a machine or account without these fonts installed, so that what is drawn can only be what ships. Record the build, the platform and the result in the table at the end.

## Automated coverage, for reference

- **Engine:** the tests in `crates/adoc-engine/src/font_sets.rs` (the default, each built-in set, `[editor]` settings replacing the source role, what a workspace may set, sets of your own and bad ones, saving, the command).
- **Files:** the tests in `crates/adocfoundry/src/fonts.rs` (each file declares the family it is listed under, each family has the four basic styles, each built-in set but Current ships its fonts, nothing ships unused).
- **App:** `workshop_draws_every_role_by_default`, `the_reading_size_follows_the_font_scale`, `current_puts_back_the_fonts_the_app_started_with`, `a_set_overrides_a_themes_fonts_and_current_keeps_them`, `a_set_naming_a_missing_font_warns_and_falls_back`, `the_font_picker_previews_without_saving` and `the_font_picker_saves_the_set_confirmed` in `crates/adocfoundry/src/ui_tests.rs`.

## Checks

1. **Default.** With no settings file, the interface is in IBM Plex Sans, the source editor and code blocks in IBM Plex Mono, the preview's prose in IBM Plex Sans. Help › Show Diagnostics lists each as `(bundled)`.
2. **Styles.** In each of Workshop, Reading Room and Legible, a page with `*bold*`, `_italic_`, `*_both_*` and semibold panel titles draws true bold and italic faces, not slanted or smeared regular ones. On Windows, check the semibold panel titles in particular: the semibold files have a family name of their own.
3. **Picker.** View › Select Font Set…: moving through the list redraws the interface, source editor, preview and visual editor at once; Escape puts back what was shown; Enter keeps the set, across a restart.
4. **Source editor.** In each set, the caret, selection and gutter stay aligned with the text after switching, and an IME composition draws in place.
5. **Inline formulas.** In each set, in the preview and the visual editor, inline STEM sits on the text's baseline with no overlap or wide gap, at `[appearance] font_scale` 1 and 1.5.
6. **Current.** `font_set = "current"` draws what 0.6.0 drew: the system's interface font, and JetBrains Mono in the source editor if installed.
7. **Startup.** Compare the time to the first frame with 0.6.0's: loading the shipped fonts adds nothing noticeable.

## Results

| Build | Platform | Result | Notes |
|---|---|---|---|
| | | | |
