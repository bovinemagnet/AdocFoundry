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

- [x] `adoc-mcp <docs> --http 127.0.0.1:8765` prints its address and a token. An MCP client configured with `url` and `headers` (`Authorization: Bearer <token>`) lists the tools and reads a page. Checked with a Streamable HTTP client (`curl`) against the `phase-6` debug build on `tests/fixtures/references`: `initialize` answered with `serverInfo` `adoc-mcp` and a session id, `tools/list` gave the nine read-only tools, and `read_page` returned `index.adoc` (2026-10-25).
- [x] Without the header, or with a wrong token, the client is refused: both answered 401 (2026-10-25).
- [x] `--http 0.0.0.0:8765` is refused at start-up (exit 1, "not a loopback address"). Ctrl-C (SIGINT) stops the server (2026-10-25).

## Signed releases

- [ ] macOS: a downloaded DMG opens without a Gatekeeper prompt; the app dragged to Applications opens without one, offline too; `spctl -a -vv /Applications/AdocFoundry.app` reports `accepted` and `source=Notarized Developer ID`.
- [ ] macOS: the tools in `Contents/MacOS` and in their own archives run from a terminal, and `codesign -dv` shows the Developer ID and the hardened runtime.
- [ ] Windows 11: the MSI installs with no SmartScreen warning, and the executables' Properties show a valid digital signature.

## Updates

- [ ] macOS and Windows: an older signed build, pointed at a feed offering a newer release, says once that it is available. After **Update** and confirming, it installs the newer release and relaunches, or, on Windows, runs its installer.
- [ ] The notice is not shown again after the app restarts. With `[updates] check = false` there is no check at start-up, and the command still checks.
- [ ] An ad-hoc or unsigned build, a deb, an rpm and a portable zip only notify, with a link to the release.
- [ ] The AppImage's update information is found by AppImageUpdate.

## Image viewer (#125)

Open `tests/fixtures/image-viewer` as the workspace. Check on macOS, Windows and Linux, on a HiDPI display and a standard one.

- [ ] From Modules (gallery 1.0 › ROOT › images) and from Files, a click shows `logo.png`, `screenshot.jpg` and `architecture.svg` in the image viewer, in one italic preview tab; a double-click keeps it. Nothing opens in another application.
- [ ] `Photo Café.PNG`, `still.gif`, `still.webp`, `bitmap.bmp`, `icon.ico` and `scan.tiff` show; `oriented.jpg` stands upright (2 × 4); `broken.png` and `notes.png` say they are damaged or not an image; `photo.heic` says it cannot be shown and Open Externally opens it.
- [ ] An image opens whole and centred and a small one is not enlarged; Fit to Window (`Cmd`/`Ctrl`+`9`) fills the canvas and follows a window resize; a chosen zoom stays through a resize; Actual Size (`Cmd`/`Ctrl`+`0`) is sharp, one image pixel to one screen pixel, on a HiDPI display.
- [ ] Scrolling and dragging reach every edge of a large screenshot; the zoom keys work only with focus in the viewer, and the editor's own keys are untouched in a page.
- [ ] The checkerboard behind `logo.png` is quiet in the light and the dark theme.
- [ ] Replacing an open image in another application shows the new pixels and size; deleting it shows the last pixels marked stale.
- [ ] Ctrl/Cmd-clicking `image::logo.png[]` in `index.adoc` opens the image; Back returns to the page.
- [ ] With an image active, the Source, Visual and Split buttons are gone, and Save and the formatting commands do nothing; Save All saves edited pages and leaves the image file unchanged.

## Doctor report (#105)

Check on macOS, Windows and Linux (Wayland and X11), from a package, not a source build.

- [ ] `adocfoundry-cli doctor` and `adocfoundry-cli doctor <workspace>` print every section; the OS line names the system and its version; each installed tool shows its path and version, and a missing one says "not found on PATH".
- [ ] With `[tools] asciidoctor` set to a file that does not exist, the report says so; with `[pdf] fonts_dir` pointing nowhere and a theme that names fonts there, the PDF section lists the missing fonts.
- [ ] Help › Show Diagnostics opens the report without freezing the window while the tools answer; the Display section names the graphics backend (and the GPU and display server on Linux) and whether the editor font is installed; Copy puts the whole report on the clipboard, and it pastes into a new GitHub issue's Diagnostics field intact.

## Results

| Date | Build | Platform | Result |
|---|---|---|---|
| 2026-10-25 | `phase-6` branch, debug | macOS 26 (arm64) | MCP over HTTP: every item passes. `cargo test --workspace` (721 tests), clippy, `cargo deny` and `cargo xtask layers` pass. CI on PR #113 passes on Ubuntu, macOS and Windows. Inline STEM, signing and updates are not yet checked by hand: signing needs the Apple and Azure secrets, updating needs two signed releases. |

## Known limitations found
