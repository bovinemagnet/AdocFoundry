//! The collaboration core under untrusted updates (COLLAB design §6, §12): any bytes a modified
//! peer could send, raw or as one-byte changes to an honest update, are offered to a replica
//! as from the guest who wrote the honest one and as from the host, and their authors read. Nothing panics; a refused update changes neither the text
//! nor the state vector unless it poisons the replica; an accepted one's byte edits rebuild the
//! text exactly.
//!
//! The first byte chooses: its low bits an honest update (an insertion, a deletion or a
//! replacement), its high bit whether the rest is the update itself or `(position, byte)` pairs
//! changing the honest one, which reaches deeper into the decoder than random bytes do.
//!
//! `yrs` itself can panic on hostile bytes (a `debug_assert!` on a client id wider than 53
//! bits, in builds with debug assertions, as fuzz builds are), and the replica guards every call
//! into it with `catch_unwind`. libFuzzer's panic hook would abort on those caught panics, so a
//! panic raised inside `yrs` is let unwind to the guard; any other panic still aborts.

#![no_main]

use std::sync::Once;

use adoc_collab::{
    apply_edits, authors, ByteEdit, EncodedUpdate, RelativeSelection, Replica, ReplicaId, Sender,
};
use libfuzzer_sys::fuzz_target;

static GUARDED_PANICS: Once = Once::new();

/// Lets panics raised inside `yrs` unwind to the replica's guard instead of aborting.
fn let_yrs_panics_unwind() {
    GUARDED_PANICS.call_once(|| {
        let fatal = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let in_yrs = info
                .location()
                .is_some_and(|location| location.file().contains("/yrs-"));
            // A panic in `yrs` the replica guards is let unwind to its guard. One that escapes
            // it aborts in libFuzzer's entry ("panic in a function that cannot unwind"); to
            // find it, print `info` and `std::backtrace::Backtrace::force_capture()` here.
            if !in_yrs {
                fatal(info);
            }
        }));
    });
}

fn id(value: u64) -> ReplicaId {
    ReplicaId::new(value).expect("a replica id")
}

fuzz_target!(|data: &[u8]| {
    let_yrs_panics_unwind();
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let host = Replica::new("= Page\n\nUn café 👋\r\nMore.\n", id(1)).expect("the host");
    let snapshot = host.snapshot().expect("a snapshot");
    let mut author = Replica::from_snapshot(&snapshot, id(2)).expect("the author");
    let edit = match mode % 3 {
        0 => ByteEdit::new(8, 8, "Typed "),
        1 => ByteEdit::new(11, 16, ""),
        _ => ByteEdit::new(8, 17, "Thé 🇬🇧"),
    };
    let honest = author
        .apply_local(&[edit])
        .expect("an honest edit")
        .outbound
        .expect("an update");
    let input = if mode & 0x80 == 0 {
        rest.to_vec()
    } else {
        let mut mutated = honest.0;
        for pair in rest.chunks_exact(2) {
            if !mutated.is_empty() {
                let at = pair[0] as usize % mutated.len();
                mutated[at] = pair[1];
            }
        }
        mutated
    };
    // Another participant's selection, from the same bytes: read without a panic, and never
    // past the end of the text.
    let (anchor, head) = rest.split_at(rest.len() / 2);
    if let Ok(selection) = RelativeSelection::from_wire(anchor.to_vec(), head.to_vec()) {
        if let Some((start, end)) = host.resolve(&selection) {
            assert!(start <= host.len() && end <= host.len());
        }
    }
    let update = EncodedUpdate(input);
    let _ = authors(&update);

    // The host receiving it from the guest who wrote it, and a guest receiving it as if the
    // host had forwarded it.
    for sender in [Sender::Guest(id(2)), Sender::Host] {
        let mut receiver = Replica::from_snapshot(&snapshot, id(3)).expect("the receiver");
        let before = receiver.text();
        let state = receiver.state_vector();
        match receiver.apply_remote(&update, sender) {
            Ok(change) => {
                let mut rope = before;
                apply_edits(&mut rope, &change.edits);
                assert_eq!(rope, receiver.text());
                // Whatever was accepted is passed on whole only as a snapshot that rebuilds the
                // same text.
                match receiver.snapshot() {
                    Ok(snapshot) => {
                        let copy =
                            Replica::from_snapshot(&snapshot, id(4)).expect("its own snapshot");
                        assert_eq!(copy.text(), receiver.text());
                    }
                    // Refused: waiting for something, or a state whose encoding would
                    // rebuild another text, which the host then rebuilds from its own.
                    Err(error) => assert!(
                        change.pending || error == adoc_collab::CollabError::ProjectionMismatch,
                        "{error}"
                    ),
                }
            }
            Err(_) if !receiver.is_poisoned() => {
                assert_eq!(receiver.text(), before);
                assert_eq!(receiver.state_vector(), state);
            }
            Err(_) => {}
        }
    }
});
