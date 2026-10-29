# adoc-ls

`adoc-ls` is a language server for AsciiDoc, with first-class support for documentation laid out the [Antora](https://antora.org) way. It runs the same engine as the AdocFoundry desktop editor and `adocfoundry-cli`, so an editor using it reports exactly what they report. Nothing else is needed at run time: no Ruby, Node.js or Java.

## Features

- **Diagnostics** for unresolved xrefs, anchors, includes and images, duplicate anchors, and Antora problems: unknown modules and resources, malformed resource ids, unreadable `antora.yml` descriptors, unresolved navigation entries and versions not on their branch. They are republished for every open page a change affects. Diagnostics are conservative: no diagnostic rather than a false positive.
- **Completion** of xref, include and image targets, anchors and attribute names.
- **Hover**, **Go to Definition** and **references**: on a page, the pages that link to it and the includes that include it; on an anchor or a reference to one, every reference to that anchor.
- **Document symbols** for the title and sections, with navigation files outlined as their navigation tree, and **workspace symbols** for titles, sections, anchors and attributes.
- **Renaming**: renaming an `.adoc` file in the editor first rewrites the xrefs and nav entries that name it; renaming an anchor id renames it and its references.
- **Antora resolution** across components, versions and modules, including aliases.
- **HTML preview** through code actions, one-off or live (re-rendered on save), opened in the system browser.
- **PDF export** of a page, or of a component version as a book, in builds with Typst.

## Installation

Release archives for Linux, macOS and Windows on x86-64 and AArch64 are on the [AdocFoundry releases page](https://github.com/bovinemagnet/AdocFoundry/releases) as `adoc-ls-<version>-<platform>`. They are built with Typst and Git. Unpack the binary somewhere on your `PATH`.

To build it yourself, with a Rust toolchain:

```sh
cargo install --git https://github.com/bovinemagnet/AdocFoundry adoc-ls
```

The default build is lean, without Typst or Git. For PDF export, typeset STEM in the preview and Git:

```sh
cargo install --git https://github.com/bovinemagnet/AdocFoundry adoc-ls --features typst,git
```

Check the install with `adoc-ls --version`.

## Usage

```text
adoc-ls --stdio      serve LSP over standard input and output
adoc-ls --version    print the version
adoc-ls --help       print usage
```

The workspace is the client's workspace folders, or its root URI when it sends no folders.

### Zed

The [AsciiDoc and Antora](https://github.com/bovinemagnet/zed-asciidoc-antora) extension (`asciidoc-antora`) starts `adoc-ls` for you. It uses an `adoc-ls` on `PATH` when there is one, and otherwise downloads it from the latest release. Zed's `lsp.adoc-ls.binary` setting can name another binary instead.

### Neovim

With Neovim 0.11 or later, for example in `init.lua`:

```lua
vim.lsp.config('adoc_ls', {
  cmd = { 'adoc-ls', '--stdio' },
  filetypes = { 'asciidoc' },
  root_markers = { 'antora.yml', 'antora-playbook.yml', '.git' },
})
vim.lsp.enable('adoc_ls')
```

### Other editors

Any LSP client can run `adoc-ls --stdio` for AsciiDoc files (`.adoc`, `.asciidoc`, `.ad`). Point the workspace root at the folder holding your Antora playbook or components, so cross-component references resolve.

## Commands

The server advertises these through `workspace/executeCommand`; the preview commands are also offered as code actions on any AsciiDoc page.

| Command | What it does |
| --- | --- |
| `adoc.renderPreview` | Renders the page to HTML once and opens it. |
| `adoc.renderLivePreview` | Opens a preview that re-renders whenever the page is saved. |
| `adoc.stopLivePreview` | Stops the live preview. |
| `adoc.checkWorkspace` | Publishes the diagnostics of every file in the workspace, open or not. |
| `adoc.exportPdf { uri, component?, output, theme?, standards? }` | Writes a PDF of the page, or of a component version as a book. Only in builds with Typst. |

The preview renders through Asciidoctor when `tools.asciidoctor` is set or `asciidoctor` is on `PATH`, otherwise through the engine, with diagrams and typeset STEM. Preview files are written to `adoc-ls-preview` in the system's temporary directory.

## Settings

`adoc-ls` reads the same TOML settings files as the desktop editor: the user file `adocfoundry/adocfoundry.toml` in the platform configuration directory (`~/.config` on Linux, `~/Library/Application Support` on macOS, `%APPDATA%` on Windows), and a workspace's `.adocfoundry.toml` at its root. Both are optional. The keys most relevant here:

```toml
[tools]
asciidoctor = ""      # optional absolute path for the HTML preview; discovered on PATH if empty

[git]
enabled = true        # off in adoc-ls unless the user file turns it on

[pdf]
theme = "default"     # a bundled theme, a name in themes_dir, or a .yml file
```

Git is opt-in for `adoc-ls`: it looks for no repository unless the user settings file sets `[git] enabled = true`. The full list of settings is in the [AdocFoundry README](../../README.md#settings).

## Diagnostic codes

| Code | Meaning |
| --- | --- |
| `adoc.unresolved-xref-file` | An xref names a page that does not exist. |
| `adoc.unresolved-anchor` | An xref names an anchor the target page does not define. |
| `adoc.unresolved-include` | An include target cannot be found. |
| `adoc.unresolved-image` | An image target cannot be found. |
| `adoc.duplicate-anchor` | An anchor id is defined more than once on a page. |
| `adoc.antora.unknown-module` | A resource id names a module the component version does not have. |
| `adoc.antora.unknown-resource` | A resource id names a resource that does not exist. |
| `adoc.antora.invalid-family` | A resource id names an unknown family, such as `partials$`. |
| `adoc.antora.invalid-coordinate` | A resource id is malformed. |
| `adoc.antora.invalid-descriptor` | An `antora.yml` descriptor cannot be read. |
| `adoc.antora.resolved-via-alias` | A reference resolves only through a page alias. |
| `adoc.antora.nav-unresolved` | A navigation entry does not resolve. |
| `adoc.antora.version-not-on-branch` | A component version is not on the branch being edited. |

`adocfoundry-cli check` reports the same codes, which suits CI.

## Licence

Apache-2.0, as the rest of [AdocFoundry](https://github.com/bovinemagnet/AdocFoundry).
