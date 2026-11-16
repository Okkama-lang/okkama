//! #601 V6: plain await in SPB 17. The async block of the registry (Champ draft 3 §14.1), frame layouts (D21),
//! the codec forms, and the load gate's async rules (draft 3 §15). Positive programs follow the coro goldens of
//! `okkama-oir/tests/fixtures/async/coro` after low: no step of a close suspends in 0.6.0, so only awaits and
//! their unwind adapters are bracketed. Each negative is valid up to the rule it names.
use super::*;
use super::load_tests::refresh;
use std::collections::BTreeMap;

use Argument as A;
use Opcode as O;
use ServiceKind as K;

pub(super) const FUTURE_INT: Type = Type::Future(0);
pub(super) const FUTURE_VOID: Type = Type::Future(1);
/// Scopes: 0 is @work's, 1 is @main's, 2 is a child of @main that no instruction opens, 3 is @consume's
/// and 4 is @make's. Each scope's boundary has its index.
pub(super) const WORK: u32 = 0;
pub(super) const MAIN: u32 = 1;
pub(super) const INNER: u32 = 2;
pub(super) const CONSUME: u32 = 3;
pub(super) const MAKE: u32 = 4;

pub(super) fn l(location: Location) -> Argument {
    A::Location(location)
}

pub(super) fn p(slot: u32) -> Source {
    Source::ProducedSlot(slot)
}

pub(super) fn s(location: Location) -> Source {
    Source::Location(location)
}

pub(super) fn value(ty: Type) -> FrameSlot {
    FrameSlot { ty: ProducedType::Value(ty), linear: false }
}

pub(super) fn service(kind: ServiceKind) -> FrameSlot {
    FrameSlot { ty: ProducedType::Service(kind), linear: kind.linear() }
}

/// Builds one function block by block; edge targets stay labels until `finish`.
pub(super) struct Body {
    pub(super) f: Function,
    labels: BTreeMap<&'static str, u32>,
    targets: Vec<(usize, usize, &'static str, Vec<Source>)>,
    pub(super) sites: BTreeMap<u32, u32>,
    /// A counterexample: closes run the empty settle form before settling their duties.
    settle_empty_first: bool,
    /// How a cancelling close delivers its deferred cancellation: the D18 form or one counterexample.
    delivery: Delivery,
    /// Executors emitted so far, for their block labels.
    executors: usize,
    /// Scopes whose closes settle their dynamic service entries (ABI D25.3), and where.
    pub(super) dynamic: BTreeMap<u32, Dynamic>,
}

/// Where a close settles its dynamic service entries: after the static duties (D25.3), or one counterexample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Dynamic {
    #[default]
    AfterStatic,
    /// Before the static settle step.
    BeforeStatic,
    /// Twice after the static duties.
    Twice,
}

/// A close with `cancel=true` settles, runs its plan, delivers, then runs the aborted reserve's executor plan
/// (D18, decision L2 of #601 V7). The other variants each break one step of that form.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Delivery {
    #[default]
    AfterPlan,
    /// Delivers before the settle phase.
    BeforeSettle,
    /// Delivers after the settle phase, before its only plan.
    BeforePlan,
    /// Delivers twice after the plan.
    Twice,
    /// Delivers after the plan and finishes without the aborted reserve's plan.
    NoSecondPlan,
    /// Runs the aborted reserve's plan without the executor.
    DirectSecondPlan,
    /// Runs a third plan after the aborted reserve's.
    ThirdPlan,
    /// Runs a second plan without delivering.
    UndeliveredSecondPlan,
}

impl Body {
    pub(super) fn new(symbol: &str, entry_abi: EntryAbi, results: Vec<Type>) -> Self {
        let f = Function {
            symbol: symbol.into(), entry_abi, registers: vec![], services: vec![], params: vec![], results,
            blocks: vec![], proofs: FunctionProofs::default(), code: vec![],
        };
        Self { f, labels: BTreeMap::new(), targets: vec![], sites: BTreeMap::new(), settle_empty_first: false,
            delivery: Delivery::AfterPlan, executors: 0, dynamic: Default::default() }
    }

    /// An async body: its only parameter is its frame.
    pub(super) fn asynchronous(symbol: &str, layout: u32, results: Vec<Type>) -> (Self, Location) {
        let mut body = Self::new(symbol, EntryAbi::AsyncBody { layout }, results);
        let frame = body.service(K::Frame);
        body.f.params = vec![frame];
        (body, frame)
    }

    pub(super) fn register(&mut self, ty: Type) -> Location {
        self.f.registers.push(ty);
        Location::Register(self.f.registers.len() as u32 - 1)
    }

    pub(super) fn service(&mut self, kind: ServiceKind) -> Location {
        self.f.services.push(kind);
        Location::Service(self.f.services.len() as u32 - 1)
    }

    pub(super) fn pc(&self) -> u32 {
        self.f.code.len() as u32
    }

    pub(super) fn block(&mut self, label: &'static str, params: &[Location]) {
        self.labels.insert(label, self.pc());
        self.f.blocks.push(Block { pc: self.pc(), params: params.to_vec() });
    }

    pub(super) fn next(&mut self, op: Opcode, arguments: Vec<Argument>, outputs: Vec<Location>) {
        self.f.code.push(Instruction::Champ { op, arguments, flow: Flow::Next(outputs) });
    }

    /// One output of a new location of `kind`.
    pub(super) fn emit(&mut self, op: Opcode, arguments: Vec<Argument>, kind: ServiceKind) -> Location {
        let output = self.service(kind);
        self.next(op, arguments, vec![output]);
        output
    }

    pub(super) fn constant(&mut self, constant: Constant, ty: Type) -> Location {
        let output = self.register(ty);
        let Location::Register(id) = output else { unreachable!() };
        self.f.code.push(Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(id),
            immediate: Immediate::Constant(constant) });
        output
    }

    pub(super) fn outcomes(&mut self, op: Opcode, arguments: Vec<Argument>,
        edges: Vec<(Tag, &'static str, Vec<Source>)>)
    {
        let pc = self.f.code.len();
        let mut outcomes = Vec::new();
        for (ordinal, (tag, label, sources)) in edges.into_iter().enumerate() {
            self.targets.push((pc, ordinal, label, sources));
            outcomes.push(Outcome { tag, edge: Edge { target_pc: 0, assignments: vec![] } });
        }
        self.f.code.push(Instruction::Champ { op, arguments, flow: Flow::Outcomes(outcomes) });
    }

    /// A branch on a bool register to `yes` or `no`.
    pub(super) fn cond(&mut self, condition: Location, yes: &'static str, no: &'static str) {
        let Location::Register(condition) = condition else { panic!("a condition is a register") };
        let pc = self.f.code.len();
        self.targets.push((pc, 0, yes, vec![]));
        self.targets.push((pc, 1, no, vec![]));
        let edge = Edge { target_pc: 0, assignments: vec![] };
        self.f.code.push(Instruction::Cond { condition, yes: edge.clone(), no: edge });
    }

    pub(super) fn br(&mut self, label: &'static str, sources: Vec<Source>) {
        self.targets.push((self.f.code.len(), 0, label, sources));
        self.f.code.push(Instruction::Br(Edge { target_pc: 0, assignments: vec![] }));
    }

    pub(super) fn ret(&mut self) {
        self.f.code.push(Instruction::Ret(vec![]));
    }

    pub(super) fn selection(&mut self) -> u32 {
        self.f.proofs.selections.push(Selection { locations: vec![] });
        self.f.proofs.selections.len() as u32 - 1
    }

    /// `frame.save` of continuation `k`, then the suspending operation at the site the layout names.
    pub(super) fn save(&mut self, frame: Location, k: u32, values: &[Location]) -> Location {
        let mut arguments = vec![l(frame), A::Continuation(k)];
        arguments.extend(values.iter().copied().map(l));
        let saved = self.emit(O::FrameSave, arguments, K::Frame);
        self.sites.insert(k, self.pc());
        saved
    }

    /// `frame.resume` of continuation `k`: the frame, then the saved values of `types`.
    pub(super) fn resume(&mut self, frame: Location, k: u32, types: &[ProducedType]) -> (Location, Vec<Location>) {
        let resumed = self.service(K::Frame);
        let values = types.iter().map(|ty| match *ty {
            ProducedType::Value(ty) => self.register(ty),
            ProducedType::Service(kind) => self.service(kind),
        }).collect::<Vec<_>>();
        let mut outputs = vec![resumed];
        outputs.extend(&values);
        self.next(O::FrameResume, vec![l(frame), A::Continuation(k)], outputs);
        (resumed, values)
    }

    /// A close with a direct plan over an empty inventory, or the executor of a merge plan, and the D18
    /// envelope `failures`. It settles `duties` of a new schedule in order; when the envelope declares a deferred
    /// cancellation, it delivers it after the plan and runs the aborted reserve's executor plan. Returns the
    /// cleanup.finish outputs.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn close(&mut self, state: Location, pending: Option<Location>, failure: Option<Location>, scope: u32,
        duties: &[Location], failures: u32, merge: bool) -> (Location, Location, Location)
    {
        let proofs = &mut self.f.proofs;
        let close = proofs.closes.len() as u32;
        let envelope = proofs.envelopes.len() as u32;
        proofs.envelopes.push(Envelope { failures });
        proofs.closes.push(CloseContract { begin_pc: self.f.code.len() as u32, boundary: scope,
            reserve: pending.is_some(), active_failure: failure.is_some(), envelope });
        let schedule = proofs.schedules.len() as u32;
        proofs.schedules.push(Schedule { services: duties.to_vec() });
        let mut arguments = vec![l(state)];
        arguments.extend(pending.into_iter().chain(failure).map(l));
        arguments.extend([A::Boundary(scope), A::Envelope(envelope)]);
        let mut cleanup = self.emit(O::CleanupBegin, arguments, K::Cleanup);
        let cancel = failures & ENVELOPE_CANCEL != 0;
        let delivery = if cancel { self.delivery } else { Delivery::AfterPlan };
        if cancel && delivery == Delivery::BeforeSettle {
            cleanup = self.deliver(cleanup);
        }
        let dynamic = self.dynamic.get(&scope).copied();
        if dynamic == Some(Dynamic::BeforeStatic) {
            cleanup = self.emit(O::CleanupSettleDynamic, vec![l(cleanup)], K::Cleanup);
        }
        if duties.is_empty() || self.settle_empty_first {
            cleanup = self.emit(O::CleanupSettle, vec![l(cleanup), A::Schedule(schedule)], K::Cleanup);
        }
        for &duty in duties {
            cleanup = self.emit(O::CleanupSettle, vec![l(cleanup), A::Schedule(schedule), l(duty)], K::Cleanup);
        }
        let after = match dynamic {
            Some(Dynamic::AfterStatic) => 1,
            Some(Dynamic::Twice) => 2,
            _ => 0,
        };
        for _ in 0..after {
            cleanup = self.emit(O::CleanupSettleDynamic, vec![l(cleanup)], K::Cleanup);
        }
        if cancel && delivery == Delivery::BeforePlan {
            cleanup = self.deliver(cleanup);
        }
        cleanup = self.plan(cleanup, close, pending.is_some(), merge);
        let after_plan = matches!(delivery, Delivery::AfterPlan | Delivery::Twice | Delivery::NoSecondPlan
            | Delivery::DirectSecondPlan | Delivery::ThirdPlan);
        if cancel && after_plan {
            cleanup = self.deliver(cleanup);
            if delivery == Delivery::Twice {
                cleanup = self.deliver(cleanup);
            }
        }
        if cancel && !matches!(delivery, Delivery::NoSecondPlan | Delivery::BeforePlan) {
            // The aborted reserve's plan: its inventory is decided at run time, so it runs the executor.
            cleanup = self.plan(cleanup, close, pending.is_some(), delivery != Delivery::DirectSecondPlan);
            if delivery == Delivery::ThirdPlan {
                cleanup = self.plan(cleanup, close, pending.is_some(), true);
            }
        }
        let outputs = [self.service(K::Closed), self.service(K::FailureState), self.service(K::ExitState)];
        self.next(O::CleanupFinish, vec![l(cleanup)], outputs.to_vec());
        (outputs[0], outputs[1], outputs[2])
    }

    pub(super) fn deliver(&mut self, cleanup: Location) -> Location {
        self.emit(O::CleanupDeliverCancel, vec![l(cleanup)], K::Cleanup)
    }

    /// One plan of close `close` and its evidence, then its executor when it merges. Returns the cleanup state
    /// after the plan has run.
    pub(super) fn plan(&mut self, cleanup: Location, close: u32, reserve: bool, merge: bool) -> Location {
        let selection = self.selection();
        let proofs = &mut self.f.proofs;
        proofs.order_witnesses.push(OrderWitness { creation_pcs: vec![] });
        let plan = proofs.plans.len() as u32;
        proofs.plans.push(Plan { merge, selection, order_witness: proofs.order_witnesses.len() as u32 - 1 });
        let mut sources = vec![MergeSource::Scope];
        if reserve {
            sources.push(MergeSource::Reserve);
        }
        proofs.cleanup_plans.push(CleanupPlan { plan, reserve, close, reserve_empty: !merge,
            sources, slots: vec![], preserves: vec![], joins: vec![], suffixes: vec![] });
        let planned = self.emit(O::CleanupPlan, vec![l(cleanup), A::Plan(plan)], K::Cleanup);
        if merge { self.executor(planned) } else { planned }
    }

    /// The merge executor: destroy selected keys until done, capturing failures. Returns the done cleanup.
    pub(super) fn executor(&mut self, cleanup: Location) -> Location {
        let [looping, selected, key, destroyed, failure_state, failed, failure, recorded, done] = [
            self.service(K::Cleanup), self.service(K::Cleanup), self.service(K::StableKey), self.service(K::Cleanup),
            self.service(K::FailureState), self.service(K::Cleanup), self.register(Type::Failure),
            self.service(K::Cleanup), self.service(K::Cleanup),
        ];
        // Each executor has its own blocks; the labels live as long as the test.
        let label = |name: &str, index: usize| -> &'static str { Box::leak(format!("{name}{index}").into_boxed_str()) };
        let index = self.executors;
        self.executors += 1;
        let [looping_label, destroy_label, failed_label, finish_label] =
            ["x_loop", "x_destroy", "x_failed", "x_finish"].map(|name| label(name, index));
        self.br(looping_label, vec![s(cleanup)]);
        self.block(looping_label, &[looping]);
        self.outcomes(O::CleanupNext, vec![l(looping)],
            vec![(Tag::Selected, destroy_label, vec![p(0), p(1)]), (Tag::Done, finish_label, vec![p(0)])]);
        self.block(destroy_label, &[selected, key]);
        self.next(O::CleanupDestroy, vec![l(selected), l(key)], vec![destroyed, failure_state]);
        self.outcomes(O::FailureDispatch, vec![l(failure_state)],
            vec![(Tag::None, looping_label, vec![s(destroyed)]), (Tag::Some, failed_label, vec![s(destroyed), p(0)])]);
        self.block(failed_label, &[failed, failure]);
        self.next(O::CleanupFailure, vec![l(failed), l(failure)], vec![recorded]);
        self.br(looping_label, vec![s(recorded)]);
        self.block(finish_label, &[done]);
        done
    }

    pub(super) fn dispatch(&mut self, finished: (Location, Location, Location), flags: [bool; 3],
        edges: Vec<(Tag, &'static str, Vec<Source>)>)
    {
        let (closed, failure, exit) = finished;
        let mut arguments = vec![l(closed), l(failure), l(exit)];
        arguments.extend(flags.map(A::Flag));
        self.outcomes(O::ExitDispatch, arguments, edges);
    }

    pub(super) fn finish(self) -> Function {
        let mut f = self.finish_unchecked();
        refresh(&mut f);
        f
    }

    /// Resolve the edges only; the proof maps stay empty.
    pub(super) fn finish_unchecked(mut self) -> Function {
        for (pc, ordinal, label, sources) in std::mem::take(&mut self.targets) {
            let target = self.labels[label];
            let params = self.f.blocks.iter().find(|block| block.pc == target).unwrap().params.clone();
            let edge = match &mut self.f.code[pc] {
                Instruction::Br(edge) => edge,
                Instruction::Champ { flow: Flow::Outcomes(outcomes), .. } => &mut outcomes[ordinal].edge,
                Instruction::Cond { yes, .. } if ordinal == 0 => yes,
                Instruction::Cond { no, .. } => no,
                _ => unreachable!("edge"),
            };
            edge.target_pc = target;
            edge.assignments = params.into_iter().zip(sources)
                .map(|(destination, source)| Assignment { destination, source }).collect();
        }
        self.f
    }
}

/// The program around async bodies and their layouts; scopes as the constants above name them. A `main` with an
/// input packet is an ordinary async body: a fixed root (the entry, a test) starts empty (#667), which
/// `fixed_root_program` builds.
pub(super) fn program(functions: Vec<Function>, frames: Vec<FrameLayout>, entry: Option<u32>) -> Program {
    let scopes = [(0, None), (1, None), (1, Some(MAIN)), (2, None), (3, None)].into_iter()
        .filter(|&(function, _)| function < functions.len() as u32).collect::<Vec<_>>();
    let mut p = Program {
        module: "async".into(),
        functions,
        entry,
        tables: ChampTables {
            scopes: scopes.iter().map(|&(function, parent)| Scope { function, parent, boundary: 0 }).collect(),
            boundaries: (0..scopes.len() as u32).map(|scope| Boundary { scope }).collect(),
            frames,
            future_results: vec![Some(Type::Int), None],
            ..ChampTables::default()
        },
        ..Program::default()
    };
    for (index, scope) in p.tables.scopes.iter_mut().enumerate() {
        scope.boundary = index as u32;
    }
    p
}

/// Returns `n` through a direct close; the coro golden `async-plain-await`'s @work.
pub(super) fn work() -> (Function, FrameLayout) {
    let (mut b, frame) = Body::asynchronous("work", 0, vec![Type::Int]);
    b.block("entry", &[frame]);
    let (body, start) = b.resume(frame, 0, &[ProducedType::Value(Type::Int), ProducedType::Service(K::Packet)]);
    let [n, inputs] = start[..] else { unreachable!() };
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(WORK), A::Boundary(WORK)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs)], K::ScopeState);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(WORK), l(n), l(empty),
        A::Selection(selection)], K::Pending);
    let finished = b.close(q1, Some(pending), None, WORK, &[], 0, false);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let [result, packet, deps] = [b.register(Type::Int), b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Caller)], vec![result, packet, deps]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(result), l(packet), l(deps)], vec![]);
    b.ret();
    let layout = FrameLayout {
        slots: vec![value(Type::Int), service(K::Packet)],
        continuations: vec![Continuation { site: None, saved: vec![0, 1], operands: vec![] }],
    };
    (b.finish(), layout)
}

/// What a @main variant does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Main {
    /// `async main() { let x = await work(7); }`: the coro golden `async-plain-await`.
    Plain,
    /// The same, with a deferred cancellation delivered at the final exit (D18 `cancel=true`).
    Cancelling,
    /// The await's task and receipt cross a second await before their adapter runs.
    TaskSaved,
    /// The cache duty lives across the await without being saved.
    CacheUnsaved,
    /// The awaited Future is saved as well as being the await's operand.
    SavedOperand,
    /// The failure close delivers a deferred cancellation.
    CancelOnFailure,
    /// The return is reserved before the await: the pending exit lives in the frame across both suspensions.
    PendingAcross,
    /// The final close delivers its deferred cancellation before settling the duty.
    CancelBeforeSettle,
    /// The final close delivers its deferred cancellation in the given way.
    Delivers(Delivery),
}

/// @main's slots: 0 inputs, 1 owner state, 2 cache duty, 3 the Future, 4–6 the unwind operands, 7–9 a second
/// unwind's operands.
fn main_slots() -> Vec<FrameSlot> {
    vec![service(K::Packet), service(K::ScopeState), service(K::CacheState), value(FUTURE_INT),
        value(Type::Failure), service(K::TaskState), service(K::Receipt), value(Type::Failure),
        service(K::TaskState), service(K::Receipt)]
}

pub(super) fn slot_types(slots: &[FrameSlot], indices: &[u32]) -> Vec<ProducedType> {
    indices.iter().map(|&slot| slots[slot as usize].ty).collect()
}

/// Closes @main's scope after a failure, settling its duty, and publishes the failure.
#[allow(clippy::too_many_arguments)]
pub(super) fn fail_close(b: &mut Body, body: Location, state: Location, failure: Location, duty: Location,
    pending: Option<Location>, label: &'static str, cancel: bool)
{
    let failures = if cancel { ENVELOPE_CANCEL } else { 0 };
    let finished = b.close(state, pending, Some(failure), MAIN, &[duty], failures, false);
    let [closed, combined] = [b.service(K::Closed), b.register(Type::Failure)];
    b.dispatch(finished, [pending.is_some(), true, cancel], vec![(Tag::Failure, label, vec![p(0), p(1)])]);
    b.block(label, &[closed, combined]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(false), l(combined)], vec![]);
    b.ret();
}

/// The await.unwind adapter of continuation `k` (form b), then the failure close. A pending return in
/// slot 10 lives across the adapter too.
#[allow(clippy::too_many_arguments)]
pub(super) fn adapter(b: &mut Body, slots: &[FrameSlot], frame: Location, k: u32, state: Location,
    unwind: [Location; 3],
    duty: Location, pending: Option<Location>, label: &'static str, cancel: bool)
{
    let mut values = vec![duty];
    values.extend(pending);
    let saved = b.save(frame, k, &values);
    let [failure, task, receipt] = unwind;
    let [after, adapted] = [b.service(K::ScopeState), b.register(Type::Failure)];
    b.next(O::AwaitUnwind, vec![l(state), l(failure), l(task), l(receipt)], vec![after, adapted]);
    let indices = if pending.is_some() { vec![2, 10] } else { vec![2] };
    let (body, resumed) = b.resume(saved, k, &slot_types(slots, &indices));
    fail_close(b, body, after, adapted, resumed[0], resumed.get(1).copied(), label, cancel);
}

fn main(variant: Main) -> (Function, FrameLayout) {
    let slots = main_slots();
    let cache_saved = variant != Main::CacheUnsaved;
    let (mut b, frame) = Body::asynchronous("main", 1, vec![]);
    b.block("entry", &[frame]);
    let (body, start) = b.resume(frame, 0, &[ProducedType::Service(K::Packet)]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(MAIN), A::Boundary(MAIN)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(start[0])], K::ScopeState);
    let seven = b.constant(Constant::Int("7".into()), Type::Int);
    let offered = b.emit(O::OwnEmpty, vec![], K::Packet);
    let external = b.emit(O::DepsOf, vec![l(seven)], K::Deps);
    b.f.proofs.dependencies.push(Dependency { value: external, obligations: vec![seven] });
    let [future, cache, q2] = [b.register(FUTURE_INT), b.service(K::CacheState), b.service(K::ScopeState)];
    b.next(O::FutureNew, vec![A::Entry(0), A::Scope(MAIN), A::Result(Some(FUTURE_INT)), l(seven), l(offered),
        l(external), l(q1)], vec![future, cache, q2]);
    let mut first_saved = vec![1, 2];
    let mut first_values = vec![q2, cache];
    let mut slots = slots;
    if !cache_saved {
        first_saved = vec![1];
        first_values = vec![q2];
    }
    if variant == Main::SavedOperand {
        slots.push(value(FUTURE_INT));
        first_saved.push(10);
        first_values.push(future);
    }
    let early = (variant == Main::PendingAcross).then(|| {
        slots.push(service(K::Pending));
        first_saved.push(10);
        let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
        let selection = b.selection();
        let pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(MAIN), l(empty),
            A::Selection(selection)], K::Pending);
        first_values.push(pending);
        pending
    });
    let first = b.save(body, 1, &first_values);
    b.outcomes(O::Await, vec![l(future)], vec![(Tag::Normal, "ok", vec![p(0), p(1)]),
        (Tag::Unwind, "bad", vec![p(0), p(1), p(2)])]);
    let [x, xd] = [b.register(Type::Int), b.service(K::Deps)];
    b.block("ok", &[x, xd]);
    let (body, resumed) = b.resume(first, 1, &slot_types(&slots, &first_saved));
    let (q3, c1) = (resumed[0], if cache_saved { resumed[1] } else { cache });
    let pending = match early {
        Some(_) => resumed[2],
        None => {
            let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
            let selection = b.selection();
            b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(MAIN), l(empty),
                A::Selection(selection)], K::Pending)
        }
    };
    let cancel = matches!(variant, Main::Cancelling | Main::CancelBeforeSettle | Main::Delivers(_));
    if variant == Main::CancelBeforeSettle {
        b.delivery = Delivery::BeforeSettle;
    }
    if let Main::Delivers(delivery) = variant {
        b.delivery = delivery;
    }
    let failures = if cancel { ENVELOPE_CANCEL } else { 0 };
    let finished = b.close(q3, Some(pending), None, MAIN, &[c1], failures, false);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    let mut edges = vec![(Tag::Pending, "done", vec![p(0), p(1)])];
    if cancel {
        edges.push((Tag::Failure, "cancelled", vec![p(0), p(1)]));
    }
    b.dispatch(finished, [true, false, cancel], edges);
    b.block("done", &[closed, reserved]);
    let [packet, deps] = [b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Caller)], vec![packet, deps]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(packet), l(deps)], vec![]);
    b.ret();
    if cancel {
        let [closed, cancelled] = [b.service(K::Closed), b.register(Type::Failure)];
        b.block("cancelled", &[closed, cancelled]);
        b.next(O::FuturePublish, vec![l(body), A::Flag(false), l(cancelled)], vec![]);
        b.ret();
    }
    let unwind = [b.register(Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad", &unwind);
    let (body, resumed) = b.resume(first, 1, &slot_types(&slots, &first_saved));
    let (q4, c2) = (resumed[0], if cache_saved { resumed[1] } else { cache });
    let mut continuations = vec![
        Continuation { site: None, saved: vec![0], operands: vec![] },
        Continuation { site: None, saved: first_saved, operands: vec![3] },
    ];
    if variant != Main::TaskSaved {
        let pending = (variant == Main::PendingAcross).then(|| resumed[2]);
        adapter(&mut b, &slots, body, 2, q4, unwind, c2, pending, "failed", variant == Main::CancelOnFailure);
        let saved = if pending.is_some() { vec![2, 10] } else { vec![2] };
        continuations.push(Continuation { site: None, saved, operands: vec![1, 4, 5, 6] });
    } else {
        // Await the Future again while the first await's task and receipt wait in the frame.
        let second = b.save(body, 2, &[q4, c2, unwind[0], unwind[1], unwind[2]]);
        b.outcomes(O::Await, vec![l(future)], vec![(Tag::Normal, "again", vec![p(0), p(1)]),
            (Tag::Unwind, "again_bad", vec![p(0), p(1), p(2)])]);
        let [x2, xd2] = [b.register(Type::Int), b.service(K::Deps)];
        b.block("again", &[x2, xd2]);
        let (body, resumed) = b.resume(second, 2, &slot_types(&slots, &[1, 2, 4, 5, 6]));
        let unwind = [resumed[2], resumed[3], resumed[4]];
        adapter(&mut b, &slots, body, 3, resumed[0], unwind, resumed[1], None, "failed", false);
        let unwind2 = [b.register(Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
        b.block("again_bad", &unwind2);
        let (body, resumed) = b.resume(second, 2, &slot_types(&slots, &[1, 2, 4, 5, 6]));
        let [q5, c5, e3, t3, r3] = resumed[..] else { unreachable!() };
        let kept = b.save(body, 4, &[c5, e3, t3, r3]);
        let [q6, ignored] = [b.service(K::ScopeState), b.register(Type::Failure)];
        b.next(O::AwaitUnwind, vec![l(q5), l(unwind2[0]), l(unwind2[1]), l(unwind2[2])], vec![q6, ignored]);
        let (body, resumed) = b.resume(kept, 4, &slot_types(&slots, &[2, 4, 5, 6]));
        let unwind = [resumed[1], resumed[2], resumed[3]];
        adapter(&mut b, &slots, body, 5, q6, unwind, resumed[0], None, "failed_again", false);
        continuations.extend([
            Continuation { site: None, saved: vec![1, 2, 4, 5, 6], operands: vec![3] },
            Continuation { site: None, saved: vec![2], operands: vec![1, 4, 5, 6] },
            Continuation { site: None, saved: vec![2, 4, 5, 6], operands: vec![1, 7, 8, 9] },
            Continuation { site: None, saved: vec![2], operands: vec![1, 4, 5, 6] },
        ]);
    }
    for (k, continuation) in continuations.iter_mut().enumerate().skip(1) {
        continuation.site = Some(b.sites[&(k as u32)]);
    }
    (b.finish(), FrameLayout { slots, continuations })
}

fn plain_with(variant: Main) -> Program {
    let (work, work_layout) = work();
    let (main, main_layout) = main(variant);
    program(vec![work, main], vec![work_layout, main_layout], None)
}

pub(super) fn plain() -> Program {
    plain_with(Main::Plain)
}

fn main_of(p: &mut Program) -> &mut Function {
    &mut p.functions[1]
}

pub(super) fn find(f: &Function, op: Opcode) -> usize {
    f.code.iter().position(|instruction| matches!(instruction, Instruction::Champ { op: found, .. } if *found == op))
        .unwrap()
}

pub(super) fn arguments(f: &mut Function, pc: usize) -> &mut Vec<Argument> {
    let Instruction::Champ { arguments, .. } = &mut f.code[pc] else { panic!("not a champ instruction") };
    arguments
}

#[test]
fn plain_await_loads() {
    for variant in [Main::Plain, Main::Cancelling] {
        verify_load(&plain_with(variant), &NoExternalContracts).unwrap();
    }
}

/// Creates `futures` Futures of @work, reads and cancels the first, and never awaits any: each cache duty is
/// settled at the scope's close (coro golden `async-never-awaited`). No suspension.
fn lazy(futures: usize, settle_empty_first: bool) -> (Function, FrameLayout) {
    let (mut b, frame) = Body::asynchronous("lazy", 1, vec![]);
    b.settle_empty_first = settle_empty_first;
    b.block("entry", &[frame]);
    let (body, start) = b.resume(frame, 0, &[ProducedType::Service(K::Packet)]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(MAIN), A::Boundary(MAIN)], K::ScopeState);
    let mut state = b.emit(O::OwnAccept, vec![l(q0), l(start[0])], K::ScopeState);
    let seven = b.constant(Constant::Int("7".into()), Type::Int);
    let mut duties = Vec::new();
    let mut first = None;
    for _ in 0..futures {
        let offered = b.emit(O::OwnEmpty, vec![], K::Packet);
        let [future, cache, next] = [b.register(FUTURE_INT), b.service(K::CacheState), b.service(K::ScopeState)];
        b.next(O::FutureNew, vec![A::Entry(0), A::Scope(MAIN), A::Result(Some(FUTURE_INT)), l(seven), l(offered),
            l(state)], vec![future, cache, next]);
        state = next;
        duties.push(cache);
        first = first.or(Some(future));
    }
    let future = first.unwrap();
    let [done, cancelled] = [b.register(Type::Bool), b.register(Type::Bool)];
    b.next(O::FutureDone, vec![l(future)], vec![done]);
    b.next(O::FutureCancel, vec![l(future)], vec![]);
    b.next(O::FutureCancelled, vec![l(future)], vec![cancelled]);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(MAIN), l(empty),
        A::Selection(selection)], K::Pending);
    let finished = b.close(state, Some(pending), None, MAIN, &duties, 0, false);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let [packet, deps] = [b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Caller)], vec![packet, deps]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(packet), l(deps)], vec![]);
    b.ret();
    let layout = FrameLayout {
        slots: vec![service(K::Packet)],
        continuations: vec![Continuation { site: None, saved: vec![0], operands: vec![] }],
    };
    (b.finish(), layout)
}

fn lazy_program(futures: usize) -> Program {
    lazy_variant(futures, false)
}

fn lazy_variant(futures: usize, settle_empty_first: bool) -> Program {
    let (work, work_layout) = work();
    let (lazy, lazy_layout) = lazy(futures, settle_empty_first);
    program(vec![work, lazy], vec![work_layout, lazy_layout], None)
}

/// `async action twice() is int { let f = work(7); let a = await f; let b = await f; return a + b; }`: the
/// Future is the operand of both awaits and the first result is saved across the second (coro golden
/// `async-repeated-await`). Unless `keep_first`, the first result lives across the second await unsaved.
fn twice(keep_first: bool) -> (Function, FrameLayout) {
    let slots = vec![service(K::Packet), service(K::ScopeState), service(K::CacheState), value(FUTURE_INT),
        value(Type::Failure), service(K::TaskState), service(K::Receipt), value(Type::Int)];
    let (mut b, frame) = Body::asynchronous("twice", 1, vec![Type::Int]);
    b.block("entry", &[frame]);
    let (body, start) = b.resume(frame, 0, &[ProducedType::Service(K::Packet)]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(MAIN), A::Boundary(MAIN)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(start[0])], K::ScopeState);
    let seven = b.constant(Constant::Int("7".into()), Type::Int);
    let offered = b.emit(O::OwnEmpty, vec![], K::Packet);
    let [future, cache, q2] = [b.register(FUTURE_INT), b.service(K::CacheState), b.service(K::ScopeState)];
    b.next(O::FutureNew, vec![A::Entry(0), A::Scope(MAIN), A::Result(Some(FUTURE_INT)), l(seven), l(offered),
        l(q1)], vec![future, cache, q2]);
    let first = b.save(body, 1, &[q2, cache]);
    b.outcomes(O::Await, vec![l(future)], vec![(Tag::Normal, "first", vec![p(0), p(1)]),
        (Tag::Unwind, "first_bad", vec![p(0), p(1), p(2)])]);
    let [a, ad] = [b.register(Type::Int), b.service(K::Deps)];
    b.block("first", &[a, ad]);
    let (body, resumed) = b.resume(first, 1, &slot_types(&slots, &[1, 2]));
    let second_saved: Vec<u32> = if keep_first { vec![7, 1, 2] } else { vec![1, 2] };
    let mut values = if keep_first { vec![a] } else { vec![] };
    values.extend(&resumed);
    let second = b.save(body, 2, &values);
    b.outcomes(O::Await, vec![l(future)], vec![(Tag::Normal, "second", vec![p(0), p(1)]),
        (Tag::Unwind, "second_bad", vec![p(0), p(1), p(2)])]);
    let [bv, bd] = [b.register(Type::Int), b.service(K::Deps)];
    b.block("second", &[bv, bd]);
    let (body, resumed) = b.resume(second, 2, &slot_types(&slots, &second_saved));
    let (kept, rest) = if keep_first { (resumed[0], &resumed[1..]) } else { (a, &resumed[..]) };
    let sum = b.register(Type::Int);
    let (Location::Register(left), Location::Register(right), Location::Register(out)) = (kept, bv, sum) else {
        unreachable!()
    };
    b.f.code.push(Instruction::Data { op: DataOp::IntAdd, inputs: vec![left, right], output: Some(out),
        immediate: Immediate::None });
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(MAIN), l(sum), l(empty),
        A::Selection(selection)], K::Pending);
    let finished = b.close(rest[0], Some(pending), None, MAIN, &[rest[1]], 0, false);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let [result, packet, deps] = [b.register(Type::Int), b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Caller)], vec![result, packet, deps]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(result), l(packet), l(deps)], vec![]);
    b.ret();
    let unwind = [b.register(Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("first_bad", &unwind);
    let (body, resumed) = b.resume(first, 1, &slot_types(&slots, &[1, 2]));
    adapter(&mut b, &slots, body, 3, resumed[0], unwind, resumed[1], None, "first_failed", false);
    let unwind = [b.register(Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("second_bad", &unwind);
    let (body, resumed) = b.resume(second, 2, &slot_types(&slots, &second_saved));
    let rest = if keep_first { &resumed[1..] } else { &resumed[..] };
    adapter(&mut b, &slots, body, 4, rest[0], unwind, rest[1], None, "second_failed", false);
    let mut continuations = vec![
        Continuation { site: None, saved: vec![0], operands: vec![] },
        Continuation { site: None, saved: vec![1, 2], operands: vec![3] },
        Continuation { site: None, saved: second_saved, operands: vec![3] },
        Continuation { site: None, saved: vec![2], operands: vec![1, 4, 5, 6] },
        Continuation { site: None, saved: vec![2], operands: vec![1, 4, 5, 6] },
    ];
    for (k, continuation) in continuations.iter_mut().enumerate().skip(1) {
        continuation.site = Some(b.sites[&(k as u32)]);
    }
    (b.finish(), FrameLayout { slots, continuations })
}

fn twice_program(keep_first: bool) -> Program {
    let (work, work_layout) = work();
    let (twice, twice_layout) = twice(keep_first);
    program(vec![work, twice], vec![work_layout, twice_layout], None)
}

/// Object 0: `Res`, an identity with a destroy.
pub(super) fn res() -> Object {
    Object {
        name: "Res".into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: false,
        role: None, implements: vec![], any_box: false, enum_names: vec![], fields: vec![],
        layout: ObjectRuntimeLayout { storage_fields: vec![], allocation_words: 5, identity_offset: Some(8),
            has_destroy: true, may_reach_resource: true, may_async_destroy: false, trace_layout: vec![],
            selection_layout: vec![] },
    }
}

/// The destroy entry of `Res`, declaring `failures`.
pub(super) fn destroy(failures: u32) -> Function {
    let mut b = Body::new("destroy", EntryAbi::Observation, vec![]);
    let target = b.register(Type::Object(0));
    b.f.params = vec![target];
    b.f.proofs.entry = Some(EntryContract { destroy: Some(0), failures, ..EntryContract::default() });
    b.block("entry", &[target]);
    b.ret();
    b.finish()
}

/// @consume takes a `Res`: its inputs are not plain, so a cold Future of it holds obligations its cache
/// settle releases. Its own close needs the executor.
fn consume(failures: u32) -> (Function, FrameLayout) {
    let (mut b, frame) = Body::asynchronous("consume", 2, vec![]);
    b.block("entry", &[frame]);
    let (body, start) = b.resume(frame, 0, &[ProducedType::Value(Type::Object(0)), ProducedType::Service(K::Packet)]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(CONSUME), A::Boundary(CONSUME)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(start[1])], K::ScopeState);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(CONSUME), l(empty),
        A::Selection(selection)], K::Pending);
    let finished = b.close(q1, Some(pending), None, CONSUME, &[], failures, true);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    let mut edges = vec![(Tag::Pending, "done", vec![p(0), p(1)])];
    if failures != 0 {
        edges.push((Tag::Failure, "failed", vec![p(0), p(1)]));
    }
    b.dispatch(finished, [true, false, failures != 0], edges);
    b.block("done", &[closed, reserved]);
    let [packet, deps] = [b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Caller)], vec![packet, deps]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(packet), l(deps)], vec![]);
    b.ret();
    if failures != 0 {
        let [closed, failure] = [b.service(K::Closed), b.register(Type::Failure)];
        b.block("failed", &[closed, failure]);
        b.next(O::FuturePublish, vec![l(body), A::Flag(false), l(failure)], vec![]);
        b.ret();
    }
    let layout = FrameLayout {
        slots: vec![value(Type::Object(0)), service(K::Packet)],
        continuations: vec![Continuation { site: None, saved: vec![0, 1], operands: vec![] }],
    };
    (b.finish(), layout)
}

/// A synchronous owner (D24.2, review B7): `action make(r: Res) { let f = consume(r); }` never awaits f; its
/// close settles the cache duty, which may release `r`'s obligation into the plan.
fn make(merge: bool, failures: u32) -> Function {
    let mut b = Body::new("make", EntryAbi::Observation, vec![]);
    let target = b.register(Type::Object(0));
    b.f.params = vec![target];
    b.f.proofs.entry = Some(EntryContract { failures, ..EntryContract::default() });
    b.block("entry", &[target]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(MAKE), A::Boundary(MAKE)], K::ScopeState);
    let offered = b.emit(O::OwnEmpty, vec![], K::Packet);
    let [future, cache, q1] = [b.register(FUTURE_VOID), b.service(K::CacheState), b.service(K::ScopeState)];
    b.next(O::FutureNew, vec![A::Entry(2), A::Scope(MAKE), A::Result(Some(FUTURE_VOID)), l(target), l(offered),
        l(q0)], vec![future, cache, q1]);
    let finished = b.close(q1, None, None, MAKE, &[cache], failures, merge);
    let closed = b.service(K::Closed);
    let mut edges = vec![(Tag::Success, "done", vec![p(0)])];
    if failures != 0 {
        edges.push((Tag::Failure, "failed", vec![p(0), p(1)]));
    }
    b.dispatch(finished, [false, false, failures != 0], edges);
    b.block("done", &[closed]);
    b.ret();
    if failures != 0 {
        let [closed, failure] = [b.service(K::Closed), b.register(Type::Failure)];
        b.block("failed", &[closed, failure]);
        let Location::Register(id) = failure else { unreachable!() };
        b.f.code.push(Instruction::Raise(id));
    }
    b.finish()
}

pub(super) fn owner_program(merge: bool) -> Program {
    owner_failing(merge, 0, 0)
}

/// `Res`'s destroy may fail with `destroys`; @make's close declares `envelope`.
fn owner_failing(merge: bool, destroys: u32, envelope: u32) -> Program {
    let (work, work_layout) = work();
    let (main, main_layout) = main(Main::Plain);
    let (consume, consume_layout) = consume(destroys);
    let mut p = program(vec![work, main, consume, make(merge, envelope), destroy(destroys)],
        vec![work_layout, main_layout, consume_layout], None);
    p.objects.push(res());
    p
}

pub(super) fn reject_with(p: &Program, code: &'static str, message: &str) {
    let failure = verify_load(p, &NoExternalContracts).unwrap_err();
    assert_eq!(failure.rule, code, "{failure}");
    assert!(failure.message.contains(message), "{failure}");
}

// --- Registry and codec ---------------------------------------------------------------------------------------

#[test]
fn async_block_ids_follow_draft3() {
    let assigned = [(64, O::FutureNew, "fut.new"), (66, O::Await, "await"), (70, O::FuturePublish, "fut.publish"),
        (71, O::FutureCancel, "fut.cancel"), (72, O::FutureDone, "fut.done"), (73, O::FutureCancelled, "fut.cancelled"),
        (74, O::FrameSave, "frame.save"), (75, O::FrameResume, "frame.resume"), (76, O::AwaitUnwind, "await.unwind"),
        (77, O::CleanupDeliverCancel, "cleanup.deliver_cancel"), (67, O::AwaitTake, "await.take"),
        (68, O::UniqueCacheProof, "unique_cache.proof"), (69, O::CacheRehome, "cache.rehome"),
        (84, O::CleanupSettleDynamic, "cleanup.settle.dynamic"), (85, O::ServiceMaterialize, "service.materialize"),
        (86, O::ServiceAbsorb, "service.absorb"), (65, O::FutureSpawn, "fut.spawn")];
    for (id, op, name) in assigned {
        assert_eq!((op.id(), op.name()), (id, name));
        assert_eq!(Opcode::from_id(id).unwrap(), op);
    }
    // Pins, workers and the unassigned tail stay reserved until their slices (spawn is assigned by #601 P1).
    for id in (78..=83).chain(87..=95) {
        assert!(Opcode::from_id(id).unwrap_err().message.contains("reserved"), "{id}");
    }
    assert_eq!(Opcode::CleanupSettle.id(), 21);
}

#[test]
fn async_classes_and_suspension_sites_follow_draft3() {
    use HelperClass::*;
    let classes = [(O::FutureNew, G), (O::Await, S), (O::FuturePublish, L), (O::FutureCancel, L),
        (O::FutureDone, L), (O::FutureCancelled, L), (O::FrameSave, L), (O::FrameResume, L), (O::AwaitUnwind, S),
        (O::CleanupDeliverCancel, L), (O::CleanupSettle, L), (O::AwaitTake, S), (O::CacheRehome, L),
        (O::FutureSpawn, X)];
    for (op, class) in classes {
        assert_eq!(op.helper_class(), Some(class), "{}", op.name());
        assert_eq!(op.suspends(), class == S, "{}", op.name());
    }
    // Draft 3 §14.4: the helper of each async operation, whose shared effects give the operation's class.
    // `fut.done`, `frame.save` and `frame.resume` are inline; the nonempty settle has its own helper.
    let helpers = [(O::FutureNew, Some("champ_future_new")), (O::Await, Some("champ_await_view")),
        (O::FuturePublish, Some("champ_future_publish")), (O::FutureCancel, Some("champ_future_cancel")),
        (O::FutureDone, None), (O::FutureCancelled, Some("champ_future_cancelled")), (O::FrameSave, None),
        (O::FrameResume, None), (O::AwaitUnwind, Some("champ_await_unwind")),
        (O::CleanupDeliverCancel, Some("champ_cleanup_deliver_cancel")), (O::FutureSpawn, Some("champ_future_spawn"))];
    let class_of = |helper: Helper| if helper.effects & HELPER_SUSPENDS != 0 {
        S
    } else if helper.effects & HELPER_REENTERS != 0 {
        X
    } else if helper.effects & (HELPER_COLLECTS | HELPER_UNWINDS) != 0 {
        G
    } else {
        L
    };
    for (op, name) in helpers {
        let found = op.helper();
        assert_eq!(found.map(|helper| helper.name), name, "{}", op.name());
        if let Some(found) = found {
            assert_eq!(found.opcode, Some(op), "{}", op.name());
            assert_eq!(Some(class_of(found)), op.helper_class(), "{}", op.name());
        }
    }
    let settle = helper("champ_cleanup_settle").unwrap();
    assert_eq!((settle.opcode, Some(class_of(settle))), (Some(O::CleanupSettle), O::CleanupSettle.helper_class()));
    assert_eq!(O::InvokeCallOwn.helper_class(), None);
    // The proof has no helper and no class: the loader proves the capability (draft 3 §14.4, ID 68).
    assert_eq!(O::UniqueCacheProof.helper_class(), None);
    assert!(!O::UniqueCacheProof.suspends());
    let frame_bound = [O::Await, O::AwaitTake, O::AwaitUnwind, O::FuturePublish, O::FrameSave, O::FrameResume,
        O::CleanupDeliverCancel];
    for op in Opcode::ALL {
        assert_eq!(op.frame_bound(), frame_bound.contains(op), "{}", op.name());
    }
}

#[test]
fn async_result_schemas() {
    use ProducedType::{Service as Sv, Value};
    let valued = ResultContext { value: Some(Type::Int), ..ResultContext::default() };
    let void = ResultContext::default();
    assert_eq!(O::Await.tags(&void), [Tag::Normal, Tag::Unwind]);
    assert!(O::Await.is_terminator());
    assert_eq!(O::Await.results(Tag::Normal, &valued).unwrap(), [Value(Type::Int), Sv(K::Deps)]);
    assert_eq!(O::Await.results(Tag::Normal, &void).unwrap(), [Sv(K::Deps)]);
    assert_eq!(O::Await.results(Tag::Unwind, &void).unwrap(),
        [Value(Type::Failure), Sv(K::TaskState), Sv(K::Receipt)]);
    let future = ResultContext { value: Some(FUTURE_INT), ..ResultContext::default() };
    assert_eq!(O::FutureNew.results(Tag::Normal, &future).unwrap(),
        [Value(FUTURE_INT), Sv(K::CacheState), Sv(K::ScopeState)]);
    assert!(O::FutureNew.results(Tag::Normal, &void).is_err());
    assert_eq!(O::AwaitUnwind.results(Tag::Normal, &void).unwrap(), [Sv(K::ScopeState), Value(Type::Failure)]);
    assert!(O::FuturePublish.results(Tag::Normal, &void).unwrap().is_empty());
    assert!(O::FutureCancel.results(Tag::Normal, &void).unwrap().is_empty());
    assert_eq!(O::FutureDone.results(Tag::Normal, &void).unwrap(), [Value(Type::Bool)]);
    assert_eq!(O::FutureCancelled.results(Tag::Normal, &void).unwrap(), [Value(Type::Bool)]);
    assert_eq!(O::FrameSave.results(Tag::Normal, &void).unwrap(), [Sv(K::Frame)]);
    let slots = ResultContext { slots: vec![Sv(K::ScopeState), Value(Type::Int)], ..ResultContext::default() };
    assert_eq!(O::FrameResume.results(Tag::Normal, &slots).unwrap(),
        [Sv(K::Frame), Sv(K::ScopeState), Value(Type::Int)]);
    assert_eq!(O::CleanupDeliverCancel.results(Tag::Normal, &void).unwrap(), [Sv(K::Cleanup)]);
    for op in [O::FutureNew, O::FuturePublish, O::FrameSave, O::FrameResume, O::AwaitUnwind] {
        assert!(op.results(Tag::Unwind, &future).is_err(), "{}", op.name());
    }
    // The frame token takes no word and a receipt is a raw word; the duty tokens hold the Future frame.
    use layout::service_layout;
    assert_eq!((service_layout(K::Frame).words, service_layout(K::Frame).references), (0, &[][..]));
    assert_eq!((service_layout(K::Receipt).words, service_layout(K::Receipt).references), (1, &[][..]));
    for kind in [K::CacheState, K::TaskState] {
        assert_eq!((service_layout(kind).words, service_layout(kind).references), (1, &[0][..]));
    }
    assert!(FUTURE_INT.traced());
    assert!([K::Frame, K::CacheState, K::TaskState, K::Receipt].iter().all(|kind| kind.linear()));
}

fn used_opcodes(p: &Program) -> std::collections::BTreeSet<u32> {
    p.functions.iter().flat_map(|f| &f.code).filter_map(|instruction| match instruction {
        Instruction::Champ { op, .. } => Some(op.id()),
        _ => None,
    }).collect()
}

#[test]
fn async_codec_round_trips_every_new_form() {
    let programs = [plain(), plain_with(Main::Cancelling), plain_with(Main::TaskSaved), lazy_program(2),
        owner_program(true)];
    let mut used = std::collections::BTreeSet::new();
    for p in &programs {
        let bytes = encode(p).unwrap();
        assert_eq!(&decode(&bytes).unwrap(), p);
        let text = disassemble(p).unwrap();
        assert_eq!(&assemble(&text).unwrap(), p);
        assert_eq!(disassemble(&assemble(&text).unwrap()).unwrap(), text);
        used.extend(used_opcodes(p));
    }
    for id in [64, 66, 70, 71, 72, 73, 74, 75, 76, 77] {
        assert!(used.contains(&id), "opcode {id} is not round-tripped");
    }
    // The nonempty settle carries its duty; the frame table, Future types and the async entry ABI travel too.
    let p = &programs[0];
    let settles = p.functions[1].code.iter().filter(|instruction| matches!(instruction,
        Instruction::Champ { op: O::CleanupSettle, arguments, .. } if arguments.len() == 3)).count();
    assert_eq!(settles, 2);
    assert_eq!(p.tables.frames.len(), 2);
    assert_eq!(p.functions[1].entry_abi, EntryAbi::AsyncBody { layout: 1 });
    let text = disassemble(p).unwrap();
    for form in ["frames", "future_results", "champ fut.new", "champ await", "champ frame.save",
        "champ frame.resume", "champ await.unwind", "champ fut.publish"]
    {
        assert!(text.contains(form), "{form}");
    }
}

#[test]
fn programs_without_async_forms_keep_their_assembly() {
    // The new table fields are optional in assembly: a P1 program prints no `frames` or `future_results`.
    let mut p = plain();
    p.functions.clear();
    p.tables = ChampTables::default();
    p.entry = None;
    let text = disassemble(&p).unwrap();
    assert!(!text.contains("frames") && !text.contains("future_results"), "{text}");
    assert_eq!(assemble(&text).unwrap(), p);
}

#[test]
fn async_codec_refuses_reserved_opcodes_and_unknown_tags() {
    let p = plain();
    let text = disassemble(&p).unwrap();
    let reserved = text.replacen("champ fut.new", "champ pin", 1);
    assert!(assemble(&reserved).unwrap_err().message.contains("pin"));
    let bytes = encode(&p).unwrap();
    // Re-encode the first fut.new (ID 64) as the reserved pin (78): the binary reader refuses it.
    let mut pinned = bytes.clone();
    let marker = [255u8, 64];
    let at = pinned.windows(2).position(|window| window == marker).unwrap();
    pinned[at + 1] = 78;
    assert!(decode(&pinned).unwrap_err().message.contains("reserved"));
    // Tags 0..=5 are assigned (4 is the async defer entry of #601 R4, 5 the async external of #601 Q1).
    let unknown = text.replacen("entry_abi 3 ", "entry_abi 6 ", 1);
    assert!(assemble(&unknown).unwrap_err().message.contains("entry ABI"));
}

// --- Positive programs ---------------------------------------------------------------------------------------

#[test]
fn never_awaited_futures_settle_at_their_owner() {
    for futures in [1, 2] {
        verify_load(&lazy_program(futures), &NoExternalContracts).unwrap();
    }
}

#[test]
fn repeated_await_saves_the_first_result() {
    verify_load(&twice_program(true), &NoExternalContracts).unwrap();
}

#[test]
fn sync_owner_releases_a_cold_future_into_the_executor() {
    verify_load(&owner_program(true), &NoExternalContracts).unwrap();
}

// --- SPB_ASYNC_ENTRY -----------------------------------------------------------------------------------------

#[test]
fn frame_bound_operations_belong_to_async_bodies() {
    let mut b = Body::new("sync", EntryAbi::Observation, vec![]);
    let future = b.register(FUTURE_INT);
    b.f.params = vec![future];
    b.block("entry", &[future]);
    b.outcomes(O::Await, vec![l(future)], vec![(Tag::Normal, "ok", vec![p(0), p(1)]),
        (Tag::Unwind, "bad", vec![p(0), p(1), p(2)])]);
    let ok = [b.register(Type::Int), b.service(K::Deps)];
    b.block("ok", &ok);
    b.ret();
    let bad = [b.register(Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad", &bad);
    b.ret();
    let p = program(vec![b.finish_unchecked()], vec![], None);
    reject_with(&p, suspension::ASYNC_ENTRY, "belongs to async bodies only");
}

#[test]
fn an_async_body_takes_exactly_its_frame() {
    let mut p = plain();
    let f = &mut p.functions[0];
    let extra = Location::Register(f.registers.len() as u32);
    f.registers.push(Type::Int);
    f.params.push(extra);
    f.blocks[0].params.push(extra);
    reject_with(&p, suspension::ASYNC_ENTRY, "takes exactly its frame");
}

#[test]
fn a_frame_token_stays_in_async_bodies() {
    let mut p = owner_program(true);
    p.functions[3].services.push(K::Frame);
    reject_with(&p, suspension::ASYNC_ENTRY, "frame token outside an async body");
}

#[test]
fn a_frame_layout_belongs_to_one_async_body() {
    let mut p = plain();
    p.functions[1].entry_abi = EntryAbi::AsyncBody { layout: 0 };
    reject_with(&p, suspension::ASYNC_ENTRY, "exactly one async body");
}

#[test]
fn the_start_holds_the_inputs_then_one_packet() {
    let mut p = plain();
    p.tables.frames[0].continuations[0].saved = vec![1, 0];
    reject_with(&p, suspension::ASYNC_ENTRY, "start holds the language inputs");
}

#[test]
fn every_async_path_publishes_before_ret() {
    let mut p = plain();
    let f = &mut p.functions[0];
    let publish = find(f, O::FuturePublish);
    let packet = Location::Service(f.services.len() as u32);
    f.services.push(K::Packet);
    f.code[publish] = Instruction::Champ { op: O::OwnEmpty, arguments: vec![], flow: Flow::Next(vec![packet]) };
    reject_with(&p, suspension::ASYNC_ENTRY, "ends in fut.publish then ret");
}

#[test]
fn an_async_body_never_raises() {
    let mut p = plain();
    let f = main_of(&mut p);
    let publish = f.code.iter().rposition(|instruction| matches!(instruction,
        Instruction::Champ { op: O::FuturePublish, arguments, .. } if arguments[1] == A::Flag(false))).unwrap();
    let A::Location(Location::Register(failure)) = arguments(f, publish)[2] else { panic!("failure register") };
    let future = f.registers.iter().position(|ty| *ty == FUTURE_INT).unwrap() as u32;
    // Keep the frame consumed by a frame-free step and leave by raise instead of fut.publish.
    f.code[publish] = Instruction::Champ { op: O::FutureCancel, arguments: vec![l(Location::Register(future))],
        flow: Flow::Next(vec![]) };
    f.code[publish + 1] = Instruction::Raise(failure);
    reject_with(&p, suspension::ASYNC_ENTRY, "never raise");
}

#[test]
fn fut_new_names_an_async_body() {
    let mut p = owner_program(true);
    let f = main_of(&mut p);
    let new = find(f, O::FutureNew);
    arguments(f, new)[0] = A::Entry(3);
    reject_with(&p, suspension::ASYNC_ENTRY, "must name an async body");
}

#[test]
fn deferred_cancellation_needs_its_envelope() {
    let mut p = plain_with(Main::Cancelling);
    let f = main_of(&mut p);
    f.proofs.envelopes[0].failures = 0;
    reject_with(&p, suspension::ASYNC_ENTRY, "exactly when its envelope declares one");
}

#[test]
fn a_cancelling_envelope_needs_its_delivery() {
    let mut p = plain();
    main_of(&mut p).proofs.envelopes[0].failures = ENVELOPE_CANCEL;
    reject_with(&p, suspension::ASYNC_ENTRY, "exactly when its envelope declares one");
}

#[test]
fn deferred_cancellation_only_at_the_final_exit() {
    reject_with(&plain_with(Main::CancelOnFailure), suspension::ASYNC_ENTRY, "only at the final exit");
}

#[test]
fn only_async_bodies_cancel_at_their_exit() {
    let mut p = owner_program(true);
    p.functions[3].proofs.envelopes[0].failures = ENVELOPE_CANCEL;
    reject_with(&p, suspension::ASYNC_ENTRY, "only an async body delivers");
}

// --- SPB_FRAME_LINEAR ----------------------------------------------------------------------------------------

#[test]
fn a_linear_token_is_saved_across_a_suspension() {
    reject_with(&plain_with(Main::CacheUnsaved), suspension::FRAME_LINEAR, "lives across the suspension");
}

#[test]
fn a_token_is_never_saved_and_an_operand() {
    reject_with(&plain_with(Main::SavedOperand), suspension::FRAME_LINEAR, "saved and an operand");
}

#[test]
fn saved_values_match_the_layout() {
    let mut p = plain();
    p.tables.frames[1].slots[2] = service(K::TaskState);
    reject_with(&p, suspension::FRAME_LINEAR, "differ from the continuation's saved slots");
}

#[test]
fn slot_linearity_follows_its_type() {
    let mut p = plain();
    p.tables.frames[1].slots[1].linear = false;
    reject_with(&p, suspension::FRAME_LINEAR, "linearity differs");
    let mut p = plain();
    p.tables.frames[1].slots[3].linear = true;
    reject_with(&p, suspension::FRAME_LINEAR, "value slot is not linear");
    let mut p = plain();
    p.tables.frames[1].slots.push(service(K::Frame));
    reject_with(&p, suspension::FRAME_LINEAR, "never stored in a frame slot");
}

// --- SPB_RESUME_TABLE ----------------------------------------------------------------------------------------

#[test]
fn a_successor_resumes_first() {
    let mut p = plain();
    let f = main_of(&mut p);
    // Swap the ok block's resume with the independent instruction after it.
    let resume = f.code.iter().enumerate().position(|(pc, instruction)| pc > 0 && matches!(instruction,
        Instruction::Champ { op: O::FrameResume, .. })).unwrap();
    f.code.swap(resume, resume + 1);
    refresh(f);
    reject_with(&p, suspension::RESUME_TABLE, "resumes its continuation from the saved frame first");
}

#[test]
fn a_resume_reloads_the_frame_its_continuation_saved() {
    let mut p = plain();
    let f = main_of(&mut p);
    let resumes = f.code.iter().enumerate().filter(|(_, instruction)| matches!(instruction,
        Instruction::Champ { op: O::FrameResume, .. })).map(|(pc, _)| pc).collect::<Vec<_>>();
    let start = match &f.code[0] {
        Instruction::Champ { flow: Flow::Next(outputs), .. } => outputs[0],
        _ => unreachable!(),
    };
    arguments(f, resumes[1])[0] = l(start);
    reject_with(&p, suspension::RESUME_TABLE, "from the saved frame first");
}

#[test]
fn the_body_starts_by_resuming_continuation_zero() {
    let mut p = plain();
    p.tables.frames[1].continuations[0].site = Some(0);
    reject_with(&p, suspension::RESUME_TABLE, "continuation 0 is the start");
    // The body opens its scope before reading its start: the start resume is not its first instruction.
    let mut p = plain();
    p.functions[0].code.swap(0, 1);
    refresh(&mut p.functions[0]);
    reject_with(&p, suspension::RESUME_TABLE, "starts by resuming continuation 0 from its entry frame");
}

#[test]
fn continuations_have_distinct_sites() {
    let mut p = plain();
    let site = p.tables.frames[1].continuations[1].site;
    p.tables.frames[1].continuations[2].site = site;
    reject_with(&p, suspension::RESUME_TABLE, "share one site");
}

// --- SPB_SUSPEND_BRACKET -------------------------------------------------------------------------------------

#[test]
fn a_save_brackets_its_continuation_site() {
    let mut p = plain();
    let new = find(main_of(&mut p), O::FutureNew) as u32;
    p.tables.frames[1].continuations[1].site = Some(new);
    reject_with(&p, suspension::SUSPEND_BRACKET, "brackets exactly the suspending site");
}

#[test]
fn the_start_is_never_saved() {
    let mut p = plain();
    let f = main_of(&mut p);
    let save = find(f, O::FrameSave);
    arguments(f, save)[1] = A::Continuation(0);
    reject_with(&p, suspension::SUSPEND_BRACKET, "start continuation is never saved");
}

// --- SPB_TASK_RECEIPT ----------------------------------------------------------------------------------------

#[test]
fn task_and_receipt_go_straight_to_their_adapter() {
    reject_with(&plain_with(Main::TaskSaved), suspension::TASK_RECEIPT, "reaches frame.save");
}

// --- SPB_SCHEDULE --------------------------------------------------------------------------------------------

#[test]
fn a_duty_is_settled_by_its_owner() {
    let mut p = plain();
    let f = main_of(&mut p);
    let new = find(f, O::FutureNew);
    arguments(f, new)[1] = A::Scope(INNER);
    reject_with(&p, suspension::SCHEDULE, "does not own it");
}

#[test]
fn duties_are_settled_in_schedule_order() {
    let mut p = lazy_program(2);
    main_of(&mut p).proofs.schedules[0].services.reverse();
    reject_with(&p, suspension::SCHEDULE, "in schedule order");
}

#[test]
fn a_schedule_names_cache_duties_once() {
    let mut p = lazy_program(1);
    let duty = main_of(&mut p).proofs.schedules[0].services[0];
    main_of(&mut p).proofs.schedules[0].services.push(duty);
    reject_with(&p, suspension::SCHEDULE, "names one duty twice");
    let mut p = lazy_program(1);
    let f = main_of(&mut p);
    let state = f.services.iter().position(|kind| *kind == K::ScopeState).unwrap() as u32;
    f.proofs.schedules[0].services[0] = Location::Service(state);
    reject_with(&p, suspension::SCHEDULE, "not a cache-state");
}

#[test]
fn a_nonempty_schedule_is_settled_duty_by_duty() {
    reject_with(&lazy_variant(1, true), suspension::SCHEDULE, "settled duty by duty");
}

#[test]
fn a_released_outcome_needs_the_executor() {
    reject_with(&owner_program(false), suspension::SCHEDULE, "needs the executor");
}

// --- SPB_ROOTS_SUSPEND ---------------------------------------------------------------------------------------

#[test]
fn values_kept_across_a_suspension_are_saved() {
    reject_with(&twice_program(false), suspension::ROOTS_SUSPEND, "lives across the suspension");
}

#[test]
fn operand_slots_hold_the_operands() {
    let mut p = plain();
    p.tables.frames[1].continuations[1].operands = vec![4];
    reject_with(&p, suspension::ROOTS_SUSPEND, "operand slots differ");
}

#[test]
fn one_slot_holds_one_value_of_a_suspension() {
    let mut p = plain();
    p.tables.frames[1].continuations[1].saved = vec![1, 1];
    reject_with(&p, suspension::ROOTS_SUSPEND, "holds two values");
    let mut p = plain();
    p.tables.frames[1].continuations[1].operands = vec![2];
    reject_with(&p, suspension::ROOTS_SUSPEND, "holds two values");
}

// --- SPB_FUTURE_TYPES ----------------------------------------------------------------------------------------

#[test]
fn future_types_are_distinct_and_declared() {
    let mut p = plain();
    p.tables.future_results.push(Some(Type::Int));
    reject_with(&p, verify::FUTURE_TYPES, "must be distinct");
    let mut p = plain();
    main_of(&mut p).registers.push(Type::Future(7));
    reject_with(&p, verify::FUTURE_TYPES, "names no future_results entry");
}

#[test]
fn await_needs_a_future() {
    let mut p = plain();
    let f = main_of(&mut p);
    let seven = f.registers.iter().position(|ty| *ty == Type::Int).unwrap() as u32;
    let wait = find(f, O::Await);
    arguments(f, wait)[0] = l(Location::Register(seven));
    reject_with(&p, verify::FUTURE_TYPES, "await needs a Future");
}

#[test]
fn fut_new_produces_its_body_s_future_type() {
    let mut p = plain();
    let f = main_of(&mut p);
    let new = find(f, O::FutureNew);
    arguments(f, new)[2] = A::Result(Some(FUTURE_VOID));
    reject_with(&p, verify::FUTURE_TYPES, "differs from the async body's result");
}

#[test]
fn no_object_holds_a_future() {
    let mut p = owner_program(true);
    let mut holder = res();
    holder.name = "Holder".into();
    holder.layout = ObjectRuntimeLayout { storage_fields: vec![FUTURE_INT], allocation_words: 2, identity_offset: None,
        has_destroy: false, may_reach_resource: false, may_async_destroy: false, trace_layout: vec![0],
        selection_layout: vec![] };
    p.objects.push(holder);
    reject_with(&p, verify::FUTURE_TYPES, "object storage cannot hold a Future");
}

// --- Existing rules over the new forms -----------------------------------------------------------------------

#[test]
fn private_tokens_are_not_entry_arguments() {
    for kind in [K::CacheState, K::TaskState, K::Receipt] {
        let mut p = owner_program(true);
        let f = &mut p.functions[3];
        let token = Location::Service(f.services.len() as u32);
        f.services.push(kind);
        f.params.push(token);
        f.blocks[0].params.push(token);
        reject_with(&p, "ENTRY_PRIVATE_SERVICE", "private service");
    }
}

/// An async body that commits a `Res` into a packet, then offers it to a plain @work or publishes it.
fn leaking(offer: bool) -> Program {
    let (mut b, frame) = Body::asynchronous("leak", 1, vec![]);
    b.block("entry", &[frame]);
    let (body, start) = b.resume(frame, 0, &[ProducedType::Service(K::Packet)]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(MAIN), A::Boundary(MAIN)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(start[0])], K::ScopeState);
    let created = b.register(Type::Object(0));
    let Location::Register(id) = created else { unreachable!() };
    b.f.code.push(Instruction::Data { op: DataOp::ObjNew, inputs: vec![], output: Some(id),
        immediate: Immediate::Object(0) });
    let packet = b.emit(O::OwnCreateCommit, vec![l(created), A::Object(0), A::Boundary(MAIN)], K::Packet);
    if offer {
        let seven = b.constant(Constant::Int("7".into()), Type::Int);
        let [future, cache, q2] = [b.register(FUTURE_INT), b.service(K::CacheState), b.service(K::ScopeState)];
        b.next(O::FutureNew, vec![A::Entry(0), A::Scope(MAIN), A::Result(Some(FUTURE_INT)), l(seven), l(packet),
            l(q1)], vec![future, cache, q2]);
        let deps = b.service(K::Deps);
        b.next(O::DepsEmpty, vec![], vec![deps]);
        let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
        b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(empty), l(deps)], vec![]);
    } else {
        let deps = b.service(K::Deps);
        b.next(O::DepsEmpty, vec![], vec![deps]);
        b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(packet), l(deps)], vec![]);
    }
    b.ret();
    let mut f = b.f;
    f.proofs.states.clear();
    let layout = FrameLayout {
        slots: vec![service(K::Packet)],
        continuations: vec![Continuation { site: None, saved: vec![0], operands: vec![] }],
    };
    let (work, work_layout) = work();
    let mut p = program(vec![work, f, destroy(0)], vec![work_layout, layout], None);
    p.objects.push(res());
    p
}

#[test]
fn plain_future_inputs_carry_no_obligation() {
    reject_with(&leaking(true), "CALL_PLAIN_PACKET", "plain Future inputs");
}

#[test]
fn a_plain_outcome_carries_no_obligation() {
    reject_with(&leaking(false), "RETURN_PLAIN_PACKET", "plain outcome");
}

#[test]
fn a_pending_exit_crosses_suspensions_in_the_frame() {
    verify_load(&plain_with(Main::PendingAcross), &NoExternalContracts).unwrap();
}

#[test]
fn released_obligations_count_toward_the_close_envelope() {
    verify_load(&owner_failing(true, 1, 1), &NoExternalContracts).unwrap();
    reject_with(&owner_failing(true, 1, 0), "D18_EXECUTOR", "omits destroy failures");
}

#[test]
fn an_async_body_returns_no_value() {
    let mut p = plain();
    let f = &mut p.functions[0];
    let result = f.registers.iter().rposition(|ty| *ty == Type::Int).unwrap() as u32;
    *f.code.last_mut().unwrap() = Instruction::Ret(vec![result]);
    reject_with(&p, suspension::ASYNC_ENTRY, "publishes instead of returning");
}

#[test]
fn fut_publish_is_followed_by_ret() {
    let mut p = plain();
    *p.functions[0].code.last_mut().unwrap() = Instruction::Unreachable;
    reject_with(&p, suspension::ASYNC_ENTRY, "followed by ret");
}

#[test]
fn the_published_value_has_the_future_type() {
    let mut p = plain();
    let f = &mut p.functions[0];
    let publish = find(f, O::FuturePublish);
    let deps = arguments(f, publish)[4].clone();
    arguments(f, publish)[2] = deps;
    reject_with(&p, verify::FUTURE_TYPES, "published value differs");
}

/// An async body with a start and one await. Unless `bracket`, the await has no frame.save; with
/// `stray_start`, the start is resumed twice. Its code is checked structurally only.
fn loose(bracket: bool, stray_start: bool) -> Program {
    let (mut b, frame) = Body::asynchronous("loose", 1, vec![]);
    b.block("entry", &[frame]);
    let (mut body, start) = b.resume(frame, 0, &[ProducedType::Service(K::Packet)]);
    if stray_start {
        // A second resume of the start.
        body = b.resume(body, 0, &[ProducedType::Service(K::Packet)]).0;
    }
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(MAIN), A::Boundary(MAIN)], K::ScopeState);
    let seven = b.constant(Constant::Int("7".into()), Type::Int);
    let [future, cache, q1] = [b.register(FUTURE_INT), b.service(K::CacheState), b.service(K::ScopeState)];
    b.next(O::FutureNew, vec![A::Entry(0), A::Scope(MAIN), A::Result(Some(FUTURE_INT)), l(seven), l(start[0]),
        l(q0)], vec![future, cache, q1]);
    let saved = bracket.then(|| b.save(body, 1, &[]));
    let site = b.pc();
    b.outcomes(O::Await, vec![l(future)], vec![(Tag::Normal, "ok", vec![p(0), p(1)]),
        (Tag::Unwind, "bad", vec![p(0), p(1), p(2)])]);
    let ok = [b.register(Type::Int), b.service(K::Deps)];
    b.block("ok", &ok);
    if let Some(saved) = saved {
        body = b.resume(saved, 1, &[]).0;
    }
    let deps = b.emit(O::DepsEmpty, vec![], K::Deps);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(empty), l(deps)], vec![]);
    b.ret();
    let bad = [b.register(Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad", &bad);
    if let Some(saved) = saved {
        body = b.resume(saved, 1, &[]).0;
    }
    b.next(O::FuturePublish, vec![l(body), A::Flag(false), l(bad[0])], vec![]);
    b.ret();
    let layout = FrameLayout {
        slots: vec![service(K::Packet), value(FUTURE_INT)],
        continuations: vec![Continuation { site: None, saved: vec![0], operands: vec![] },
            Continuation { site: Some(site), saved: vec![], operands: vec![1] }],
    };
    let (work, work_layout) = work();
    program(vec![work, b.finish_unchecked()], vec![work_layout, layout], None)
}

#[test]
fn a_suspending_operation_needs_its_save() {
    reject_with(&loose(false, false), suspension::SUSPEND_BRACKET, "has no frame.save");
}

#[test]
fn a_continuation_site_is_a_suspending_operation() {
    let mut p = plain();
    let new = find(main_of(&mut p), O::FutureNew) as u32;
    p.tables.frames[1].continuations.push(Continuation { site: Some(new), saved: vec![], operands: vec![] });
    reject_with(&p, suspension::SUSPEND_BRACKET, "site is not a suspending operation");
}

#[test]
fn the_start_is_resumed_once() {
    reject_with(&loose(true, true), suspension::RESUME_TABLE, "outside its continuation's successors");
}

#[test]
fn deferred_cancellation_follows_the_settle_phase() {
    reject_with(&plain_with(Main::CancelBeforeSettle), suspension::ASYNC_ENTRY, "after the settle phase");
}

// --- #601 V7: fixed roots, deps inputs and the D18 delivery (decisions L1–L3) ------------------------------------

/// `async main() { let x = await work(7); }` whose final close has no reserve, as a fixed root's (#667). With
/// `packet` it starts with an input packet like an ordinary body, without it starts empty like a fixed root. Its
/// final close delivers a deferred cancellation after its plan; with `cancel_on_failure` its failure close does too.
fn rooted(packet: bool, cancel_on_failure: bool) -> (Function, FrameLayout) {
    let slots = main_slots();
    let (mut b, frame) = Body::asynchronous("main", 1, vec![]);
    b.block("entry", &[frame]);
    let start_types = if packet { vec![ProducedType::Service(K::Packet)] } else { vec![] };
    let (body, start) = b.resume(frame, 0, &start_types);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(MAIN), A::Boundary(MAIN)], K::ScopeState);
    let q1 = if packet { b.emit(O::OwnAccept, vec![l(q0), l(start[0])], K::ScopeState) } else { q0 };
    let seven = b.constant(Constant::Int("7".into()), Type::Int);
    let offered = b.emit(O::OwnEmpty, vec![], K::Packet);
    let [future, cache, q2] = [b.register(FUTURE_INT), b.service(K::CacheState), b.service(K::ScopeState)];
    b.next(O::FutureNew, vec![A::Entry(0), A::Scope(MAIN), A::Result(Some(FUTURE_INT)), l(seven), l(offered),
        l(q1)], vec![future, cache, q2]);
    let first = b.save(body, 1, &[q2, cache]);
    b.outcomes(O::Await, vec![l(future)], vec![(Tag::Normal, "ok", vec![p(0), p(1)]),
        (Tag::Unwind, "bad", vec![p(0), p(1), p(2)])]);
    let [x, xd] = [b.register(Type::Int), b.service(K::Deps)];
    b.block("ok", &[x, xd]);
    let (body_ok, resumed) = b.resume(first, 1, &slot_types(&slots, &[1, 2]));
    let finished = b.close(resumed[0], None, None, MAIN, &[resumed[1]], ENVELOPE_CANCEL, false);
    let closed = b.service(K::Closed);
    b.dispatch(finished, [false, false, true], vec![(Tag::Success, "done", vec![p(0)]),
        (Tag::Failure, "cancelled", vec![p(0), p(1)])]);
    b.block("done", &[closed]);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let deps = b.emit(O::DepsEmpty, vec![], K::Deps);
    b.next(O::FuturePublish, vec![l(body_ok), A::Flag(true), l(empty), l(deps)], vec![]);
    b.ret();
    let [cancelled_closed, cancelled] = [b.service(K::Closed), b.register(Type::Failure)];
    b.block("cancelled", &[cancelled_closed, cancelled]);
    b.next(O::FuturePublish, vec![l(body_ok), A::Flag(false), l(cancelled)], vec![]);
    b.ret();
    let unwind = [b.register(Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad", &unwind);
    let (body_bad, resumed) = b.resume(first, 1, &slot_types(&slots, &[1, 2]));
    adapter(&mut b, &slots, body_bad, 2, resumed[0], unwind, resumed[1], None, "failed", cancel_on_failure);
    let start_saved = if packet { vec![0] } else { vec![] };
    let continuations = vec![
        Continuation { site: None, saved: start_saved, operands: vec![] },
        Continuation { site: Some(b.sites[&1]), saved: vec![1, 2], operands: vec![3] },
        Continuation { site: Some(b.sites[&2]), saved: vec![2], operands: vec![1, 4, 5, 6] },
    ];
    (b.finish(), FrameLayout { slots, continuations })
}

fn rooted_program(entry: Option<u32>, packet: bool, cancel_on_failure: bool) -> Program {
    let (work, work_layout) = work();
    let (main, main_layout) = rooted(packet, cancel_on_failure);
    program(vec![work, main], vec![work_layout, main_layout], entry)
}

#[test]
fn a_fixed_root_starts_empty_and_delivers_at_its_close() {
    verify_load(&rooted_program(Some(1), false, false), &NoExternalContracts).unwrap();
}

#[test]
fn a_fixed_root_starts_without_a_packet() {
    reject_with(&rooted_program(Some(1), true, false), suspension::ASYNC_ENTRY, "a fixed root starts without inputs");
    reject_with(&plain_with_entry(Some(1)), suspension::ASYNC_ENTRY, "a fixed root starts without inputs");
}

#[test]
fn an_ordinary_async_body_starts_with_its_packet() {
    reject_with(&rooted_program(None, false, false), suspension::ASYNC_ENTRY, "start holds the language inputs");
}

#[test]
fn fut_new_cannot_create_a_fixed_root() {
    let mut p = rooted_program(Some(1), false, false);
    let f = main_of(&mut p);
    let new = find(f, O::FutureNew);
    arguments(f, new)[0] = A::Entry(1);
    reject_with(&p, suspension::ASYNC_ENTRY, "fut.new cannot create a fixed root");
}

#[test]
fn only_a_fixed_root_delivers_without_a_reserve() {
    reject_with(&rooted_program(None, true, false), suspension::ASYNC_ENTRY, "only at the final exit");
}

#[test]
fn a_failure_close_never_delivers() {
    reject_with(&rooted_program(Some(1), false, true), suspension::ASYNC_ENTRY, "only at the final exit");
}

fn plain_with_entry(entry: Option<u32>) -> Program {
    let mut p = plain();
    p.entry = entry;
    p
}

/// `async work(n is int) is int { return n; }` whose start holds the deps of a borrowed input as well: after `n`,
/// or, with `deps_first`, before it.
fn work_with_deps(deps_first: bool) -> (Function, FrameLayout) {
    let (mut b, frame) = Body::asynchronous("work", 0, vec![Type::Int]);
    b.block("entry", &[frame]);
    let (int, deps) = (ProducedType::Value(Type::Int), ProducedType::Service(K::Deps));
    let types = if deps_first { [deps, int] } else { [int, deps] };
    let (body, start) = b.resume(frame, 0, &[types[0], types[1], ProducedType::Service(K::Packet)]);
    let n = if deps_first { start[1] } else { start[0] };
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(WORK), A::Boundary(WORK)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(start[2])], K::ScopeState);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(WORK), l(n), l(empty),
        A::Selection(selection)], K::Pending);
    let finished = b.close(q1, Some(pending), None, WORK, &[], 0, false);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let [result, packet, deps] = [b.register(Type::Int), b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Caller)], vec![result, packet, deps]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(result), l(packet), l(deps)], vec![]);
    b.ret();
    let slot = |ty: ProducedType| FrameSlot { ty, linear: false };
    let layout = FrameLayout {
        slots: vec![slot(types[0]), slot(types[1]), service(K::Packet)],
        continuations: vec![Continuation { site: None, saved: vec![0, 1, 2], operands: vec![] }],
    };
    (b.finish(), layout)
}

fn deps_program(deps_first: bool) -> Program {
    let (work, work_layout) = work_with_deps(deps_first);
    let (main, main_layout) = main(Main::Plain);
    program(vec![work, main], vec![work_layout, main_layout], None)
}

#[test]
fn the_start_holds_the_deps_of_its_inputs_before_the_packet() {
    verify_load(&deps_program(false), &NoExternalContracts).unwrap();
    reject_with(&deps_program(true), suspension::ASYNC_ENTRY, "start holds the language inputs");
}

#[test]
fn a_cancelling_close_delivers_after_its_plan_then_runs_the_aborted_reserve() {
    verify_load(&plain_with(Main::Delivers(Delivery::AfterPlan)), &NoExternalContracts).unwrap();
    let p = plain_with(Main::Delivers(Delivery::AfterPlan));
    let main = &p.functions[1];
    let cancelling = main.proofs.closes.iter().position(|close| {
        main.proofs.envelopes[close.envelope as usize].failures & ENVELOPE_CANCEL != 0
    }).unwrap() as u32;
    let plans = main.proofs.cleanup_plans.iter().filter(|plan| plan.close == cancelling).count();
    assert_eq!(plans, 2, "the plan and the aborted reserve's plan");
}

#[test]
fn a_delivery_follows_the_first_plan() {
    reject_with(&plain_with(Main::Delivers(Delivery::BeforePlan)), suspension::ASYNC_ENTRY,
        "after the settle phase and the first plan");
}

#[test]
fn a_deferred_cancellation_is_delivered_once() {
    reject_with(&plain_with(Main::Delivers(Delivery::Twice)), suspension::ASYNC_ENTRY, "is delivered once");
}

#[test]
fn a_delivery_runs_the_aborted_reserves_plan() {
    reject_with(&plain_with(Main::Delivers(Delivery::NoSecondPlan)), suspension::ASYNC_ENTRY,
        "runs the aborted reserve's plan before the close finishes");
    reject_with(&plain_with(Main::Delivers(Delivery::DirectSecondPlan)), suspension::ASYNC_ENTRY,
        "the aborted reserve runs its own executor plan");
}

#[test]
fn a_second_plan_follows_a_delivery_and_there_is_no_third() {
    reject_with(&plain_with(Main::Delivers(Delivery::UndeliveredSecondPlan)), suspension::ASYNC_ENTRY,
        "a second plan follows a delivered cancellation");
    reject_with(&plain_with(Main::Delivers(Delivery::ThirdPlan)), suspension::ASYNC_ENTRY, "at most two plans");
}

#[test]
fn a_settled_duty_is_dead_while_its_schedule_continues() {
    // Two duties of one schedule: after the first settle only the second duty and the cleanup state stay live.
    let p = lazy_program(2);
    let f = &p.functions[1];
    let settles = f.code.iter().enumerate().filter_map(|(pc, instruction)| match instruction {
        Instruction::Champ { op: O::CleanupSettle, arguments, .. } if arguments.len() == 3 => {
            let A::Location(duty) = arguments[2] else { panic!("duty operand") };
            Some((pc, duty))
        }
        _ => None,
    }).collect::<Vec<_>>();
    let [(_, first), (second_pc, second)] = settles[..] else { panic!("two duty settles") };
    let live = live_locations(f).unwrap();
    assert!(!live[second_pc].contains(&first), "the first duty was consumed by its settle");
    assert!(live[second_pc].contains(&second));
}

#[test]
fn a_delivery_needs_a_cancelling_envelope() {
    let mut p = plain_with(Main::Cancelling);
    main_of(&mut p).proofs.envelopes[0].failures = 0;
    reject_with(&p, suspension::ASYNC_ENTRY, "exactly when its envelope declares one: none here");
}

#[test]
fn a_cancelling_envelope_finishes_only_after_its_delivery() {
    let mut p = plain();
    main_of(&mut p).proofs.envelopes[0].failures = ENVELOPE_CANCEL;
    reject_with(&p, suspension::ASYNC_ENTRY, "exactly when its envelope declares one: never here");
}
