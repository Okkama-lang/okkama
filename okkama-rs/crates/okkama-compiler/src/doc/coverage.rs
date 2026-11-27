//! Public declaration coverage and the incremental std gate (#514).
//! The denominator comes from the shared public model, including overloads,
//! enum cases and cap fields; module overviews do not count as declarations.

use super::{DocText, ModuleDoc};
use crate::source::Span;

pub const COMPLETED_WAVES: &str = include_str!("completed-waves.txt");

pub fn completed_wave(module: &str) -> bool {
    COMPLETED_WAVES.lines().any(|line| !line.starts_with('#') && line == module)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingDoc { pub name: String, pub span: Span }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    pub total: usize,
    pub documented: usize,
    pub missing: Vec<MissingDoc>,
}

impl ModuleDoc {
    pub fn coverage(&self) -> Coverage {
        let mut coverage = Coverage::default();
        let mut count = |name: String, span: Span, doc: Option<&DocText>| {
            coverage.total += 1;
            if doc.is_some_and(|doc| !doc.text.trim().is_empty()) { coverage.documented += 1; }
            else { coverage.missing.push(MissingDoc { name, span }); }
        };
        for item in &self.items {
            count(item.name.clone(), item.span, item.doc.as_ref());
            for member in &item.members {
                count(format!("{}.{}", item.name, member.name), member.span, member.doc.as_ref());
            }
        }
        coverage
    }
}

/// A user module cannot opt into std errors merely by declaring a std name.
/// File identity is the loader's (#1116): the compiler itself never asks the file system.
pub fn enforced(doc: &ModuleDoc, loader: &mut dyn crate::lowering::ModuleLoader) -> bool {
    enforced_file(&doc.file, loader)
}

pub fn enforced_file(file: &str, loader: &mut dyn crate::lowering::ModuleLoader) -> bool {
    let identity = loader.identity(file);
    COMPLETED_WAVES.lines().filter(|line| !line.is_empty() && !line.starts_with('#')).any(|module| {
        let Ok(Some(path)) = loader.resolve_std(module, file) else { return false; };
        loader.identity(&path) == identity
    })
}
