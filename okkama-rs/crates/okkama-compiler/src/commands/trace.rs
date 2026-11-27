//! Место и стек вызовов неперехваченного отказа — для диагностики запуска.
//!
//! Стек собирает рантайм при раскрутке отказа (`Failure::trace`): пары
//! «функция, номер инструкции» в нумерации, сохранённой при генерации Sparrow. Здесь они
//! превращаются в файлы, строки и колонки по отладочным сведениям OIR.
//!
//! Since #1125 it lives with the commands: the CLI and the playground (okkama-wasm, which has the module but no
//! Sparrow) draw one stack. Nothing here reads the environment: the caller passes its current directory.
use crate::diagnostics::display_width;
use crate::{ir, source::SourceFile};
use okkama_runtime_core::failure::{Site, frame_name, stack_text_by};
use std::collections::HashMap;
use std::fmt::Write;

/// Место отказа (строка исходника с подчёркиванием) и стек вызовов.
/// Пустая строка — стек пуст. Paths inside `cwd` are shown relative to it.
pub fn render_trace(module: &ir::Module, trace: &[Site], cwd: Option<&str>) -> String {
    if trace.is_empty() {
        return String::new();
    }
    // Строка кадра — одна на место, а не на кадр: глубокая рекурсия даёт
    // миллионы кадров из нескольких мест (#241). Кадры — номера строк.
    let mut texts: Vec<String> = Vec::new();
    let mut by_text: HashMap<String, usize> = HashMap::new();
    let mut by_site: HashMap<Site, usize> = HashMap::new();
    let frames: Vec<usize> = trace
        .iter()
        .map(|site| {
            *by_site.entry(*site).or_insert_with(|| {
                let func = ir::FuncId(site.function as usize);
                let name = module
                    .functions
                    .get(func.0)
                    .map_or("?", |function| frame_name(&function.symbol));
                let text = match place(module, *site) {
                    Some((source, span)) => {
                        let at = source.location(span.start);
                        format!("{name} ({}:{}:{})", display_path(source, cwd), at.line, at.column)
                    }
                    None => format!("{name} (pc {})", site.pc),
                };
                *by_text.entry(text.clone()).or_insert_with(|| {
                    texts.push(text);
                    texts.len() - 1
                })
            })
        })
        .collect();
    // Подчёркивается ближайший к отказу кадр кода программы, а не импорта:
    // отказ внутри std чаще всего вызван строкой программы.
    let shown = trace
        .iter()
        .find(|site| module.in_root(ir::FuncId(site.function as usize)) && place(module, **site).is_some())
        .or_else(|| trace.iter().find(|site| place(module, **site).is_some()));
    let mut out = String::new();
    if let Some((source, span)) = shown.and_then(|site| place(module, *site)) {
        out.push_str(&snippet(source, span, cwd));
    }
    out.push_str(&stack_text_by("Stack trace (most recent call first):", &frames, |text| texts[*text].clone()));
    out
}

/// Исходник и место инструкции кадра.
fn place(module: &ir::Module, site: Site) -> Option<(&SourceFile, crate::source::Span)> {
    let func = ir::FuncId(site.function as usize);
    let source = module.source_of(func)?;
    let span = module.functions.get(func.0)?.span_at(site.pc)?;
    Some((source, span))
}

/// ` --> файл:строка:колонка`, строка исходника и подчёркивание места.
fn snippet(source: &SourceFile, span: crate::source::Span, cwd: Option<&str>) -> String {
    let start = source.location(span.start);
    let end = source.location(span.end);
    let text = source.line_text(start.line);
    let gutter = start.line.to_string();
    let pad = " ".repeat(gutter.len());
    // Columns are characters, the terminal shows cells: the carets follow
    // display width, as the compiler's diagnostics do (#1017; a wide
    // character before the place shifted them left).
    let chars: Vec<char> = text.chars().collect();
    let first = (start.column.saturating_sub(1) as usize).min(chars.len());
    let last = if end.line == start.line {
        (end.column.saturating_sub(1) as usize).clamp(first, chars.len())
    } else {
        chars.len()
    };
    let cells = |part: &[char]| part.iter().map(|&ch| display_width(ch)).sum::<usize>();
    let shown: String = chars.iter().map(|&ch| if ch.is_control() { ' ' } else { ch }).collect();
    let mut out = String::new();
    let _ = writeln!(out, "{pad}--> {}:{}:{}", display_path(source, cwd), start.line, start.column);
    let _ = writeln!(out, "{pad} |");
    let _ = writeln!(out, "{gutter} | {shown}");
    let _ = writeln!(out, "{pad} | {}{}", " ".repeat(cells(&chars[..first])), "^".repeat(cells(&chars[first..last]).max(1)));
    out
}

/// Путь файла: относительно текущего каталога, если файл внутри него (the diagnostics' rule, #1119).
fn display_path(source: &SourceFile, cwd: Option<&str>) -> String {
    cwd.map_or_else(|| source.name().to_string(), |dir| crate::source::display_name(source.name(), dir))
}
