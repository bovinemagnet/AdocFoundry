mod asciidoctor;
mod formula;
mod html;
mod request;
mod result;
mod text;
mod view;

/// DOM value types the view tree shares, so renderers need not depend on the DOM crate.
pub use adoc_dom::{
    AdmonitionKind, Align, Callout, CellStyle, Column, ColumnWidth, ListKind, MediaKind, SourceSpan,
};
pub use asciidoctor::{MockRenderer, Renderer, SystemAsciidoctor};
pub use formula::{formula_image, with_baseline, FormulaImage, INLINE_FORMULA_POINTS};
pub use html::{embed_local_files, escape, inlines as inlines_html, to_html, HtmlOptions};
pub use request::{RenderRequest, RenderSafeMode};
pub use result::{RenderError, RenderOutput};
pub use text::to_text;
pub use view::{
    block_at, block_by_trail, build_view, inline_stem_attributes, source_location, stem_kind,
    DiagramView, LocalResolver, ResolvedLink, TocEntry, ViewBlock, ViewCallout, ViewCell,
    ViewDescription, ViewFootnote, ViewImage, ViewInline, ViewKind, ViewListItem, ViewResolver,
    ViewTable, ViewTree,
};
