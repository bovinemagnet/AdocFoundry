use lsp_types::{
    CodeActionProviderCapability, CompletionOptions, ExecuteCommandOptions, OneOf,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions,
};

use crate::handlers::code_actions::{
    RENDER_LIVE_PREVIEW_COMMAND, RENDER_PREVIEW_COMMAND, STOP_LIVE_PREVIEW_COMMAND,
};

#[must_use]
pub(crate) fn server_capabilities(
    encoding: crate::position::PositionEncoding,
) -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(encoding.lsp_kind()),
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                // A live preview re-renders on save, which needs the notification.
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                ..TextDocumentSyncOptions::default()
            },
        )),
        document_symbol_provider: Some(OneOf::Left(true)),
        definition_provider: Some(OneOf::Left(true)),
        // The trigger set is only a hint about when to ask. `completion_context` decides
        // whether there is anything to offer, so `:` firing on an attribute line is fine.
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![
                ":".to_owned(),
                "$".to_owned(),
                "#".to_owned(),
                "/".to_owned(),
                "<".to_owned(),
            ]),
            resolve_provider: Some(false),
            ..CompletionOptions::default()
        }),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        execute_command_provider: Some(ExecuteCommandOptions {
            // A client may drop a code action whose command is missing from this list —
            // Zed does — so every command a code action carries belongs here.
            commands: vec![
                RENDER_PREVIEW_COMMAND.to_owned(),
                RENDER_LIVE_PREVIEW_COMMAND.to_owned(),
                STOP_LIVE_PREVIEW_COMMAND.to_owned(),
            ],
            ..ExecuteCommandOptions::default()
        }),
        ..ServerCapabilities::default()
    }
}

#[cfg(test)]
mod tests {
    use super::server_capabilities;
    use crate::{
        handlers::code_actions::{code_actions_for, LivePreview},
        position::PositionEncoding,
    };

    /// A client may silently drop a code action whose command is not advertised — Zed
    /// does — so the two lists have to stay in step. Nothing is logged when they do not.
    #[test]
    fn every_code_action_command_is_advertised() {
        let capabilities = server_capabilities(PositionEncoding::Utf16);
        let advertised = capabilities
            .execute_command_provider
            .expect("execute command provider")
            .commands;

        for live in [LivePreview::Inactive, LivePreview::Active] {
            for action in code_actions_for("file:///docs/guide.adoc", live) {
                assert!(
                    advertised.iter().any(|command| command == action.command),
                    "`{}` is offered as a code action but not advertised: {advertised:?}",
                    action.command
                );
            }
        }
    }

    #[test]
    fn advertises_completion_with_its_trigger_characters() {
        let capabilities = server_capabilities(PositionEncoding::Utf16);
        let completion = capabilities
            .completion_provider
            .expect("completion provider");

        let triggers = completion.trigger_characters.expect("trigger characters");
        for expected in [":", "$", "#", "/", "<"] {
            assert!(
                triggers.iter().any(|trigger| trigger == expected),
                "`{expected}` must trigger completion: {triggers:?}"
            );
        }
        assert_eq!(completion.resolve_provider, Some(false));
    }
}
