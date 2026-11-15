//! #601 V8 and V9: the Champ runtime and the JIT for plain await on assembled SPB 17 (Champ draft 3 §§2–7, 13;
//! plan §§3.6–3.7).
//!
//! Every scenario runs its async bodies twice: compiled by the JIT, and by a reference body that runs the same
//! SPB code with the same helpers under draft 3 §3 and ABI D20 §2.2 (resume dispatch on RESUME, continuation 0
//! read and cleared at the start, the saved and operand slots stored at PENDING with RESUME = k, reloaded and
//! cleared at resume, the step re-invoked). The reference body keeps its locations rooted across every helper
//! call, as a stack map does. The two engines must give the same trace, with the collector idle and with a
//! collection at every safepoint.
//!
//! Expectations come from draft 3 and Tomato: the counters of draft 2 §7.2 with a plain outcome, Tomato's
//! FIFO wakes and its deadlock report.
use super::*;
use super::cleanup::{rt_cleanup_begin, rt_cleanup_destroy, rt_cleanup_finish, rt_cleanup_next, rt_cleanup_plan};
use super::cleanup::{rt_cleanup_failure, rt_cleanup_settle_empty};
use super::futures::FutureStep;
use crate::runtime::{frame_pointer, Ctx, FailureKind, FAILED, OK};
use crate::tasks::{self, get, set};
use crate::word::{self, Repr, Word};
use okkama_sparrow::spb17::{self as s, Argument as A, Location, Opcode as O, ServiceKind as K, Tag};
use std::collections::BTreeMap;
use std::sync::Arc;

const FUTURE_INT: s::Type = s::Type::Future(0);
/// Function indices. Each async function's frame layout and lexical scope have its index.
const WORK: u32 = 0;
const FAILS: u32 = 1;
const CHAIN: u32 = 2;
const WATCH: u32 = 3;
const MAIN: u32 = 4;
const TWICE: u32 = 5;
const LAZY: u32 = 6;
const MAIN_FAILS: u32 = 7;
const CONSUME: u32 = 8;
const NESTED: u32 = 9;
const DESTROY: u32 = 10;
const ASYNC_FUNCTIONS: u32 = 10;
/// The lexical scope `nested` begins after its first suspension, a child of `nested`'s own scope.
const INNER: u32 = 10;

// --- SPB 17 programs ------------------------------------------------------------------------------------------

fn l(location: Location) -> A {
    A::Location(location)
}

fn p(slot: u32) -> s::Source {
    s::Source::ProducedSlot(slot)
}

fn value(ty: s::Type) -> s::FrameSlot {
    s::FrameSlot { ty: s::ProducedType::Value(ty), linear: false }
}

fn service(kind: K) -> s::FrameSlot {
    s::FrameSlot { ty: s::ProducedType::Service(kind), linear: kind.linear() }
}

fn types(slots: &[s::FrameSlot], indices: &[u32]) -> Vec<s::ProducedType> {
    indices.iter().map(|&slot| slots[slot as usize].ty).collect()
}

/// Builds one function block by block; edge targets stay labels until `finish`.
struct Body {
    f: s::Function,
    labels: BTreeMap<&'static str, u32>,
    targets: Vec<(usize, usize, &'static str, Vec<s::Source>)>,
    sites: BTreeMap<u32, u32>,
}

impl Body {
    fn new(symbol: &str, entry_abi: s::EntryAbi, results: Vec<s::Type>) -> Self {
        let f = s::Function {
            symbol: symbol.into(), entry_abi, registers: vec![], services: vec![], params: vec![], results,
            blocks: vec![], proofs: s::FunctionProofs::default(), code: vec![],
        };
        Self { f, labels: BTreeMap::new(), targets: vec![], sites: BTreeMap::new() }
    }

    /// An async body of the function with index `index`: its only parameter is its frame.
    fn asynchronous(symbol: &str, index: u32, results: Vec<s::Type>) -> (Self, Location) {
        let mut body = Self::new(symbol, s::EntryAbi::AsyncBody { layout: index }, results);
        let frame = body.service(K::Frame);
        body.f.params = vec![frame];
        body.block("entry", &[frame]);
        (body, frame)
    }

    fn register(&mut self, ty: s::Type) -> Location {
        self.f.registers.push(ty);
        Location::Register(self.f.registers.len() as u32 - 1)
    }

    fn service(&mut self, kind: K) -> Location {
        self.f.services.push(kind);
        Location::Service(self.f.services.len() as u32 - 1)
    }

    fn pc(&self) -> u32 {
        self.f.code.len() as u32
    }

    fn block(&mut self, label: &'static str, params: &[Location]) {
        self.labels.insert(label, self.pc());
        self.f.blocks.push(s::Block { pc: self.pc(), params: params.to_vec() });
    }

    fn next(&mut self, op: O, arguments: Vec<A>, outputs: Vec<Location>) {
        self.f.code.push(s::Instruction::Champ { op, arguments, flow: s::Flow::Next(outputs) });
    }

    fn emit(&mut self, op: O, arguments: Vec<A>, kind: K) -> Location {
        let output = self.service(kind);
        self.next(op, arguments, vec![output]);
        output
    }

    fn constant(&mut self, constant: s::Constant, ty: s::Type) -> Location {
        let output = self.register(ty);
        let Location::Register(id) = output else { unreachable!() };
        self.f.code.push(s::Instruction::Data { op: s::DataOp::Const, inputs: vec![], output: Some(id),
            immediate: s::Immediate::Constant(constant) });
        output
    }

    fn outcomes(&mut self, op: O, arguments: Vec<A>, edges: Vec<(Tag, &'static str, Vec<s::Source>)>) {
        let pc = self.f.code.len();
        let mut outcomes = Vec::new();
        for (ordinal, (tag, label, sources)) in edges.into_iter().enumerate() {
            self.targets.push((pc, ordinal, label, sources));
            outcomes.push(s::Outcome { tag, edge: s::Edge { target_pc: 0, assignments: vec![] } });
        }
        self.f.code.push(s::Instruction::Champ { op, arguments, flow: s::Flow::Outcomes(outcomes) });
    }

    fn ret(&mut self) {
        self.f.code.push(s::Instruction::Ret(vec![]));
    }

    fn selection(&mut self) -> u32 {
        self.f.proofs.selections.push(s::Selection { locations: vec![] });
        self.f.proofs.selections.len() as u32 - 1
    }

    /// `frame.save` of continuation `k`; the suspending operation follows at the site the layout names.
    fn save(&mut self, frame: Location, k: u32, values: &[Location]) -> Location {
        let mut arguments = vec![l(frame), A::Continuation(k)];
        arguments.extend(values.iter().copied().map(l));
        let saved = self.emit(O::FrameSave, arguments, K::Frame);
        self.sites.insert(k, self.pc());
        saved
    }

    /// `frame.resume` of continuation `k`: the frame, then the saved values of `types`.
    fn resume(&mut self, frame: Location, k: u32, types: &[s::ProducedType]) -> (Location, Vec<Location>) {
        let resumed = self.service(K::Frame);
        let values = types.iter().map(|ty| match *ty {
            s::ProducedType::Value(ty) => self.register(ty),
            s::ProducedType::Service(kind) => self.service(kind),
        }).collect::<Vec<_>>();
        let mut outputs = vec![resumed];
        outputs.extend(&values);
        self.next(O::FrameResume, vec![l(frame), A::Continuation(k)], outputs);
        (resumed, values)
    }

    /// A close with a direct plan over the scope's inventory (a merge executor when `merge`) that settles
    /// `duties` of a new schedule in order. Returns the `cleanup.finish` outputs.
    fn close(&mut self, state: Location, pending: Option<Location>, failure: Option<Location>, scope: u32,
        duties: &[Location], merge: bool) -> (Location, Location, Location)
    {
        let proofs = &mut self.f.proofs;
        let close = proofs.closes.len() as u32;
        let envelope = proofs.envelopes.len() as u32;
        proofs.envelopes.push(s::Envelope { failures: 0 });
        proofs.closes.push(s::CloseContract { begin_pc: self.f.code.len() as u32, boundary: scope,
            reserve: pending.is_some(), active_failure: failure.is_some(), envelope });
        let schedule = proofs.schedules.len() as u32;
        proofs.schedules.push(s::Schedule { services: duties.to_vec() });
        let mut arguments = vec![l(state)];
        arguments.extend(pending.into_iter().chain(failure).map(l));
        arguments.extend([A::Boundary(scope), A::Envelope(envelope)]);
        let mut cleanup = self.emit(O::CleanupBegin, arguments, K::Cleanup);
        if duties.is_empty() {
            cleanup = self.emit(O::CleanupSettle, vec![l(cleanup), A::Schedule(schedule)], K::Cleanup);
        }
        for &duty in duties {
            cleanup = self.emit(O::CleanupSettle, vec![l(cleanup), A::Schedule(schedule), l(duty)], K::Cleanup);
        }
        let selection = self.selection();
        let proofs = &mut self.f.proofs;
        proofs.order_witnesses.push(s::OrderWitness { creation_pcs: vec![] });
        let plan = proofs.plans.len() as u32;
        proofs.plans.push(s::Plan { merge, selection, order_witness: proofs.order_witnesses.len() as u32 - 1 });
        let mut sources = vec![s::MergeSource::Scope];
        if pending.is_some() {
            sources.push(s::MergeSource::Reserve);
        }
        proofs.cleanup_plans.push(s::CleanupPlan { plan, reserve: pending.is_some(), close, reserve_empty: !merge,
            sources, slots: vec![], preserves: vec![], joins: vec![], suffixes: vec![] });
        cleanup = self.emit(O::CleanupPlan, vec![l(cleanup), A::Plan(plan)], K::Cleanup);
        if merge {
            cleanup = self.executor(cleanup);
        }
        let outputs = [self.service(K::Closed), self.service(K::FailureState), self.service(K::ExitState)];
        self.next(O::CleanupFinish, vec![l(cleanup)], outputs.to_vec());
        (outputs[0], outputs[1], outputs[2])
    }

    /// The merge executor: destroy selected keys until done, capturing failures. Returns the done cleanup.
    /// Each executor of a function has its own labels.
    fn executor(&mut self, cleanup: Location) -> Location {
        let earlier = self.labels.keys().filter(|label| label.starts_with("x_loop")).count();
        let label = |name: &'static str| -> &'static str {
            if earlier == 0 { name } else { Box::leak(format!("{name}{earlier}").into_boxed_str()) }
        };
        let [x_loop, x_destroy, x_failed, x_finish] = [label("x_loop"), label("x_destroy"), label("x_failed"),
            label("x_finish")];
        let [looping, selected, key, destroyed, failure_state, failed, failure, recorded, done] = [
            self.service(K::Cleanup), self.service(K::Cleanup), self.service(K::StableKey), self.service(K::Cleanup),
            self.service(K::FailureState), self.service(K::Cleanup), self.register(s::Type::Failure),
            self.service(K::Cleanup), self.service(K::Cleanup),
        ];
        self.br(x_loop, vec![s::Source::Location(cleanup)]);
        self.block(x_loop, &[looping]);
        self.outcomes(O::CleanupNext, vec![l(looping)],
            vec![(Tag::Selected, x_destroy, vec![p(0), p(1)]), (Tag::Done, x_finish, vec![p(0)])]);
        self.block(x_destroy, &[selected, key]);
        self.next(O::CleanupDestroy, vec![l(selected), l(key)], vec![destroyed, failure_state]);
        self.outcomes(O::FailureDispatch, vec![l(failure_state)], vec![
            (Tag::None, x_loop, vec![s::Source::Location(destroyed)]),
            (Tag::Some, x_failed, vec![s::Source::Location(destroyed), p(0)]),
        ]);
        self.block(x_failed, &[failed, failure]);
        self.next(O::CleanupFailure, vec![l(failed), l(failure)], vec![recorded]);
        self.br(x_loop, vec![s::Source::Location(recorded)]);
        self.block(x_finish, &[done]);
        done
    }

    fn br(&mut self, label: &'static str, sources: Vec<s::Source>) {
        self.targets.push((self.f.code.len(), 0, label, sources));
        self.f.code.push(s::Instruction::Br(s::Edge { target_pc: 0, assignments: vec![] }));
    }

    fn dispatch(&mut self, finished: (Location, Location, Location), flags: [bool; 3],
        edges: Vec<(Tag, &'static str, Vec<s::Source>)>)
    {
        let (closed, failure, exit) = finished;
        let mut arguments = vec![l(closed), l(failure), l(exit)];
        arguments.extend(flags.map(A::Flag));
        self.outcomes(O::ExitDispatch, arguments, edges);
    }

    /// Return `value` (or nothing) through a close of `state` that settles `duties`, then publish it.
    fn return_value(&mut self, body: Location, state: Location, scope: u32, value: Option<Location>,
        duties: &[Location], label: &'static str)
    {
        let empty = self.emit(O::OwnEmpty, vec![], K::Packet);
        let selection = self.selection();
        let mut arguments = vec![A::Kind(s::ExitKind::Return), A::Boundary(scope)];
        arguments.extend(value.map(l));
        arguments.extend([l(empty), A::Selection(selection)]);
        let pending = self.emit(O::ExitPrepare, arguments, K::Pending);
        let finished = self.close(state, Some(pending), None, scope, duties, false);
        let [closed, reserved] = [self.service(K::Closed), self.service(K::Pending)];
        self.dispatch(finished, [true, false, false], vec![(Tag::Pending, label, vec![p(0), p(1)])]);
        self.block(label, &[closed, reserved]);
        let ty = value.map(|value| match value {
            Location::Register(id) => self.f.registers[id as usize],
            Location::Service(_) => unreachable!(),
        });
        let mut outputs = ty.map(|ty| vec![self.register(ty)]).unwrap_or_default();
        outputs.extend([self.service(K::Packet), self.service(K::Deps)]);
        self.next(O::ExitCommit, vec![l(reserved), A::Destination(s::ExitDestination::Caller)], outputs.clone());
        let mut published = vec![l(body), A::Flag(true)];
        published.extend(outputs.into_iter().map(l));
        self.next(O::FuturePublish, published, vec![]);
        self.ret();
    }

    /// Close `state` with the active `failure`, settling `duties`, and publish the combined failure.
    fn fail_close(&mut self, body: Location, state: Location, failure: Location, scope: u32, duties: &[Location],
        label: &'static str)
    {
        let finished = self.close(state, None, Some(failure), scope, duties, false);
        let [closed, combined] = [self.service(K::Closed), self.register(s::Type::Failure)];
        self.dispatch(finished, [false, true, false], vec![(Tag::Failure, label, vec![p(0), p(1)])]);
        self.block(label, &[closed, combined]);
        self.next(O::FuturePublish, vec![l(body), A::Flag(false), l(combined)], vec![]);
        self.ret();
    }

    /// `fail` with a program failure of `text`; its unwind enters a block that receives the failure.
    fn fail(&mut self, text: &str, label: &'static str) -> Location {
        let message = self.constant(s::Constant::Str(text.into()), s::Type::Str);
        self.outcomes(O::Fail, vec![A::Label(1), l(message)], vec![(Tag::Unwind, label, vec![p(0)])]);
        let failure = self.register(s::Type::Failure);
        self.block(label, &[failure]);
        failure
    }

    /// The `await.unwind` adapter at continuation `k` (form b): `kept` lives across it in the frame.
    fn adapter(&mut self, frame: Location, k: u32, state: Location, unwind: [Location; 3], kept: &[Location],
        kept_types: &[s::ProducedType]) -> (Location, Location, Location, Vec<Location>)
    {
        let saved = self.save(frame, k, kept);
        let [failure, task, receipt] = unwind;
        let [after, adapted] = [self.service(K::ScopeState), self.register(s::Type::Failure)];
        self.next(O::AwaitUnwind, vec![l(state), l(failure), l(task), l(receipt)], vec![after, adapted]);
        let (body, resumed) = self.resume(saved, k, kept_types);
        (body, after, adapted, resumed)
    }

    fn finish(mut self) -> s::Function {
        for (pc, ordinal, label, sources) in std::mem::take(&mut self.targets) {
            let target = self.labels[label];
            let params = self.f.blocks.iter().find(|block| block.pc == target).unwrap().params.clone();
            let edge = match &mut self.f.code[pc] {
                s::Instruction::Br(edge) => edge,
                s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. } => &mut outcomes[ordinal].edge,
                _ => unreachable!("edge"),
            };
            edge.target_pc = target;
            edge.assignments = params.into_iter().zip(sources)
                .map(|(destination, source)| s::Assignment { destination, source }).collect();
        }
        let f = &mut self.f;
        f.proofs.states = s::service_states(f).unwrap();
        let live = s::live_locations(f).unwrap();
        f.proofs.roots = live.iter().enumerate().map(|(pc, locations)| s::RootRequirement {
            pc: pc as u32, locations: locations.iter().copied().collect(),
        }).collect();
        for pc in 0..f.code.len().saturating_sub(1) {
            if matches!(f.code[pc], s::Instruction::Champ { flow: s::Flow::Next(_), .. }) {
                f.proofs.post_roots.push(s::RootRequirement {
                    pc: pc as u32, locations: live[pc + 1].iter().copied().collect(),
                });
            }
        }
        self.f
    }

    fn layout(&self, slots: Vec<s::FrameSlot>, continuations: Vec<(Vec<u32>, Vec<u32>)>) -> s::FrameLayout {
        let continuations = continuations.into_iter().enumerate().map(|(k, (saved, operands))| s::Continuation {
            site: (k > 0).then(|| self.sites[&(k as u32)]), saved, operands,
        }).collect();
        s::FrameLayout { slots, continuations }
    }
}

/// `async work(n: int) is int { return n; }`.
fn work() -> (s::Function, s::FrameLayout) {
    let (mut b, frame) = Body::asynchronous("work", WORK, vec![s::Type::Int]);
    let start = [s::ProducedType::Value(s::Type::Int), s::ProducedType::Service(K::Packet)];
    let (body, inputs) = b.resume(frame, 0, &start);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(WORK), A::Boundary(WORK)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[1])], K::ScopeState);
    b.return_value(body, q1, WORK, Some(inputs[0]), &[], "done");
    let layout = b.layout(vec![value(s::Type::Int), service(K::Packet)], vec![(vec![0, 1], vec![])]);
    (b.finish(), layout)
}

/// `async fails() is int { fail "boom"; }`.
fn fails() -> (s::Function, s::FrameLayout) {
    let (mut b, frame) = Body::asynchronous("fails", FAILS, vec![s::Type::Int]);
    let (body, inputs) = b.resume(frame, 0, &[s::ProducedType::Service(K::Packet)]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(FAILS), A::Boundary(FAILS)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[0])], K::ScopeState);
    let failure = b.fail("boom", "raised");
    b.fail_close(body, q1, failure, FAILS, &[], "out");
    let layout = b.layout(vec![service(K::Packet)], vec![(vec![0], vec![])]);
    (b.finish(), layout)
}

/// Slots of a body that awaits its Future parameter: 0 the Future, 1 inputs, 2 owner state, 3–5 the unwind.
fn parameter_slots() -> Vec<s::FrameSlot> {
    vec![value(FUTURE_INT), service(K::Packet), service(K::ScopeState), value(s::Type::Failure),
        service(K::TaskState), service(K::Receipt)]
}

/// `async chain(g: Future<int>) is int { await g; fail "chain"; }` (`fails` is false) or
/// `async watch(f: Future<int>) is int { return await f; }`.
fn awaits_parameter(symbol: &str, index: u32, fails: bool) -> (s::Function, s::FrameLayout) {
    let slots = parameter_slots();
    let (mut b, frame) = Body::asynchronous(symbol, index, vec![s::Type::Int]);
    let (body, inputs) = b.resume(frame, 0, &types(&slots, &[0, 1]));
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(index), A::Boundary(index)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[1])], K::ScopeState);
    let first = b.save(body, 1, &[q1]);
    b.outcomes(O::Await, vec![l(inputs[0])], vec![(Tag::Normal, "ok", vec![p(0), p(1)]),
        (Tag::Unwind, "bad", vec![p(0), p(1), p(2)])]);
    let [x, deps] = [b.register(s::Type::Int), b.service(K::Deps)];
    b.block("ok", &[x, deps]);
    let (body_ok, resumed) = b.resume(first, 1, &types(&slots, &[2]));
    if fails {
        let failure = b.fail("chain", "raised");
        b.fail_close(body_ok, resumed[0], failure, index, &[], "out");
    } else {
        b.return_value(body_ok, resumed[0], index, Some(x), &[], "done");
    }
    let unwind = [b.register(s::Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad", &unwind);
    let (body_bad, resumed) = b.resume(first, 1, &types(&slots, &[2]));
    let (body_adapted, after, adapted, _) = b.adapter(body_bad, 2, resumed[0], unwind, &[], &[]);
    b.fail_close(body_adapted, after, adapted, index, &[], "failed");
    let layout = b.layout(slots, vec![(vec![0, 1], vec![]), (vec![2], vec![0]), (vec![], vec![2, 3, 4, 5])]);
    (b.finish(), layout)
}

/// Slots of a body that creates and awaits a Future: 0 inputs, 1 owner state, 2 cache duty, 3 the Future,
/// 4–6 the unwind.
fn creator_slots() -> Vec<s::FrameSlot> {
    vec![service(K::Packet), service(K::ScopeState), service(K::CacheState), value(FUTURE_INT),
        value(s::Type::Failure), service(K::TaskState), service(K::Receipt)]
}

/// `fut.new` of `entry` with `args` in the owner state `state`: (future, cache duty, state).
fn future_new(b: &mut Body, scope: u32, entry: u32, args: &[Location], state: Location)
    -> (Location, Location, Location)
{
    let offered = b.emit(O::OwnEmpty, vec![], K::Packet);
    let [future, cache, next] = [b.register(FUTURE_INT), b.service(K::CacheState), b.service(K::ScopeState)];
    let mut arguments = vec![A::Entry(entry), A::Scope(scope), A::Result(Some(FUTURE_INT))];
    arguments.extend(args.iter().copied().map(l));
    arguments.extend([l(offered), l(state)]);
    b.next(O::FutureNew, arguments, vec![future, cache, next]);
    (future, cache, next)
}

/// `async main() is int { return await work(7); }` (entry `WORK`) or `return await fails();` (entry `FAILS`).
/// The cache duty settles at the close of either path.
fn awaits_created(symbol: &str, index: u32, entry: u32) -> (s::Function, s::FrameLayout) {
    let slots = creator_slots();
    let (mut b, frame) = Body::asynchronous(symbol, index, vec![s::Type::Int]);
    let (body, inputs) = b.resume(frame, 0, &types(&slots, &[0]));
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(index), A::Boundary(index)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[0])], K::ScopeState);
    let args = if entry == WORK { vec![b.constant(s::Constant::Int("7".into()), s::Type::Int)] } else { vec![] };
    let (future, cache, q2) = future_new(&mut b, index, entry, &args, q1);
    let first = b.save(body, 1, &[q2, cache]);
    b.outcomes(O::Await, vec![l(future)], vec![(Tag::Normal, "ok", vec![p(0), p(1)]),
        (Tag::Unwind, "bad", vec![p(0), p(1), p(2)])]);
    let [x, deps] = [b.register(s::Type::Int), b.service(K::Deps)];
    b.block("ok", &[x, deps]);
    let (body_ok, resumed) = b.resume(first, 1, &types(&slots, &[1, 2]));
    b.return_value(body_ok, resumed[0], index, Some(x), &[resumed[1]], "done");
    let unwind = [b.register(s::Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad", &unwind);
    let (body_bad, resumed) = b.resume(first, 1, &types(&slots, &[1, 2]));
    let (body_adapted, after, adapted, kept) =
        b.adapter(body_bad, 2, resumed[0], unwind, &[resumed[1]], &types(&slots, &[2]));
    b.fail_close(body_adapted, after, adapted, index, &[kept[0]], "failed");
    let layout = b.layout(slots, vec![(vec![0], vec![]), (vec![1, 2], vec![3]), (vec![2], vec![1, 4, 5, 6])]);
    (b.finish(), layout)
}

/// `async twice() is int { let f = work(7); let a = await f; let b = await f; return a + b; }`: the Future is
/// the operand of both awaits and the first result is saved across the second.
fn twice() -> (s::Function, s::FrameLayout) {
    let mut slots = creator_slots();
    slots.push(value(s::Type::Int));
    let (mut b, frame) = Body::asynchronous("twice", TWICE, vec![s::Type::Int]);
    let (body, inputs) = b.resume(frame, 0, &types(&slots, &[0]));
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(TWICE), A::Boundary(TWICE)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[0])], K::ScopeState);
    let seven = b.constant(s::Constant::Int("7".into()), s::Type::Int);
    let (future, cache, q2) = future_new(&mut b, TWICE, WORK, &[seven], q1);
    let first = b.save(body, 1, &[q2, cache]);
    b.outcomes(O::Await, vec![l(future)], vec![(Tag::Normal, "first", vec![p(0), p(1)]),
        (Tag::Unwind, "first_bad", vec![p(0), p(1), p(2)])]);
    let [a, a_deps] = [b.register(s::Type::Int), b.service(K::Deps)];
    b.block("first", &[a, a_deps]);
    let (body_first, resumed) = b.resume(first, 1, &types(&slots, &[1, 2]));
    let second = b.save(body_first, 2, &[a, resumed[0], resumed[1]]);
    b.outcomes(O::Await, vec![l(future)], vec![(Tag::Normal, "second", vec![p(0), p(1)]),
        (Tag::Unwind, "second_bad", vec![p(0), p(1), p(2)])]);
    let [c, c_deps] = [b.register(s::Type::Int), b.service(K::Deps)];
    b.block("second", &[c, c_deps]);
    let (body_second, resumed) = b.resume(second, 2, &types(&slots, &[7, 1, 2]));
    let sum = b.register(s::Type::Int);
    let (Location::Register(left), Location::Register(right), Location::Register(out)) = (resumed[0], c, sum) else {
        unreachable!()
    };
    b.f.code.push(s::Instruction::Data { op: s::DataOp::IntAdd, inputs: vec![left, right], output: Some(out),
        immediate: s::Immediate::None });
    b.return_value(body_second, resumed[1], TWICE, Some(sum), &[resumed[2]], "done");
    for (k, label, saved) in [(3, "first_bad", vec![1, 2]), (4, "second_bad", vec![7, 1, 2])] {
        let unwind = [b.register(s::Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
        b.block(label, &unwind);
        let source = if k == 3 { first } else { second };
        let (body_bad, resumed) = b.resume(source, k - 2, &types(&slots, &saved));
        let rest = &resumed[resumed.len() - 2..];
        let (body_adapted, after, adapted, kept) =
            b.adapter(body_bad, k, rest[0], unwind, &[rest[1]], &types(&slots, &[2]));
        b.fail_close(body_adapted, after, adapted, TWICE, &[kept[0]], if k == 3 { "failed" } else { "failed2" });
    }
    let layout = b.layout(slots, vec![(vec![0], vec![]), (vec![1, 2], vec![3]), (vec![7, 1, 2], vec![3]),
        (vec![2], vec![1, 4, 5, 6]), (vec![2], vec![1, 4, 5, 6])]);
    (b.finish(), layout)
}

/// `async lazy() is bool { let f = work(7); let g = work(7); f.cancel(); return f.cancelled(); }`: neither
/// Future is awaited and both cache duties settle at the close.
fn lazy() -> (s::Function, s::FrameLayout) {
    let (mut b, frame) = Body::asynchronous("lazy", LAZY, vec![s::Type::Bool]);
    let (body, inputs) = b.resume(frame, 0, &[s::ProducedType::Service(K::Packet)]);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(LAZY), A::Boundary(LAZY)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[0])], K::ScopeState);
    let seven = b.constant(s::Constant::Int("7".into()), s::Type::Int);
    let (f, f_cache, q2) = future_new(&mut b, LAZY, WORK, &[seven], q1);
    let (_, g_cache, q3) = future_new(&mut b, LAZY, WORK, &[seven], q2);
    let done = b.register(s::Type::Bool);
    b.next(O::FutureDone, vec![l(f)], vec![done]);
    b.next(O::FutureCancel, vec![l(f)], vec![]);
    let cancelled = b.register(s::Type::Bool);
    b.next(O::FutureCancelled, vec![l(f)], vec![cancelled]);
    b.return_value(body, q3, LAZY, Some(cancelled), &[f_cache, g_cache], "done");
    let layout = b.layout(vec![service(K::Packet)], vec![(vec![0], vec![])]);
    (b.finish(), layout)
}

/// `async consume(r: Res) { }`: its inputs hold `r`'s obligation; its own close merges.
fn consume() -> (s::Function, s::FrameLayout) {
    let (mut b, frame) = Body::asynchronous("consume", CONSUME, vec![]);
    let start = [s::ProducedType::Value(s::Type::Object(0)), s::ProducedType::Service(K::Packet)];
    let (body, inputs) = b.resume(frame, 0, &start);
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(CONSUME), A::Boundary(CONSUME)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[1])], K::ScopeState);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let pending = b.emit(O::ExitPrepare, vec![A::Kind(s::ExitKind::Return), A::Boundary(CONSUME), l(empty),
        A::Selection(selection)], K::Pending);
    let finished = b.close(q1, Some(pending), None, CONSUME, &[], true);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let [packet, deps] = [b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(s::ExitDestination::Caller)], vec![packet, deps]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(packet), l(deps)], vec![]);
    b.ret();
    let layout = b.layout(vec![value(s::Type::Object(0)), service(K::Packet)], vec![(vec![0, 1], vec![])]);
    (b.finish(), layout)
}

/// `async nested(g1: Future<int>, g2: Future<int>) is int { await g1; { return await g2; } }`: the inner scope
/// begins after the first suspension, so its parent owner comes from the scope area that resume reloads, and it
/// is open across the second.
fn nested() -> (s::Function, s::FrameLayout) {
    // 0 g1, 1 g2, 2 inputs, 3 the outer state, 4 the inner state, 5–7 the unwind.
    let slots = vec![value(FUTURE_INT), value(FUTURE_INT), service(K::Packet), service(K::ScopeState),
        service(K::ScopeState), value(s::Type::Failure), service(K::TaskState), service(K::Receipt)];
    let (mut b, frame) = Body::asynchronous("nested", NESTED, vec![s::Type::Int]);
    let (body, inputs) = b.resume(frame, 0, &types(&slots, &[0, 1, 2]));
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(NESTED), A::Boundary(NESTED)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs[2])], K::ScopeState);
    let first = b.save(body, 1, &[q1, inputs[1]]);
    b.outcomes(O::Await, vec![l(inputs[0])], vec![(Tag::Normal, "ok1", vec![p(0), p(1)]),
        (Tag::Unwind, "bad1", vec![p(0), p(1), p(2)])]);
    let [x, x_deps] = [b.register(s::Type::Int), b.service(K::Deps)];
    b.block("ok1", &[x, x_deps]);
    let (body_first, resumed) = b.resume(first, 1, &types(&slots, &[3, 1]));
    let i0 = b.emit(O::ScopeBegin, vec![A::Scope(INNER), A::Boundary(INNER)], K::ScopeState);
    let second = b.save(body_first, 2, &[i0, resumed[0]]);
    b.outcomes(O::Await, vec![l(resumed[1])], vec![(Tag::Normal, "ok2", vec![p(0), p(1)]),
        (Tag::Unwind, "bad2", vec![p(0), p(1), p(2)])]);
    let [y, y_deps] = [b.register(s::Type::Int), b.service(K::Deps)];
    b.block("ok2", &[y, y_deps]);
    let (body_second, resumed) = b.resume(second, 2, &types(&slots, &[4, 3]));
    let finished = b.close(resumed[0], None, None, INNER, &[], false);
    let closed = b.service(K::Closed);
    b.dispatch(finished, [false, false, false], vec![(Tag::Success, "inner_closed", vec![p(0)])]);
    b.block("inner_closed", &[closed]);
    b.return_value(body_second, resumed[1], NESTED, Some(y), &[], "done");
    let unwind = [b.register(s::Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad2", &unwind);
    let (body_bad, resumed) = b.resume(second, 2, &types(&slots, &[4, 3]));
    let (body_adapted, after, adapted, kept) =
        b.adapter(body_bad, 4, resumed[0], unwind, &[resumed[1]], &types(&slots, &[3]));
    let finished = b.close(after, None, Some(adapted), INNER, &[], false);
    let [closed, combined] = [b.service(K::Closed), b.register(s::Type::Failure)];
    b.dispatch(finished, [false, true, false], vec![(Tag::Failure, "inner_failed", vec![p(0), p(1)])]);
    b.block("inner_failed", &[closed, combined]);
    b.fail_close(body_adapted, kept[0], combined, NESTED, &[], "failed2");
    let unwind = [b.register(s::Type::Failure), b.service(K::TaskState), b.service(K::Receipt)];
    b.block("bad1", &unwind);
    let (body_bad, resumed) = b.resume(first, 1, &types(&slots, &[3, 1]));
    let (body_adapted, after, adapted, _) = b.adapter(body_bad, 3, resumed[0], unwind, &[], &[]);
    b.fail_close(body_adapted, after, adapted, NESTED, &[], "failed");
    let layout = b.layout(slots, vec![(vec![0, 1, 2], vec![]), (vec![3, 1], vec![0]), (vec![4, 3], vec![1]),
        (vec![], vec![3, 5, 6, 7]), (vec![3], vec![4, 5, 6, 7])]);
    (b.finish(), layout)
}

/// The destroy entry of `Res`.
fn destroy() -> s::Function {
    let mut b = Body::new("destroy", s::EntryAbi::Observation, vec![]);
    let target = b.register(s::Type::Object(0));
    b.f.params = vec![target];
    b.f.proofs.entry = Some(s::EntryContract { destroy: Some(0), ..s::EntryContract::default() });
    b.block("entry", &[target]);
    b.ret();
    b.finish()
}

/// Object 0: `Res`, an identity with a destroy.
fn res() -> s::Object {
    s::Object {
        name: "Res".into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: false, role: None,
        implements: vec![], any_box: false, enum_names: vec![], fields: vec![],
        layout: s::ObjectRuntimeLayout { storage_fields: vec![], allocation_words: 5, identity_offset: Some(8),
            has_destroy: true, may_reach_resource: true, may_async_destroy: false, trace_layout: vec![],
            selection_layout: vec![] },
    }
}

fn program() -> s::Program {
    let mut functions = Vec::new();
    let mut frames = Vec::new();
    for (function, layout) in [work(), fails(), awaits_parameter("chain", CHAIN, true),
        awaits_parameter("watch", WATCH, false), awaits_created("main", MAIN, WORK), twice(), lazy(),
        awaits_created("main_fails", MAIN_FAILS, FAILS), consume(), nested()]
    {
        functions.push(function);
        frames.push(layout);
    }
    functions.push(destroy());
    s::Program {
        module: "futures".into(),
        objects: vec![res()],
        functions,
        tables: s::ChampTables {
            scopes: (0..ASYNC_FUNCTIONS).map(|index| s::Scope { function: index, parent: None, boundary: index })
                .chain([s::Scope { function: NESTED, parent: Some(NESTED), boundary: INNER }])
                .collect(),
            boundaries: (0..=INNER).map(|scope| s::Boundary { scope }).collect(),
            frames,
            future_results: vec![Some(s::Type::Int), None],
            ..s::ChampTables::default()
        },
        ..s::Program::default()
    }
}

/// Who runs the async bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Engine {
    /// The Champ JIT (V9).
    Jit,
    /// The reference body below, the oracle.
    Reference,
}

/// Each engine, with the collector idle and with a collection at every safepoint.
fn configurations() -> impl Iterator<Item = (bool, Engine)> {
    [false, true].into_iter().flat_map(|stress| [Engine::Jit, Engine::Reference].map(|engine| (stress, engine)))
}

/// The program, through its text form, on a Champ runtime whose async bodies `engine` runs.
fn load(stress: bool, engine: Engine) -> Box<Runtime> {
    let built = program();
    s::verify_load(&built, &s::NoExternalContracts).unwrap();
    let assembled = s::assemble(&s::disassemble(&built).unwrap()).unwrap();
    assert_eq!(assembled, built);
    let mut runtime = Runtime::new_champ(assembled, Vec::new()).unwrap();
    runtime.heap.set_stress(stress);
    if engine == Engine::Reference {
        for function in 0..ASYNC_FUNCTIONS as usize {
            runtime.invocations[function].code = reference as *const () as u64;
        }
    }
    runtime
}

/// The steps of a run, by function: frames differ between runs, functions do not.
type Trace = Vec<(FutureStep, usize)>;

fn trace(runtime: &Runtime) -> Trace {
    runtime.champ.future_events.iter().map(|event| (event.step, event.function)).collect()
}

/// The traces of one scenario in every configuration: both engines, with and without collections, agree.
#[derive(Default)]
struct Traces(Vec<((bool, Engine), Trace)>);

impl Traces {
    fn record(&mut self, configuration: (bool, Engine), runtime: &Runtime) {
        // Every body that ran was the engine's: machine code from the JIT, or the reference body.
        let reference = reference as *const () as u64;
        for event in runtime.champ.future_events.iter().filter(|event| event.step == FutureStep::Start) {
            let code = runtime.invocations[event.function].code;
            assert_ne!(code, 0);
            assert_eq!(code == reference, configuration.1 == Engine::Reference, "{configuration:?}");
        }
        self.0.push((configuration, trace(runtime)));
    }

    fn agree(&self) {
        let (first, expected) = &self.0[0];
        assert!(!expected.is_empty());
        for (configuration, trace) in &self.0[1..] {
            assert_eq!(trace, expected, "{configuration:?} against {first:?}");
        }
    }
}

// --- The reference body -----------------------------------------------------------------------------------------

/// What one instruction does to the activation.
enum Step {
    Next(usize),
    Done,
    Pending,
    Stop,
}

/// One activation of an async body: its locations, by register and service, and its scope area.
struct Activation {
    ctx: *mut Ctx,
    frame: Word,
    index: usize,
    program: Arc<s::Program>,
    map: FrameMap,
    registers: Vec<Word>,
    services: Vec<Vec<Word>>,
    scopes: Vec<[Word; 2]>,
}

fn locations(arguments: &[A]) -> Vec<Location> {
    arguments.iter().filter_map(|argument| match argument {
        A::Location(location) => Some(*location),
        _ => None,
    }).collect()
}

fn words(kind: K) -> usize {
    s::layout::service_layout(kind).words as usize
}

/// `(ctx, frame) -> 0 | 1`, the async body ABI (draft 3 §13).
extern "C" fn reference(ctx: *mut Ctx, frame: Word) -> Word {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let index = runtime.function_of(frame);
    let program = runtime.champ.machine.as_ref().unwrap().source.clone();
    let map = runtime.champ_frame_map(index).unwrap().clone();
    let function = &program.functions[index];
    let mut activation = Activation {
        ctx, frame, index, map,
        registers: vec![0; function.registers.len()],
        services: function.services.iter().map(|&kind| vec![0; words(kind)]).collect(),
        scopes: vec![[0; 2]; program.tables.scopes.len()],
        program,
    };
    let k = get(frame, tasks::RESUME);
    let mut pc = if k == 0 { 0 } else { activation.reload(k as u32) };
    loop {
        match activation.step(pc) {
            Step::Next(next) => pc = next,
            Step::Done => return 0,
            Step::Pending => return 1,
            Step::Stop => return 0,
        }
    }
}

/// The runtime behind a context the reference body was entered with.
fn runtime_of<'a>(ctx: *mut Ctx) -> &'a mut Runtime {
    unsafe { &mut *(*ctx).runtime }
}

impl Activation {

    fn function(&self) -> &s::Function {
        &self.program.functions[self.index]
    }

    fn layout(&self) -> &s::FrameLayout {
        self.program.frame_layout(self.function()).unwrap()
    }

    fn read(&self, location: Location) -> Vec<Word> {
        match location {
            Location::Register(id) => vec![self.registers[id as usize]],
            Location::Service(id) => self.services[id as usize].clone(),
        }
    }

    fn word(&self, location: Location) -> Word {
        self.read(location).first().copied().unwrap_or(0)
    }

    fn write(&mut self, location: Location, values: &[Word]) {
        match location {
            Location::Register(id) => self.registers[id as usize] = values[0],
            Location::Service(id) => self.services[id as usize].copy_from_slice(values),
        }
    }

    /// The frame.save of continuation `k` and its saved locations, and the site's operand locations.
    fn bracket(&self, k: u32) -> (Vec<Location>, Vec<Location>) {
        let site = self.layout().continuations[k as usize].site.unwrap() as usize;
        let code = &self.function().code;
        let (s::Instruction::Champ { arguments: saved, .. }, s::Instruction::Champ { arguments: step, .. }) =
            (&code[site - 1], &code[site]) else { unreachable!("verified bracket") };
        (locations(saved)[1..].to_vec(), locations(step))
    }

    /// Store `saved ∪ operands` of continuation `k` and the scope area into the frame: PENDING (D20 §2.2).
    fn suspend(&mut self, k: u32) -> Step {
        let (saved, operands) = self.bracket(k);
        let continuation = self.layout().continuations[k as usize].clone();
        let pairs = continuation.saved.iter().zip(&saved).chain(continuation.operands.iter().zip(&operands));
        for (&slot, &location) in pairs {
            let offset = self.map.slots[slot as usize].offset;
            for (word, value) in self.read(location).into_iter().enumerate() {
                set(self.frame, offset + 8 * word as u64, value);
            }
        }
        for &(scope, offset) in &self.map.scopes {
            set(self.frame, offset, self.scopes[scope as usize][0]);
            set(self.frame, offset + 8, self.scopes[scope as usize][1]);
        }
        set(self.frame, tasks::RESUME, u64::from(k));
        Step::Pending
    }

    /// Reload what `suspend` stored, clearing those words, and return the site to re-invoke.
    fn reload(&mut self, k: u32) -> usize {
        let (saved, operands) = self.bracket(k);
        let continuation = self.layout().continuations[k as usize].clone();
        let pairs = continuation.saved.iter().zip(&saved).chain(continuation.operands.iter().zip(&operands));
        for (&slot, &location) in pairs {
            let slot = self.map.slots[slot as usize].clone();
            let values = (0..slot.reprs.len() as u64).map(|word| get(self.frame, slot.offset + 8 * word))
                .collect::<Vec<_>>();
            for word in 0..slot.reprs.len() as u64 {
                set(self.frame, slot.offset + 8 * word, 0);
            }
            self.write(location, &values);
        }
        for &(scope, offset) in &self.map.scopes {
            self.scopes[scope as usize] = [get(self.frame, offset), get(self.frame, offset + 8)];
            set(self.frame, offset, 0);
            set(self.frame, offset + 8, 0);
        }
        continuation.site.unwrap() as usize
    }

    /// Root every location across a helper call, as a stack map does; returns the roots' previous length.
    fn root(&self) -> usize {
        let function = self.function();
        let runtime = runtime_of(self.ctx);
        let base = runtime.roots.len();
        for (id, &word) in self.registers.iter().enumerate() {
            let repr = machine::repr(function.registers[id]);
            if repr != Repr::Raw && word != 0 {
                runtime.roots.push((word, repr));
            }
        }
        for (id, values) in self.services.iter().enumerate() {
            let references = s::layout::service_layout(function.services[id]).references;
            for &offset in references {
                let word = values[offset as usize / 8];
                if word != 0 {
                    runtime.roots.push((word, Repr::Ref));
                }
            }
        }
        base
    }

    fn call<T>(&self, helper: impl FnOnce(*mut Ctx) -> T) -> T {
        let base = self.root();
        let result = helper(self.ctx);
        runtime_of(self.ctx).roots.truncate(base);
        result
    }

    fn edge(&mut self, edge: &s::Edge, produced: &[Vec<Word>]) -> Step {
        let values = edge.assignments.iter().map(|assignment| match assignment.source {
            s::Source::Location(location) => self.read(location),
            s::Source::ProducedSlot(slot) => produced[slot as usize].clone(),
        }).collect::<Vec<_>>();
        for (assignment, values) in edge.assignments.iter().zip(values) {
            self.write(assignment.destination, &values);
        }
        Step::Next(edge.target_pc as usize)
    }

    fn outcome(&mut self, flow: &s::Flow, tag: Tag, produced: &[Vec<Word>]) -> Step {
        let s::Flow::Outcomes(outcomes) = flow else { unreachable!("verified outcomes") };
        let edge = outcomes.iter().find(|outcome| outcome.tag == tag).expect("verified outcome").edge.clone();
        self.edge(&edge, produced)
    }

    /// The failure of an Unwind outcome, traced at this pc and taken off the bridge, as the JIT does.
    fn land(&self, pc: usize) {
        crate::runtime::rt_trace(self.ctx, self.index as u64, pc as u64, self.frame);
        unsafe {
            (*self.ctx).status = OK;
            (*self.ctx).failure = 0;
        }
    }

    fn step(&mut self, pc: usize) -> Step {
        let instruction = self.function().code[pc].clone();
        match instruction {
            s::Instruction::Data { op: s::DataOp::Const, output: Some(output), immediate, .. } => {
                let value = match immediate {
                    s::Immediate::Constant(s::Constant::Int(text)) => word::small(text.parse().unwrap()).unwrap(),
                    s::Immediate::Constant(s::Constant::Str(text)) => runtime_of(self.ctx).heap.allocate_text(&text),
                    s::Immediate::Constant(s::Constant::Bool(flag)) => u64::from(flag),
                    other => panic!("reference body: constant {other:?}"),
                };
                self.registers[output as usize] = value;
                Step::Next(pc + 1)
            }
            s::Instruction::Data { op: s::DataOp::ObjNew, output: Some(output), immediate: s::Immediate::Object(id),
                .. } => {
                self.registers[output as usize] = runtime_of(self.ctx).heap.object_new(id, &[]);
                Step::Next(pc + 1)
            }
            s::Instruction::Data { op: s::DataOp::IntAdd, inputs, output: Some(output), .. } => {
                let value = |id: u32| word::small_value(self.registers[id as usize]).unwrap();
                self.registers[output as usize] = word::small(value(inputs[0]) + value(inputs[1])).unwrap();
                Step::Next(pc + 1)
            }
            s::Instruction::Br(edge) => self.edge(&edge, &[]),
            s::Instruction::Ret(_) => Step::Done,
            s::Instruction::Champ { op, arguments, flow } => self.champ(pc, op, &arguments, &flow),
            other => panic!("reference body: {other:?}"),
        }
    }

    /// A Champ instruction: its produced values go to its outputs, or it transfers control itself.
    fn champ(&mut self, pc: usize, op: O, arguments: &[A], flow: &s::Flow) -> Step {
        let produced = match self.operation(pc, op, arguments, flow) {
            Ok(produced) => produced,
            Err(step) => return step,
        };
        if let s::Flow::Next(outputs) = flow {
            for (&output, values) in outputs.iter().zip(&produced) {
                self.write(output, values);
            }
        }
        Step::Next(pc + 1)
    }

    fn operation(&mut self, pc: usize, op: O, arguments: &[A], flow: &s::Flow) -> Result<Vec<Vec<Word>>, Step> {
        let operands = locations(arguments);
        let produced = match op {
            O::FrameResume => self.frame_resume(argument(arguments, |argument| match argument {
                A::Continuation(k) => Some(*k),
                _ => None,
            })),
            O::FrameSave | O::DepsOf | O::DepsEmpty => vec![vec![]],
            O::ScopeBegin => self.scope_begin(argument(arguments, |argument| match argument {
                A::Scope(id) => Some(*id),
                _ => None,
            })),
            O::OwnAccept => {
                let mut state = ScopeSlot { record: self.word(operands[0]) };
                let mut input = PacketSlot { record: self.word(operands[1]) };
                let mut slot = ScopeSlot::default();
                assert_eq!(self.call(|ctx| rt_own_accept(ctx, &mut state, &mut input, &mut slot)), NORMAL);
                self.write(operands[0], &[state.record]);
                self.write(operands[1], &[input.record]);
                vec![vec![slot.record]]
            }
            O::OwnEmpty => {
                let mut slot = PacketSlot::default();
                assert_eq!(self.call(|ctx| rt_own_empty(ctx, &mut slot)), NORMAL);
                vec![vec![slot.record]]
            }
            O::OwnAdmit => {
                let mut state = ScopeSlot { record: self.word(operands[0]) };
                let mut input = PacketSlot { record: self.word(operands[1]) };
                let mut slot = ScopeSlot::default();
                assert_eq!(self.call(|ctx| rt_own_admit(ctx, &mut state, &mut input, &mut slot)), NORMAL);
                self.write(operands[0], &[state.record]);
                self.write(operands[1], &[input.record]);
                vec![vec![slot.record]]
            }
            O::OwnCreateCommit => {
                let descriptor = argument(arguments, |argument| match argument {
                    A::Object(id) => Some(u64::from(*id)),
                    _ => None,
                });
                let boundary = argument(arguments, |argument| match argument {
                    A::Boundary(id) => Some(*id),
                    _ => None,
                });
                let scope = self.program.tables.boundaries[boundary as usize].scope;
                let owner = self.scopes[scope as usize][1];
                let object = self.word(operands[0]);
                let mut slot = PacketSlot::default();
                let committed = self.call(|ctx| rt_own_create_commit(ctx, object, descriptor, owner, &mut slot));
                assert_eq!(committed, NORMAL);
                vec![vec![slot.record]]
            }
            O::CleanupNext => {
                let mut input = CleanupSlot { record: self.word(operands[0]) };
                let mut slot = NextPost::default();
                let ordinal = self.call(|ctx| rt_cleanup_next(ctx, &mut input, &mut slot));
                self.write(operands[0], &[0]);
                return Err(match ordinal {
                    NORMAL => self.outcome(flow, Tag::Selected,
                        &[vec![slot.state.record], vec![slot.key.object(), slot.key.serial()]]),
                    1 => self.outcome(flow, Tag::Done, &[vec![slot.state.record]]),
                    _ => Step::Stop,
                });
            }
            O::CleanupDestroy => {
                let mut input = CleanupSlot { record: self.word(operands[0]) };
                let key = self.read(operands[1]);
                let key = KeyRef { object: key[0], serial: key[1] };
                let mut slot = DestroyPost::default();
                if self.call(|ctx| rt_cleanup_destroy(ctx, &mut input, key, &mut slot)) != NORMAL {
                    return Err(Step::Stop);
                }
                self.write(operands[0], &[0]);
                vec![vec![slot.state.record], vec![slot.failure]]
            }
            O::FailureDispatch => {
                let failure = self.word(operands[0]);
                return Err(if failure == 0 {
                    self.outcome(flow, Tag::None, &[])
                } else {
                    self.outcome(flow, Tag::Some, &[vec![failure]])
                });
            }
            O::CleanupFailure => {
                let mut input = CleanupSlot { record: self.word(operands[0]) };
                let failure = self.word(operands[1]);
                let mut slot = CleanupSlot::default();
                assert_eq!(self.call(|ctx| rt_cleanup_failure(ctx, &mut input, failure, &mut slot)), NORMAL);
                self.write(operands[0], &[0]);
                vec![vec![slot.record]]
            }
            O::FutureNew => self.future_new(arguments, &operands).ok_or(Step::Stop)?,
            O::Await => return Err(self.await_step(pc, operands[0], flow)),
            O::AwaitTake => return Err(self.take_step(pc, arguments, operands[0], flow)),
            O::UniqueCacheProof => vec![vec![]],
            O::CacheRehome => {
                let destination = argument(arguments, |argument| match argument {
                    A::Destination(destination) => Some(*destination),
                    _ => None,
                });
                let target = match destination {
                    s::ExitDestination::Caller => TARGET_CALLER,
                    s::ExitDestination::Loop(id) => self.scopes[self.program.tables.boundaries[id as usize].scope
                        as usize][1],
                    s::ExitDestination::Scope(id) => self.scopes[id as usize][1],
                };
                let mut pending = PendingSlot { record: self.word(operands[0]) };
                let future = self.word(operands[1]);
                let mut cache = CacheSlot { record: future };
                let mut slot = RehomePost::default();
                let rehomed = self.call(|ctx| rt_cache_rehome(ctx, &mut pending, std::ptr::null_mut(), &mut cache,
                    std::ptr::null_mut(), future, target, &mut slot));
                assert_eq!(rehomed, NORMAL, "{:?}", runtime_of(self.ctx).fatal);
                self.write(operands[0], &[0]);
                self.write(operands[1], &[0]);
                vec![vec![slot.pending.record]]
            }
            O::AwaitUnwind => self.await_unwind(pc, &operands)?,
            O::ExitPrepare => self.exit_prepare(arguments, &operands),
            O::CleanupBegin => self.cleanup_begin(&operands),
            O::CleanupSettle => self.cleanup_settle(arguments, &operands),
            O::CleanupPlan => {
                let local = argument(arguments, |argument| match argument {
                    A::Plan(id) => Some(*id),
                    _ => None,
                });
                let plan = runtime_of(self.ctx).champ.machine.as_ref().unwrap().plans[self.index][local as usize];
                let mut input = CleanupSlot { record: self.word(operands[0]) };
                let mut slot = CleanupSlot::default();
                assert_eq!(self.call(|ctx| rt_cleanup_plan(ctx, &mut input, plan, &mut slot)), NORMAL);
                self.write(operands[0], &[0]);
                vec![vec![slot.record]]
            }
            O::CleanupFinish => {
                let mut input = CleanupSlot { record: self.word(operands[0]) };
                let mut slot = FinishPost::default();
                assert_eq!(self.call(|ctx| rt_cleanup_finish(ctx, &mut input, &mut slot)), NORMAL);
                self.write(operands[0], &[0]);
                vec![vec![], vec![slot.failure], vec![slot.pending.record]]
            }
            O::CleanupDeliverCancel => {
                let mut input = CleanupSlot { record: self.word(operands[0]) };
                let mut slot = CleanupSlot::default();
                let frame = self.frame;
                let delivered = self.call(|ctx| rt_cleanup_deliver_cancel(ctx, frame, &mut input, &mut slot));
                assert_eq!(delivered, NORMAL, "{:?}", runtime_of(self.ctx).fatal);
                self.write(operands[0], &[0]);
                vec![vec![slot.record]]
            }
            O::ExitDispatch => return Err(self.exit_dispatch(arguments, &operands, flow)),
            O::ExitCommit => self.exit_commit(arguments, &operands, flow),
            O::FuturePublish => self.publish(arguments, &operands),
            O::FutureDone => {
                let state = get(self.word(operands[0]), tasks::STATE);
                vec![vec![u64::from(state == tasks::DONE || state == tasks::FAILURE_STATE)]]
            }
            O::FutureCancel => {
                let future = self.word(operands[0]);
                assert_eq!(self.call(|ctx| rt_cancel_request(ctx, future)), NORMAL);
                vec![]
            }
            O::FutureCancelled => {
                let future = self.word(operands[0]);
                let mut slot = FlagPost::default();
                assert_eq!(self.call(|ctx| rt_future_cancelled(ctx, future, &mut slot)), NORMAL);
                vec![vec![slot.value]]
            }
            O::Fail => return Err(self.fail(pc, arguments, operands[0], flow)),
            other => panic!("reference body: {} is not used by these programs", other.name()),
        };
        Ok(produced)
    }

    /// Continuation 0 is read from the frame and cleared; any other continuation's values are where its
    /// `frame.save` found them, or where `reload` put them back.
    fn frame_resume(&mut self, k: u32) -> Vec<Vec<Word>> {
        let mut values = vec![vec![]];
        if k != 0 {
            let (saved, _) = self.bracket(k);
            values.extend(saved.iter().map(|&location| self.read(location)));
            return values;
        }
        for slot in self.map.start.clone() {
            let slot = self.map.slots[slot as usize].clone();
            let words = 0..slot.reprs.len() as u64;
            values.push(words.clone().map(|word| get(self.frame, slot.offset + 8 * word)).collect());
            for word in words {
                set(self.frame, slot.offset + 8 * word, 0);
            }
        }
        values
    }

    fn scope_begin(&mut self, scope: u32) -> Vec<Vec<Word>> {
        let parent = self.program.tables.scopes[scope as usize].parent;
        let parent_owner = parent.map_or(0, |parent| self.scopes[parent as usize][1]);
        let index = self.index as u64;
        let mut slot = ScopeSlot::default();
        let begun = self.call(|ctx| machine::rt_machine_scope(ctx, index, u64::from(scope), parent_owner, &mut slot));
        assert_eq!(begun, NORMAL);
        let mut destination = ExitDestination { holder: HolderId(0), owner: OwnerId(0) };
        let found = self.call(|ctx| machine::rt_machine_destination(ctx, slot.record, &mut destination));
        assert_eq!(found, NORMAL);
        self.scopes[scope as usize] = [destination.holder.get(), destination.owner.get()];
        vec![vec![slot.record]]
    }

    /// The continuation whose site is `pc`.
    fn site(&self, pc: usize) -> u32 {
        self.layout().continuations.iter().position(|continuation| continuation.site == Some(pc as u32)).unwrap()
            as u32
    }

    fn await_step(&mut self, pc: usize, operand: Location, flow: &s::Flow) -> Step {
        let future = self.word(operand);
        let frame = self.frame;
        let mut slot = AwaitPost::default();
        let outcome = self.call(|ctx| rt_await_view(ctx, frame, future, &mut slot));
        let Location::Register(id) = operand else { unreachable!("verified Future operand") };
        let s::Type::Future(result) = self.function().registers[id as usize] else { unreachable!("Future type") };
        let valued = self.program.tables.future_results[result as usize].is_some();
        match outcome {
            OUTCOME_PENDING => self.suspend(self.site(pc)),
            NORMAL => {
                let mut values = if valued { vec![vec![slot.value]] } else { vec![] };
                values.push(vec![]);
                self.outcome(flow, Tag::Normal, &values)
            }
            UNWIND => {
                self.land(pc);
                self.outcome(flow, Tag::Unwind, &[vec![slot.value], vec![slot.task], vec![slot.receipt]])
            }
            _ => Step::Stop,
        }
    }

    /// `await.take`: the Future, its zero-word capability and the boundary scope, whose owner takes the packet.
    fn take_step(&mut self, pc: usize, arguments: &[A], operand: Location, flow: &s::Flow) -> Step {
        let future = self.word(operand);
        let scope = argument(arguments, |argument| match argument {
            A::Scope(id) => Some(*id),
            _ => None,
        });
        let boundary = self.scopes[scope as usize][1];
        let frame = self.frame;
        let mut slot = AwaitTakePost::default();
        let outcome = self.call(|ctx| rt_await_take(ctx, frame, future, boundary, &mut slot));
        let Location::Register(id) = operand else { unreachable!("verified Future operand") };
        let s::Type::Future(result) = self.function().registers[id as usize] else { unreachable!("Future type") };
        let valued = self.program.tables.future_results[result as usize].is_some();
        match outcome {
            OUTCOME_PENDING => self.suspend(self.site(pc)),
            NORMAL => {
                let mut values = if valued { vec![vec![slot.value]] } else { vec![] };
                values.extend([vec![slot.packet.record], vec![]]);
                self.outcome(flow, Tag::Normal, &values)
            }
            UNWIND => {
                self.land(pc);
                self.outcome(flow, Tag::Unwind, &[vec![slot.value], vec![slot.task], vec![slot.receipt]])
            }
            _ => Step::Stop,
        }
    }

    fn await_unwind(&mut self, pc: usize, operands: &[Location]) -> Result<Vec<Vec<Word>>, Step> {
        let mut state = ScopeSlot { record: self.word(operands[0]) };
        let failure = self.word(operands[1]);
        let mut task = self.word(operands[2]);
        let mut receipt = self.word(operands[3]);
        let frame = self.frame;
        let mut slot = AwaitUnwindPost::default();
        let outcome = self.call(|ctx| {
            rt_await_unwind(ctx, frame, &mut state, failure, &mut task, &mut receipt, &mut slot)
        });
        match outcome {
            NORMAL => {}
            OUTCOME_PENDING => return Err(self.suspend(self.site(pc))),
            _ => return Err(Step::Stop),
        }
        self.write(operands[0], &[state.record]);
        self.write(operands[2], &[task]);
        self.write(operands[3], &[receipt]);
        Ok(vec![vec![slot.state.record], vec![slot.failure]])
    }

    fn exit_prepare(&mut self, arguments: &[A], operands: &[Location]) -> Vec<Vec<Word>> {
        let kind = argument(arguments, |argument| match argument {
            A::Kind(kind) => Some(*kind as u64),
            _ => None,
        });
        let boundary = argument(arguments, |argument| match argument {
            A::Boundary(id) => Some(u64::from(*id)),
            _ => None,
        });
        let (value, value_repr) = match operands[..operands.len() - 1] {
            [Location::Register(id)] => {
                (self.registers[id as usize], machine::repr(self.function().registers[id as usize]) as u64)
            }
            _ => (0, 0),
        };
        let packet = *operands.last().unwrap();
        let mut input = PacketSlot { record: self.word(packet) };
        let inputs = ExitInputs { kind, boundary, value, value_repr, packet: &mut input, selection: 0 };
        let mut slot = PendingSlot::default();
        assert_eq!(self.call(|ctx| rt_exit_prepare(ctx, &inputs, &mut slot)), NORMAL);
        self.write(packet, &[input.record]);
        vec![vec![slot.record]]
    }

    fn cleanup_begin(&mut self, operands: &[Location]) -> Vec<Vec<Word>> {
        let scope = matches!(operands[0], Location::Service(id)
            if self.function().services[id as usize] == K::ScopeState);
        let mut source = ScopeSlot { record: self.word(operands[0]) };
        let mut packet = PacketSlot { record: self.word(operands[0]) };
        let mut pending = PendingSlot::default();
        let mut pending_location = None;
        let mut failure = 0;
        for &location in &operands[1..] {
            match location {
                Location::Service(_) => {
                    pending.record = self.word(location);
                    pending_location = Some(location);
                }
                Location::Register(_) => failure = self.word(location),
            }
        }
        let inputs = CloseInputs {
            scope: if scope { &mut source } else { std::ptr::null_mut() },
            packet: if scope { std::ptr::null_mut() } else { &mut packet },
            pending: if pending_location.is_some() { &mut pending } else { std::ptr::null_mut() },
            failure,
            settlements: 0,
        };
        let mut slot = CleanupSlot::default();
        assert_eq!(self.call(|ctx| rt_cleanup_begin(ctx, &inputs, &mut slot)), NORMAL);
        self.write(operands[0], &[0]);
        if let Some(location) = pending_location {
            self.write(location, &[0]);
        }
        vec![vec![slot.record]]
    }

    /// The empty settlement, or one scheduled duty at its position in the close's schedule.
    fn cleanup_settle(&mut self, arguments: &[A], operands: &[Location]) -> Vec<Vec<Word>> {
        let mut input = CleanupSlot { record: self.word(operands[0]) };
        let mut slot = CleanupSlot::default();
        if operands.len() == 1 {
            assert_eq!(self.call(|ctx| rt_cleanup_settle_empty(ctx, &mut input, &mut slot)), NORMAL);
            self.write(operands[0], &[0]);
            return vec![vec![slot.record]];
        }
        let local = argument(arguments, |argument| match argument {
            A::Schedule(id) => Some(*id),
            _ => None,
        });
        let duty = self.word(operands[1]);
        let duties = &self.function().proofs.schedules[local as usize].services;
        let position = duties.iter().position(|&location| location == operands[1]).unwrap() as u64;
        let schedule = runtime_of(self.ctx).champ_schedule_id(self.index, local).unwrap();
        let frame = self.frame;
        let settled = self.call(|ctx| rt_service_settle(ctx, frame, &mut input, schedule, position, duty, &mut slot));
        assert_eq!(settled, NORMAL, "{:?}", runtime_of(self.ctx).fatal);
        self.write(operands[0], &[0]);
        self.write(operands[1], &[0]);
        vec![vec![slot.record]]
    }

    fn exit_dispatch(&mut self, arguments: &[A], operands: &[Location], flow: &s::Flow) -> Step {
        let failure = self.word(operands[1]);
        let pending = argument(arguments, |argument| match argument {
            A::Flag(flag) => Some(*flag),
            _ => None,
        });
        if failure != 0 {
            self.outcome(flow, Tag::Failure, &[vec![], vec![failure]])
        } else if pending {
            let exit = self.read(operands[2]);
            self.outcome(flow, Tag::Pending, &[vec![], exit])
        } else {
            self.outcome(flow, Tag::Success, &[vec![]])
        }
    }

    /// `exit.commit`: into a scope of the body (its scope area words), or `caller`, which in an async body is the
    /// Future's cache (ABI C-2).
    fn exit_commit(&mut self, arguments: &[A], operands: &[Location], flow: &s::Flow) -> Vec<Vec<Word>> {
        let target = argument(arguments, |argument| match argument {
            A::Destination(destination) => Some(*destination),
            _ => None,
        });
        let mut input = PendingSlot { record: self.word(operands[0]) };
        let mut destination = ExitDestination { holder: HolderId(0), owner: OwnerId(0) };
        let frame = self.frame;
        match target {
            s::ExitDestination::Caller => {
                assert_eq!(self.call(|ctx| rt_machine_future_destination(ctx, frame, &mut destination)), NORMAL);
            }
            s::ExitDestination::Scope(scope) => {
                let [holder, owner] = self.scopes[scope as usize];
                destination = ExitDestination { holder: HolderId(holder), owner: OwnerId(owner) };
            }
            s::ExitDestination::Loop(_) => panic!("reference body: no loop exits in these programs"),
        }
        let mut slot = CommitPost::default();
        assert_eq!(self.call(|ctx| rt_exit_commit(ctx, &mut input, &destination, &mut slot)), NORMAL);
        self.write(operands[0], &[0]);
        let s::Flow::Next(outputs) = flow else { unreachable!("verified commit") };
        if outputs.len() == 3 {
            vec![vec![slot.value], vec![slot.packet.record], vec![]]
        } else {
            vec![vec![slot.packet.record], vec![]]
        }
    }

    fn publish(&mut self, arguments: &[A], operands: &[Location]) -> Vec<Vec<Word>> {
        let success = arguments.contains(&A::Flag(true));
        let value = operands[1..].iter().find(|location| matches!(location, Location::Register(_)))
            .map_or(0, |&location| self.word(location));
        let packet_location = operands[1..].iter().copied().find(|location| matches!(location,
            Location::Service(id) if self.function().services[*id as usize] == K::Packet));
        let mut packet = PacketSlot { record: packet_location.map_or(0, |location| self.word(location)) };
        let inputs = PublishIn {
            tag: u64::from(!success),
            value: if success { value } else { 0 },
            packet: &mut packet,
            failure: if success { 0 } else { value },
        };
        let frame = self.frame;
        assert_eq!(self.call(|ctx| rt_future_publish(ctx, frame, &inputs)), NORMAL);
        if let Some(location) = packet_location {
            self.write(location, &[packet.record]);
        }
        vec![]
    }

    fn fail(&mut self, pc: usize, arguments: &[A], payload: Location, flow: &s::Flow) -> Step {
        let kind = argument(arguments, |argument| match argument {
            A::Label(kind) => Some(u64::from(*kind)),
            _ => None,
        });
        let payload = self.word(payload);
        let failure = self.call(|ctx| machine::rt_machine_fail(ctx, kind, payload));
        unsafe {
            (*self.ctx).failure = failure;
            (*self.ctx).status = FAILED;
        }
        self.land(pc);
        self.outcome(flow, Tag::Unwind, &[vec![failure]])
    }

    fn future_new(&mut self, arguments: &[A], operands: &[Location]) -> Option<Vec<Vec<Word>>> {
        let entry = argument(arguments, |argument| match argument {
            A::Entry(id) => Some(u64::from(*id)),
            _ => None,
        });
        let args = operands.iter().filter(|location| matches!(location, Location::Register(_)))
            .map(|&location| self.word(location)).collect::<Vec<_>>();
        let kind = |location: &Location| match location {
            Location::Service(id) => Some(self.function().services[*id as usize]),
            Location::Register(_) => None,
        };
        let packet = *operands.iter().find(|location| kind(location) == Some(K::Packet)).unwrap();
        let state = *operands.iter().find(|location| kind(location) == Some(K::ScopeState)).unwrap();
        let mut inputs = PacketSlot { record: self.word(packet) };
        let mut owner = ScopeSlot { record: self.word(state) };
        let io = FutureNewIn {
            entry, args: args.as_ptr(), argc: args.len() as u64, inputs: &mut inputs, owner: &mut owner,
        };
        let mut slot = FutureNewPost::default();
        if self.call(|ctx| rt_future_new_owned(ctx, &io, &mut slot)) != NORMAL {
            return None;
        }
        self.write(packet, &[inputs.record]);
        self.write(state, &[owner.record]);
        Some(vec![vec![slot.future], vec![slot.cache], vec![slot.state.record]])
    }
}

/// The one argument `pick` recognizes.
fn argument<T>(arguments: &[A], pick: impl Fn(&A) -> Option<T>) -> T {
    arguments.iter().find_map(pick).expect("verified argument")
}

// --- Harness ----------------------------------------------------------------------------------------------------

fn context(runtime: &mut Runtime) -> Ctx {
    Ctx { status: OK, stack_limit: 0, failure: 0, runtime, entry_frame: 0, scratch: 0 }
}

fn steps(runtime: &Runtime, function: u32) -> Vec<FutureStep> {
    runtime.champ.future_events.iter().filter(|event| event.function == function as usize)
        .map(|event| event.step).collect()
}

fn count(runtime: &Runtime, function: u32, step: FutureStep) -> usize {
    steps(runtime, function).into_iter().filter(|&found| found == step).count()
}

/// A scope state with fresh ids, rooted for the rest of the test.
fn owner(ctx: &mut Ctx) -> ScopeSlot {
    let runtime = unsafe { &mut *ctx.runtime };
    let holder = runtime.champ_new_holder().unwrap();
    let owner = runtime.champ_new_owner().unwrap();
    let instance = ScopeInstance { holder, owner, parent_owner: 0 };
    let mut slot = ScopeSlot::default();
    assert_eq!(rt_scope_begin(ctx, &instance, &mut slot), NORMAL);
    runtime.roots.push((slot.record, Repr::Ref));
    slot
}

/// `fut.new` from the test, which keeps its operands and the new Future rooted, as a caller's stack map does.
fn new_future(ctx: &mut Ctx, entry: u32, args: &[Word], state: &mut ScopeSlot, inputs: PacketSlot) -> Word {
    let runtime = unsafe { &mut *ctx.runtime };
    let mut inputs = inputs;
    let base = runtime.roots.len();
    runtime.roots.push((inputs.record, Repr::Ref));
    let io = FutureNewIn { entry: entry as u64, args: args.as_ptr(), argc: args.len() as u64, inputs: &mut inputs,
        owner: state };
    let mut post = FutureNewPost::default();
    assert_eq!(rt_future_new_owned(ctx, &io, &mut post), NORMAL, "{:?}", runtime.fatal);
    runtime.roots.truncate(base);
    assert_eq!(inputs.record, 0);
    assert_eq!(post.cache, post.future);
    *state = post.state;
    runtime.roots.push((post.future, Repr::Ref));
    post.future
}

fn empty(ctx: &mut Ctx) -> PacketSlot {
    let mut slot = PacketSlot::default();
    assert_eq!(rt_own_empty(ctx, &mut slot), NORMAL);
    slot
}

/// A Res committed to `owner`, in a packet of its own.
fn resource(ctx: &mut Ctx, owner: OwnerId) -> (Word, PacketSlot) {
    let runtime = unsafe { &mut *ctx.runtime };
    let object = runtime.heap.object_new(0, &[]);
    runtime.roots.push((object, Repr::Ref));
    let mut slot = PacketSlot::default();
    assert_eq!(rt_own_create_commit(ctx, object, 0, owner.get(), &mut slot), NORMAL);
    (object, slot)
}

/// A Future of `work` started elsewhere and suspended there: the test plays its body.
fn gate(ctx: &mut Ctx, state: &mut ScopeSlot) -> Word {
    gate_of(ctx, state)
}

fn gate_of(ctx: &mut Ctx, state: &mut ScopeSlot) -> Word {
    let inputs = empty(ctx);
    let gate = new_future(ctx, WORK, &[word::small(1).unwrap()], state, inputs);
    let runtime = unsafe { &mut *ctx.runtime };
    let tail = runtime.champ_frame_map(WORK as usize).unwrap().tail;
    set(gate, tasks::STATE, tasks::RUNNING);
    set(gate, tail + TASK, gate);
    gate
}

fn tail(runtime: &Runtime, frame: Word) -> u64 {
    runtime.champ_frame_map(runtime.function_of(frame)).unwrap().tail
}

/// Every slot and scope word of `frame` is zero (the clearing rule, draft 3 §2.3).
fn cleared(runtime: &Runtime, frame: Word) -> bool {
    (AREA..tail(runtime, frame)).step_by(8).all(|offset| get(frame, offset) == 0)
}

fn kind(runtime: &Runtime, failure: Word) -> String {
    format!("{:?}", runtime.failure(failure).unwrap().kind)
}

/// Close `state` settling `duties` in schedule order with a merge plan; the close's failure.
fn close(ctx: &mut Ctx, state: &mut ScopeSlot, duties: &[Word]) -> Word {
    let runtime = unsafe { &mut *ctx.runtime };
    let schedule = runtime.champ_register_schedule(duties.len() as u64);
    let plan = runtime.champ_register_cleanup_plan(CleanupPlan { reserve: false, direct: false, may_fail: false });
    let inputs = CloseInputs {
        scope: state, packet: std::ptr::null_mut(), pending: std::ptr::null_mut(), failure: 0, settlements: 0,
    };
    let mut cleanup = CleanupSlot::default();
    assert_eq!(rt_cleanup_begin(ctx, &inputs, &mut cleanup), NORMAL);
    let base = runtime.roots.len();
    runtime.roots.push((cleanup.record, Repr::Ref));
    for (index, &duty) in duties.iter().enumerate() {
        let outcome = rt_service_settle(ctx, 0, &mut cleanup, schedule, index as u64, duty, &mut cleanup);
        assert_eq!(outcome, NORMAL, "{:?}", runtime.fatal);
    }
    assert_eq!(rt_cleanup_plan(ctx, &mut cleanup, plan, &mut cleanup), NORMAL);
    loop {
        let mut next = NextPost::default();
        let ordinal = rt_cleanup_next(ctx, &mut cleanup, &mut next);
        cleanup = next.state;
        if ordinal == 1 {
            break;
        }
        assert_eq!(ordinal, NORMAL);
        let mut destroyed = DestroyPost::default();
        assert_eq!(rt_cleanup_destroy(ctx, &mut cleanup, next.key, &mut destroyed), NORMAL, "{:?}", runtime.fatal);
        assert_eq!(destroyed.failure, 0);
        cleanup = destroyed.state;
    }
    let mut finished = FinishPost::default();
    assert_eq!(rt_cleanup_finish(ctx, &mut cleanup, &mut finished), NORMAL);
    runtime.roots.truncate(base);
    finished.failure
}

// --- Tests ------------------------------------------------------------------------------------------------------

#[test]
fn champ_v8_frame_map_follows_the_layout_service_words_and_scopes() {
    let mut p = program();
    // A layout with every word shape: a value, a two-word key, a five-word borrow, a raw receipt, the packet.
    p.tables.frames[WORK as usize].slots = vec![value(s::Type::Int), service(K::StableKey),
        service(K::DestroyBorrow), service(K::Receipt), service(K::Packet)];
    p.tables.frames[WORK as usize].continuations[0].saved = vec![0, 4];
    let map = FrameMap::new(&p, WORK as usize).unwrap();
    let offsets = map.slots.iter().map(|slot| slot.offset).collect::<Vec<_>>();
    assert_eq!(offsets, [72, 80, 96, 136, 144]);
    let (r, x) = (Repr::Ref, Repr::Raw);
    assert_eq!(map.slots[0].reprs, [Repr::Int]);
    assert_eq!(map.slots[1].reprs, [r, x]);
    assert_eq!(map.slots[2].reprs, [r, x, x, x, r]);
    assert_eq!(map.slots[3].reprs, [x]);
    assert_eq!(map.slots[4].reprs, [r]);
    assert_eq!(map.scopes, [(WORK, 152)]);
    assert_eq!(map.tail, 168);
    assert_eq!(map.words, 1 + 8 + 10 + 2 + 4);
    assert_eq!(map.start, [0, 4]);
    assert_eq!((map.inputs(), map.packet()), (1, Some(144)));
    let mut expected = vec![x, Repr::Int, r, r, r, x, x, r, Repr::Int, r, x, r, x, x, x, r, x, r, x, x];
    expected.extend([r; 4]);
    assert_eq!(map.trace, expected);
    // A void body's RESULT word is raw; functions without a layout have no map.
    assert_eq!(FrameMap::new(&p, CONSUME as usize).unwrap().trace[1], x);
    assert_eq!(FrameMap::new(&p, DESTROY as usize), None);
    // The loaded runtime traces each async body's frame by its map.
    let runtime = load(false, Engine::Jit);
    for function in 0..ASYNC_FUNCTIONS as usize {
        let map = runtime.champ_frame_map(function).unwrap();
        assert_eq!(runtime.heap.layouts().frames[function], map.trace);
    }
    assert!(runtime.champ_frame_map(DESTROY as usize).is_none());
    // Tomato's declarations name a Future's outcome type.
    let legacy = &runtime.loaded.program;
    let futures = legacy.functions[MAIN as usize].registers.iter().filter_map(|&ty| match legacy.types[ty.0 as usize] {
        okkama_sparrow::Type::Future(outcome) => Some(outcome.map(|outcome| legacy.types[outcome.0 as usize].clone())),
        _ => None,
    }).collect::<Vec<_>>();
    assert_eq!(futures, [Some(okkama_sparrow::Type::Int)]);
}

#[test]
fn champ_v8_future_new_allocates_first_and_accepts_inputs_into_continuation_zero() {
    for (stress, engine) in configurations() {
        let mut runtime = load(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let scope_owner = super::helpers::scope(&runtime, state.record).unwrap().owner;
        let (r, inputs) = resource(&mut ctx, scope_owner);
        let before = runtime.heap.champ_identity(r).unwrap();
        let record = state.record;
        let future = new_future(&mut ctx, CONSUME, &[r], &mut state, inputs);
        assert_eq!(state.record, record, "the owner state is threaded through");
        assert_eq!(crate::heap::kind(runtime.heap.header_of(future)), crate::heap::FRAME);
        assert_eq!(runtime.function_of(future), CONSUME as usize);
        assert_eq!(get(future, tasks::STATE), tasks::COLD);
        assert_eq!(get(future, tasks::FLAGS), CHAMP);
        let cache = runtime.champ_cache(future).unwrap();
        assert_eq!((cache.owner, cache.packet, cache.flags), (scope_owner, 0, 0));
        assert_ne!(cache.holder.get(), 0);
        let map = runtime.champ_frame_map(CONSUME as usize).unwrap().clone();
        assert_eq!(get(future, map.slots[0].offset), r);
        let accepted = PacketSlot { record: get(future, map.packet().unwrap()) };
        assert_eq!(runtime.champ_packet_keys(&accepted).unwrap().iter().map(|key| key.object()).collect::<Vec<_>>(),
            [r]);
        // The inputs holder T takes the runs; their owner stays (draft 3 §2.4).
        let after = runtime.heap.champ_identity(r).unwrap();
        assert_ne!(after.holder_id, before.holder_id);
        assert_ne!(after.holder_id, cache.holder.get());
        assert_eq!((after.owner_id, after.epoch, after.life), (before.owner_id, before.epoch + 1, Life::Live));
        let emergency = get(future, map.tail + EMERGENCY);
        assert_eq!(kind(&runtime, emergency), "Cancelled");
        assert_eq!((get(future, map.tail + INPUTS), get(future, map.tail + STEP), get(future, map.tail + TASK)),
            (0, 0, 0));
        // A refused creation accepts nothing: the inputs and the owner state stay with the caller.
        let (other, mut offered) = resource(&mut ctx, scope_owner);
        let kept = runtime.heap.champ_identity(other).unwrap();
        let io = FutureNewIn { entry: CONSUME as u64, args: [other, other].as_ptr(), argc: 2,
            inputs: &mut offered, owner: &mut state };
        let mut post = FutureNewPost::default();
        assert_eq!(rt_future_new_owned(&mut ctx, &io, &mut post), OUTCOME_STOP);
        assert!(runtime.fatal.as_deref().unwrap().contains("fut.new argument count"));
        assert_eq!(post.future, 0);
        assert_eq!(state.record, record);
        assert_eq!(runtime.champ_packet_keys(&offered).unwrap().len(), 1);
        assert_eq!(runtime.heap.champ_identity(other).unwrap(), kept);
        ctx.status = OK;
        runtime.fatal = None;
        // A synchronous function is not a Future body.
        let io = FutureNewIn { entry: DESTROY as u64, args: [other].as_ptr(), argc: 1, inputs: &mut offered,
            owner: &mut state };
        assert_eq!(rt_future_new_owned(&mut ctx, &io, &mut post), OUTCOME_STOP);
        assert_eq!(runtime.champ_packet_keys(&offered).unwrap().len(), 1);
        ctx.status = OK;
        runtime.fatal = None;
        // Both resources close with their owner: r through the Future's cache duty, other directly.
        let mut joined = PacketSlot::default();
        let mut parts = [offered];
        assert_eq!(rt_own_join(&mut ctx, parts.as_mut_ptr(), 1, &mut joined), NORMAL);
        let mut admitted = ScopeSlot::default();
        assert_eq!(rt_own_admit(&mut ctx, &mut state, &mut joined, &mut admitted), NORMAL);
        state = admitted;
        runtime.roots.push((state.record, Repr::Ref));
        assert_eq!(close(&mut ctx, &mut state, &[future]), 0);
        for object in [r, other] {
            assert_eq!(runtime.heap.champ_identity(object).unwrap().life, Life::Dead);
        }
    }
}

#[test]
fn champ_v8_body_runs_once() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load(stress, engine);
        let result = runtime.run_function(MAIN as usize, crate::STACK_BYTES);
        assert_eq!(result.unwrap(), word::small(7).unwrap());
        assert_eq!(steps(&runtime, WORK), [FutureStep::Start, FutureStep::Published]);
        assert_eq!(steps(&runtime, MAIN), [FutureStep::Start, FutureStep::Published]);
        assert_eq!(stress, runtime.heap.stats().collections > 0);
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_v8_repeated_await_sees_one_outcome() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load(stress, engine);
        let result = runtime.run_function(TWICE as usize, crate::STACK_BYTES);
        assert_eq!(result.unwrap(), word::small(14).unwrap());
        // The second await reads the cache: the body ran once and settled once.
        assert_eq!(steps(&runtime, WORK), [FutureStep::Start, FutureStep::Published]);
        assert_eq!(steps(&runtime, TWICE), [FutureStep::Start, FutureStep::Published]);
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_v8_cached_failure_is_seen_by_two_waiters() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        // F awaits a task suspended elsewhere, then fails; W1 and W2 await F, in this order.
        let gate = gate(&mut ctx, &mut state);
        let inputs = empty(&mut ctx);
        let f = new_future(&mut ctx, CHAIN, &[gate], &mut state, inputs);
        let mut watchers = Vec::new();
        for _ in 0..2 {
            let inputs = empty(&mut ctx);
            watchers.push(new_future(&mut ctx, WATCH, &[f], &mut state, inputs));
        }
        let [w1, w2] = watchers[..] else { unreachable!() };
        for waiter in [w1, w2] {
            runtime.champ_future_start(&mut ctx, 0, waiter);
            assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
            assert_eq!(get(waiter, tasks::STATE), tasks::RUNNING);
            assert_eq!(get(waiter, tasks::AWAITING), f);
        }
        assert_eq!(get(f, tasks::STATE), tasks::RUNNING);
        assert_eq!(get(f, tasks::AWAITING), gate);
        // F started on W1's stack and belongs to W1's task; each waiter is its own task.
        assert_eq!(runtime.champ_task_of(f).unwrap(), w1);
        assert_eq!(runtime.champ_task_of(w2).unwrap(), w2);
        // Suspended frames hold exactly the saved slots of their continuation: W1 keeps its owner state.
        let saved = runtime.champ_frame_map(WATCH as usize).unwrap().slots[2].offset;
        assert!(super::helpers::scope(&runtime, get(w1, saved)).is_ok());
        let mark = runtime.champ.future_events.len();
        // G's body never read its continuation 0; publication clears the whole area.
        assert!(!cleared(&runtime, gate));
        let mut outcome = empty(&mut ctx);
        let published = PublishIn { tag: 0, value: word::small(5).unwrap(), packet: &mut outcome, failure: 0 };
        assert_eq!(rt_future_publish(&mut ctx, gate, &published), NORMAL);
        assert!(cleared(&runtime, gate));
        assert_eq!((outcome.record, get(gate, tasks::RESULT)), (0, word::small(5).unwrap()));
        runtime.schedule(&mut ctx, w2);
        assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
        let trace = runtime.champ.future_events[mark..].iter()
            .map(|event| (event.step, event.frame)).collect::<Vec<_>>();
        use FutureStep::*;
        assert_eq!(trace, [(Published, gate), (Queued, f), (Resume, f), (Published, f), (Queued, w1), (Queued, w2),
            (Resume, w1), (Published, w1), (Resume, w2), (Published, w2)]);
        // Draft 2 §7.2 with a plain outcome: one body entry and one terminal transition of F, one wake of each
        // waiter, and both observe the same carrier.
        assert_eq!((count(&runtime, CHAIN, Start), count(&runtime, CHAIN, Published)), (1, 1));
        let failure = get(f, tasks::FAILURE);
        assert!(kind(&runtime, failure).contains("chain"), "{}", kind(&runtime, failure));
        for frame in [f, w1, w2] {
            assert_eq!(get(frame, tasks::STATE), tasks::FAILURE_STATE);
            assert_eq!(get(frame, tasks::FAILURE), failure);
            assert!(cleared(&runtime, frame));
        }
        // A repeated await sees the same outcome without entering F's body.
        let inputs = empty(&mut ctx);
        let w3 = new_future(&mut ctx, WATCH, &[f], &mut state, inputs);
        runtime.champ_future_start(&mut ctx, 0, w3);
        assert_eq!(get(w3, tasks::FAILURE), failure);
        assert_eq!(count(&runtime, CHAIN, Start), 1);
        // Every cache duty settles at the owner's close; nothing remains.
        assert_eq!(close(&mut ctx, &mut state, &[gate, f, w1, w2, w3]), 0);
        for frame in [gate, f, w1, w2, w3] {
            assert_eq!(runtime.champ_cache(frame).unwrap().flags, RELEASED);
        }
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_v8_failure_reaches_the_root_through_every_await() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load(stress, engine);
        let report = runtime.run_function(MAIN_FAILS as usize, crate::STACK_BYTES).unwrap_err().0;
        assert!(report.starts_with("RuntimeError: boom"), "{report}");
        assert_eq!(steps(&runtime, FAILS), [FutureStep::Start, FutureStep::Published]);
        assert_eq!(steps(&runtime, MAIN_FAILS), [FutureStep::Start, FutureStep::Published]);
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_v8_never_awaited_future_never_runs_and_settles_at_owner_close() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        // Two cold Futures, one cancelled: neither body runs, and the close settles both duties.
        let mut runtime = load(stress, engine);
        assert_eq!(runtime.run_function(LAZY as usize, crate::STACK_BYTES).unwrap(), 1);
        assert!(steps(&runtime, WORK).is_empty());
        assert_eq!(steps(&runtime, LAZY), [FutureStep::Start, FutureStep::Published]);
        traces.record(configuration, &runtime);
        // With resource inputs: a cold Future and a Future cancelled before start hold their inputs until the
        // owner closes; the close releases them into its plan and settles the Futures as Cancelled.
        let mut runtime = load(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let scope_owner = super::helpers::scope(&runtime, state.record).unwrap().owner;
        let (r, inputs) = resource(&mut ctx, scope_owner);
        let cold = new_future(&mut ctx, CONSUME, &[r], &mut state, inputs);
        let (q, inputs) = resource(&mut ctx, scope_owner);
        let cancelled = new_future(&mut ctx, CONSUME, &[q], &mut state, inputs);
        let emergency = get(cancelled, tail(&runtime, cancelled) + EMERGENCY);
        assert_eq!(rt_cancel_request(&mut ctx, cancelled), NORMAL);
        assert_eq!((get(cancelled, tasks::STATE), get(cancelled, tasks::FAILURE)), (tasks::FAILURE_STATE, emergency));
        assert_eq!(runtime.heap.champ_identity(q).unwrap().life, Life::Live);
        assert!(!cleared(&runtime, cancelled), "cancel never moves a packet");
        assert!(runtime.champ.destroy_events.is_empty());
        let serials = [r, q].map(|object| runtime.heap.champ_identity(object).unwrap().serial);
        assert_eq!(close(&mut ctx, &mut state, &[cold, cancelled]), 0);
        assert_eq!(runtime.champ.destroy_events, [serials[1], serials[0]]);
        assert!(steps(&runtime, CONSUME).is_empty());
        for frame in [cold, cancelled] {
            assert_eq!(get(frame, tasks::STATE), tasks::FAILURE_STATE);
            assert_eq!(kind(&runtime, get(frame, tasks::FAILURE)), "Cancelled");
            assert_ne!(get(frame, tasks::FLAGS) & tasks::champ_flags::OBSERVED, 0);
            assert_eq!(runtime.champ_cache(frame).unwrap().flags, RELEASED);
            assert!(cleared(&runtime, frame));
        }
        let mut flag = FlagPost::default();
        assert_eq!(rt_future_cancelled(&mut ctx, cold, &mut flag), NORMAL);
        assert_eq!(flag.value, 1);
    }
    traces.agree();
}

#[test]
fn champ_v8_cache_settle_follows_its_rows_and_refuses_a_second_settle() {
    for engine in [Engine::Jit, Engine::Reference] {
        cache_settle_rows(engine);
    }
}

fn cache_settle_rows(engine: Engine) {
    let mut runtime = load(true, engine);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let mut state = owner(&mut ctx);
    let mut other = owner(&mut ctx);
    // A running Future detaches to the root: not cancelled, its waiters keep it, shutdown owns it.
    let running = gate(&mut ctx, &mut state);
    // A cache of another owner settles to nothing.
    let inputs = empty(&mut ctx);
    let foreign = new_future(&mut ctx, WORK, &[word::small(2).unwrap()], &mut other, inputs);
    // A success releases its outcome packet.
    let inputs = empty(&mut ctx);
    let done = new_future(&mut ctx, WORK, &[word::small(3).unwrap()], &mut state, inputs);
    runtime.champ_future_start(&mut ctx, 0, done);
    assert_eq!(get(done, tasks::RESULT), word::small(3).unwrap());
    assert_eq!(close(&mut ctx, &mut state, &[running, foreign, done]), 0);
    assert_eq!(get(running, tasks::STATE), tasks::RUNNING);
    assert_ne!(get(running, tasks::FLAGS) & DETACHED, 0);
    assert_eq!(get(running, tasks::FLAGS) & tasks::champ_flags::CANCEL_REQUESTED, 0);
    assert_eq!(runtime.champ_cache(running).unwrap().owner, runtime.champ.root_owner.unwrap());
    assert_eq!(runtime.champ_task_of(running).unwrap(), running);
    assert_eq!(runtime.spawned, [running]);
    assert_eq!(runtime.champ_cache(foreign).unwrap().flags, 0);
    assert_eq!(get(foreign, tasks::STATE), tasks::COLD);
    let done_cache = runtime.champ_cache(done).unwrap();
    assert_eq!((done_cache.flags, done_cache.packet), (RELEASED, 0));
    // A duty settled twice is refused before anything changes.
    let schedule = runtime.champ_register_schedule(2);
    let inputs = CloseInputs {
        scope: &mut other, packet: std::ptr::null_mut(), pending: std::ptr::null_mut(), failure: 0, settlements: 0,
    };
    let mut cleanup = CleanupSlot::default();
    assert_eq!(rt_cleanup_begin(&mut ctx, &inputs, &mut cleanup), NORMAL);
    runtime.roots.push((cleanup.record, Repr::Ref));
    assert_eq!(rt_service_settle(&mut ctx, 0, &mut cleanup, schedule, 0, foreign, &mut cleanup), NORMAL);
    let snapshot = (get(foreign, tasks::STATE), runtime.champ_cache(foreign).unwrap().flags);
    assert_eq!(snapshot, (tasks::FAILURE_STATE, RELEASED));
    assert_eq!(rt_service_settle(&mut ctx, 0, &mut cleanup, schedule, 1, foreign, &mut cleanup), OUTCOME_STOP);
    assert!(runtime.fatal.as_deref().unwrap().contains("settled twice"));
    ctx.status = OK;
    runtime.fatal = None;
    // Out of schedule order: the first duty opens the settlement.
    let mut fresh = owner(&mut ctx);
    let inputs = CloseInputs {
        scope: &mut fresh, packet: std::ptr::null_mut(), pending: std::ptr::null_mut(), failure: 0, settlements: 0,
    };
    let mut cleanup = CleanupSlot::default();
    assert_eq!(rt_cleanup_begin(&mut ctx, &inputs, &mut cleanup), NORMAL);
    assert_eq!(rt_service_settle(&mut ctx, 0, &mut cleanup, schedule, 1, done, &mut cleanup), OUTCOME_STOP);
    assert!(runtime.fatal.as_deref().unwrap().contains("schedule order"));
}

#[test]
fn champ_v8_cancel_request_follows_tomato_and_await_unwind_and_deliver_cancel_finish_it() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        // W and X await a spawned task G, in this order.
        let gate = gate(&mut ctx, &mut state);
        set(gate, tasks::FLAGS, get(gate, tasks::FLAGS) | tasks::champ_flags::SPAWNED);
        let mut waiters = Vec::new();
        for _ in 0..2 {
            let inputs = empty(&mut ctx);
            let waiter = new_future(&mut ctx, WATCH, &[gate], &mut state, inputs);
            runtime.champ_future_start(&mut ctx, 0, waiter);
            assert_eq!(get(waiter, tasks::AWAITING), gate);
            waiters.push(waiter);
        }
        let [w, x] = waiters[..] else { unreachable!() };
        // A shared child keeps running: W is withdrawn and resumed to raise Cancelled at its await.
        assert_eq!(rt_cancel_request(&mut ctx, w), NORMAL);
        let flags = get(w, tasks::FLAGS);
        assert_ne!(flags & tasks::champ_flags::CANCEL_REQUESTED, 0);
        assert_ne!(flags & tasks::champ_flags::CANCEL_DELIVERED, 0);
        assert_ne!(flags & tasks::DELIVER_NOW, 0);
        assert_eq!((get(w, tasks::AWAITING), get(gate, tasks::FLAGS) & tasks::champ_flags::CANCEL_REQUESTED), (0, 0));
        assert_eq!(runtime.ready.back().copied(), Some(w));
        runtime.schedule(&mut ctx, w);
        assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
        assert_eq!(kind(&runtime, get(w, tasks::FAILURE)), "Cancelled");
        assert_eq!(get(gate, tasks::STATE), tasks::RUNNING);
        assert_eq!(runtime.heap.payload::<tasks::Waiters>(get(gate, tasks::WAITER)).unwrap().0, [x]);
        // A request on a settled Future changes nothing.
        let before = (get(w, tasks::STATE), get(w, tasks::FLAGS));
        assert_eq!(rt_cancel_request(&mut ctx, w), NORMAL);
        assert_eq!((get(w, tasks::STATE), get(w, tasks::FLAGS)), before);
        // The sole awaiter of a spawned child cancels it and waits for it to settle (#147), then raises.
        assert_eq!(rt_cancel_request(&mut ctx, x), NORMAL);
        assert_ne!(get(gate, tasks::FLAGS) & tasks::champ_flags::CANCEL_REQUESTED, 0);
        assert_eq!(get(x, tasks::AWAITING), gate);
        assert!(!runtime.ready.contains(&x));
        let mut outcome = empty(&mut ctx);
        let published = PublishIn { tag: 0, value: word::small(5).unwrap(), packet: &mut outcome, failure: 0 };
        assert_eq!(rt_future_publish(&mut ctx, gate, &published), NORMAL);
        runtime.schedule(&mut ctx, x);
        assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
        assert_eq!(kind(&runtime, get(x, tasks::FAILURE)), "Cancelled");
        // await.unwind removes a registration that is still present and consumes the task and the receipt.
        let other = gate_of(&mut ctx, &mut state);
        let inputs = empty(&mut ctx);
        let parked = new_future(&mut ctx, WATCH, &[other], &mut state, inputs);
        runtime.champ_future_start(&mut ctx, 0, parked);
        assert_eq!(get(other, tasks::WAITER), parked);
        let mut scope = owner(&mut ctx);
        let failure = runtime.failure_new(FailureKind::Program("unwound".into()));
        runtime.roots.push((failure, Repr::Ref));
        let (mut task, mut receipt) = (other, super::futures::RECEIPT_WITHDRAWN);
        let mut post = AwaitUnwindPost::default();
        let record = scope.record;
        let outcome = rt_await_unwind(&mut ctx, parked, &mut scope, failure, &mut task, &mut receipt, &mut post);
        assert_eq!(outcome, NORMAL);
        assert_eq!((task, receipt, scope.record), (0, 0, 0));
        assert_eq!((post.state.record, post.failure), (record, failure));
        assert_eq!((get(other, tasks::WAITER), get(parked, tasks::AWAITING)), (0, 0));
        // deliver_cancel delivers nothing at the final exit (ruling #601 K2, Tomato parity): a pending request
        // neither becomes the close's primary nor is marked delivered.
        let tail = tail(&runtime, parked);
        let emergency = get(parked, tail + EMERGENCY);
        // Decision L2 (4c152ad02): the delivery follows the settle phase and the first plan.
        let plan = runtime.champ_register_cleanup_plan(CleanupPlan { reserve: false, direct: false, may_fail: true });
        let deliver = |ctx: &mut Ctx, scope: &mut ScopeSlot, primary: Word| {
            let inputs = CloseInputs {
                scope, packet: std::ptr::null_mut(), pending: std::ptr::null_mut(), failure: primary, settlements: 0,
            };
            let mut cleanup = CleanupSlot::default();
            assert_eq!(rt_cleanup_begin(ctx, &inputs, &mut cleanup), NORMAL);
            assert_eq!(rt_cleanup_settle_empty(ctx, &mut cleanup, &mut cleanup), NORMAL);
            assert_eq!(rt_cleanup_plan(ctx, &mut cleanup, plan, &mut cleanup), NORMAL);
            assert_eq!(rt_cleanup_deliver_cancel(ctx, parked, &mut cleanup, &mut cleanup), NORMAL);
            super::helpers::cleanup(unsafe { &*ctx.runtime }, cleanup.record).unwrap().failure
        };
        let mut quiet = owner(&mut ctx);
        assert_eq!(deliver(&mut ctx, &mut quiet, 0), 0, "nothing requested");
        set(parked, tasks::FLAGS, get(parked, tasks::FLAGS) | tasks::champ_flags::CANCEL_REQUESTED);
        let mut busy = owner(&mut ctx);
        assert_eq!(deliver(&mut ctx, &mut busy, failure), failure, "a primary keeps the request for later");
        assert_eq!(get(parked, tasks::FLAGS) & tasks::champ_flags::CANCEL_DELIVERED, 0);
        let mut last = owner(&mut ctx);
        assert_eq!(deliver(&mut ctx, &mut last, 0), 0, "the request stays pending");
        assert_eq!(get(parked, tasks::FLAGS) & tasks::champ_flags::CANCEL_DELIVERED, 0);
        assert_eq!(get(parked, tail + EMERGENCY), emergency);
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_v8_publish_refuses_a_future_that_is_not_running_or_a_foreign_outcome() {
    let mut runtime = load(false, Engine::Jit);
    let mut ctx = context(&mut runtime);
    ctx.entry_frame = frame_pointer!();
    let mut state = owner(&mut ctx);
    let inputs = empty(&mut ctx);
    let cold = new_future(&mut ctx, WORK, &[word::small(1).unwrap()], &mut state, inputs);
    let mut outcome = empty(&mut ctx);
    let published = PublishIn { tag: 0, value: word::small(1).unwrap(), packet: &mut outcome, failure: 0 };
    assert_eq!(rt_future_publish(&mut ctx, cold, &published), OUTCOME_STOP);
    assert_eq!(get(cold, tasks::STATE), tasks::COLD);
    ctx.status = OK;
    runtime.fatal = None;
    // An outcome packet that exit.commit did not move to the cache's holder and owner.
    let running = gate(&mut ctx, &mut state);
    let scope_owner = super::helpers::scope(&runtime, state.record).unwrap().owner;
    let (_, mut foreign) = resource(&mut ctx, scope_owner);
    runtime.roots.push((foreign.record, Repr::Ref));
    let published = PublishIn { tag: 0, value: 0, packet: &mut foreign, failure: 0 };
    assert_eq!(rt_future_publish(&mut ctx, running, &published), OUTCOME_STOP);
    assert!(runtime.fatal.as_deref().unwrap().contains("outside its cache"));
    assert_eq!(get(running, tasks::STATE), tasks::RUNNING);
    assert_ne!(foreign.record, 0);
}

#[test]
fn champ_v8_root_reports_the_first_unobserved_spawned_failure() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        // A failed task nobody observed, registered as P1's spawn registers it (draft 3 §7.3 step 3).
        let inputs = empty(&mut ctx);
        let task = new_future(&mut ctx, FAILS, &[], &mut state, inputs);
        runtime.champ_future_start(&mut ctx, 0, task);
        assert_eq!(get(task, tasks::STATE), tasks::FAILURE_STATE);
        assert_eq!(get(task, tasks::FLAGS) & tasks::champ_flags::OBSERVED, 0);
        set(task, tasks::FLAGS, get(task, tasks::FLAGS) | tasks::champ_flags::SPAWNED);
        runtime.spawned.push(task);
        let report = runtime.run_function(MAIN as usize, crate::STACK_BYTES).unwrap_err().0;
        assert!(report.starts_with("RuntimeError: boom"), "{report}");
        assert_eq!(count(&runtime, MAIN, FutureStep::Published), 1);
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

/// A body that returns with a failure status instead of publishing it.
extern "C" fn returns_failure(ctx: *mut Ctx, _frame: Word) -> Word {
    let failure = runtime_of(ctx).failure_new(FailureKind::Program("escaped".into()));
    unsafe {
        (*ctx).failure = failure;
        (*ctx).status = FAILED;
    }
    0
}

/// A body that says it is done without publishing.
extern "C" fn returns_unpublished(_ctx: *mut Ctx, _frame: Word) -> Word {
    0
}

/// A body that publishes a success holding a resource, committed into its cache as `exit.commit` does.
extern "C" fn returns_resource(ctx: *mut Ctx, frame: Word) -> Word {
    let runtime = runtime_of(ctx);
    let owner = runtime.champ_cache(frame).unwrap().owner;
    let object = runtime.heap.object_new(0, &[]);
    runtime.roots.push((object, Repr::Ref));
    let mut packet = PacketSlot::default();
    assert_eq!(rt_own_create_commit(ctx, object, 0, owner.get(), &mut packet), NORMAL);
    let inputs = ExitInputs { kind: 0, boundary: 0, value: 0, value_repr: 0, packet: &mut packet, selection: 0 };
    let mut pending = PendingSlot::default();
    assert_eq!(rt_exit_prepare(ctx, &inputs, &mut pending), NORMAL);
    let mut destination = ExitDestination { holder: HolderId(0), owner: OwnerId(0) };
    assert_eq!(rt_machine_future_destination(ctx, frame, &mut destination), NORMAL);
    let mut committed = CommitPost::default();
    assert_eq!(rt_exit_commit(ctx, &mut pending, &destination, &mut committed), NORMAL);
    let published = PublishIn { tag: 0, value: word::small(0).unwrap(), packet: &mut committed.packet, failure: 0 };
    assert_eq!(rt_future_publish(ctx, frame, &published), NORMAL);
    0
}

#[test]
fn champ_v8_run_body_and_root_refuse_what_a_body_must_not_do() {
    let cases: [(u32, extern "C" fn(*mut Ctx, Word) -> Word, &str); 3] = [
        (WORK, returns_failure, "returned a failure instead of publishing it"),
        (WORK, returns_unpublished, "result disagrees with its Future"),
        (MAIN, returns_resource, "main returned an ownership packet"),
    ];
    for (function, body, message) in cases {
        let mut runtime = load(true, Engine::Jit);
        runtime.invocations[function as usize].code = body as *const () as u64;
        let report = runtime.run_function(MAIN as usize, crate::STACK_BYTES).unwrap_err().0;
        assert!(report.contains(message), "{report}");
    }
}

/// Tomato's own deadlock: async main awaits an Event nobody sets (conformance `deadlock.two-tasks`).
const TOMATO_DEADLOCK: &str = r#"sparrow 16
module "deadlock" "1.0.0"
type t0 int
type t1 bool
type t2 object obj0
type t3 future t2

object obj0 "Event" {}

function f0 "Event.new" intrinsic "okkama.event.Event.new_ready" [obj0] [] {
    registers [t1]
    params [r0]
    result t2
}
function f1 "Event.wait" async intrinsic "okkama.event.Event.wait" [] [] {
    registers [t2]
    params [r0]
    result t2
}
function f2 "main" async {
    registers [t1, t2, t3, t2, t0]
    params []
    result t0
    0: const r0, bool false
    1: call r1, f0, [r0]
    2: call r2, f1, [r1]
    3: await r3, r2
    4: const r4, int 0
    5: ret r4
}
"#;

#[test]
fn champ_v8_deadlock_diagnostic_matches_tomato() {
    let tomato = {
        let program = okkama_sparrow::assemble(TOMATO_DEADLOCK).unwrap();
        program.verify().unwrap();
        let mut runtime = Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap());
        runtime.run_function(2, crate::STACK_BYTES).unwrap_err().0
    };
    assert_eq!(tomato, "RuntimeError: async main cannot complete: no task can make progress");
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        // The root awaits a task that never progresses and nothing external can wake.
        let gate = gate(&mut ctx, &mut state);
        let report = runtime.champ_run_async_main(&mut ctx, WATCH as usize, &[gate]).unwrap_err().0;
        assert_eq!(report, tomato);
        assert_eq!(steps(&runtime, WATCH), [FutureStep::Start, FutureStep::Pending]);
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

/// Publish a gate's success as its body would.
fn open_gate(ctx: &mut Ctx, gate: Word, value: i64) {
    let mut outcome = empty(ctx);
    let published = PublishIn { tag: 0, value: word::small(value).unwrap(), packet: &mut outcome, failure: 0 };
    assert_eq!(rt_future_publish(ctx, gate, &published), NORMAL);
}

#[test]
fn champ_v9_saved_slots_and_the_scope_area_cross_each_suspension() {
    let mut traces = Traces::default();
    for configuration in configurations() {
        let (stress, engine) = configuration;
        let mut runtime = load(stress, engine);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let first = gate(&mut ctx, &mut state);
        let second = gate(&mut ctx, &mut state);
        let inputs = empty(&mut ctx);
        let nested = new_future(&mut ctx, NESTED, &[first, second], &mut state, inputs);
        let map = runtime.champ_frame_map(NESTED as usize).unwrap().clone();
        let slot = |index: usize| map.slots[index].offset;
        let area = |scope: u32| map.scopes.iter().find(|(id, _)| *id == scope).unwrap().1;
        // Exactly these words of the slot and scope areas may be nonzero while suspended.
        let only = |held: &[u64]| (AREA..map.tail).step_by(8).filter(|offset| !held.contains(offset))
            .all(|offset| get(nested, offset) == 0);
        runtime.champ_future_start(&mut ctx, 0, nested);
        assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
        // PENDING at continuation 1: saved [3, 1], operand [0], and the outer scope's two words.
        assert_eq!(get(nested, tasks::RESUME), 1);
        let outer_record = get(nested, slot(3));
        let outer = super::helpers::scope(&runtime, outer_record).unwrap();
        assert_eq!((get(nested, slot(0)), get(nested, slot(1))), (first, second));
        assert_eq!((get(nested, area(NESTED)), get(nested, area(NESTED) + 8)), (outer.holder.get(), outer.owner.get()));
        assert!(only(&[slot(0), slot(1), slot(3), area(NESTED), area(NESTED) + 8]));
        // The first gate settles: the body resumes, begins the inner scope under the reloaded outer owner, and
        // suspends at continuation 2 with both scopes open.
        open_gate(&mut ctx, first, 4);
        assert_eq!(runtime.ready.pop_front(), Some(nested));
        assert!(!runtime.run_body(&mut ctx, nested));
        assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
        assert_eq!(get(nested, tasks::RESUME), 2);
        let inner = super::helpers::scope(&runtime, get(nested, slot(4))).unwrap();
        assert_eq!(inner.parent_owner, outer.owner.get());
        assert_eq!((get(nested, slot(3)), get(nested, slot(1))), (outer_record, second));
        assert_eq!((get(nested, area(INNER)), get(nested, area(INNER) + 8)), (inner.holder.get(), inner.owner.get()));
        let held = [slot(1), slot(3), slot(4), area(NESTED), area(NESTED) + 8, area(INNER), area(INNER) + 8];
        assert!(only(&held));
        open_gate(&mut ctx, second, 9);
        runtime.schedule(&mut ctx, nested);
        assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
        assert_eq!((get(nested, tasks::STATE), get(nested, tasks::RESULT)), (tasks::DONE, word::small(9).unwrap()));
        assert!(cleared(&runtime, nested));
        use FutureStep::*;
        // Each gate's publication queues the body once.
        assert_eq!(steps(&runtime, NESTED), [Start, Pending, Queued, Resume, Pending, Queued, Resume, Published]);
        traces.record(configuration, &runtime);
    }
    traces.agree();
}

#[test]
fn champ_v9_a_body_without_stack_closes_its_inputs_and_publishes_the_overflow() {
    for stress in [false, true] {
        let mut runtime = load(stress, Engine::Jit);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let scope_owner = super::helpers::scope(&runtime, state.record).unwrap().owner;
        let (r, inputs) = resource(&mut ctx, scope_owner);
        let future = new_future(&mut ctx, CONSUME, &[r], &mut state, inputs);
        let serial = runtime.heap.champ_identity(r).unwrap().serial;
        // The body's entry lies below the limit; the rejection runs in the headroom reserved below it.
        let marker = 0u8;
        ctx.stack_limit = &marker as *const u8 as u64 + 64 * 1024;
        runtime.champ_future_start(&mut ctx, 0, future);
        ctx.stack_limit = 0;
        assert_eq!(ctx.status, OK, "{:?}", runtime.fatal);
        assert_eq!(get(future, tasks::STATE), tasks::FAILURE_STATE);
        assert!(kind(&runtime, get(future, tasks::FAILURE)).contains("Stack overflow"));
        assert_eq!(runtime.champ.destroy_events, [serial]);
        assert_eq!(runtime.heap.champ_identity(r).unwrap().life, Life::Dead);
        assert!(cleared(&runtime, future));
        assert_eq!(steps(&runtime, CONSUME), [FutureStep::Start, FutureStep::Published]);
        // A resumed body holds state only its own code can close: running out of stack there stops.
        let inputs = empty(&mut ctx);
        let resumed = new_future(&mut ctx, WORK, &[word::small(1).unwrap()], &mut state, inputs);
        set(resumed, tasks::STATE, tasks::RUNNING);
        set(resumed, tasks::RESUME, 1);
        ctx.stack_limit = &marker as *const u8 as u64 + 64 * 1024;
        runtime.run_body(&mut ctx, resumed);
        ctx.stack_limit = 0;
        assert!(runtime.fatal.as_deref().unwrap().contains("while resuming an async body"));
    }
}

// --- #601 V7: fixed roots and deps inputs (#667) ---------------------------------------------------------------

/// `async main() is int { 7; }` as the program's entry: a fixed root starts without inputs and without a packet.
fn fixed_root() -> (s::Function, s::FrameLayout) {
    let (mut b, frame) = Body::asynchronous("main", 0, vec![s::Type::Int]);
    let (body, inputs) = b.resume(frame, 0, &[]);
    assert!(inputs.is_empty());
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(0), A::Boundary(0)], K::ScopeState);
    let seven = b.constant(s::Constant::Int("7".into()), s::Type::Int);
    b.return_value(body, q0, 0, Some(seven), &[], "done");
    let layout = b.layout(vec![], vec![(vec![], vec![])]);
    (b.finish(), layout)
}

#[test]
fn champ_v7_fixed_root_runs_from_an_empty_start() {
    let (function, layout) = fixed_root();
    let program = s::Program {
        module: "root".into(),
        functions: vec![function],
        entry: Some(0),
        tables: s::ChampTables {
            scopes: vec![s::Scope { function: 0, parent: None, boundary: 0 }],
            boundaries: vec![s::Boundary { scope: 0 }],
            frames: vec![layout],
            ..s::ChampTables::default()
        },
        ..s::Program::default()
    };
    s::verify_load(&program, &s::NoExternalContracts).unwrap();
    for stress in [false, true] {
        let mut runtime = Runtime::new_champ(program.clone(), Vec::new()).unwrap();
        runtime.heap.set_stress(stress);
        runtime.invocations[0].code = reference as *const () as u64;
        let map = runtime.champ_frame_map(0).unwrap();
        assert_eq!((map.start.len(), map.inputs()), (0, 0));
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        assert_eq!(runtime.champ_run_async_main(&mut ctx, 0, &[]), Ok(word::small(7).unwrap()));
    }
}

#[test]
fn champ_v7_deps_inputs_take_no_words() {
    let mut p = program();
    // `work(n is int)` with the deps of a borrowed input after it: one language input, then the packet.
    let layout = &mut p.tables.frames[WORK as usize];
    layout.slots = vec![value(s::Type::Int), service(K::Deps), service(K::Packet)];
    layout.continuations[0].saved = vec![0, 1, 2];
    let map = FrameMap::new(&p, WORK as usize).unwrap();
    assert_eq!(map.slots[1].reprs, []);
    assert_eq!((map.inputs(), map.packet()), (1, Some(map.slots[2].offset)));
}

/// One step of a close that must succeed.
fn step(runtime: &Runtime, outcome: Outcome) {
    assert_eq!(outcome, NORMAL, "{:?}", runtime.fatal);
}

/// Run the current plan's executor to done, destroying what it selects; the serials it destroyed.
fn execute(ctx: &mut Ctx, cleanup: &mut CleanupSlot) -> Vec<u64> {
    let runtime = unsafe { &mut *ctx.runtime };
    let mut destroyed = Vec::new();
    loop {
        let mut next = NextPost::default();
        let ordinal = rt_cleanup_next(ctx, cleanup, &mut next);
        *cleanup = next.state;
        if ordinal == 1 {
            return destroyed;
        }
        step(runtime, ordinal);
        destroyed.push(next.key.serial());
        let mut post = DestroyPost::default();
        step(runtime, rt_cleanup_destroy(ctx, cleanup, next.key, &mut post));
        assert_eq!(post.failure, 0);
        *cleanup = post.state;
    }
}

/// The D18 step of a final exit keeps its place and its once-only rule (decision L2), and it delivers nothing: a
/// pending request leaves the result to commit (ruling #601 K2, Tomato parity; the step goes with #727).
#[test]
fn champ_v10_d18_step_runs_once_between_the_plans_and_leaves_the_result() {
    for (stress, requested) in [(false, true), (true, true), (false, false), (true, false)] {
        let mut runtime = load(stress, Engine::Jit);
        let mut ctx = context(&mut runtime);
        ctx.entry_frame = frame_pointer!();
        let mut state = owner(&mut ctx);
        let scope_owner = super::helpers::scope(&runtime, state.record).unwrap().owner;
        // The body at its final exit: a pending Return whose reserve holds R.
        let inputs = empty(&mut ctx);
        let body = new_future(&mut ctx, WORK, &[word::small(1).unwrap()], &mut state, inputs);
        if requested {
            set(body, tasks::FLAGS, get(body, tasks::FLAGS) | tasks::champ_flags::CANCEL_REQUESTED);
        }
        let (r, mut packet) = resource(&mut ctx, scope_owner);
        let serial = runtime.heap.champ_identity(r).unwrap().serial;
        let returned = ExitInputs { kind: 0, boundary: 0, value: 0, value_repr: 0, packet: &mut packet, selection: 0 };
        let mut pending = PendingSlot::default();
        step(&runtime, rt_exit_prepare(&mut ctx, &returned, &mut pending));
        runtime.roots.push((pending.record, Repr::Ref));
        let plan = CleanupPlan { reserve: true, direct: false, may_fail: false };
        let first = runtime.champ_register_cleanup_plan(plan);
        let second = runtime.champ_register_cleanup_plan(plan);
        let inputs = CloseInputs {
            scope: &mut state, packet: std::ptr::null_mut(), pending: &mut pending, failure: 0, settlements: 0,
        };
        let mut cleanup = CleanupSlot::default();
        step(&runtime, rt_cleanup_begin(&mut ctx, &inputs, &mut cleanup));
        runtime.roots.push((cleanup.record, Repr::Ref));
        step(&runtime, rt_cleanup_settle_empty(&mut ctx, &mut cleanup, &mut cleanup));
        // The second plan and the delivery are refused before the first plan.
        assert_eq!(rt_cleanup_deliver_cancel(&mut ctx, body, &mut cleanup, &mut cleanup), OUTCOME_STOP);
        assert!(runtime.fatal.take().unwrap().contains("delivered once, between the two plans"));
        ctx.status = OK;
        step(&runtime, rt_cleanup_plan(&mut ctx, &mut cleanup, first, &mut cleanup));
        assert_eq!(rt_cleanup_plan(&mut ctx, &mut cleanup, second, &mut cleanup), OUTCOME_STOP);
        assert!(runtime.fatal.take().unwrap().contains("follows a delivered cancellation"));
        ctx.status = OK;
        assert!(execute(&mut ctx, &mut cleanup).is_empty(), "the reserve is not the first plan's");
        // The delivery runs once, after the first plan has run to done.
        step(&runtime, rt_cleanup_deliver_cancel(&mut ctx, body, &mut cleanup, &mut cleanup));
        assert_eq!(rt_cleanup_deliver_cancel(&mut ctx, body, &mut cleanup, &mut cleanup), OUTCOME_STOP);
        runtime.fatal = None;
        ctx.status = OK;
        let direct = runtime.champ_register_cleanup_plan(CleanupPlan { direct: true, ..plan });
        assert_eq!(rt_cleanup_plan(&mut ctx, &mut cleanup, direct, &mut cleanup), OUTCOME_STOP);
        assert!(runtime.fatal.take().unwrap().contains("its own executor plan"));
        ctx.status = OK;
        // The second plan finds no aborted reserve: R stays pending, requested or not.
        step(&runtime, rt_cleanup_plan(&mut ctx, &mut cleanup, second, &mut cleanup));
        let destroyed = execute(&mut ctx, &mut cleanup);
        assert_eq!(rt_cleanup_plan(&mut ctx, &mut cleanup, second, &mut cleanup), OUTCOME_STOP, "never a third");
        runtime.fatal = None;
        ctx.status = OK;
        let mut finished = FinishPost::default();
        step(&runtime, rt_cleanup_finish(&mut ctx, &mut cleanup, &mut finished));
        let flags = get(body, tasks::FLAGS);
        assert_eq!(flags & tasks::champ_flags::CANCEL_DELIVERED, 0);
        assert_eq!(flags & tasks::champ_flags::CANCEL_REQUESTED != 0, requested, "a request stays pending");
        assert_eq!((destroyed, finished.failure), (vec![], 0));
        // The reserve commits to the caller and closes there.
        let mut kept = finished.pending;
        let destination = ExitDestination {
            holder: runtime.champ_new_holder().unwrap(), owner: runtime.champ_new_owner().unwrap(),
        };
        let mut committed = CommitPost::default();
        step(&runtime, rt_exit_commit(&mut ctx, &mut kept, &destination, &mut committed));
        let inputs = CloseInputs {
            scope: std::ptr::null_mut(), packet: &mut committed.packet, pending: std::ptr::null_mut(), failure: 0,
            settlements: 0,
        };
        let mut caller = CleanupSlot::default();
        step(&runtime, rt_cleanup_begin(&mut ctx, &inputs, &mut caller));
        step(&runtime, rt_cleanup_settle_empty(&mut ctx, &mut caller, &mut caller));
        let plain = runtime.champ_register_cleanup_plan(CleanupPlan { reserve: false, ..plan });
        step(&runtime, rt_cleanup_plan(&mut ctx, &mut caller, plain, &mut caller));
        assert_eq!(execute(&mut ctx, &mut caller), [serial]);
        let mut closed = FinishPost::default();
        step(&runtime, rt_cleanup_finish(&mut ctx, &mut caller, &mut closed));
        assert_eq!(closed.failure, 0);
        assert_eq!(runtime.champ.destroy_events, [serial]);
        assert_eq!(runtime.heap.champ_identity(r).unwrap().life, Life::Dead);
    }
}

#[path = "async_resources_tests.rs"]
mod resources;

#[path = "async_cancel_tests.rs"]
mod cancel;
