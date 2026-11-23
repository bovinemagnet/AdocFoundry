# Collaborative Editing — Release QA

The release record for collaborative editing (COLLAB 8/8, #695; `docs/prd/PRD-AdocFoundry_COLAB.md` §§9, 10 and 12; epic #686). It maps each area of the coverage matrix to the tests that hold it, records the security review, lists what runs across two processes, and lists the manual checks that tests cannot make. The performance report is `docs/qa/collab-performance.md`.

A case is marked passed only when a named test holds it, or a person has run it and recorded the result here. Nothing unverified is marked as passing.

**State:** every automated row below passes on macOS. The manual checks have not been run yet. No release decision is open.

## Coverage matrix

| Area | Case | Held by |
|---|---|---|
| Convergence | Concurrent insertions at the same offset, overlapping deletions | `collab_replicas_converge`, `a_deletion_straddling_concurrent_insertions_converges` (adoc-collab); `collab_simulation_converges` |
| | Duplicate, out-of-order and late updates | `duplicates_and_reordering_are_refused_unread`, `repeated_disconnects_replay_once` (adoc-collab); `updates_out_of_order_close_the_connection`, `a_reconnected_guest_delivers_each_edit_once`; the simulation's duplicates, held deliveries and dropped links; `a_duplicate_after_a_snapshot_is_discarded`, `an_update_sent_again_after_a_reconnection_is_discarded` |
| | Long sessions | `a_guest_joins_after_a_long_session_with_deletions_and_undo`, `a_long_session_grows_with_its_text` (adoc-collab); `collab_simulation_soak` (ignored; 400 seeds of 1,500 steps run for this record) |
| | Several pages at once | `collab_pages_converge_independently`; the simulation's two pages; `collab_two_process_release_suite` |
| Unicode and IME | ASCII, multi-byte scripts, combining marks, emoji, CRLF | `collab_edits_are_byte_exact`, `emoji_and_composition_converge`, `collab_offsets_inside_a_character_are_refused` (adoc-collab); the simulation's alphabets; the two-process suite's emoji |
| | Whole-composition replacement | `a_composition_survives_a_remote_edit_before_it` (app); input methods on each platform: **manual** |
| Undo and redo | Local-only undo after remote edits, redo, group boundaries | `collab_local_undo_keeps_remote_edits`, `groups_end_where_the_engine_says`, `remote_edits_never_enter_the_local_history` (adoc-collab); the simulation checks after every undo that no other participant's text went |
| | History from before the session | `local_undo_keeps_remote_edits_and_forgets_the_time_before_sharing` |
| | Rapid typing | the simulation's runs of edits between deliveries; typing on two machines: **manual** |
| Source and visual parity | Visual edits, partials, preview refresh, no echo | `collab_visual_edits_share_the_bridge`, `collab_remote_edit_is_not_echoed`, `a_shared_partial_edited_elsewhere_shows_in_the_preview` |
| | Caret remapping | `collab_remote_carets_follow_edits` (engine and app), `a_selection_follows_its_text_on_every_replica` (adoc-collab) |
| Authentication | Expired or used invitation, wrong fingerprint | `collab_wrong_fingerprint_sends_nothing` (adoc-share) |
| | Untrusted host | the pinned fingerprint and safety code: `collab_wrong_fingerprint_sends_nothing`; the two-process suite compares the safety codes |
| | Role promotion and revocation | `collab_unauthorised_requests_are_refused`, `a_role_taken_away_while_edits_are_in_flight`, `collab_viewer_cannot_edit` (app); the simulation's role changes; the two-process suite's revocation |
| | Replay | `duplicates_and_reordering_are_refused_unread`; invitations single use: `collab_wrong_fingerprint_sends_nothing` |
| Workspace security | Traversal, links leaving the root, hidden, `.git` and credential paths, unknown file ids | `collab_unauthorised_requests_are_refused` (engine and adoc-share), `collab_manifest_offers_only_what_may_be_shared`, `a_file_replaced_by_a_link_after_sharing_is_not_opened`, `display_paths_can_only_be_shown` |
| | No host path sent | `assert_no_host_paths` on every settle of every in-memory session test; `collab_scope_never_leaks` |
| | Unshared pages, metadata and media | `collab_scope_never_leaks` (navigation, attributes, title, start page, unshared pages, the guest's own clone, a decoy on disk) |
| | Cross-session queries | `collab_scope_never_leaks` (Quick Open, symbols and graph of the guest's own workspace) |
| | Structural commands | `collab_structural_commands_refused`, `every_command_is_decided_for_a_session` |
| Resource bounds | Oversized frames, malformed updates | `frames_over_the_limit_are_refused_before_reading`, `collab_malformed_update_changes_nothing`, `counts_claiming_more_than_follows_are_refused` (adoc-collab); `collab_frame_limits_close_the_connection` (adoc-share); fuzz targets `collab_update`, `collab_frame` |
| | Huge media | not applicable: media are not shared |
| | Slow client, queue overflow, timeouts | `collab_slow_peer_never_blocks_host`, `a_knock_not_answered_in_time_is_turned_away` (adoc-share); `a_session_drains_a_bounded_number_of_events_a_pump` |
| | Too many components | `components_beyond_a_message_are_left_out` |
| Host file system | Save with edits, a save in flight | `collab_save_writes_converged_text`, `an_edit_during_a_save_is_not_claimed_saved`; the simulation checks each saved revision against the disk |
| | Watcher collision, change on disk | `collab_external_change_is_a_conflict`, `a_change_the_watcher_reports_is_a_conflict_never_an_edit`, `reloading_from_disk_starts_everyone_again_in_a_new_epoch` |
| | Rename and delete | `collab_structural_commands_refused`; `an_editor_is_not_dropped_when_the_host_closed_a_file_that_is_gone` |
| | Disk failure | `a_failed_save_keeps_the_file_and_the_shared_text` |
| | Recovery | `a_guests_text_is_never_kept_for_recovery_but_the_hosts_is` |
| Reliability | Disconnect and reconnect | `collab_loopback_session_syncs` (adoc-share), `a_reconnected_guest_delivers_each_edit_once`, `a_guest_away_through_a_new_epoch_catches_up`, `an_edit_waiting_when_the_link_drops_is_kept`, `a_snapshot_racing_a_resume_is_followed_by_a_resync`; the two-process suite |
| | Guest crash | **manual** (no test kills a guest mid-session; a dropped link, which the host sees the same way, is held by the reconnection tests above) |
| | Host restart, epoch change | `collab_stale_epoch_is_refused`, `an_edit_crossing_a_reload_is_spent`, `stopping_closes_every_connection` |
| | Slow or blocked participant | `collab_slow_peer_never_blocks_host`; held deliveries in the simulation |
| Regression | Ordinary editing, undo and saves | `collab_off_changes_nothing`; the engine's whole suite |
| | No-network builds | `cargo xtask layers` (`COLLAB_FREE`, `TLS_ALLOWED`), the release workflow building the app apart (`test_the_app_is_built_apart_from_the_other_binaries`) |
| | Parser, index, search, exports | the workspace's suites, unchanged |

## Across two processes

`collab_two_process_release_suite` (`crates/adoc-share/tests/two_process.rs`) runs a guest engine in a process of its own over loopback TLS, the fingerprint pinned:

- [x] Handshake, with the same safety code on both sides, and nothing sent before the host admits.
- [x] The file listing, two pages, by path from the shared folder only.
- [x] Both pages edited from both sides, Unicode included, converging.
- [x] Every update of the guest's acknowledged.
- [x] Revocation: a Viewer's edit refused there, the host's text unchanged.
- [x] Reconnection in the same epoch, the host editing meanwhile, the guest catching up and editing again.
- [x] The host stopping: the guest's connection closes, the guest ends cleanly, and nothing listens on the port afterwards.

The rest of the matrix is held in process, by the tests named above. Twenty runs in a row passed, with no guest process left behind.

## Security review

Each threat, its defence, and the test that holds it. Defects found in `yrs` during this work were reported to its maintainers privately; they are described here only by kind.

| Threat | Defence | Held by |
|---|---|---|
| Someone on the network connects | Nothing listens until the host confirms Share; never on every interface; only loopback or the chosen local address | `collab_share_needs_confirmation` (app), `the_host_listens_only_where_told` |
| A stolen or replayed invitation | Single use, ten minutes, kept as a hash; the host admits each knock by hand | `collab_wrong_fingerprint_sends_nothing`, `a_knock_not_answered_in_time_is_turned_away` |
| A host impersonated | TLS 1.3 only, the certificate pinned by its fingerprint, the handshake's signature still checked, a safety code to compare | `collab_wrong_fingerprint_sends_nothing` |
| Secrets in logs | `Secret`'s `Debug` hides it; invitations never on a command line in tests | `secrets_are_never_printed` |
| Reading outside the shared folder | Every path confined lexically and once links are followed, again at first open; hidden, ignored and linked folders refused | `collab_unauthorised_requests_are_refused`, `a_file_replaced_by_a_link_after_sharing_is_not_opened` |
| Learning about the host's machine | Fixed reasons, root-relative display paths, component names and versions only | `assert_no_host_paths`, `collab_scope_never_leaks` |
| A Viewer or revoked guest writing | Every update checked against the sender's current role; a guest cannot save | `collab_unauthorised_requests_are_refused`, `a_role_taken_away_while_edits_are_in_flight` |
| Forged authorship | An update may create items only under the guest's own client id, and only one run from the host's clock for it | `collab_forged_author_is_refused`, `collab_authors_are_read_before_applying` |
| Updates replayed across snapshots | Each update names the snapshot it was made on; earlier ones are discarded | `a_duplicate_after_a_snapshot_is_discarded` |
| Where each participant is | Only the shared file's id, or none; a host on a page it does not share is elsewhere, and nothing sent names that page; a guest naming a file not shared closes its connection | `a_host_on_a_page_it_does_not_share_is_elsewhere`, `a_focus_on_a_file_not_shared_closes_the_connection` |
| A guest changing others' comments (#762) | The host decides every change by the guest's role and the comment's author, refusing with a reason and keeping the connection; a request from an earlier epoch is refused; a guest never reads or writes a comments file | `what_a_participant_may_do_is_the_hosts_to_decide`, `a_request_from_an_earlier_epoch_is_refused`, `a_comment_on_a_file_not_opened_closes_the_connection` |
| Learning who commented | Guests are told commenters' names, never the e-mail addresses the page's comments file holds | `guests_are_never_told_an_email_address` |
| A guest's e-mail address leaking (#761) | Sent only to the host in `Authenticate`, checked by `commit_email`; never in a `Member`, a comment thread sent to a guest (which says only whether it is the guest's own), an error or a log line | `a_guests_email_reaches_only_the_host`, `addresses_are_held_to_the_rule`, `an_invalid_address_is_refused_before_connecting` |
| A hostile co-authors file (#761) | `coauthors.json` read as untrusted: 1 MiB, 10,000 files, 100 co-authors a file, every name and address checked; an unusable or newer file is left alone and never written over | `an_unusable_state_file_is_left_alone` |
| Crediting someone who only read | A guest is recorded only when an update it sent changed the text | `a_participant_who_only_read_is_never_credited` |
| A guest claiming another's address (#761) | Shown, not checked: the host decides whose a comment is by the address a guest gives, checked only for its form, so the Session panel shows a knocking guest's address under their name, as the guest's own and not checked, for the host to turn away a claim it does not expect. The address is shown only there, on the host; no network check is made. A guest admitted with someone else's address can edit or delete that person's comments, and is credited as them in the host's commits unless the host unticks them. Names were already as self-chosen; Editors can rewrite the page anyway | `the_host_sees_a_knocking_guests_address`, `the_session_panel_shows_a_knocking_guests_address` (app); decided (see Release decisions) |
| A guest's suggestion changing text (#763) | A suggestion changes no text, on any participant, until the host accepts it | `a_viewers_suggestion_changes_nothing_until_accepted` |
| A suggestion made on text the host does not hold (#763) | The host checks the `original` a guest sends, on suggesting and on editing a suggestion, and changes nothing on a mismatch | `the_host_refuses_a_suggestion_made_on_other_text`, `the_host_refuses_an_edit_of_a_suggestion_made_on_other_text` |
| A Viewer accepting (#763) | Refused by the host, whatever the guest sends | `a_viewer_cannot_accept` |
| An accept applied twice (#763) | Two accepts of one suggestion apply once, against the host's text | `two_accepts_apply_once` |
| Comments flooding a guest | Every comment and reply held to its limits on the wire; at most 500 a file and 32 told a pump | `comments_are_held_to_their_limits` |
| Guest pages parsed on every update (cost, not a breach) | Parsed once typing pauses | `a_guests_page_is_parsed_once_typing_pauses` |
| Malformed or hostile CRDT data | A grammar check before `yrs` decodes anything; panics caught; the whole text compared after each remote update; a replica that may be wrong is poisoned and rebuilt in a new epoch | `collab_malformed_update_changes_nothing`, `an_update_splitting_a_character_poisons_the_replica`, `a_hostile_update_makes_the_host_rebuild_and_the_guest_start_again`; fuzz target `collab_update` |
| Allocation bombs | Every count bounded by the bytes that follow; frames bounded before allocation | `counts_claiming_more_than_follows_are_refused`, `frames_over_the_limit_are_refused_before_reading`; fuzz target `collab_frame` |
| Flooding | Bounded per-peer queues; a slow peer closed, never waited for; bounded events per pump | `collab_slow_peer_never_blocks_host`, `a_session_drains_a_bounded_number_of_events_a_pump` |
| Remote commands | Structural commands refused while hosting; no command runs on a guest's request; no diagram program runs on shared text | `collab_structural_commands_refused`, `every_command_is_decided_for_a_session` |
| A guest's disk read for shared pages | Shared pages live under a folder never read; links lead only to shared pages | `collab_scope_never_leaks` |
| Dependencies | `rustls` with ring, `tokio-rustls`, `rcgen`, only in `adoc-share` and the app; licences and advisories | `cargo deny check`, `cargo xtask layers` (`TLS_CRATES`, `TLS_ALLOWED`, `COLLAB_FREE`) |

Malicious authenticated clients are tested as raw peers that break the protocol on purpose (`crates/adoc-share/tests/transport.rs`) and by forged messages in the in-memory session (`collab_session.rs`).

## Manual checks

Run with two copies of the app on two machines on one local network, unless the check says otherwise. Record the systems, versions and network beside each result.

| Check | macOS | Windows | Linux |
|---|---|---|---|
| Share on the local network: the firewall (or, on macOS, local network) prompt appears once, says why, and a guest can then connect | [ ] | [ ] | [ ] |
| Share on *This computer only*: no prompt; a second copy on the same machine joins | [ ] | [ ] | [ ] |
| Copy Invitation and paste it into Join Session on the other machine; the safety codes match | [ ] | [ ] | [ ] |
| Admit, Let Edit, Make Viewer and Remove behave as the Session panel says | [ ] | [ ] | [ ] |
| Both people type fast in one page for a minute; texts match, carets stay on their text, nothing lags | [ ] | [ ] | [ ] |
| Input methods (Japanese or Chinese, and dead keys) on each side while the other types | [ ] | [ ] | [ ] |
| Copy and paste between a shared page and elsewhere, both ways | [ ] | [ ] | [ ] |
| Pull the network cable or turn off Wi-Fi for a minute; Reconnect brings the guest back with its edits | [ ] | [ ] | [ ] |
| Quit the guest's app mid-session; the host carries on and shows the guest disconnected | [ ] | [ ] | [ ] |
| The host saves; the guest's status bar shows Saved; the file on disk is the text | [ ] | [ ] | [ ] |
| Change a shared file in another editor; the host is asked which to keep | [ ] | [ ] | [ ] |
| Screen reader: the Session panel's participants, roles and caret places are read | [ ] | [ ] | [ ] |
| Follow someone moving between pages and typing: your view goes with them and keeps their caret in view; a click, a scroll or typing ends it (#767) | [ ] | [ ] | [ ] |
| Comment on a shared page from each side, a Viewer included, while the other types around it; both see the same comments on the same text, and the host's `.afd` file holds them (#762) | [ ] | [ ] | [ ] |
| A guest joins with an address; the host's Session panel shows it under their name, marked not checked, before admitting them; the host edits nothing, the guest changes one page; the host's commit dialog offers the guest for that page only, and the commit pushed to GitHub shows them as a co-author; the other guest's screen never shows the address (#761) | [ ] | [ ] | [ ] |
| Stop Sharing: guests are told, the port is closed | [ ] | [ ] | [ ] |
| The Flatpak package shares and joins on the local network (Linux only) | | | [ ] |

## Release decisions

- **A guest claiming another's address (#761):** whose a comment is, and who is credited, rest on the address a guest gives, which nobody verifies. Decided: the host sees the address when the guest knocks. The Session panel shows it under the guest's name, marked as given by the guest and not checked (`Gives the address ada@example.com (not checked)`), so the host can turn away a claim it does not expect before admitting it. It is display text only, shown on the host alone, never sent to a guest or logged. No DNS or other network check is made; that is left for a later issue.
- **Flatpak:** the package may use the network (`--share=network` in its manifest, held by `packaging/test_bundle.py`), so it can share and join; nothing listens or connects until the person chooses to.
