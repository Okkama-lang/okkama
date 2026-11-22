//! Okkama compiler library: source text to verified Sparrow bytecode (SPB).
//!
//! * [`source`] — source files, byte spans and line/column mapping;
//! * [`diagnostics`] — structured lexical and syntax diagnostics;
//! * [`syntax`] — tokens, lexer, typed AST, parser, production pruning, AST printers and the formatter;
//! * [`analysis`] — names, types, overloads, successors and the rest of the language rules, producing THIR;
//! * [`lowering`] — THIR to OIR-2 (specialization, ownership regions, passes);
//! * [`ir`] — OIR-2: typed SSA modules, the text form and the verifier (stages: `docs/map/pipeline.md`);
//! * [`codegen`] — Sparrow bytecode generation;
//! * [`ide`] and [`doc`] — the language server's index and `okkama man`;
//! * [`jit`] — an empty boundary: machine code is Sparrow's business.
//!
//! The library never reads command-line arguments and never terminates the process; it prints only
//! development diagnostics a variable asks for (`OKKAMA_PROFILE_REGISTERS`, `OKKAMA_DEV`). Callers receive values and diagnostics and decide
//! how to present them (see the `okkama-cli` crate).

pub mod analysis;
pub mod codegen;
pub mod diagnostics;
pub mod doc;
pub mod ide;
pub mod ir;
pub mod jit;
pub mod lowering;
pub mod source;
pub mod syntax;

pub use diagnostics::{Diagnostic, DiagnosticKind};
pub use source::{LineColumn, SourceFile, Span};
