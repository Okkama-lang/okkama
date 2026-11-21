//! Publishing from the canonical public doc model (#515): man(7), standalone
//! static HTML and JSON schema v1. No source parsing or execution happens here.

use std::collections::HashSet;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde_json::{Value, json};
use super::{DocText, ItemDoc, ItemKind, MemberKind, ModuleDoc, declaration_target};
use crate::source::Span;

pub const JSON_SCHEMA: &str = include_str!("schema-v1.json");
pub const SCHEMA_VERSION: u32 = 1;

/// Flat names are unambiguous for Okkama's slash-separated identifier paths.
pub fn page_name(path: &str) -> String { path.replace('/', "-") }

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
        .replace('"', "&quot;").replace('\'', "&#39;")
}

fn roff_escape(text: &str) -> String {
    // Prefix every source line, even inside literal blocks: no source text
    // can become a roff request or escape (including .so and \\!).
    format!("\\&{}", text.replace('\\', "\\e")
        .replace('"', "\\(dq").replace('\n', "\n\\&"))
}

fn html_start(title: &str) -> String {
    format!("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{}</title><style>body{{max-width:72rem;margin:2rem auto;padding:0 1rem;font:1rem/1.5 system-ui}}pre{{overflow:auto;padding:1rem;background:#eee;color:#111}}code{{font-family:monospace}}table{{border-collapse:collapse}}th,td{{border:1px solid #888;padding:.3rem}}a{{overflow-wrap:anywhere}}</style></head><body>\n<nav><a href=\"index.html\">Okkama standard library</a></nav>\n<main>\n", html_escape(title))
}

/// Relative HTML location of a stable Phase B target; its canonical identity
/// is also retained in data-okkama-target. No source paths become filenames.
fn html_target(target: &str) -> String {
    let Some(rest) = target.strip_prefix("okkama-doc:/") else { return target.into(); };
    let (identity, anchor) = rest.split_once('#').unwrap_or((rest, ""));
    let path = identity.split('?').next().unwrap_or(identity);
    format!("{}.html#{anchor}", page_name(path))
}

fn safe_url(url: &str) -> bool {
    let url = url.trim();
    if url.chars().any(char::is_control) { return false; }
    match url.split_once(':') {
        Some((scheme, _)) => ["http", "https", "mailto", "okkama-doc"].iter().any(|s| scheme.eq_ignore_ascii_case(s)),
        None => true,
    }
}

fn markdown_html(text: &str) -> String {
    let mut out = String::new();
    let mut table_head = false;
    let mut links = Vec::new();
    for event in Parser::new_ext(text, Options::ENABLE_TABLES) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => out.push_str("<p>"),
                Tag::Heading { level, .. } => out.push_str(&format!("<{level}>")),
                Tag::CodeBlock(_) => out.push_str("<pre><code>"),
                Tag::List(Some(start)) => out.push_str(&format!("<ol start=\"{start}\">")),
                Tag::List(None) => out.push_str("<ul>"),
                Tag::Item => out.push_str("<li>"),
                Tag::Emphasis => out.push_str("<em>"),
                Tag::Strong => out.push_str("<strong>"),
                Tag::BlockQuote(_) => out.push_str("<blockquote>"),
                Tag::Link { dest_url, .. } => {
                    let safe = safe_url(&dest_url);
                    links.push(safe);
                    if safe { out.push_str(&format!("<a href=\"{}\" data-okkama-target=\"{}\">", html_escape(&html_target(&dest_url)), html_escape(&dest_url))); }
                }
                Tag::Table(_) => out.push_str("<table>"),
                Tag::TableHead => { table_head = true; out.push_str("<thead><tr>"); }
                Tag::TableRow => out.push_str("<tr>"),
                Tag::TableCell => out.push_str(if table_head { "<th>" } else { "<td>" }),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => out.push_str("</p>\n"),
                TagEnd::Heading(level) => out.push_str(&format!("</{level}>\n")),
                TagEnd::CodeBlock => out.push_str("</code></pre>\n"),
                TagEnd::List(true) => out.push_str("</ol>\n"),
                TagEnd::List(false) => out.push_str("</ul>\n"),
                TagEnd::Item => out.push_str("</li>\n"),
                TagEnd::Emphasis => out.push_str("</em>"),
                TagEnd::Strong => out.push_str("</strong>"),
                TagEnd::BlockQuote(_) => out.push_str("</blockquote>\n"),
                TagEnd::Link => { if links.pop() == Some(true) { out.push_str("</a>"); } }
                TagEnd::Table => out.push_str("</table>\n"),
                TagEnd::TableHead => { table_head = false; out.push_str("</tr></thead>\n"); }
                TagEnd::TableRow => out.push_str("</tr>\n"),
                TagEnd::TableCell => out.push_str(if table_head { "</th>" } else { "</td>" }),
                _ => {}
            },
            Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => out.push_str(&html_escape(&text)),
            Event::Code(text) => out.push_str(&format!("<code>{}</code>", html_escape(&text))),
            Event::SoftBreak => out.push('\n'),
            Event::HardBreak => out.push_str("<br>\n"),
            Event::Rule => out.push_str("<hr>\n"),
            _ => {}
        }
    }
    out
}

fn markdown_man(text: &str) -> String {
    let mut out = String::new();
    let mut lists = Vec::new();
    for event in Parser::new_ext(text, Options::ENABLE_TABLES) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => out.push_str("\n.PP\n"),
                Tag::Heading { .. } => out.push_str("\n.SS\n"),
                Tag::CodeBlock(_) => out.push_str("\n.PP\n.nf\n"),
                Tag::List(number) => { lists.push(number); out.push_str("\n.RS\n"); }
                Tag::Item => {
                    let marker = match lists.last_mut() {
                        Some(Some(number)) => { let marker = format!("{number}."); *number += 1; marker }
                        _ => "\\(bu".into(),
                    };
                    out.push_str(&format!("\n.IP \"{marker}\" 4\n"));
                }
                Tag::Link { dest_url, .. } => out.push_str(&format!("\n.UR {}\n", roff_escape(&dest_url))),
                Tag::TableHead | Tag::TableRow => out.push_str("\n.PP\n"),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::CodeBlock => out.push_str("\n.fi\n"),
                TagEnd::List(_) => { lists.pop(); out.push_str("\n.RE\n"); }
                TagEnd::Link => out.push_str("\n.UE\n"),
                TagEnd::TableCell => out.push_str(" | "),
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item => out.push('\n'),
                _ => {}
            },
            Event::Text(text) | Event::Code(text) | Event::Html(text) | Event::InlineHtml(text) => out.push_str(&roff_escape(&text)),
            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push_str("\n.br\n"),
            Event::Rule => out.push_str("\n.PP\n\\&----\n"),
            _ => {}
        }
    }
    out.push('\n');
    out
}

struct Page { out: String, html: bool, anchors: HashSet<String> }
impl Page {
    fn heading(&mut self, level: usize, title: &str) {
        if self.html { self.out.push_str(&format!("<h{level}>{}</h{level}>\n", html_escape(title))); }
        else { self.out.push_str(&format!("\n.{} \"{}\"\n", if level == 2 { "SH" } else { "SS" }, roff_escape(title).trim_start_matches("\\&"))); }
    }
    fn markdown(&mut self, text: &str) {
        self.out.push_str(&if self.html { markdown_html(text) } else { markdown_man(text) });
    }
    fn code(&mut self, code: &str) {
        if self.html { self.out.push_str(&format!("<pre><code>{}</code></pre>\n", html_escape(code))); }
        else { self.out.push_str(&format!("\n.PP\n.nf\n{}\n.fi\n", roff_escape(code))); }
    }
    fn anchor(&mut self, doc: &ModuleDoc, name: &str) {
        if self.html && self.anchors.insert(name.into()) {
            self.out.push_str(&format!("<a id=\"{}\" data-okkama-target=\"{}\"></a>\n", html_escape(name), html_escape(&declaration_target(doc, name))));
        }
    }
    fn item(&mut self, doc: &ModuleDoc, item: &ItemDoc) {
        self.anchor(doc, &item.name);
        self.heading(3, &format!("{} {}", item.kind.word(), item.name));
        self.code(&item.signature);
        if let Some(text) = &item.doc { self.markdown(&text.markdown()); }
        for member in &item.members {
            let name = format!("{}.{}", item.name, member.name);
            self.anchor(doc, &name);
            self.heading(4, &name);
            self.code(&member.signature);
            if let Some(text) = &member.doc { self.markdown(&text.markdown()); }
        }
    }
}

fn module_page(doc: &ModuleDoc, html: bool) -> String {
    let mut page = Page { out: if html { html_start(&doc.path) } else {
        format!(".TH \"{}\" \"3ok\" \"\" \"Okkama\" \"Library reference\"\n", page_name(&doc.path))
    }, html, anchors: HashSet::new() };
    page.heading(2, "NAME");
    let summary = doc.overview.as_ref().map(|text| super::summary(&text.markdown())).unwrap_or_default();
    page.markdown(&format!("{}{}", doc.path, if summary.is_empty() { String::new() } else { format!(" - {summary}") }));
    if let Some(version) = &doc.version { page.heading(2, "VERSION"); page.markdown(version); }
    let parts = super::render::overview(doc);
    if let Some(synopsis) = parts.synopsis { page.heading(2, "SYNOPSIS"); page.code(&synopsis); }
    if !parts.description.trim().is_empty() { page.heading(2, "DESCRIPTION"); page.markdown(&parts.description); }
    if doc.items.iter().any(|item| item.kind != ItemKind::Error) {
        page.heading(2, "PUBLIC API");
        for item in doc.items.iter().filter(|item| item.kind != ItemKind::Error) { page.item(doc, item); }
    }
    if !parts.errors.trim().is_empty() || doc.items.iter().any(|item| item.kind == ItemKind::Error) {
        page.heading(2, "ERRORS");
        if !parts.errors.trim().is_empty() { page.markdown(&parts.errors); }
        for item in doc.items.iter().filter(|item| item.kind == ItemKind::Error) { page.item(doc, item); }
    }
    if !parts.engines.trim().is_empty() { page.heading(2, "ENGINE NOTES"); page.markdown(&parts.engines); }
    if !parts.see_also.trim().is_empty() { page.heading(2, "SEE ALSO"); page.markdown(&parts.see_also); }
    if html { page.out.push_str("</main></body></html>\n"); }
    page.out
}

pub fn module_man(doc: &ModuleDoc) -> String { module_page(doc, false) }
pub fn module_html(doc: &ModuleDoc) -> String { module_page(doc, true) }

pub fn index_html(entries: &[super::render::IndexEntry]) -> String {
    let mut out = html_start("Okkama standard library");
    out.push_str("<h1>Okkama standard library</h1>\n<ul>\n");
    for entry in entries { out.push_str(&format!("<li><a href=\"{}.html\">{}</a> — {}</li>\n", html_escape(&page_name(&entry.path)), html_escape(&entry.path), html_escape(&entry.summary))); }
    out.push_str("</ul></main></body></html>\n"); out
}

pub fn index_man(entries: &[super::render::IndexEntry]) -> String {
    format!(".TH \"okkama-std\" \"3ok\" \"\" \"Okkama\" \"Library reference\"\n{}", markdown_man(&super::render::index_markdown(entries)))
}

fn span(span: Span) -> Value { json!({ "start": span.start, "end": span.end }) }
fn text(text: &Option<DocText>) -> Value {
    match text {
        None => Value::Null,
        Some(text) => json!({ "text": text.text, "markdown": text.markdown(), "span": span(text.span),
            "links": text.links.iter().map(|link| json!({ "name": link.name, "range": { "start": link.range.start, "end": link.range.end },
                "span": span(link.span), "target": link.target,
                "declarations": link.declarations.iter().map(|decl| json!({ "file": decl.file, "span": span(decl.span) })).collect::<Vec<_>>() })).collect::<Vec<_>>() }),
    }
}
fn module_json(doc: &ModuleDoc) -> Value {
    json!({ "path": doc.path, "version": doc.version, "file": doc.file, "target": declaration_target(doc, ""), "overview": text(&doc.overview),
        "items": doc.items.iter().map(|item| json!({ "name": item.name, "kind": match item.kind {
            ItemKind::Object => "object", ItemKind::Role => "role", ItemKind::Enum => "enum", ItemKind::Error => "error", ItemKind::Const => "const",
            ItemKind::Native => "native", ItemKind::NativeObject => "native-object", ItemKind::Cap => "cap" },
            "signature": item.signature, "target": declaration_target(doc, &item.name), "span": span(item.span), "doc": text(&item.doc),
            "members": item.members.iter().map(|member| json!({ "name": member.name, "kind": match member.kind {
                MemberKind::Constructor => "constructor", MemberKind::Action => "action", MemberKind::Signature => "signature",
                MemberKind::Cap => "cap", MemberKind::Case => "case", MemberKind::Field => "field" },
                "signature": member.signature, "target": declaration_target(doc, &format!("{}.{}", item.name, member.name)),
                "span": span(member.span), "doc": text(&member.doc) })).collect::<Vec<_>>() })).collect::<Vec<_>>(),
        "notes": doc.notes.iter().map(|note| json!({ "span": span(note.span), "message": note.message })).collect::<Vec<_>>(),
        "link_warnings": doc.link_warnings.iter().map(|warning| json!({ "name": warning.name, "span": span(warning.span) })).collect::<Vec<_>>() })
}

/// Schema v1 is independent of Rust layout. Source spans are UTF-8 byte
/// offsets, end-exclusive; overloads remain separate records with one target.
pub fn json(modules: &[ModuleDoc]) -> String {
    serde_json::to_string_pretty(&json!({ "schema_version": SCHEMA_VERSION,
        "modules": modules.iter().map(module_json).collect::<Vec<_>>() })).expect("JSON values serialize") + "\n"
}
