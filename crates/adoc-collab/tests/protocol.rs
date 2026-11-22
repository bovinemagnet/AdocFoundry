//! The session protocol's frames (design §8): every message survives a round trip, and
//! anything else is refused before it is believed.

use adoc_collab::{
    protocol::{
        commit_email, decode, display_path, frame_length, CommentChange, Credential, ErrorCode,
        FileKind, FrameError, ManifestFile, Message, PeerId, Role, SaveOutcome, Secret, SessionId,
        SharedOutcome, SharedReply, SharedResolution, SharedSuggestion, SharedThread,
        SuggestionChange, MANIFEST_PART, MAX_COMMENT_QUOTE, MAX_COMMENT_REPLIES, MAX_COMMENT_TEXT,
        MAX_CONTROL, MAX_EMAIL, MAX_FRAME, MAX_NAME, MAX_PATH, MAX_TEXT,
    },
    EncodedUpdate, RelativeSelection, Replica, ReplicaId, StateVectorBytes, MAX_UPDATE,
};

fn round_trip(message: &Message) -> Message {
    let frame = message.encode().unwrap();
    let length = frame_length(frame[..4].try_into().unwrap()).unwrap();
    assert_eq!(length, frame.len() - 4);
    decode(&frame[4..]).unwrap()
}

fn every_message() -> Vec<Message> {
    let replica = Replica::new("= Page\n", ReplicaId::new(7).unwrap()).unwrap();
    let selection = replica
        .selection(2, 4, adoc_collab::Affinity::After)
        .unwrap();
    vec![
        Message::Hello {
            min: 1,
            max: 1,
            app: "0.0.0".to_owned(),
        },
        Message::Authenticate {
            credential: Credential::Invitation(Secret([7; 32])),
            name: "Ada".to_owned(),
            email: None,
        },
        Message::Authenticate {
            credential: Credential::Resume(Secret([9; 32])),
            name: "Ada".to_owned(),
            email: None,
        },
        Message::Join {
            peer: PeerId(1),
            role: Role::Viewer,
            session: SessionId([3; 16]),
            resume: Secret([1; 32]),
        },
        Message::Manifest {
            files: vec![ManifestFile {
                file: 1,
                path: "docs/modules/ROOT/pages/index.adoc".to_owned(),
                kind: FileKind::Page,
                size: 7,
            }],
            more: false,
        },
        Message::OpenFile { file: 1 },
        Message::CloseFile { file: 1 },
        Message::Snapshot {
            file: 1,
            epoch: 1,
            client: 12,
            update: replica.snapshot().unwrap(),
        },
        Message::Update {
            file: 1,
            epoch: 1,
            seq: 1,
            base: 12,
            update: EncodedUpdate(vec![1, 2, 3]),
        },
        Message::UpdateAck {
            file: 1,
            epoch: 1,
            seq: 1,
        },
        Message::Awareness {
            file: 1,
            epoch: 1,
            peer: PeerId(2),
            selection: selection.clone(),
        },
        Message::SaveRequest {
            file: 1,
            epoch: 1,
            request: 4,
        },
        Message::SaveResult {
            request: 4,
            outcome: SaveOutcome::Saved {
                revision: "abc".to_owned(),
            },
        },
        Message::SaveResult {
            request: 5,
            outcome: SaveOutcome::PermissionDenied,
        },
        Message::RoleChanged {
            peer: PeerId(1),
            role: Role::Editor,
        },
        Message::Components {
            components: vec![
                adoc_collab::protocol::SharedComponent {
                    dir: "docs".to_owned(),
                    name: "guide".to_owned(),
                    version: Some("2.0".to_owned()),
                },
                adoc_collab::protocol::SharedComponent {
                    dir: String::new(),
                    name: "notes".to_owned(),
                    version: None,
                },
            ],
        },
        Message::Roster {
            workspace: "docs".to_owned(),
            members: vec![
                adoc_collab::protocol::Member {
                    peer: PeerId::HOST,
                    name: "Host".to_owned(),
                    role: Role::Owner,
                    connected: true,
                    file: None,
                    following: None,
                },
                adoc_collab::protocol::Member {
                    peer: PeerId(3),
                    name: "Ada".to_owned(),
                    role: Role::Viewer,
                    connected: false,
                    file: None,
                    following: None,
                },
            ],
        },
        Message::Roster {
            workspace: "docs".to_owned(),
            members: vec![adoc_collab::protocol::Member {
                peer: PeerId(2),
                name: "Bea".to_owned(),
                role: Role::Editor,
                connected: true,
                file: Some(4),
                following: Some(PeerId::HOST),
            }],
        },
        Message::Focus {
            file: Some(3),
            following: Some(PeerId(2)),
        },
        Message::Focus {
            file: None,
            following: None,
        },
        Message::FileState {
            file: 1,
            epoch: 2,
            saved: Some("0f".repeat(32)),
            conflict: false,
        },
        Message::FileState {
            file: 1,
            epoch: 2,
            saved: None,
            conflict: true,
        },
        Message::SaveResult {
            request: 6,
            outcome: SaveOutcome::Stale,
        },
        Message::Resync {
            file: 1,
            epoch: 2,
            state: replica.state_vector(),
        },
        Message::Leave {
            reason: "done".to_owned(),
        },
        Message::Error {
            code: ErrorCode::Slow,
            message: "too slow".to_owned(),
        },
        Message::Comment {
            file: 3,
            epoch: 2,
            request: 9,
            change: CommentChange::Add {
                text: "Why?\n\tSee below.".to_owned(),
                suggestion: None,
            },
            selection: Some(selection.clone()),
        },
        Message::Comment {
            file: 3,
            epoch: 2,
            request: 10,
            change: CommentChange::Edit {
                comment: 11,
                reply: Some(12),
                text: "Because.".to_owned(),
            },
            selection: None,
        },
        Message::Comment {
            file: 3,
            epoch: 2,
            request: 11,
            change: CommentChange::Resolve {
                comment: 11,
                resolved: true,
            },
            selection: None,
        },
        Message::CommentThread {
            file: 3,
            epoch: 2,
            thread: Box::new(thread()),
            selection: Some(selection.clone()),
        },
        Message::CommentThread {
            file: 3,
            epoch: 2,
            thread: Box::new(SharedThread {
                detached: true,
                ..thread()
            }),
            selection: None,
        },
        Message::CommentRemoved {
            file: 3,
            epoch: 2,
            comment: 11,
        },
        Message::CommentRefused {
            file: 3,
            request: 9,
            reason: "a Viewer cannot resolve comments".to_owned(),
        },
        Message::Heartbeat,
    ]
}

fn thread() -> SharedThread {
    SharedThread {
        id: 11,
        author: "Ada".to_owned(),
        own: true,
        created: 1_795_000_000,
        edited: None,
        quote: "Page".to_owned(),
        text: "Why?".to_owned(),
        resolved: Some(SharedResolution {
            by: "Bea".to_owned(),
            at: 1_795_000_100,
        }),
        detached: false,
        suggestion: None,
        replies: vec![SharedReply {
            id: 12,
            author: "Bea".to_owned(),
            own: false,
            created: 1_795_000_050,
            edited: Some(1_795_000_060),
            text: "Because.".to_owned(),
        }],
    }
}

#[test]
fn comments_are_held_to_their_limits() {
    let replica = Replica::new("= Page\n", ReplicaId::new(7).unwrap()).unwrap();
    let selection = replica
        .selection(2, 4, adoc_collab::Affinity::After)
        .unwrap();
    let add = |text: String, selection: Option<RelativeSelection>| Message::Comment {
        file: 1,
        epoch: 1,
        request: 1,
        change: CommentChange::Add {
            text,
            suggestion: None,
        },
        selection,
    };
    assert!(add("x".repeat(MAX_COMMENT_TEXT), Some(selection.clone()))
        .encode()
        .is_ok());
    for refused in [
        add("x".repeat(MAX_COMMENT_TEXT + 1), Some(selection.clone())),
        add("  ".to_owned(), Some(selection.clone())),
        add("bell\u{7}".to_owned(), Some(selection.clone())),
        add("No range.".to_owned(), None),
        Message::Comment {
            file: 1,
            epoch: 1,
            request: 1,
            change: CommentChange::Delete {
                comment: 1,
                reply: None,
            },
            selection: Some(selection.clone()),
        },
        Message::CommentThread {
            file: 1,
            epoch: 1,
            thread: Box::new(SharedThread {
                replies: vec![thread().replies[0].clone(); MAX_COMMENT_REPLIES + 1],
                ..thread()
            }),
            selection: Some(selection.clone()),
        },
        Message::CommentThread {
            file: 1,
            epoch: 1,
            thread: Box::new(SharedThread {
                author: "a".repeat(MAX_NAME + 1),
                ..thread()
            }),
            selection: Some(selection.clone()),
        },
        Message::CommentThread {
            file: 1,
            epoch: 1,
            thread: Box::new(thread()),
            selection: None,
        },
    ] {
        assert!(refused.encode().is_err(), "{refused:?}");
    }
    // A thread's JSON that names a field it does not know is refused.
    let frame = Message::CommentThread {
        file: 1,
        epoch: 1,
        thread: Box::new(thread()),
        selection: Some(selection),
    }
    .encode()
    .unwrap();
    let text = String::from_utf8_lossy(&frame[4..]).replace("\"quote\"", "\"quota\"");
    assert!(decode(text.as_bytes()).is_err());
}

#[test]
fn every_message_survives_a_round_trip() {
    for message in every_message() {
        assert_eq!(round_trip(&message), message);
    }
}

#[test]
fn frames_over_the_limit_are_refused_before_reading() {
    let over = u32::try_from(MAX_FRAME + 1).unwrap().to_be_bytes();
    assert_eq!(frame_length(over), Err(FrameError::TooLarge(MAX_FRAME + 1)));
    assert_eq!(
        frame_length(u32::MAX.to_be_bytes()),
        Err(FrameError::TooLarge(u32::MAX as usize))
    );
    assert_eq!(frame_length(2u32.to_be_bytes()), Err(FrameError::Truncated));
    assert_eq!(
        frame_length(u32::try_from(MAX_FRAME).unwrap().to_be_bytes()),
        Ok(MAX_FRAME)
    );
}

#[test]
fn the_largest_update_fits_a_frame() {
    let message = Message::Update {
        file: u64::MAX,
        epoch: u32::MAX,
        seq: u64::MAX,
        base: u64::MAX,
        update: EncodedUpdate(vec![0; MAX_UPDATE]),
    };
    assert_eq!(round_trip(&message), message);
    let over = Message::Update {
        file: 1,
        epoch: 1,
        seq: 1,
        base: 0,
        update: EncodedUpdate(vec![0; MAX_UPDATE + 1]),
    };
    assert_eq!(over.encode(), Err(FrameError::TooLarge(MAX_UPDATE + 1)));
}

/// The body of a frame: kind, control length, control, payload.
fn body(kind: u8, control: &str, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![kind];
    frame.extend_from_slice(&u16::try_from(control.len()).unwrap().to_be_bytes());
    frame.extend_from_slice(control.as_bytes());
    frame.extend_from_slice(payload);
    frame
}

#[test]
fn malformed_frames_are_refused() {
    assert_eq!(decode(&[]), Err(FrameError::Truncated));
    assert_eq!(decode(&[5, 0]), Err(FrameError::Truncated));
    // A control part longer than the frame.
    assert_eq!(decode(&[5, 0, 9, b'{']), Err(FrameError::Truncated));
    assert_eq!(decode(&body(0, "{}", &[])), Err(FrameError::UnknownKind(0)));
    assert_eq!(
        decode(&body(200, "{}", &[])),
        Err(FrameError::UnknownKind(200))
    );
    // Not JSON, the wrong JSON, an unknown field, a missing field.
    for control in [
        "{",
        "[]",
        r#"{"file":1,"extra":true}"#,
        r#"{}"#,
        r#"{"file":-1}"#,
        r#"{"file":"1"}"#,
    ] {
        assert!(
            matches!(decode(&body(5, control, &[])), Err(FrameError::Control(_))),
            "{control}"
        );
    }
    // A payload where the kind has none, and none where it needs one.
    assert_eq!(
        decode(&body(5, r#"{"file":1}"#, &[1])),
        Err(FrameError::Payload)
    );
    assert_eq!(
        decode(&body(8, r#"{"file":1,"epoch":1,"seq":1,"base":0}"#, &[])),
        Err(FrameError::Payload)
    );
    assert_eq!(
        decode(&body(14, r#"{"file":1,"epoch":1}"#, &[])),
        Err(FrameError::Payload)
    );
    // An awareness anchor longer than the payload.
    assert_eq!(
        decode(&body(
            10,
            r#"{"file":1,"epoch":1,"peer":1,"anchor":9}"#,
            &[1, 2]
        )),
        Err(FrameError::Payload)
    );
    // A heartbeat with anything in it.
    assert!(matches!(
        decode(&body(17, r#"{"x":1}"#, &[])),
        Err(FrameError::Control(_))
    ));
}

#[test]
fn fields_outside_their_limits_are_refused() {
    let long_name = "a".repeat(MAX_NAME + 1);
    for name in ["", " ", "a\nb", "a\u{7}", long_name.as_str()] {
        let message = Message::Authenticate {
            credential: Credential::Invitation(Secret([0; 32])),
            name: name.to_owned(),
            email: None,
        };
        assert_eq!(
            message.encode(),
            Err(FrameError::Invalid("name")),
            "{name:?}"
        );
        let control = format!(
            r#"{{"credential":{{"invitation":"{}"}},"name":{}}}"#,
            "0".repeat(64),
            serde_json::to_string(name).unwrap()
        );
        assert_eq!(
            decode(&body(2, &control, &[])),
            Err(FrameError::Invalid("name")),
            "{name:?}"
        );
    }
    // A secret that is not 64 hexadecimal digits.
    for secret in ["", "00", &"g".repeat(64), &"A".repeat(64), &"0".repeat(65)] {
        let control = format!(r#"{{"credential":{{"invitation":"{secret}"}},"name":"Ada"}}"#);
        assert!(matches!(
            decode(&body(2, &control, &[])),
            Err(FrameError::Control(_))
        ));
    }
    // The host's own number and role are never given to a guest.
    let host = Message::Join {
        peer: PeerId::HOST,
        role: Role::Viewer,
        session: SessionId([0; 16]),
        resume: Secret([0; 32]),
    };
    assert_eq!(host.encode(), Err(FrameError::Invalid("peer")));
    let owner = Message::RoleChanged {
        peer: PeerId(2),
        role: Role::Owner,
    };
    assert_eq!(owner.encode(), Err(FrameError::Invalid("role")));
    // A client id outside what yrs keeps.
    for client in [0, ReplicaId::MAX + 1] {
        let snapshot = Message::Snapshot {
            file: 1,
            epoch: 1,
            client,
            update: EncodedUpdate(vec![0]),
        };
        assert_eq!(snapshot.encode(), Err(FrameError::Invalid("client")));
    }
    let versions = Message::Hello {
        min: 2,
        max: 1,
        app: String::new(),
    };
    assert_eq!(versions.encode(), Err(FrameError::Invalid("versions")));
    let long = Message::Leave {
        reason: "a".repeat(MAX_TEXT + 1),
    };
    assert_eq!(long.encode(), Err(FrameError::TooLarge(MAX_TEXT + 1)));
    let selection = Message::Awareness {
        file: 1,
        epoch: 1,
        peer: PeerId(1),
        selection: RelativeSelection {
            anchor: vec![0; adoc_collab::MAX_ANCHOR + 1],
            head: Vec::new(),
        },
    };
    assert!(matches!(selection.encode(), Err(FrameError::TooLarge(_))));
    for revision in ["", "abc", &"G".repeat(64), &"A".repeat(64), &"0".repeat(65)] {
        let state = Message::FileState {
            file: 1,
            epoch: 1,
            saved: Some(revision.to_owned()),
            conflict: false,
        };
        assert_eq!(
            state.encode(),
            Err(FrameError::Invalid("revision")),
            "{revision}"
        );
    }
    for (name, version) in [
        ("a b", None),
        ("x: y", None),
        ("guide", Some("1\n2")),
        ("", None),
    ] {
        let bad = Message::Components {
            components: vec![adoc_collab::protocol::SharedComponent {
                dir: String::new(),
                name: name.to_owned(),
                version: version.map(str::to_owned),
            }],
        };
        assert_eq!(
            bad.encode(),
            Err(FrameError::Invalid("component")),
            "{name:?}"
        );
    }
    let unnamed = Message::Roster {
        workspace: "docs".to_owned(),
        members: vec![adoc_collab::protocol::Member {
            peer: PeerId(1),
            name: String::new(),
            role: Role::Viewer,
            connected: true,
            file: None,
            following: None,
        }],
    };
    assert_eq!(unnamed.encode(), Err(FrameError::Invalid("name")));
    let crowd = Message::Roster {
        workspace: "docs".to_owned(),
        members: vec![
            adoc_collab::protocol::Member {
                peer: PeerId(1),
                name: "A".to_owned(),
                role: Role::Viewer,
                connected: true,
                file: None,
                following: None,
            };
            adoc_collab::protocol::MAX_PARTICIPANTS + 1
        ],
    };
    assert!(matches!(crowd.encode(), Err(FrameError::TooLarge(_))));
    let empty = Message::Resync {
        file: 1,
        epoch: 1,
        state: StateVectorBytes(Vec::new()),
    };
    assert_eq!(empty.encode(), Err(FrameError::Payload));
}

#[test]
fn display_paths_can_only_be_shown() {
    for good in ["index.adoc", "modules/ROOT/pages/a b.adoc", "ü/ñ.adoc"] {
        assert_eq!(display_path(good), Ok(()), "{good}");
    }
    let long = "a".repeat(MAX_PATH + 1);
    for bad in [
        "",
        "/etc/passwd",
        "../secret.adoc",
        "a/../b.adoc",
        "a/./b.adoc",
        "a//b.adoc",
        "a/",
        ".git/config",
        "pages/.hidden.adoc",
        "a\\b.adoc",
        "C:/x.adoc",
        "a\nb.adoc",
        long.as_str(),
    ] {
        assert_eq!(
            display_path(bad),
            Err(FrameError::Invalid("path")),
            "{bad:?}"
        );
    }
}

#[test]
fn a_manifest_part_of_the_longest_paths_fits_its_control_part() {
    // Quotes are the one character JSON escapes that a path may hold: each becomes two bytes.
    let path = "\"".repeat(MAX_PATH);
    let files = (0..MANIFEST_PART)
        .map(|file| ManifestFile {
            file: u64::MAX - file as u64,
            path: path.clone(),
            kind: FileKind::Partial,
            size: u64::MAX,
        })
        .collect::<Vec<_>>();
    let message = Message::Manifest { files, more: true };
    let frame = message.encode().unwrap();
    assert!(frame.len() - 7 <= MAX_CONTROL);
    assert_eq!(round_trip(&message), message);

    let over = Message::Manifest {
        files: vec![
            ManifestFile {
                file: 1,
                path: "a.adoc".to_owned(),
                kind: FileKind::Page,
                size: 1,
            };
            MANIFEST_PART + 1
        ],
        more: false,
    };
    assert_eq!(over.encode(), Err(FrameError::TooLarge(MANIFEST_PART + 1)));
}

#[test]
fn secrets_are_never_printed() {
    let secret = Secret([0xab; 32]);
    assert_eq!(format!("{secret:?}"), "Secret(..)");
    let message = Message::Authenticate {
        credential: Credential::Invitation(secret.clone()),
        name: "Ada".to_owned(),
        email: None,
    };
    assert!(!format!("{message:?}").contains("abab"));
    assert_eq!(Secret::from_hex(&secret.to_hex()), Some(secret));
}

#[test]
fn an_address_round_trips() {
    let with = Message::Authenticate {
        credential: Credential::Invitation(Secret([7; 32])),
        name: "Ada".to_owned(),
        email: Some("ada@example.org".to_owned()),
    };
    assert_eq!(round_trip(&with), with);
    let without = Message::Authenticate {
        credential: Credential::Invitation(Secret([7; 32])),
        name: "Ada".to_owned(),
        email: None,
    };
    assert_eq!(round_trip(&without), without);
    let frame = without.encode().unwrap();
    assert!(!String::from_utf8_lossy(&frame).contains("email"));
}

#[test]
fn addresses_are_held_to_the_rule() {
    for good in ["ada@example.org", "a.b+c@d.e"] {
        assert_eq!(commit_email(good), Ok(()), "{good:?}");
    }
    let long = format!("{}@example.org", "a".repeat(MAX_EMAIL));
    assert!(long.len() > MAX_EMAIL);
    let refused = [
        "",
        "ada",
        "@x",
        "a@",
        "a@b@c",
        "a b@c",
        "a@b\n",
        "<a@b>",
        "a,b@c",
        "a\"b@c",
        "a\\b@c",
        long.as_str(),
    ];
    for bad in refused {
        assert_eq!(
            commit_email(bad),
            Err(FrameError::Invalid("email")),
            "{bad:?}"
        );
        let message = Message::Authenticate {
            credential: Credential::Invitation(Secret([0; 32])),
            name: "Ada".to_owned(),
            email: Some(bad.to_owned()),
        };
        assert_eq!(
            message.encode(),
            Err(FrameError::Invalid("email")),
            "{bad:?}"
        );
        let control = format!(
            r#"{{"credential":{{"invitation":"{}"}},"name":"Ada","email":{}}}"#,
            "0".repeat(64),
            serde_json::to_string(bad).unwrap()
        );
        assert_eq!(
            decode(&body(2, &control, &[])),
            Err(FrameError::Invalid("email")),
            "{bad:?}"
        );
    }
}

#[test]
fn threads_say_whose_they_are() {
    let replica = Replica::new("= Page\n", ReplicaId::new(7).unwrap()).unwrap();
    let selection = replica
        .selection(2, 4, adoc_collab::Affinity::After)
        .unwrap();
    let mut shared = thread();
    shared.replies[0].own = true;
    let message = Message::CommentThread {
        file: 3,
        epoch: 2,
        thread: Box::new(shared),
        selection: Some(selection),
    };
    let back = round_trip(&message);
    let Message::CommentThread { thread, .. } = &back else {
        panic!("{back:?}");
    };
    assert!(thread.own && thread.replies[0].own);
    assert_eq!(back, message);
}

fn suggestion_message(change: CommentChange, selection: bool) -> Message {
    let replica = Replica::new("= Page\n", ReplicaId::new(7).unwrap()).unwrap();
    Message::Comment {
        file: 1,
        epoch: 1,
        request: 1,
        change,
        selection: selection.then(|| {
            replica
                .selection(2, 4, adoc_collab::Affinity::After)
                .unwrap()
        }),
    }
}

#[test]
fn suggestions_round_trip() {
    let changes = [
        (
            CommentChange::Add {
                text: "Clearer.".to_owned(),
                suggestion: Some(SuggestionChange {
                    original: "Page".to_owned(),
                    replacement: "Leaf\n\tnew".to_owned(),
                }),
            },
            true,
        ),
        (CommentChange::Accept { comment: 4 }, false),
        (CommentChange::Reject { comment: 4 }, false),
        (
            CommentChange::EditSuggestion {
                comment: 4,
                replacement: String::new(),
                text: String::new(),
            },
            false,
        ),
    ];
    for (change, selection) in changes {
        let message = suggestion_message(change, selection);
        assert_eq!(round_trip(&message), message);
    }
    for outcome in [
        None,
        Some(SharedOutcome::Accepted),
        Some(SharedOutcome::Rejected),
    ] {
        let message = Message::CommentThread {
            file: 3,
            epoch: 2,
            thread: Box::new(SharedThread {
                suggestion: Some(SharedSuggestion {
                    original: "Page".to_owned(),
                    replacement: "Leaf".to_owned(),
                    outcome,
                    uncertain: outcome.is_none(),
                }),
                ..thread()
            }),
            selection: Some(
                Replica::new("= Page\n", ReplicaId::new(7).unwrap())
                    .unwrap()
                    .selection(2, 4, adoc_collab::Affinity::After)
                    .unwrap(),
            ),
        };
        assert_eq!(round_trip(&message), message);
    }
}

#[test]
fn suggestion_fields_are_held_to_their_limits() {
    let add = |text: &str, original: String, replacement: String| {
        suggestion_message(
            CommentChange::Add {
                text: text.to_owned(),
                suggestion: Some(SuggestionChange {
                    original,
                    replacement,
                }),
            },
            true,
        )
    };
    let edit = |replacement: String, text: &str| {
        suggestion_message(
            CommentChange::EditSuggestion {
                comment: 1,
                replacement,
                text: text.to_owned(),
            },
            false,
        )
    };
    let thread_with = |text: &str, original: String, replacement: String| Message::CommentThread {
        file: 1,
        epoch: 1,
        thread: Box::new(SharedThread {
            text: text.to_owned(),
            suggestion: Some(SharedSuggestion {
                original,
                replacement,
                outcome: None,
                uncertain: false,
            }),
            ..thread()
        }),
        selection: Some(
            Replica::new("= Page\n", ReplicaId::new(7).unwrap())
                .unwrap()
                .selection(2, 4, adoc_collab::Affinity::After)
                .unwrap(),
        ),
    };
    for accepted in [
        add("", "Page".to_owned(), String::new()),
        add(
            "",
            "x".repeat(MAX_COMMENT_QUOTE),
            "x".repeat(MAX_COMMENT_TEXT),
        ),
        edit(String::new(), ""),
        thread_with("", "Page".to_owned(), String::new()),
    ] {
        assert!(accepted.encode().is_ok(), "{accepted:?}");
    }
    for refused in [
        add("", "x".repeat(MAX_COMMENT_QUOTE + 1), "y".to_owned()),
        add("", "Page".to_owned(), "x".repeat(MAX_COMMENT_TEXT + 1)),
        add("", "Pa\u{7}ge".to_owned(), "y".to_owned()),
        add("", "Page".to_owned(), "y\u{7}".to_owned()),
        add("", String::new(), "y".to_owned()),
        edit("x".repeat(MAX_COMMENT_TEXT + 1), ""),
        edit("bell\u{7}".to_owned(), ""),
        edit("y".to_owned(), "bell\u{7}"),
        thread_with("", String::new(), "y".to_owned()),
        thread_with("", "Page".to_owned(), "bell\u{7}".to_owned()),
        thread_with("", "x".repeat(MAX_COMMENT_QUOTE + 1), "y".to_owned()),
        // An empty note is allowed only with a suggestion.
        suggestion_message(
            CommentChange::Add {
                text: String::new(),
                suggestion: None,
            },
            true,
        ),
        Message::CommentThread {
            file: 1,
            epoch: 1,
            thread: Box::new(SharedThread {
                text: String::new(),
                ..thread()
            }),
            selection: Some(
                Replica::new("= Page\n", ReplicaId::new(7).unwrap())
                    .unwrap()
                    .selection(2, 4, adoc_collab::Affinity::After)
                    .unwrap(),
            ),
        },
    ] {
        assert!(refused.encode().is_err(), "{refused:?}");
    }
}
