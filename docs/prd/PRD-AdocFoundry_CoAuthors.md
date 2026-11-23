# Co-authorship of Shared Edits

**Issue:** #761, a follow-on of the collaborative editing epic #686 (planned in `docs/prd/PRD-AdocFoundry_COLAB.md` §14, C6a).
**Author:** Paul Snow. **Version:** 0.0.0. **Status:** design, for approval.

## 1. Intent

In a collaboration session only the host writes files and commits, so every commit carries every participant's edits under the host's name. When the host changes file a, a guest changes file b and both change file c, the history should say so: the commits of b and c credit the guest, the commit of a credits nobody else.

Git has a convention for this, which GitHub, GitLab and others show: a `Co-authored-by: Name <email>` trailer at the end of the commit message. This design gives each guest a way to be credited, records on the host who changed which file, and lets the host's commit dialog add the trailers, with the list shown so the host can remove anyone before committing.

Calls made in writing this design, for the maintainer to check, are marked **(call)**.

## 2. Decisions taken by the maintainer

- A guest's identity comes from their global Git settings, editable on the Join screen (as comments already do through `Engine::comment_author`).
- Who changed a file is recorded by the host as it applies each guest's edit (approach 1, section 4), not read from the shared text's item authorship.
- The record lasts until the files are committed, across the session's end and the app quitting, in the app's own state folder (not the page's `.afd` file, which is usually committed, and which images and partials do not have).

## 3. Goals, non-goals, anti-goals

**Goals**

- A guest can give an e-mail address when joining, filled in from their global Git settings, editable, optional.
- The host records, per file, each guest who changed its text in a session (insertions and deletions alike), and keeps that until the file is committed.
- The commit dialog lists the guests who changed any file the commit takes, each with a tick, and the commit adds a trailer for each one left ticked.
- After a commit, the files it took start again with nobody recorded.

**Non-goals**

- Who wrote each line, shown in the editor (the issue's "later, optionally").
- Credit in commits made outside the app (the command line, another Git client), or by the CLI or MCP server.
- Crediting the host's own edits: the host is the commit's author.

**Anti-goals** (a failure even if it works)

- A guest's e-mail address reaching any other guest, a log, a Roster, a comment thread sent over the wire, or the workspace before the host commits (but for the guest's own comments, section 5.5).
- A guest credited for a file they only opened, read, or moved the caret in.
- Credit silently lost because the session ended or the app quit before the commit.

## 4. Approaches

1. **The host records it as it applies each edit (chosen).** Every guest edit already reaches one place on the host, `host_update` in `collab_session.rs`, which knows the sender. When an applied update changes the text, the host notes "this guest changed this file". Deletions count as insertions do; reconnections and rebuilds in a new epoch do not matter, as the record is keyed by the file's path, not by the replica. Small: a few lines in the session and a small state file. It cannot later show who wrote each line.
2. **Read it from the shared text's item authorship** (the issue's first proposal). `yrs` records the client that typed each item, which could later colour lines by author; but a deletion leaves no author behind, so someone who only deleted text goes uncredited, and each guest gets a fresh client id per snapshot, so the host would need a history mapping ids to participants across epochs. More code, more ways to get credit wrong.

## 5. Design

### 5.1 The guest's address, on the wire

- `Message::Authenticate` gains `email: Option<String>`, `#[serde(default, skip_serializing_if = "Option::is_none")]` in its control JSON (which keeps `deny_unknown_fields`).
- `PROTOCOL_VERSION` becomes 2 **(call)**: a guest of this release sends a field an earlier host refuses, so the versions are told apart by `Hello`'s existing negotiation, not by a confusing refusal. Both sides of a session run the same release in practice; collaboration is still off by default.
- `decode` and `encode` hold the address to `commit_email` (a new check beside `display_name`): at most 254 bytes; exactly one `@` with a non-empty part on each side; no whitespace, control character, `<`, `>`, `,`, `"` or `\`. An invalid address is a framing error, as an invalid name is. The fuzz target `collab_frame` covers it through the existing decoder.
- `adoc-share` carries it through: the host's handshake reads it from `Authenticate`, and `LinkEvent::Knock` gains `email: Option<String>`. The guest's `authenticate` takes it beside the name (and sends it again when resuming, as it sends the name).
- The host keeps it on its `Peer` (`email: Option<String>`). It is never put in a `Member`, a `Roster`, a `SharedThread`, an `Error` or any other message to a guest, nor in a log line.

### 5.2 Who changed what, on the host

- In `host_update`, on `SharedReceipt::Applied`, when the update changed the document's text (`apply_shared_update`'s edits not empty, or the text's version moved), the host records the peer against the shared file's path.
- Only a peer with an e-mail address is recorded **(call)**: without one the trailer cannot be written, and the dialog has nothing to offer. The Session panel already names who is editing.
- A guest is identified by their address, compared without regard to case; the name recorded is the latest one they used.
- The record is `CoAuthors`, a new engine module `src/coauthors.rs`:
  - `CoAuthor { name: String, email: String }`.
  - A map from a file's real path (below) to the co-authors recorded for it, each with `changed`, the wall-clock time in milliseconds (`Clock::wall`) of their latest recorded change.
  - `record(&mut self, path: &Path, name: &str, email: &str, at: u64)` moves the stamp in memory and returns whether the state file needs writing, which is only when the guest is new to the file.
  - `for_paths(&self, paths) -> Vec<CoAuthor>`, the union for the files given, deduplicated by address, in the order first recorded.
  - `forget(&mut self, fixed: &[(PathBuf, u64)])` after a commit: each file with the time at which its committed content was fixed; an author whose `changed` is later keeps their record, with that stamp written, so an edit made after that point is credited in the next commit **(call)**.
- When a commit's content is fixed: for the `paths` a commit is given, when `git_commit` saves and stages them; for a file already staged, when the engine staged it (`stage_paths` keeps the stamp per path, in memory), or the commit's start for one staged outside the app. Guest edits keep arriving during a signed commit's wait, and are later than the stamp.
- The path recorded is the shared file's real path, once links are followed (for a file that has gone, its folder's real path and its name), and every path the commit side asks about is made real the same way before it is compared **(call)**. A test shares a page through a linked root and finds the credit in its commit. A file outside every repository is recorded too, harmlessly: no commit ever asks for it, and it is dropped as unmodified (section 5.3).
- Renaming a recorded file in the app (`move_file`) moves its record with it **(call)**; deleting it keeps the record, so committing the deletion credits whoever changed it.

### 5.3 Kept until committed

- `<state_dir>/coauthors.json`, JSON as the engine's other state files are: `{"format": 1, "files": {"<real path>": [{"name", "email", "changed"}]}}`.
- Written by the engine's writer (`write_later`) whenever `record`, `forget`, a move or a prune changed it (rarely: once per guest per file, and once per commit). The writer reads the file as it is then, applies this engine's changes to it and writes it back, so two windows hosting at once do not lose each other's records.
- Read on first use, as untrusted: at most 1 MiB, 10,000 files, 100 authors a file, each name and address held to the protocol's limits; a file that breaks them, or a newer `format`, is ignored and left alone (and nothing is written over it).
- Without a state folder (tests, `adoc-ls`, the CLI) the record is kept in memory only.
- A record whose file is unmodified in Git (its text back to `HEAD`'s, or committed elsewhere) and not open with unsaved changes is dropped when the commit dialog opens, through `Engine::prune_coauthors` (`coauthors_for_commit` only reads) **(call)**: there is nothing left to credit, and it would otherwise credit a later, unrelated change.
- Recording needs no repository, so an edit applied before the host's repository is discovered is recorded like any other.

### 5.4 The commit

- `Engine::coauthors_for_commit(&self, paths: &[PathBuf]) -> Vec<CoAuthor>`: the co-authors of the files the commit would take, which are the `paths` given and every file already staged (the same set the dialog lists), less any whose address is the commit's author's or committer's (`adoc_git::Repository::identity` and a new `author_identity`; they usually agree).
- `git.commit` accepts `"coauthors": [{"name", "email"}]` in its `Custom` arguments. The engine checks each (the same `commit_email` rule; a name with `<`, `>` or a control character has them removed) and appends the trailers through `with_trailers(message, coauthors) -> String`:
  - a blank line, then one `Co-authored-by: Name <email>` line each, or the lines added to the message's last paragraph when that is already a block of trailers;
  - none already in the message (compared by address without regard to case), none for the committer;
  - with no co-authors the message is unchanged.
- The engine never adds trailers on its own: only those the front end passes. A commit without `coauthors` is as today.
- After a commit succeeds (`Committed`, or a signed commit's `GitCommitted`), the engine calls `forget` with every file the commit took and its fixing stamp (section 5.2), read from the index before committing (a new `adoc_git::Repository::staged_paths`, the paths whose index entry differs from `HEAD`'s tree) and carried through the signed commit's worker message. A failed commit forgets nothing.

### 5.5 Comments by guests

A guest's comment or reply is recorded on the host with the guest's address as its `email`, where today it has none **(call)**, as the comments design planned (`PRD-AdocFoundry_Comments.md`, "and the e-mail once #761 sends it"). The address goes into the page's `.afd` file, which is usually committed: the guest gave it to be credited in the repository's history, which is where it goes. This is the one place it reaches the workspace before a commit.

"Own" then matches by address on the host. The guest cannot compare addresses, as it is never told any, so the host tells each guest which threads and replies are its own: `SharedThread` and `SharedReply` have their `email` field (always empty today) replaced by `own: bool`, so no address can be put there by mistake, set per peer by the host as it sends them (and part of what makes a thread due to be sent again), which the guest's `Actor::allowed` uses in place of comparing names **(call)**. A guest can then edit their own comment in a later session under another name. `SharedThread` still carries names only.

### 5.6 In the app

- **Join Session…** gains an *E-mail for commit credit* field under the name, optional, filled from `Engine::comment_author`'s address (`[comments] email`, else the global Git `user.email`). The name field is filled from `comment_author`'s name where `collaboration::own_name` has none today **(call)**. An invalid address is shown under the field and Join waits until it is fixed or emptied.
- **The commit dialog** shows *Co-authors* under the file list when `coauthors_for_commit` names anyone: each a checkbox, ticked, labelled `Name <email>`. The commit passes those left ticked. Nothing shows when nobody else changed the files.
- The user guide's collaboration page explains how guests are credited and what the host sees.

## 6. Acceptance

| Requirement (#761) | Test |
|---|---|
| Host changes a, guest changes b, both change c: committing a offers no co-author; b and c each the guest | `a_commit_credits_only_those_who_changed_its_files` |
| A participant who only read a file is never credited | `a_participant_who_only_read_is_never_credited` (opens, moves the caret, sends an update that changes nothing) |
| Authorship survives a reconnection and a rebuild in a new epoch | `credit_survives_a_reconnection_and_a_new_epoch` |
| A guest's e-mail address reaches only the host | `a_guests_email_reaches_only_the_host` (every frame sent to a second guest, and the comments it is told, searched for the address) |
| Kept until committed, across the session's end and a restart | `credit_outlives_the_session_and_the_engine` (a state folder; a new engine reads it) |
| The files committed start again | `committed_files_start_again`; a failed commit keeps them |
| An edit after the commit's content was fixed is credited next time | `an_edit_after_staging_is_credited_next_time` (a signed commit's wait uses the same stamps, computed before its worker starts) |
| Through a linked root | `credit_follows_a_page_shared_through_a_link` |
| A guest's own comments across sessions | `a_guest_edits_their_own_comment_under_another_name` |
| Trailers | `with_trailers` unit tests: blank line, no duplicates, committer left out, unchanged without co-authors |
| On the wire | `commit_email` limits in `protocol.rs`'s tests; an old-version guest refused by `Hello` |
| In the app | `the_commit_dialog_offers_co_authors` (a co-author unticked is not in the commit message) |

## 7. Packages

1. **CO-AUTHORS 1/2: the address and the record** — sections 5.1 to 5.5, the engine's and transport's tests, `docs/qa/collab.md`'s security review rows for the address.
2. **CO-AUTHORS 2/2: in the app** — section 5.6, the app test, the user guide, README, CHANGELOG, the manual checks (a real commit on GitHub showing its co-author).

## 8. Implementation notes

**CO-AUTHORS 1/2.**
- The address rule is in the protocol (`adoc_collab::protocol::commit_email`) and again in the engine (`coauthors::is_commit_email`), since the engine builds without `adoc-collab` for the CLI, the language server and the MCP server. `the_address_rule_is_the_protocols` holds the two together.
- `record` reports a write as due only for someone new to a file, as §5.2 says. Every later time is queued too, one per file and person, and goes with the next write. `forget` queues each remaining author's latest time, so the file agrees with memory after any write.
- `prune_coauthors` reads each recorded file's text against `HEAD` from the repository, not from the status the engine last read, which may not be read yet. It touches only files in this workspace's repositories, so a record another window made in the same state file is never dropped.
- A commit's fixing times come from `Repository::staged_paths`, read as the commit starts. If it cannot be read, nothing is forgotten.
- Through a linked root the engine's Git status was not read again after a save (the watcher reports the file by its real path); staging reads it again. This is the Git status's behaviour, not co-authorship's, and is left for its own issue.
- `git.commit` also takes `offered`, the co-authors the front end showed, ticked or not; a commit then forgets only those (and its author and committer). A guest whose first edit reached a file after the dialog listed its co-authors, which the commit still takes as it saves the page again, keeps their record and is offered next time (`a_guest_who_first_edits_while_the_dialog_is_open_is_kept`).
- Staging a page open with unsaved changes stamps it with when the text on disk was saved, not when it is staged, since staging takes the file (`SaveStamps`, `staging_takes_what_was_saved_not_what_is_unsaved`).
- Open, for the maintainer: whose a comment is (§5.5) rests on an address a guest gives and nobody checks, and the host does not see it when admitting the guest (`docs/qa/collab.md`, Release decisions).
