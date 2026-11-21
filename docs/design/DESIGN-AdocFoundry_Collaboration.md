# Design: AdocFoundry — Collaborative Editing

| Field | Value |
| --- | --- |
| Status | Proposed by COLLAB 1/8 (#688) and accepted on 2026-11-20; each later package changes it first where it departs (the PRD's §14) |
| Author | Paul Snow |
| Version | 0.0.0 |
| PRD | `docs/prd/PRD-AdocFoundry_COLAB.md` (epic #686) |
| Evidence | `docs/qa/spike-collab.md` and the tests in `spikes/collab/` |
| Protocol version | 1 |

This document holds the decisions and contracts that COLLAB 2/8 to 8/8 build on. Where the PRD left a choice open (§13), this document settles it and the PRD's §14 records the outcome. Anything a later package changes is changed here first, in the same pull request.

## 1. The invariant

**The engine remains the editing API. The CRDT and the rope are two verified views of the same shared text, not independent owners.**
- Every change to a shared file passes through the engine's one edit gateway.
- A projection failure (what the CRDT reports changing is not what the rope changed) stops collaboration on that file and asks for a resynchronisation before any further edit is accepted.
- No external server: the host's app is the server.

## 2. Topology and roles

- **A star.** Guests connect to the host only. The host applies a guest's update and only then forwards it to the other guests. Each link is one TLS stream, ordered both ways.
  - So every replica receives updates in causal order.
  - The spike found that `yrs` 0.28.2 can leave an update pending for ever when three replicas exchange updates out of the order they were sent (record, finding 1). The star never produces that order: 2,000 seeded star schedules converged.
- **When something is left pending:**
  - **A guest,** after an update from the host: it sends `Resync` for that file and refuses local edits to it until the resynchronisation completes.
  - **The host,** after an update from a guest: this never happens with an honest guest, since the host holds everything the guest has seen. A modified guest can cause it with a valid-looking update whose missing dependency can't be seen before applying (record, finding 4). So the host never resynchronises from a guest. It drops the guest (`Error { protocol }`), rebuilds the file's replica from its own text in a new epoch under a new client id, and sends the other guests a new `Snapshot`. The host's own editing never waits on a guest.
- **Roles.** Owner (the host), Editor, Viewer. A guest joins as a Viewer. The host checks every request against the peer's current role; disabled buttons are a courtesy, not a control.
- **v1:** a host and its guests, up to `MAX_PARTICIPANTS` in all (COLLAB 6/8 lets the host admit more than one; the star with two guests is tested).

## 3. Crates and layers

| Crate | Layer | Holds | Depends on |
|---|---|---|---|
| `adoc-collab` (C2) | L2 | Per-file `Replica` over `yrs` (with `sync`, `OffsetKind::Bytes`); its own `ByteEdit { range: SourceRange, text: String }`; origins; per-author undo; anchors; `authors`; the protocol's message types and their codec, with every limit. No network, filesystem, UI or parser | `adoc-core`, `yrs` |
| `adoc-engine`, feature `collab` (C3, C5) | L3 | The session attachment per document, the edit bridge, per-author undo routing, role, scope and epoch checks, host-only saving and conflict states. No network code | `adoc-collab` |
| `adoc-share` (C4, new) | L4 | The transport: listener and client, TLS, framing, invitations, timeouts, queues. It moves bytes between the network and bounded channels to the engine's thread | `adoc-collab` (for frames and limits), `rustls` with `ring`, `tokio-rustls`, `tokio`, `rcgen` |
| `adocfoundry` (C6) | L4 | Share and Join, participants, presence, remote carets | `adoc-engine` (`collab`), `adoc-share` |

Settled from the PRD's §13:
- **Where the edit type lives:** `adoc-collab` has its own `ByteEdit` over `adoc_core::SourceRange`, and the engine converts from `TextEdit`. Moving `TextEdit` to `adoc-core` would touch every crate that edits, for no gain.
- **Transport stack:**
  - **No WebSocket.** It is framed TLS over TCP. There are no browser peers, so a WebSocket would add a crate and an HTTP upgrade surface for nothing.
  - **Its own L4 crate,** `adoc-share`, so its loopback tests run on every platform without GPUI. Its TLS crates are confined to it and the app (`TLS_CRATES` and `TLS_ALLOWED` in `xtask/src/layers.rs`; the HTTP lists, `NETWORK_ALLOWED` and `NETWORK_SERVER_ALLOWED`, are about HTTP and do not name it), and `AGENTS.md`'s confinement rules changed in the same pull request.
  - **Libraries:** `rustls` 0.23 with the `ring` provider, TLS 1.3 only (no `tls12` feature), and `tokio-rustls`; `tokio`, already there through `adoc-mcp`. Both TLS crates were in the lock file through GPUI's HTTP client, but no shipped build compiled them: this is new cryptography for the app (ring, rustls, webpki), from COLLAB 6/8, when the app first depends on `adoc-share`. `ring` builds C and assembly, with clang on Windows on ARM, so the release builds of every target are checked then.
  - **Runtime:** a current-thread runtime on a thread of its own, for the host's listener and for each guest's connection.
- **Front ends:** the engine's `collab` feature is off in `[workspace.dependencies]`. Only the app names it. `adoc-ls`, `adoc-mcp` and the CLI never do, and `cargo xtask layers` checks that their builds compile neither `adoc-collab` nor `adoc-share`. The release builds the app apart from the other three (COLLAB 6/8), since built together Cargo's feature unification would compile the app's features into them.
- **Dependency footprint:** `yrs` with `sync` brings 34 crates, of which only `smallstr` is new to the workspace. `adoc-share` adds `rcgen` (MIT or Apache-2.0, without its `pem` or aws-lc features) and `yasna`, and builds `rustls`, `tokio-rustls` and `ring` for the first time. Every new dependency is pinned exactly and passes `cargo deny check`.

## 4. Positions, text and line endings

- Positions are UTF-8 byte offsets (`OffsetKind::Bytes`), as `SourceRange` is everywhere else. The spike showed that a text event's deltas count bytes in this mode.
- **Every offset is checked against the rope before `yrs` sees it.** `yrs` does not refuse an offset inside a character; it moves the insertion silently (record, finding 2). The engine's `TextEdit::validate` does the check; `adoc-collab` checks again and refuses with `InvalidBoundary`.
- Text is shared exactly as it is held: line endings, a byte order mark and Unicode normalisation are never rewritten. A file the host opens with `\r\n` is shared with `\r\n`.
- A shared file's text is at most `MAX_SHARED_FILE` bytes; a larger file is not offered.

## 5. Identity, pairing and trust

- **The host's identity is per session.** Share generates a new key pair and self-signed certificate in memory (`rcgen`). Neither is ever written to disk or a keychain, and both go when sharing stops. The fingerprint is the SHA-256 of the certificate's DER encoding.
- **The invitation** is one line of text the host copies and sends by whatever channel they trust:
  - format: `adocfoundry:join?v=1&host=<address>&port=<port>&fp=<fingerprint>&secret=<secret>`;
  - the secret is 256 random bits, single use, valid for `INVITE_LIFETIME`;
  - the host keeps only its SHA-256 hash, in memory;
  - the address is the one the host bound.
- **Joining:**
  1. The guest's client connects, completes TLS, and compares the certificate's fingerprint with the invitation's. On a mismatch it disconnects having sent nothing, not even a name.
  2. Then it shows the **safety code**, the first 64 bits of the fingerprint as four groups of five decimal digits, which the host's sharing panel also shows. The guest confirms they match before going on. The invitation pins the certificate; the code guards against a tampered invitation.
  3. Only then does the guest send `Authenticate` with the secret and a display name. A secret is consumed by the first authentication that uses it.
- **Host consent:** an authenticated guest waits until the host chooses Admit, or Deny. Nothing about the workspace, not even its file names, is sent before Admit. An admitted guest is a Viewer.
- **Resuming:**
  - Each admitted peer gets a 256-bit resume token, held in memory on both sides and bound to the session, the peer and the files' epochs.
  - It is replaced on every reconnect and invalidated on revocation, on Leave and when sharing stops.
  - Invitation secrets and resume tokens are never persisted, logged or shown again.
- **Binding:**
  - The host chooses an interface address from those the system lists. There is no default to all interfaces and no automatic port forwarding.
  - The port is chosen by the system unless the host sets one.
  - Nothing listens before the host confirms the scope (Share's dialog), and Stop Sharing closes the listener and every connection at once.
- **Revoking:** the host can take editing away or remove a peer at any moment. Removal closes the connection and invalidates the resume token. Requests already in flight are checked against the role at the time they are processed, not when they were sent.

## 6. Threat model

Every guest is treated as possibly modified, and everything it sends as untrusted data, even over an authenticated connection.

| Threat | Mitigation |
|---|---|
| A connection before the host consents | Nothing listens until Share is confirmed; each guest also waits for Admit before any workspace data is sent |
| A man in the middle | TLS with the certificate pinned from the invitation, plus the safety code compared out of band |
| A guessed, replayed or stolen invitation | 256-bit secrets, held as hashes, single use, `INVITE_LIFETIME`; resume tokens rotated per reconnect and revoked with the peer |
| Forged file ids, guessed paths, `..`, links leaving the root, `.git`, credentials | Guests name files only by opaque `SharedFileId`s from their own manifest. The host resolves an id to a path it already confined (lexically inside the root and again once links are followed, as `adoc-mcp`'s `paths::confine` does) and confines it again when the file is opened. Dotfiles, `.git`, ignored folders, binaries and anything outside the manifest are never offered |
| Forged authorship, overlaps and reorderings (an update creating another participant's items, or items the receiver already holds, or not next in line) | Every update is admitted before `yrs` reads it (`Replica::apply_remote` with its `Sender`): each participant's items must be one run starting exactly where the receiver's copy of that participant's work ends, and a guest's update may hold only the guest's own items (the host assigns each guest its `yrs` client id at Admit). In the star an honest update always passes. This is a memory-safety rule as well as an authorship one: `yrs` 0.28.2 integrates a block overlapping items it holds into freed memory, reporting success (a use-after-free found by COLLAB 2/8's fuzz target), so such a block must never reach it, from a guest or from the host. Deletions name no author and are admitted; the engine checks who may delete |
| Oversized or decompression-bomb updates | `MAX_FRAME` checked from the length prefix before reading; `MAX_UPDATE` before decoding. `yrs` 0.28.2 reserves memory for as many participants, blocks, map entries or values as an update claims before reading any, and a hash table's reservation is touched as it is made, so six bytes cost 8.6 GB (found by COLLAB 2/8's fuzz target). So every update is checked first against the grammar of what a text replica sends (`adoc-collab`'s `wire.rs`): string, deleted, garbage-collected and skipped blocks of the one root text, ids within 53 bits, every count bounded by the bytes left; no map, array, JSON, embed, format or type content ever reaches the decoder. State vectors are checked the same way. Decoding runs inside a panic guard. The text after applying is held to `MAX_SHARED_FILE`, the replica poisoned otherwise |
| An update that changes text its byte edits do not show (`yrs` counts in UTF-16 units, and an item placed inside a character splits it into replacement characters) | Every remote update's result is compared whole with the text before it and the edits reported, and a mismatch poisons the replica: the host rebuilds it from its own text in a new epoch and drops the sender |
| An update that leaves a state whose encoding rebuilds a different text (applied consistently by everyone connected, but a later joiner would diverge) | Every snapshot is checked by building a replica from it and comparing texts (`Replica::snapshot`); a mismatch makes the host rebuild from its own text in a new epoch. A reconnecting guest is sent a checked snapshot, not a diff. A replica holding a pending update encodes nothing (`CollabError::Pending`) |
| A valid-looking update that `yrs` parks as pending, to stall the host | The host never resynchronises from a guest. An update from a guest that leaves the host with anything pending drops that guest, and the host rebuilds the file's replica from its own text in a new epoch |
| An update from a stale session or epoch | Every `Update`, `SaveRequest` and `Awareness` carries its file's epoch, and the receiver refuses a mismatch before decoding |
| An unauthorised save, role change or command | Each is a request the host checks against the peer's current role. Guests cannot build, export, run commands, use Git, create, rename or delete files, or reach MCP tools; the protocol has no message for any of them |
| One session's updates sent into another | Connections are bound to one session; a session id that does not match closes the connection |
| A slow or flooding peer | Bounded queues and rates (§8); a peer over a limit is disconnected, and the host's editor never waits for the network |
| Leaks through logs | No secret, token or document text in logs at default levels; paths are logged root-relative |
| Leaks through previews, links and diagnostics | The guest's engine sees only the manifest's files; out-of-scope targets are placeholders (§11) |

## 7. Identifiers and epochs

- `SessionId`: 128 random bits, per Share.
- `PeerId`: a small number the host assigns at Admit, never reused in a session. The host owns `PeerId` 0.
- `yrs` client id: the host assigns a random 53-bit id per peer per snapshot, distinct within the session, and tells the peer in `Snapshot`. A file's text is seeded under a client of its own, derived from the host's by flipping its top bit, which never types: `yrs` merges a client's consecutive items, and text typed at the end of a long page seeded under the typist's own client would merge into the whole page on every keystroke (COLLAB 8/8).
- `SharedFileId`: a counter the host assigns per session, opaque to guests.
- `Epoch`: a number per shared file, starting at 1. It is bumped whenever the file's replica is rebuilt from text: a reload from disk, a host-initiated resynchronisation, or a projection failure.
- None of these is the engine's `DocumentId` or `OpenDocument::version`, which stay local to a process and are never sent. A state vector is never treated as permission.

## 8. Protocol, version 1

**Frames.** Each frame is laid out as follows:
- a 4-byte big-endian length, at most `MAX_FRAME`;
- a 1-byte message kind;
- a 2-byte big-endian length of the control part, then the control part as JSON (`serde_json`, already in the workspace);
- then any binary payload: a `yrs` v1 update, state vector or sticky index.

An unknown kind, malformed JSON, or a field outside its limit closes the connection with `Error`.

**Messages.**

| Message | From | Holds |
|---|---|---|
| `Hello` | both | Protocol versions supported (`min`, `max`), the application's version, capabilities. The host answers with the version chosen, or `Error { incompatible }`, and both show it |
| `Authenticate` | guest | The invitation secret or a resume token; a display name |
| `Join` | host | The decision after Admit: `PeerId`, role, session id; or `Error { denied }` |
| `Roster` | host | Who is in the session (each participant's `PeerId`, name, role, whether connected, the shared file they are in or none, and whom they follow, the host first) and the shared folder's name; sent after `Join` and whenever it changes |
| `Focus` | guest | The shared file the guest is in, or none (on a page of its own, an image or nothing), and whom it follows; sent when either changes and again after `Join` or resuming; a file not in the manifest closes the connection. The host tells the others in its `Roster`, at most once a pump (#767) |
| `Comment` | guest | A change to a shared file's comments (#762): add, with the range as a selection relative to the shared text, reply, resolve or reopen, edit, or delete, numbered by `request`; the host makes it as the guest if the guest may, and a request from an earlier epoch is refused |
| `CommentThread` | host | One comment of a file and its replies, with its place as a selection (none while detached) and names only, never an e-mail address; sent after the file's snapshot, and again whenever it changes or its place would resolve otherwise on the guest's side, at most 32 a pump per file |
| `CommentRemoved` | host | A comment of a file was deleted |
| `CommentRefused` | host | Why the host refused a guest's `Comment` request, naming it; the connection stays |
| `Manifest` | host | The shared files: `SharedFileId`, root-relative display path, kind (page or partial), size; in parts of at most `MANIFEST_PART` files, `more` until the last, so each control part fits its two-byte length whatever the paths |
| `Components` | host | The Antora component versions the shared files belong to: each one's folder from the shared root, name and version, words the protocol checks, and nothing else of its `antora.yml`; at most `MAX_COMPONENTS`, checked whole as it is built so it fits one frame; sent after the manifest (COLLAB 7/8) |
| `OpenFile` | guest | A `SharedFileId` |
| `CloseFile` | guest | A `SharedFileId` the guest no longer follows |
| `Snapshot` | host | File id, epoch, the peer's client id for that epoch, the whole state as a `yrs` update |
| `Update` | both | File id, epoch, a per-sender sequence number, `base` (from a guest, the `client` of the snapshot the change was made on; from the host, 0), a `yrs` update |
| `UpdateAck` | host | File id, epoch, the highest sequence number applied |
| `Awareness` | both | File id, epoch, the participant's `PeerId` (the host sets it to the sender's when forwarding, whatever the guest wrote), the selection as two encoded sticky indexes; throttled to one per `AWARENESS_INTERVAL` |
| `SaveRequest` | guest | File id, epoch, a request id |
| `SaveResult` | host | Request id; `Saved { revision }`, `Queued`, `Conflict`, `PermissionDenied`, `Stale` (the request named an epoch the file has moved on from) or `Failed { reason }` |
| `FileState` | host | File id, epoch, the revision on disk while the shared text is that text (else none), and whether a change made outside the session waits for the host; sent after every `Snapshot` and whenever it changes |
| `RoleChanged` | host | A peer's new role |
| `Resync` | guest | File id, epoch, the sender's state vector; the host answers with a checked `Snapshot` (§6), never a diff, and never asks a guest for one |
| `Leave` | both | A reason; the connection closes after it |
| `Error` | both | A stable code and a message; a fatal one closes the connection |
| `Heartbeat` | both | Nothing; sent every `HEARTBEAT_INTERVAL`, so a link silent for `LINK_TIMEOUT` is down |

**Acknowledgements.**
- The host acknowledges a guest's `Update` only after applying it (and forwarding it, if more guests exist).
- Sequence numbers are per sender and per file, and start at 1 after every `Snapshot` the host sends that peer: a snapshot replaces whatever the receiver had, so what was in flight before it means nothing after. The host discards a guest's updates to a file after sending it a snapshot until one numbered 1 arrives; the guest numbers from 1 once it has applied the snapshot.
- Numbers alone cannot tell an update sent before a snapshot from one after it, and an update that only deletes names no author, so each update names its snapshot's `client` as `base`. The host discards an update whose epoch or base is not the peer's latest, and one from the document's previous epoch when the host has started the file again and not yet told the peer; the new snapshot follows (COLLAB 8/8).
- An update numbered below the next expected was applied already: after a reconnection the guest sends again the updates after the first acknowledgement it receives, and an acknowledgement sent over the new link may cross them. The host discards it. A gap closes the connection. So does an update the receiver's replica does not admit (§6).
- An update from a peer whose role no longer allows editing (taken away while it was on its way) changes nothing; the host sends `Error { unauthorised }` and a new snapshot, and the guest starts again from the host's text.
- The interface shows a guest's edit as Acknowledged once its sequence number is acknowledged. Acknowledged is not Saved (§10).

**Limits** (constants in `adoc-collab`, each with tests at its bound; confirmed by COLLAB 2/8):

| Constant | Value |
|---|---|
| `MAX_FRAME` | 16 MiB and 64 KiB, room for `MAX_UPDATE` and its control part |
| `MAX_UPDATE` | 16 MiB: a whole file's snapshot must fit, its text at most `MAX_SHARED_FILE` and the history around it (COLLAB 2/8 raised it from 4 MiB, which no 4–8 MiB file's snapshot fit) |
| `MAX_SHARED_FILE` | 8 MiB of text |
| `MAX_OPEN_FILES` | 64 per peer |
| `MAX_PARTICIPANTS` | 8 (v1 admits one guest) |
| `MAX_QUEUED` | 1,024 frames or 16 MiB per peer, outbound (`MAX_QUEUED_FRAMES`, `MAX_QUEUED_BYTES`); a guest also stops editing with 1,024 updates unacknowledged |
| `MANIFEST_PART` | 24 files a `Manifest` message; `MAX_MANIFEST_FILES` 100,000 in all |
| `HANDSHAKE_FRAME` | 4 KiB: the largest frame read before a connection has authenticated |
| `MAX_PENDING` | 8 connections not yet admitted; more are closed at once |
| `AWARENESS_INTERVAL` | 50 ms |
| `INVITE_LIFETIME` | 10 minutes |
| `HANDSHAKE_TIMEOUT` | 10 s, from accepting a connection to its `Authenticate` |
| `HEARTBEAT_INTERVAL`, `LINK_TIMEOUT` | every 15 s; a link silent for 45 s is down |

**Backpressure.**
- The transport and the engine exchange messages through bounded channels.
- The engine processes at most `SESSION_EVENTS_PER_PUMP` (64) link events per `pump`, and sends its documents' updates; while more wait, or updates are unsent, the next pump is due at once (`Engine::next_due`, with a test in `tests/wake.rs`). The transport calls the front end's waker when an event arrives.
- The transport's queue to the engine holds `EVENT_QUEUE` (256) events; a peer whose message would be one more waits, so a flood holds up only its sender.
- A peer whose outbound queue is full is disconnected (`Error { slow }`), and it may resume. The engine never waits on the network, and the transport never holds the engine.

**Disconnecting and reconnecting.**
- A disconnected guest cannot edit. Its last text stays visible and copyable, marked unsaved, and no edit typed while disconnected is accepted.
- While the session lives, the guest reconnects with its resume token, which the host's `Join` replaces. For each file the guest has open, the host sends `UpdateAck` with the last of the guest's updates it applied, and forwards nothing more to it yet. The guest keeps every update until it is acknowledged; it sends again those after the acknowledged one, then `Resync`, and the host answers with a checked `Snapshot`, which already holds them. So an edit made before the link dropped arrives once, and nothing the host sent while the guest was away is lost.
- If the epoch no longer matches, the host sends the new epoch's `Snapshot` instead, and the guest's replica is rebuilt from it; its updates of the old epoch are dropped. A guest away when the epoch moved keeps its old epoch recorded on the host, so resuming always finds the difference; a `Resync` from an earlier epoch is answered with a snapshot too.
- A guest's `Resync` sends its edits not yet sent first, so the snapshot answering holds them. A snapshot that reaches a resuming guest before the host has read its return is followed by a `Resync`, since the host's acknowledgement then waits to be asked (COLLAB 8/8).
- A host restart is a new session; the guest rejoins explicitly with a new invitation.

## 9. The edit bridge (COLLAB 3/8)

- **Local edit of a shared file:**
  1. The engine validates the `TextEdit`s against the rope, converts them to `ByteEdit`s and applies them to the replica as one transaction.
  2. It compares the delta the replica reports with the edits it asked for. A mismatch fails closed (§1).
  3. It applies the edits to the rope through the existing `edit_document` path, with `EditOrigin::Local`.
  4. It queues the update for the transport.
  - A refused command changes neither the replica nor the rope, and sends nothing.
- **Remote update:**
  1. On the engine's thread: role, file, epoch, sequence and authorship are checked first.
  2. Then `apply_remote`, giving byte edits.
  3. Then `edit_document` with `EditOrigin::Remote`: one incremental patch per update, never a whole-text replacement, never echoed, never entered into local undo.
  4. Then the pending check (§2).
- **One gateway:** every text-producing route already reaches `apply_edits` (or `edit_document`, for undo and redo): typing, IME, paste, visual steps, spelling replacements, table formatting, case changes, a reload. The bridge sits in `apply_edits` (`Engine::apply_shared_edits`), and nothing is special-cased. As built in COLLAB 3/8: `share_document`, `open_shared`, `apply_shared_update` with its `SharedReceipt`, `take_shared_updates`, `reset_shared`, `stop_sharing`.
- **Events:** `EngineEvent::DocumentChanged` gains the edit's origin. Existing listeners that ignore it are unaffected.
- **Verification:** whole-text comparisons of rope and replica cost about 90 µs a keystroke on a 1 MiB page (record, finding 8). They run in tests, after a resynchronisation and before a save, not on every keystroke.

## 10. Undo, saving and recovery

- **Undo:**
  - When a file becomes shared, its `UndoStack` is dropped, with a one-time notice: its byte ranges cannot be replayed across merged edits.
  - While shared, undo and redo go to the replica's `UndoManager`, which tracks only the local origin.
  - Groups end where the engine says (`end_group`): the `COALESCE_WINDOW` and caret rules typing uses today, and every non-typing command's boundary. The manager's own clock is not used.
  - On leaving, the replica's history goes with it and the file starts a new, empty `UndoStack`.
  - Commands that edit several files as one group (rename, move, New Page's navigation entry) are refused while any file they touch is shared.
- **Saving:**
  - Only the host writes files, through `Engine::save` and the writer, with their conflict checks. Host auto-save writes the merged text. A guest's own Save is refused; it asks the host (`Engine::request_shared_save`).
  - A revision is the SHA-256 of the saved text, in hexadecimal (`revision_of`). COLLAB 5/8 changed this from the replica's state vector: a deletion does not move a state vector, so two texts would share one. A guest computes the same hash from its own text.
  - A save takes its text on the engine's thread as the writer's snapshot, and records that snapshot's revision. When the write succeeds, the file holds that revision, whatever was typed since; edits that arrived during the write leave the document unsaved, and no answer claims them.
  - A guest's `SaveRequest` is answered by the host's rules: only an Editor, only for the file's current epoch (`Stale` otherwise), never in a conflict (`Conflict`). A file already saved is not written again (`Saved` with its revision), so repeating a request changes nothing. The host makes the save at once and waits for it, as its own Save does.
  - The host announces each file's state (`FileState`) when it changes. A guest is saved only while it is connected and its own text hashes to the revision last announced; out of touch it knows nothing (`Unknown`), whatever it was told before.
- **Changes on disk:**
  - A shared file changed on disk (Git, another editor, a tool) is never reloaded or overwritten automatically, and the change never flows in as an edit.
  - The file enters a conflict state, reported by the file watcher or found by a save's own check; the file being removed is one too. Nothing is saved while it lasts (but an overwrite the host itself chooses), and it clears if the file goes back to what was saved.
  - The host chooses (`Engine::resolve_disk_conflict`): keep the shared text (what is on disk becomes the base the next save expects, so the next save writes the shared text, and a further change conflicts again), or reload from disk (a new epoch: every peer gets a `Snapshot`). Saving the shared text as a copy beside it is the app's to offer (COLLAB 6/8), with the file commands it already has.
- **Recovery:** recovery records keep accepted, unsaved shared text on the host as they keep any dirty buffer. Restarting the host recovers the text without restarting sharing. A guest keeps nothing: its shared documents have no recovery records and are not in the window session.

## 11. Scope, previews and media

- **The manifest:** the host offers the `.adoc` pages and partials under the shared root that its catalog and index know, minus dotfiles, `.git`, `[workspace] ignore` folders and anything reached through a link leaving the root. Display paths are root-relative with `/`.
- **The guest's engine:**
  - It sees only the manifest's files, through a virtual-file resolver over the replicas it has open: a folder of the session's in the temporary folder, never read from disk, with a catalog built from the manifest and `Components` and an index of the pages open (COLLAB 7/8). It renders previews itself, from its own replica text.
  - A cross reference or include to a file outside the manifest is an "unavailable" placeholder.
  - An image is its alt text with a placeholder: v1 shares no media.
  - Diagnostics are root-relative and never name a file outside the manifest.
  - Host-rendered preview data was rejected: serialising view trees would be a new attack and maintenance surface.
- **Partials:** a partial is editable only as its own shared file. Included text stays read-only where it is included, as the visual editor already treats it.

## 12. Test matrix for COLLAB 2/8 to 8/8

The PRD's §10 names the tests; this adds those the spike's findings call for.

| Package | Tests |
|---|---|
| C2 `adoc-collab` | `collab_replicas_converge` (one and two guests in the star, 1,000 seeds each); `collab_edits_are_byte_exact`; `collab_local_undo_keeps_remote_edits`; `collab_malformed_update_changes_nothing`; `collab_offsets_inside_a_character_are_refused`; `collab_pending_update_asks_for_resync`; `collab_authors_are_read_before_applying`; `collab_limits_hold_at_their_bounds`; `duplicates_and_reordering_are_refused_unread`; `an_update_splitting_a_character_poisons_the_replica`; `counts_claiming_more_than_follows_are_refused`; `a_guest_update_left_pending_makes_the_host_rebuild`; the fuzz target `collab_update` in `fuzz/` (both senders) |
| C3 engine bridge | `collab_engines_converge`; `collab_remote_edit_is_not_echoed`; `collab_visual_edits_share_the_bridge`; `collab_off_changes_nothing`; `collab_projection_failure_fails_closed`; `collab_undo_groups_follow_the_engine` |
| C4 transport | In `adoc-share`'s `tests/transport.rs`, over loopback TLS with raw peers that break the protocol: `collab_loopback_session_syncs`; `collab_unauthorised_requests_are_refused`; `collab_wrong_fingerprint_sends_nothing`; `collab_slow_peer_never_blocks_host`; `collab_nothing_is_sent_before_admit`; `collab_frame_limits_close_the_connection`. In the engine's `tests/collab_session.rs`, over an in-memory link that encodes every frame and can lose what is in flight: `collab_forged_author_is_refused`, `collab_unauthorised_requests_are_refused`, `collab_nothing_is_sent_before_admit`, `a_reconnected_guest_delivers_each_edit_once`, `a_role_taken_away_while_edits_are_in_flight`, `collab_manifest_offers_only_what_may_be_shared`. `adoc-collab`'s `tests/protocol.rs` and the fuzz target `collab_frame` |
| C5 saving | In the engine's `tests/collab_save.rs`: `collab_save_writes_converged_text`; `collab_external_change_is_a_conflict`; `collab_stale_epoch_is_refused`; `an_edit_during_a_save_is_not_claimed_saved`; `a_change_the_watcher_reports_is_a_conflict_never_an_edit`; `reloading_from_disk_starts_everyone_again_in_a_new_epoch`; `a_failed_save_keeps_the_file_and_the_shared_text`; `a_guest_out_of_touch_does_not_know_what_is_saved`; `a_guests_text_is_never_kept_for_recovery_but_the_hosts_is` |
| C6 interface | `collab_share_needs_confirmation`; `collab_viewer_cannot_edit`; `collab_remote_carets_follow_edits` |
| C7 scope | `collab_pages_converge_independently`; `collab_scope_never_leaks`; `collab_structural_commands_refused` |
| C8 release | `collab_simulation_converges`; `collab_two_process_release_suite`; budgets from the record's measurements; `docs/qa/collab.md` |

## 13. Still open

- **Platforms (C8):** the firewall prompts and interface lists on Windows, macOS and Linux, and whether a signed app is needed to listen without repeated prompts on macOS.
- **Media for guests (C7 or later):** sharing images read-only, individually authorised.
- **Upstream:** reported on 2026-11-20 (y-crdt/y-crdt#778, #779, and two private security reports). When `yrs` fixes them, the checks in `adoc-collab` stay: they are the shape of what a session sends, not only workarounds.
