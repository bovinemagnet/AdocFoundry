use lsp_types::{
    CodeActionProviderCapability, ExecuteCommandOptions, OneOf, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
};

use crate::handlers::code_actions::RENDER_PREVIEW_COMMAND;

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
                ..TextDocumentSyncOptions::default()
            },
        )),
        document_symbol_provider: Some(OneOf::Left(true)),
        definition_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: vec![RENDER_PREVIEW_COMMAND.to_owned()],
            ..ExecuteCommandOptions::default()
        }),
        ..ServerCapabilities::default()
    }
}
