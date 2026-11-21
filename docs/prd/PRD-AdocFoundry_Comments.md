# Comments and Page Metadata

**Issue:** #762 (comments on shared text), a follow-on of the collaborative editing epic #686.
**Author:** Paul Snow. **Version:** 0.0.0. **Status:** design, for approval.

## 1. Intent

Readers and writers of a page want to leave notes on its text, reply to them and resolve them, without the notes ever becoming part of the page. A note on a sentence should stay on that sentence while people type around it, alone or in a collaboration session. It should still be there next week, in another copy of the repository, after a `git pull`.

The maintainer chose to keep such notes in a **metadata file beside the page**, as an `.xmp` file sits beside a photo: `install.adoc` has `.install.adoc.afd` ("AdocFoundry"). Comments are the first kind of metadata the file holds; later kinds (suggested changes #763, perhaps authorship #761) go in the same file. The file belongs to the page, not to a session, so comments work whenever the page is open.

## 2. Decisions taken by the maintainer

- Comments are kept after a session, in a sidecar file beside the page, never in the page itself.
- The sidecar is `.<file name>.afd` in the page's folder: `pages/install.adoc` → `pages/.install.adoc.afd`.
- The format is TOML.
- The sidecar is general page metadata; comments are its first section.
- Comments belong to the page (option A): seen, added, replied to and resolved alone or in a session.
- Approach 1 (section 4): comments are held by the page in the engine; in a session the host holds them and decides who may do what.
- For #761, a participant's identity comes from their global Git settings, editable on the Join screen. Comments use the same identity (section 6).

Calls made in writing this design, for the maintainer to check, are marked **(call)**.

## 3. Goals, non-goals, anti-goals

**Goals**

- Add a comment to a range of text; reply; resolve and reopen; edit and delete one's own.
- A comment follows its text through edits made here, edits made by participants, undo and redo, and changes made outside the app.
- A comment whose text is gone is kept, shown as detached, with the text it was on.
- Comments are kept in `.<name>.afd`, written so that a Git diff of it is readable and changes only when comments do. The page's `.adoc` text and its diff never change.
- In a session every participant sees the comments of the shared pages they have open; Viewers can comment and reply but not edit text.
- Reachable by keyboard, with accessible labels; colour never the only cue.

**Non-goals (this design)**

- Suggested changes (#763), authorship (#761): later kinds of metadata in the same file.
- Comments on images or on files that are not AsciiDoc.
- Showing comments in exports, the preview or the published site.
- Comments through the CLI, the language server or the MCP server.
- Re-attaching a detached comment to a new range by hand.
- Notifications, mentions, reactions.
- Authorship as a security boundary (section 6).

**Anti-goals: a failure even if it works**

- Any change to the bytes of a `.adoc` file, or to what Antora publishes.
- A comment silently lost: overwritten by a save, dropped by a reload, or discarded because the file held something the app did not understand.
- A sidecar from a repository someone else controls able to crash, hang or exhaust the app, or to make it write outside the page's folder.
- Comments slowing typing on a large page.
- Sidecars conflicting in Git when only the page changed.

## 4. Approaches

1. **Held by the page in the engine (chosen).** Each open AsciiDoc page keeps its comments as byte ranges, mapped through every edit the page's text takes. Every change reaches the rope through `edit_document`: typing, undo and redo, a participant's update, and a change on disk applied as an edit (`open_document_changed_on_disk`, `DiskResolution::ReloadDisk`). So the ranges stay right in every case. In a session the host holds the list and decides who may add, reply, resolve, edit and delete. Positions cross the wire as the session's `RelativeSelection`s, as remote carets do. The sidecar stores each range, its quoted text and some text either side; reopening a changed page finds each comment by its quote.
2. **Inside the shared `yrs` document.** Comments synced as `yrs` data beside the text. Rejected: `adoc-collab`'s checks admit only a text replica's grammar, built that narrowly after fuzzing found memory bugs in `yrs`; widening them risks that work. It would also give nothing outside a session.
3. **Quote search every time.** Find each comment by its quoted text on every change. Rejected: a phrase that occurs twice leaves the comment guessing, and it moves when someone types near it. Approach 1 uses quote search only when a page is opened or its sidecar is read again.

## 5. The sidecar file

### 5.1 Name and place

- For an AsciiDoc page the engine opens in a workspace root (pages, partials, navigation files): the same folder, `.` + its file name + `.afd`.
- **Antora skips it.** The leading dot keeps it out of Antora's content catalog and publishing. Antora 3.2's documentation says: "Hidden files do not get added to Antora's content catalog … and aren't published".
- **The engine's own catalog must skip it too (call).** It takes hidden files today: `collect_files` in `adoc-antora/src/discovery.rs`, and `resource_at` and `coordinate_in` in the engine, have no hidden-file filter. COMMENTS 1/4 adds one to both, to match Antora. Hidden files are then left out of the Modules panel, xref completion, impact analysis and Git snapshots, as Antora leaves them out, with a test in `adoc-antora` and one in the engine. The engine's index already leaves `.afd` files out by their extension (`is_asciidoc_path`).
- The Files view lists it like any file. Git sees it like any file; the app neither ignores nor stages it specially.
- A page not yet saved (untitled) keeps its comments in memory until it is saved, then writes them.
- **No sidecar for some pages (call).** A page reached through a link, or in a hidden or `[workspace] ignore` folder, gets none. A page is confined as `offer()` in `collab_session.rs` confines a shared one, but with every AsciiDoc extension `is_asciidoc_path` accepts and no size limit: a page too large to share can still carry comments.

### 5.2 Contents

TOML, UTF-8. Illustrative:

```toml
# AdocFoundry page metadata
format = 1

[[comment]]
id = "7d1e0c3a9b2f4e61"
author = "Paul Snow"
email = "paul@example.org"        # only when the author gave one
created = 2026-11-21T06:30:00Z
start = 1204                      # byte offsets into the page as it was when the comments last changed
end = 1251
revision = "sha256:9f2c…"         # SHA-256 of that text
quote = "the installer refuses an unsigned package"
before = "On macOS "              # up to 32 bytes before `start`, cut at a character boundary
after = ", so sign it first."      # up to 32 bytes after `end`
text = """
Is this still true for 0.14?"""
resolved = false

[[comment.reply]]
id = "a40b7c11d2e94f08"
author = "Ada"
created = 2026-11-21T06:41:00Z
text = "Yes, notarised builds only."
```

- `format` is the file's version. A file with a higher `format` is read for what this version understands and never rewritten, so a newer app's data is never lost. Comments on that page are read-only, with a notice.
- Unknown keys and tables at any level are kept as they are when the file is rewritten (`toml_edit`, as `Engine::save_appearance` does). So later kinds of metadata, and other tools' data, survive.
- **The revision is per comment (call).** Each comment carries the `revision` its offsets refer to. The file is rewritten only when comments change (5.5), so a comment written last month keeps last month's offsets and revision until someone touches a comment on that page.
- `detached = true` marks a comment whose text was not found; its `quote` stays.
- `edited` (a time) appears once a comment or reply has been edited; `resolved_by` and `resolved_at` once resolved.
- Ids are 64 random bits as 16 hex digits, unique within the file.
- **Times (call).** They are UTC, RFC 3339, from the wall clock. `clock.rs`'s `Clock` gives only a monotonic `Instant`, so it gains a wall-clock method, so that tests can fix the time.
- Comments are written in the order of their `start`, detached ones last, so a diff stays small.
- A file holding Git conflict markers is not TOML. The notice for an unusable file (5.3) says so in that case: "The comments file has a merge conflict; resolve it to see this page's comments."

### 5.3 Limits

A sidecar may come from a repository someone else controls. **The limits (call)** apply both when reading and when writing:

| | Limit |
| --- | --- |
| File | 4 MiB |
| Comments | 500 |
| Replies a comment | 100 |
| Text of a comment or reply | 8 KiB |
| `author` | 64 bytes (`MAX_NAME`) |
| `email` | 254 bytes |
| `quote` | 4 KiB; a longer range keeps its first 4 KiB, and is found by that |
| `before`, `after` | 32 bytes |

- **Reading:** past any limit, not TOML, or wrongly typed, the page's comments are not shown and a notice says why. The file is **never rewritten** while it stays so, and adding a comment there is refused with that reason.
- **Writing (call):** a change that would take the file past a limit is refused before anything is written: "This page has as many comments as it can hold", or "This comment is too long". A file the app writes is always one it reads.
- Offsets past the text, or not on character boundaries, are treated as a changed page (found by quote).
- Nothing in it names another file; it is read only as the page's own neighbour.
- Its parser gets a fuzz target in `fuzz/`.

### 5.4 Ranges through edits, and finding comments again

**While the page is open (call):**

- A range is mapped through every edit.
- Text typed at either edge falls outside the range; text typed inside it grows it.
- Deleting part of a range shrinks it. A comment becomes detached only once its range is empty.
- `quote` follows the range's text, and is written as it is then.
- A detached comment keeps the quote it had when its range emptied.

**When the sidecar is read (on opening a page, and when the sidecar changes on disk):**

- A comment whose `revision` matches the page's text is at `start..end`.
- Otherwise it is searched for by `quote`. Every occurrence is scored by how much of `before` and `after` matches around it and by its distance from the old `start`; the best wins, and none means detached.

**Detached comments are searched for again (call)** by their quote, near where they were, each time the page settles after typing (`INDEX_SETTLE`). So undoing the deletion of a comment's text brings the comment back. Undoing a deletion of part of a range does not grow the range back: the range stays as it shrank. This is a known limit, and keeps comment ranges out of the undo history.

### 5.5 Writing

- **The file changes only when comments do (call).** A comment added, replied to, resolved, reopened, edited, deleted or detached rewrites the file. Each changed comment gets the offsets, quote, context and `revision` of the page's current text, saved or not. A page save does not rewrite it. So two branches that edit a commented page, and no comment, never conflict on its sidecar. A comment's stored offsets may be old; reading finds it by its quote (5.4).
- **Through the writer (call).** Only the engine's writer thread writes sidecars (`writer.rs`), in order with page saves, through a temporary file renamed over the old one, after a short settle so that typing a reply writes once. Its write is a new job kind, not `Job::Write`, which neither checks nor reports. It:
  - checks first that the file on disk is still the one last read, by its hash, as page saves check;
  - reports success or failure;
  - on a mismatch, writes nothing and reads the file again (5.6);
  - on a failure, keeps the change in memory and shows a notice, never dropping it.
- The last comment deleted, with nothing unknown left in the file, removes the file.
- Comment changes are not part of the page's undo history and never mark the page unsaved.

### 5.6 Changes on disk

- The watcher reports a changed sidecar (a `git pull`, another editor). The engine reads it again and finds its comments.
- A change made here and not yet written is applied on top, matched by id. A comment made here is never lost; one deleted on disk and edited here is kept.
- A sidecar removed on disk while the page is open is written again only when its comments change here.

### 5.7 Moving with the page

- **Rename and move (call).** `page.rename`, and the staged moves of `staging.rs`, move the sidecar with the page in the same undo group. The sidecar moves along a path of its own, not as a `FileMove`, since `apply_group` opens every moved file as a document. Undoing the move puts the sidecar back, and a failure to do so is reported, not dropped as `replay_moves` drops errors today.
- **Deletion (call).** `file.delete` moves the sidecar into the same deletion folder, as a third named entry beside `file` and `path` (`sidecar`). `read_entry` accepts an entry with or without it. `file.undo_delete` and `file.restore_deleted` put both back.
- **A sidecar already where the page lands (call).**
  - A rename, a move or a restore whose new path already has a sidecar is refused: "A comments file is already at …".
  - A new page made where an old sidecar was left takes it: its comments are found by quote or shown detached, where they can be seen and deleted.
- A page moved or deleted outside the app leaves its sidecar where it was.

## 6. Who writes a comment

- **Alone:** the author is `[comments] name` and `email` in the user settings file if set. Otherwise it is the global Git identity, `user.name` and `user.email`, read through a new `adoc-git` function that reads the user's global Git configuration without a repository (`Repository::identity` needs one). Git's identity is skipped without the `git` feature. Failing both, it is the operating system's user name. The e-mail is optional.
- **Hosting:** the host's own comments carry the same identity as alone.
- **As a guest:** the host names a guest's comments from its roster: the name the guest joined with, and the e-mail once #761 sends it. A guest never names an author in its messages. **The name a guest joins with (call)** defaults to the same identity, the comments name and then the global Git name, in place of the operating system's user name (`own_name` in the app's `collaboration.rs`); the Join screen still lets them change it.
- **What counts as "one's own" (call):** the same e-mail when both have one, else the same name. This is a courtesy, not a security boundary. The host admits each participant by name, and anyone with the repository can edit the sidecar. Alone, the user may do anything to any comment, since the files are theirs.
- **Permissions in a session:**
  - Anyone, Viewers included, adds comments and replies.
  - Resolving and reopening: the host, Editors, and a comment's author.
  - Editing or deleting a comment or reply: its author; the host may delete any.

## 7. In a session

- Guests never read or write sidecars. The host reads the sidecar of each shared file when it opens the file for a guest, since it is the host's page, and sends the file's comments after the snapshot.
- **The host keeps byte ranges as the truth (call).** Every edit on its copy reaches its rope through `edit_document`, so byte ranges stay right. A `Rebuilt` or `ReloadDisk` replica does not touch them. `RelativeSelection`s are made from the host's replica only to send.
- **Guests hold `RelativeSelection`s,** resolved in their replica when drawn, as `peer_carets` are, so they converge with the host's text.
- **New messages (call)**, held to section 5.3's limits and to the frame's (`MAX_CONTROL`, `MAX_FRAME`).
  - Every message carries `file` and `epoch`. One from an earlier epoch is discarded, as updates are.
  - Each request carries a `request` number, so that a refusal can name it.
  - Guest → host:
    - `CommentAdd { file, epoch, request, selection, text }`
    - `CommentReply { file, epoch, request, comment, text }`
    - `CommentResolve { file, epoch, request, comment, resolved }`
    - `CommentEdit { file, epoch, request, comment, reply?, text }`
    - `CommentDelete { file, epoch, request, comment, reply? }`
  - Host → guest:
    - `CommentThread { file, epoch, thread }` is one comment with its replies and its selection in the replica, the text in the binary payload. The host sends every thread of a file after its snapshot, and each thread again whenever it changes, at most once a pump.
    - `CommentRemoved { file, epoch, comment }`.
    - `CommentRefused { file, request, reason }`.
- A request the role does not allow, or whose selection does not resolve in the host's replica (its edits discarded after a reconnection), gets `CommentRefused`; the connection stays, and the guest's app shows the reason. A malformed message, or one naming a file not shared, closes the connection (`Protocol`), as `Focus` does.
- After a new epoch (`Rebuilt`, `ReloadDisk`, a resync) the host sends every thread again with the new snapshot.
- The host's own comment changes, and each change it accepts from a guest, are written to the sidecar as in 5.5.
- The `comments.*` commands are allowed while hosting and on a guest's page. They join `ALLOWED_WHILE_HOSTING` in `every_command_is_decided_for_a_session`.
- The simulation (`tests/collab_simulation.rs`) gains operations to add, reply to and resolve comments. After each settle it checks that every participant with a file open has the same comments, at ranges covering the same text.

## 8. In the app

- **Add Comment** (Cmd-Alt-M / Ctrl-Alt-M) on the selection in the source or visual editor opens a comment box in the Comments panel. It is allowed whenever the page is AsciiDoc and its sidecar is usable, a Viewer's page included.
- **The editors** mark commented ranges with a fill and a gutter mark. The source editor uses the widget's range and gutter decorations, as remote carets do; the visual editor draws over its text layouts. Resolved comments are not marked. With the caret inside a commented range, the status bar names the comment.
- **The Comments panel** is a sidebar tab, `SidebarTab::Comments`.
  - It lists the active page's open comments in text order, each with its author, time, quoted text and replies. Reply, Resolve, Edit and Delete are offered as allowed.
  - A Resolved section is folded by default.
  - A Detached section shows each comment's quote.
  - Choosing a comment selects its text.
  - A page whose sidecar is unusable shows the reason.
- **Commands,** all in a menu and the palette: Add Comment, Next Comment, Previous Comment, Show Comments. Each button has an accessible label naming the comment's author and quote.
- Collaboration off changes nothing here: comments are a page feature.

## 9. Engine surface (for the builder to shape)

- **Commands:** `comments.add`, `comments.reply`, `comments.resolve`, `comments.reopen`, `comments.edit`, `comments.delete`, `comments.next` and `comments.previous`, with `CommandArgs` carrying the selection or the comment's id.
- **Queries:**
  - `Engine::comments(id) -> Vec<Comment>`: the comments resolved to byte ranges in the current text, with `detached`, `resolved` and what the user may do.
  - `comment_at(id, offset)`.
  - `comments_state(id)`: usable, unusable with a reason, or read-only for a newer format.
- **Events:** `EngineEvent::CommentsChanged(id)`.
- **The pump:** every new step in `pump` (settle writes, re-searching detached comments) has its deadline in `next_due` and a test in `tests/wake.rs`.

## 10. Packages

One stack, each package green and documented:

1. **COMMENTS 1/4: the sidecar and comments alone.**
   - `.afd` read and write, limits, the writer's checked job, finding comments again, mapping through edits, undo and redo, changes on disk.
   - Rename and delete; identity, with the global Git identity; the wall clock.
   - The hidden-file filter in `adoc-antora` and the engine.
   - The commands and queries, and a fuzz target.
   - Engine tests, alone.
2. **COMMENTS 2/4: comments in a session.** The messages, the host's authority, guests' relative positions, epochs and the simulation's new operations, with engine and protocol tests.
3. **COMMENTS 3/4: the app.** The Comments panel, Add Comment, the source editor's marks, commands in menus, the Join name's default, and an app test.
4. **COMMENTS 4/4: the visual editor's marks,** and the manual checks in `docs/qa/comments.md`.

The user guide gains a "Comments" page and the developer guide's engine page a section. README, CHANGELOG and the collaboration PRD's §14 note follow.

## 11. Acceptance (from #762, widened)

- A comment follows its text through concurrent edits in both directions (`a_comment_follows_its_text_through_concurrent_edits`).
- Comments never appear in the saved page or its diff (`comments_never_change_the_page`).
- A Viewer can comment but not edit (`a_viewer_can_comment_but_not_edit`).
- A comment survives closing and reopening the page, and a change to the page made outside the app (`a_comment_is_found_again_after_an_outside_change`).
- Saving a page whose comments did not change leaves its sidecar's bytes as they were (`a_page_save_leaves_the_sidecar_alone`).
- A sidecar past its limits or malformed is never rewritten and never panics (`an_unusable_sidecar_is_left_alone`, and the fuzz target).
- A sidecar changed on disk while a change waits to be written loses neither (`a_sidecar_changed_on_disk_keeps_both`).

## 12. Left to the builder

- Module layout in the engine (a `page_meta.rs` for the file and a `comments.rs` for the model is the expectation; a new crate only if the fuzz target needs one).
- The exact scoring of quote matches, within 5.4.
- The settle time before a comment change is written.
- Visual details of marks and the panel, within the theme's colours and the accessibility rule.
- The random source for ids, from what `Cargo.lock` already has.

## 13. Implementation notes

### COMMENTS 1/4: the sidecar and comments alone

Where the implementation departs from this design, or settles what it left open:

- **A comment is rewritten when its own text is edited,** as well as when the comment changes (5.5). Otherwise a page edited inside a commented phrase, then saved and closed, would keep only the old quote, which a later opening could not find. A page edited elsewhere still leaves the file alone.
- **Untitled pages keep comments in memory only.** The engine has no Save As that gives an untitled page a path, so there is never a file to write them to (5.1).
- **Values left as they are written.** A rewrite leaves a key whose value is unchanged exactly as it is written, a hand-written multi-line string included, so a diff shows only what changed.
- **Unusable files are named, not counted.** A file past a limit on reading is `MetaProblem::Invalid` naming the limit, so the notice says which; `TooMany` and `TooLong` are the refusals of a change before it is written.
- **The builder's choices (section 12):**
  - The modules are `src/page_meta.rs` (the file) and `src/comments.rs` (the model). The fuzz target `page_meta` links the engine without default features, so no new crate was needed.
  - A quote's occurrences, at most 1,000, are scored by how many bytes of `before` end the text before each, plus how many of `after` begin the text after it. Nearness to the old start breaks ties.
  - `COMMENT_SETTLE` is 500 ms, and a failed write is tried again after 5 s or at the next change.
  - Ids come from `getrandom`, already in the tree.
- **A page closed with a write still to make** keeps its comments, by path, until the write is made (`closing_comments`); reopening it meanwhile takes them back. `flush_writes`, which dropping the engine calls, writes every page's comments and retries a conflict before returning.
- **Commands are available on any active page** (`CommandContext::ActiveDocument`). The app shows none of them until COMMENTS 3/4, and its user guide page comes with that package; the developer guide's engine page and the settings page describe them now.
