/// Identifier of the command a preview code action asks the client to execute.
pub const RENDER_PREVIEW_COMMAND: &str = "adoc.renderPreview";

/// An editor-agnostic code action. `protocol.rs` maps this onto `lsp_types::CodeAction`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewAction {
    pub title: String,
    pub command: &'static str,
    pub uri: String,
}

#[must_use]
pub fn code_actions_for(uri: &str) -> Vec<PreviewAction> {
    vec![PreviewAction {
        title: "AsciiDoc: render preview".to_owned(),
        command: RENDER_PREVIEW_COMMAND,
        uri: uri.to_owned(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_the_preview_action_for_an_open_document() {
        let actions = code_actions_for("file:///docs/guide.adoc");

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].command, RENDER_PREVIEW_COMMAND);
        assert_eq!(actions[0].uri, "file:///docs/guide.adoc");
    }

    #[test]
    fn preview_action_title_names_the_operation() {
        let actions = code_actions_for("file:///docs/guide.adoc");

        assert!(actions[0].title.contains("preview"), "{}", actions[0].title);
    }
}
