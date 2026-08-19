/// Renders the document once.
pub const RENDER_PREVIEW_COMMAND: &str = "adoc.renderPreview";

/// Renders the document and keeps the artefact up to date as the document is saved.
pub const RENDER_LIVE_PREVIEW_COMMAND: &str = "adoc.renderLivePreview";

/// Stops following the document, leaving the artefact as it stands.
pub const STOP_LIVE_PREVIEW_COMMAND: &str = "adoc.stopLivePreview";

/// Whether a document is already being followed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LivePreview {
    Active,
    Inactive,
}

/// An editor-agnostic code action. `protocol.rs` maps this onto `lsp_types::CodeAction`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewAction {
    pub title: String,
    pub command: &'static str,
    pub uri: String,
}

#[must_use]
pub fn code_actions_for(uri: &str, live: LivePreview) -> Vec<PreviewAction> {
    // A one-off render stays available either way: a frozen snapshot is still useful
    // while the document is being followed.
    let (title, command) = match live {
        LivePreview::Inactive => ("AsciiDoc: render live preview", RENDER_LIVE_PREVIEW_COMMAND),
        LivePreview::Active => ("AsciiDoc: stop live preview", STOP_LIVE_PREVIEW_COMMAND),
    };

    vec![
        PreviewAction {
            title: "AsciiDoc: render preview".to_owned(),
            command: RENDER_PREVIEW_COMMAND,
            uri: uri.to_owned(),
        },
        PreviewAction {
            title: title.to_owned(),
            command,
            uri: uri.to_owned(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_both_a_one_off_and_a_live_preview() {
        let actions = code_actions_for("file:///docs/guide.adoc", LivePreview::Inactive);

        let commands: Vec<_> = actions.iter().map(|action| action.command).collect();
        assert_eq!(
            commands,
            vec![RENDER_PREVIEW_COMMAND, RENDER_LIVE_PREVIEW_COMMAND]
        );
        assert!(actions.iter().all(|a| a.uri == "file:///docs/guide.adoc"));
    }

    #[test]
    fn the_two_actions_are_distinguishable_in_a_menu() {
        let actions = code_actions_for("file:///docs/guide.adoc", LivePreview::Inactive);

        assert_ne!(actions[0].title, actions[1].title);
        assert!(actions[1].title.contains("live"), "{}", actions[1].title);
    }

    /// Offering "start" on a document already following the buffer would be noise; the
    /// useful action there is the one that stops it.
    #[test]
    fn a_live_document_is_offered_a_way_to_stop() {
        let actions = code_actions_for("file:///docs/guide.adoc", LivePreview::Active);

        let commands: Vec<_> = actions.iter().map(|action| action.command).collect();
        assert_eq!(
            commands,
            vec![RENDER_PREVIEW_COMMAND, STOP_LIVE_PREVIEW_COMMAND]
        );
    }

    #[test]
    fn a_one_off_render_stays_available_while_live() {
        let actions = code_actions_for("file:///docs/guide.adoc", LivePreview::Active);

        assert_eq!(actions[0].command, RENDER_PREVIEW_COMMAND);
    }

    #[test]
    fn preview_action_title_names_the_operation() {
        let actions = code_actions_for("file:///docs/guide.adoc", LivePreview::Inactive);

        assert!(actions[0].title.contains("preview"), "{}", actions[0].title);
    }
}
