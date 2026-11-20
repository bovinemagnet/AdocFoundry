//! The session protocol's frame decoder (COLLAB 4/8): any bytes a peer sends must be refused or
//! decoded without panicking or allocating past the frame, and what decodes must encode again
//! to the same message, so nothing the host forwards is closed on at the other end.
#![no_main]

use adoc_collab::protocol::{decode, frame_length};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some(prefix) = data.get(..4) {
        let _ = frame_length(prefix.try_into().unwrap());
    }
    if let Ok(message) = decode(data) {
        let frame = message.encode().expect("a decoded message encodes");
        let again = decode(&frame[4..]).expect("an encoded message decodes");
        assert_eq!(again, message);
    }
});
