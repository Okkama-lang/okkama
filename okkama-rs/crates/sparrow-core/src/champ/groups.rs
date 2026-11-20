//! D23 FutureGroup on Champ (#810; memo OIR-2-async-remaining §6, rulings PR-B, PR-C, PR-E).
//!
//! A group's record is the payload of its runtime cell (PR-C): Tomato's `Group` (members, seal, cancellation, the
//! cached result), which the `wait` driver shares with Tomato, and the duties `start` registered. Registration is
//! decided by the packet (§6.3): a fresh Future's duties come in `start`'s packet and join the record; a bound
//! Future's packet is empty, and the group only retains the Future. Either way `start` then spawns the Future as
//! `fut.spawn.bound` does (PR-E). The group's own duty `{group, SERVICE_GROUP}` is in the packet `new` returns and
//! follows the packet ABI like a Future's entries; it settles in its owner's dynamic step, where the registered duties
//! settle by their own rows, nested first (§6.4).
//!
//! The settle does not seal the group: only `wait` and `cancel` do (std `async/scope`, Tomato from the rebuild
//! tree). A member that a running Future starts later, after the owner closed, has no group duty to join: it is
//! spawned, and its duties settle at once by their rows (a running one detaches to the root).
use super::helpers::{consume, owner_preflight, owner_transition, packet, replace, stop, store};
use super::records::{Data, Packet};
use super::*;
use crate::heap::Payload;
use crate::primitives::Group;
use crate::runtime::{self, Ctx};
use crate::word::Repr;
use okkama_sparrow::spb17 as s;

#[derive(Debug)]
pub(super) struct GroupHolder {
    pub group: Group,
    /// The duties `start` registered, in registration order (I-G1); they take the group's owner.
    pub registered: Packet,
    /// The owner of the group's own duty: the scope or cache whose packet holds it. None until the first admission.
    pub owner: Option<OwnerId>,
    /// The group's own duty has settled: its owner closed. Later members are not registered.
    pub settled: bool,
}

impl Payload for GroupHolder {
    fn trace(&self, visit: &mut dyn FnMut(Word, Repr)) {
        self.group.entries.iter().for_each(|&entry| visit(entry, Repr::Ref));
        if let Some(Ok(word) | Err(word)) = self.group.result {
            visit(word, Repr::Ref);
        }
        self.registered.trace(visit);
    }
    fn external_bytes(&self) -> usize {
        self.group.entries.capacity() * size_of::<Word>() + self.group.owned.capacity()
            + self.registered.services.capacity() * size_of::<Service>()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl Runtime {
    /// Whether `object` is a FutureGroup specialization of the running program.
    pub(super) fn champ_group_object(&self, object: Word) -> bool {
        if object == 0 || object & 1 != 0 || heap::kind(self.heap.header_of(object)) != heap::OBJECT {
            return false;
        }
        let descriptor = heap::payload(self.heap.header_of(object)) as usize;
        self.champ.machine.as_ref().is_some_and(|machine| machine.groups.get(descriptor).copied().unwrap_or(false))
    }

    pub(super) fn champ_group(&self, object: Word) -> Result<&GroupHolder, Error> {
        if !self.champ_group_object(object) {
            return Err(error("a group entry whose holder is not a FutureGroup"));
        }
        self.heap.payload::<GroupHolder>(self.heap.field(object, 0)).ok_or_else(|| error("a FutureGroup without its record"))
    }

    pub(super) fn champ_group_mut(&mut self, object: Word) -> Result<&mut GroupHolder, Error> {
        if !self.champ_group_object(object) {
            return Err(error("a group entry whose holder is not a FutureGroup"));
        }
        let cell = self.heap.field(object, 0);
        self.heap.payload_mut::<GroupHolder>(cell).ok_or_else(|| error("a FutureGroup without its record"))
    }

    /// Tomato's group state of a Champ group, for the drivers both engines share (`primitives::group_mut`).
    pub(crate) fn champ_group_state(&mut self, object: Word) -> Option<&mut Group> {
        self.champ_group_mut(object).ok().map(|holder| &mut holder.group)
    }

    /// Preflight of the group's own duty taking `owner` (G1 §3 through ABI D25.1): its registered duties and
    /// identities can take the transition, as a cache's outcome and inputs do.
    pub(super) fn champ_group_preflight(&self, group: Word, owner: OwnerId) -> Result<(), Error> {
        let holder = self.champ_group(group)?;
        if holder.owner == Some(owner) {
            return Ok(());
        }
        owner_preflight(self, &holder.registered, owner)
    }

    /// The owner transition of a preflighted group duty: the group and everything it registered take `owner`.
    pub(super) fn champ_group_transition(&mut self, group: Word, owner: OwnerId) {
        let holder = self.champ_group_mut(group).expect("preflighted group");
        if holder.owner == Some(owner) {
            return;
        }
        holder.owner = Some(owner);
        let registered = std::mem::take(&mut holder.registered);
        let moved = owner_transition(self, registered, owner);
        self.champ_group_mut(group).expect("preflighted group").registered = moved;
    }

    /// Plan the settle of a group's own duty (memo §6.4, §2.4), changing nothing: each duty it registered settles by
    /// its own row (D24.2) in registration order, each released packet's entries first; the identities it holds join
    /// `released`. A registered member is never cold: `start` spawned it.
    pub(super) fn champ_plan_group(&self, service: Service, closing: Option<OwnerId>,
        plan: &mut Vec<(Service, CacheSettle)>, released: &mut Packet) -> Result<(), Error>
    {
        let holder = self.champ_group(service.holder)?;
        if holder.settled {
            return Err(error("a group duty settled twice"));
        }
        if closing.is_some() && holder.owner != closing {
            return Err(error("a group duty settles in a close of another owner"));
        }
        let mut registered = holder.registered.clone();
        plan.push((service, CacheSettle::Nothing));
        let nested = std::mem::take(&mut registered.services);
        self.champ_plan_services(&nested, closing, plan, released)?;
        released.append(registered);
        Ok(())
    }

    /// Apply a planned group settle: nothing registered, and nothing will be. The group is not sealed: `start` still
    /// accepts members, which settle on their own (`start`).
    pub(super) fn champ_group_settled(&mut self, group: Word) -> Result<(), Error> {
        let holder = self.champ_group_mut(group)?;
        if std::mem::replace(&mut holder.settled, true) {
            return Err(error("a group duty settled twice"));
        }
        holder.registered = Packet::default();
        Ok(())
    }
}

/// The synchronous members of a ready holder row (`name` is Tomato's): FutureGroup's and FutureCompletions'
/// count. `wait` and `completion` are drivers (`drivers.rs`).
pub(super) fn invoke(ctx: *mut Ctx, function: usize, name: &str, io: *mut CallOwn, args: &[Word]) -> Word {
    match name {
        "okkama.scope.FutureGroup.new" => new(ctx, function, io),
        "okkama.scope.FutureGroup.start" => start(ctx, function, io, args[0], args[1]),
        "okkama.scope.FutureGroup.active" => {
            let runtime = unsafe { &mut *(*ctx).runtime };
            let Ok(holder) = runtime.champ_group(args[0]) else {
                return stop(ctx, error("FutureGroup.active of an object that is not a group"));
            };
            let active = holder.group.entries.iter()
                .filter(|&&entry| matches!(crate::tasks::get(entry, crate::tasks::STATE), crate::tasks::COLD | crate::tasks::RUNNING))
                .count();
            runtime.heap.int(active as i64)
        }
        "okkama.scope.FutureGroup.cancel" => {
            let runtime = unsafe { &mut *(*ctx).runtime };
            if runtime.champ_group(args[0]).is_err() {
                return stop(ctx, error("FutureGroup.cancel of an object that is not a group"));
            }
            runtime.group_cancel(ctx, args[0]);
            0
        }
        // A stream's count (#810 G1 step 4): Tomato's intrinsic, over the stream in the receiver's runtime word.
        "okkama.collect.FutureCompletions.remaining_count" => crate::intrinsics::champ_call(ctx, function, args),
        _ => stop(ctx, error(&format!("{name} has no synchronous entry"))),
    }
}

/// `FutureGroup()`: a fresh group, and an output packet that holds its own duty (§6.2).
fn new(ctx: *mut Ctx, function: usize, io: *mut CallOwn) -> Word {
    let runtime = unsafe { &mut *(*ctx).runtime };
    let source = runtime.champ.machine.as_ref().expect("Champ program").source.clone();
    let Some(&s::Type::Object(descriptor)) = source.functions[function].results.first() else {
        return stop(ctx, error("FutureGroup.new without its object"));
    };
    if io.is_null() {
        return stop(ctx, error("FutureGroup.new without its packet"));
    }
    let input = unsafe { &mut (*io).input };
    if !packet(runtime, input.record).is_ok_and(|offered| offered.is_empty()) {
        return stop(ctx, error("FutureGroup.new received obligations"));
    }
    let record = GroupHolder { group: Group::default(), registered: Packet::default(), owner: None, settled: false };
    let cell = runtime.heap.allocate_payload(Box::new(record));
    let group = runtime.heap.object_new(descriptor, &[cell]);
    let mut post = CallPost::default();
    if rt_own_move(ctx, input, &mut post.packet) != NORMAL {
        return OUTCOME_STOP;
    }
    let duty = Service { holder: group, kind: SERVICE_GROUP, target: None };
    replace(runtime, post.packet.record, Data::Packet(Packet { services: vec![duty], ..Packet::default() }));
    post.value = group;
    unsafe { (*io).output.write(post) };
    NORMAL
}

/// `start(future)` (§6.3): a group that is sealed refuses a Future that is not already its member with
/// `FinishedFutureGroup`; otherwise the Future becomes a member once, the duties in the packet are registered, and
/// the Future is spawned. A refused packet closes here, as a failing body closes what it accepted: a fresh Future
/// settles Cancelled and never starts, as on Tomato, and its inputs are released once. A group whose own duty has
/// settled registers nothing: the Future is spawned first, then the packet closes, and its duties settle by their
/// rows (a running member detaches to the root, a finished one releases its outcome).
fn start(ctx: *mut Ctx, function: usize, io: *mut CallOwn, group: Word, future: Word) -> Word {
    let runtime = unsafe { &mut *(*ctx).runtime };
    if io.is_null() {
        return stop(ctx, error("FutureGroup.start without its packet"));
    }
    let input = unsafe { &mut (*io).input };
    let (sealed, known, owner, settled) = match runtime.champ_group(group) {
        Ok(holder) => (holder.group.sealed, holder.group.entries.contains(&future), holder.owner, holder.settled),
        Err(failure) => return stop(ctx, failure),
    };
    if sealed && !known {
        let kind = crate::intrinsics::thrown(runtime, function, 0, &[]);
        let failure = runtime.failure_new(kind);
        let roots = runtime.roots.len();
        runtime.roots.push((failure, Repr::Ref));
        let closed = super::cleanup::close_packet(ctx, input, failure);
        unsafe { &mut *(*ctx).runtime }.roots.truncate(roots);
        let failure = match closed {
            Ok(combined) => combined,
            Err(_) if matches!(unsafe { (*ctx).status }, runtime::FATAL | runtime::EXHAUSTED) => return OUTCOME_STOP,
            Err(failure) => return stop(ctx, failure),
        };
        unsafe {
            (*ctx).failure = failure;
            (*ctx).status = runtime::FAILED;
            (*io).output.write(CallPost { value: 0, packet: PacketSlot::default(), failure });
        }
        return UNWIND;
    }
    if settled {
        return start_unregistered(ctx, io, group, future, known);
    }
    // Registration: every duty the packet holds joins the record and takes the group's owner, checked before the
    // first store (I-G1: one place per duty).
    // A fresh temporary group comes with its own duty in the packet (review F1): the duty never joins the group's
    // own record. It closes in this call, after the registration and the spawn, as a fresh argument does.
    let mut own = Vec::new();
    let registered = (|| {
        let mut offered = packet(runtime, input.record)?;
        let itself = |service: &Service| service.holder == group && service.kind == SERVICE_GROUP;
        own = offered.services.iter().copied().filter(itself).collect();
        offered.services.retain(|service| !itself(service));
        if offered.is_empty() {
            return Ok(());
        }
        let owner = owner.ok_or_else(|| error("FutureGroup.start of a group without its owner"))?;
        let mut all = runtime.champ_group(group)?.registered.clone();
        all.append(offered.clone());
        super::helpers::validate(runtime, &all)?;
        owner_preflight(runtime, &offered, owner)?;
        // #882: a temporary Future started here leaves the starter for the group's owner; it borrows what it
        // reaches of every other owner.
        let futures: Vec<Word> = offered.services.iter()
            .filter(|service| service.kind == SERVICE_CACHE).map(|service| service.holder).collect();
        let reached = super::cleanup::reached_from(runtime, &futures)?;
        super::cleanup::register_borrowers(runtime, &futures, &reached, owner.0)?;
        let moved = owner_transition(runtime, offered, owner);
        runtime.champ_group_mut(group)?.registered.append(moved);
        Ok(())
    })();
    if let Err(failure) = registered {
        return stop(ctx, failure);
    }
    consume(runtime, input.record);
    input.record = 0;
    if !known {
        let holder = runtime.champ_group_mut(group).expect("checked group");
        holder.group.entries.push(future);
        holder.group.owned.push(false);
    }
    // PR-E: a cold member starts as a task of its own on this stack; a running or settled one is left as it is.
    if rt_future_spawn_bound(ctx, future) != NORMAL {
        return OUTCOME_STOP;
    }
    if !own.is_empty() {
        let runtime = unsafe { &mut *(*ctx).runtime };
        let mut slot = PacketSlot { record: store(runtime, Data::Packet(Packet { services: own, ..Packet::default() })) };
        return finish(ctx, io, &mut slot);
    }
    let mut post = CallPost::default();
    let runtime = unsafe { &mut *(*ctx).runtime };
    post.packet.record = store(runtime, Data::Packet(Packet::default()));
    unsafe { (*io).output.write(post) };
    NORMAL
}

/// The end of a `start` that closes a packet in the call: each duty settles for its own owner (D24.2 rows); a destroy
/// failure fails the call, otherwise its output packet is empty.
fn finish(ctx: *mut Ctx, io: *mut CallOwn, packet: &mut PacketSlot) -> Word {
    let failure = match super::cleanup::close_packet(ctx, packet, 0) {
        Ok(failure) => failure,
        Err(_) if matches!(unsafe { (*ctx).status }, runtime::FATAL | runtime::EXHAUSTED) => return OUTCOME_STOP,
        Err(failure) => return stop(ctx, failure),
    };
    if failure != 0 {
        unsafe {
            (*ctx).failure = failure;
            (*ctx).status = runtime::FAILED;
            (*io).output.write(CallPost { value: 0, packet: PacketSlot::default(), failure });
        }
        return UNWIND;
    }
    let mut post = CallPost::default();
    let runtime = unsafe { &mut *(*ctx).runtime };
    post.packet.record = store(runtime, Data::Packet(Packet::default()));
    unsafe { (*io).output.write(post) };
    NORMAL
}

/// `start` on a group whose own duty has settled: the Future becomes a member and is spawned, with its packet rooted,
/// then the packet closes for each duty's own owner (D24.2 rows), so no duty is left without a place.
fn start_unregistered(ctx: *mut Ctx, io: *mut CallOwn, group: Word, future: Word, known: bool) -> Word {
    let runtime = unsafe { &mut *(*ctx).runtime };
    if !known {
        let holder = runtime.champ_group_mut(group).expect("checked group");
        holder.group.entries.push(future);
        holder.group.owned.push(false);
    }
    let input = unsafe { &mut (*io).input };
    let roots = runtime.roots.len();
    runtime.roots.push((input.record, Repr::Ref));
    let spawned = rt_future_spawn_bound(ctx, future);
    unsafe { &mut *(*ctx).runtime }.roots.truncate(roots);
    if spawned != NORMAL {
        return OUTCOME_STOP;
    }
    finish(ctx, io, input)
}
