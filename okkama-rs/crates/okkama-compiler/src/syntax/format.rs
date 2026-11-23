//! Форматтер исходников Okkama (`okkama fmt`, форматирование в редакторе).
//!
//! Стиль — perlstyle: отступ 4 пробела, открывающая фигурная скобка на строке
//! конструкции, каждый оператор и член объекта на своей строке, `else` и
//! `catch` на новой строке после `}` («uncuddled elses»), пробелы вокруг
//! бинарных операторов и после запятых, пустая строка между объявлениями и
//! между членами объекта (подряд идущие `has` остаются вместе). Пустые строки
//! автора сохраняются (не больше одной подряд).
//!
//! Длинные списки (аргументы, параметры, литералы) переносятся по одному
//! элементу на строку, если не помещаются в [`WIDTH`] столбцов, если автор
//! уже разбил их на строки или поставил висячую запятую. Цепочки `&&`/`||`
//! переносятся перед оператором, вызовы через точку — там, где их перенёс
//! автор. Строка, которая всё ещё длиннее [`WIDTH`], переносится в самом
//! внешнем месте, где это возможно (список, цепочка операторов или вызовов),
//! пока не поместится (#1006); длиннее остаются только строки, которые нечем
//! перенести (длинный литерал, комментарий, тип). Комментарий между
//! заголовком объявления и `{` остаётся на строке заголовка, `//` — переезжает
//! в первую строку тела.
//!
//! Форматтер меняет только пробелы и переводы строк: токены остаются теми же
//! и в том же порядке, комментарии сохраняются (комментарий внутри выражения
//! может переехать на строку перед следующим оператором). Это проверяется
//! после форматирования: при расхождении — [`FormatError::Internal`], а не
//! испорченный файл.

use crate::diagnostics::Diagnostic;
use crate::source::{SourceFile, Span};
use crate::syntax::ast::*;
use crate::syntax::{ParseMode, ParseOptions, lexer, parse, token::TokenKind};

/// Ширина строки, после которой списки переносятся.
pub const WIDTH: usize = 100;
const INDENT: &str = "    ";

#[derive(Debug)]
pub enum FormatError {
    /// Исходник не разбирается: форматируется только корректный код.
    Syntax(Vec<Diagnostic>),
    /// Ошибка форматтера (результат изменил бы токены или потерял комментарий).
    Internal(String),
}

/// Отформатированный текст файла.
pub fn format(source: &SourceFile) -> Result<String, FormatError> {
    let options = ParseOptions {
        mode: ParseMode::Full,
        ..ParseOptions::default()
    };
    let program = parse(source, &options).map_err(FormatError::Syntax)?;
    let text = source.text();
    let mut printer = Printer::new(text, comments(text), WIDTH);
    printer.program(&program);
    let output = printer.finish();
    verify(text, &output)?;
    Ok(output)
}

/// Комментарий исходника.
#[derive(Clone, Debug)]
struct Comment {
    span: Span,
}

/// Комментарии лежат только в промежутках между токенами.
fn comments(text: &str) -> Vec<Comment> {
    let lexed = lexer::lex(text);
    let mut found = Vec::new();
    let mut previous = 0usize;
    for token in &lexed.tokens {
        let gap = &text[previous..token.span.start as usize];
        let mut index = 0;
        while index < gap.len() {
            let rest = &gap[index..];
            if rest.starts_with("//") {
                let end = rest.find('\n').unwrap_or(rest.len());
                found.push(Comment {
                    span: Span::new((previous + index) as u32, (previous + index + end) as u32),
                });
                index += end;
            } else if rest.starts_with("/*") {
                let end = rest.find("*/").map_or(rest.len(), |at| at + 2);
                found.push(Comment {
                    span: Span::new((previous + index) as u32, (previous + index + end) as u32),
                });
                index += end;
            } else {
                index += rest.chars().next().map_or(1, char::len_utf8);
            }
        }
        previous = token.span.end as usize;
    }
    found
}

/// Токены и комментарии результата совпадают с исходными.
fn verify(input: &str, output: &str) -> Result<(), FormatError> {
    let before = lexer::lex(input);
    let after = lexer::lex(output);
    if !after.diagnostics.is_empty() {
        return Err(FormatError::Internal(
            "formatted text does not lex".to_string(),
        ));
    }
    // `Atomic<int>=x` лексер склеивает в `>=`, а разбор внутри типа делит
    // обратно: составные токены из `<`, `>`, `=` сравниваются посимвольно.
    let tokens =
        |text: &str, tokens: &[crate::syntax::token::Token]| -> Vec<(Option<TokenKind>, String)> {
            let mut out = Vec::new();
            for token in tokens {
                let spelling = &text[token.span.range()];
                if spelling.chars().all(|ch| matches!(ch, '<' | '>' | '=')) {
                    out.extend(spelling.chars().map(|ch| (None, ch.to_string())));
                } else {
                    out.push((Some(token.kind), spelling.to_string()));
                }
            }
            out
        };
    let (left, right) = (tokens(input, &before.tokens), tokens(output, &after.tokens));
    if left != right {
        let at = left
            .iter()
            .zip(&right)
            .position(|(a, b)| a != b)
            .unwrap_or(left.len().min(right.len()));
        return Err(FormatError::Internal(format!(
            "formatting would change tokens at token {at}: {:?} became {:?}",
            left.get(at),
            right.get(at)
        )));
    }
    let texts = |text: &str| -> Vec<String> {
        let mut all: Vec<String> = comments(text)
            .iter()
            .map(|comment| text[comment.span.range()].trim_end().to_string())
            .collect();
        all.sort();
        all
    };
    if texts(input) != texts(output) {
        return Err(FormatError::Internal(
            "formatting would lose a comment".to_string(),
        ));
    }
    Ok(())
}

struct Printer<'s> {
    text: &'s str,
    out: String,
    indent: usize,
    /// Отступ текущего оператора: продолжения строк — на один глубже.
    stmt_indent: usize,
    /// Следующее слово начинает строку (отступ ещё не выведен).
    line_start: bool,
    comments: Vec<Comment>,
    next_comment: usize,
    width: usize,
    /// One-line widths of list items already measured (#979), shared with every scratch printer.
    /// A width is measured in a scratch printer with no width limit and no comments, so it does
    /// not depend on where the item stands; without the cache each nesting level measured its
    /// items again inside the measurement of its parent, doubling the time per level.
    flat_widths: FlatWidths,
    /// Break points that must break (#1006), shared with every scratch printer so that a
    /// measurement sees the layout the line will get.
    forced: Forced,
    /// The order in which this printer forced break points, to undo the breaks that did not help.
    forced_log: Vec<BreakKey>,
    /// Break points printed unbroken, one frame per [`Printer::line`] being fitted.
    frames: Vec<Vec<BreakPoint>>,
}

type FlatWidths = std::rc::Rc<std::cell::RefCell<std::collections::HashMap<(u32, u32, std::any::TypeId), Option<usize>>>>;
type Forced = std::rc::Rc<std::cell::RefCell<std::collections::HashSet<BreakKey>>>;

/// A place the printer may break a line (#1006): the source span of a list (from its opening
/// bracket), an operator chain or a call chain, and its kind.
type BreakKey = (u32, u32, BreakKind);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum BreakKind {
    /// A bracketed list: one item per line.
    List,
    /// A chain of `&&` or `||`: a break before each operator.
    Logical,
    /// A chain of binary operators of one precedence: breaks after operators, filling lines.
    Binary,
    /// A call chain `a.f().g()`: a break before each call.
    Chain,
}

/// A break point printed unbroken, at byte `at` of the output.
#[derive(Clone, Copy, Debug)]
struct BreakPoint {
    key: BreakKey,
    at: usize,
}

/// Attempts at fitting one element into [`WIDTH`] (#1006): each forces one more break point.
const FIT_ATTEMPTS: usize = 24;

impl<'s> Printer<'s> {
    fn new(text: &'s str, comments: Vec<Comment>, width: usize) -> Self {
        Self {
            text,
            out: String::new(),
            indent: 0,
            stmt_indent: 0,
            line_start: true,
            comments,
            next_comment: 0,
            width,
            flat_widths: FlatWidths::default(),
            forced: Forced::default(),
            forced_log: Vec::new(),
            frames: Vec::new(),
        }
    }

    /// A scratch printer for measuring: no width limit, no comments, the shared width cache.
    fn scratch(&self) -> Printer<'s> {
        let mut scratch = Printer::new(self.text, Vec::new(), usize::MAX);
        scratch.flat_widths = self.flat_widths.clone();
        scratch.forced = self.forced.clone();
        scratch.line_start = false;
        scratch
    }

    fn finish(mut self) -> String {
        let _ = self.leading_comments(self.text.len() as u32, false);
        let mut out = self.out.trim_end().to_string();
        out.push('\n');
        out
    }

    // ---- вывод ----

    fn word(&mut self, text: &str) {
        if self.line_start {
            for _ in 0..self.indent {
                self.out.push_str(INDENT);
            }
            self.line_start = false;
        }
        self.out.push_str(text);
    }

    fn newline(&mut self) {
        if self.line_start {
            return;
        }
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        self.out.push('\n');
        self.line_start = true;
    }

    fn blank_line(&mut self) {
        self.newline();
        if !self.out.is_empty() && !self.out.ends_with("\n\n") && !self.out.ends_with("{\n") {
            self.out.push('\n');
        }
    }

    /// Перенос внутри оператора: продолжение на один уровень глубже оператора.
    fn continuation(&mut self) {
        self.newline();
        self.indent = self.indent.max(self.stmt_indent + 1);
    }

    fn column(&self) -> usize {
        if self.line_start {
            return self.indent * INDENT.len();
        }
        let line = self.out.rsplit('\n').next().unwrap_or("");
        line.chars().count()
    }

    fn slice(&self, span: Span) -> &'s str {
        &self.text[span.range()]
    }

    // ---- исходник ----

    /// Между `from` и `to` в исходнике есть перевод строки (вне комментариев
    /// не различаем: комментарий тоже кончается переводом строки).
    fn broken(&self, from: u32, to: u32) -> bool {
        from < to && self.text[from as usize..to as usize].contains('\n')
    }

    /// Перед `offset` (через пробелы) — пустая строка.
    fn blank_before(&self, offset: u32) -> bool {
        let before = &self.text[..offset as usize];
        let gap = &before[before.trim_end().len()..];
        gap.matches('\n').count() >= 2
    }

    /// В исходнике между `from` и `to` есть запятая (висячая запятая списка).
    fn comma_between(&self, from: u32, to: u32) -> bool {
        from < to
            && self.text[from as usize..to as usize]
                .trim_start()
                .starts_with(',')
    }

    fn has_comment_in(&self, span: Span) -> bool {
        self.comments[self.next_comment..]
            .iter()
            .any(|comment| span.start <= comment.span.start && comment.span.end <= span.end)
    }

    /// The offset of the first source character at or after `from` that is not a space or a
    /// comment.
    fn next_token(&self, from: u32) -> u32 {
        let mut at = from as usize;
        loop {
            let rest = &self.text[at..];
            let trimmed = rest.trim_start();
            at += rest.len() - trimmed.len();
            if trimmed.starts_with("//") {
                at += trimmed.find('\n').unwrap_or(trimmed.len());
            } else if trimmed.starts_with("/*") {
                at += trimmed.find("*/").map_or(trimmed.len(), |end| end + 2);
            } else {
                return at as u32;
            }
        }
    }

    /// The end of a list's closing bracket after its last item ending at `last` (#1006): past an
    /// optional trailing comma and any comments. `end` when the bracket is not there.
    fn list_close(&self, last: u32, close: &str, end: u32) -> u32 {
        let mut at = self.next_token(last);
        if self.text[at as usize..].starts_with(',') {
            at = self.next_token(at + 1);
        }
        if self.text[at as usize..].starts_with(close) {
            (at + close.len() as u32).min(end)
        } else {
            end
        }
    }

    /// The offset of the `{` that opens a declaration body after its header ends at `from`
    /// (headers of objects, roles and caps hold no braces or strings).
    fn brace_after(&self, from: u32) -> u32 {
        let mut at = from;
        while (at as usize) < self.text.len() {
            at = self.next_token(at);
            if self.text[at as usize..].starts_with('{') {
                return at;
            }
            at += self.text[at as usize..].chars().next().map_or(1, char::len_utf8) as u32;
        }
        at
    }

    // ---- комментарии ----

    /// Комментарии до `offset` — каждый на своей строке. `first` — первый
    /// элемент блока: без пустой строки перед ним.
    fn leading_comments(&mut self, offset: u32, first: bool) -> bool {
        let mut first = first;
        let mut printed = false;
        while let Some(comment) = self.comments.get(self.next_comment).cloned() {
            if comment.span.start >= offset {
                break;
            }
            self.next_comment += 1;
            self.newline();
            if !first && self.blank_before(comment.span.start) {
                self.blank_line();
            }
            first = false;
            let text = self.slice(comment.span).trim_end();
            self.word(text);
            self.newline();
            printed = true;
        }
        printed
    }

    /// Комментарий на той же строке после `end`.
    fn trailing_comment(&mut self, end: u32) {
        let Some(comment) = self.comments.get(self.next_comment).cloned() else {
            return;
        };
        if comment.span.start >= end && !self.broken(end, comment.span.start) {
            self.next_comment += 1;
            self.word(" ");
            let text = self.slice(comment.span).trim_end();
            self.word(text);
        }
    }

    /// Comments between a declaration header's last token and `before` (its `{`, `;` or result),
    /// owner's decision for #1006: a one-line block comment stays on the header line where the
    /// author wrote it. A `//` comment cannot precede `{` on one line, so from the first `//` (or
    /// a block comment spanning lines) on, the comments stay pending: a body prints them on its
    /// first line, a bodiless header after its `;` ([`Printer::header_trailing`]).
    fn header_comments(&mut self, before: u32) {
        while let Some(comment) = self.comments.get(self.next_comment).cloned() {
            if comment.span.start >= before {
                break;
            }
            let text = self.slice(comment.span).trim_end();
            if text.starts_with("//") || text.contains('\n') {
                break;
            }
            self.next_comment += 1;
            self.word(" ");
            self.word(text);
        }
    }

    /// The first comment left in a bodiless header (role signature, native action) before its
    /// end, printed after its `;` (#1006). A same-line comment written after the `;` keeps that
    /// place; the header's comments then print on their own lines below.
    fn header_trailing(&mut self, end: u32) {
        let Some(comment) = self.comments.get(self.next_comment).cloned() else {
            return;
        };
        if comment.span.start >= end {
            return;
        }
        if let Some(after) = self.comments.get(self.next_comment + 1)
            && after.span.start >= end
            && !self.broken(end, after.span.start)
        {
            return;
        }
        self.next_comment += 1;
        self.word(" ");
        let text = self.slice(comment.span).trim_end();
        self.word(text);
    }

    /// Элемент на своей строке (оператор, член, объявление).
    fn line(&mut self, span: Span, first: bool, blank: bool, print: impl Fn(&mut Self)) {
        // Обязательная пустая строка — перед комментариями элемента: они
        // документируют его и остаются над ним.
        if !first && blank {
            self.newline();
            self.blank_line();
        }
        let commented = self.leading_comments(span.start, first);
        self.newline();
        let first = first && !commented;
        if !first && self.blank_before(span.start) {
            self.blank_line();
        }
        let saved = (self.indent, self.stmt_indent);
        self.stmt_indent = self.indent;
        self.fit(&print);
        self.indent = saved.0;
        self.stmt_indent = saved.1;
        self.trailing_comment(span.end);
        self.newline();
    }

    // ---- ширина ----

    /// Prints an element so that its lines fit into the width wherever a break point allows
    /// (#1006). Each list, operator chain and call chain decides its own layout from the source
    /// and its own width, blind to what follows it on the line; when a line still overflows,
    /// the outermost break point printed unbroken on it is forced to break and the element is
    /// printed again, until it fits or no break point is left. The layout with the least
    /// overflow wins, the earliest among equals: breaks that did not shorten the overflow are
    /// undone (a long string stays where it was). Nested statements are fitted by their own
    /// [`Printer::line`].
    fn fit(&mut self, print: &dyn Fn(&mut Self)) {
        if self.width == usize::MAX {
            print(self);
            return;
        }
        let start = (self.out.len(), self.indent, self.stmt_indent, self.line_start, self.next_comment);
        let attempt = |p: &mut Self| -> (Vec<BreakPoint>, usize) {
            p.out.truncate(start.0);
            (p.indent, p.stmt_indent, p.line_start, p.next_comment) = (start.1, start.2, start.3, start.4);
            p.frames.push(Vec::new());
            print(p);
            let points = p.frames.pop().unwrap_or_default();
            let overflow = p.overflow(start.0);
            (points, overflow)
        };
        let (mut points, mut overflow) = attempt(self);
        let mut best = (overflow, self.forced_log.len(), self.snapshot(start.0));
        for _ in 0..FIT_ATTEMPTS {
            if overflow == 0 {
                break;
            }
            let Some(key) = self.break_to_force(start.0, &points) else {
                break;
            };
            self.force(key);
            (points, overflow) = attempt(self);
            if overflow < best.0 {
                best = (overflow, self.forced_log.len(), self.snapshot(start.0));
            }
        }
        // The breaks forced after the best layout did not shorten the overflow: undone.
        while self.forced_log.len() > best.1 {
            if let Some(undone) = self.forced_log.pop() {
                self.unforce(undone);
            }
        }
        self.restore(start.0, best.2);
    }

    /// The output of the element being fitted and the printer state after it.
    fn snapshot(&self, from: usize) -> (String, usize, usize, bool, usize) {
        (self.out[from..].to_string(), self.indent, self.stmt_indent, self.line_start, self.next_comment)
    }

    fn restore(&mut self, from: usize, state: (String, usize, usize, bool, usize)) {
        self.out.truncate(from);
        self.out.push_str(&state.0);
        (self.indent, self.stmt_indent, self.line_start, self.next_comment) = (state.1, state.2, state.3, state.4);
    }

    /// Columns past the width, summed over the lines from the one holding byte `from` on.
    fn overflow(&self, from: usize) -> usize {
        let line_start = self.out[..from].rfind('\n').map_or(0, |at| at + 1);
        self.out[line_start..]
            .split('\n')
            .map(|line| line.chars().count().saturating_sub(self.width))
            .sum()
    }

    /// The break point to force next: on the first overflowing line that has unbroken ones, the
    /// outermost (the longest source span, then the first).
    fn break_to_force(&self, from: usize, points: &[BreakPoint]) -> Option<BreakKey> {
        let mut line_start = self.out[..from].rfind('\n').map_or(0, |at| at + 1);
        for line in self.out[line_start..].split('\n') {
            let line_end = line_start + line.len();
            if line.chars().count() > self.width {
                let found = points
                    .iter()
                    .filter(|point| line_start <= point.at && point.at <= line_end)
                    .filter(|point| !self.is_forced(point.key))
                    .max_by_key(|point| (point.key.1 - point.key.0, std::cmp::Reverse(point.key.0)));
                if let Some(point) = found {
                    return Some(point.key);
                }
            }
            line_start = line_end + 1;
        }
        None
    }

    fn is_forced(&self, key: BreakKey) -> bool {
        self.forced.borrow().contains(&key)
    }

    /// Forces a break point and forgets the one-line widths measured over it.
    fn force(&mut self, key: BreakKey) {
        if self.forced.borrow_mut().insert(key) {
            self.forced_log.push(key);
            self.forget_widths(key);
        }
    }

    fn unforce(&mut self, key: BreakKey) {
        self.forced.borrow_mut().remove(&key);
        self.forget_widths(key);
    }

    fn forget_widths(&self, key: BreakKey) {
        self.flat_widths
            .borrow_mut()
            .retain(|&(start, end, _), _| !(start <= key.0 && key.1 <= end));
    }

    /// A break point printed unbroken here, for the element being fitted.
    fn break_point(&mut self, key: BreakKey) {
        let at = self.out.len();
        if let Some(frame) = self.frames.last_mut() {
            frame.push(BreakPoint { key, at });
        }
    }

    /// Ширина первой строки `print`.
    fn first_line(&self, print: impl FnOnce(&mut Printer<'s>)) -> usize {
        let mut scratch = self.scratch();
        print(&mut scratch);
        scratch
            .out
            .split('\n')
            .next()
            .map_or(0, |line| line.chars().count())
    }

    /// Ширина `print` в одну строку (`None` — многострочное).
    fn flat(&self, print: impl FnOnce(&mut Printer<'s>)) -> Option<usize> {
        let mut scratch = self.scratch();
        print(&mut scratch);
        (!scratch.out.contains('\n')).then(|| scratch.out.chars().count())
    }

    /// [`Printer::flat`] of a list item, measured once per item (#979).
    fn flat_item<T: 'static>(&self, span: Span, print: impl FnOnce(&mut Printer<'s>)) -> Option<usize> {
        let key = (span.start, span.end, std::any::TypeId::of::<T>());
        if let Some(width) = self.flat_widths.borrow().get(&key) {
            return *width;
        }
        let width = self.flat(print);
        self.flat_widths.borrow_mut().insert(key, width);
        width
    }

    /// The module version as the source spells it (#980). `v1.0 .0` is a valid way to write v1.0.0,
    /// but it lexes differently (`0` `.` `0`, not the float `0.0`), so printing `v1.0.0` would
    /// change the tokens; inner spaces are kept, one per gap. Comments inside fall back to the
    /// canonical form, as before.
    fn version_as_written(&self, version: &crate::syntax::ast::Version) -> String {
        let canonical = version.text();
        let written = self.text.get(version.span.range()).unwrap_or("");
        if written == canonical || written.contains('/') {
            return canonical;
        }
        written.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    // ---- файл ----

    fn program(&mut self, program: &Program) {
        self.line(program.module.span, true, false, |p| {
            p.word("module ");
            let path: Vec<&str> = program
                .module
                .path
                .iter()
                .map(|part| part.text.as_str())
                .collect();
            p.word(&path.join("/"));
            if let Some(version) = &program.module.version {
                p.word(" ");
                let written = p.version_as_written(version);
                p.word(&written);
            }
            p.word(";");
        });
        for (index, import) in program.imports.iter().enumerate() {
            self.line(import.span, false, index == 0, |p| p.import(import));
        }
        for item in &program.items {
            self.line(item.span(), false, true, |p| p.item(item));
        }
    }

    fn import(&mut self, import: &ImportDecl) {
        self.word("import ");
        self.word(self.slice(import.path.span));
        if let Some(alias) = &import.alias {
            self.word(" as ");
            self.word(&alias.text);
        }
        if !import.take.is_empty() {
            self.word(" take ");
            for (index, name) in import.take.iter().enumerate() {
                if index > 0 {
                    self.word(",");
                    if self.column() + 1 + name.text.len() + 1 > self.width {
                        self.continuation();
                    } else {
                        self.word(" ");
                    }
                }
                self.word(&name.text);
            }
        }
        self.word(";");
    }

    fn item(&mut self, item: &Item) {
        match item {
            Item::Object(decl) => self.object(decl),
            Item::Role(decl) => self.role(decl),
            Item::Enum(decl) => {
                if decl.hidden {
                    self.word("hidden ");
                }
                self.word("enum ");
                self.word(&decl.name.text);
                self.generics(&decl.generics);
                if let Some(role) = &decl.role {
                    self.word(" is ");
                    self.word(&role.text);
                }
                // Cases are a list like any other (#594): one per line when the
                // author broke them, wrote a trailing comma, or they do not fit.
                let cases: Vec<usize> = (0..decl.cases.len()).collect();
                self.word(" = ");
                self.list(
                    "(",
                    ")",
                    &cases,
                    decl.span.end,
                    |p, &index| {
                        p.word(&decl.cases[index].text);
                        if !decl.payloads[index].is_empty() {
                            p.params(&decl.payloads[index], decl.case_spans[index].end);
                        }
                    },
                    |&index| decl.case_spans[index],
                );
                self.word(";");
            }
            Item::Test(test) => self.test(test),
            Item::NativeAction(decl) => {
                self.word("native ");
                if decl.is_async {
                    self.word("async ");
                }
                self.word(&decl.name.text);
                self.params(&decl.params, decl.span.end);
                self.header_comments(decl.result.span().start);
                self.result(&decl.result);
                self.header_comments(decl.span.end);
                self.word(";");
                self.header_trailing(decl.span.end);
            }
            Item::NativeObject(decl) => {
                self.word("native object ");
                self.word(&decl.name.text);
                self.word(";");
            }
            Item::Cap(decl) => self.cap(decl),
            Item::Action(decl) => self.action(decl),
            Item::Const(decl) => {
                if decl.hidden {
                    self.word("hidden ");
                }
                self.word("const ");
                self.word(&decl.name.text);
                if let Some(ty) = &decl.ty {
                    self.word(" is ");
                    self.ty(ty);
                }
                self.word(" = ");
                self.expr(&decl.value);
                self.word(";");
            }
        }
    }

    fn generics(&mut self, generics: &[GenericParam]) {
        if generics.is_empty() {
            return;
        }
        self.word("<");
        for (index, param) in generics.iter().enumerate() {
            if index > 0 {
                self.word(", ");
            }
            self.word(&param.name.text);
            if let Some(bound) = &param.bound {
                self.word(": ");
                self.ty(bound);
            }
        }
        let last = generics[generics.len() - 1].span.end;
        if self.text[last as usize..].trim_start().starts_with(',') {
            self.word(",");
        }
        self.word(">");
    }

    fn object(&mut self, decl: &ObjectDecl) {
        if decl.hidden {
            self.word("hidden ");
        }
        self.word("object ");
        self.word(&decl.name.text);
        self.generics(&decl.generics);
        if !decl.roles.is_empty() {
            self.word(" does ");
            self.filled(&decl.roles, " {".len(), |p, role| p.ty(role), |role| role.span);
        }
        self.header_comments(self.brace_after(decl.name.span.end));
        self.indent = self.stmt_indent;
        self.word(" ");
        self.members(&decl.members, decl.span);
    }

    /// Тело объекта или fake: члены через пустую строку, кроме подряд идущих `has`.
    /// `span` — всё объявление: его ещё не напечатанные комментарии лежат в теле.
    fn members(&mut self, members: &[ObjectMember], span: Span) {
        let end = span.end;
        self.word("{");
        if members.is_empty() && !self.has_comment_in(span) {
            self.word("}");
            return;
        }
        self.indent += 1;
        let mut previous_has = false;
        for (index, member) in members.iter().enumerate() {
            let is_has = matches!(member, ObjectMember::Has(_));
            let blank = index > 0 && !(is_has && previous_has);
            self.line(member.span(), index == 0, blank, |p| p.member(member));
            previous_has = is_has;
        }
        self.leading_comments(end.saturating_sub(1), members.is_empty());
        self.newline();
        self.indent -= 1;
        self.word("}");
    }

    fn member(&mut self, member: &ObjectMember) {
        match member {
            ObjectMember::Has(has) => {
                self.word("has ");
                let names: Vec<&str> = has.names.iter().map(|name| name.text.as_str()).collect();
                self.word(&names.join(", "));
                if let Some(ty) = &has.ty {
                    self.word(" is ");
                    self.ty(ty);
                }
                if let Some(default) = &has.default {
                    self.word(" = ");
                    self.expr(default);
                }
                self.word(";");
            }
            ObjectMember::Cap(cap) => self.cap(cap),
            ObjectMember::Constructor(ctor) => {
                self.word(&ctor.name.text);
                match &ctor.form {
                    ConstructorForm::Full { params, body } => {
                        self.params(params, body.span.start);
                        self.header_comments(body.span.start);
                        self.word(" ");
                        self.header_block(body);
                    }
                    ConstructorForm::Shorthand { fields, body } => {
                        let close = body.as_ref().map_or(ctor.span.end, |body| body.span.start);
                        self.list(
                            "(",
                            ")",
                            fields,
                            close,
                            |p, field| {
                                p.word(":");
                                p.word(&field.text);
                            },
                            |field| Span::new(field.span.start.saturating_sub(1), field.span.end),
                        );
                        match body {
                            Some(body) => {
                                self.header_comments(body.span.start);
                                self.word(" ");
                                self.header_block(body);
                            }
                            None => {
                                self.header_comments(ctor.span.end);
                                self.word(";");
                                self.header_trailing(ctor.span.end);
                            }
                        }
                    }
                }
            }
            ObjectMember::Destroy(destroy) => {
                if destroy.is_async {
                    self.word("async ");
                }
                self.word("destroy ");
                self.header_block(&destroy.body);
            }
            ObjectMember::Test(test) => self.test(test),
            ObjectMember::Action(action) => self.action(action),
        }
    }

    fn cap(&mut self, cap: &CapDecl) {
        self.word("cap ");
        if let Some(owner) = &cap.owner {
            self.word(&owner.text);
            self.word("::");
        }
        self.word(&cap.name.text);
        self.header_comments(self.brace_after(cap.name.span.end));
        self.word(" {");
        // An empty body keeps the comments written inside it (#988).
        if cap.fields.is_empty() && !self.has_comment_in(cap.span) {
            self.word("}");
            return;
        }
        self.indent += 1;
        for (index, field) in cap.fields.iter().enumerate() {
            self.line(field.span, index == 0, index > 0, |p| {
                p.word(&field.name.text);
                p.word(" is ");
                p.ty(&field.ty);
                p.header_comments(field.body.span.start);
                p.word(" ");
                p.header_block(&field.body);
            });
        }
        self.leading_comments(cap.span.end.saturating_sub(1), cap.fields.is_empty());
        self.newline();
        self.indent -= 1;
        self.word("}");
    }

    fn test(&mut self, test: &TestDecl) {
        if test.is_async {
            self.word("async ");
        }
        self.word("test ");
        self.word(self.slice(test.name.span));
        if let TestBody::Parsed(block) = &test.body {
            self.header_comments(block.span.start);
        }
        self.word(" ");
        match &test.body {
            TestBody::Parsed(block) => self.header_block(block),
            TestBody::Skipped(span) => self.word(self.slice(*span)),
        }
    }

    fn action(&mut self, action: &ActionDecl) {
        if action.visibility == Visibility::Hidden {
            self.word("hidden ");
        }
        if action.is_async {
            self.word("async ");
        }
        self.word(&action.name.text);
        self.params(&action.params, action.body.span.start);
        self.header_comments(action.result.span().start);
        self.result(&action.result);
        self.header_comments(action.body.span.start);
        self.word(" ");
        self.header_block(&action.body);
    }

    fn role(&mut self, decl: &RoleDecl) {
        if decl.hidden {
            self.word("hidden ");
        }
        self.word("role ");
        self.word(&decl.name.text);
        self.generics(&decl.generics);
        self.header_comments(self.brace_after(decl.name.span.end));
        self.word(" {");
        if decl.members.is_empty() && !self.has_comment_in(decl.span) {
            self.word("}");
            return;
        }
        self.indent += 1;
        let mut previous_signature = false;
        for (index, member) in decl.members.iter().enumerate() {
            let signature = matches!(member, RoleMember::Signature(_));
            let blank = index > 0 && !(signature && previous_signature);
            self.line(member.span(), index == 0, blank, |p| match member {
                RoleMember::Signature(signature) => {
                    if signature.is_async {
                        p.word("async ");
                    }
                    p.word(&signature.name.text);
                    p.params(&signature.params, signature.span.end);
                    p.header_comments(signature.result.span().start);
                    p.result(&signature.result);
                    p.header_comments(signature.span.end);
                    p.word(";");
                    p.header_trailing(signature.span.end);
                }
                RoleMember::Fake(fake) => {
                    p.word("fake ");
                    match &fake.body {
                        FakeBody::Parsed(members) => p.members(members, fake.span),
                        FakeBody::Skipped(span) => p.word(p.slice(*span)),
                    }
                }
                RoleMember::Test(test) => p.test(test),
            });
            previous_signature = signature;
        }
        self.leading_comments(decl.span.end.saturating_sub(1), decl.members.is_empty());
        self.newline();
        self.indent -= 1;
        self.word("}");
    }

    fn params(&mut self, params: &[Param], close: u32) {
        self.list(
            "(",
            ")",
            params,
            close,
            |p, param| {
                p.word(&param.name.text);
                // `a, b is T` keeps its single type (#232).
                if param.shares_type {
                    return;
                }
                p.word(" is ");
                if param.variadic {
                    p.word("...");
                }
                p.ty(&param.ty);
                if let Some(default) = &param.default {
                    p.word(" = ");
                    p.expr(default);
                }
            },
            |param| param.span,
        );
    }

    fn result(&mut self, result: &ResultSpec) {
        self.word(" is ");
        match result {
            ResultSpec::Void(_) => self.word("void"),
            ResultSpec::Successor { ty, .. } => {
                self.word("^");
                self.ty(ty);
            }
            ResultSpec::Type(ty) => self.ty(ty),
        }
    }

    fn ty(&mut self, ty: &TypeExpr) {
        match &ty.kind {
            TypeKind::Named { path, args } => {
                let name: Vec<&str> = path.iter().map(|part| part.text.as_str()).collect();
                self.word(&name.join("."));
                if !args.is_empty() {
                    self.word("<");
                    for (index, arg) in args.iter().enumerate() {
                        if index > 0 {
                            self.word(", ");
                        }
                        self.ty(arg);
                    }
                    self.word(">");
                }
            }
            TypeKind::Nested { owner, name } => {
                self.ty(owner);
                self.word("::");
                self.word(&name.text);
            }
            TypeKind::Array(element) => {
                self.word("@");
                self.ty(element);
            }
            TypeKind::Map { key, value } => {
                self.word("%");
                self.ty(key);
                self.word(":");
                self.ty(value);
            }
            TypeKind::Action {
                is_async,
                params,
                result,
            } => {
                if *is_async {
                    self.word("async ");
                }
                self.word("(");
                for (index, param) in params.iter().enumerate() {
                    if index > 0 {
                        self.word(", ");
                    }
                    self.ty(param);
                }
                self.word(") is ");
                match result {
                    ActionResult::Void(_) => self.word("void"),
                    ActionResult::Type(ty) => self.ty(ty),
                }
            }
        }
    }

    /// Список в скобках: в строку или по элементу на строке (не помещается,
    /// автор разбил, висячая запятая, комментарии внутри). Последний
    /// многострочный элемент (блок, closure) остаётся «прижатым»: `f(a, b -> {`.
    /// `close` — смещение закрывающей скобки или позже (висячая запятая
    /// ищется между последним элементом и ним).
    fn list<T: 'static>(
        &mut self,
        open: &str,
        close: &str,
        items: &[T],
        end: u32,
        print: impl Fn(&mut Self, &T),
        span: impl Fn(&T) -> Span,
    ) {
        if items.is_empty() {
            self.word(open);
            self.word(close);
            return;
        }
        let first = span(&items[0]);
        let last = span(&items[items.len() - 1]);
        // The list ends at its real closing bracket, never at the block or `;` after it: a
        // comment between them belongs to the header, not to the last item (#1006).
        let end = self.list_close(last.end, close, end);
        let start = self.text[..first.start as usize]
            .rfind(open)
            .map_or(first.start, |at| at as u32);
        let trailing = self.comma_between(last.end, end);
        let key = (start, end, BreakKind::List);
        let mut broken = trailing || self.has_comment_in(Span::new(start, end)) || self.is_forced(key);
        for pair in items.windows(2) {
            broken |= self.broken(span(&pair[0]).end, span(&pair[1]).start);
        }
        broken |= self.broken(start, first.start);
        let mut widths = Vec::new();
        if !broken {
            for item in items {
                widths.push(self.flat_item::<T>(span(item), |p| print(p, item)));
            }
            let hug = widths[..widths.len() - 1].iter().all(Option::is_some)
                && widths[widths.len() - 1].is_none();
            if hug {
                // Прижатый последний элемент: первая строка целиком должна помещаться.
                let before: usize = widths[..widths.len() - 1]
                    .iter()
                    .flatten()
                    .map(|width| width + 2)
                    .sum();
                let head = self.first_line(|p| print(p, &items[items.len() - 1]));
                broken = self.column() + open.len() + before + head > self.width;
            } else {
                match widths.iter().copied().collect::<Option<Vec<usize>>>() {
                    Some(widths) => {
                        let total: usize = widths.iter().sum::<usize>()
                            + 2 * (widths.len() - 1)
                            + open.len()
                            + close.len();
                        broken = self.column() + total > self.width;
                    }
                    None => broken = true,
                }
            }
        }
        if !broken {
            self.break_point(key);
        }
        self.word(open);
        if !broken {
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    self.word(", ");
                }
                print(self, item);
            }
            self.word(close);
            return;
        }
        let saved = (self.indent, self.stmt_indent);
        self.indent += 1;
        self.stmt_indent = self.indent;
        for (index, item) in items.iter().enumerate() {
            // A continuation inside the previous item does not shift this one (#1006).
            self.indent = saved.0 + 1;
            self.stmt_indent = self.indent;
            self.leading_comments(span(item).start, index == 0);
            self.newline();
            print(self, item);
            if index + 1 < items.len() || trailing {
                self.word(",");
            }
            let next = items.get(index + 1).map_or(end, |next| span(next).start);
            self.trailing_comment_before(span(item).end, next);
        }
        self.leading_comments(end.saturating_sub(1), false);
        self.newline();
        self.indent = saved.0;
        self.stmt_indent = saved.1;
        self.word(close);
    }

    /// Items separated by `, ` without brackets (the roles after `does`, a case's patterns),
    /// filling each line (#1006): an item that would pass the width, with `tail` after the last
    /// one, starts a continuation line.
    fn filled<T: 'static>(&mut self, items: &[T], tail: usize, print: impl Fn(&mut Self, &T), span: impl Fn(&T) -> Span) {
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                self.word(",");
                let width = self.flat_item::<T>(span(item), |p| print(p, item)).unwrap_or(0);
                let after = if index + 1 < items.len() { ",".len() } else { tail };
                if self.width != usize::MAX && self.column() + 1 + width + after > self.width {
                    self.continuation();
                } else {
                    self.word(" ");
                }
            }
            print(self, item);
        }
    }

    /// Комментарий в конце строки элемента списка (после его запятой).
    fn trailing_comment_before(&mut self, end: u32, limit: u32) {
        if let Some(comment) = self.comments.get(self.next_comment)
            && comment.span.start < limit {
                self.trailing_comment(end);
            }
    }

    // ---- блоки и операторы ----

    /// The body of a statement or declaration: indented from the statement's line even when its
    /// header broke onto continuation lines (#1006), and `}` back at the statement's indent.
    fn header_block(&mut self, block: &Block) {
        self.indent = self.stmt_indent;
        self.block(block);
    }

    fn block(&mut self, block: &Block) {
        self.word("{");
        // Comments still pending before the `{` were written in the header (a `//` there, #1006):
        // they open the body.
        let header = self
            .comments
            .get(self.next_comment)
            .is_some_and(|comment| comment.span.start < block.span.start);
        if block.stmts.is_empty() && !header && !self.has_comment_in(block.span) {
            self.word("}");
            return;
        }
        self.indent += 1;
        let opened = self.leading_comments(block.span.start, true);
        for (index, stmt) in block.stmts.iter().enumerate() {
            self.line(stmt.span, index == 0 && !opened, false, |p| p.stmt(stmt));
        }
        self.leading_comments(block.span.end.saturating_sub(1), block.stmts.is_empty());
        self.newline();
        self.indent -= 1;
        self.word("}");
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let { name, ty, value, once } => {
                if *once {
                    self.word("once ");
                }
                self.word("let ");
                self.word(&name.text);
                if let Some(ty) = ty {
                    self.word(" is ");
                    self.ty(ty);
                }
                self.word(" = ");
                self.expr(value);
                self.word(";");
            }
            StmtKind::Defer { value } => {
                self.word("defer ");
                self.expr(value);
                self.word(";");
            }
            StmtKind::Try(body) => {
                self.try_body(body);
                if self.slice(stmt.span).trim_end().ends_with(';') {
                    self.word(";");
                }
            }
            StmtKind::Throw { value, condition } => {
                self.word("throw");
                if let Some(value) = value {
                    self.word(" ");
                    self.expr(value);
                }
                self.postfix_if(condition.as_ref());
                self.word(";");
            }
            StmtKind::Return { value, condition } => {
                self.word("return");
                if let Some(value) = value {
                    self.word(" ");
                    self.expr(value);
                }
                self.postfix_if(condition.as_ref());
                self.word(";");
            }
            StmtKind::Confirm {
                condition,
                fallback,
            } => {
                // Only this statement's output is searched: recounting the
                // whole output was quadratic on large files (#988). The start
                // is taken before the keyword, which `newline` never trims.
                let start = self.out.len();
                self.word("confirm ");
                self.expr(condition);
                let multiline = self.out[start..].contains('\n');
                if let Some(fallback) = fallback {
                    let (keyword, word, value) = match fallback {
                        ConfirmFallback::Panic { keyword, value } => (*keyword, "panic ", value),
                        ConfirmFallback::Throw { keyword, value } => (*keyword, "throw ", value),
                    };
                    let tail = self.flat(|p| {
                        p.word(" else ");
                        p.word(word);
                        p.expr(value);
                        p.word(";");
                    });
                    let wide = tail.is_none_or(|width| self.column() + width > self.width);
                    if multiline || self.broken(condition.span.end, keyword.start) || wide {
                        self.continuation();
                        self.word("else ");
                    } else {
                        self.word(" else ");
                    }
                    self.word(word);
                    self.expr(value);
                }
                self.word(";");
            }
            StmtKind::SuccessorAssign { name, value } => {
                self.word("^");
                self.word(&name.text);
                self.word(" = ");
                self.expr(value);
                self.word(";");
            }
            StmtKind::Block(block) => self.header_block(block),
            StmtKind::If(found) => {
                self.if_stmt(found);
                if self.slice(stmt.span).trim_end().ends_with(';') {
                    self.word(";");
                }
            }
            StmtKind::Match(body) => {
                self.match_body(body);
                if self.slice(stmt.span).trim_end().ends_with(';') {
                    self.word(";");
                }
            }
            StmtKind::For {
                is_async,
                source,
                bindings,
                body,
            } => {
                self.word(if *is_async { "for await " } else { "for " });
                self.expr(source);
                self.word(" -> ");
                for (index, binding) in bindings.iter().enumerate() {
                    if index > 0 {
                        self.word(", ");
                    }
                    self.word(&binding.name.text);
                }
                self.word(" ");
                self.header_block(body);
            }
            StmtKind::Next { condition } => {
                self.word("next");
                self.postfix_if(condition.as_ref());
                self.word(";");
            }
            StmtKind::Last { value, condition } => {
                self.word("last");
                if let Some(value) = value {
                    self.word(" ");
                    self.expr(value);
                }
                self.postfix_if(condition.as_ref());
                self.word(";");
            }
            StmtKind::FieldBind { name, value } => {
                self.word(&name.text);
                self.word(" = ");
                self.expr(value);
                self.word(";");
            }
            StmtKind::SelfFieldBind { name, value, .. } => {
                self.word("self.");
                self.word(&name.text);
                self.word(" = ");
                self.expr(value);
                self.word(";");
            }
            StmtKind::Expr { expr, condition } => {
                self.expr(expr);
                self.postfix_if(condition.as_ref());
                // Preserve optional terminators on tail expressions and loops.
                if self.slice(stmt.span).trim_end().ends_with(';') {
                    self.word(";");
                }
            }
        }
    }

    fn postfix_if(&mut self, condition: Option<&Expr>) {
        if let Some(condition) = condition {
            self.word(" if ");
            self.expr(condition);
        }
    }

    fn if_stmt(&mut self, found: &IfStmt) {
        self.word("if ");
        self.expr(&found.condition);
        self.word(" ");
        self.header_block(&found.then_block);
        match &found.else_branch {
            None => {}
            Some(ElseBranch::Block(block)) => {
                self.else_line(block.span.start);
                self.word("else ");
                self.header_block(block);
            }
            Some(ElseBranch::If(next)) => {
                self.else_line(next.span.start);
                self.word("else ");
                self.if_stmt(next);
            }
        }
    }

    /// `}` и `else`/`catch` — на разных строках; комментарии перед ними — на своих.
    fn else_line(&mut self, before: u32) {
        self.newline();
        self.leading_comments(before, true);
    }

    fn match_body(&mut self, body: &MatchBody) {
        self.word("match ");
        self.expr(&body.subject);
        self.indent = self.stmt_indent;
        self.word(" {");
        self.indent += 1;
        for (index, case) in body.cases.iter().enumerate() {
            self.line(case.span, index == 0, false, |p| {
                p.word("case ");
                let tail = if case.compact { ":".len() } else { " {".len() };
                p.filled(&case.patterns, tail, |p, pattern| p.expr(pattern), |pattern| pattern.span);
                if case.compact {
                    p.word(": ");
                    p.compact_value(&case.body);
                    p.word(";");
                } else {
                    p.word(" ");
                    p.header_block(&case.body);
                }
            });
        }
        if let Some(block) = &body.else_block {
            if body.else_compact {
                self.line(block.span, body.cases.is_empty(), false, |p| {
                    p.word("case _: ");
                    p.compact_value(block);
                    p.word(";");
                });
            } else {
                let span = Span::new(self.else_keyword(block.span.start), block.span.end);
                self.line(span, body.cases.is_empty(), false, |p| {
                    p.word("else ");
                    p.header_block(block);
                });
            }
        }
        self.leading_comments(body.span.end.saturating_sub(1), false);
        self.newline();
        self.indent -= 1;
        self.word("}");
    }

    /// Начало `else` перед блоком, начинающимся в `block`.
    fn else_keyword(&self, block: u32) -> u32 {
        self.text[..block as usize]
            .rfind("else")
            .map_or(block, |at| at as u32)
    }

    fn try_body(&mut self, body: &TryBody) {
        // `test.throws(Type) { … }` (#216) is parsed as the `try` it means:
        // its catch binds a name source cannot spell, and its block ends with
        // the panic the parser added.
        if let [catch] = body.catches.as_slice()
            && catch.name.as_ref().is_some_and(|name| name.text == "test.throws#error")
            && let [ty] = catch.types.as_slice()
        {
            self.word("test.throws(");
            self.ty(ty);
            self.word(") ");
            let mut written = body.body.clone();
            written.stmts.pop();
            self.header_block(&written);
            return;
        }
        self.word("try ");
        self.header_block(&body.body);
        for catch in &body.catches {
            self.else_line(catch.span.start);
            self.word("catch ");
            if let Some(name) = &catch.name {
                self.word(&name.text);
                self.word(" is ");
                for (index, ty) in catch.types.iter().enumerate() {
                    if index > 0 {
                        self.word(", ");
                    }
                    self.ty(ty);
                }
            } else {
                for (index, pattern) in catch.patterns.iter().enumerate() {
                    if index > 0 {
                        self.word(", ");
                    }
                    self.expr(pattern);
                }
            }
            self.word(" ");
            self.header_block(&catch.body);
        }
    }

    // ---- выражения ----

    /// The value of a compact match case (#893): its one expression statement.
    fn compact_value(&mut self, block: &Block) {
        if let StmtKind::Expr { expr, .. } = &block.stmts[0].kind {
            self.expr(expr);
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Throw(value) => {
                self.word("throw ");
                self.expr(value);
            }
            ExprKind::TypeValue(ty) => self.ty(ty),
            ExprKind::Coalesce { value, fallback } => {
                self.expr(value);
                self.word(" else ");
                self.expr(fallback);
            }
            ExprKind::Literal(_) => self.word(self.slice(expr.span)),
            ExprKind::Name(name) => self.word(&name.text),
            ExprKind::SelfValue => self.word("self"),
            ExprKind::Grouping(inner) => {
                self.word("(");
                self.expr(inner);
                self.word(")");
            }
            ExprKind::Unary { op, operand, .. } => {
                self.word(op.symbol());
                // `- -x`, `- -1`: не склеивать в другой токен.
                let operand_text = self.slice(operand.span);
                if *op == UnaryOp::Negate && operand_text.starts_with('-') {
                    self.word(" ");
                }
                self.expr(operand);
            }
            ExprKind::Binary { op, .. } => {
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    self.logical_chain(expr);
                } else {
                    self.binary_chain(expr);
                }
            }
            ExprKind::StreamWrite {
                sink,
                source,
                op_span,
            } => {
                self.expr(sink);
                self.word(" <-");
                if self.broken(op_span.end, source.span.start) {
                    self.continuation();
                } else {
                    self.word(" ");
                }
                self.expr(source);
            }
            ExprKind::Range {
                inclusive,
                start,
                end,
                ..
            } => {
                self.expr(start);
                self.word(if *inclusive { "..=" } else { ".." });
                self.expr(end);
            }
            ExprKind::Call { callee, .. } if matches!(callee.kind, ExprKind::Get { .. }) => self.chain(expr),
            ExprKind::Call { callee, args } => {
                self.expr(callee);
                self.args(args, expr.span.end);
            }
            ExprKind::ArrayCast { target, args, .. } => {
                self.word("@");
                self.word(&target.text);
                self.args(args, expr.span.end);
            }
            ExprKind::Get { .. } | ExprKind::Functional { .. } => self.chain(expr),
            ExprKind::Select { object, name } => {
                self.expr(object);
                self.word("::");
                self.word(&name.text);
            }
            ExprKind::OwnerField { owner, name } => {
                self.word(&owner.text);
                self.word("::");
                self.word(&name.text);
            }
            ExprKind::Index {
                object,
                index,
                fallback,
            } => {
                self.expr(object);
                self.word("[");
                self.expr(index);
                self.word("]");
                if let Some(fallback) = fallback {
                    self.word(" else ");
                    self.expr(fallback);
                }
            }
            ExprKind::Unpack { operand, .. } => {
                self.expr(operand);
                self.word("...");
            }
            ExprKind::AnonymousAction {
                params,
                result,
                body,
            } => {
                self.params(params, body.span.start);
                self.header_comments(match result {
                    ActionResult::Void(span) => span.start,
                    ActionResult::Type(ty) => ty.span.start,
                });
                self.word(" is ");
                match result {
                    ActionResult::Void(_) => self.word("void"),
                    ActionResult::Type(ty) => self.ty(ty),
                }
                self.header_comments(body.span.start);
                self.word(" ");
                self.block(body);
            }
            ExprKind::Array { elements } => {
                self.list(
                    "[",
                    "]",
                    elements,
                    expr.span.end,
                    |p, element| p.expr(element),
                    |element| element.span,
                );
            }
            ExprKind::Map { entries } => {
                if entries.is_empty() {
                    self.word("[:]");
                } else {
                    self.list(
                        "[",
                        "]",
                        entries,
                        expr.span.end,
                        |p, entry| {
                            p.expr(&entry.key);
                            p.word(": ");
                            p.expr(&entry.value);
                        },
                        |entry| entry.span,
                    );
                }
            }
            ExprKind::If {
                condition,
                then_block,
                else_block,
            } => {
                // The compact form (#921) keeps its spelling: `if c: a else: b`.
                if let (Some(then_value), Some(else_value)) = (sole_expression(then_block), sole_expression(else_block)) {
                    self.word("if ");
                    self.expr(condition);
                    self.word(": ");
                    self.expr(then_value);
                    self.word(" else: ");
                    self.expr(else_value);
                    return;
                }
                self.word("if ");
                self.expr(condition);
                self.word(" ");
                self.header_block(then_block);
                self.else_line(else_block.span.start);
                self.word("else ");
                match else_if(else_block) {
                    Some(next) => self.expr(next),
                    None => self.header_block(else_block),
                }
            }
            ExprKind::Match(body) => self.match_body(body),
            ExprKind::Loop { body } => {
                self.word("loop ");
                self.header_block(body);
            }
            ExprKind::Try(body) => self.try_body(body),
            ExprKind::Panic { value } => {
                self.word("panic ");
                self.expr(value);
            }
            ExprKind::Await { value } => {
                self.word("await ");
                self.expr(value);
            }
            ExprKind::TryMark { force, value } => {
                self.word(if *force { "try! " } else { "try? " });
                self.expr(value);
            }
            ExprKind::Spawn { call } => {
                self.word("spawn ");
                self.expr(call);
            }
        }
    }

    fn args(&mut self, args: &[Argument], end: u32) {
        self.list(
            "(",
            ")",
            args,
            end,
            |p, arg| {
                if let Some(name) = &arg.name {
                    p.word(&name.text);
                    p.word(": ");
                }
                p.expr(&arg.value);
            },
            |arg| arg.span,
        );
    }

    /// A chain of binary operators of one precedence, `a + b - c` (#1006): a break after an
    /// operator where the author broke the line; once forced to break, the operands fill each
    /// line and the chain breaks after the last operator that fits. A scratch printer breaks a
    /// forced chain after every operator, so a measurement sees it take several lines, as the
    /// source will once formatted.
    fn binary_chain(&mut self, expr: &Expr) {
        let ExprKind::Binary { op, .. } = &expr.kind else {
            return self.expr(expr);
        };
        let mut links = Vec::new();
        let first = collect_level(expr, precedence(*op), &mut links);
        let key = (expr.span.start, expr.span.end, BreakKind::Binary);
        let forced = self.is_forced(key);
        if !forced {
            self.break_point(key);
        }
        self.expr(first);
        let mut previous = first.span.end;
        for (index, (op, op_span, right)) in links.iter().enumerate() {
            self.word(" ");
            self.word(op.symbol());
            let breaks = if !forced {
                self.broken(previous, op_span.start) || self.broken(op_span.end, right.span.start)
            } else if self.width == usize::MAX {
                true
            } else {
                let width = self
                    .flat_item::<BinaryOperand>(right.span, |p| p.expr(right))
                    .unwrap_or_else(|| self.first_line(|p| p.expr(right)));
                let operator = links.get(index + 1).map_or(0, |next| next.0.symbol().len() + 1);
                self.column() + 1 + width + operator > self.width
            };
            if breaks {
                self.continuation();
            } else {
                self.word(" ");
            }
            self.expr(right);
            previous = right.span.end;
        }
    }

    /// A call chain `root.field.f(x).g(y) -> v { … }` (#1006): a break before a link where the
    /// author broke the line; once forced to break, a break before every call, except the first
    /// call on a plain name (`out.writeln(…)` keeps its receiver).
    fn chain(&mut self, expr: &Expr) {
        let mut links = Vec::new();
        let mut root = expr;
        loop {
            root = match &root.kind {
                ExprKind::Call { callee, .. } => match &callee.kind {
                    ExprKind::Get { object, .. } => {
                        links.push(root);
                        object
                    }
                    _ => break,
                },
                ExprKind::Get { object, .. } => {
                    links.push(root);
                    object
                }
                ExprKind::Functional { receiver, .. } => {
                    links.push(root);
                    receiver
                }
                _ => break,
            };
        }
        links.reverse();
        let plain = matches!(
            root.kind,
            ExprKind::Name(_) | ExprKind::SelfValue | ExprKind::OwnerField { .. } | ExprKind::Select { .. }
        );
        let calls = |link: &&Expr| !matches!(link.kind, ExprKind::Get { .. });
        let mut breakable: Vec<bool> = links.iter().map(calls).collect();
        if plain && let Some(first) = breakable.iter().position(|&call| call) {
            breakable[first] = false;
        }
        let key = (expr.span.start, expr.span.end, BreakKind::Chain);
        let forced = self.is_forced(key);
        if !forced && breakable.contains(&true) {
            self.break_point(key);
        }
        self.expr(root);
        let mut previous = root.span.end;
        for (link, breakable) in links.iter().zip(breakable) {
            let name = match &link.kind {
                ExprKind::Call { callee, .. } => match &callee.kind {
                    ExprKind::Get { name, .. } => name,
                    _ => unreachable!("a chain link calls a field"),
                },
                ExprKind::Get { name, .. } => name,
                ExprKind::Functional { operation_name, .. } => operation_name,
                _ => unreachable!("a chain link is a call, a field or a functional call"),
            };
            if self.broken(previous, name.span.start) || (forced && breakable) {
                self.continuation();
            }
            self.word(".");
            self.word(&name.text);
            match &link.kind {
                ExprKind::Call { args, .. } => self.args(args, link.span.end),
                ExprKind::Functional { args, bindings, body, .. } => {
                    self.args(args, body.span.start);
                    self.word(" ->");
                    for (index, binding) in bindings.iter().enumerate() {
                        self.word(if index > 0 { ", " } else { " " });
                        self.word(&binding.text);
                    }
                    self.word(" ");
                    self.block(body);
                }
                _ => {}
            }
            previous = link.span.end;
        }
    }

    /// `a && b || c`: в строку, если помещается и автор не переносил; иначе
    /// перенос перед каждым `&&`/`||` цепочки одного оператора.
    fn logical_chain(&mut self, expr: &Expr) {
        let ExprKind::Binary { op, .. } = &expr.kind else {
            return self.expr(expr);
        };
        let mut operands = Vec::new();
        collect_chain(expr, *op, &mut operands);
        let user_broken = operands
            .windows(2)
            .any(|pair| self.broken(pair[0].span.end, pair[1].span.start));
        let flat = self.flat(|p| {
            for (index, operand) in operands.iter().enumerate() {
                if index > 0 {
                    p.word(" ");
                    p.word(op.symbol());
                    p.word(" ");
                }
                p.expr(operand);
            }
        });
        let fits = flat.is_some_and(|width| self.column() + width <= self.width);
        let key = (expr.span.start, expr.span.end, BreakKind::Logical);
        let breaks = user_broken || !fits || self.is_forced(key);
        if !breaks {
            self.break_point(key);
        }
        for (index, operand) in operands.iter().enumerate() {
            if index > 0 {
                if breaks {
                    self.continuation();
                } else {
                    self.word(" ");
                }
                self.word(op.symbol());
                self.word(" ");
            }
            self.expr(operand);
        }
    }
}

/// The expression of a branch the parser built from one (a compact `if`, #921): a block holding that expression
/// alone and spanning exactly it. A written block also spans its braces.
fn sole_expression(block: &Block) -> Option<&Expr> {
    match block.stmts.as_slice() {
        [Stmt { kind: StmtKind::Expr { expr, condition: None }, span }] if *span == block.span => Some(expr),
        _ => None,
    }
}

/// Вложенный `if` звена `else if` в if-выражении: парсер хранит его как
/// else-блок из одного этого `if` с его же span, а написанный блок включает
/// ещё и фигурные скобки.
fn else_if(block: &Block) -> Option<&Expr> {
    match block.stmts.as_slice() {
        [Stmt { kind: StmtKind::Expr { expr, condition: None }, span }]
            if *span == block.span && matches!(expr.kind, ExprKind::If { .. }) =>
        {
            Some(expr)
        }
        _ => None,
    }
}

/// Marks the cached one-line width of a binary operand (#1006).
struct BinaryOperand;

/// The parser's precedence level of a binary operator other than `&&` and `||`.
fn precedence(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Or | BinaryOp::And => 0,
        BinaryOp::Equal | BinaryOp::NotEqual => 1,
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => 2,
        BinaryOp::BitOr => 3,
        BinaryOp::BitXor => 4,
        BinaryOp::BitAnd => 5,
        BinaryOp::ShiftLeft | BinaryOp::ShiftRight | BinaryOp::ShiftRightUnsigned => 6,
        BinaryOp::Add | BinaryOp::Subtract => 7,
        BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Remainder => 8,
    }
}

/// The first operand of a left-associative chain of operators of one precedence `level`, and
/// each following operator with its operand.
fn collect_level<'e>(expr: &'e Expr, level: u8, links: &mut Vec<(BinaryOp, Span, &'e Expr)>) -> &'e Expr {
    match &expr.kind {
        ExprKind::Binary { op, left, right, op_span }
            if !matches!(op, BinaryOp::And | BinaryOp::Or) && precedence(*op) == level =>
        {
            let first = collect_level(left, level, links);
            links.push((*op, *op_span, right));
            first
        }
        _ => expr,
    }
}

/// Операнды левоассоциативной цепочки одного логического оператора.
fn collect_chain<'e>(expr: &'e Expr, op: BinaryOp, out: &mut Vec<&'e Expr>) {
    match &expr.kind {
        ExprKind::Binary {
            op: inner,
            left,
            right,
            ..
        } if *inner == op => {
            collect_chain(left, op, out);
            out.push(right);
        }
        _ => out.push(expr),
    }
}

#[cfg(test)]
mod tests;
