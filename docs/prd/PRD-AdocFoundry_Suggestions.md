# Suggested Changes

**Issue:** #763 (suggested changes), a follow-on of the collaborative editing epic #686, after comments (#762) and co-authors (#761).
**Author:** Paul Snow. **Version:** 0.0.0. **Status:** design, for approval.

## 1. Intent

A reader who sees a better wording should be able to propose it without changing the page: the proposal is shown on the text it would replace, and someone allowed to edit the page accepts it, which makes the change, or rejects it, which leaves the text as it was. In a session this gives Viewers a use beyond reading and commenting. Alone, it lets a reviewer go through a colleague's page, commit the suggestions with it, and leave the author to accept or reject them after a `git pull`, as a Word document's tracked changes are reviewed.

## 2. Decisions taken by the maintainer

- Suggestions are kept in the page's metadata file, `.<name>.afd`, beside its comments (the comments spec, §1), never in the page.
- **Approach 1 (section 4):** a suggestion is a kind of comment. It carries the text it would replace and the text it proposes, and is made from a selection through a box, not by typing in a suggesting mode.
- Like comments, suggestions belong to the page: made, accepted and rejected alone or in a session.
- Accepting credits the suggestion's author as a co-author of the change (#761).
- The issue's spike was settled by this design rather than by throwaway code: overlapping and outdated suggestions and undo in section 6, the look in section 9, checked against gpui-component 0.9's editor (section 9.1).

Calls made in writing this design, for the maintainer to check, are marked **(call)**.

## 3. Goals, non-goals, anti-goals

**Goals**

- Suggest replacing a selected range with other text (deleting it included), with an optional note.
- Accept a suggestion: the page's text changes by exactly that edit, once, for everyone, credited to its author.
- Reject a suggestion: the text is untouched.
- A suggestion follows its text through edits as a comment does, and is never accepted onto text other than the text it was made on.
- Suggestions survive closing the page, restarting the app, a session's end and a `git pull`, as comments do.
- In a session, Viewers can suggest; the host and Editors can accept.
- Reachable by keyboard, with accessible labels; colour never the only cue.

**Non-goals (this design)**

- A suggesting mode in which typing becomes suggestions (approach 2, section 4). Its stored form would be this design's, so it can follow.
- Suggestions on files that are not AsciiDoc, or through the CLI, the language server or the MCP server.
- Accepting or rejecting several suggestions at once.
- Merging overlapping suggestions into one.
- Showing suggestions in exports, the preview or the published site.

**Anti-goals: a failure even if it works**

- Accepting a suggestion onto text that has changed since it was made, or applying it twice.
- Any change to a `.adoc` file's bytes from suggesting, rejecting or reading suggestions; only accepting changes the page.
- An older AdocFoundry rewriting a file that holds suggestions and losing them, or letting them be resolved as plain comments.
- A suggestion from a repository someone else controls able to crash, hang or exhaust the app, or to change the page without the user accepting it.

## 4. Approaches

1. **A suggestion is a kind of comment (chosen).** It reuses everything comments built: a range that follows edits, finding it again by its quote, the file beside the page and its writer, the host's authority in a session, the Comments panel and its permissions. What is new is the proposed text, the check that the text is still what was suggested on, accepting as an edit, and the marks.
2. **A suggesting mode, as Google Docs has.** Typing becomes suggestions. Rejected for now: each participant's editor would show text that is not the document's, against the rule that the widget holds the engine's text (`editor/sync.rs`); it would tangle with input methods, undo and the shared replica; and the source editor cannot draw inserted text inline (section 9.1). It could come later over this design's stored form.
3. **Pending edits in the session's replica,** as the issue first phrased it (an edit anchored by relative positions, not applied until accepted). Rejected: it exists only in a session and is gone when the session ends, against the decision to keep suggestions in the metadata file; and `adoc-collab` admits only a text replica's grammar (comments spec §4, approach 2).

## 5. The stored suggestion

A suggestion is a `[[comment]]` with a `suggestion` table. Illustrative:

```toml
format = 2

[[comment]]
id = "3b0f6a7c19e2d448"
author = "Ada"
email = "ada@example.org"
created = 2026-11-23T09:12:00Z
start = 1204
end = 1251
revision = "sha256:9f2c…"
quote = "the installer refuses an unsigned package"
before = "On macOS "
after = ", so sign it first."
text = ""                          # the note; may be empty for a suggestion
resolved = false

[comment.suggestion]
original = "the installer refuses an unsigned package"
replacement = "Gatekeeper refuses an unsigned package"
# once decided:
outcome = "accepted"               # or "rejected"
```

- **`original`** is the text the suggestion was made on, fixed when it is made. `quote` follows the range's text as for any comment (comments spec §5.4); `original` never changes. **(call)**
- **`replacement`** is the proposed text; empty means "delete this".
- **`outcome`** is set when the suggestion is accepted or rejected, together with `resolved = true`, `resolved_by` and `resolved_at`. A pending suggestion has no `outcome`.
- **The note** is the comment's `text`, and may be empty for a suggestion only. Replies work as on any comment.
- **Format 2 (call).** A file holding any suggestion is written with `format = 2`; a file without one stays at 1. An older version reads a format 2 file without rewriting it and shows its comments read-only with a notice (comments spec §5.2), so it can neither lose a suggestion nor resolve one as a plain comment. Format 1 files stay readable and writable by older versions.
- **Limits (call):**
  - `original` at most 4 KiB (`MAX_COMMENT_QUOTE`), so a suggestion's quote always holds all of it. A larger selection is refused: "Select at most 4 KiB of text to suggest a change."
  - `replacement` at most 8 KiB (`MAX_COMMENT_TEXT`).
  - Both may hold line breaks, tabs and carriage returns, and no other control character.
  - The range may not be empty: an insertion is suggested by selecting a neighbouring word and retyping it with the addition. An empty range is a detached comment's, and a point between two characters cannot be told apart from text typed at it. **(call)**
  - A replacement equal to its original is refused: it changes nothing.
  - Suggestions count towards the file's 500 comments.
- Reading holds every value to these limits; past one, the file is unusable, as for any broken comment. A pending suggestion whose stored `quote` differs from its `original` is simply outdated: an edit inside its range is written with the current quote, as for any comment. The `page_meta` fuzz target covers the new table.

## 6. Its life

**States.** A suggestion is *pending*, *outdated*, *accepted* or *rejected*.

- **Pending:** its range's current text equals `original`. It can be accepted.
- **Outdated (call):** pending in the file, but its range's text differs from `original`, or it is detached, or it was found again in a place it cannot be sure of (below). It cannot be accepted ("The text has changed since this was suggested"); it can be rejected, deleted or replied to, and its author can edit it to suggest again on the current text (which sets `original` to the range's text and takes the place as certain). It becomes pending again whenever its range's text equals `original` again. Text restored at the range's edges falls outside it, as for comments (comments spec §5.4), so undoing a deletion at an edge can leave it outdated; the author then suggests again.
- **Accepted or rejected:** resolved, with the outcome. Shown with resolved comments, folded.

**Through edits.** The range is mapped through every edit as a comment's is (comments spec §5.4): text typed at an edge falls outside it, typed inside grows it, so typing inside makes it outdated.

**Found again: the same place, not just the same words (call).** The comments' search takes the best occurrence of a quote at any score, so a phrase that appears twice could move a suggestion onto the other one, and accepting would change text nobody suggested on. So a suggestion found again by its quote (on reading a file whose `revision` differs from the page, or after being detached) is pending only when its stored `before` and `after` both match the text around that occurrence whole, and no other occurrence matches as well; otherwise it is placed as a comment would be but is outdated. A suggestion at its stored offsets under a matching `revision` is certain. The cost: a suggestion whose neighbouring 32 bytes were edited elsewhere needs suggesting again.

**Overlapping suggestions (call).** Allowed, and never merged. Accepting one is an edit like any other: every other suggestion whose range that edit touched is then outdated, since its text no longer equals its `original`; those it did not touch are unaffected. Nothing is applied that was not suggested on the text as it is, so the text is never corrupted.

**Accepting.** One edit of the page, through the same `apply_edits` path as typing:

- It replaces only what differs: the longest common prefix and suffix of `original` and `replacement` (cut at character boundaries) are left alone, so carets, comments and suggestions on the unchanged ends keep their places. **(call)**
- It is refused if the suggestion is not pending, if the page is not editable here (a Viewer's page, a guest's page while away), or if the edit is refused (a shared document's rules).
- **Credit (#761):** when the author has an address the co-authors rule accepts (`is_commit_email`) and is not the one accepting, they are recorded against the file's real path, as a guest's edit is (`CoAuthors::record`), alone or hosting. The commit dialog then offers them, and they can be unticked there.
- After the edit, the suggestion's range is set to cover exactly the replacement as it now stands, whatever the comments' edge rule would have made of it (a pure insertion at its end would otherwise fall outside it). So undoing the accept leaves the range holding `original` again.
- The suggestion is marked accepted at once, and the file written as for any comment change.
- **When an accept does not last (call).** The outcome and the credit record decisions, not the page's text, and stay when the accept is undone, or the page closed without saving or reloaded from disk. The commit dialog already drops a guest's credit for a file back to its committed text (`prune_coauthors`), and its checkboxes let the committer untick anyone else. Withdrawing one accept's credit alone would need `coauthors.json` to record where each credit came from; this design does not. The user guide says so.

**Rejecting** marks it rejected; the page is untouched.

**Undo (call).**

- Accepting is an edit in the page's undo history, alone and for the host's own accepts. Undoing it puts the text back; the suggestion stays accepted, as comment changes stay out of undo (comments spec §5.5). Reopening it then makes it pending again, its text being its original once more.
- Rejecting is not undone by Undo; Reopen undoes it.
- An accept asked for by a guest is applied by the host as a participant's edit is: in nobody's undo history (section 8).

**Reopen** (the comments' `comments.reopen`) clears the outcome. A suggestion can be accepted only once while pending, and the outcome is set by the same step that applies the edit, so two accepts cannot both apply.

## 7. Who may do what

| | Alone | Host | Editor | Viewer |
| --- | --- | --- | --- | --- |
| Suggest | yes | yes | yes | yes |
| Accept | yes | yes | yes | no |
| Reject | yes | yes | yes | own only (withdraw) |
| Reopen | yes | yes | yes | own only |
| Edit (replacement and note) | yes | own, while not decided | own, while not decided | own, while not decided |
| Delete | yes | any | own | own |
| Reply | yes | yes | yes | yes |

- Alone, the user may do everything, as with comments: the files are theirs.
- "Own" is the comments' rule (`Actor::owns`).
- The plain Resolve of comments does not apply to a suggestion: it is resolved by accepting or rejecting. **(call)**
- `Allowed` gains `accept` and `reject`; the host computes them per guest, as it does today, and checks every request again.

## 8. In a session

- **The host applies every accept (call).** A guest's accept is a request; the host checks the role and the suggestion's state against its own text and, if allowed, applies the edit on its copy. It reaches everyone as the host's edit, and the guest's own accept is never applied locally first. So an accept is applied once, by one participant, against the text the host holds.
- **A guest's accept is in nobody's undo history (call).** The host applies it outside its own undo, as it applies a participant's edits; the guest's undo touches only its own edits. To undo it, edit the text or suggest the reverse.
- **Suggesting from a guest.** `CommentChange::Add` gains `suggestion: Option<SuggestionChange { original, replacement }>`. The guest sends the text it saw as `original`; the host refuses the request if its own text at the resolved selection differs ("The text changed while you were suggesting; try again"), so a replacement is never paired with text its author did not see.
- **New changes:** `CommentChange::Accept { comment }` and `Reject { comment }`. `Edit` of a suggestion carries the new replacement.
- **Told to guests:** `SharedThread` gains `suggestion: Option<SharedSuggestion { original, replacement, outcome }>`; whether a guest may accept or reject it is its role's, which it knows, and the host checks again. Addresses stay on the host, as for comments.
- **Protocol version 3** (`PROTOCOL_VERSION`): the messages change shape, and every field is checked by the protocol's own limits (section 5).
- **Simulation:** `tests/collab_simulation.rs` gains `Suggest`, `Accept` and `Reject` operations, two participants accepting the same suggestion at once among them. After each settle every participant's text is the same, and every accepted suggestion's edit appears in it exactly once.

## 9. In the app

- **Suggest Change** (Edit menu and palette; `cmd-alt-s` is proposed, mapped off macOS as Add Comment's is, the builder to check it is free) on a non-empty selection, in the source or the visual editor, opens a box in the Comments panel: the selected text in an editable field, and an optional note. Allowed wherever Add Comment is.
- **The card** shows the author, the time, the note, and the change as the original with the removed part struck through and the replacement with the added part marked, both in words for screen readers ("Suggestion by Ada: replace ‘the installer’ with ‘Gatekeeper’"). Its buttons are Accept and Reject, then Reply, Edit, Delete as allowed; an outdated suggestion says so and offers no Accept.
- **The source editor** frames each pending or outdated suggestion's range in the suggestion colour, strikes through the part accepting would remove, and marks the line in the gutter. Inserted text is not drawn in the text (9.1); the card and the status bar show it. With the caret inside a suggestion, the status bar names it.
- **The visual editor** strikes through the part to be removed and draws the inserted text inline after it, in the suggestion colour and underlined, as source text: it is display only, never selectable or editable, the caret stepping over it as it steps over an inline formula's gap (`Shown`). **(call)**
- Accepted and rejected suggestions are not marked in either editor.
- **Colour:** one theme colour for suggestions, distinct from comments' fill; the strike, the underline and the frame are the cues that do not depend on it.

### 9.1 What the source editor can draw (checked)

Read in gpui-component 0.9 and gpui-base 0.9's editor:

- `TextDecorationCollection` (`create_decorations_collection`) applies a GPUI `HighlightStyle` to a byte range, strikethrough and colour included, without changing the layout. Deleted text can be shown struck through where it is.
- `RangeDecorationCollection` frames or fills a range; `GutterDecorationCollection` marks lines. Both are used today for remote carets and comments.
- The only text the widget draws that is not in its buffer is one LSP inline completion at the caret (`layout_inline_completion`). So inserted text cannot be drawn inline at several places without changing gpui-component, which this design does not do.

The builder's first step in the app package confirms the strikethrough draws as expected, before building the rest of the marks on it.

## 10. Engine surface (for the builder to shape)

- **Commands:** `comments.suggest` (`CommentRequest::Suggest { id, range, replacement, text }`), `comments.accept`, `comments.reject`; `comments.edit` takes a replacement for a suggestion; `comments.reopen`, `comments.delete` and `comments.reply` as they are. All in `ALLOWED_WHILE_HOSTING` and allowed on a guest's page.
- **Queries:** `Comment` gains `suggestion: Option<Suggestion { original, replacement, state }>`, `state` being pending, outdated, accepted or rejected, and the removed and inserted parts (byte ranges in the current text, and the inserted text) for the marks. `Allowed` gains `accept` and `reject`.
- **Events:** `CommentsChanged` as now; accepting also gives the edit's `DocumentChanged`.
- No new pump step is expected; if one is added, its deadline goes in `next_due` with a test in `tests/wake.rs`.

## 11. Packages

One stack, each package green and documented:

1. **SUGGEST 1/4: suggestions alone.** The stored form and format 2, limits and the fuzz target, the states, accepting as an edit with undo, credit, reopen, the commands and queries, with engine tests.
2. **SUGGEST 2/4: suggestions in a session.** Protocol version 3, the new changes and thread field, the host applying accepts outside its undo, the simulation's operations, with protocol, transport and engine tests.
3. **SUGGEST 3/4: the app.** Suggest Change, the box, the card and its buttons, the source editor's marks, the status bar, menus, and an app test.
4. **SUGGEST 4/4: the visual editor** (struck and inserted text inline), and the release record `docs/qa/suggestions.md` with the manual checks.

The user guide's Comments page gains a "Suggested changes" section, and the developer guide's engine page one beside comments. README, CHANGELOG and the collaboration PRD's §14 note follow.

## 12. Acceptance (from #763, widened)

- A Viewer's suggestion changes nothing until an Editor or the host accepts it (`a_viewers_suggestion_changes_nothing_until_accepted`).
- Accepting applies exactly the suggested edit, once, for everyone (`accepting_applies_the_suggested_edit_once_for_everyone`), even when two participants accept at once (`two_accepts_apply_once`).
- Rejecting leaves no trace in the text (`rejecting_leaves_no_trace_in_the_text`).
- Overlapping suggestions never corrupt the text: accepting one makes those it touched outdated (`overlapping_suggestions_never_corrupt_the_text`).
- An outdated suggestion cannot be accepted, and becomes pending again when its range's text is its original again (`an_outdated_suggestion_cannot_be_accepted`).
- A suggestion found again where its context does not match whole is outdated, never moved onto the same words elsewhere (`a_suggestion_never_moves_to_the_same_words_elsewhere`).
- Undoing an accept puts the text back and leaves the suggestion accepted until reopened (`undoing_an_accept_restores_the_text`).
- A suggestion survives closing and reopening the page, and a change made outside the app (`a_suggestion_is_found_again_after_an_outside_change`).
- Accepting credits the author with an address (`accepting_credits_the_suggestions_author`).
- A file holding suggestions is written as format 2, and a format 1 file without them stays at 1 (`suggestions_raise_the_format_only_when_present`).

## 13. Left to the builder

- Module layout: the expectation is the suggestion model in `comments.rs` beside comments, and its stored form in `page_meta.rs`.
- How the host applies a guest's accept outside its undo history.
- The diff shown in the card, within section 9 (the common prefix and suffix are the minimum; a word diff is welcome).
- The suggestion colour, within the theme and the accessibility rule.
- The binding, if `cmd-alt-s` is taken.
