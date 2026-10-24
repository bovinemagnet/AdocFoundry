//! The editable document model behind the visual editor.
//!
//! The page text stays the source of truth. A [`Model`] describes it as typed blocks whose text
//! is read losslessly into pieces, each knowing the exact source bytes it came from; a step on
//! the model produces the smallest source edits that make it, so text the model does not touch
//! is never rewritten.

mod block;
mod build;
mod inline;
mod mapping;
mod scan;
mod serialise;
mod source_marks;
mod step;
mod table;
mod text;
mod tree;
mod words;

pub use block::{BlockType, NewBlock};
pub use mapping::{apply_edits, invert, Assoc, Mapping, SourceEdit};
pub use source_marks::{toggle_mark_in_source, SourceToggle};
pub use step::{Step, StepError, StepResult};
pub use text::Segment;
pub use tree::*;
pub use words::{next_word_end, previous_word_start, word_at};
