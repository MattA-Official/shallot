//! A WGSL compiler: lexer, parser, type checker and SPIR-V back end.
//!
//! [`compile`] takes WGSL source to a checked [`Module`], which reflects entry points and
//! resource bindings (what WebGPU's `layout: "auto"` needs) and lowers to SPIR-V for Vulkan with
//! [`Module::to_spirv`].
//!
//! The front end follows the WGSL specification's grammar and typing rules, including abstract
//! numeric literals and constant evaluation. The back end does not yet cover every builtin;
//! unsupported ones are reported as compile errors rather than miscompiled.

pub mod ast;
mod check;
mod consts;
pub mod ir;
pub mod lexer;
pub mod parser;
mod spirv;
pub mod types;

pub use check::check;
pub use ir::{Binding, EntryPoint, Module, ResourceKind, Stage};
pub use parser::parse;
pub use spirv::SpirvOptions;

use std::fmt;

/// A byte range in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// A compile error at a place in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    pub span: Span,
}

impl Error {
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }

    /// One-based line and column of the error's start in `source`.
    pub fn location(&self, source: &str) -> (usize, usize) {
        let before = &source[..self.span.start.min(source.len())];
        let line = before.matches('\n').count() + 1;
        let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
        (line, column)
    }

    /// The error with its line, column and the offending source line, for printing.
    pub fn render(&self, source: &str) -> String {
        let (line, column) = self.location(source);
        let text = source.lines().nth(line - 1).unwrap_or("");
        let width = (self.span.end.saturating_sub(self.span.start)).max(1);
        let width = width.min(text.len().saturating_sub(column - 1).max(1));
        format!(
            "{line}:{column}: {}\n{text}\n{}{}",
            self.message,
            " ".repeat(column - 1),
            "^".repeat(width)
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at byte {})", self.message, self.span.start)
    }
}

impl std::error::Error for Error {}

/// Parses and checks WGSL source.
pub fn compile(source: &str) -> Result<Module, Error> {
    check(&parse(source)?)
}
