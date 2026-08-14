#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServerCapabilities {
    pub full_document_sync: bool,
    pub document_symbols: bool,
    pub definition: bool,
    pub diagnostics: bool,
}

#[must_use]
pub const fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        full_document_sync: true,
        document_symbols: true,
        definition: true,
        diagnostics: true,
    }
}
