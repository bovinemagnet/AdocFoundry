# adoc-ls

`adoc-ls` is a language server for AsciiDoc, with first-class support for documentation laid out the [Antora](https://antora.org) way. It runs the same engine as the AdocFoundry desktop editor and `adocfoundry-cli`, so an editor using it reports exactly what they report. Nothing else is needed at run time: no Ruby, Node.js or Java.

## Features

- **Diagnostics** for unresolved xrefs, anchors, includes and images, duplicate anchors, and Antora problems: unknown modules and resources, malformed resource ids, unreadable `antora.yml` descriptors, unresolved navigation entries and versions not on their branch. They are republished for every open page a change affects. Diagnostics are conservative: no diagnostic rather than a false positive.
- **Completion** of xref, include and image targets, anchors and attribute names.
- **Hover**, **Go to Definition** and **references**: on a page, the pages that link to it and the includes that include it; on an anchor or a reference to one, every reference to that anchor.
- **Document symbols** for the title and sections, with navigation files outlined as their navigation tree, and **workspace symbols** for titles, sections, anchors and attributes.
- **Renaming**: renaming an `.adoc`, `.asciidoc` or `.ad` file in the editor first rewrites the xrefs and nav entries that name it, or answers with the reason it cannot (a file naming it changed on disk and is not read again yet, say), so the file is never moved with its links left behind; renaming an anchor id renames it and its references.
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

The editor owns the pages it has open: `adoc-ls` works from the buffers the editor sends, and a change to an open page's file on disk never replaces them. A typed change's diagnostics are published once the server has diagnosed the new text, when typing pauses.

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
| `adoc.checkWorkspace` | Publishes the diagnostics of every file in the workspace, open or not, and clears those of a closed file an earlier check reported once they are fixed. |
| `adoc.exportPdf { uri, component?, output, theme?, standards? }` | Writes a PDF of the page, or of a component version as a book. Only in builds with Typst. |

The preview renders through Asciidoctor when `tools.asciidoctor` is set or `asciidoctor` is on `PATH`, otherwise through the engine, with diagrams and typeset STEM. Preview files are written to a folder of the server's own in the system's temporary directory (`adoc-ls-preview-` and a random suffix, readable only by you on Unix, removed when the server stops), one per page, named after it (letters, digits, `-` and `_` only) and a hash of its path. On Windows they are opened with `rundll32 url.dll,FileProtocolHandler`, never through `cmd.exe`. The browser is started apart from the server's standard input and output, which carry the protocol, and the server does not wait for it. A render through Asciidoctor is stopped after 60 seconds.

The spelling commands `adoc.spelling.addToWorkspace` and `adoc.spelling.addToUser`, run from a misspelling's quick fixes, refuse a range that is no longer a misspelt word because the page changed since the fix was offered.

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

A settings file that does not parse, or a workspace that cannot be opened, is reported with a `window/showMessage` warning naming the file; a user settings file that does not parse gives the default settings.

Git is opt-in for `adoc-ls`: it looks for no repository unless the user settings file sets `[git] enabled = true`. `[editor] auto_save` has no effect here: the editor owns its buffers and saves them, and `adoc-ls` never writes a file on its own. The full list of settings is in the [AdocFoundry README](../../README.md#settings).

## Diagnostic codes

| Code | Meaning |
| --- | --- |
| `adoc.unresolved-xref-file` | An xref names a page that does not exist. |
| `adoc.unresolved-anchor` | An xref names an anchor the target page does not define. |
| `adoc.unresolved-include` | An include target cannot be found. |
| `adoc.unresolved-image` | An image target cannot be found. |
| `adoc.duplicate-anchor` | An anchor id is defined more than once on a page. |
| `adoc.broken-code-span` | A code span does not close where it seems to: its closing backtick is followed by an apostrophe or a letter. |
| `adoc.antora.unknown-module` | A resource id names a module the component version does not have. |
| `adoc.antora.unknown-resource` | A resource id names a resource that does not exist. |
| `adoc.antora.invalid-family` | A resource id names an unknown family, such as `partials$`. |
| `adoc.antora.invalid-coordinate` | A resource id is malformed. |
| `adoc.antora.invalid-descriptor` | An `antora.yml` descriptor cannot be read. |
| `adoc.antora.missing-key` | An `antora.yml` has no `name` or no `version`. |
| `adoc.antora.unknown-key` | An `antora.yml` has a key Antora does not use. |
| `adoc.antora.missing-nav-file` | A `nav` entry in `antora.yml` names a file that does not exist. |
| `adoc.antora.invalid-attribute-value` | An `asciidoc.attributes` value in `antora.yml` is a list or mapping. |
| `adoc.antora.unresolved-start-page` | The playbook's `site.start_page` names no page in the workspace. |
| `adoc.antora.resolved-via-alias` | A reference resolves only through a page alias. |
| `adoc.antora.nav-unresolved` | A navigation entry does not resolve. |
| `adoc.antora.version-not-on-branch` | A component version is not on the branch being edited. |

`adocfoundry-cli check` reports the same codes, which suits CI.

## Licence

Apache-2.0, as the rest of [AdocFoundry](https://github.com/bovinemagnet/AdocFoundry).
