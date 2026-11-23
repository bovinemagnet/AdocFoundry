# Comments — Release QA

The release record for comments on a page's text (#762, COMMENTS 1/4 to 4/4; `docs/prd/PRD-AdocFoundry_Comments.md`). It maps each requirement to the tests that hold it and lists the manual checks that tests cannot make. Comments in a collaboration session are also in `docs/qa/collab.md`, whose security review and manual checks include them.

A case is marked passed only when a named test holds it, or a person has run it and recorded the result here. Nothing unverified is marked as passing.

**State:** every automated row below passes on macOS. The manual checks have not been run yet.

## Coverage

| Requirement | Case | Held by |
|---|---|---|
| The page never changes | Adding, replying, resolving, editing and deleting change neither the page's text nor whether it is unsaved, nor its undo history | `comments_never_change_the_page` |
| | The comments file is hidden from Antora and the engine's catalog | `hidden_files_are_not_catalogued` (adoc-antora), `a_hidden_file_made_in_a_module_is_not_catalogued` |
| A comment follows its text | Typed before, at its edges, inside; part of it deleted | `a_comment_follows_its_text_through_edits`, `a_range_moves_with_edits` |
| | Concurrent edits in a session, both ways | `a_comment_follows_its_text_through_concurrent_edits`; the simulation's `check_comments`; `a_comment_on_redone_text_is_placed_alike` (#786) |
| | Its text deleted, then brought back by undo | `a_comment_whose_text_is_deleted_is_detached_and_undo_brings_it_back`, `a_detached_comment_is_due_once_typing_pauses` |
| Found again | The page changed outside the app | `a_comment_is_found_again_after_an_outside_change`, `a_comment_whose_text_went_elsewhere_is_detached`, `the_nearer_of_two_quotes_wins` |
| The file | Read beside the page, and round trip with unknown data kept | `a_pages_comments_are_read_from_beside_it`, `a_sidecar_round_trips`, `unknown_data_is_kept` |
| | Written only when comments change, after a settle | `a_comment_is_written_once_its_settle_passes`, `a_page_save_leaves_the_sidecar_alone`, `a_comment_change_is_due_after_its_settle` |
| | Changed on disk meanwhile: both kept | `a_sidecar_changed_on_disk_keeps_both` |
| | Unusable files never rewritten: conflict markers, a wrong type, duplicate ids, past a limit | `an_unusable_sidecar_is_left_alone`, `unusable_files_are_named`, `limits_are_held_reading_and_writing`; fuzz target `page_meta` |
| | A newer format read-only | `a_newer_format_is_read_but_never_written` |
| | A failed write kept and tried again | `a_failed_sidecar_write_keeps_the_change` |
| | Written when the page closes or the app quits | `a_comment_on_a_page_closed_before_its_settle_is_written` |
| | Removed with the last comment | `the_last_comment_deleted_removes_the_sidecar`, `the_last_comment_gone_removes_the_file` |
| With the page | Renamed, moved, undone; a case-only rename; refused onto another comments file | `renaming_a_page_moves_its_sidecar_and_undo_moves_it_back`, `a_case_only_rename_moves_the_sidecar`, `a_rename_onto_a_sidecar_is_refused` |
| | Deleted and restored | `deleting_a_page_keeps_its_sidecar_for_restoring` |
| Who writes them | Settings, else Git, user file only | `the_comment_author_comes_from_settings_first`, `comments_settings_are_user_only`, `identity_is_read_from_the_files_given` (adoc-git) |
| In a session | A Viewer can comment but not edit | `a_viewer_can_comment_but_not_edit` |
| | Suggestions in a session (#763) | `crates/adoc-engine/tests/collab_suggestions.rs`; the simulation's `check_suggestions` |
| | The host decides; refusals keep the connection; stale requests refused | `what_a_participant_may_do_is_the_hosts_to_decide`, `a_request_from_an_earlier_epoch_is_refused`, `a_comment_on_a_file_not_opened_closes_the_connection` |
| | Guests told names only, and every comment however many | `guests_are_never_told_an_email_address`, `more_comments_than_one_pump_tells_all_reach_a_guest`, `comments_are_told_again_after_a_reconnection` |
| | The wire held to its limits | `comments_are_held_to_their_limits`; fuzz target `collab_frame` |
| In the app | Add Comment on the selection, the panel, the source editor's marks, the status bar, Next Comment, Resolve | `a_comment_is_added_from_the_selection_and_shown` |
| | The Add Comment box follows its text: typing before the words moves the comment with them; the words deleted, it is refused (#785) | `a_comment_box_follows_its_text_as_the_page_changes`, `a_comment_box_whose_text_is_deleted_adds_nothing` |
| | Suggest Change, the card, Accept and undo | `a_suggestion_is_made_accepted_and_shown` |
| | A suggestion whose words changed while its box was open is refused; text typed before them moves it with them | `a_suggestion_box_refuses_text_changed_while_open`, `a_suggestion_box_follows_its_text_as_the_page_changes` |
| | A suggestion framed, struck and in the gutter; the status bar names it | `a_suggestion_is_marked_in_the_source_editor` |
| | The visual editor's marks | `the_visual_editor_marks_a_pages_comments`, `comments_fill_the_runs_they_cover` |

## Manual checks

Run on each platform with the app built from the branch. Record the system and version beside each result.

| Check | macOS | Windows | Linux |
|---|---|---|---|
| Add Comment with the keyboard (Cmd-Alt-M on macOS, Ctrl-Alt-M elsewhere) and from the Edit menu; the box takes the focus, and the focus returns to the page after Comment or Cancel | [ ] | [ ] | [ ] |
| Commented text is marked in the source editor and the visual editor, in light and dark themes, and the mark stays readable over selected text | [ ] | [ ] | [ ] |
| Type at the edges of and inside a commented passage, and delete across it; the mark follows as expected, and undo brings a detached comment back | [ ] | [ ] | [ ] |
| Commit `.page.adoc.afd`, change the page on another branch, merge: the comments are found again; a conflict in the comments file shows its notice and the file is left as it is | [ ] | [ ] | [ ] |
| Rename and delete a commented page in the app, then undo; the comments come back with it | [ ] | [ ] | [ ] |
| Antora builds the site with comments files beside pages: none is published | [ ] | [ ] | [ ] |
| Screen reader: the Comments panel's comments, their authors and buttons are read; the status bar names the comment at the caret | [ ] | [ ] | [ ] |
| In a session, comment from each side, a Viewer included, while the other types around it (also in `docs/qa/collab.md`) | [ ] | [ ] | [ ] |
