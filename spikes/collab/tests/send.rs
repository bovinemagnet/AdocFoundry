//! Whether `yrs`'s handles can live in a `Send` engine, which the MCP server and the app move
//! between threads.
fn send<T: Send>() {}

#[test]
fn yrs_handles_are_send() {
    send::<yrs::Doc>();
    send::<yrs::TextRef>();
    send::<yrs::undo::UndoManager<()>>();
}

#[test]
fn the_replica_is_send() {
    send::<adocfoundry_collab_spike::Replica>();
}
