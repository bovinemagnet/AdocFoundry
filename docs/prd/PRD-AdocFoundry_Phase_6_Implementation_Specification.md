# PRD: AdocFoundry — Phase 6 Implementation Specification

| | |
|---|---|
| Author | Paul Snow |
| Version | 0.0.0 |
| Date | 2026-10-25 |
| Status | Draft |
| Scope | Phase 6: inline STEM in the preview's text flow, MCP over HTTP, signed releases, and updates |
| Depends on | `docs/design/DESIGN-AdocFoundry_Architecture.md`, Phases 1 to 5 as released in 0.2.1, the issues labelled `phase 6` (#54 to #58) |

## 1. Document Purpose

Phase 5 made AdocFoundry releasable and left four things for later: inline formulas typeset in the line, an HTTP transport for the MCP server, signed packages, and updates. Phase 6 does those four. It adds no new crate and no new editing feature.

Phase 6 must provide:

- **Inline STEM in the text flow (#57):** `stem:[sqrt(4) = 2]` drawn as its typeset formula in the line, in the preview and the visual editor, on the text's baseline, without making the line noticeably taller.
- **MCP over HTTP (#56):** `adoc-mcp --http <addr>` serves MCP's Streamable HTTP transport on a loopback address, to clients that hold its token.
- **Signed releases (#54):** a macOS app signed with a Developer ID, using the hardened runtime, notarised and stapled. On Windows, the executables and the MSI are signed with Authenticode.
- **Updates (#55):** the app notices a newer release and says so once. With the user's consent, it updates itself on macOS and Windows. The AppImage carries update information for AppImageUpdate.
- **The specification itself (#58):** this document, with the phase added to the design's phase table and the README.

## 2. Decisions

| Question | Decision | Consequence |
|---|---|---|
| Order | inline STEM, then MCP over HTTP, then signing, then the update check, then updating in place | The two packages that CI can test fully land first. Signing comes before updating, so the updater can check what it installs. |
| Inline formulas in GPUI's text | the formula's place in the text is held by a gap: spaces measured to the formula's width. The element paints the formula's image over the gap, aligned on the line's baseline | `StyledText` cannot hold an element, but its layout gives each index's position and the line's ascent. Wrapping, hit testing and links keep working, since the text is still one string. |
| A formula's size and baseline | `adoc-pdf` gains `render_math` returning the SVG together with its width, height and baseline in points, taken from Typst's frame. The engine's STEM provider records them, and `ViewInline::Stem` carries them | The app needs a formula's size when it lays out text, before the image has loaded. The baseline comes from Typst, not from a guess. |
| Formula scale | an inline formula is drawn at the size of the text beside it (the render size, 13 pt, scaled by the ratio of the body text size to 13). It is shrunk to fit only if it would exceed 1.5 line heights | `sqrt(4) = 2` and similar keep the line's height. A tall fraction is drawn smaller rather than pushing the lines apart. |
| What copy and screen readers get | the formula's source, as before: it stays the formula's tooltip and accessible label. The gap is never copied as spaces | Matches #57. The preview has no text selection today, so copying is a visual-editor concern, where the atom already copies its source. |
| HTTP server | rmcp's `transport-streamable-http-server` feature, served by `hyper` 1.12.0 and `hyper-util` 0.1.21 with only their server and HTTP/1 features, on the current-thread runtime. Both are already in the tree | No new crate enters the lock file. No client feature of hyper is compiled into `adoc-mcp`. |
| HTTP exposure | `--http` accepts only loopback addresses (`127.0.0.1`, `::1`, `localhost`). Every request needs `Authorization: Bearer <token>`, compared in constant time. The token comes from `ADOC_MCP_TOKEN` or is generated, and is printed to standard error at start-up. rmcp's `Host` check (loopback only) is kept | A local web page cannot reach the server through DNS rebinding, and another local user cannot call it without the token. |
| HTTP and the layering check | `NETWORK_CRATES` splits into client crates (`reqwest`, `ureq`, `isahc`, `curl`, `attohttpc`, `surf`, `gpui-pre-reqwest`) and `hyper`. `hyper` is allowed in `adoc-mcp` (`NETWORK_SERVER_ALLOWED`) only when `cargo tree -p adoc-mcp -e features` shows none of hyper's or hyper-util's `client` features | Matches #56's rule: a server listener in `adoc-mcp`, and an HTTP client nowhere but the app. |
| macOS signing | a Developer ID Application certificate and a notarisation app password in the release workflow's secrets. The certificate is imported into a temporary keychain. Each Mach-O in the bundle is signed separately with `--options runtime --timestamp`, without `--deep`. The app is notarised and stapled, then the DMG is signed, notarised and stapled | `spctl -a -vv` accepts the app, and an app copied out of the DMG passes Gatekeeper offline too. |
| Windows signing | Azure Trusted Signing (`azure/trusted-signing-action`) signs the four executables before the MSI is built, then the MSI. The secrets are an Azure tenant, client id and secret, plus the account and profile names | Authenticode certificates have been issued on hardware or cloud key stores since 2023, so a PFX file in a secret is no longer an option. Trusted Signing is the cloud key store GitHub Actions can reach. |
| Unsigned builds | both platforms sign only when their secrets are present. Otherwise they build exactly as in 0.2.1 (ad hoc on macOS, unsigned on Windows), and the job says so in its summary | A fork, or a release cut before the certificates exist, still builds. |
| Update feed | `https://api.github.com/repos/bovinemagnet/AdocFoundry/releases/latest`, which excludes drafts and pre-releases. Each release also publishes `SHA256SUMS`, written by the `publish` job | One request per check. The checksum file lets the updater verify a download before it trusts the signature. |
| How the app fetches | the system's `curl` (`/usr/bin/curl` on macOS, `%SystemRoot%\System32\curl.exe` on Windows, `curl` on `PATH` on Linux), run on a background thread. With no `curl`, there is no update check | Both systems ship `curl` with their own TLS, so no TLS stack is compiled into the app. All network access stays in `adocfoundry`, as the layering rule requires. |
| Where update logic lives | `adoc-engine`'s `updates` module: parsing the feed, comparing versions, deciding whether a check is due, choosing the package, verifying checksums, and the state file. It makes no request itself | Everything that can be tested headlessly is. The app does only the fetching, the asking and the installing. |
| How often, and the switch | at start-up, at most once a day, when `[updates] check = true` (the default). The last check time and the last version announced are kept in `update-state.toml` beside the user settings file. A command checks on demand | "Says so once" survives restarts. The request sends nothing but the app's `User-Agent` (`AdocFoundry/<version>`), which is no telemetry, and the README says how to switch it off. |
| Updating in place | macOS: the DMG is downloaded and checked against `SHA256SUMS`. The new app must pass `codesign --verify --strict` and `spctl -a` and have the running app's Team ID. It replaces the running bundle, and the app relaunches. Windows: the MSI is downloaded and checked against `SHA256SUMS`, and must have a valid Authenticode signature from the running executable's signer. The app quits and `msiexec /i … /passive` installs it. An ad-hoc or unsigned running build only notifies | An update is never less trusted than the build it replaces. |
| Linux | the AppImage is built with `UPDATE_INFORMATION` (`gh-releases-zsync|bovinemagnet|AdocFoundry|latest|AdocFoundry-*-<arch>.AppImage.zsync`) and its `.zsync` file is published. When running as an AppImage, the notice points to AppImageUpdate. deb, rpm and Flatpak installs are notified with a link | Their package managers own updates, as #55 says. |

## 3. Architecture

```
L4  adocfoundry   (inline formula element; update notice and installer; curl)
    adoc-mcp      (--http: Streamable HTTP on hyper, server features only)
L3  adoc-engine   (updates module; formula metrics from the STEM provider)
L2  adoc-render   (ViewInline::Stem carries a rendered formula's metrics)
    adoc-pdf      (render_math: SVG, width, height, baseline)
```

- No crate is added, and no dependency direction changes.
- `xtask/src/layers.rs` gains the client/server split and its check (§5). The ladder in `CLAUDE.md` and the developer documentation are updated in the same PR.
- Any new dependency is pinned exactly, already in `Cargo.lock` where possible, and passes `cargo deny check`.

## 4. Inline STEM in the text flow (WP 6.1, #57)

```rust
// adoc-pdf
pub struct RenderedMath { pub svg: String, pub width: f64, pub height: f64, pub baseline: f64 }   // points; baseline from the top
pub fn render_math(math: &TypstMath, block: bool, size: f64) -> Result<RenderedMath, PdfError>;
// render_math_svg stays, as render_math(…).svg

// adoc-render
pub struct FormulaImage { pub path: PathBuf, pub width: f32, pub height: f32, pub baseline: f32 }   // points
ViewInline::Stem { latex: bool, source: String, rendered: Option<FormulaImage> }
```

- **Metrics:** `render_math` takes the baseline from the typeset page's frame. The 1 pt margin is included in all three measures, so they describe the SVG as drawn. The STEM provider stores the metrics beside the cached SVG, so a cache hit has them without typesetting again. Old cache entries without metrics are typeset again.
- **Placing:** `preview::inlines` lays a formula out as a gap rather than its source:
  - the gap is a run of U+2003 EM SPACE, U+2009 THIN SPACE and U+200A HAIR SPACE in the body font, chosen greedily from their measured advances to come within 0.5 px of the formula's drawn width;
  - the gap's range is recorded with the formula;
  - spaces cannot break inside the gap: wrapping that would split it moves it whole to the next line, using U+2060 WORD JOINER between its characters.
- **Painting:** a wrapper element around the `StyledText` reads the gap's start position, and its line's ascent, from the `TextLayout` after layout. It paints the formula's image there, with the formula's baseline on the line's baseline.
- **Colour:** the SVG is drawn black on a white rounded backing in the dark theme too, as block formulas are. In the light theme the backing is the page colour.
- **Interaction:** the formula's tooltip still shows its source, and its accessible label is the source. A formula still pending or failed shows its source styled as code, as today.
- **Visual editor:** a STEM atom whose formula is rendered is drawn the same way, as one atom: the caret steps over it and copying gives its source. The visual document's view tree already carries the rendered formula, so the visual editor looks it up by the atom's source span.

Acceptance tests:

- `inline_math_records_its_baseline` (adoc-pdf): the baseline lies strictly inside the height, and `x^2` has a smaller descent than `sum_(i=1)^n i`.
- `inline_stem_carries_its_metrics` (engine): after rendering, a page's inline formula has a `FormulaImage` whose size matches its SVG, and the same comes back from the cache.
- `inline_stem_reserves_its_width` (app, `preview::inlines`): the gap's measured width is within 0.5 px of the formula's drawn width, and the text's runs still cover it.
- `preview_draws_inline_stem_in_the_line` (app): a paragraph with `stem:[sqrt(4) = 2]` paints a `stem-inline` image within its line's bounds. The paragraph's height is within 2 px of the same paragraph with the formula replaced by text.
- `visual_editor_draws_inline_stem` (app).

## 5. MCP over HTTP (WP 6.2, #56)

```text
adoc-mcp <root>… [--allow-write] [--http <addr>]
```

- **Transport:** with `--http`, the server binds `<addr>` (such as `127.0.0.1:8765`, or port 0 for any free port) and serves rmcp's `StreamableHttpService` at `/mcp` through `hyper-util`'s server. Without `--http`, it speaks stdio as today. The bound address is printed to standard error as `adoc-mcp: listening on http://<addr>/mcp`.
- **Sessions:** one engine for the process. Each HTTP session gets a `Server` clone sharing it, so every tool still locks the one engine.
- **Exposure:** an address that is not loopback is refused at start-up. Every request without `Authorization: Bearer <token>` gets `401 Unauthorized`, before rmcp sees it. The token is `ADOC_MCP_TOKEN` when set and non-empty, otherwise 32 random bytes in hex, printed once to standard error. The `Host` header must name a loopback host, which is rmcp's default.
- **Tools and paths:** exactly as over stdio. Write tools stay unlisted and refused without `--allow-write`, and every path goes through `paths::confine`.
- **Shutting down:** Ctrl-C (or SIGTERM) cancels the service's token and the server exits cleanly.
- **Layering:** see §2. A test builds metadata where `adoc-mcp` reaches `hyper` with its `client` feature and expects a violation; the current workspace still passes.

Acceptance tests (`crates/adoc-mcp/tests/http.rs`, driving the binary):

- `serves_over_http_with_a_token`: initialise, list the tools and read a page.
- `requests_without_the_token_are_refused`: no header and a wrong token both get 401.
- `http_binds_loopback_only`: `--http 0.0.0.0:0` exits with an error.
- `write_tools_need_allow_write_over_http`

And `http_servers_are_allowed_only_in_adoc_mcp` (xtask).

Documentation: the MCP page and the README's MCP section show the `url` and `headers` form of an `mcpServers` entry.

## 6. Signed releases (WP 6.3, #54)

- **macOS (`packaging/macos/bundle.sh`, `release.yml`):**
  - When `APPLE_CERTIFICATE` (a base64 `.p12`) and `APPLE_CERTIFICATE_PASSWORD` are set, the workflow imports the certificate into a temporary keychain and sets `APPLE_SIGNING_IDENTITY`.
  - `bundle.sh` signs `adocfoundry-cli`, `adoc-ls` and `adoc-mcp`, then the app, each with `--options runtime --timestamp`. An entitlements file is used only if the hardened runtime needs one, and none is expected.
  - It notarises the app (a `ditto` zip through `notarytool submit --wait`) and staples it. Then it builds the DMG, signs it, notarises it and staples it.
  - Without the secrets it signs ad hoc as today.
  - The keychain is deleted at the end of the job, whether the job succeeds or not.
- **Windows (`release.yml`):**
  - When `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT` and `AZURE_SIGNING_PROFILE` are set, the four executables are signed after the build. The MSI is built from the signed executables and signed too. Both use SHA-256 and an RFC 3161 timestamp.
  - The zip carries the signed executables.
- **Checks:** the smoke job runs `packaging/smoke-test.py --expect-signed` when the release was signed:
  - on macOS: `codesign --verify --strict` on the app and every tool, and `spctl -a -vv` on the app;
  - on Windows: `Get-AuthenticodeSignature` reports `Valid` for each executable.

  `smoke-test.py`'s signature check has unit tests that run without a signed build.
- **Documentation:** the README's Gatekeeper and SmartScreen steps are replaced by a note that releases from 0.3.0 are signed. The developer documentation lists the secrets and how to obtain them.

Acceptance: `signature_checks_report_unsigned_builds` (Python `unittest` in `packaging/`, run by CI). The manual checks in `docs/qa/phase6.md`:

- a downloaded DMG opens with no Gatekeeper prompt;
- `spctl -a -vv` accepts the app;
- the MSI installs on Windows 11 with no SmartScreen warning.

## 7. The update check (WP 6.4, #55)

```rust
// adoc-engine::updates
pub struct Version { major: u64, minor: u64, patch: u64, pre: Option<String> }   // `v` prefix accepted
pub struct Release { pub version: Version, pub tag: String, pub page: String, pub assets: Vec<Asset> }
pub struct Asset { pub name: String, pub url: String, pub size: u64 }
pub struct UpdateState { pub last_checked: Option<u64>, pub announced: Option<String> }   // Unix seconds
pub enum Install { MacApp, WindowsMsi, AppImage, Other }
pub fn parse_release(json: &str) -> Result<Release, UpdateError>;
pub fn is_newer(current: &Version, candidate: &Version) -> bool;
pub fn check_due(state: &UpdateState, now: u64) -> bool;             // never checked, or 24 h since
pub fn should_announce(state: &UpdateState, release: &Release, current: &Version) -> bool;
impl UpdateState { pub fn load(path: &Path) -> Self; pub fn save(&self, path: &Path) -> io::Result<()>; }
```

- **Settings:** `[updates] check = true`. With `false` there is no check at start-up. The command `app.check_for_updates` still checks when invoked, since that is the user asking.
- **Flow:** at start-up the app asks the engine whether a check is due. If it is, the app runs `curl -fsSL --max-time 20 -H "Accept: application/vnd.github+json" -A "AdocFoundry/<version>" <feed>` on a background thread and hands the body to `parse_release`. When `should_announce` holds, the app shows one notification: "AdocFoundry <version> is available", with **Update** (§8), **Release notes** (opens the release page) and **Later**. Showing it records `announced`.
- **Failures:** no network, no `curl`, rate limiting or an unreadable feed are logged at debug level and never shown at start-up. A check through the command reports its failure.
- **Version in pre-releases:** a pre-release build is offered the next final release with the same or a higher version. A final build is never offered a pre-release, since the feed excludes them.

Acceptance tests (engine): `parses_the_latest_release`, `newer_versions_are_detected`, `checks_at_most_daily`, `announces_a_release_once`, `updates_can_be_switched_off`. And `update_notice_is_shown_once` (app, with a fake fetcher).

## 8. Updating in place (WP 6.5, #55)

```rust
// adoc-engine::updates
pub enum UpdatePlan { InPlace { asset: Asset, checksum: String }, AppImage { page: String }, Notify { page: String } }
pub fn plan(release: &Release, install: Install, os: &str, arch: &str, sums: Option<&str>) -> UpdatePlan;
pub fn verify_checksum(path: &Path, expected: &str) -> Result<(), UpdateError>;   // SHA-256 (`sha2`, in the tree)
pub fn install_kind(executable: &Path, env: &impl Fn(&str) -> Option<String>) -> Install;
```

- **Choosing the package:**
  - macOS: the universal DMG;
  - Windows: `AdocFoundry-<version>-<arch>.msi` for the running architecture;
  - an AppImage (`APPIMAGE` set): `AppImage`;
  - anything else: `Notify`.

  A release without `SHA256SUMS`, or without a line for the asset, is only notified.
- **Installer kind:**
  - an executable inside `*.app/Contents/MacOS` is `MacApp`;
  - on Windows, an executable under `Program Files` beside the MSI's registry entry is `WindowsMsi`; the portable zip is `Other`;
  - `APPIMAGE` set means `AppImage`.
- **Downloading:** `curl -fL --max-time 600` into a new temporary directory. The progress shows in the notification, and the download can be cancelled. The checksum is verified before anything is opened.
- **macOS:**
  - `hdiutil attach -nobrowse -readonly`, then check the new app:
    - `codesign --verify --strict --deep` passes;
    - `spctl -a -t exec` passes;
    - its Team ID (`codesign -dv`) equals the running app's.
  - `ditto` the new app beside the running one, swap them by renaming, move the old one to the Trash and detach the image.
  - Relaunch through a detached `/bin/sh` that waits for this process to exit and then runs `open`. The app quits normally, so unsaved documents ask as on any quit.
- **Windows:**
  - `powershell -NoProfile -Command Get-AuthenticodeSignature` on the MSI must report `Valid`, with the running executable's signer subject.
  - The app spawns `msiexec /i <msi> /passive` detached, then quits. The MSI's `MajorUpgrade` replaces the installed version; this package checks that the WiX source has one.
- **Refusals:** a running build that is ad hoc or unsigned, a failed check, or a bundle in a directory the user cannot write (such as a read-only DMG) all fall back to **Release notes** with the reason.
- **AppImage:** `release.yml` builds with `UPDATE_INFORMATION` and publishes the `.zsync`. The notice offers AppImageUpdate when it is on `PATH`, otherwise the release page.
- **Release workflow:** `publish` writes `SHA256SUMS` over every asset before uploading.

Acceptance tests (engine): `chooses_the_package_for_the_platform`, `only_notifies_without_checksums`, `checksums_are_verified`, `install_kind_follows_the_executable`. And (xtask) `msi_has_a_major_upgrade`. Manual checks in `docs/qa/phase6.md`: on macOS and on Windows, a build of 0.3.0 installed and pointed at a test feed (`ADOCFOUNDRY_UPDATE_FEED`, read only in debug builds) offering 0.3.1 updates itself after asking.

## 9. Out of Scope in Phase 6, Open Questions and Implementation Notes

Out of scope:

- Editing STEM visually.
- Delta updates, background downloads without asking, and update channels (beta).
- Publishing to package managers (Homebrew, winget, Flathub).
- MCP over HTTP on a non-loopback address, TLS, or OAuth.
- Signing the Linux packages.

Open questions, to settle before the package that needs them:

- [ ] **WP 6.3:** the Apple Developer ID and Azure Trusted Signing accounts exist, and their secrets are in the repository. Until they are, the workflow builds unsigned, and the manual checks wait.
- [ ] **WP 6.5:** whether a Linux AppImage should run AppImageUpdate itself, or only point to it.

Implementation notes, as each package lands:

| Area | Specification | Implementation | Why |
|---|---|---|---|

## 10. Implementation Sequence

| # | Package | Depends on |
|---|---|---|
| 6.1 | Inline STEM in the preview's text flow and the visual editor (#57) | Phase 5 |
| 6.2 | MCP over Streamable HTTP, on loopback with a token (#56) | Phase 5 |
| 6.3 | Signed releases: Developer ID and notarisation, Azure Trusted Signing (#54) | Phase 5 |
| 6.4 | The update check and its notice (#55) | 6.3 |
| 6.5 | Updating in place on macOS and Windows, AppImage update information, `SHA256SUMS` (#55) | 6.4 |

## 11. Definition of Done for Phase 6

- An inline formula is typeset in the line in the preview and the visual editor, in both themes, without noticeably changing the line's height, and its source is still its tooltip.
- `adoc-mcp --http 127.0.0.1:<port>` serves an MCP client holding the token, refuses one without it, refuses a non-loopback address, and `cargo xtask layers` passes.
- With the secrets present, the release workflow produces a notarised, stapled DMG and Authenticode-signed executables and MSI, and the smoke test checks the signatures. Without them it still builds.
- An older build notices a newer release and says so once. On macOS and Windows it updates itself after asking, and only to a build that passes the same signature checks.
- Every acceptance test above exists and passes. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check` and `cargo xtask layers` pass.
- README, the documentation site, `CHANGELOG.md` and `docs/qa/phase6.md` describe what shipped.
