//! Symbol links, documentation lookup and search (#513), shared by man and IDE.

use std::collections::HashMap;
use std::ops::Range;

use pulldown_cmark::{BrokenLink, LinkType, Options, Parser};

use super::{DocText, ModuleDoc, summary};
use crate::analysis::registry::{Registry, SourceModule, TypeRef};
use crate::lowering::{LoadedModule, ModuleLoader};
use crate::source::{SourceFile, Span};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkDeclaration {
    pub file: String,
    pub span: Span,
}

/// A shortcut reference to a public declaration (all overloads share a target).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclarationLink {
    pub name: String,
    pub range: Range<usize>,
    pub span: Span,
    pub target: Option<String>,
    pub declarations: Vec<LinkDeclaration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkWarning {
    pub name: String,
    pub span: Span,
}

impl DocText {
    /// CommonMark with explicitly resolved symbol links; code and ordinary
    /// Markdown links remain byte-for-byte as written.
    pub fn markdown(&self) -> String {
        let mut text = self.text.clone();
        for link in self.links.iter().rev() {
            if let Some(target) = &link.target {
                text.insert_str(link.range.end, &format!("({target})"));
            }
        }
        text
    }
}

/// Stable, source-position-independent reference, suitable for any renderer.
pub fn target(module: &ModuleDoc, name: &str) -> String {
    fn escape(text: &str) -> String {
        let mut result = String::new();
        for byte in text.bytes() {
            if byte.is_ascii_alphanumeric() || b"/_-.~".contains(&byte) {
                result.push(byte as char);
            } else {
                result.push_str(&format!("%{byte:02X}"));
            }
        }
        result
    }
    let version = module.version.as_ref().map(|v| format!("?version={}", escape(v))).unwrap_or_default();
    format!("okkama-doc:/{}{version}#{}", escape(&module.path), escape(name))
}

/// Source documentation for every file in the compiler's module graph.
#[derive(Debug)]
pub struct Documentation {
    pub modules: Vec<ModuleDoc>,
}

impl Documentation {
    pub fn new(registry: &Registry<'_>, files: &[SourceFile], loader: &mut dyn ModuleLoader) -> Self {
        let mut modules: Vec<_> = registry.modules.iter().zip(files)
            .map(|(module, file)| ModuleDoc::of(file, module.program)).collect();
        let mut external: HashMap<(String, String), Option<ModuleDoc>> = HashMap::new();
        for module in 0..modules.len() {
            // Resolution reads the immutable public inventory, never partially
            // rewritten Markdown from another declaration.
            let mut doc = modules[module].clone();
            let mut warnings = Vec::new();
            let mut resolve = |text: &mut DocText| {
                for (name, range) in shortcuts(&text.text) {
                    let span = Span::new(text.offsets[range.start], text.offsets[range.end - 1] + 1);
                    let found = resolve_link(registry, &modules, module, &name, loader, &mut external);
                    let (target, declarations) = found.map_or((None, Vec::new()), |(target, spans)| (Some(target), spans));
                    if target.is_none() {
                        warnings.push(LinkWarning { name: name.clone(), span });
                    }
                    text.links.push(DeclarationLink { name, range, span, target, declarations });
                }
            };
            if let Some(overview) = &mut doc.overview { resolve(overview); }
            for item in &mut doc.items {
                if let Some(text) = &mut item.doc { resolve(text); }
                for member in &mut item.members {
                    if let Some(text) = &mut member.doc { resolve(text); }
                }
            }
            warnings.sort_by_key(|warning| warning.span.start);
            // A named enum-case comment is also part of its enum's prose.
            // Both views refer to the same source link, hence one diagnostic.
            warnings.dedup_by(|a, b| a.span == b.span && a.name == b.name);
            doc.link_warnings = warnings;
            modules[module] = doc;
        }
        Documentation { modules }
    }

    /// The exact declaration selected by analysis, including its overload.
    pub fn hover(&self, module: usize, span: Span) -> Option<String> {
        let doc = self.modules.get(module)?;
        for item in &doc.items {
            if item.span.start == span.start {
                return Some(super::render::hover(doc, item, None));
            }
            if let Some(member) = item.members.iter().find(|member| member.span.start == span.start) {
                return Some(super::render::hover(doc, item, Some(member)));
            }
        }
        None
    }
}

/// Reads a module and its imports with the compiler's resolver and symbol
/// registry. No bodies are checked, lowered or executed.
pub fn document(source: &SourceFile, loader: &mut dyn ModuleLoader) -> Result<ModuleDoc, String> {
    document_graph(source, loader)?.modules.pop().ok_or_else(|| "no module to document".into())
}

/// The same read-only extraction for a root and its complete import graph.
/// Batch publishers can import their catalogue once instead of rebuilding
/// every dependency graph for each output page. The root is the last module.
pub fn document_graph(source: &SourceFile, loader: &mut dyn ModuleLoader) -> Result<Documentation, String> {
    crate::lowering::document(source, loader)
}

fn identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn shortcuts(text: &str) -> Vec<(String, Range<usize>)> {
    let mut links = Vec::new();
    let mut callback = |broken: BrokenLink<'_>| {
        let name = broken.reference.as_ref();
        if broken.link_type == LinkType::Shortcut
            && name.split(['/', '.']).all(identifier)
            && text.get(broken.span.clone()) == Some(&format!("[{name}]"))
        {
            links.push((name.to_string(), broken.span));
        }
        None
    };
    // The Markdown parser invokes this only outside code, images and ordinary
    // inline/reference links. Returning None preserves unresolved source text.
    for _ in Parser::new_with_broken_link_callback(text, Options::ENABLE_TABLES, Some(&mut callback)) {}
    links.sort_by_key(|(_, range)| range.start);
    links.dedup();
    links
}

fn type_owner(registry: &Registry<'_>, ty: TypeRef) -> usize {
    match ty {
        TypeRef::Object(id) => registry.objects[id.0].module,
        TypeRef::Role(id) => registry.roles[id.0].module,
        TypeRef::Enum(id) => registry.enums[id.0].module,
        TypeRef::Handle(id) => registry.handles[id.0].module,
    }
}

fn owner(registry: &Registry<'_>, module: usize, name: &str) -> Option<usize> {
    registry.find_type(module, name).map(|ty| type_owner(registry, ty))
        .or_else(|| registry.find_native(module, name).map(|id| registry.natives[id].module))
        .or_else(|| registry.find_const(module, name).map(|id| registry.consts[id].module))
}

fn public_target(docs: &[&ModuleDoc], name: &str) -> Option<(String, Vec<LinkDeclaration>)> {
    let (item, member) = name.split_once('.').map_or((name, None), |(item, member)| (item, Some(member)));
    let mut declarations = Vec::new();
    let mut uri = None;
    for doc in docs {
        let Some(item) = doc.item(item) else { continue; };
        let spans = match member {
            Some(name) => item.members_named(name).iter().map(|member| member.span).collect(),
            None => vec![item.span],
        };
        for span in spans {
            declarations.push(LinkDeclaration { file: doc.file.clone(), span });
            uri = Some(target(doc, name));
        }
    }
    uri.map(|uri| (uri, declarations))
}

fn resolve_link(
    registry: &Registry<'_>, docs: &[ModuleDoc], module: usize, name: &str,
    loader: &mut dyn ModuleLoader, external: &mut HashMap<(String, String), Option<ModuleDoc>>,
) -> Option<(String, Vec<LinkDeclaration>)> {
    let (head, tail) = name.split_once('.').map_or((name, None), |(head, tail)| (head, Some(tail)));
    let scope = &registry.modules[module];
    if !head.contains('/') {
        if let Some(namespace) = scope.namespaces.get(head) {
            let tail = tail?;
            let item = tail.split('.').next()?;
            if namespace.only.as_ref().is_some_and(|only| !only.iter().any(|name| name == item)) {
                return None;
            }
            return public_target(&[&docs[namespace.module]], tail);
        }
        if let Some(owner) = owner(registry, module, head) {
            let path = &docs[owner].path;
            let version = &docs[owner].version;
            let same_module: Vec<_> = docs.iter().filter(|doc| &doc.path == path && &doc.version == version).collect();
            return public_target(&same_module, name);
        }
        // Module-scope capabilities are in the doc inventory, not Names.
        if let Some(found) = public_target(&[&docs[module]], name) { return Some(found); }
    }
    let tail = tail?;
    // Lower-case module names (including single-segment names) and slash
    // paths are explicit module qualification. Never use a filesystem path.
    if !head.contains('/') && !head.starts_with(|ch: char| ch.is_ascii_lowercase() || ch == '_') {
        return None;
    }
    let (std_only, path) = head.strip_prefix("std/").map_or((false, head), |path| (true, path));
    let found: Vec<_> = docs.iter().filter(|doc| doc.path == path).collect();
    if !std_only && !found.is_empty() {
        return public_target(&found, tail);
    }
    let key = (docs[module].file.clone(), head.to_string());
    let other = external.entry(key).or_insert_with(|| {
        loader.check_import(path, &docs[module].file).ok()?;
        let loaded = if std_only { loader.load_std(path, &docs[module].file) } else { loader.load(path, &docs[module].file) }.ok()??;
        external_doc(loaded)
    }).as_ref()?;
    public_target(&[other], tail)
}

fn external_doc(loaded: LoadedModule) -> Option<ModuleDoc> {
    // Register names with the same compiler symbol model as an imported
    // module. Only its own public declarations can be explicit targets.
    let doc = ModuleDoc::of(&loaded.source, &loaded.program);
    let registry = Registry::build(vec![SourceModule { path: doc.path.clone(), program: &loaded.program, imports: Vec::new() }]);
    let names = &registry.modules[0].own;
    let mut doc = doc;
    doc.items.retain(|item| names.types.contains_key(&item.name) || names.natives.contains_key(&item.name)
        || names.consts.contains_key(&item.name) || item.kind == super::ItemKind::Cap);
    Some(doc)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchEntry {
    pub module: String,
    pub name: Option<String>,
    pub summary: String,
}

impl ModuleDoc {
    /// Literal, Unicode case-insensitive substring search over public names,
    /// module paths and first-sentence summaries, in source order.
    pub fn search(&self, query: &str) -> Vec<SearchEntry> {
        let query = query.to_lowercase();
        let mut entries = vec![SearchEntry { module: self.path.clone(), name: None, summary: self.summary() }];
        for item in &self.items {
            entries.push(SearchEntry { module: self.path.clone(), name: Some(item.name.clone()),
                summary: item.doc.as_ref().map(|doc| summary(&doc.text)).unwrap_or_default() });
            for member in &item.members {
                entries.push(SearchEntry { module: self.path.clone(), name: Some(format!("{}.{}", item.name, member.name)),
                    summary: member.doc.as_ref().map(|doc| summary(&doc.text)).unwrap_or_default() });
            }
        }
        entries.retain(|entry| entry.module.to_lowercase().contains(&query)
            || entry.name.as_ref().is_some_and(|name| name.to_lowercase().contains(&query))
            || entry.summary.to_lowercase().contains(&query));
        entries.dedup();
        entries
    }
}
