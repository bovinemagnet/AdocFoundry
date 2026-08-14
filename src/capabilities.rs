use lsp_types::{
    OneOf, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions,
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
                ..TextDocumentSyncOptions::default()
            },
        )),
        document_symbol_provider: Some(OneOf::Left(true)),
        definition_provider: Some(OneOf::Left(true)),
        ..ServerCapabilities::default()
    }
}
