//! Intrinsics std `async/*` (OIR-1 §14.2): действия и конструкторы объектов,
//! чьё состояние держит рантайм. Lowering выпускает для них функции без тела
//! `[intrinsic "okkama.<модуль>.<Объект>.<действие>"]` со списком объектов,
//! которые рантайм создаёт (результаты, Error std); тело даёт backend.

use super::ctx::Ctx;
use super::{LoweringError, Result};
use crate::analysis::registry::{CtorShape, ObjectInfo, TypeRef};
use crate::analysis::ty::Ty;
use crate::ir::{self, ObjectId, ResultType, Type};
use crate::source::Span;

/// Объекты std с intrinsic-действиями: модуль и имя.
const OBJECTS: [(&str, &str); 23] = [
    ("async/scope", "FutureScope"),
    ("parallel/map", "ParallelMapWorkers"),
    ("async/lock", "Lock"),
    ("async/lock", "LockGuard"),
    ("async/semaphore", "Semaphore"),
    ("async/semaphore", "Permit"),
    ("async/barrier", "Barrier"),
    ("async/barrier", "BarrierState"),
    ("async/channel", "Channel"),
    ("async/channel", "Sending"),
    ("async/channel", "Receiving"),
    ("async/channel", "SelectedReceiving"),
    ("async/event", "Event"),
    ("async/atomic", "Atomic"),
    ("async/cancellation", "Cancellation"),
    ("async/cancellation", "CancellationToken"),
    ("async/scope", "FutureGroup"),
    ("async/collect", "AllFutures"),
    ("async/collect", "Raced"),
    ("async/collect", "FirstSuccessful"),
    ("async/collect", "FutureCompletions"),
    ("async/timeout", "Within"),
    ("async/timeout", "Before"),
];

/// Whether the runtime holds the state of std object `name` of `module` (#956):
/// such an object is not a value, it compares by identity and cannot key a map.
pub(crate) fn runtime_backed(module: &str, name: &str) -> bool {
    OBJECTS.contains(&(module, name))
}

/// Действие или конструктор объекта.
#[derive(Copy, Clone)]
pub(super) enum Member {
    Method(usize),
    Ctor(usize),
}

/// The parameter names of constructor `ctor`, in order.
fn ctor_params(info: &ObjectInfo<'_>, ctor: usize) -> Vec<String> {
    match &info.ctors[ctor].shape {
        CtorShape::Full { params, .. } => params.iter().map(|(name, _)| name.clone()).collect(),
        CtorShape::Shorthand { fields, .. } => fields.iter().map(|field| info.fields[*field].name.clone()).collect(),
    }
}

/// The intrinsic being lowered, for a diagnostic about its module's shape:
/// where it is declared and how it reads in source (`Barrier.wait`).
struct Site {
    at: Span,
    shown: String,
}

impl Site {
    /// The module of a runtime intrinsic is user input (any source may declare
    /// `module async/barrier`): a shape the runtime cannot work with is a
    /// semantic error at the intrinsic, never an internal one (#1088).
    fn refuse(&self, needs: impl std::fmt::Display) -> LoweringError {
        LoweringError::invalid(self.at, format!("Runtime intrinsic \"{}\" {needs}", self.shown))
    }
}

/// Объекты, которые рантайм создаёт для intrinsic: `Self` — сам объект,
/// `X<T>` — объект модуля с аргументами receiver, `X` — без аргументов,
/// `E.case` — error enum модуля, чей первый case `case` рантайм бросает,
/// `core::ArraySequence` — адаптер `@T` как `Sequencing<T>`.
fn related(name: &str) -> &'static [&'static str] {
    match name {
        "okkama.channel.Channel.new_capacity" => &["Self", "InvalidChannelCapacity.capacity"],
        "okkama.channel.Channel.sending" => &["Sending<T>"],
        "okkama.channel.Channel.receiving" => &["Receiving<T>"],
        "okkama.channel.SelectedReceiving.new_receivers" =>
            &["Self", "EmptyChannelSelection.empty", "DuplicateChannelReceiver.duplicate"],
        "okkama.channel.SelectedReceiving.item" =>
            &["SelectedItem<T>", "ChannelClosed.closed", "ConcurrentChannelSelection.pending"],
        "okkama.channel.Sending.send"
        | "okkama.channel.Receiving.item"
        | "okkama.channel.Sending.close"
        | "okkama.channel.Receiving.close" => &["ChannelClosed.closed"],
        "okkama.channel.Sending.new"
        | "okkama.channel.Receiving.new"
        | "okkama.event.Event.new"
        | "okkama.event.Event.new_ready"
        | "okkama.atomic.Atomic.new_value"
        | "okkama.cancellation.Cancellation.new"
        | "okkama.cancellation.CancellationToken.new"
        | "okkama.scope.FutureGroup.new" => &["Self"],
        "okkama.atomic.Atomic.value"
        | "okkama.atomic.Atomic.replace"
        | "okkama.atomic.Atomic.compare_and_replace" => &["AtomicClosed.closed"],
        "okkama.cancellation.Cancellation.cancelling" => &["CancellationToken"],
        "okkama.scope.FutureGroup.start" => &["FinishedFutureGroup.finished"],
        "okkama.collect.AllFutures.results" => &["core::ArraySequence"],
        "okkama.collect.Raced.winner"
        | "okkama.collect.FirstSuccessful.outcome"
        | "okkama.collect.FutureCompletions.completion" => &["NoFutures.none"],
        "okkama.scope.FutureScope.outcome" => &["FutureGroup<T0>"],
        "okkama.lock.Lock.new" => &["Self"],
        "okkama.lock.Lock.guard" => &["LockGuard"],
        "okkama.semaphore.Semaphore.permit" => &["Permit"],
        "okkama.barrier.Barrier.wait" => &["BrokenBarrier.broken"],
        "okkama.barrier.Barrier.state" => &["BarrierState"],
        "okkama.timeout.Within.outcome" | "okkama.timeout.Before.outcome" => &["TimedOut.elapsed"],
        _ => &[],
    }
}

/// Действия программы, которые вызывает intrinsic: (значение, действие,
/// арность, типы аргументов). Значение — поле receiver или `^N`: тип
/// результата N-го. Типы аргументов (те же ссылки) выбирают перегрузку:
/// `minus(MonotonicInstant)` даёт `Duration`, `minus(Duration)` — момент (#215).
/// Действие `Cap::field` читает поле cap: его функция, как действие без
/// аргументов, принимает одно значение и отдаёт поле.
type Helper = (&'static str, &'static str, usize, &'static [&'static str]);

fn helpers(name: &str) -> &'static [Helper] {
    match name {
        "okkama.timeout.Within.outcome" => &[("limit", "Span::seconds", 0, &[]), ("clock", "wait", 1, &[])],
        "okkama.timeout.Before.outcome" => &[
            ("clock", "monotonic_instant", 0, &[]),
            ("deadline", "minus", 1, &["^0"]),
            ("^1", "Span::seconds", 0, &[]),
            ("clock", "wait", 1, &[]),
        ],
        _ => &[],
    }
}

impl Ctx<'_, '_> {
    /// Имя intrinsic члена объекта, если его даёт рантайм.
    pub(super) fn intrinsic_name(&self, object: ObjectId, member: Member) -> Option<String> {
        let decl = self.objects[object.0].decl;
        let info = &self.reg.objects[decl.0];
        let module = self.reg.modules[info.module].name.as_str();
        if !OBJECTS.contains(&(module, info.name.as_str())) {
            return None;
        }
        let family = module.rsplit('/').next().unwrap_or(module);
        let action = match member {
            Member::Method(method) => info.methods[method].name.clone(),
            Member::Ctor(ctor) => std::iter::once("new".to_string())
                .chain(ctor_params(info, ctor))
                .collect::<Vec<_>>()
                .join("_"),
        };
        let name = format!("okkama.{family}.{}.{action}", info.name);
        ir::INTRINSICS.contains(&name.as_str()).then_some(name)
    }

    /// Объекты, которые рантайм создаёт для intrinsic `name` объекта `object`.
    fn intrinsic_objects(
        &mut self,
        object: ObjectId,
        name: &str,
        site: &Site,
        span: Span,
    ) -> Result<Vec<ObjectId>> {
        let decl = self.objects[object.0].decl;
        let module = self.reg.objects[decl.0].module;
        let args: Vec<Ty> = self.objects[object.0]
            .env
            .iter()
            .map(|(_, ty)| ty.clone())
            .collect();
        let mut objects = Vec::new();
        for entry in related(name) {
            let id = match *entry {
                "Self" => object,
                "core::ArraySequence" => {
                    let adapter = self.reg.adapters.array_sequence.ok_or_else(|| {
                        LoweringError::internal(span, "no Sequencing role")
                    })?;
                    let element = args.first().cloned().unwrap_or(Ty::Never);
                    self.object_instance(adapter, vec![element.clone(), element], span)?
                }
                other if other.contains('.') && !other.starts_with("core::") => {
                    // Рантайм создаёт error enum как объект его первого
                    // случая с полями payload (#290): case обязан быть первым.
                    let (name, case) = other.split_once('.').expect("enum case");
                    let found = match self.reg.modules[module].own.types.get(name).copied() {
                        Some(TypeRef::Enum(found)) => Some(found),
                        _ => None,
                    }
                    .filter(|found| {
                        let info = &self.reg.enums[found.0];
                        info.error && info.params.is_empty() && info.cases.first().is_some_and(|c| c == case)
                    });
                    let Some(found) = found else {
                        return Err(site.refuse(format_args!(
                            "throws {other}: its module must declare {name} as an error enum \
                             without type parameters whose first case is {case}"
                        )));
                    };
                    match self.ir_type(&Ty::Variant(found, Vec::new()), span)? {
                        Type::Variant(id) => self.variants[id.0].cases[0].1,
                        _ => unreachable!("error enums lower to variants"),
                    }
                }
                other => {
                    // `X<T>` — все аргументы receiver, `X<T0>` — только первый.
                    let (name, arity) =
                        match (other.strip_suffix("<T>"), other.strip_suffix("<T0>")) {
                            (Some(name), _) => (name, args.len()),
                            (_, Some(name)) => (name, 1),
                            _ => (other, 0),
                        };
                    let found = match self.reg.modules[module].own.types.get(name).copied() {
                        Some(TypeRef::Object(found)) if self.reg.objects[found.0].params.len() == arity => found,
                        _ => {
                            return Err(site.refuse(format_args!(
                                "creates {other}: its module must declare object {name} with {arity} type parameter(s)"
                            )));
                        }
                    };
                    let type_args = args.iter().take(arity).cloned().collect();
                    self.object_instance(found, type_args, span)?
                }
            };
            objects.push(id);
        }
        Ok(objects)
    }

    /// Переходники действий программы для intrinsic `name` (см. [`helpers`]).
    fn intrinsic_helpers(
        &mut self,
        object: ObjectId,
        name: &str,
        site: &Site,
        span: Span,
    ) -> Result<Vec<ir::FuncId>> {
        let decl = self.objects[object.0].decl;
        let env = self.objects[object.0].env.clone();
        let reg = self.reg;
        let info = &reg.objects[decl.0];
        let mut found: Vec<(ir::FuncId, Type)> = Vec::new();
        for (path, action, arity, args) in helpers(name) {
            let resolve = |this: &mut Self, path: &str| -> Result<Type> {
                if let Some(index) = path.strip_prefix('^') {
                    let index: usize = index.parse().expect("helper index");
                    return Ok(found[index].1.clone());
                }
                let field = info
                    .fields
                    .iter()
                    .find(|field| field.name == path)
                    .ok_or_else(|| site.refuse(format_args!("reads field {path}: object {} must declare it", info.name)))?;
                this.ir_type(&field.ty.subst(&env), span)
            };
            let receiver = resolve(self, path)?;
            if let Some((cap_name, field_name)) = action.split_once("::") {
                found.push(self.intrinsic_cap_field(&receiver, cap_name, field_name, site, span)?);
                continue;
            }
            let params = if args.is_empty() {
                None
            } else {
                Some(args.iter().map(|arg| resolve(self, arg)).collect::<Result<Vec<_>>>()?)
            };
            let callable = match &receiver {
                Type::Object(target) => self.member_method(*target, action, *arity, params.as_deref(), span)?.is_some(),
                Type::Role(role) => self.roles[role.0]
                    .methods
                    .iter()
                    .any(|method| method.name == *action && method.params.len() == *arity),
                _ => false,
            };
            if !callable {
                return Err(site.refuse(format_args!(
                    "calls {action} with {arity} argument(s) on {path}: its type must have that action"
                )));
            }
            let func = self.member_thunk(receiver.clone(), action, *arity, params.clone(), false);
            // Тип результата действия: для цепочки `^N`.
            let result = match &receiver {
                Type::Object(target) => {
                    let target_env = self.objects[target.0].env.clone();
                    let target_info = &reg.objects[self.objects[target.0].decl.0];
                    let method = self
                        .member_method(*target, action, *arity, params.as_deref(), span)?
                        .map(|method| &target_info.methods[method]);
                    match method.map(|method| method.result.clone()) {
                        Some(result) => match self.ir_result(&result, &target_env, span)? {
                            ResultType::Value(ty) => ty,
                            ResultType::Void => Type::Never,
                        },
                        None => Type::Never,
                    }
                }
                Type::Role(role) => match self.roles[role.0]
                    .methods
                    .iter()
                    .find(|method| method.name == *action)
                    .map(|method| method.result.clone())
                {
                    Some(ResultType::Value(ty)) => ty,
                    _ => Type::Never,
                },
                _ => Type::Never,
            };
            found.push((func, result));
        }
        Ok(found.into_iter().map(|(func, _)| func).collect())
    }

    /// Функция поля `cap_name::field_name` объекта `receiver` и тип поля.
    fn intrinsic_cap_field(
        &mut self,
        receiver: &Type,
        cap_name: &str,
        field_name: &str,
        site: &Site,
        span: Span,
    ) -> Result<(ir::FuncId, Type)> {
        let missing = || {
            site.refuse(format_args!("reads {cap_name}::{field_name}: its receiver must be an object with that cap field"))
        };
        let Type::Object(target) = receiver else {
            return Err(missing());
        };
        let decl = self.objects[target.0].decl;
        let env = self.objects[target.0].env.clone();
        let args = env.iter().map(|(_, ty)| ty.clone()).collect();
        let reg = self.reg;
        let info = &reg.objects[decl.0];
        let cap = info.caps.iter().position(|cap| cap.name == cap_name).ok_or_else(missing)?;
        let field = info.caps[cap]
            .fields
            .iter()
            .position(|(name, ..)| name == field_name)
            .ok_or_else(missing)?;
        let ty = self.ir_type(&info.caps[cap].fields[field].1.subst(&env), span)?;
        let id = self.cap_instance(decl, args, cap, span)?;
        Ok((self.caps[id.0].2[field].1, ty))
    }

    /// Функция intrinsic: параметры (receiver первым у действия), результат,
    /// владелец; тела нет.
    pub(super) fn lower_intrinsic(
        &mut self,
        symbol: String,
        object: ObjectId,
        member: Member,
        name: String,
    ) -> Result<ir::Function> {
        let decl = self.objects[object.0].decl;
        let env = self.objects[object.0].env.clone();
        let span = self.objects[object.0].span;
        let reg = self.reg;
        let info = &reg.objects[decl.0];
        let site = match member {
            Member::Method(method) => Site {
                at: info.methods[method].decl.map_or(span, |decl| decl.span),
                shown: format!("{}.{}", info.name, info.methods[method].name),
            },
            Member::Ctor(ctor) => Site {
                at: info.ctors[ctor].span,
                shown: format!("{}({})", info.name, ctor_params(info, ctor).join(", ")),
            },
        };
        let mut values = Vec::new();
        let mut attrs = Vec::new();
        let result = match member {
            Member::Method(method) => {
                let target = &info.methods[method];
                values.push(Type::Object(object));
                for (_, ty) in &target.params {
                    values.push(self.ir_type(&ty.subst(&env), span)?);
                }
                attrs.push(ir::Attr::Method(object));
                if target.is_async {
                    attrs.push(ir::Attr::Async);
                }
                self.ir_result(&target.result, &env, span)?
            }
            Member::Ctor(ctor) => {
                let types: Vec<Ty> = match &info.ctors[ctor].shape {
                    CtorShape::Full { params, .. } => {
                        params.iter().map(|(_, ty)| ty.clone()).collect()
                    }
                    CtorShape::Shorthand { fields, .. } => fields
                        .iter()
                        .map(|field| info.fields[*field].ty.clone())
                        .collect(),
                };
                for ty in types {
                    values.push(self.ir_type(&ty.subst(&env), span)?);
                }
                attrs.push(ir::Attr::Ctor(object));
                ResultType::Value(Type::Object(object))
            }
        };
        let objects = self.intrinsic_objects(object, &name, &site, span)?;
        let helpers = self.intrinsic_helpers(object, &name, &site, span)?;
        attrs.push(ir::Attr::Intrinsic {
            name,
            objects,
            helpers,
        });
        let params = (0..values.len()).map(ir::ValueId).collect();
        Ok(ir::Function {
            symbol,
            params,
            result,
            attrs,
            values,
            blocks: Vec::new(),
            span,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{SourceFile, ir, lowering::{self, LoweringError}, syntax};

    fn lowered(text: &str) -> (String, Result<ir::Module, LoweringError>) {
        let source = SourceFile::new("intrinsics.ok", text).unwrap();
        let program = syntax::parse(&source, &syntax::ParseOptions::default()).unwrap();
        (text.to_string(), lowering::lower_program(&syntax::prune::strip_test_items(program)))
    }

    /// The module lowers to OIR with the intrinsic `name`.
    fn accepted(text: &str, name: &str) {
        let module = lowered(text).1.unwrap_or_else(|error| panic!("{error:?}"));
        let found = module.functions.iter().flat_map(|function| &function.attrs).any(|attr| {
            matches!(attr, ir::Attr::Intrinsic { name: found, .. } if found == name)
        });
        assert!(found, "no intrinsic {name}");
    }

    /// The module is refused with a semantic error at `at` that says `needs`.
    fn refused(text: &str, at: &str, needs: &str) {
        let (text, result) = lowered(text);
        let error = result.unwrap_err();
        assert!(matches!(error, LoweringError::Invalid { .. }), "{error:?}");
        assert!(error.message().contains(needs), "{}", error.message());
        assert!(text[error.span().start as usize..].starts_with(at), "{error:?}");
    }

    const BARRIER: &str = "module async/barrier v1.0.0;\n\
        object Barrier { has amount is int; Barrier(parties is int) { amount = parties; }\n\
        async wait() is int { panic \"intrinsic\"; } }\n";

    /// #1088: the runtime throws the first case of a module's error enum; a
    /// module without it is refused at the intrinsic, never an internal error.
    #[test]
    fn an_intrinsic_throws_the_first_case_of_a_declared_error_enum() {
        accepted(&format!("{BARRIER}enum BrokenBarrier is Error = (broken);"), "okkama.barrier.Barrier.wait");
        let needs = "\"Barrier.wait\" throws BrokenBarrier.broken: its module must declare BrokenBarrier as an error enum";
        for shape in ["", "enum BrokenBarrier is Error = (other, broken);", "enum BrokenBarrier = (broken);",
                      "object BrokenBarrier { BrokenBarrier(); }"] {
            refused(&format!("{BARRIER}{shape}"), "async wait()", needs);
        }
    }

    const CHANNEL: &str = "module async/channel v1.0.0;\n\
        enum InvalidChannelCapacity is Error = (capacity(capacity is int));\n";

    /// `Sending<T>`: the runtime makes the result object with every type
    /// argument of the receiver, so both declare as many parameters.
    #[test]
    fn an_intrinsic_result_object_takes_the_receivers_type_parameters() {
        let program = |params: &str, args: &str| {
            format!("{CHANNEL}object Channel<{params}> {{ Channel(capacity is int) {{}} sending() is Sending<T> {{ Sending(); }} }}\n\
                object Sending<T> {{ Sending() {{}} }}\n\
                main() is int {{ let c is Channel<{args}> = Channel(capacity: 1); let s = c.sending(); 0; }}")
        };
        accepted(&program("T", "int"), "okkama.channel.Channel.sending");
        refused(&program("T", "int").replace("enum InvalidChannelCapacity is Error", "enum InvalidChannelCapacity"),
            "Channel(capacity is int)", "\"Channel(capacity)\" throws InvalidChannelCapacity.capacity");
        refused(&program("T, U", "int, bool"), "sending()",
            "\"Channel.sending\" creates Sending<T>: its module must declare object Sending with 2 type parameter(s)");
    }

    /// `Within.outcome` reads `limit::Span::seconds` and calls `clock.wait(…)`.
    #[test]
    fn an_intrinsic_helper_needs_its_fields_cap_and_action() {
        let program = |fields: &str| {
            format!("module async/timeout v1.0.0;\n\
                object Span5 {{ Span5(); cap Span {{ seconds is int {{ 5; }} }} }}\n\
                object Clock {{ Clock(); async wait(seconds is int) is void {{}} }}\n\
                object Still {{ Still(); }}\n\
                object Within<T> {{ has origin is Future<T>; {fields} Within(origin is Future<T>) {{ self.origin = origin; }}\n\
                async outcome() is T {{ throw TimedOut.elapsed; }} }}\n\
                enum TimedOut is Error = (elapsed);\n\
                object Work {{ Work(); async one() is int {{ 1; }} }}\n\
                async main() is int {{ await Within(Work().one()).outcome(); }}")
        };
        accepted(&program("has limit is Span5 = Span5(); has clock is Clock = Clock();"), "okkama.timeout.Within.outcome");
        refused(&program("has clock is Clock = Clock();"), "async outcome()", "\"Within.outcome\" reads field limit: object Within must declare it");
        refused(&program("has limit is int = 5; has clock is Clock = Clock();"), "async outcome()",
            "\"Within.outcome\" reads Span::seconds: its receiver must be an object with that cap field");
        refused(&program("has limit is Span5 = Span5(); has clock is Still = Still();"), "async outcome()",
            "\"Within.outcome\" calls wait with 1 argument(s) on clock: its type must have that action");
    }
}
