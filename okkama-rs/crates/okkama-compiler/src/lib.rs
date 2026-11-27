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
//! * [`commands`] — what the read-only commands report (check, fmt, tokens, ast, ir, build), as values;
//! * [`host`] — the clock and development outputs, the only calls to the machine (none on wasm32, #1116);
//! * [`jit`] — an empty boundary: machine code is Sparrow's business.
//!
//! The library never reads command-line arguments and never terminates the process; it prints only
//! development diagnostics requested by `OKKAMA_PROFILE_REGISTERS` and `OKKAMA_DEV`,
//! and a one-time warning for unknown passes in `OKKAMA_IR_PASSES` or a selected pass list. Callers receive values and diagnostics and decide
//! how to present them (see the `okkama-cli` crate).

// The one switch policy of every crate (#1095), under its old path.
#[doc(hidden)]
pub use okkama_devtrace::env_switch;

pub mod analysis;
pub mod codegen;
pub mod commands;
pub mod diagnostics;
pub mod doc;
pub mod host;
pub mod ide;
pub mod ir;
pub mod jit;
pub mod lowering;
pub mod source;
pub mod syntax;

pub use diagnostics::{Diagnostic, DiagnosticKind};
pub use source::{LineColumn, SourceFile, Span};


#[cfg(test)]
extern crate self as okkama_compiler;
#[cfg(test)]
#[path = "../../../fuzz/fuzz_targets/support/lowering.rs"]
mod fuzz_lowering_policy;
