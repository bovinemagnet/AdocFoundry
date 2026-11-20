//! The session protocol's frames (design §8): every message survives a round trip, and
//! anything else is refused before it is believed.

use adoc_collab::{
    protocol::{
        decode, display_path, frame_length, Credential, ErrorCode, FileKind, FrameError,
        ManifestFile, Message, PeerId, Role, SaveOutcome, Secret, SessionId, MANIFEST_PART,
        MAX_CONTROL, MAX_FRAME, MAX_NAME, MAX_PATH, MAX_TEXT,
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
        },
        Message::Authenticate {
            credential: Credential::Resume(Secret([9; 32])),
            name: "Ada".to_owned(),
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
            selection,
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
                },
                adoc_collab::protocol::Member {
                    peer: PeerId(3),
                    name: "Ada".to_owned(),
                    role: Role::Viewer,
                    connected: false,
                },
            ],
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
        Message::Heartbeat,
    ]
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
        update: EncodedUpdate(vec![0; MAX_UPDATE]),
    };
    assert_eq!(round_trip(&message), message);
    let over = Message::Update {
        file: 1,
        epoch: 1,
        seq: 1,
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
        decode(&body(8, r#"{"file":1,"epoch":1,"seq":1}"#, &[])),
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
    };
    assert!(!format!("{message:?}").contains("abab"));
    assert_eq!(Secret::from_hex(&secret.to_hex()), Some(secret));
}
