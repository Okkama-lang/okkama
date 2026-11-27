//! Engine-independent failure records carried on exceptional control-flow edges. A record's kind and
//! message do not change; its cause and secondary links are set as failures are linked (`RefCell`).
use std::{
    cell::RefCell,
    fmt,
    rc::Rc,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Native(Rc<crate::abi::NativeFailure>),
    Thrown {
        object: crate::heap::Ref<crate::heap::Object>,
        name: String,
        message: Option<String>,
    },
    Panic(String),
    Program(String),
}

/// Место кадра на пути отказа: функция программы и номер инструкции в ней.
/// Нумерация сохраняется при генерации Sparrow: инструкции блоков по порядку, терминатор
/// блока — после его инструкций (кодогенератор Sparrow сохраняет её 1:1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Site {
    pub function: u32,
    pub pc: u32,
}

/// An uncaught failure of `main`: its OCS category (`uncaught-error`,
/// `runtime`, `panic`), the error type of a thrown value or `NativeFailure`,
/// its message, and the activations it unwound through, innermost first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uncaught {
    pub category: &'static str,
    pub error_type: Option<String>,
    pub message: String,
    /// Function and instruction numbers; they are shared with OIR, so a
    /// compiler can map them to source (`okkama_compiler::commands::trace::render_trace`).
    pub trace: Vec<Site>,
    /// Activations of the root's unwind past the recorded limit (#1009),
    /// missing between the innermost and the outermost ones of `trace` (#1034).
    pub trace_omitted: u64,
    /// Byte offset after the root and its secondaries in the plain report,
    /// before its cause entries.
    pub trace_offset: usize,
    /// Byte offsets after cause entries, their original unwind stacks and the
    /// activations each left out (#1034). Secondary cleanup failures
    /// deliberately have no diagnostic stack.
    pub cause_traces: Vec<(usize, Vec<Site>, u64)>,
}

impl Uncaught {
    /// Add source or SPB stacks to the plain report, keeping cause stacks
    /// indented under their entries. Only called when traces are requested.
    pub fn render_report(&self, report: &str, render: impl Fn(&[Site]) -> String) -> String {
        let mut output = String::new();
        let mut start = 0;
        let mut traces: Vec<_> = self.cause_traces.iter()
            .map(|(offset, trace, omitted)| (*offset, trace.as_slice(), *omitted, "  "))
            .chain(std::iter::once((self.trace_offset, self.trace.as_slice(), self.trace_omitted, "")))
            .collect();
        traces.sort_by_key(|entry| entry.0);
        for (offset, trace, omitted, indent) in traces {
            output.push_str(&report[start..offset]);
            let mut stack = render(trace);
            // The stack shows the innermost and the outermost activations; the
            // ones between were only counted (#1009, #1034).
            if omitted > 0 && !stack.is_empty() {
                stack = format!(
                    "{}\n... and {omitted} more activations in the middle, not recorded\n",
                    stack.trim_end_matches('\n')
                );
            }
            if !stack.is_empty() {
                output.push('\n');
                for (index, line) in stack.trim_end_matches('\n').split('\n').enumerate() {
                    if index != 0 {
                        output.push('\n');
                    }
                    output.push_str(indent);
                    output.push_str(line);
                }
            }
            start = offset;
        }
        output.push_str(&report[start..]);
        output
    }
}


impl Uncaught {
    /// The category of an uncaught failure by its name, for a reader of a serialized one (the playground, #1125):
    /// one of the names the runtime gives.
    pub fn category_named(name: &str) -> Option<&'static str> {
        ["uncaught-error", "runtime", "panic", "exhausted"].into_iter().find(|known| *known == name)
    }
}

/// `main`'s outcome as a process ends (#1125, the rule of `okkama run`, shared with the playground): its result in
/// 0..255 is the exit status; any other result, or a failure, is status 1 with what standard error says after the
/// program's own output, ending with a newline.
pub fn main_outcome(result: Result<i64, &str>) -> (u8, String) {
    match result {
        Ok(value) => match u8::try_from(value) {
            Ok(code) => (code, String::new()),
            Err(_) => (1, format!("main() must return an int in 0..255, got {value}\n")),
        },
        Err(report) => {
            let mut text = report.to_string();
            if !text.ends_with('\n') {
                text.push('\n');
            }
            (1, text)
        }
    }
}

/// Отказ изменяем на месте: вторичные отказы
/// и причина дописываются в тот же объект, и это видят все его ссылки.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: Kind,
    pub secondary: RefCell<Vec<Rc<Failure>>>,
    pub cause: RefCell<Option<Rc<Failure>>>,
}

impl Failure {
    pub fn new(kind: Kind) -> Rc<Self> {
        Rc::new(Self {
            kind,
            secondary: RefCell::new(Vec::new()),
            cause: RefCell::new(None),
        })
    }

    /// `Rt.secondary`: отказ cleanup при уже идущем отказе — вторичный.
    ///
    /// A failure is never its own secondary, nor twice the same one: a cleanup
    /// that rethrows the failure in flight (a group awaited during unwinding)
    /// would make the graph a cycle, and walks over it would not end.
    pub fn with_secondary(self: &Rc<Self>, cleanup: Rc<Self>) -> Rc<Self> {
        if Rc::ptr_eq(self, &cleanup) {
            return self.clone();
        }
        let mut secondary = self.secondary.borrow_mut();
        if !secondary.iter().any(|known| Rc::ptr_eq(known, &cleanup)) {
            secondary.push(cleanup);
        }
        drop(secondary);
        self.clone()
    }

    pub fn secondaries(&self) -> Vec<Rc<Self>> {
        self.secondary.borrow().clone()
    }

    pub fn cause(&self) -> Option<Rc<Self>> {
        self.cause.borrow().clone()
    }

    /// `Rt.caused`: обработанный отказ — причина в конце цепочки нового.
    pub fn caused(fresh: &Rc<Self>, handled: &Rc<Self>) -> Rc<Self> {
        if !matches!(fresh.kind, Kind::Thrown { .. } | Kind::Native(_)) || Rc::ptr_eq(fresh, handled) {
            return fresh.clone();
        }
        let mut last = fresh.clone();
        loop {
            let next = last.cause();
            match next {
                Some(cause) if Rc::ptr_eq(&cause, handled) => return fresh.clone(),
                Some(cause) => last = cause,
                None => break,
            }
        }
        *last.cause.borrow_mut() = Some(handled.clone());
        fresh.clone()
    }

    pub fn message(&self) -> &str {
        match &self.kind {
            Kind::Native(failure) => &failure.message,
            Kind::Thrown { message, .. } => message.as_deref().unwrap_or(""),
            Kind::Panic(message) | Kind::Program(message) => message,
        }
    }
    pub fn category(&self) -> &'static str {
        match self.kind {
            Kind::Panic(_) => "panic",
            Kind::Program(_) | Kind::Native(_) => "runtime",
            Kind::Thrown { .. } => "uncaught-error",
        }
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&chain_text(self))
    }
}

/// Сколько отказов показывает отчёт: дальше цепочка обрывается строкой.
pub const CHAIN_ENTRIES: usize = 64;

/// Текст неперехваченного отказа без места и стека, как у ядра Sparrow (#270):
/// сам отказ, затем вторичные отказы cleanup и причина, у каждого — свои.
/// Обход в глубину стеком; отказ, уже показанный выше, — ссылкой на номер,
/// так что цикл в графе отказов не зацикливает отчёт.
pub fn chain_text(root: &Failure) -> String {
    let mut output = describe(root);
    let mut shown: std::collections::HashMap<*const Failure, usize> = Default::default();
    shown.insert(root as *const Failure, 0);
    let mut pending = Vec::new();
    push_links(root, &mut pending);
    while let Some((failure, label)) = pending.pop() {
        output.push('\n');
        output.push_str(label);
        output.push_str("\n  ");
        if let Some(index) = shown.get(&Rc::as_ptr(&failure)) {
            output.push_str(&format!("[failure #{index} already shown]"));
            continue;
        }
        if shown.len() == CHAIN_ENTRIES {
            output.push_str("... further failures not shown");
            break;
        }
        shown.insert(Rc::as_ptr(&failure), shown.len());
        output.push_str(&describe(&failure).replace('\n', "\n  "));
        push_links(&failure, &mut pending);
    }
    output
}

/// Причина и вторичные отказы в порядке стека: первыми снимаются вторичные.
fn push_links(failure: &Failure, pending: &mut Vec<(Rc<Failure>, &'static str)>) {
    if let Some(cause) = failure.cause() {
        pending.push((cause, "Caused by:"));
    }
    pending.extend(failure.secondaries().into_iter().rev().map(|secondary| (secondary, "Secondary cleanup failure:")));
}

/// Имя Okkama-типа Error-объекта: последний сегмент имени типа
/// (`async.channel::ChannelClosed` → `ChannelClosed`).
pub fn error_type_name(name: &str) -> String {
    let class = name.replace(['.', ':', '<', '>', ',', ' ', '@', '%', '^'], "$");
    class.rsplit('$').next().unwrap_or(&class).to_string()
}

/// Текст одного отказа в отчёте, общий для движков (#270): `Тип: text()`,
/// `NativeFailure: …` с подробностями хоста, `Panic: …`, `RuntimeError: …`.
pub fn describe(failure: &Failure) -> String {
    match &failure.kind {
        Kind::Native(native) => format!(
            "NativeFailure: {}\ncode: {}\ndomain: {}\noperation: {}",
            native.message, native.code, native.domain, native.operation
        ),
        Kind::Thrown { name, message: Some(message), .. } => format!("{name}: {message}"),
        Kind::Thrown { name, message: None, .. } => format!("Uncaught {name}"),
        Kind::Panic(message) => format!("Panic: {message}"),
        Kind::Program(message) => format!("RuntimeError: {message}"),
    }
}

/// Наибольшее число строк стека в диагностике: длиннее — середина опускается.
const STACK_LINES: usize = 40;

/// Имя функции для стека: символ без номера перегрузки или конструктора
/// (`Bag.new#1` → `Bag.new`); номер анонимного действия остаётся.
pub fn frame_name(symbol: &str) -> &str {
    match symbol.rsplit_once('#') {
        Some((name, number))
            if !name.ends_with("lambda") && !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) =>
        {
            name
        }
        _ => symbol,
    }
}

/// Текст стека вызовов из строк кадров (от места отказа к корню): подряд
/// идущие одинаковые строки (рекурсия) схлопываются, а слишком длинный стек
/// показывается началом и концом.
pub fn stack_text(header: &str, frames: &[String]) -> String {
    stack_text_by(header, frames, String::clone)
}

/// `stack_text` над кадрами любого вида: подряд идущие равные кадры
/// схлопываются, а `render` вызывается только для показанных строк. Стек в
/// миллионы кадров (глубокая рекурсия до переполнения) печатается за время,
/// линейное в числе кадров, без строки на каждый (#241).
pub fn stack_text_by<T: PartialEq>(header: &str, frames: &[T], render: impl Fn(&T) -> String) -> String {
    // (первый кадр серии, её длина)
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        match runs.last_mut() {
            Some((start, count)) if frames[*start] == *frame => *count += 1,
            _ => runs.push((index, 1)),
        }
    }
    let line = |&(start, count): &(usize, usize)| {
        if count > 1 {
            format!("  at {}  (\u{00d7}{count})", render(&frames[start]))
        } else {
            format!("  at {}", render(&frames[start]))
        }
    };
    let mut lines: Vec<String> = Vec::new();
    if runs.len() > STACK_LINES {
        let tail = 10;
        let head = STACK_LINES - tail - 1;
        let omitted = runs.len() - head - tail;
        lines.extend(runs[..head].iter().map(line));
        lines.push(format!("  ... {omitted} more frames ..."));
        lines.extend(runs[runs.len() - tail..].iter().map(line));
    } else {
        lines.extend(runs.iter().map(line));
    }
    let mut out = String::from(header);
    out.push('\n');
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Owned payload roots in a cached or propagating failure, with cycle protection.
pub fn thrown_errors(failure: &Rc<Failure>) -> Vec<crate::Value> {
    let mut errors = Vec::new();
    let mut pending = vec![failure.clone()];
    // The graph may share links or even loop: each failure is looked at once.
    let mut seen = std::collections::HashSet::new();
    while let Some(link) = pending.pop() {
        if !seen.insert(Rc::as_ptr(&link)) {
            continue;
        }
        if let Kind::Thrown { object, .. } = &link.kind {
            errors.push(crate::Value::Object(object.0.clone()));
        }
        pending.extend(link.secondaries().into_iter().rev());
        if let Some(cause) = link.cause() {
            pending.push(cause);
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_names_drop_overload_numbers_but_keep_lambdas() {
        assert_eq!(frame_name("Bag.new#1"), "Bag.new");
        assert_eq!(frame_name("Box<int>.get"), "Box<int>.get");
        assert_eq!(frame_name("lambda#0"), "lambda#0");
        assert_eq!(frame_name("main"), "main");
    }

    #[test]
    fn stack_text_collapses_recursion_and_elides_long_stacks() {
        let frames = ["f (a.ok:1:1)", "f (a.ok:2:1)", "f (a.ok:2:1)", "main (a.ok:3:1)"].map(String::from);
        assert_eq!(
            stack_text("Stack:", &frames),
            "Stack:\n  at f (a.ok:1:1)\n  at f (a.ok:2:1)  (\u{00d7}2)\n  at main (a.ok:3:1)\n"
        );
        let frames: Vec<String> = (0..100).map(|index| format!("f{index}")).collect();
        let text = stack_text("Stack:", &frames);
        assert_eq!(text.lines().count(), 1 + STACK_LINES);
        assert!(text.contains("  ... 61 more frames ...\n"), "{text}");
        assert!(text.ends_with("  at f99\n"), "{text}");
    }

    /// A stack of millions of frames renders only the frames it shows (#241).
    #[test]
    fn stack_text_by_renders_only_shown_frames() {
        let rendered = std::cell::Cell::new(0);
        let mut frames: Vec<u32> = vec![7; 3_000_000];
        frames.extend(0..100);
        let text = stack_text_by("Stack:", &frames, |frame| {
            rendered.set(rendered.get() + 1);
            format!("f{frame}")
        });
        assert_eq!(rendered.get(), STACK_LINES - 1);
        assert!(text.starts_with("Stack:\n  at f7  (\u{00d7}3000000)\n  at f0\n"), "{text}");
        assert!(text.contains("  ... 62 more frames ...\n"), "{text}");
    }
}
