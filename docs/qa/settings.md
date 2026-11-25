# Manual QA: key bindings and the settings window

Custom key bindings (#79) and the settings window (#80). The tests drive both on a test platform; these checks are what they cannot cover.

Start from a settings file of your own with comments in it, and keep a copy.

## Key bindings

- [ ] `"document.close_all" = "cmd-k x"` under `[keymap]`, saved in AdocFoundry, works at once; taking the line out and saving gives the default back.
- [ ] The same, saved in another editor while a workspace is open.
- [ ] The palette shows the keys in force, and the menus too (a chord's first stroke); on macOS the app menu shows Quit's and Settings'.
- [ ] With Undo moved off Cmd/Ctrl-Z, Cmd/Ctrl-Z does nothing in the source and the visual editor.
- [ ] A binding with a letter whose place differs on a layout that is not QWERTY works.
- [ ] An unknown command id shows a notification, at launch too, and the app starts.
- [ ] Windows and Linux: `cmd` is Control.

## Settings window

- [ ] Cmd/Ctrl-, opens it, and again brings it to the front; the macOS app menu's Settings… does too.
- [ ] Every page reads well at the window's smallest useful size, in a light and a dark theme, and at `font_scale` 1.5.
- [ ] Search finds a setting by its title, description and id.
- [ ] Changing the font size, wrap, tab width, theme and font set shows in the editor at once.
- [ ] The file keeps its comments and order; a file that is a symbolic link stays one.
- [ ] Typing in a text field does not lose the caret or characters as it is saved.
- [ ] Tab moves between the fields, and a screen reader names each.
- [ ] With a workspace whose `.adocfoundry.toml` sets `editor.font_size`, the field says so and cannot be changed.
- [ ] With the settings file edited and unsaved in a tab, a change is refused and says why.
- [ ] Closing the window, and quitting, just after typing keeps what was typed.

## Results

| Date | Build | Platform | Result | Notes |
|---|---|---|---|---|
