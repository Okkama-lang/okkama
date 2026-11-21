//! Documentation examples (#514): CommonMark blocks and original byte spans.
//! `okkama` / `okkama,compile` compile; `okkama,ignore` illustrates;
//! `okkama,run` requires an immediately following `output` fence (exact stdout).
//! Untagged, indented and other-language blocks illustrate, never execute.

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use super::{DocText, ModuleDoc};
use crate::source::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExampleKind { Illustrative, Compile, Run }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Example {
    pub kind: ExampleKind,
    pub code: String,
    pub expected: Option<String>,
    pub span: Span,
    offsets: Vec<u32>,
}

impl Example {
    /// Map a span in the extracted code to the original `//` comment.
    pub fn source_span(&self, span: Span) -> Span {
        let start = self.offsets.get(span.start as usize).copied().unwrap_or(self.span.end);
        let end = if span.is_empty() { start } else {
            self.offsets.get(span.end.saturating_sub(1) as usize).map_or(self.span.end, |at| at + 1)
        };
        Span::new(start, end)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExampleError { pub span: Span, pub message: String }

struct Block {
    info: String,
    code: String,
    offsets: Vec<u32>,
    range: std::ops::Range<usize>,
}

impl ModuleDoc {
    /// Every public documentation block, in source order. Invalid Okkama
    /// metadata is an error rather than an unreported skipped check.
    pub fn examples(&self) -> Result<Vec<Example>, ExampleError> {
        let mut texts = Vec::new();
        if let Some(text) = &self.overview { texts.push(text); }
        for item in &self.items {
            if let Some(text) = &item.doc { texts.push(text); }
            for member in &item.members {
                if let Some(text) = &member.doc { texts.push(text); }
            }
        }
        texts.sort_by_key(|text| text.span.start);
        texts.dedup_by_key(|text| text.span.start);
        let mut examples = Vec::new();
        for text in texts { extract(text, &mut examples)?; }
        Ok(examples)
    }
}

fn extract(text: &DocText, examples: &mut Vec<Example>) -> Result<(), ExampleError> {
    let mut blocks = Vec::new();
    let mut block: Option<Block> = None;
    for (event, range) in Parser::new(&text.text).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => block = Some(Block {
                info: match kind { CodeBlockKind::Fenced(info) => info.to_string(), CodeBlockKind::Indented => String::new() },
                code: String::new(), offsets: Vec::new(), range,
            }),
            Event::Text(code) if block.is_some() => {
                let block = block.as_mut().unwrap();
                let mut cursor = range.start;
                // CommonMark may remove container indentation. Locate each
                // verbatim code line within its event's original source range.
                for line in code.split_inclusive('\n') {
                    let body = line.trim_end_matches('\n');
                    let at = text.text[cursor..range.end].find(body).map_or(cursor, |at| cursor + at);
                    block.code.push_str(line);
                    block.offsets.extend((0..body.len()).map(|byte| text.offsets[at + byte]));
                    if line.ends_with('\n') {
                        block.offsets.push(text.offsets.get(at + body.len()).copied().unwrap_or(text.span.end));
                    }
                    cursor = (at + line.len()).min(range.end);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(mut block) = block.take() {
                    block.range.end = range.end;
                    blocks.push(block);
                }
            }
            _ => {}
        }
    }
    for (index, block) in blocks.iter().enumerate() {
        if block.info == "output" { continue; }
        let span = Span::new(text.offsets[block.range.start], text.offsets[block.range.end - 1] + 1);
        let parts: Vec<_> = block.info.split(',').map(str::trim).collect();
        let is_okkama = block.info.split(|ch: char| ch == ',' || ch.is_whitespace()).next() == Some("okkama");
        let kind = if !is_okkama { ExampleKind::Illustrative } else {
            match parts.as_slice() {
                ["okkama"] | ["okkama", "compile"] => ExampleKind::Compile,
                ["okkama", "ignore"] => ExampleKind::Illustrative,
                ["okkama", "run"] => ExampleKind::Run,
                _ => return Err(ExampleError { span, message: format!("unknown doctest metadata `{}`", block.info) }),
            }
        };
        let expected = if kind == ExampleKind::Run {
            let output = blocks.get(index + 1).filter(|next| next.info == "output"
                && text.text[block.range.end..next.range.start].trim().is_empty())
                .ok_or_else(|| ExampleError { span, message: "runnable doctest needs an adjacent `output` fence with expected output (empty is allowed)".into() })?;
            Some(output.code.clone())
        } else { None };
        examples.push(Example { kind, code: block.code.clone(), expected, span, offsets: block.offsets.clone() });
    }
    Ok(())
}
