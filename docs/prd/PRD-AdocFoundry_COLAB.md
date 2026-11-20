# PRD: AdocFoundry — Collaborative Editing

| Field | Value |
| --- | --- |
| Status | Proposed, 2026-11-17 (epic #686). COLLAB 1/8, the feasibility and contract gate, proposed its decisions on 2026-11-20 (§14), pending the maintainer's acceptance |
| Date | 2026-11-17 |
| Author | Paul Snow |
| Version | 0.0.0 |
| Scope | Real-time editing of one workspace's AsciiDoc pages and partials by a host and an invited guest, in native Rust: a CRDT per shared file, an engine bridge, an authenticated private-network transport, host-only saving, and the desktop interface |
| Repository | [bovinemagnet/AdocFoundry](https://github.com/bovinemagnet/AdocFoundry) |
| Related work | Epic #686; work packages #688 to #695 (§11) |
| Related design | `docs/design/DESIGN-AdocFoundry_Architecture.md` (whose non-goal of collaborative editing this document revises, §14); `AGENTS.md`; `cargo xtask layers`; `docs/prd/PRD-AdocFoundry_MCP_Agent_Workflows.md` (the loopback HTTP model) |
| Existing code | `crates/adoc-engine/src/engine.rs`, `document.rs`, `edit.rs`, `event.rs`, `undo.rs`, `visual.rs`, `workspace.rs`, `writer.rs`, `recovery.rs`, `auto_save.rs`; `crates/adoc-model/src/mapping.rs`; `crates/adocfoundry/src/app.rs`, `editor/mod.rs`, `menus.rs`; `xtask/src/layers.rs` |

## 1. Purpose and problem

AdocFoundry has one authoritative `Engine` per process. Open documents are `ropey::Rope`s with a process-local `DocumentId`, a `version` and a dirty flag; edits are `TextEdit`s in UTF-8 byte offsets; undo is a per-document `UndoStack` of byte-range inverses; the file watcher notices changes made elsewhere and the writer saves. None of that can merge concurrent edits made in two processes: byte ranges recorded in one are meaningless in the other, and the undo stack would undo another person's work.

Let two AdocFoundry users edit the **same AsciiDoc workspace at the same time**, see each other's changes and carets, undo only their own work, and save safely, with the host's files never at risk. The experience is comparable to Zed's collaborative editing, without Zed's code, services or accounts.

This is real-time editing. It is not Git merging, file-watcher synchronisation, shared storage, screen sharing, LSP or MCP remote editing, or a shared terminal.

## 2. Goals

1. Two separate AdocFoundry processes edit the same `.adoc` file and, once updates settle, hold byte-identical UTF-8 text.
2. Concurrent insertions and deletions at one place converge, for Unicode, IME composition and CRLF text alike.
3. Remote changes reach the source editor, the visual editor, diagnostics, the index and the preview through the existing pipelines, without echoing back and without replacing the whole document on each keystroke.
4. Undo and redo act on the local author's operations only, however remote edits interleave.
5. Several eligible pages and partials can be shared and edited at once; switching files and reconnecting neither duplicate nor lose text.
6. Only the host writes workspace files. Guest saves are requests the host checks. Accepted content is never silently lost to a stale update, an external rewrite or a crash.
7. Sharing is off by default, opens no listener without consent, authenticates and encrypts every connection, and checks every request against the participant's current role and the shared scope.
8. Everything runs in process, in native Rust, with permissive dependencies; with collaboration off, nothing changes for existing workflows, builds or benchmarks.

## 3. Non-goals

For the first release:

- An account or cloud service, a public relay, NAT traversal, discovery broadcasting, or automatic port forwarding.
- Voice, video, screen sharing, a shared terminal, or shared builds and exports.
- More than one host's filesystem; editing binary files; guest Git operations.
- Renaming, moving, deleting or creating shared files, navigation rewrites, and multi-file refactors as distributed transactions.
- Offline-first editing: a disconnected guest cannot edit.
- A graphical three-way merge of external changes, full multi-user history, and exact Zed parity.
- Porting Zed's implementation or using any GPL-licensed Zed crate.

A guest never needs the project checked out, at the host's path or anywhere else, and never writes to a local clone in order to take part.

## 4. User workflows

**Host.** Choose Share Workspace…, review the exact root, the eligible files, the network interface and the host identity, then start a private, authenticated session and copy a short-lived invitation. Each guest joins as a Viewer; the host may allow editing, revoke it, save the merged text, or stop sharing.

**Guest.** Choose Join…, enter the invitation, and verify the host's certificate fingerprint before any content is sent. The scoped file list appears only after authorisation. Remote tabs are marked as remote; remote carets and selections are drawn with names. On a network interruption the guest sees Disconnected and is never told unsent edits are saved; while the same session lives it reconnects and resynchronises, otherwise it rejoins explicitly.

**Roles.** The host is Owner; a guest is Editor or Viewer. Roles are enforced by the host on every request, not by disabling buttons.

## 5. Functional requirements

### 5.1 Sessions, roles and scope

- A session shares one explicitly chosen workspace root. Guests see opaque, session-scoped `SharedFileId`s and validated, normalised relative display paths; never an absolute host path.
- Initially only `.adoc` pages and partials are shared. Images needed for previews are a separate, read-only, individually authorised capability.
- `.git`, dotfiles, credential stores, binaries, ignored folders (`[workspace] ignore`) and anything reached through a link leaving the root are excluded. Allowlisting more must be deliberate and visible.
- Guests may not build, export, run commands, change Git, create, rename or delete files, or call MCP host operations. A modified guest client must not get round any of this.
- The first release supports one host and one guest; types and protocol allow more.

### 5.2 Edits and convergence

- Text is held in a CRDT per shared file. `yrs` (Yjs-compatible) is the first candidate; `loro` and `automerge`, which the architecture document names, are the alternatives (§13).
- Positions are UTF-8 byte offsets, matching `SourceRange`; every offset is checked to fall on a character boundary. Line endings and Unicode normalisation are never rewritten.
- Every text-producing route goes through one engine bridge: typing, IME, paste, visual-editor steps, spelling replacements, table formatting, case changes, undo and redo. None is special-cased.
- The bridge is atomic: a refused command leaves the CRDT and the rope unchanged and sends nothing. A projection failure stops that file's collaboration and asks for a verified resynchronisation rather than hiding a divergence.
- Remote updates are applied once; duplicates and reordering do not change the result. They reach the editor as incremental patches, never as a whole-text replacement for ordinary typing.
- Caret and selection anchors travel as CRDT relative positions, never as raw byte offsets.

### 5.3 Undo

- While a file is shared, undo and redo go through a per-author CRDT undo manager that tracks only the local author's transactions. Another participant's edit never enters the local history.
- Pre-session history is set aside when a session starts and not replayed afterwards: its byte ranges would be stale.
- Typing still coalesces (`COALESCE_WINDOW` in `undo.rs`, 500 ms) without swallowing remote edits; visual commands keep their group boundaries.
- Grouped multi-file commands are refused while shared until a tested transactional design exists.
- Undoing a word typed before a peer inserted text inside it, then redoing it, never deletes the peer's text.
- With collaboration off, the existing undo model is unchanged.

### 5.4 Saving, conflicts and recovery

- Only the host writes real files, through `Engine::save` and the writer, which keep their conflict checks; host auto-save writes the merged text.
- Joining uses the host's current buffer, unsaved edits included, never a stale disk snapshot.
- A save snapshots the merged text and its CRDT revision on the engine thread and acknowledges exactly that revision. Edits arriving during the save stay dirty.
- A change on disk from Git, another editor or a tool never silently overwrites a shared buffer, nor flows in as an ordinary edit: the file enters a conflict state and the host chooses to keep the shared text, reload from disk (a new epoch for that file), or copy it aside.
- Guest Save is a request: `Saved`, `Queued`, `Conflict`, `PermissionDenied` or a specific failure. Duplicate requests are idempotent; no guest can bypass a conflict check.
- If the host goes away the guest keeps the last acknowledged text visible and copyable, marked unsaved. Reconnecting to the same session resynchronises by state vector; a new session or epoch needs an explicit rejoin.
- Recovery records keep accepted but unsaved text. Invitation tokens and session secrets are never persisted. Restarting the host recovers text without restarting sharing.

### 5.5 Security and privacy

- Off by default; nothing listens and nothing is shared before the host confirms one scope.
- TLS (native Rust, such as `rustls`) on every connection, with a host certificate whose fingerprint the guest verifies, and high-entropy, single-use, short-lived invitations held as hashes. An invitation is not a substitute for certificate verification.
- Authenticate before sending a manifest or a single file name. Bind each connection to session, epoch, peer and role.
- Bind to an interface the host picks; never all interfaces by default.
- Bound frame size, update size, file size, open shared documents, queued messages and participants. A slow or hostile peer is slowed or disconnected; it never blocks the host's editor.
- Path confinement as `adoc-mcp`'s `paths::confine` does it: lexically inside the root and again once links are followed, rechecked when a file is opened.
- No tokens, secrets or document text in logs at default levels.

### 5.6 Multiple files and Antora

- Each shared file has its own replica, state vector, epoch, presence and save state; opening a second file changes nothing about the first.
- A guest reopening a tab rejoins the existing replica and never bootstraps its text a second time.
- A partial is editable only as its own authorised file; included text stays read-only where it is included, as the visual editor already treats it.
- Navigation, xrefs and diagnostics never disclose unshared pages, component metadata, media or host paths: an out-of-scope target is an unavailable placeholder, and diagnostics are relative or redacted.
- How a guest previews and navigates is settled by COLLAB 1/8 (§13): host-resolved, scoped metadata with a virtual-file resolver, or host-rendered, sanitised preview data for one page.

### 5.7 Interface

- Commands with stable `CommandId`s, each placed in `MENUS` or `NOT_IN_MENUS` in `crates/adocfoundry/src/menus.rs` (`every_command_is_in_a_menu`): Share Workspace…, Join…, Copy Invitation, Stop Sharing, Leave Session, Allow Editing and Revoke Editing (`collaboration.*`, names settled in COLLAB 6/8).
- Session state (`Hosting`, `Connected`, `Reconnecting`, `Disconnected`), participants with name and role, per-file presence, coloured carets and selections with labels (never colour alone), a remote-tab indicator, and save states (`Unsaved`, `Saving`, saved revision, conflict, permission revoked).
- Presence is throttled and ephemeral: never in saved content, Git diffs or exports. Local caret, selection, scroll and focus survive remote edits.
- Follow Participant is a follow-on.

## 6. Existing code to reuse

| Code | What it gives | How collaboration uses it |
| --- | --- | --- |
| `crates/adoc-engine/src/document.rs` | `OpenDocument { text: Rope, version, dirty, .. }`; `DocumentState` (`saved_hash`, `saved_len`, `changed_on_disk`, `client_owned`) | The rope stays the engine's text. A shared document gets a separate collaboration attachment; `client_owned` (the language client's buffers) is not reused for guest files |
| `crates/adoc-engine/src/edit.rs` | `TextEdit { range: SourceRange, replacement }`, in pre-edit coordinates, and `validate` | The bridge converts CRDT deltas to and from it. It lives at L3, so `adoc-collab` (L2) defines its own edit type over `adoc_core::SourceRange` or the type moves to `adoc-core` (§13) |
| `crates/adoc-engine/src/engine.rs` | `apply_edits` and the private `edit_document` (version, dirty, diagnostics, index, preview, visual model, `DocumentChanged`); `undo`, `redo`, `save`, `overwrite`, `reload`, `mark_saved` | One gateway in front of `edit_document` for shared files; ordinary documents keep today's path and performance |
| `crates/adoc-engine/src/event.rs` | `EngineEvent::DocumentChanged { id, version, edits }`, `DocumentChangedOnDisk` | Extended or complemented with the edit's origin, without breaking existing listeners |
| `crates/adoc-engine/src/undo.rs` | `UndoStack`, `UndoEntry`, `COALESCE_WINDOW` | Unchanged for ordinary documents; bypassed for shared ones |
| `crates/adoc-engine/src/visual.rs`, `crates/adoc-model/src/step.rs` | `apply_visual`, which turns a model step's `StepResult` into text edits | Visual edits go through the same bridge |
| `crates/adoc-model/src/mapping.rs` | `Mapping::map`, `map_range` | Keeps carets and selections across remote patches |
| `crates/adoc-engine/src/workspace.rs` | `open_document_changed_on_disk` (clean buffers follow disk, dirty ones raise `DocumentChangedOnDisk`) | A shared file's disk change becomes a collaboration conflict instead |
| `crates/adoc-engine/src/writer.rs`, `recovery.rs`, `auto_save.rs` | Ordered saves with conflict checks, recovery records, auto-save | Host saves and recovery of accepted text |
| `crates/adocfoundry/src/editor/mod.rs` | `InputEditsEvent`, `send_widget_edit`, `own_versions` (echo suppression), `sync_widget_to_engine`, `reconcile` | Remote patch batches are applied without echo; `reconcile`'s single prefix/suffix diff is not enough for batches |
| `crates/adocfoundry/src/app.rs` | The single engine owner, `PumpedEngine`, the waker | Network tasks hand bounded messages to the engine's thread and wake it |
| `crates/adoc-mcp/src/paths.rs`, `src/http.rs` | `paths::confine`; a token-gated listener on loopback | Reference for confinement and listener hardening |

## 7. Architecture

```text
L4  adocfoundry   Share/Join UI, participants, remote carets, session status;
                  the network transport (TLS, framing, invitations), §13
L3  adoc-engine   `collab` feature: session attachment per document, the edit
                  bridge (CRDT <-> rope), per-author undo, role and scope checks,
                  host-only saving and conflict states; no network code
L2  adoc-collab   NEW, headless: per-file CRDT replicas, transaction origin,
                  update and state-vector encoding, per-author undo, relative
                  anchors, limits; no networking, UI, filesystem or parser
L0/L1             adoc-core (SourceRange) and the existing parsing, Antora and index crates
```

- `adoc-collab` sits at **L2** and may depend on `adoc-core` and the chosen CRDT crate only. It needs an entry in `LAYERS` in `xtask/src/layers.rs`, an entry in `[workspace.dependencies]`, and a mention in `AGENTS.md`, `README.md`, the design document and the developer documentation's crate table and diagram (`architecture.adoc`, `crate-layers.mmd`), which `every_crate_is_documented` checks.
- The engine's `collab` feature is opt-in: off in `[workspace.dependencies]`, like `typst`, `docx` and `git`, so each front end names it. `adoc-ls`, `adoc-mcp` and the CLI do not; `cargo xtask layers` checks that their default builds compile no `adoc-collab`.
- Networking stays where the layering allows it. Today `NETWORK_ALLOWED` is `adocfoundry` alone, `NETWORK_CRATES` lists HTTP clients only, and the app's one network use is the update check, which runs the system's `curl` (`src/updates.rs`). The TLS and WebSocket crates chosen are added to the confinement lists so that no crate below L4 can reach them, and `AGENTS.md`'s confinement rules are updated in the same change.
- The engine is single-threaded. Transport tasks send validated, bounded messages to the engine's thread and wake it; they never touch the rope or GPUI entities, and never hold the engine while sending. Any new `pump` step has its deadline in `Engine::next_due` and a test in `tests/wake.rs`; any worker is started with `worker::spawn`.
- The invariant: **the engine remains the editing API; the CRDT and the rope are two verified views of the same shared text, not independent owners.**
- New dependencies are pinned exactly and pass `cargo deny check`.

## 8. Headless contracts

Names are illustrative; COLLAB 1/8 and 2/8 settle them and record the outcome in §14.

```rust
struct CollaborationSession {
    id: SessionId,
    shared_root: PathBuf,                 // host only; never sent to guests
    participants: BTreeMap<PeerId, Role>,
    files: BTreeMap<SharedFileId, SharedText>,
}

enum Role { Owner, Editor, Viewer }

struct SharedText {
    document: DocumentId,                 // local to this process
    replica: Replica,                     // adoc-collab, wrapping the CRDT text
    epoch: Epoch,
    saved_revision: Option<Revision>,
}

enum EditOrigin { Local(PeerId), Remote(PeerId), Undo, Redo, Filesystem, InitialSync }
```

`adoc-collab`'s `Replica`: `new(text, actor)`, `apply_local(&[Edit])`, `apply_remote(&EncodedUpdate)` (each giving the byte edits, the outbound update and the resulting state vector), `state_vector`, `missing_since`, `undo_local`, `redo_local`, and relative selections to encode and resolve. Errors are typed (`InvalidBoundary`, `MalformedUpdate`, `TooLarge`, `EpochMismatch`, `ProjectionMismatch`, `UndoUnavailable`). No CRDT handle or transaction crosses an asynchronous boundary.

Protocol messages: `Hello`, `Authenticate`, `Join`, `Manifest`, `OpenFile`, `Snapshot`, `Update`, `UpdateAck`, `Awareness`, `SaveRequest`, `SaveResult`, `RoleChanged`, `Resync`, `Leave`, `Error`.

- The protocol is versioned apart from the application; incompatible peers fail visibly.
- Small versioned control messages; CRDT updates as binary payloads, never whole books serialised as JSON.
- `DocumentId` and `OpenDocument::version` are process-local and never sent as shared revisions; a state vector is never proof of permission.
- Limits are named constants with tests at their bounds.

## 9. Safety, performance and reliability

Release gates, each held by tests from the package that introduces it, not deferred to COLLAB 8/8:

- **Convergence:** byte-identical text after concurrent edits; character-boundary offsets; no echo.
- **Undo isolation:** an author undoes only their own transactions.
- **Disk safety:** host-only saves, external changes as conflicts, exact saved revisions.
- **Trust boundary:** private opt-in listener, verified identity, per-request authorisation, scoped files, no remote commands.
- **Workspace fidelity:** includes and xrefs cannot leak unshared pages; remote tabs never assume host paths exist locally.
- **No regressions:** single-user typing, the visual editor, export, the language server, the CLI and the layering rules are unchanged.

Invariants across the packages:

1. Local and remote changes pass through the same engine owner and converge in the CRDT; rope, CRDT and interface are not competing authorities.
2. A host file is changed by a guest only through authorised host-owned operations, checked on every request.
3. Undo never undoes remote-only edits, and pre-session byte-range undo is never replayed across merged changes.
4. Guests get opaque file ids and scoped metadata, never absolute host paths or hidden or credential files.
5. In-process native Rust only: no Zed services, no GPL code, no external server executable.
6. No file is shared, listener opened or secret persisted before the host consents. Guests cannot edit offline.
7. "Guest may edit" does not imply "guest may save"; an acknowledged update is not a saved file; a saved revision is never ahead of the bytes on disk.

Performance: measure local typing latency (p50 and p95) with and without collaboration, remote update latency on an emulated LAN, update bytes per character and paragraph, CRDT memory over a long session, initial snapshot and reconnect cost, for a small page, a 1 MiB document and several shared files. Budgets come from COLLAB 1/8's measurements; CI does not gate on wall-clock timings, but alerts on clear regressions and unbounded growth.

## 10. Tests and acceptance criteria

Each test below exists under its name when its work package is done. Names may be refined by the package that writes them, and the change recorded in §14.

| # | Test | Package | What it shows |
| --- | --- | --- | --- |
| 1 | `collab_spike_replicas_converge` | C1 | Two replicas of the chosen CRDT converge byte for byte under concurrent Unicode edits, duplication and reordering; reconnect by state vector syncs exactly once |
| 2 | `collab_spike_undo_is_per_author` | C1 | An author's undo after interleaved remote edits affects only that author's operations |
| 3 | `collab_replicas_converge` | C2 | Two or three replicas fed seeded adversarial schedules converge after all updates are delivered, in any order, with duplicates |
| 4 | `collab_edits_are_byte_exact` | C2 | Local and remote edits give ordered, non-overlapping, character-boundary byte edits that rebuild the text; Unicode, IME and CRLF fixtures included |
| 5 | `collab_local_undo_keeps_remote_edits` | C2 | Undo and redo never remove a remote edit, including text inserted inside the undone word |
| 6 | `collab_malformed_update_changes_nothing` | C2 | Malformed and oversized updates are refused without changing the replica |
| 7 | `collab_engines_converge` | C3 | Two headless engines over an in-memory queue converge; each engine's rope, replica and preview text agree after every step |
| 8 | `collab_remote_edit_is_not_echoed` | C3 | A remote one-character edit is one patch, not a whole-text replacement, and produces no outgoing update |
| 9 | `collab_visual_edits_share_the_bridge` | C3 | Visual-editor steps, paste and spelling replacements travel the same path as typing |
| 10 | `collab_off_changes_nothing` | C3 | Without the `collab` feature, or with no session, engine tests, undo and the lean builds are unchanged |
| 11 | `collab_loopback_session_syncs` | C4 | A loopback TLS session authenticates, opens a file, exchanges updates and reconnects within one epoch |
| 12 | `collab_unauthorised_requests_are_refused` | C4 | Unauthenticated, revoked and Viewer peers, traversal, links leaving the root, `.git` and unknown file ids are refused, even from a modified client |
| 13 | `collab_wrong_fingerprint_sends_nothing` | C4 | A fingerprint mismatch or an expired or reused invitation fails before any content is sent |
| 14 | `collab_slow_peer_never_blocks_host` | C4 | A slow or flooding peer is slowed or dropped; host typing is unaffected and memory stays bounded |
| 15 | `collab_save_writes_converged_text` | C5 | A save after interleaved edits writes exactly the converged text and acknowledges that revision; an edit during the save stays dirty |
| 16 | `collab_external_change_is_a_conflict` | C5 | A disk change under shared edits is a conflict, never an automatic reload or overwrite |
| 17 | `collab_stale_epoch_is_refused` | C5 | After stop-sharing or a host restart, updates and saves from the old epoch are refused |
| 18 | `collab_share_needs_confirmation` | C6 | No listener exists until the host confirms; Stop Sharing closes it at once |
| 19 | `collab_viewer_cannot_edit` | C6 | A Viewer can select and copy but not change text through source, visual, paste or shortcuts |
| 20 | `collab_remote_carets_follow_edits` | C6 | Remote carets and selections stay on their text through local and remote edits; the local caret does not move for edits elsewhere |
| 21 | `collab_pages_converge_independently` | C7 | Two peers editing two pages, and a page and its partial, converge per file; previews follow |
| 22 | `collab_scope_never_leaks` | C7 | Navigation, xrefs, includes, diagnostics and images never disclose unshared pages, metadata, media or host paths |
| 23 | `collab_structural_commands_refused` | C7 | Rename, move, delete, New Page and navigation rewrites are refused while shared, leaving nothing half-applied |
| 24 | `collab_simulation_converges` | C8 | Checked-in seeds of the multi-replica simulation, with failure injection, converge and keep every invariant of §9 |
| 25 | `collab_two_process_release_suite` | C8 | Host and guest processes over loopback TLS run the coverage matrix, cleaning up their listeners |

Manual checks that tests cannot make (two machines on a LAN, firewall prompts on macOS, Windows and Linux, clipboard, IME, accessibility, presence under fast typing) are recorded in `docs/qa/collab.md` by COLLAB 8/8.

Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo xtask layers` and `cargo deny check`.

## 11. Work packages

| WP | Deliverable | Depends on | Issue |
| --- | --- | --- | --- |
| C1 | CRDT spike, security model and protocol; `docs/design/DESIGN-AdocFoundry_Collaboration.md` | — | #688 (COLLAB 1/8) |
| C2 | `adoc-collab` L2 crate: replicas, byte edits, state vectors, per-author undo, anchors, limits | C1 | #689 (COLLAB 2/8) |
| C3 | Engine bridge: CRDT and rope, origin-aware events, one edit gateway, per-author undo | C1, C2 | #690 (COLLAB 3/8) |
| C4 | Native Rust transport, TLS identity, invitations, roles and scoped sessions | C1, C2 | #691 (COLLAB 4/8) |
| C5 | Host-only saving, external-change conflicts and session recovery | C2, C3, C4 | #692 (COLLAB 5/8) |
| C6 | Share and Join interface, participants, presence and remote carets | C1, C3, C4, C5 | #693 (COLLAB 6/8) |
| C7 | Several files, Antora scope and consistent previews | C2 to C6 | #694 (COLLAB 7/8) |
| C8 | Convergence, security, performance and cross-platform release QA | C1 to C7 | #695 (COLLAB 8/8) |

Order: C1, then C2; C3 and C4 in parallel on C2's contracts; then C5; C6 (its Share, Join and presence work may begin once C3 and C4 land, its save states once C5 does); C7; C8 verifies and documents the release. Per-author undo, security checks and save correctness are not deferred to C8: each package carries its own tests.

## 12. Definition of done

1. Two AdocFoundry processes on different machines of a private network edit the same pages and partials concurrently and converge, with remote carets, per-author undo and host-only saving.
2. Every release gate of §9 holds, shown by the tests of §10 in CI and the manual checks in `docs/qa/collab.md`.
3. With collaboration off, no listener exists and single-user editing, exports, the language server, the CLI, the MCP server and the benchmarks are unchanged.
4. The user and developer documentation explain sharing on a private network, fingerprint verification, firewall and interface choice, scope, roles, who owns the files, what Saved and Acknowledged mean, the offline limits, revocation and manual recovery.
5. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo xtask layers` and `cargo deny check` pass.

## 13. Open decisions

Each is settled before work starts on the package it affects, and the outcome is recorded in that package's pull request and in §14.

- **CRDT library (C1).** `yrs` first, for its state-vector sync, update observers, relative positions, undo manager and `OffsetKind::Bytes`; `loro` and `automerge` as alternatives if measurements or missing features justify one. Record exact versions, licences, transitive dependencies, memory, update size, decoding limits and threading.
- **Where the edit type lives (C2).** `TextEdit` is in `adoc-engine` (L3), out of `adoc-collab`'s reach. Either `adoc-collab` has its own edit over `adoc_core::SourceRange`, converted by the engine, or `TextEdit` moves to `adoc-core`.
- **Undo across a session boundary (C1, C3).** How pre-session history is set aside and whether anything returns on leaving; how grouped multi-file edits are refused.
- **Transport stack and its home (C1, C4).** `rustls` with a WebSocket crate, or framed TLS without WebSocket; tokio or blocking threads. Whether it lives in the `adocfoundry` crate (whose tests CI runs on macOS only) or in a new L4 transport crate added to `NETWORK_ALLOWED`, so its loopback tests run on every platform without GPUI.
- **Trust and pairing (C1, C4).** Ephemeral or persistent host identity; how the fingerprint is shown and compared; invitation format and lifetime; frame and message limits.
- **Guest preview and navigation (C1, C7).** Scoped host metadata with a virtual-file resolver in the guest's engine, or host-rendered, sanitised preview data for one page; how much ships in the first release.
- **Media and manifest scope (C1, C7).** Which images are shared, and how ignored folders and links are treated.
- **Guest saves and conflict prompts (C5).** Whether a guest may request saves at all in the first release, and the host's conflict choices.
- **Platforms (C8).** Firewall and interface-selection experience on Windows, macOS and Linux, and its effect on packaging and signing.

## 14. Implementation notes

Each work package records here where the implementation departs from this document and how the open decisions of §13 were settled.

### Before C1: corrections to existing documents

- `docs/design/DESIGN-AdocFoundry_Architecture.md` lists collaborative editing as a non-goal and names `loro` and `automerge` as the CRDTs to consider "if collaboration ever becomes a goal". This document makes it a goal; C1 updates the design document with its decision.
- `AGENTS.md`'s confinement rules allow no HTTP client below `adocfoundry`, which runs the system's `curl` for its update check (`src/updates.rs`), and nothing there covers a listener in the app. C4 adds the app's TLS and WebSocket crates to the confinement lists in `xtask/src/layers.rs` and updates `AGENTS.md` in the same pull request.

### C1 (COLLAB 1/8, #688)

The spike is `spikes/collab/`, recorded in `docs/qa/spike-collab.md`. The contracts are in `docs/design/DESIGN-AdocFoundry_Collaboration.md`, which later packages change first when they depart from it. Settled from §13:

- **CRDT library:** `yrs` 0.28.2 with its `sync` feature (without it the undo manager is not `Send`) and `OffsetKind::Bytes`. Its deltas count bytes. It does not refuse an offset inside a character, so the bridge checks every offset first.
- **Topology:** a star through the host over ordered TLS streams. With three or more replicas exchanging updates out of the order sent, `yrs` can leave an update pending for ever; a star never produces that order. A guest left with something pending asks the host for a resynchronisation. The host never resynchronises from a guest: a guest update that leaves it pending drops the guest, and the host rebuilds the file's replica from its own text in a new epoch.
- **Where the edit type lives:** `adoc-collab` has its own `ByteEdit` over `adoc_core::SourceRange`, converted by the engine; `TextEdit` stays in the engine.
- **Undo across a session boundary:** a file's `UndoStack` is dropped when it becomes shared and starts empty when it stops being shared. While shared, undo is the replica's, grouped where the engine's coalescing rules end a group. Multi-file grouped commands are refused while a file they touch is shared.
- **Transport stack and its home:** framed TLS over TCP, no WebSocket, with `rustls` 0.23 (`ring`), `tokio-rustls` and `tokio`, all already in the lock file, plus `rcgen`, in a new L4 crate `adoc-share` added to the network confinement lists. The protocol's types and limits live in `adoc-collab`.
- **Trust and pairing:**
  - The host's identity is per session, never persisted.
  - Invitations carry the certificate's fingerprint and a 256-bit, single-use secret valid for 10 minutes.
  - A safety code is compared out of band before the secret is sent.
  - The host admits each authenticated guest before anything about the workspace is sent.
  - The host assigns each guest's CRDT client id and refuses updates that create items for any other.
- **Guest preview and navigation:** the guest's engine renders from its own replicas through a resolver over the shared files only; out-of-scope links and includes are placeholders.
- **Media and manifest scope:** v1 shares `.adoc` pages and partials only, with no media; dotfiles, `.git`, ignored folders and links leaving the root are never offered.
- **Guest saves and conflict prompts:** Editors may request saves. A change on disk to a shared file is a conflict the host resolves: keep the shared text, reload from disk (a new epoch), or save a copy.
- **Still open:** platform firewall and interface experience (C8); media for guests.

## 15. References

- Epic #686.
- Zed's collaboration, as a reference for the experience only: <https://zed.dev/docs/collaboration/overview>
- `yrs`: <https://docs.rs/yrs/latest/yrs/>; its offset modes: <https://docs.rs/yrs/latest/yrs/doc/enum.OffsetKind.html>
- `loro`: <https://docs.rs/loro/latest/loro/>
- `automerge`: <https://docs.rs/automerge/latest/automerge/>
