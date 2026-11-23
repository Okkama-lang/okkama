//! `okkama repl` on Sparrow (#588): one long-lived session, one heap.
//!
//! **Input.** Declarations (`object`, `enum`, `role`, `native`, `cap`) and imports accumulate in a `<repl>`
//! file. Statements become a cell: an action `ReplCellN.run(<previous variables>)` in a second file of the same
//! module (spec §5.1), which returns its variables as `ReplKeep<T>` values.
//!
//! **Each input** compiles the declarations and the new cell with the ordinary compiler. Then:
//! 1. the result merges into the session's published program (`okkama_sparrow::session::extend`);
//! 2. the live runtime grows by `Runtime::extend`;
//! 3. only the new cell runs; earlier cells' side effects never repeat.
//!
//! **Values.** They stay in the heap. The session region owns what the variables own (`Runtime::session_keep`):
//! a dropped or rebound variable's resources are destroyed when the next cell has run, and everything left on
//! `:reset` or exit. A value that reaches a Future or a closure is not kept.
//!
//! On a terminal, input is line-edited (arrows, history in `~/.okkama_history`). Without one, stdin is read line
//! by line and nothing but the program's output goes to stdout.
use super::{lower_source, pinned_icu};
use okkama_compiler::codegen;
use okkama_compiler::source::SourceFile;
use okkama_compiler::syntax::{self, ParseOptions, ast};
use sparrow_core::program::Loaded;
use sparrow_core::runtime::{Capacities, Runtime};
use sparrow_core::word::{Repr, Word};
use std::io::{self, BufRead, IsTerminal, Write};
use std::process::ExitCode;

/// The generic keeper object: an instance's name carries the variable's type.
const KEEPER: &str = "ReplKeep";
/// The input file; its first line is the header, the user's input starts on the second.
const INPUT: &str = "<input>";
/// The session's bounds (design §6): functions and object types.
const CAPACITIES: Capacities = Capacities { functions: 16_384, objects: 4_096 };

const HELP: &str = "\
Enter statements, expressions, declarations (object, enum, role) or imports.
The value of a trailing expression is dumped; variables live on between inputs.

    :vars     variables and their types
    :decls    declarations and imports so far
    :reset    forget everything (destroys what the variables own)
    :help     this text
    :quit     leave (also Ctrl-D)

On a terminal: arrows move the cursor and walk the history (~/.okkama_history),
Ctrl-R searches it. Actions (closures) and futures are not kept between inputs.
";

struct Var {
    name: String,
    ty: String,
    value: Word,
}

#[derive(Default)]
struct Session {
    imports: Vec<String>,
    decls: Vec<String>,
    vars: Vec<Var>,
    cells: usize,
    runtime: Option<Box<Runtime>>,
}

/// Lines from a terminal (edited, with history) or from a pipe.
enum Input {
    Terminal(Box<rustyline::DefaultEditor>, Option<std::path::PathBuf>),
    Pipe(io::Stdin),
}

impl Input {
    /// The next line with its newline; `None` at the end of input.
    fn line(&mut self, prompt: &str) -> Option<String> {
        match self {
            Input::Terminal(editor, _) => loop {
                match editor.readline(prompt) {
                    Ok(line) => return Some(line + "\n"),
                    // Ctrl-C drops the line being typed, as in a shell.
                    Err(rustyline::error::ReadlineError::Interrupted) => continue,
                    Err(_) => return None,
                }
            },
            Input::Pipe(stdin) => {
                let mut line = String::new();
                match stdin.lock().read_line(&mut line) {
                    Ok(0) | Err(_) => None,
                    Ok(_) => Some(line),
                }
            }
        }
    }

    fn remember(&mut self, entry: &str) {
        if let Input::Terminal(editor, _) = self {
            let _ = editor.add_history_entry(entry.trim_end());
        }
    }

    fn save(&mut self) {
        if let Input::Terminal(editor, Some(path)) = self {
            let _ = editor.save_history(path);
        }
    }
}

/// The REPL on a thread with the VM's stack (#1009): a cell runs with
/// `sparrow_core::STACK_BYTES` as its stack limit, so the thread must really
/// have it; on the 8 MiB main thread deep recursion killed the process
/// instead of failing with "Stack overflow".
pub(crate) fn run() -> ExitCode {
    let thread = std::thread::Builder::new()
        .name("okkama-repl".into())
        .stack_size(sparrow_core::STACK_BYTES as usize)
        .spawn(repl);
    match thread.map(std::thread::JoinHandle::join) {
        Ok(Ok(code)) => code,
        Ok(Err(_)) => ExitCode::from(70),
        Err(error) => {
            eprintln!("okkama: REPL thread: {error}");
            ExitCode::from(70)
        }
    }
}

fn repl() -> ExitCode {
    if std::env::var_os("OKKAMA_ICU_LIBDIR").is_none()
        && let Some(icu) = pinned_icu()
    {
        sparrow_core::default_environment("OKKAMA_ICU_LIBDIR", &icu.display().to_string());
    }
    let mut input = if io::stdin().is_terminal() {
        match rustyline::DefaultEditor::new() {
            Ok(mut editor) => {
                let history = std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".okkama_history"));
                if let Some(path) = &history {
                    let _ = editor.load_history(path);
                }
                println!("okkama {} REPL (Sparrow) — :help for commands", env!("CARGO_PKG_VERSION"));
                Input::Terminal(Box::new(editor), history)
            }
            Err(_) => Input::Pipe(io::stdin()),
        }
    } else {
        Input::Pipe(io::stdin())
    };
    let mut session = Session::default();
    let mut text = String::new();
    loop {
        let prompt = if text.is_empty() { "okkama> " } else { "   ...> " };
        let Some(line) = input.line(prompt) else { break };
        if text.is_empty() && line.trim().is_empty() {
            continue;
        }
        if text.is_empty() && line.trim_start().starts_with(':') {
            input.remember(&line);
            if !session.command(line.trim()) {
                break;
            }
            continue;
        }
        text.push_str(&line);
        if complete(&text) {
            input.remember(&text);
            session.eval(std::mem::take(&mut text).trim());
        }
    }
    if !text.trim().is_empty() {
        session.eval(text.trim());
    }
    input.save();
    session.close();
    ExitCode::SUCCESS
}

fn flush() {
    let _ = okkama_runtime_core::stdout::flush();
    let _ = io::stdout().flush();
}

impl Session {
    /// A `:...` command; `false` leaves.
    fn command(&mut self, command: &str) -> bool {
        match command {
            ":quit" | ":q" | ":exit" => return false,
            ":help" | ":h" => print!("{HELP}"),
            ":vars" => {
                if self.vars.is_empty() {
                    println!("(no variables)");
                }
                for var in &self.vars {
                    println!("{}: {}", var.name, var.ty);
                }
            }
            ":decls" => {
                for import in &self.imports {
                    println!("{import}");
                }
                for decl in &self.decls {
                    println!("{decl}");
                }
            }
            ":reset" => {
                self.close();
                *self = Self::default();
            }
            other => eprintln!("unknown command `{other}` (:help lists commands)"),
        }
        flush();
        true
    }

    /// Destroy what the variables own (`:reset`, exit).
    fn close(&mut self) {
        self.vars.clear();
        if let Some(runtime) = &mut self.runtime
            && let Err(error) = runtime.session_close()
        {
            eprintln!("{}", error.0);
        }
        flush();
    }

    fn eval(&mut self, text: &str) {
        let first = text.split_whitespace().next().unwrap_or("");
        match first {
            "import" => {
                self.imports.push(text.to_string());
                if let Err(errors) = self.load(None) {
                    self.imports.pop();
                    eprint!("{errors}");
                }
            }
            "object" | "enum" | "role" | "native" | "cap" => {
                self.decls.push(text.to_string());
                if let Err(errors) = self.load(None) {
                    self.decls.pop();
                    eprint!("{errors}");
                }
            }
            _ => self.statements(text),
        }
        flush();
    }

    /// The first line of an input file: the module and the imports (each file keeps its own).
    fn header(&self) -> String {
        let mut header = String::from("module repl v1.0.0;");
        for import in &self.imports {
            header.push(' ');
            header.push_str(import);
        }
        header
    }

    /// Compile the declarations (and `input`, a cell file) and grow the session's runtime with the result. On an
    /// error nothing changes: the published program and the runtime stay as they were.
    fn load(&mut self, input: Option<&str>) -> Result<(), String> {
        let mut decls = String::from(
            "module repl v1.0.0;\nimport \"debug/dumper\" as repl_dumper;\nimport \"io/stdout\" as repl_io;\n",
        );
        for import in &self.imports {
            decls.push_str(import);
            decls.push('\n');
        }
        for decl in &self.decls {
            decls.push_str(decl);
            decls.push('\n');
        }
        // `ReplResources` has a destroy, so the session starts with resources on and a later resource type
        // never changes how earlier frames and code were made (design §1).
        decls.push_str(&format!(
            "object {KEEPER}<T> {{ has value is T; {KEEPER}(:value); }}\n\
             object ReplShow {{ ReplShow(); show(value is any) is void \
             {{ repl_io.Stdout().writeln(repl_dumper.Dumper(value).all()); }} }}\n\
             object ReplResources {{ ReplResources(); destroy {{}} }}\n\
             main() is int {{ let repl_resources = ReplResources(); 0; }}\n"
        ));
        let mut sources = vec![SourceFile::new("<repl>", decls).map_err(|error| format!("{error}\n"))?];
        if let Some(input) = input {
            sources.push(SourceFile::new(INPUT, input).map_err(|error| format!("{error}\n"))?);
        }
        let (module, _) = lower_source(&sources, false).map_err(|outcome| shift_input_lines(&outcome.stderr))?;
        let fresh = codegen::sparrow::generate(&module).map_err(|error| format!("okkama: {error}\n"))?;
        match &mut self.runtime {
            Some(runtime) => {
                let merged = okkama_sparrow::session::extend(&runtime.loaded.program, &fresh)
                    .map_err(|error| format!("okkama: {error}; :reset starts a fresh session\n"))?;
                runtime.extend(merged).map_err(|error| format!("okkama: {}; :reset starts a fresh session\n", error.0))?;
            }
            None => {
                let runtime = Runtime::session(Loaded::verified(fresh).map_err(|error| format!("okkama: {}\n", error.0))?, CAPACITIES).map_err(|error| format!("okkama: {}\n", error.0))?;
                self.runtime = Some(Box::new(runtime));
            }
        }
        Ok(())
    }

    fn statements(&mut self, text: &str) {
        // A closing `;` is optional: it is added when the input then parses (after `if { }` it is not needed,
        // after `xs.map() -> v { }` it is).
        let closed = format!("{text};");
        let (body, shape) = match Shape::of(&closed) {
            Some(found) if !text.ends_with(';') => (closed, found),
            _ => (text.to_string(), Shape::of(text).unwrap_or_default()),
        };
        // An earlier variable declared again is a new variable of the cell.
        let carried: Vec<usize> = (0..self.vars.len()).filter(|index| !shape.lets.contains(&self.vars[*index].name)).collect();
        let mut kept: Vec<String> = carried.iter().map(|index| self.vars[*index].name.clone()).collect();
        for name in &shape.lets {
            if !kept.contains(name) {
                kept.push(name.clone());
            }
        }
        self.cells += 1;
        let cell = format!("ReplCell{}", self.cells);
        let params: Vec<String> = carried.iter().map(|index| format!("{} is {}", self.vars[*index].name, self.vars[*index].ty)).collect();
        let asynchronous = text.contains("await");
        let header = self.header();
        let source = |body: &str| {
            let keeps: Vec<String> = kept.iter().map(|name| format!("{KEEPER}(value: {name})")).collect();
            format!(
                "{} object {cell} {{ {cell}(); {}run({}) is @any {{\n{body}\nlet repl_kept is @any = [{}]; repl_kept; }} }}\n",
                header,
                if asynchronous { "async " } else { "" },
                params.join(", "),
                keeps.join(", ")
            )
        };
        // First with the trailing expression's value shown; if that does not compile (`void`, an action), without
        // it: then the errors, if any, are the input's own.
        let shown = shape.shown(&body).map(|shown| source(&shown));
        let plain = source(&body);
        let loaded = match shown {
            Some(shown) => self.load(Some(&shown)).or_else(|_| self.load(Some(&plain))),
            None => self.load(Some(&plain)),
        };
        if let Err(errors) = loaded {
            eprint!("{errors}");
            return;
        }
        let runtime = self.runtime.as_mut().expect("loaded");
        let run = format!("{cell}.run");
        let Some(entry) = runtime.loaded.program.functions.iter().position(|f| f.symbol == run) else {
            eprintln!("okkama: internal error: the cell `{cell}` was not compiled");
            return;
        };
        // The cell object has no fields and `run` never reads `self`: no instance is made.
        let args: Vec<Word> = std::iter::once(sparrow_core::word::UNBOUND)
            .chain(carried.iter().map(|index| self.vars[*index].value))
            .collect();
        flush();
        let result = runtime.run_function_with(entry, &args, sparrow_core::STACK_BYTES);
        flush();
        let values = match result {
            Ok(values) => values,
            Err(error) => {
                eprintln!("{}", shift_input_lines(&error.0.replace(&run, INPUT)));
                return;
            }
        };
        let mut vars = Vec::new();
        for (name, element) in kept.into_iter().zip(runtime.heap.elements(values).iter().copied().collect::<Vec<_>>()) {
            // An `@any` element is boxed with its type; the box holds the keeper.
            let keeper = runtime.heap.payload::<sparrow_core::ops::AnyBox>(element).map_or(element, |boxed| boxed.value);
            let descriptor = sparrow_core::heap::payload(runtime.heap.header_of(keeper)) as usize;
            let keeper_name = &runtime.loaded.program.objects[descriptor].name;
            let ty = keeper_name
                .split_once('<')
                .and_then(|(_, rest)| rest.strip_suffix('>'))
                .unwrap_or("any")
                .to_string();
            let repr = runtime.heap.layouts().objects[descriptor].first().copied().unwrap_or(Repr::Ref);
            let value = runtime.heap.field(keeper, 0);
            if runtime.heap.reaches_frame_or_closure(value, repr) {
                eprintln!("note: `{name}` ({ty}) is not kept between inputs");
                continue;
            }
            vars.push(Var { name, ty, value });
        }
        if let Err(error) = runtime.session_keep(values) {
            eprintln!("{}", error.0);
        }
        self.vars = vars;
    }
}

/// What the input holds: its top-level `let` names and its last expression.
#[derive(Default)]
struct Shape {
    lets: Vec<String>,
    /// Bytes of the last expression statement: (statement, expression).
    last: Option<((usize, usize), (usize, usize))>,
}

impl Shape {
    /// `None`: the input does not parse (the cell's compilation reports it).
    fn of(body: &str) -> Option<Self> {
        let prefix = "module repl v1.0.0;\nmain() is int {\n";
        let source = SourceFile::new("<shape>", format!("{prefix}{body}\n}}\n")).ok()?;
        let program = syntax::parse(&source, &ParseOptions::default()).ok()?;
        let Some(ast::Item::Action(main)) = program.items.first() else {
            return None;
        };
        let at = |span: okkama_compiler::source::Span| (span.start as usize - prefix.len(), span.end as usize - prefix.len());
        let mut shape = Self::default();
        for stmt in &main.body.stmts {
            if let ast::StmtKind::Let { name, .. } = &stmt.kind
                && !shape.lets.contains(&name.text)
            {
                shape.lets.push(name.text.clone());
            }
        }
        if let Some(stmt) = main.body.stmts.last()
            && let ast::StmtKind::Expr { expr, condition: None } = &stmt.kind
        {
            shape.last = Some((at(stmt.span), at(expr.span)));
        }
        Some(shape)
    }

    /// The input with its last expression kept and shown.
    fn shown(&self, body: &str) -> Option<String> {
        let ((start, end), (expr_start, expr_end)) = self.last?;
        let expr = body.get(expr_start..expr_end)?;
        Some(format!("{}let repl_value = {expr}; ReplShow().show(repl_value);{}", body.get(..start)?, body.get(end..)?))
    }
}

/// The input is complete: brackets balance outside strings, characters and comments.
fn complete(text: &str) -> bool {
    let mut depth = 0i32;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' | '\'' => {
                while let Some(inner) = chars.next() {
                    if inner == '\\' {
                        chars.next();
                    } else if inner == ch {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                for inner in chars.by_ref() {
                    if inner == '\n' {
                        break;
                    }
                }
            }
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    depth <= 0
}

/// Line numbers of the input file as the user sees them: its first line is the header.
fn shift_input_lines(text: &str) -> String {
    let marker = format!("{INPUT}:");
    let mut out = String::new();
    let mut in_input = false;
    for line in text.split_inclusive('\n') {
        if let Some(at) = line.find(&marker) {
            in_input = true;
            let (head, rest) = line.split_at(at + marker.len());
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            match rest[..digits].parse::<u32>() {
                Ok(number) => {
                    out.push_str(head);
                    out.push_str(&number.saturating_sub(1).to_string());
                    out.push_str(&rest[digits..]);
                }
                Err(_) => out.push_str(line),
            }
            continue;
        }
        if line.contains(':') && !line.trim_start().starts_with(|c: char| c.is_ascii_digit() || c == '|') {
            in_input = line.contains(INPUT);
        }
        let trimmed = line.trim_start();
        let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
        if in_input && digits > 0 && trimmed[digits..].starts_with(" |") {
            let indent = &line[..line.len() - trimmed.len()];
            let number: u32 = trimmed[..digits].parse().unwrap_or(1);
            out.push_str(indent);
            out.push_str(&format!("{:>width$}", number.saturating_sub(1), width = digits));
            out.push_str(&trimmed[digits..]);
        } else {
            out.push_str(line);
        }
    }
    out
}
