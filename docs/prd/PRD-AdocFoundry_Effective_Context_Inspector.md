# AdocFoundry — Effective-Context Inspector (issue #99)

Author: Paul Snow · Version 0.0.0 · Status: implemented

## 1. Problem

The same partial can render differently in different pages, because an attribute's value depends on the playbook, the component's `antora.yml`, the page header, the declarations before the reference (in the page and in what it includes) and Antora's hard and soft sets. Hover gives one value and one source, found by "first definition wins", which is not how Antora and Asciidoctor decide.

## 2. Goal

An inspector for the attribute at the caret — a `{name}` reference, an attribute entry `:name: value`, or a name in `ifdef::`/`ifndef::` — showing:

- its effective value at the caret (set, with its value; unset; or never set);
- every place that sets or unsets it, with the file and line, in precedence order, each marked as the one that wins, overridden by a later one, ignored because a higher scope hard-sets it, or after the caret;
- inside a partial, the value at the caret in each page that includes it.

**Done when** (acceptance test `inspector_shows_component_and_page_header_and_which_wins`): on an attribute set in both `antora.yml` and a page header, the inspector shows both, says which wins, and links to each.

## 3. Rules followed

From Antora's documentation ([site attributes](https://docs.antora.org/antora/latest/playbook/asciidoc-attributes/), [component attributes](https://docs.antora.org/antora/latest/component-attributes/), [page attributes](https://docs.antora.org/antora/latest/page/define-and-modify-attributes/)):

1. Scopes, highest first: playbook, component version (`antora.yml`), page. (The CLI is not seen by the editor.)
2. A value in the playbook or `antora.yml` is **hard set** unless it ends in `@` (**soft set**). `false` **soft unsets**; `~` **hard unsets**.
3. A component attribute applies only when the playbook does not name the attribute, or names it soft (set or unset).
4. A page declaration (`:name: value`, `:name!:`, `:!name:`) applies only when the attribute that reaches the page from above is soft or absent. Antora's intrinsic page attributes (`page-component-name`, `page-module`, `page-component-version`, the family directories) are hard.
5. Within the page, declarations apply in document order, an include's declarations at the place of its `include::` line; the value at the caret is the last that applies before it. Asciidoctor's built-ins are defaults any scope may change.

Not traced (documented as limits): the CLI and environment, `ifdef`/`ifeval` around a declaration (a declaration inside a conditional is listed as written), include tag filters, and values written with other attributes in them (shown as written).

## 4. Design

- **`adoc-antora`**: `Playbook::attribute_entries` and `ComponentDescriptor::attribute_entries` — every key under `asciidoc.attributes` as an `AttributeEntry { value: Option<String>, soft: bool, range }` (the key's range in the YAML), unsets included. `asciidoc_attributes` keeps its meaning.
- **`adoc-lang`** `effective.rs`: `attribute_name_at(text, offset)` and `effective_attribute(ws, path, document, offset) -> Option<EffectiveAttribute>` (`name`, `range`, `value: Effective`, `steps: Vec<Step>` each with `origin`, `location`, `setting`, `outcome`, and `includers: Vec<IncluderValue>` for a partial: each top page reached through `includes_of`, with the value there and the step that wins). The walk expands includes in order through the same resolution as `composed_files`, bounded by `MAX_INCLUDE_DEPTH`. Hover takes its value and source from the winning step, so the two never disagree.
- **`adoc-engine`**: `Engine::effective_attribute(id, offset)` and the command `navigate.inspect_attribute` (`CommandArgs::Offset`, `CommandOutcome::Attribute`).
- **`adocfoundry`**: Inspect Attribute (palette, Go menu) opens a panel below the editor, as Find References does; each row opens its location.
- **`adoc-mcp`**: `get_page_context` gives each attribute's effective value at the end of the page's header and its winning source, keeping `declared`.

## 5. Work packages

1. Attribute entries with ranges in `adoc-antora` → unit tests on playbook and descriptor.
2. The resolver in `adoc-lang` with fixture `tests/fixtures/effective-attributes` → the acceptance test and tests for each rule in §3.
3. Hover from the resolver → existing hover tests still pass.
4. Engine command and query → `tests/` engine test.
5. App panel → `ui_tests.rs` test.
6. MCP `get_page_context` effective values → contract test updated.
7. README, user documentation, this spec's Implementation Notes.

## 6. Implementation Notes

- The source editor has no context menu of its own, so Inspect Attribute is in the Go menu and the palette only, with no default key.
- Go to Definition on an attribute reference now leads to the winning step's place (the YAML key in `antora.yml` or the playbook, where it used to open the file at its start), so definition, hover and the inspector agree.
- Hover on an attribute that is unset at the reference now says so (`is unset`, with where); it used to show nothing.
- `completion` still offers every attribute name through `attributes::attribute_definitions`, whose first-definition order only ranks the offer.
- The rows are worded by the engine (`AttributeInspection`, `InspectionRow`) so every front end shows the same text; the app draws them as they are.
- `get_page_context` keeps `declared` unchanged and adds `effective` at the end of the header (the first blank line); `effective_values` is `at_header_end`.
