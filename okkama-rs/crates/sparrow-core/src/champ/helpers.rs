//! P1 L/G helpers. Preflight never mutates inputs; the following publication batch has no safepoint.
#![allow(clippy::not_unsafe_ptr_arg_deref)]
use super::records::*;
use super::{HolderId, KeyRef, Life, OwnerId};
use crate::heap;
use crate::runtime::{self, Ctx, FailureKind, Runtime, FAILED};
use crate::word::{Repr, Word};
use crate::Error;
use std::collections::HashSet;

pub(super) fn error(message: &str) -> Error {
    Error(format!("Champ: {message}"))
}
pub(super) fn data(runtime: &Runtime, record: Word) -> Result<&Data, Error> {
    if record == 0 {
        return Err(error("empty or consumed service slot"));
    }
    runtime.heap.payload::<Record>(record).map(|record| &record.0).ok_or_else(|| error("service type"))
}
pub(super) fn store(runtime: &mut Runtime, data: Data) -> Word {
    runtime.heap.allocate_payload(Box::new(Record(data)))
}
pub(super) fn consume(runtime: &mut Runtime, record: Word) {
    runtime.heap.payload_mut::<Record>(record).expect("validated record").0 = Data::Consumed;
}
pub(super) fn packet(runtime: &Runtime, record: Word) -> Result<Packet, Error> {
    match data(runtime, record)? {
        Data::Packet(packet) => Ok(packet.clone()),
        _ => Err(error("expected packet")),
    }
}
pub(super) fn scope(runtime: &Runtime, record: Word) -> Result<Scope, Error> {
    match data(runtime, record)? {
        Data::Scope(scope) => Ok(scope.clone()),
        _ => Err(error("expected scope")),
    }
}
fn pending(runtime: &Runtime, record: Word) -> Result<Pending, Error> {
    match data(runtime, record)? {
        Data::Pending(pending) => Ok(pending.clone()),
        _ => Err(error("expected pending")),
    }
}
pub(super) fn cleanup(runtime: &Runtime, record: Word) -> Result<Cleanup, Error> {
    match data(runtime, record)? {
        Data::Cleanup(cleanup) => Ok(cleanup.clone()),
        _ => Err(error("expected cleanup")),
    }
}
pub(super) fn replace(runtime: &mut Runtime, record: Word, data: Data) {
    runtime.heap.payload_mut::<Record>(record).expect("validated record").0 = data;
}
pub(super) fn stop(ctx: *mut Ctx, failure: Error) -> Outcome {
    unsafe {
        runtime::fatal(&mut *ctx, &mut *(*ctx).runtime, failure.0);
    }
    OUTCOME_STOP
}
pub(super) fn fail(ctx: *mut Ctx, failure: Error) -> Word {
    unsafe {
        let runtime = &mut *(*ctx).runtime;
        let failure = runtime.failure_new(FailureKind::Program(failure.0));
        (*ctx).failure = failure;
        (*ctx).status = FAILED;
        failure
    }
}
fn leaf(ctx: *mut Ctx, body: impl FnOnce(&mut Runtime) -> Result<(), Error>) -> Outcome {
    if unsafe { (*ctx).status } != runtime::OK || unsafe { (*ctx).failure } != 0 {
        return stop(ctx, error("L helper entered with active bridge failure"));
    }
    match body(unsafe { &mut *(*ctx).runtime }) {
        Ok(()) => NORMAL,
        Err(failure) => stop(ctx, failure),
    }
}

impl Runtime {
    /// Typed binding/field metadata is installed with verified function metadata before execution.
    pub fn champ_register_value_repr(&mut self, owner: OwnerId, slot: u64, stage: bool, repr: Repr) {
        self.champ.value_reprs.insert((owner.0, slot, stage), repr);
    }
    pub fn champ_scope_parent(&self, slot: &ScopeSlot) -> Result<u64, Error> {
        Ok(scope(self, slot.record)?.parent_owner)
    }
    pub fn champ_pending_contract(&self, slot: &PendingSlot) -> Result<(u64, u64), Error> {
        let pending = pending(self, slot.record)?;
        Ok((pending.kind, pending.boundary))
    }
    /// Consume fixed guarded slots into a chunked packet. Stack maps trace every object offset;
    /// inactive entries are zero, so late branches and safepoints never read uninitialized storage.
    pub fn champ_packet_from_static(&mut self, entries: &mut [StaticEntry], holder: HolderId, owner: OwnerId)
        -> Result<PacketSlot, Error>
    {
        let mut words = Vec::new();
        for entry in entries.iter() {
            match entry.active {
                0 if entry.object == 0 => {}
                1 => words.push(entry.object),
                _ => return Err(error("invalid static active guard")),
            }
        }
        let packet = run(self, words, holder, owner);
        validate(self, &packet)?;
        let record = store(self, Data::Packet(packet));
        for entry in entries {
            *entry = StaticEntry::default();
        }
        Ok(PacketSlot { record })
    }
    /// Function-local static plans are relocated to runtime IDs before entering machine code.
    pub fn champ_register_selection(&mut self, indices: Vec<usize>) -> PlanId {
        self.champ.selections.push(indices);
        self.champ.selections.len() as u64
    }
    pub fn champ_register_entry(&mut self, contract: EntryContract) -> EntryId {
        self.champ.entries.push(contract);
        self.loaded.program.functions.len() as u64 + self.champ.entries.len() as u64 - 1
    }
    pub(super) fn champ_entry_contract(&self, entry: EntryId) -> Option<EntryContract> {
        let index = entry.checked_sub(self.loaded.program.functions.len() as u64)?;
        self.champ.entries.get(index as usize).cloned()
    }
    pub fn champ_packet_keys(&self, slot: &PacketSlot) -> Result<Vec<KeyRef>, Error> {
        packet(self, slot.record)?.words().map(|object| {
            let identity = self.heap.champ_identity(object).ok_or_else(|| error("identity"))?;
            Ok(KeyRef { object, serial: identity.serial })
        }).collect()
    }
}

pub(super) fn validate(runtime: &Runtime, packet: &Packet) -> Result<(), Error> {
    let mut seen = HashSet::new();
    for run in packet.runs() {
        for object in run.words() {
            let identity = runtime.heap.champ_identity(object).ok_or_else(|| error("packet identity"))?;
            runtime.champ_check_key(KeyRef { object, serial: identity.serial }, run.holder, run.owner, Life::Live)?;
            if !seen.insert(object) {
                return Err(error("overlapping packets"));
            }
        }
    }
    Ok(())
}
pub(super) fn run(runtime: &Runtime, mut words: Vec<Word>, holder: HolderId, owner: OwnerId) -> Packet {
    words.sort_unstable_by_key(|object| std::cmp::Reverse(runtime.heap.champ_identity(*object).unwrap().serial));
    let chunks = words.chunks(CHUNK_ENTRIES).map(<[Word]>::to_vec).collect();
    Packet::from_run(Run { holder, owner, chunks })
}
/// All epochs are checked before the first store. A no-op writes no tail words.
pub(super) fn preflight(runtime: &Runtime, packet: &Packet, holder: HolderId, owner: OwnerId)
    -> Result<(), Error>
{
    if holder.0 == 0 || owner.0 == 0 {
        return Err(error("zero destination ID"));
    }
    validate(runtime, packet)?;
    for object in packet.words() {
        let identity = runtime.heap.champ_identity(object).unwrap();
        if (identity.holder_id != holder.0 || identity.owner_id != owner.0) && identity.epoch == u64::MAX {
            return Err(error("epoch exhausted"));
        }
    }
    Ok(())
}
pub(super) fn transition(runtime: &mut Runtime, packet: Packet, holder: HolderId, owner: OwnerId) -> Packet {
    let words: Vec<_> = packet.words().collect();
    for &object in &words {
        let identity = runtime.heap.champ_identity(object).unwrap();
        if identity.holder_id != holder.0 || identity.owner_id != owner.0 {
            // Only changed tail words are written; serial and life are untouched.
            let address = runtime.heap.identity_address(object).unwrap() as *mut u64;
            unsafe {
                if identity.holder_id != holder.0 {
                    address.add(1).write(holder.0);
                }
                if identity.owner_id != owner.0 {
                    address.add(2).write(owner.0);
                }
                address.add(3).write(identity.epoch + 1);
            }
        }
    }
    let mut result = Packet::default();
    for (index, runs) in packet.journals.into_iter().enumerate() {
        let words = runs.iter().flat_map(Run::words).collect();
        result.journals[index] = run(runtime, words, holder, owner).journals[0].clone();
    }
    result
}
fn partition(runtime: &Runtime, packet: &Packet, selected: &HashSet<Word>) -> (Packet, Packet) {
    let mut rest = Packet::default();
    let mut picked = Packet::default();
    for (index, runs) in packet.journals.iter().enumerate() {
        for source in runs {
            let (yes, no): (Vec<_>, Vec<_>) = source.words().partition(|word| selected.contains(word));
            picked.journals[index].append(&mut run(runtime, yes, source.holder, source.owner).journals[0]);
            rest.journals[index].append(&mut run(runtime, no, source.holder, source.owner).journals[0]);
        }
    }
    (rest, picked)
}
fn static_selection(runtime: &Runtime, packet: &Packet, plan: PlanId) -> Result<HashSet<Word>, Error> {
    let indices = runtime.champ.selections.get(plan.wrapping_sub(1) as usize)
        .ok_or_else(|| error("unknown static selection"))?;
    let words: Vec<_> = packet.words().collect();
    let mut selected = HashSet::new();
    for &index in indices {
        let object = *words.get(index).ok_or_else(|| error("static selection index"))?;
        if !selected.insert(object) {
            return Err(error("duplicate static selection"));
        }
    }
    Ok(selected)
}
fn checked_selection(runtime: &Runtime, packet: &Packet, selection: &Selection) -> Result<HashSet<Word>, Error> {
    selection_walk(runtime, packet, selection, true)
}
fn selection_walk(runtime: &Runtime, packet: &Packet, selection: &Selection, strict: bool)
    -> Result<HashSet<Word>, Error>
{
    validate(runtime, packet)?;
    let available: HashSet<_> = packet.words().collect();
    let mut work = if selection.count == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(selection.roots, selection.count as usize) }.to_vec()
    };
    let mut seen = HashSet::new();
    let mut selected = HashSet::new();
    while let Some(object) = work.pop() {
        if object == 0 || object & 1 != 0 || !seen.insert(object) {
            continue;
        }
        let header = runtime.heap.header_of(object);
        if matches!(heap::kind(header), heap::STRING | heap::BIGINT) {
            continue;
        }
        if heap::kind(header) != heap::OBJECT {
            return Err(error("unsupported selection root"));
        }
        if let Some(identity) = runtime.heap.champ_identity(object) {
            if strict && (!available.contains(&object) || identity.life != Life::Live) {
                return Err(error("selection outside source authority"));
            }
            if available.contains(&object) {
                selected.insert(object);
            }
        }
        let layout = runtime.heap.object_runtime_layout(heap::payload(header));
        for &index in layout.selection_layout() {
            work.push(runtime.heap.field(object, index));
        }
    }
    Ok(selected)
}

macro_rules! leaf_helper {
    ($name:ident($ctx:ident $(, $arg:ident: $ty:ty)*) |$runtime:ident| $body:block) => {
        pub extern "C" fn $name($ctx: *mut Ctx $(, $arg: $ty)*) -> Outcome {
            leaf($ctx, |$runtime| unsafe { $body })
        }
    };
}
leaf_helper!(rt_scope_begin(ctx, instance: *const ScopeInstance, post: *mut ScopeSlot) |runtime| {
    let instance = &*instance;
    if instance.holder.0 == 0 || instance.owner.0 == 0 {
        return Err(error("zero scope ID"));
    }
    let scope = Scope {
        holder: instance.holder,
        owner: instance.owner,
        parent_owner: instance.parent_owner,
        admission: Packet::default(),
        bound: Packet::default(),
        staging: Packet::default(),
        bindings: Vec::new(),
        fields: Vec::new(),
    };
    let record = store(runtime, Data::Scope(scope));
    post.write(ScopeSlot { record });
    Ok(())
});
leaf_helper!(rt_own_empty(ctx, post: *mut PacketSlot) |runtime| {
    let record = store(runtime, Data::Packet(Packet::default()));
    post.write(PacketSlot { record });
    Ok(())
});
leaf_helper!(rt_own_move(ctx, input: *mut PacketSlot, post: *mut PacketSlot) |runtime| {
    packet(runtime, (*input).record)?;
    let record = (*input).record;
    (*input).record = 0;
    post.write(PacketSlot { record });
    Ok(())
});
leaf_helper!(rt_own_join(ctx, inputs: *mut PacketSlot, count: u64, post: *mut PacketSlot) |runtime| {
    let inputs = if count == 0 { &mut [] } else { std::slice::from_raw_parts_mut(inputs, count as usize) };
    let mut joined = Packet::default();
    let mut records = HashSet::new();
    for input in inputs.iter() {
        if !records.insert(input.record) {
            return Err(error("duplicate packet consume"));
        }
        joined.append(packet(runtime, input.record)?);
    }
    validate(runtime, &joined)?;
    let record = store(runtime, Data::Packet(joined));
    for input in inputs {
        consume(runtime, input.record);
        input.record = 0;
    }
    post.write(PacketSlot { record });
    Ok(())
});
leaf_helper!(rt_own_create_commit(ctx, object: Word, descriptor: u64, owner: u64, post: *mut PacketSlot) |runtime| {
    if owner == 0 || heap::payload(runtime.heap.header_of(object)) as u64 != descriptor {
        return Err(error("completion descriptor or owner"));
    }
    let holder = runtime.champ_new_holder()?;
    let key = runtime.champ_create_commit(object, holder, OwnerId(owner))?;
    let packet = run(runtime, key.into_iter().map(KeyRef::object).collect(), holder, OwnerId(owner));
    let record = store(runtime, Data::Packet(packet));
    post.write(PacketSlot { record });
    Ok(())
});
fn admit(runtime: &mut Runtime, state: Word, input: Word, bound: bool) -> Result<Word, Error> {
    let mut scope = scope(runtime, state)?;
    let packet = packet(runtime, input)?;
    let mut all = scope.all();
    all.append(packet.clone());
    validate(runtime, &all)?;
    preflight(runtime, &packet, scope.holder, scope.owner)?;
    let packet = transition(runtime, packet, scope.holder, scope.owner);
    if bound {
        scope.bound.append(packet);
    } else {
        let [admission, mut bound, mut staging] = packet.journals;
        bound.append(&mut staging);
        scope.admission.append(Packet { journals: [admission, Vec::new(), Vec::new()] });
        scope.bound.append(Packet { journals: [Vec::new(), bound, Vec::new()] });
    }
    let record = store(runtime, Data::Scope(scope));
    consume(runtime, state);
    consume(runtime, input);
    Ok(record)
}
leaf_helper!(rt_own_admit(ctx, state: *mut ScopeSlot, input: *mut PacketSlot, post: *mut ScopeSlot) |runtime| {
    let record = admit(runtime, (*state).record, (*input).record, false)?;
    (*state).record = 0;
    (*input).record = 0;
    post.write(ScopeSlot { record });
    Ok(())
});
leaf_helper!(rt_own_accept(ctx, state: *mut ScopeSlot, input: *mut PacketSlot, post: *mut ScopeSlot) |runtime| {
    let record = admit(runtime, (*state).record, (*input).record, true)?;
    (*state).record = 0;
    (*input).record = 0;
    post.write(ScopeSlot { record });
    Ok(())
});
fn bind(runtime: &mut Runtime, state: Word, binding: u64, value: Word, input: Word, stage: bool)
    -> Result<Word, Error>
{
    let mut scope = scope(runtime, state)?;
    let packet = packet(runtime, input)?;
    let mut all = scope.all();
    all.append(packet.clone());
    validate(runtime, &all)?;
    preflight(runtime, &packet, scope.holder, scope.owner)?;
    let repr = *runtime.champ.value_reprs.get(&(scope.owner.0, binding, stage))
        .ok_or_else(|| error("missing binding/field trace layout"))?;
    let selected = if repr == Repr::Ref && value != 0 {
        selection_walk(runtime, &all, &Selection { roots: &value, count: 1 }, false)?
    } else {
        HashSet::new()
    };
    let (admission, classified) = partition(runtime, &scope.admission, &selected);
    scope.admission = admission;
    // History remains bound/staged even when the latest language value is replaced.
    let packet = transition(runtime, packet, scope.holder, scope.owner);
    if stage {
        scope.staging.append(classified);
        scope.staging.append(packet);
        scope.fields.retain(|(field, _, _)| *field != binding);
        scope.fields.push((binding, value, repr));
    } else {
        scope.bound.append(classified);
        scope.bound.append(packet);
        scope.bindings.retain(|(id, _, _)| *id != binding);
        scope.bindings.push((binding, value, repr));
    }
    let record = store(runtime, Data::Scope(scope));
    consume(runtime, state);
    consume(runtime, input);
    Ok(record)
}
leaf_helper!(rt_own_bind(ctx, state: *mut ScopeSlot, binding: u64, value: Word,
    input: *mut PacketSlot, post: *mut ScopeSlot) |runtime| {
    let record = bind(runtime, (*state).record, binding, value, (*input).record, false)?;
    (*state).record = 0;
    (*input).record = 0;
    post.write(ScopeSlot { record });
    Ok(())
});
leaf_helper!(rt_own_rebind(ctx, state: *mut ScopeSlot, binding: u64, value: Word,
    input: *mut PacketSlot, post: *mut ScopeSlot) |runtime| {
    let record = bind(runtime, (*state).record, binding, value, (*input).record, false)?;
    (*state).record = 0;
    (*input).record = 0;
    post.write(ScopeSlot { record });
    Ok(())
});
leaf_helper!(rt_own_stage(ctx, inputs: *const StageInputs, post: *mut StagePost) |runtime| {
    let inputs = &*inputs;
    if inputs.source.is_null() || inputs.source == inputs.state {
        let record = bind(runtime, (*inputs.state).record, inputs.field, inputs.value, (*inputs.packet).record, true)?;
        (*inputs.state).record = 0;
        (*inputs.packet).record = 0;
        post.write(StagePost { state: ScopeSlot { record }, source: ScopeSlot::default() });
        return Ok(());
    }
    let source_record = (*inputs.source).record;
    let state_record = (*inputs.state).record;
    if source_record == state_record {
        return Err(error("aliased stage states"));
    }
    let mut source = scope(runtime, source_record)?;
    let mut root = scope(runtime, state_record)?;
    let selected = static_selection(runtime, &source.all(), inputs.selection)?;
    let mut selected = scope_partition(runtime, &mut source, &selected);
    selected.append(packet(runtime, (*inputs.packet).record)?);
    let mut all = root.all();
    all.append(selected.clone());
    validate(runtime, &all)?;
    preflight(runtime, &selected, root.holder, root.owner)?;
    let repr = *runtime.champ.value_reprs.get(&(root.owner.0, inputs.field, true))
        .ok_or_else(|| error("missing staging trace layout"))?;
    let selected = transition(runtime, selected, root.holder, root.owner);
    root.staging.append(selected);
    root.fields.retain(|(field, _, _)| *field != inputs.field);
    root.fields.push((inputs.field, inputs.value, repr));
    let source = store(runtime, Data::Scope(source));
    let state = store(runtime, Data::Scope(root));
    consume(runtime, source_record);
    consume(runtime, state_record);
    consume(runtime, (*inputs.packet).record);
    (*inputs.source).record = 0;
    (*inputs.state).record = 0;
    (*inputs.packet).record = 0;
    post.write(StagePost { state: ScopeSlot { record: state }, source: ScopeSlot { record: source } });
    Ok(())
});
fn split(runtime: &mut Runtime, original: &Packet, selected: &HashSet<Word>) -> Result<(Packet, Packet), Error> {
    validate(runtime, original)?;
    let (rest, selected) = partition(runtime, original, selected);
    let holder = runtime.champ_new_holder()?;
    for part in selected.runs() {
        preflight(runtime, &Packet::from_run(part.clone()), holder, part.owner)?;
    }
    let mut output = Packet::default();
    for (index, runs) in selected.journals.into_iter().enumerate() {
        for part in runs {
            let owner = part.owner;
            let mut updated = transition(runtime, Packet::from_run(part), holder, owner);
            output.journals[index].append(&mut updated.journals[0]);
        }
    }
    Ok((rest, output))
}
leaf_helper!(rt_own_split_static(ctx, input: *mut PacketSlot, plan: PlanId, post: *mut SplitPost) |runtime| {
    let packet = packet(runtime, (*input).record)?;
    let selected = static_selection(runtime, &packet, plan)?;
    let (rest, selected) = split(runtime, &packet, &selected)?;
    let rest = store(runtime, Data::Packet(rest));
    let selected = store(runtime, Data::Packet(selected));
    consume(runtime, (*input).record);
    (*input).record = 0;
    post.write(SplitPost { rest: PacketSlot { record: rest }, selected: PacketSlot { record: selected }, failure: 0 });
    Ok(())
});
pub extern "C" fn rt_own_split_checked(ctx: *mut Ctx, input: *mut PacketSlot,
    selection: *const Selection, post: *mut SplitPost) -> Outcome
{
    if unsafe { (*ctx).status } != runtime::OK || unsafe { (*ctx).failure } != 0 {
        return stop(ctx, error("checked helper entered with active bridge failure"));
    }
    let runtime = unsafe { &mut *(*ctx).runtime };
    let original = unsafe { (*input).record };
    let result = (|| {
        let packet = packet(runtime, original)?;
        let selected = checked_selection(runtime, &packet, unsafe { &*selection })?;
        split(runtime, &packet, &selected)
    })();
    let (outcome, output) = match result {
        Ok((rest, selected)) => {
            let rest = store(runtime, Data::Packet(rest));
            let selected = store(runtime, Data::Packet(selected));
            consume(runtime, original);
            (NORMAL, SplitPost {
                rest: PacketSlot { record: rest }, selected: PacketSlot { record: selected }, failure: 0,
            })
        }
        Err(error) => (UNWIND, SplitPost {
            rest: PacketSlot { record: original }, selected: PacketSlot::default(), failure: fail(ctx, error),
        }),
    };
    unsafe {
        (*input).record = 0;
        post.write(output);
    }
    outcome
}
fn scope_partition(runtime: &Runtime, scope: &mut Scope, selected: &HashSet<Word>) -> Packet {
    let mut picked = Packet::default();
    for journal in [&mut scope.admission, &mut scope.bound, &mut scope.staging] {
        let (rest, part) = partition(runtime, journal, selected);
        *journal = rest;
        picked.append(part);
    }
    picked
}
pub extern "C" fn rt_scope_split(ctx: *mut Ctx, input: *mut ScopeSlot,
    selection: *const Selection, post: *mut ScopeSplitPost) -> Outcome
{
    if unsafe { (*ctx).status } != runtime::OK || unsafe { (*ctx).failure } != 0 {
        return stop(ctx, error("checked helper entered with active bridge failure"));
    }
    let runtime = unsafe { &mut *(*ctx).runtime };
    let original = unsafe { (*input).record };
    let result = (|| {
        let mut scope = scope(runtime, original)?;
        let all = scope.all();
        let selected = checked_selection(runtime, &all, unsafe { &*selection })?;
        let (_, output) = split(runtime, &all, &selected)?;
        scope_partition(runtime, &mut scope, &selected);
        let output = store(runtime, Data::Packet(output));
        Ok::<_, Error>((store(runtime, Data::Scope(scope)), output))
    })();
    let (outcome, output) = match result {
        Ok((state, selected)) => {
            consume(runtime, original);
            (NORMAL, ScopeSplitPost {
                state: ScopeSlot { record: state }, selected: PacketSlot { record: selected }, failure: 0,
            })
        }
        Err(error) => (UNWIND, ScopeSplitPost {
            state: ScopeSlot { record: original }, selected: PacketSlot::default(), failure: fail(ctx, error),
        }),
    };
    unsafe {
        (*input).record = 0;
        post.write(output);
    }
    outcome
}
pub extern "C" fn rt_scope_transfer(ctx: *mut Ctx, inputs: *const TransferInputs,
    selection: *const Selection, post: *mut TransferPost) -> Outcome
{
    if unsafe { (*ctx).status } != runtime::OK || unsafe { (*ctx).failure } != 0 {
        return stop(ctx, error("checked helper entered with active bridge failure"));
    }
    let inputs = unsafe { &*inputs };
    let runtime = unsafe { &mut *(*ctx).runtime };
    let from = unsafe { (*inputs.from).record };
    let to = unsafe { (*inputs.to).record };
    let same = inputs.from == inputs.to;
    let result = (|| {
        let mut source = scope(runtime, from)?;
        let all = source.all();
        let selected = checked_selection(runtime, &all, unsafe { &*selection })?;
        if same {
            return Ok((from, 0));
        }
        if from == to {
            return Err(error("aliased scope states"));
        }
        let mut target = scope(runtime, to)?;
        let selected = scope_partition(runtime, &mut source, &selected);
        preflight(runtime, &selected, target.holder, target.owner)?;
        let mut all = target.all();
        all.append(selected.clone());
        validate(runtime, &all)?;
        let selected = transition(runtime, selected, target.holder, target.owner);
        target.bound.append(selected);
        Ok((store(runtime, Data::Scope(source)), store(runtime, Data::Scope(target))))
    })();
    let (outcome, output) = match result {
        Ok((source, target)) => {
            if !same {
                consume(runtime, from);
                consume(runtime, to);
            }
            (NORMAL, TransferPost {
                from: ScopeSlot { record: source }, to: ScopeSlot { record: target }, failure: 0,
            })
        }
        Err(error) => (UNWIND, TransferPost {
            from: ScopeSlot { record: from }, to: ScopeSlot { record: if same { 0 } else { to } },
            failure: fail(ctx, error),
        }),
    };
    unsafe {
        (*inputs.from).record = 0;
        (*inputs.to).record = 0;
        post.write(output);
    }
    outcome
}
leaf_helper!(rt_exit_prepare(ctx, inputs: *const ExitInputs, post: *mut PendingSlot) |runtime| {
    let inputs = &*inputs;
    let reserve = packet(runtime, (*inputs.packet).record)?;
    validate(runtime, &reserve)?;
    let record = store(runtime, Data::Pending(Pending {
        kind: inputs.kind,
        boundary: inputs.boundary,
        value: inputs.value,
        selection: inputs.selection,
        value_repr: Repr::from_bits(inputs.value_repr),
        reserve,
    }));
    consume(runtime, (*inputs.packet).record);
    (*inputs.packet).record = 0;
    post.write(PendingSlot { record });
    Ok(())
});
leaf_helper!(rt_exit_extend(ctx, input: *mut PendingSlot, part: *mut PacketSlot, post: *mut PendingSlot) |runtime| {
    let mut pending = pending(runtime, (*input).record)?;
    pending.reserve.append(packet(runtime, (*part).record)?);
    validate(runtime, &pending.reserve)?;
    let record = store(runtime, Data::Pending(pending));
    consume(runtime, (*input).record);
    consume(runtime, (*part).record);
    (*input).record = 0;
    (*part).record = 0;
    post.write(PendingSlot { record });
    Ok(())
});
leaf_helper!(rt_exit_replace(ctx, input: *mut PendingSlot, replacement: *const StaticReplacement,
    post: *mut ReplacePost) |runtime| {
    let replacement = &*replacement;
    let mut pending = pending(runtime, (*input).record)?;
    let selected = static_selection(runtime, &pending.reserve, replacement.selection)?;
    let (rest, mut kept) = partition(runtime, &pending.reserve, &selected);
    kept.append(packet(runtime, (*replacement.packet).record)?);
    validate(runtime, &kept)?;
    pending.value = replacement.value;
    pending.value_repr = Repr::from_bits(replacement.value_repr);
    pending.boundary = replacement.boundary;
    pending.selection = replacement.selection;
    pending.reserve = kept;
    let record = store(runtime, Data::Pending(pending));
    let rest = store(runtime, Data::Packet(rest));
    consume(runtime, (*input).record);
    consume(runtime, (*replacement.packet).record);
    (*input).record = 0;
    (*replacement.packet).record = 0;
    post.write(ReplacePost { pending: PendingSlot { record }, rest: PacketSlot { record: rest } });
    Ok(())
});
leaf_helper!(rt_exit_commit(ctx, input: *mut PendingSlot, destination: *const ExitDestination,
    post: *mut CommitPost) |runtime| {
    let pending = pending(runtime, (*input).record)?;
    let destination = &*destination;
    preflight(runtime, &pending.reserve, destination.holder, destination.owner)?;
    let packet = transition(runtime, pending.reserve, destination.holder, destination.owner);
    let record = store(runtime, Data::Packet(packet));
    consume(runtime, (*input).record);
    (*input).record = 0;
    post.write(CommitPost { value: pending.value, packet: PacketSlot { record } });
    Ok(())
});
leaf_helper!(rt_exit_abort(ctx, input: *mut PendingSlot, state: *mut CleanupSlot,
    post: *mut CleanupSlot) |runtime| {
    let pending = pending(runtime, (*input).record)?;
    let mut cleanup = cleanup(runtime, (*state).record)?;
    cleanup.remainder.append(pending.reserve);
    validate(runtime, &cleanup.remainder)?;
    let record = store(runtime, Data::Cleanup(cleanup));
    consume(runtime, (*input).record);
    consume(runtime, (*state).record);
    (*input).record = 0;
    (*state).record = 0;
    post.write(CleanupSlot { record });
    Ok(())
});
