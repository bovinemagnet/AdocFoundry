//! Read-only exports, READONLY 3/5 (`docs/prd/PRD-AdocFoundry_READONLY.md` §10, test 14):
//! Word's enforced document protection in the settings, in the schema's order, and nothing else
//! of the package changed.

mod common;

use adoc_docx::{DocxOptions, WordProtection};
use common::{assert_valid_package, text};

const PAGE: &str = "= Approved Procedure\n\nThe text.[^1]\n\n[^1]: A note.\n\n. One\n. Two\n";

fn written(protection: WordProtection) -> (std::collections::BTreeMap<String, Vec<u8>>, bool) {
    let options = DocxOptions {
        protection,
        ..DocxOptions::default()
    };
    let mut bytes = Vec::new();
    let report = adoc_docx::write_docx(&common::article(PAGE), &options, &mut bytes).unwrap();
    (assert_valid_package(&bytes), report.protected)
}

#[test]
fn docx_readonly_settings_enforced() {
    let (protected, reported) = written(WordProtection::ReadOnly);
    assert!(reported, "the report says the document is protected");
    let settings = text(&protected, "word/settings.xml");
    assert_eq!(
        settings.matches("<w:documentProtection ").count(),
        1,
        "{settings}"
    );
    assert!(
        settings.contains(r#"<w:documentProtection w:edit="readOnly" w:enforcement="1"/>"#),
        "{settings}"
    );
    // No password, and nothing that only asks.
    for absent in [
        "w:hash",
        "w:salt",
        "w:cryptProviderType",
        "writeProtection",
        "readOnlyRecommended",
    ] {
        assert!(!settings.contains(absent), "{absent}: {settings}");
    }

    // Unprotected, the settings and every other part are as they always were; protected, only
    // the settings differ, by the one element.
    let (plain, reported) = written(WordProtection::None);
    assert!(!reported);
    let plain_settings = text(&plain, "word/settings.xml");
    assert!(!plain_settings.contains("documentProtection"));
    assert_eq!(
        settings.replace(
            r#"<w:documentProtection w:edit="readOnly" w:enforcement="1"/>"#,
            ""
        ),
        plain_settings
    );
    assert_eq!(
        protected.keys().collect::<Vec<_>>(),
        plain.keys().collect::<Vec<_>>()
    );
    for (name, data) in &protected {
        if name != "word/settings.xml" {
            assert!(plain[name] == *data, "{name} differs");
        }
    }
}
