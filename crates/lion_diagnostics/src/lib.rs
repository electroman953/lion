//! Source files, spans and diagnostics shared by every stage of the Lion toolchain.
//!
//! Every problem Lion reports — compile errors, warnings, interpreted-mode alerts,
//! runtime bugs and internal errors — is a [`Diagnostic`], rendered by [`render`]
//! with the file, line, column and an excerpt of the code.

mod diagnostic;
mod render;
mod source;

pub use diagnostic::{Diagnostic, Label, Severity};
pub use render::render;
pub use source::{SourceFile, SourceId, SourceMap, Span};
