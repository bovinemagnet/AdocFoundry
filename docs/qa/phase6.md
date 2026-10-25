# Phase 6 — Manual QA Checklist

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | Phase 6 acceptance (spec §11) that automated tests cannot cover |

Run each item on macOS 13+, Windows 10+ (x64 and ARM64) and Ubuntu 22.04+ from the release artefacts, unless the item names a platform. Record the build, the platform and the result in the table at the end.

Automated coverage for reference:

- **Inline formulas:** their measures (`inline_math_records_its_baseline`, `inline_stem_carries_its_metrics`), the gap that holds their place (`inline_stem_reserves_its_width`), and drawing them in the preview and the visual editor (`preview_draws_inline_stem_in_the_line`, `visual_editor_draws_inline_stem`, `formulas_take_the_place_of_their_macros`).
- **MCP over HTTP:** `crates/adoc-mcp/tests/http.rs`, and the layering rule `http_servers_are_allowed_only_in_adoc_mcp`.
- **Signatures:** the smoke test's checks (`packaging/test_smoke_test.py`), and the release jobs' own `codesign`, `spctl`, `stapler validate` and `Get-AuthenticodeSignature`.

## Setup

1. The release artefacts for the platform, from the GitHub release or the release workflow's run, built with the signing secrets set.
2. `git clone --recurse-submodules https://github.com/bovinemagnet/AdocFoundry.git` for the fixtures.
3. An MCP client that connects over Streamable HTTP.

## Inline STEM

- [ ] A page with inline AsciiMath and LaTeX (`stem:[sqrt(4) = 2]`, `stem:[sum_(i=1)^n i]`, `latexmath:[\frac{a}{b}]`) in the preview, in the light and the dark theme:
  - each formula sits on the text's baseline at the text's size;
  - lines with a formula are no taller than lines without;
  - a formula moves whole to the next line when the line wraps;
  - hovering shows the source.
- [ ] The same page in the visual editor:
  - formulas are drawn as in the preview;
  - the caret steps over a formula in one step;
  - typing on either side of it writes beside the macro;
  - copying a selection holding a formula copies its source;
  - an IME composition next to a formula shows in place.
- [ ] A formula in a heading and in a table cell, in both views.

## MCP over HTTP

- [ ] `adoc-mcp <docs> --http 127.0.0.1:8765` prints its address and a token. An MCP client configured with `url` and `headers` (`Authorization: Bearer <token>`) lists the tools and reads a page.
- [ ] Without the header, or with a wrong token, the client is refused.
- [ ] `--http 0.0.0.0:8765` is refused at start-up. Ctrl-C stops the server.

## Signed releases

- [ ] macOS: a downloaded DMG opens without a Gatekeeper prompt; the app dragged to Applications opens without one, offline too; `spctl -a -vv /Applications/AdocFoundry.app` reports `accepted` and `source=Notarized Developer ID`.
- [ ] macOS: the tools in `Contents/MacOS` and in their own archives run from a terminal, and `codesign -dv` shows the Developer ID and the hardened runtime.
- [ ] Windows 11: the MSI installs with no SmartScreen warning, and the executables' Properties show a valid digital signature.

## Updates

- [ ] macOS and Windows: an older signed build, pointed at a feed offering a newer release, says once that it is available. After **Update** and confirming, it installs the newer release and relaunches, or, on Windows, runs its installer.
- [ ] The notice is not shown again after the app restarts. With `[updates] check = false` there is no check at start-up, and the command still checks.
- [ ] An ad-hoc or unsigned build, a deb, an rpm and a portable zip only notify, with a link to the release.
- [ ] The AppImage's update information is found by AppImageUpdate.

## Results

| Date | Build | Platform | Result |
|---|---|---|---|

## Known limitations found
