# Suggested changes — Release QA

The release record for suggested changes on a page's text (#763, SUGGEST 1/4 to 4/4; `docs/prd/PRD-AdocFoundry_Suggestions.md`). It maps each requirement to the tests that hold it and lists the manual checks that tests cannot make. Suggestions are comments that also hold a replacement, so `docs/qa/comments.md` and, for sessions, `docs/qa/collab.md` apply too.

A case is marked passed only when a named test holds it, or a person has run it and recorded the result here. Nothing unverified is marked as passing.

**State:** every automated row below passes on macOS. The manual checks have not been run yet. In particular, the strikethrough was verified by reading gpui-base 0.9's paint path, never seen on screen.

## Coverage

| Requirement | Case | Held by |
|---|---|---|
| Making one | A suggestion on the selection; refused beyond its limits, or when it changes nothing | `a_suggestion_is_made_on_the_selection`, `suggesting_is_refused_beyond_its_limits`, `a_suggestion_that_changes_nothing_is_not_accepted` |
| | Text that changed while the box was open is refused | `a_suggestion_begun_on_other_text_is_refused` |
| Its life | Rejecting leaves the text alone; an outdated suggestion cannot be accepted; Resolve is refused | `rejecting_leaves_no_trace_in_the_text`, `an_outdated_suggestion_cannot_be_accepted`, `resolving_a_suggestion_is_refused` |
| | Found again only where it was, never on the same words elsewhere | `a_suggestion_whose_text_is_deleted_is_found_again_only_where_it_was`, `a_suggestion_never_moves_to_the_same_words_elsewhere`, `a_suggestion_is_found_again_after_an_outside_change` |
| Accepting | One minimal edit, once, cut at character boundaries, among repeated letters | `accepting_applies_the_suggested_edit`, `accepting_cuts_the_edit_at_character_boundaries`, `an_insertion_among_repeated_letters_is_applied_once` |
| | Overlapping suggestions never corrupt the text; a deletion leaves its suggestion on no text | `overlapping_suggestions_never_corrupt_the_text`, `accepting_a_deletion_leaves_its_suggestion_on_no_text` |
| | Undo and redo, including at a range's edge | `undoing_an_accept_restores_the_text`, `undoing_an_accept_at_a_range_edge_restores_its_range`, `undoing_an_accepted_deletion_never_moves_it` |
| | Credits the author as a co-author | `accepting_credits_the_suggestions_author` |
| | Nothing accepted from a read-only comments file | `nothing_is_accepted_from_a_read_only_comments_file` |
| In a session | A Viewer's suggestion changes nothing until accepted; the host applies an accept once for everyone | `a_viewers_suggestion_changes_nothing_until_accepted`, `accepting_applies_the_suggested_edit_once_for_everyone`, `two_accepts_apply_once` |
| | Who may accept; the host checks the text a guest began on | `a_viewer_cannot_accept`, `a_guest_cannot_accept_while_away`, `the_host_refuses_a_suggestion_made_on_other_text`, `the_host_compares_the_text_a_guest_began_on`, `the_host_refuses_an_edit_of_a_suggestion_made_on_other_text` |
| | A guest rejects, reopens and edits; refuses in the host's words | `a_guest_rejects_and_reopens_a_suggestion`, `a_guest_edits_its_own_suggestion_with_a_blank_note`, `a_guest_refuses_what_the_host_would_in_its_words` |
| | Accepting a guest's suggestion credits the guest | `accepting_a_guests_suggestion_credits_the_guest` |
| | The host's undo and redo of its own accept; a redo after a guest's accept | `the_hosts_own_accept_on_a_shared_page_is_undone_and_accepted_again`, `a_redo_after_a_guest_accepts_again_applies_the_suggestion_once`, `a_guest_follows_the_range_through_the_hosts_undo_and_redo_of_an_accept` |
| | A guest's accept is untracked and sent to everyone; redo cleared | `an_untracked_edit_is_sent_but_never_undone`, `clearing_the_redo_keeps_the_undo` (adoc-collab) |
| | The wire held to its limits | `suggestions_round_trip`, `suggestion_fields_are_held_to_their_limits` (adoc-collab, `protocol.rs`) |
| | Convergence under concurrent accepts, rejects and suggestions | `collab_simulation_converges` (`check_suggestions`), `a_redo_after_a_guest_accepted_again_changes_nothing` |
| In the source editor | Suggest Change, the card, Accept and undo | `a_suggestion_is_made_accepted_and_shown` |
| | A box whose text changed meanwhile is refused | `a_suggestion_box_refuses_text_changed_while_open` |
| | Framed, struck and in the gutter; the status bar names it | `a_suggestion_is_marked_in_the_source_editor` |
| In the visual editor | The suggested text shown inline, the strike stopping before it, typing beside it writing only to the page | `the_visual_editor_shows_a_suggestion_inline` |
| | The display text, offsets, caret and runs | `an_insertion_is_shown_but_never_in_the_display_text`, `the_caret_steps_over_an_insertion`, `suggestions_strike_and_underline_their_runs` |
| | Beside a formula and a composition; the caret before a leading insertion | `an_insertion_beside_a_formula_and_a_composition_keeps_both_mappings`, `a_leading_insertion_keeps_the_caret_before_it` |
| | A suggestion ending inside an atom takes it whole | `a_suggestion_ending_inside_an_atom_takes_it_whole` |
| | The input method never reads suggested text | `the_input_method_never_sees_suggested_text` |
| | The caret is drawn where typed text lands | `the_caret_is_drawn_where_typed_text_goes_beside_a_suggestion` |
| | A pending suggestion is not filled as a comment, an outdated one is | `an_outdated_suggestion_keeps_the_comment_fill` |

The engine's tests are in `crates/adoc-engine/tests/suggestions.rs` and `collab_suggestions.rs`, the simulation in `collab_simulation.rs`, the app's in `crates/adocfoundry/src/ui_tests.rs` and the visual editor's drawing in `crates/adocfoundry/src/visual/text.rs` and `blocks.rs`.

## Manual checks

Run on each platform with the app built from the branch. Record the system and version beside each result.

| Check | macOS | Windows | Linux |
|---|---|---|---|
| In the source editor, a pending suggestion's range has a frame, the removed text is struck through (never seen on screen; checked only in the editor's paint path) and the gutter is marked, in a light theme and a dark theme; the cues stay readable over selected text | [ ] | [ ] | [ ] |
| In the visual editor, the removed text is struck through and the suggested text is drawn after it, underlined, in a light theme and a dark theme; an outdated suggestion has a fill and nothing struck | [ ] | [ ] | [ ] |
| With a screen reader (VoiceOver on macOS), a suggestion's card reads "Suggestion by …: replace ‘…’ with ‘…’", and the status bar says the same with the caret inside one | [ ] | [ ] | [ ] |
| Commit a page and its comments file with a suggestion on one machine, `git pull` on another: the suggestion is pending and Accept changes the page once | [ ] | [ ] | [ ] |
| An older AdocFoundry opens a page whose comments file holds a suggestion (`format = 2`): the comments are read-only and the file is left as it is | [ ] | [ ] | [ ] |
| In a session, a Viewer suggests a change and an Editor accepts it: the page changes once on every side, and the Viewer's suggestion shows as accepted | [ ] | [ ] | [ ] |
| In the visual editor beside an inserted suggestion, compose text with an input method (for example Japanese) and use press-and-hold for an accent: both write only into the page, never into or over the suggested text | [ ] | [ ] | [ ] |
