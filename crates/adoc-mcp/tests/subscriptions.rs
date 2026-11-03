//! Resource subscriptions (MCP PRD §§7.7 and 11.4, test 26; #335): a subscriber is told when its
//! resource changes on disk, and of nothing else: not of other resources, and not of an edit
//! that is only in the server's buffers, which every session shares.

mod common;

use std::{path::Path, time::Duration};

use common::{scratch, Client};
use serde_json::{json, Value};

const UPDATED: &str = "notifications/resources/updated";
const INSTALL: &str = "product/main/modules/ROOT/pages/install.adoc";
const GUIDE: &str = "guide/modules/ROOT/pages/index.adoc";

/// Long enough for the idle pump (every 250 ms) to see a change on disk and announce it.
const SETTLE: Duration = Duration::from_millis(2_500);

fn uri(path: &Path) -> String {
    format!("file://{}", path.to_string_lossy().replace('\\', "/"))
}

fn uris(notifications: &[Value]) -> Vec<String> {
    notifications
        .iter()
        .map(|notification| notification["params"]["uri"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn mcp_resource_change_notifications_are_revision_scoped() {
    let (_directory, root) = scratch("discovery");
    let (mut client, initialized) = Client::start_with(&root, true, "2025-06-18");
    assert_eq!(initialized["capabilities"]["resources"]["subscribe"], true);
    let install = uri(&root.join(INSTALL));
    let subscribed = client.request("resources/subscribe", json!({ "uri": install }));
    assert!(subscribed.get("error").is_none(), "{subscribed}");

    // Another resource changing on disk is not this subscriber's business.
    std::fs::write(root.join(GUIDE), "= Guide\n\nChanged on disk.\n").unwrap();
    assert_eq!(
        uris(&client.notifications(UPDATED, SETTLE)),
        Vec::<String>::new()
    );

    // An edit only in the shared buffers is announced to nobody.
    client
        .call(
            "edit_page",
            json!({ "path": INSTALL, "edits": [{ "old_text": "You need things.", "new_text": "You need a licence." }] }),
        )
        .unwrap();
    assert_eq!(
        uris(&client.notifications(UPDATED, SETTLE)),
        Vec::<String>::new()
    );

    // Saved, the change is on disk: announced, once.
    client
        .call(
            "edit_page",
            json!({ "path": INSTALL, "edits": [], "save": true }),
        )
        .unwrap();
    assert_eq!(
        uris(&client.notifications(UPDATED, SETTLE)),
        std::slice::from_ref(&install)
    );

    // Written on disk by anything else: announced again.
    std::fs::write(root.join(INSTALL), "= Installing\n\nRewritten elsewhere.\n").unwrap();
    assert_eq!(
        uris(&client.notifications(UPDATED, SETTLE)),
        std::slice::from_ref(&install)
    );

    // Unsubscribed, nothing more.
    let unsubscribed = client.request("resources/unsubscribe", json!({ "uri": install }));
    assert!(unsubscribed.get("error").is_none(), "{unsubscribed}");
    std::fs::write(root.join(INSTALL), "= Installing\n\nOnce more.\n").unwrap();
    assert_eq!(
        uris(&client.notifications(UPDATED, SETTLE)),
        Vec::<String>::new()
    );

    // A resource outside the roots cannot be subscribed to.
    let outside = client.request(
        "resources/subscribe",
        json!({ "uri": "file:///etc/passwd" }),
    );
    assert!(outside.get("error").is_some(), "{outside}");
    client.stop();
}
