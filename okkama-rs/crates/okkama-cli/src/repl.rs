//! `okkama repl` on Sparrow (#588): one long-lived session, one heap.
//!
//! **Input.** Declarations (`object`, `enum`, `role`, `const`, `native`, `cap`, including
//! `hidden` declarations) and imports accumulate in a `<repl>` file. `test` and `fake`
//! declarations are refused. Statements become a cell: an action `ReplCellN.run(<previous variables>)`
//! in a second file of the same module (spec §5.1), which returns its variables as `ReplKeep<T>` values.
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
Enter statements, expressions, declarations (object, enum, role, const, native,
cap, hidden ones too) or imports. An input that ends inside brackets, a fenced
raw string or a block comment, or on an operator, goes on on the next line.
The value of a trailing expression is dumped; variables live on between inputs.

    :vars     variables and their types
    :decls    declarations and imports so far
    :reset    forget everything (destroys what the variables own)
    :help     this text
    :quit     leave (also Ctrl-D)

Native extensions: --ext PATH loads a trusted library (repeatable);
--no-native disables native extensions, including every --ext.

On a terminal: arrows move the cursor and walk the history (~/.okkama_history),
Ctrl-R searches it. Actions (closures) and futures are not kept between inputs.
";

/// `okkama help repl`.
pub(crate) fn help() -> String {
    format!("okkama repl — read-eval-print loop on Sparrow\n\nUSAGE:\n    okkama repl\n\n{HELP}")
}

struct Var {
    name: String,
    /// The type as `:vars` shows it: the keeper's type argument.
    ty: String,
    /// The type as the next cell writes it: names of other modules through
    /// the namespaces of [`Var::imports`] (#1017: `import … as` hid them).
    param: String,
    /// `(module path, name)` of each name `param` takes from another module.
    imports: Vec<(String, String)>,
    value: Word,
}

#[derive(Default)]
struct Session {
    limits: sparrow_core::budget::Limits,
    imports: Vec<String>,
    decls: Vec<String>,
    vars: Vec<Var>,
    cells: usize,
    runtime: Option<Box<Runtime>>,
    /// The module path of each type name declared in another module, from
    /// the last compilation; a name two modules declare, or the session's own
    /// declarations, is not here.
    origins: std::collections::HashMap<String, String>,
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
/// The REPL's own options (`--ext PATH`, `--no-native`); None for anything
/// else, which `okkama` then reports as a usage error naming the argument.
pub(crate) fn options(args: &[String]) -> Option<sparrow_core::budget::Limits> {
    let mut limits = sparrow_core::budget::Limits::default();
    let mut args = args;
    while let Some((option, rest)) = args.split_first() {
        match option.as_str() {
            "--no-native" => { limits.no_native = true; args = rest; }
            "--ext" if !rest.is_empty() => {
                limits.extension_paths.push(rest[0].clone().into());
                args = &rest[1..];
            }
            _ => return None,
        }
    }
    Some(limits)
}

pub(crate) fn run(limits: sparrow_core::budget::Limits) -> ExitCode {
    let thread = std::thread::Builder::new()
        .name("okkama-repl".into())
        .stack_size(sparrow_core::STACK_BYTES as usize)
        .spawn(move || repl(limits));
    match thread.map(std::thread::JoinHandle::join) {
        Ok(Ok(code)) => code,
        Ok(Err(_)) => ExitCode::from(70),
        Err(error) => {
            eprintln!("okkama: REPL thread: {error}");
            ExitCode::from(70)
        }
    }
}

fn repl(limits: sparrow_core::budget::Limits) -> ExitCode {
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
    let mut session = Session { limits, ..Session::default() };
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
                *self = Self { limits: self.limits.clone(), ..Self::default() };
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
            // Tests and fakes run in test mode only (#1017: they reached the
            // cell and failed as a statement).
            "test" | "fake" => eprintln!("okkama: {first} declarations belong to test mode (`okkama run -t`); the REPL runs none"),
            "object" | "enum" | "role" | "native" | "cap" | "const" | "hidden" => {
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
        let origins = type_origins(&module);
        match &mut self.runtime {
            Some(runtime) => {
                let merged = okkama_sparrow::session::extend(&runtime.loaded.program, &fresh)
                    .map_err(|error| format!("okkama: {error}; :reset starts a fresh session\n"))?;
                runtime.extend(merged).map_err(|error| format!("okkama: {}; :reset starts a fresh session\n", error.0))?;
            }
            None => {
                let runtime = Runtime::session_with_limits(Loaded::verified(fresh).map_err(|error| format!("okkama: {}\n", error.0))?, CAPACITIES, self.limits.clone()).map_err(|error| format!("okkama: {}\n", error.0))?;
                self.runtime = Some(Box::new(runtime));
            }
        }
        self.origins = origins;
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
        let params: Vec<String> = carried.iter().map(|index| format!("{} is {}", self.vars[*index].name, self.vars[*index].param)).collect();
        let asynchronous = text.contains("await");
        let mut header = self.header();
        header.push_str(&namespace_imports(carried.iter().flat_map(|index| self.vars[*index].imports.iter())));
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
        let bytes = if crate::core::spawns_workers(&runtime.loaded.program) {
            match okkama_sparrow::encode(&runtime.loaded.program) {
                Ok(bytes) => bytes,
                Err(error) => { eprintln!("okkama: {error}"); return; }
            }
        } else { Vec::new() };
        let (command, _worker_file) = match crate::core::workers_for(&runtime.loaded.program, &bytes, &runtime.limits) {
            Ok(workers) => workers,
            Err(error) => { eprintln!("okkama: {error}"); return; }
        };
        runtime.worker_command = command;
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
            let (param, imports) = written_type(&ty, &self.origins);
            vars.push(Var { name, ty, param, imports, value });
        }
        if let Err(error) = runtime.session_keep(values) {
            eprintln!("{}", error.0);
        }
        self.vars = vars;
    }
}

/// The module path of each type name an imported module declares, as the IR
/// names them (`io.stdout::Stdout`, `compare::Comparing<Duration>`). A name
/// the session declares itself, or two modules declare, is left out: it
/// stays as written.
fn type_origins(module: &okkama_compiler::ir::Module) -> std::collections::HashMap<String, String> {
    let names = module.objects.iter().map(|object| object.name.as_str())
        .chain(module.enums.iter().map(|decl| decl.name.as_str()))
        .chain(module.variants.iter().map(|decl| decl.name.as_str()))
        .chain(module.roles.iter().map(|decl| decl.name.as_str()));
    let base = |name: &str| name.split('<').next().unwrap_or(name).to_string();
    let mut origins = std::collections::HashMap::new();
    let mut own = std::collections::HashSet::new();
    let mut shared = std::collections::HashSet::new();
    for name in names {
        match name.split_once("::") {
            Some((path, rest)) => {
                let path = path.replace('.', "/");
                let rest = base(rest);
                if origins.get(&rest).is_some_and(|known| *known != path) {
                    shared.insert(rest.clone());
                }
                origins.insert(rest, path);
            }
            None => {
                own.insert(base(name));
            }
        }
    }
    origins.retain(|name, _| !own.contains(name) && !shared.contains(name));
    origins
}

/// `ty` (a keeper's type argument, with plain names) as a cell writes it:
/// each name declared in another module goes through a namespace of the
/// cell's own (`repl_io_stdout.Stdout`), whatever the session's imports call
/// that module; the second value lists the names taken.
fn written_type(ty: &str, origins: &std::collections::HashMap<String, String>) -> (String, Vec<(String, String)>) {
    let mut written = String::new();
    let mut imports = Vec::new();
    let mut rest = ty;
    while let Some(start) = rest.find(|c: char| c.is_ascii_alphabetic() || c == '_') {
        written.push_str(&rest[..start]);
        let word = &rest[start..];
        let end = word.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(word.len());
        let name = &word[..end];
        match origins.get(name) {
            Some(path) => {
                written.push_str(&format!("{}.{name}", namespace(path)));
                if !imports.iter().any(|(known, taken)| known == path && taken == name) {
                    imports.push((path.clone(), name.to_string()));
                }
            }
            None => written.push_str(name),
        }
        rest = &word[end..];
    }
    written.push_str(rest);
    (written, imports)
}

/// The cell's namespace of the module `path`.
fn namespace(path: &str) -> String {
    let mut name = String::from("repl_");
    name.extend(path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }));
    name
}

/// One `import "path" as namespace take …;` per module, for the header.
fn namespace_imports<'a>(taken: impl Iterator<Item = &'a (String, String)>) -> String {
    let mut by_module: std::collections::BTreeMap<&str, Vec<&str>> = std::collections::BTreeMap::new();
    for (path, name) in taken {
        let names = by_module.entry(path).or_default();
        if !names.contains(&name.as_str()) {
            names.push(name);
        }
    }
    by_module
        .into_iter()
        .map(|(path, names)| format!(" import \"{path}\" as {} take {};", namespace(path), names.join(", ")))
        .collect()
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

/// The input is complete: brackets balance outside strings, characters and
/// comments, and no fenced raw string or block comment is left open (#1017:
/// a bracket inside them opened a continuation that glued the next inputs on).
fn complete(text: &str) -> bool {
    let mut depth = 0i32;
    let mut rest = text;
    // The last character outside comments and whitespace.
    let mut last = ' ';
    while let Some(ch) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("```") {
            // The first following triple backtick closes it, as in the lexer.
            match after.find("```") {
                Some(end) => rest = &after[end + 3..],
                None => return false,
            }
            last = '`';
            continue;
        }
        if let Some(after) = rest.strip_prefix("/*") {
            // Block comments do not nest.
            match after.find("*/") {
                Some(end) => rest = &after[end + 2..],
                None => return false,
            }
            continue;
        }
        if let Some(after) = rest.strip_prefix("//") {
            rest = after.find('\n').map_or("", |end| &after[end..]);
            continue;
        }
        rest = &rest[ch.len_utf8()..];
        if !ch.is_whitespace() {
            last = ch;
        }
        match ch {
            '"' | '\'' => {
                let mut chars = rest.char_indices();
                let mut end = rest.len();
                while let Some((at, inner)) = chars.next() {
                    if inner == '\\' {
                        chars.next();
                    } else if inner == ch {
                        end = at + inner.len_utf8();
                        break;
                    }
                }
                rest = &rest[end..];
            }
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    // A line that ends on an operator goes on (#1017: `let total = a +`
    // was compiled at once).
    depth <= 0 && !matches!(last, '+' | '-' | '*' | '/' | '%' | '=' | '<' | '>' | '&' | '|' | '^' | ',' | '.')
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
