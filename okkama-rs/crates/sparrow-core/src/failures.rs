//! Failures, resource regions and the text of a failure that reaches the top.
//!
//! Regions own destroy-bearing objects created while they were open. Closing
//! a region runs each owned object's `destroy` (newest first) as an ordinary
//! machine call; a failing destroy becomes the primary failure or a secondary
//! of it, and the remaining destroys still run. Ownership states and the
//! traversal of an adopted value follow the previous engine.
#![allow(unsafe_code)]

use crate::heap::{self, Payload};
use crate::runtime::{Ctx, FAILED, Failure, FailureKind, OK, Runtime};
use crate::word::{Repr, UNBOUND, Word};
use okkama_sparrow::{CatchKind, Instruction as I, Program, Type};
use crate::word::{FastMap as HashMap, FastSet as HashSet};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Free,
    Region(Word),
    Dead,
    /// #909 2b: a static resource, destroyed by its scope's `resource.leave`/`resource.unwind`; no walk takes it.
    Static,
}

/// The owner word of an inline identity (#528): free, dead, or a region's address (aligned, so never 0 or 1).
const OWNER_FREE: u64 = 0;
const OWNER_DEAD: u64 = 1;
const OWNER_STATIC: u64 = 2;

fn encode_owner(owner: Owner) -> u64 {
    match owner {
        Owner::Free => OWNER_FREE,
        Owner::Dead => OWNER_DEAD,
        Owner::Static => OWNER_STATIC,
        Owner::Region(region) => region,
    }
}

/// What a region close does after one destroy.
enum DestroyStep {
    /// Go on with this failure so far.
    Next(Option<Word>),
    /// An async destroy to await (only when the close awaits).
    Async(Option<Word>),
    /// The run stopped (fatal or cancelled); close nothing more.
    Stop(Option<Word>),
}

fn decode_owner(word: u64) -> Owner {
    match word {
        OWNER_FREE => Owner::Free,
        OWNER_DEAD => Owner::Dead,
        OWNER_STATIC => Owner::Static,
        region => Owner::Region(region),
    }
}

#[derive(Debug)]
struct Identity {
    serial: u64,
    owner: Owner,
    /// Escaping a shared cache preserves a scope owner, never fresh call
    /// admission. The next track installs that owner before a callee can adopt.
    cache_rehomed: bool,
    /// Additional group packets retaining this Future. Its lexical owner may
    /// expire first; the last live owner drains the cache and retained inputs.
    groups: Vec<Word>,
}

/// Explicit ownership of destroy-bearing objects; disabled without `destroy`.
#[derive(Debug, Default)]
pub struct Resources {
    pub enabled: bool,
    /// Per descriptor: can it reach a destroy-bearing identity?
    pub flags: Vec<bool>,
    /// Per descriptor (#528): the byte offset of the identity words (serial, owner) kept in the object, for an
    /// object with a destroy of its own; `None` keeps the identity in the map.
    pub(crate) inline: Vec<Option<u32>>,
    /// Per descriptor (#905): may a program-built instance (`obj.new`, `obj.next`, `variant.new`) be a leaf when
    /// its fields reach no identity? Not with a destroy of its own, a builtin (a runtime cell that changes in
    /// place) or a `once let` slot (filled after construction).
    pub(crate) leaf_capable: Vec<bool>,
    serial: u64,
    identities: HashMap<Word, Identity>,
    /// Work list, visited set and open containers of `resource_graph`, reused.
    scratch: (Vec<(Word, Repr)>, Vec<Word>, Vec<Walked>),
    /// Groups and Futures queued by a closing region, reused (#932).
    close_lists: (Vec<Word>, Vec<Word>),
    /// #817: the root region. A Future still running when its owner closes detaches here, not cancelled; the
    /// root closer (`close_root`, after shutdown) closes its outcome. A GC root (`Runtime::safepoint`).
    pub(crate) root: Option<Word>,
    /// The root closer is running: what still runs then has nowhere further to go.
    root_closing: bool,
    /// #915 message pins (Book 20.2): identity → the regions holding a pin on it (a channel's packet, a receiving
    /// Future's cache region, the scope that took the value), with repeats; and the reverse index.
    pins: HashMap<Word, Vec<Word>>,
    pinned_by: HashMap<Word, Vec<Word>>,
    /// Pins a taking await or a returning scope left for the next region that takes this exact value.
    pending_pins: Option<(Word, Vec<Word>)>,
    /// Containers whose walk found no free identity (#9), with `serial` at
    /// that moment: every identity they reach is at most that old. Freeing
    /// an identity no newer than `summarized` drops them all.
    summaries: HashMap<Word, u64>,
    summarized: u64,
    /// Tweed #906: per immutable container (array version, closure, map, box), the children that may hold an
    /// identity, found once. Structure, not ownership: it never goes stale, since the container never changes
    /// and an object that is not an identity never becomes one (#905). Dropped when the container is reclaimed.
    structure: HashMap<Word, Box<[(Word, Repr)]>>,
    /// Development measurement (OKKAMA_DEV=runtime, #909): per identity, the region that first owned it and
    /// whether it ever moved to another owner. Empty unless the category is on.
    origins: HashMap<Word, (Word, bool)>,
    /// `SPARROW_TRACE_DESTROY=FILE`: one line per destroyed identity, type and
    /// creation serial, the oracle for static ownership (#555).
    trace: Option<std::fs::File>,
}

/// A container whose elements `resource_graph` is walking: its subtree is
/// done when the work list is back to `base`.
#[derive(Debug, Clone, Copy)]
struct Walked {
    container: Word,
    base: usize,
    steps: usize,
    events: usize,
}

/// Walk steps under one container worth a summary: smaller ones are cheaper
/// to walk again than to remember.
const SUMMARY_STEPS: usize = 64;

/// Entries of a region beyond its inline slots.
#[derive(Debug, Default)]
pub struct RegionValue {
    entries: Vec<Word>,
    /// The entries as a set: membership checks stay constant-time.
    members: HashSet<Word>,
    /// The oldest serial among `entries` (`None`: no entries).
    oldest: Option<u64>,
    /// A `region.settle` that waits (#154): the async destroy it awaits and
    /// the failure gathered so far.
    settling: Option<Word>,
    settled: Option<Word>,
}

/// Where a region close stopped (#154).
pub(crate) enum Closed {
    /// Every identity is destroyed; the primary failure, if any.
    Done(Option<Word>),
    /// `object`, already dead, has an async destroy to await.
    Async(Word, Option<Word>),
}

/// One close action selected without calling out or mutating the heap.
/// Returning ends the entry borrow before summary invalidation or destruction.
enum CloseAction {
    Freed(u64),
    Destroy(Word, u64),
}

/// A region is an array `[overflow, slot × REGION_SLOTS]`: identities fill the
/// slots in order, then the overflow `RegionValue`; an empty first slot means
/// an empty region (machine code checks just that word).
pub const REGION_SLOTS: usize = 4;
impl Payload for RegionValue {
    fn trace(&self, visit: &mut dyn FnMut(Word, Repr)) {
        for &entry in self.entries.iter().chain(&self.settling).chain(&self.settled) {
            visit(entry, Repr::Ref);
        }
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl Resources {
    /// Test runtimes start fresh; mark even tests with no destroyed objects.
    pub(crate) fn trace_test(name: &str) {
        use std::io::Write;
        if let Some(mut file) = std::env::var_os("SPARROW_TRACE_DESTROY")
            .and_then(|path| std::fs::OpenOptions::new().create(true).append(true).open(path).ok())
        {
            let _ = writeln!(file, "# test {name}");
        }
    }

    pub fn new(program: &Program) -> Self {
        Self {
            enabled: program.objects.iter().any(|o| o.destroy.is_some()),
            flags: descriptor_flags(program),
            inline: program.objects.iter().map(|object| (object.destroy.is_some() && !object.builtin)
                .then(|| 8 * (1 + object.fields.len().max(1)) as u32)).collect(),
            leaf_capable: program.objects.iter().map(|object| object.destroy.is_none() && !object.builtin
                && !object.fields.iter().any(|(name, _)| name.starts_with("$once"))).collect(),
            trace: std::env::var_os("SPARROW_TRACE_DESTROY")
                .and_then(|path| std::fs::OpenOptions::new().create(true).append(true).open(path).ok()),
            ..Self::default()
        }
    }
    /// #588: the ownership metadata compiled code and allocated objects depend on is unchanged in `extended`
    /// for every published descriptor (and resources stay on or off).
    pub(crate) fn metadata_prefix_of(&self, extended: &Self) -> bool {
        self.enabled == extended.enabled
            && extended.flags.starts_with(&self.flags)
            && extended.inline.starts_with(&self.inline)
            && extended.leaf_capable.starts_with(&self.leaf_capable)
    }

    /// #588: take an extended program's ownership metadata (checked by `metadata_prefix_of`); runtime state stays.
    pub(crate) fn extend_metadata(&mut self, extended: Self) {
        self.flags = extended.flags;
        self.inline = extended.inline;
        self.leaf_capable = extended.leaf_capable;
    }

    /// #909: machine code may count serials, write inline identities and destroy static resources itself when
    /// nothing observes those steps one by one (no destroy trace).
    pub(crate) fn inline_ownership(&self) -> bool {
        self.enabled && self.trace.is_none()
    }

    /// The address of the serial counter, for machine code that numbers new identities (#909). Stable: the
    /// runtime lives in a box for as long as its machine code.
    pub(crate) fn serial_address(&self) -> u64 {
        &self.serial as *const u64 as u64
    }

    /// Drop identities and summaries of objects the collector reclaimed.
    pub fn forget(&mut self, released: &[Word]) {
        for object in released {
            self.identities.remove(object);
            self.summaries.remove(object);
            self.origins.remove(object);
            self.structure.remove(object);
        }
    }
}

/// Closed-world reachability of destroy-bearing objects through declared
/// types; a role reaches what the objects doing it reach.
pub(crate) fn descriptor_flags(program: &Program) -> Vec<bool> {
    let offset = program.types.len();
    let mut parents = vec![Vec::new(); offset + program.objects.len()];
    let mut reachable = vec![false; parents.len()];
    let mut pending = Vec::new();
    let mut roles: HashMap<u32, Vec<usize>> = HashMap::default();
    for function in &program.functions {
        if let Some(intrinsic) = &function.intrinsic
            && matches!(intrinsic.name.as_str(), "okkama.scope.FutureGroup.new" | "okkama.scope.FutureScope.outcome")
        {
            pending.push(offset + intrinsic.objects[0].0 as usize);
        }
    }
    // #587: a channel's capabilities own its unread messages (OIR-2 §7.3), so Channel<T>, Sending<T> and
    // Receiving<T> carry T as if it were a field; the same rule as the compiler's (`ir::channel_carriers`).
    let object_of = |ty: okkama_sparrow::TypeRef| match program.types.get(ty.0 as usize) {
        Some(Type::Object(id)) => Some(offset + id.0 as usize),
        _ => None,
    };
    let mut carried = Vec::new();
    for function in &program.functions {
        let Some(intrinsic) = &function.intrinsic else { continue };
        let param = |index: usize| function.params.get(index).map(|register| function.registers[register.0 as usize]);
        match intrinsic.name.as_str() {
            "okkama.channel.Sending.send" => {
                if let (Some(holder), Some(message)) = (param(0).and_then(object_of), param(1)) {
                    carried.push((message.0 as usize, holder));
                }
            }
            "okkama.channel.Receiving.item" => {
                if let (Some(holder), Some(message)) = (param(0).and_then(object_of), function.result) {
                    carried.push((message.0 as usize, holder));
                }
            }
            "okkama.channel.Channel.sending" | "okkama.channel.Channel.receiving" => {
                if let (Some(channel), Some(endpoint)) = (param(0).and_then(object_of), function.result.and_then(object_of)) {
                    carried.push((channel, endpoint));
                    carried.push((endpoint, channel));
                }
            }
            _ => {}
        }
    }
    for (child, parent) in carried {
        parents[child].push(parent);
    }
    for (index, ty) in program.types.iter().enumerate() {
        let child = match ty {
            Type::Object(id) | Type::Successor(id) => Some(offset + id.0 as usize),
            Type::Array(t) => Some(t.0 as usize),
            Type::Map(_, v) => Some(v.0 as usize),
            Type::Int
            | Type::Bool
            | Type::Byte
            | Type::Char
            | Type::Float
            | Type::Rat
            | Type::Str
            | Type::Enum(_)
            | Type::Range
            | Type::Region
            | Type::Builder(_)
            | Type::MapBuilder(..)
            // The walk never enters a native state.
            | Type::Handle(_) => None,
            // A role value is one of the objects doing it (linked below);
            // runtime-made values of a builtin role (`Error`) are not.
            Type::Role(role) if !program.roles[role.0 as usize].builtin => {
                roles.entry(role.0).or_default().push(index);
                None
            }
            // A variant value is an object of one of its cases (#290).
            Type::Variant(variant) => {
                for (_, case) in &program.variants[variant.0 as usize].cases {
                    parents[offset + case.0 as usize].push(index);
                }
                None
            }
            _ => {
                pending.push(index);
                None
            }
        };
        if let Some(child) = child {
            parents[child].push(index);
        }
    }
    for (index, object) in program.objects.iter().enumerate() {
        let node = offset + index;
        if object.destroy.is_some() {
            pending.push(node);
        }
        for witness in &object.does {
            if let Some(types) = roles.get(&witness.role.0) {
                parents[node].extend(types.iter().copied());
            }
        }
        for (_, ty) in &object.fields {
            parents[ty.0 as usize].push(node);
        }
    }
    while let Some(node) = pending.pop() {
        if std::mem::replace(&mut reachable[node], true) {
            continue;
        }
        pending.extend(parents[node].iter().copied());
    }
    reachable[offset..].to_vec()
}

impl Runtime {
    /// A new object of `descriptor` gets a resource identity if it can matter.
    pub fn created(&mut self, descriptor: usize, object: Word) {
        let resources = &mut self.resources;
        if resources.enabled && resources.flags[descriptor] {
            resources.serial += 1;
            // #528: an object with a destroy keeps its identity in its own words; nothing to forget on release.
            if let Some(offset) = resources.inline[descriptor] {
                let serial = resources.serial;
                // SAFETY: the allocation of a descriptor with inline identity has its two words (`object_words`).
                unsafe {
                    *((object + offset as Word) as *mut u64) = serial;
                    *((object + offset as Word + 8) as *mut u64) = OWNER_FREE;
                }
                // `obj.next` copies the source's header: a rehomed cache is the source's, not this object's.
                unsafe { *(object as *mut u64) &= !heap::REHOMED };
                self.heap.mark_identity(object);
                return;
            }
            resources.identities.insert(object, Identity { serial: resources.serial, owner: Owner::Free, cache_rehomed: false, groups: Vec::new() });
            self.heap.mark_identity(object);
            self.heap.watch(object);
        }
    }

    /// Whether a field of a new object of `descriptor` may reach a resource identity (#905): an object that is an
    /// identity, a container that may hold one, or a reference field not yet initialized.
    fn fields_may_reach_identity(&self, descriptor: usize, object: Word) -> bool {
        let layout = &self.heap.layouts().objects[descriptor];
        layout.iter().enumerate().any(|(index, &repr)| {
            let word = self.heap.field(object, index);
            let Some(pointer) = repr.pointer(word) else {
                return repr == Repr::Ref && word == UNBOUND;
            };
            let header = self.heap.header_of(pointer);
            match heap::kind(header) {
                heap::OBJECT => header & heap::IDENTITY != 0,
                heap::ARRAY => header & heap::PACKED == 0 && Repr::from_bits((header >> 9) & 3) == Repr::Ref,
                heap::CLOSURE | heap::PAYLOAD | heap::FRAME => true,
                _ => false,
            }
        })
    }

    /// The address of `object`'s inline identity words (#528), if it keeps them.
    #[inline]
    fn inline_slot(&self, object: Word) -> Option<Word> {
        let header = self.heap.header_of(object);
        if heap::kind(header) == heap::FRAME {
            // A Future's identity lives at the end of its frame (#931); serial 0 means it has none.
            let at = self.loaded.functions[heap::payload(header) as usize].identity_at?;
            let slot = object + at;
            // SAFETY: a frame of a function with `identity_at` has those two words.
            return (unsafe { *(slot as *const u64) } != 0).then_some(slot);
        }
        if heap::kind(header) != heap::OBJECT || header & heap::IDENTITY == 0 {
            return None;
        }
        self.resources.inline[heap::payload(header) as usize].map(|offset| object + offset as Word)
    }

    /// The owner of `object`'s identity, if it has one.
    fn owner_of(&self, object: Word) -> Option<Owner> {
        if let Some(slot) = self.inline_slot(object) {
            // SAFETY: the identity words of a live object.
            return Some(decode_owner(unsafe { *((slot + 8) as *const u64) }));
        }
        self.resources.identities.get(&object).map(|identity| identity.owner)
    }

    fn set_owner(&mut self, object: Word, owner: Owner) {
        if let Some(slot) = self.inline_slot(object) {
            // SAFETY: as in `owner_of`.
            unsafe { *((slot + 8) as *mut u64) = encode_owner(owner) };
            return;
        }
        self.resources.identities.get_mut(&object).expect("identity").owner = owner;
    }

    fn rehomed(&self, object: Word) -> bool {
        if self.inline_slot(object).is_some() {
            return self.heap.header_of(object) & heap::REHOMED != 0;
        }
        self.resources.identities.get(&object).is_some_and(|identity| identity.cache_rehomed)
    }

    fn set_rehomed(&mut self, object: Word) {
        if self.inline_slot(object).is_some() {
            self.heap.set_header_bits(object, heap::REHOMED);
            return;
        }
        self.resources.identities.get_mut(&object).expect("identity").cache_rehomed = true;
    }

    /// An object the program built (`obj.new`, `obj.next`, `variant.new`: machine code's `rt_created`). Tweed #905:
    /// whether it matters is decided per instance, not per type. An object without a destroy of its own is an
    /// identity only so that walks descend into it; one whose fields, as built, reach no identity is a leaf for
    /// its whole life, since its fields never change. A field not yet initialized (escaped construction,
    /// `obj.init`) or a container keeps the old answer; so do builtins and `once` slots (`leaf_capable`), and
    /// objects the runtime makes (groups, driver results), which go through [`Runtime::created`].
    pub fn created_built(&mut self, descriptor: usize, object: Word) {
        if self.resources.enabled && self.resources.flags[descriptor] && self.resources.leaf_capable[descriptor]
            && !self.fields_may_reach_identity(descriptor, object)
        {
            okkama_devtrace::count!("runtime", "identity skipped: fields reach none");
            return;
        }
        self.created(descriptor, object);
    }


    /// A Future must be tracked even before its result exists.
    pub(crate) fn future_created(&mut self, frame: Word) {
        if self.resources.enabled {
            self.resources.serial += 1;
            let function = heap::payload(self.heap.header_of(frame)) as usize;
            if let Some(at) = self.loaded.functions[function].identity_at {
                // Inline, at the end of the frame (#931): no hash-table entry, nothing for the collector to report.
                crate::tasks::set(frame, at, self.resources.serial);
                crate::tasks::set(frame, at + 8, encode_owner(Owner::Free));
            } else {
                // A runtime whose resources were switched on by hand (tests): the side table, as before.
                self.resources.identities.insert(frame, Identity { serial: self.resources.serial, owner: Owner::Free, cache_rehomed: false, groups: Vec::new() });
                self.heap.watch(frame);
            }
        }
    }

    /// #587 (OIR-2 §10.1): the fresh call inputs of a new Future belong to it, in its INPUTS region; inputs that
    /// already have an owner stay borrowed. Made only when an argument reaches a free identity.
    pub(crate) fn own_inputs(&mut self, frame: Word, arguments: &[Word]) {
        if !self.resources.enabled || arguments.is_empty() {
            return;
        }
        let function = self.function_of(frame);
        let mut inputs = UNBOUND;
        for (index, &value) in arguments.iter().enumerate() {
            let Some(param) = self.loaded.program.functions[function].params.get(index) else { break };
            let repr = self.loaded.functions[function].kinds[param.0 as usize].repr();
            let Some(value) = repr.pointer(value) else { continue };
            if !self.may_reach_free(value) {
                continue;
            }
            if inputs == UNBOUND {
                inputs = self.frame_inputs(frame);
            }
            self.region_add(inputs, value, true);
        }
        if inputs != UNBOUND && self.region_len(inputs) == 0 {
            self.set_held(frame, crate::tasks::Held::Inputs, UNBOUND);
        }
    }

    /// Publication owns the outcome until cache cleanup or a proven unique take. What it reaches of the frame's
    /// own inputs moves with it (#587): those inputs are part of the outcome now. An async body also keeps the
    /// input Futures whose caches its outcome borrows, since its other inputs are released at completion.
    pub(crate) fn own_outcome(&mut self, frame: Word, success: bool, value: Word) {
        if !self.resources.enabled {
            return;
        }
        let roots: Vec<Word> = if success {
            let repr = self.heap.layouts().frames[self.function_of(frame)][1];
            repr.pointer(value).into_iter().collect()
        } else {
            self.failure_payloads(value)
        };
        if roots.is_empty() {
            return;
        }
        let inputs = self.held(frame, crate::tasks::Held::Inputs);
        if inputs != UNBOUND && self.region_len(inputs) > 0 {
            let owned = self.frame_region(frame);
            for &root in &roots {
                self.region_transfer(inputs, owned, root);
            }
            if !self.is_driver_frame(frame) {
                let entries: Vec<Word> = (0..self.region_len(inputs)).map(|i| self.region_get(inputs, i)).collect();
                for input in entries {
                    if heap::kind(self.heap.header_of(input)) == heap::FRAME
                        && self.owner_of(input) == Some(Owner::Region(inputs))
                        && self.outcome_reaches_frame(&roots, input)
                    {
                        self.region_transfer(inputs, owned, input);
                    }
                }
            }
        }
        // A returned value carries its pins into the cache (#915).
        if let [root] = roots[..] && self.pins_wait_for(root) {
            let owned = self.frame_region(frame);
            self.claim_pins(owned, root);
        }
        // A plain value (no identity anywhere) needs no region.
        if !roots.iter().any(|&root| self.may_reach_free(root)) {
            return;
        }
        let owned = self.frame_region(frame);
        for root in roots {
            self.region_add(owned, root, true);
        }
    }

    fn is_driver_frame(&self, frame: Word) -> bool {
        self.loaded.functions[self.function_of(frame)].driver
    }

    /// Does an outcome reached from `roots` borrow what Future `frame` owns (its cache, its inputs, or those of
    /// the Futures it owns in turn)? Those ownership edges are hidden from value walks.
    fn outcome_reaches_frame(&mut self, roots: &[Word], frame: Word) -> bool {
        let mut pending = vec![frame];
        let mut seen = HashSet::default();
        while let Some(frame) = pending.pop() {
            if !seen.insert(frame) {
                continue;
            }
            for held in [crate::tasks::Held::Owned, crate::tasks::Held::Inputs] {
                let region = self.held(frame, held);
                if region == UNBOUND || self.region_len(region) == 0 {
                    continue;
                }
                okkama_devtrace::count!("runtime", "walk from: outcome reaches input");
                if !self.resource_graph(roots, Some(region)).is_empty() {
                    return true;
                }
                for index in 0..self.region_len(region) {
                    let child = self.region_get(region, index);
                    if heap::kind(self.heap.header_of(child)) == heap::FRAME
                        && self.owner_of(child) == Some(Owner::Region(region))
                    {
                        pending.push(child);
                    }
                }
            }
        }
        false
    }

    /// A frame's private region, made on first use.
    fn frame_region(&mut self, frame: Word) -> Word {
        let owned = self.held(frame, crate::tasks::Held::Owned);
        if owned != UNBOUND {
            return owned;
        }
        let owned = self.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
        self.set_held(frame, crate::tasks::Held::Owned, owned);
        owned
    }

    /// A frame's input region (#587), made on first use.
    fn frame_inputs(&mut self, frame: Word) -> Word {
        let inputs = self.held(frame, crate::tasks::Held::Inputs);
        if inputs != UNBOUND {
            return inputs;
        }
        let inputs = self.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
        self.set_held(frame, crate::tasks::Held::Inputs, inputs);
        inputs
    }

    /// #845: a driver takes `input`, a fresh Future it made itself (a collector's sequence `item`): its INPUTS own
    /// it until the driver closes (a driver keeps its inputs past settlement, #587), so its outcome is released
    /// with the driver's owner.
    pub(crate) fn driver_input(&mut self, frame: Word, input: Word) {
        if !self.resources.enabled || heap::kind(self.heap.header_of(input)) != heap::FRAME
            || self.owner_of(input) != Some(Owner::Free)
        {
            return;
        }
        let inputs = self.frame_inputs(frame);
        self.set_owner(input, Owner::Region(inputs));
        self.region_push(inputs, input);
    }

    /// #587: a new capability of `channel` (the channel or an endpoint, `descriptor`): with an identity it holds
    /// the channel's message packet open.
    pub(crate) fn channel_holder(&mut self, channel: &Rc<RefCell<crate::primitives::Channel>>, descriptor: usize, object: Word) {
        self.created(descriptor, object);
        if self.resources.enabled && self.heap.header_of(object) & heap::IDENTITY != 0 {
            channel.borrow_mut().holders += 1;
        }
    }

    /// #587 (OIR-2 §7.3): move what region `from` owns of `value` into the region `to` makes (on first need).
    /// Values owned elsewhere stay borrowed.
    fn hand_over(&mut self, from: Word, value: Word, to: impl FnOnce(&mut Self) -> Word) {
        if from == UNBOUND || self.region_len(from) == 0 {
            return;
        }
        okkama_devtrace::count!("runtime", "walk from: channel hand-over");
        let moved: Vec<Word> = self
            .resource_graph(&[value], Some(from))
            .into_iter()
            .filter(|&object| self.owner_of(object) == Some(Owner::Region(from)))
            .collect();
        if moved.is_empty() {
            return;
        }
        let to = to(self);
        for object in moved {
            self.set_owner(object, Owner::Region(to));
            self.region_push(to, object);
            self.dev_owned(object, to);
        }
    }

    /// A send is accepted into the buffer: the channel's packet takes the fresh message from the sender's INPUTS.
    pub(crate) fn message_accept(&mut self, sender: Word, channel: &Rc<RefCell<crate::primitives::Channel>>, value: Word, repr: Repr) {
        let Some(value) = repr.pointer(value).filter(|_| self.resources.enabled) else { return };
        let packet = |runtime: &mut Self| {
            let packet = channel.borrow().packet;
            if packet != UNBOUND {
                return packet;
            }
            let packet = runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
            channel.borrow_mut().packet = packet;
            packet
        };
        let inputs = self.held(sender, crate::tasks::Held::Inputs);
        self.hand_over(inputs, value, packet);
        // #915: what has another owner is pinned by the packet; what has none, the packet owns.
        let identities = self.message_identities(value);
        if identities.is_empty() {
            return;
        }
        let packet = packet(self);
        self.hold_message(&identities, packet);
    }

    /// #915: `holder` takes the free identities of a message and pins those another owner keeps.
    fn hold_message(&mut self, identities: &[Word], holder: Word) {
        for &identity in identities {
            match self.owner_of(identity) {
                Some(Owner::Free) => {
                    self.set_owner(identity, Owner::Region(holder));
                    self.region_push(holder, identity);
                    self.dev_owned(identity, holder);
                }
                Some(Owner::Region(owner)) if owner == holder => {}
                Some(Owner::Dead) | None => {}
                Some(_) => self.pin(identity, holder),
            }
        }
    }

    /// A message reaches `receiver`: from the channel's packet (buffered) or straight from a sender's INPUTS
    /// (rendezvous), into the receiver's INPUTS; its outcome takes it from there when it settles.
    pub(crate) fn message_deliver(&mut self, from: Word, receiver: Word, value: Word, repr: Repr) {
        let Some(value) = repr.pointer(value).filter(|_| self.resources.enabled) else { return };
        self.hand_over(from, value, |runtime| runtime.frame_inputs(receiver));
        // #915: the receiving Future's cache holds the pins now (from the packet, or new ones in a rendezvous).
        let identities = self.message_identities(value);
        if identities.is_empty() {
            return;
        }
        let cache = self.frame_region(receiver);
        for &identity in &identities {
            if from != UNBOUND && self.resources.pins.get(&identity).is_some_and(|holders| holders.contains(&from)) {
                self.unpin(identity, from);
            }
        }
        let held: Vec<Word> = identities
            .into_iter()
            .filter(|&identity| !matches!(self.owner_of(identity), Some(Owner::Region(owner)) if owner == self.held(receiver, crate::tasks::Held::Inputs)))
            .collect();
        self.hold_message(&held, cache);
    }

    /// #915: every identity `value` reaches, whatever its owner (a dead one excluded).
    fn message_identities(&mut self, value: Word) -> Vec<Word> {
        let mut found = Vec::new();
        let mut seen: Vec<Word> = Vec::new();
        let mut pending = vec![(value, Repr::Ref)];
        while let Some((word, repr)) = pending.pop() {
            let Some(object) = repr.pointer(word) else { continue };
            let header = self.heap.header_of(object);
            match heap::kind(header) {
                heap::OBJECT if !self.resources.flags[heap::payload(header) as usize] => continue,
                heap::ARRAY if header & heap::PACKED != 0 || Repr::from_bits((header >> 9) & 3) != Repr::Ref => continue,
                heap::OBJECT | heap::ARRAY | heap::CLOSURE | heap::PAYLOAD | heap::FRAME => {}
                _ => continue,
            }
            if !self.heap.visit(object) {
                continue;
            }
            seen.push(object);
            if self.owner_of(object).is_some_and(|owner| owner != Owner::Dead) {
                found.push(object);
            }
            match heap::kind(header) {
                heap::OBJECT => {
                    let layout = &self.heap.layouts().objects[heap::payload(header) as usize];
                    for (index, &field) in layout.iter().enumerate() {
                        pending.push((self.heap.field(object, index), field));
                    }
                }
                heap::ARRAY => {
                    let repr = Repr::from_bits((header >> 9) & 3);
                    pending.extend(self.heap.elements(object).iter().map(|&e| (e, repr)));
                }
                heap::CLOSURE => {
                    let layout = &self.heap.layouts().closures[heap::payload(header) as usize];
                    for (index, &field) in layout.iter().enumerate() {
                        pending.push((self.heap.field(object, index), field));
                    }
                }
                heap::PAYLOAD => {
                    if let Some(map) = self.heap.payload::<crate::ops::MapValue>(object) {
                        pending.extend(map.entries.values().map(|(_, v)| (*v, map.value)));
                    } else if let Some(boxed) = self.heap.payload::<crate::ops::AnyBox>(object) {
                        pending.push((boxed.value, boxed.repr));
                    }
                }
                _ => {}
            }
        }
        for object in seen {
            self.heap.unvisit(object);
        }
        found
    }

    /// #915: `holder` pins `identity`.
    fn pin(&mut self, identity: Word, holder: Word) {
        // The holder lists it, as a tracked entry: a region with entries is never skipped at its exit (machine code
        // leaves an empty region without calling the runtime), so its pins are always released.
        self.region_push(holder, identity);
        self.resources.pins.entry(identity).or_default().push(holder);
        self.resources.pinned_by.entry(holder).or_default().push(identity);
        self.heap.set_header_bits(identity, heap::PINNED);
    }

    /// Drop one pin of `holder` on `identity`.
    fn unpin(&mut self, identity: Word, holder: Word) {
        if let Some(holders) = self.resources.pins.get_mut(&identity)
            && let Some(at) = holders.iter().position(|&h| h == holder)
        {
            holders.swap_remove(at);
            if holders.is_empty() {
                self.resources.pins.remove(&identity);
                self.heap.clear_header_bits(identity, heap::PINNED);
            }
        }
        if let Some(identities) = self.resources.pinned_by.get_mut(&holder)
            && let Some(at) = identities.iter().position(|&i| i == identity)
        {
            identities.swap_remove(at);
            if identities.is_empty() {
                self.resources.pinned_by.remove(&holder);
            }
        }
    }

    /// `holder` ends: its pins go, and the identities they pinned (repeats kept).
    fn release_pins(&mut self, holder: Word) -> Vec<Word> {
        if self.resources.pinned_by.is_empty() {
            return Vec::new();
        }
        let Some(identities) = self.resources.pinned_by.get(&holder).cloned() else { return Vec::new() };
        for &identity in &identities {
            self.unpin(identity, holder);
        }
        identities
    }

    /// A closing `region` that keeps `kept`: its pins on what `kept` reaches wait for the next region that takes
    /// `kept` (a returned value carries its pins); the rest go.
    fn release_pins_keeping(&mut self, region: Word, kept: &[Word]) {
        let released = self.release_pins(region);
        if released.is_empty() {
            return;
        }
        if let [value] = kept {
            let reached = self.message_identities(*value);
            let carried: Vec<Word> = released.into_iter().filter(|identity| reached.contains(identity)).collect();
            if !carried.is_empty() {
                self.resources.pending_pins = Some((*value, carried));
            }
        }
    }

    /// The pins waiting for `value` (a taken outcome, a returned value) move to `region`, which takes it.
    pub(crate) fn claim_pins(&mut self, region: Word, value: Word) {
        let Some((waiting, _)) = &self.resources.pending_pins else { return };
        if *waiting != value {
            return;
        }
        let (_, identities) = self.resources.pending_pins.take().expect("checked");
        for identity in identities {
            match self.owner_of(identity) {
                Some(Owner::Dead) | None => {}
                Some(Owner::Region(owner)) if owner == region => {}
                _ => self.pin(identity, region),
            }
        }
    }

    /// Whether pins wait for `value` (a lazily made region must then exist).
    pub(crate) fn pins_wait_for(&self, value: Word) -> bool {
        self.resources.pending_pins.as_ref().is_some_and(|(waiting, _)| *waiting == value)
    }

    /// The owner of pinned `identity` closes: ownership passes to a holder of a pin, which then no longer pins it.
    /// None when nothing pins it.
    fn hand_to_pin_holder(&mut self, identity: Word) -> Option<Word> {
        if self.heap.header_of(identity) & heap::PINNED == 0 {
            return None;
        }
        let holder = *self.resources.pins.get(&identity)?.first()?;
        self.unpin(identity, holder);
        okkama_devtrace::count!("runtime", "pinned identity handed to its holder");
        self.set_owner(identity, Owner::Region(holder));
        self.region_push(holder, identity);
        self.dev_owned(identity, holder);
        Some(holder)
    }

    /// GC roots of the pin tables.
    pub(crate) fn pin_roots(&self) -> Vec<Word> {
        let mut roots: Vec<Word> = self.resources.pins.keys().copied().collect();
        roots.extend(self.resources.pinned_by.keys().copied());
        if let Some((value, identities)) = &self.resources.pending_pins {
            roots.push(*value);
            roots.extend(identities);
        }
        roots
    }

    /// A capability of a channel dies: the packet, when it was the last one, to close with it.
    fn channel_released(&mut self, object: Word) -> Option<Word> {
        let Some(crate::primitives::State::Channel(channel)) = self.states.get(&object) else { return None };
        let mut channel = channel.borrow_mut();
        channel.holders = channel.holders.saturating_sub(1);
        if channel.holders > 0 || channel.packet == UNBOUND {
            return None;
        }
        Some(std::mem::replace(&mut channel.packet, UNBOUND))
    }

    /// Group registration admits fresh Futures without taking external owners.
    pub(crate) fn group_own(&mut self, group: Word, future: Word) {
        if !self.resources.enabled { return; }
        let owned = match self.group_mut(group).expect("group").resources {
            Some(owned) => owned,
            None => {
                let owned = self.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
                self.group_mut(group).expect("group").resources = Some(owned);
                owned
            }
        };
        // A Future's identity is inline (#931); the side entry keeps only the groups that list it.
        if !self.resources.identities.contains_key(&future) {
            let serial = self.serial_of(future);
            self.resources.identities.insert(future, Identity { serial, owner: Owner::Free, cache_rehomed: false, groups: Vec::new() });
            self.heap.watch(future);
        }
        let identity = self.resources.identities.get_mut(&future).expect("group entry");
        if !identity.groups.contains(&owned) { identity.groups.push(owned); }
        // Registration adds a lifetime rather than stealing one: the group lists the Future, and the scope that
        // made it, bound or a temporary its region tracks, keeps it. When that scope closes, it hands the Future
        // to the group with what the Future borrows from the scope (`region_close_until`), so a child keeps the
        // values it borrows (#914; adopting a temporary here left them behind, closed under a running child).
        self.region_push(owned, future);
    }

    fn queue_private(&self, object: Word, groups: &mut Vec<Word>, futures: &mut Vec<Word>) {
        if heap::kind(self.heap.header_of(object)) == heap::FRAME {
            futures.push(object);
        } else if matches!(self.states.get(&object), Some(crate::primitives::State::Group(_) | crate::primitives::State::Channel(_))) {
            groups.push(object);
        }
    }

    /// A Future's private regions (its cached outcome, its inputs) or a group's packet of registered Futures.
    fn private_regions(&self, object: Word) -> [Option<Word>; 2] {
        if heap::kind(self.heap.header_of(object)) == heap::FRAME {
            let slot = |held| Some(self.held(object, held)).filter(|&region| region != UNBOUND);
            [slot(crate::tasks::Held::Owned), slot(crate::tasks::Held::Inputs)]
        } else if let Some(crate::primitives::State::Group(group)) = self.states.get(&object) {
            [group.resources, None]
        } else { [None, None] }
    }

    /// Release only this cache's actual package. External borrows retain their
    /// owners. No allocation, safepoint or user code occurs before region.track
    /// admits the successfully delivered result in the caller.
    pub(crate) fn take_outcome(&mut self, frame: Word) {
        let owned = self.held(frame, crate::tasks::Held::Owned);
        if owned == UNBOUND {
            return;
        }
        // #915: the cache's pins go with the taken value to the region that takes it.
        let pinned = self.release_pins(owned);
        if !pinned.is_empty() {
            self.resources.pending_pins = Some((crate::tasks::get(frame, crate::tasks::RESULT), pinned));
        }
        for index in 0..self.region_len(owned) {
            let object = self.region_get(owned, index);
            if self.owner_of(object) == Some(Owner::Region(owned)) {
                self.set_owner(object, Owner::Free);
                let serial = self.serial_of(object);
                self.freed(serial);
            }
        }
        self.region_clear(owned);
        self.set_held(frame, crate::tasks::Held::Owned, UNBOUND);
    }

    /// Free or `via`-owned identities reachable from `roots`, oldest first.
    fn resource_graph(&mut self, roots: &[Word], via: Option<Word>) -> Vec<Word> {
        self.resource_graph_into(roots, via, None)
    }

    /// `resource_graph`, not descending into identities already in `known`
    /// (a region): objects are immutable, so their graph is already there.
    ///
    /// Containers are immutable too, so what a container reaches is at most
    /// as old as the container. One whose walk meets no free identity (only
    /// identities owned elsewhere, dead or plain) is summarized (#9): later
    /// walks skip it until an identity at least as old becomes free again,
    /// and a `via` walk skips it while every entry of `via` is newer. A big
    /// structure held by an open region then costs one walk, not one per
    /// value that points into it.
    fn resource_graph_into(&mut self, roots: &[Word], via: Option<Word>, known: Option<Word>) -> Vec<Word> {
        if !self.resources.enabled {
            return Vec::new();
        }
        // Scratch storage reused across calls (regions close on every scope
        // exit); visited objects carry a header mark until the walk ends.
        let (mut pending, mut seen, mut open) = std::mem::take(&mut self.resources.scratch);
        pending.clear();
        seen.clear();
        open.clear();
        pending.extend(roots.iter().map(|&w| (w, Repr::Ref)));
        let mut found = Vec::new();
        // `events` counts what keeps a container from a summary: a free or
        // `via` identity, and anything met twice (its first meeting may have
        // been one).
        let (mut steps, mut events) = (0, 0);
        let mut oldest = None;
        loop {
            while let Some(&walked) = open.last()
                && pending.len() <= walked.base
            {
                open.pop();
                if walked.events == events && steps - walked.steps >= SUMMARY_STEPS {
                    self.summarize(walked.container);
                }
            }
            let Some((word, repr)) = pending.pop() else {
                break;
            };
            steps += 1;
            let Some(object) = repr.pointer(word) else {
                continue;
            };
            let header = self.heap.header_of(object);
            // Leaves cannot reach an identity: skip them before the visited set.
            match heap::kind(header) {
                heap::OBJECT if !self.resources.flags[heap::payload(header) as usize] => continue,
                heap::ARRAY if header & heap::PACKED != 0 || Repr::from_bits((header >> 9) & 3) != Repr::Ref => continue,
                heap::OBJECT => {
                    if header & heap::IDENTITY == 0 {
                        continue;
                    }
                    let Some(owner) = self.owner_of(object) else {
                        continue;
                    };
                    if owner != Owner::Free && Some(owner) != via.map(Owner::Region) {
                        continue;
                    }
                    events += 1;
                    if known.is_some_and(|region| self.region_contains(region, object)) || !self.heap.visit(object) {
                        continue;
                    }
                    seen.push(object);
                    let layout = &self.heap.layouts().objects[heap::payload(header) as usize];
                    for (index, &field) in layout.iter().enumerate().rev() {
                        pending.push((self.heap.field(object, index), field));
                    }
                    found.push(object);
                    continue;
                }
                heap::ARRAY | heap::CLOSURE | heap::PAYLOAD | heap::FRAME => {}
                _ => continue,
            }
            if header & heap::SUMMARY != 0 && self.summary_holds(object, via, &mut oldest) {
                continue;
            }
            if !self.heap.visit(object) {
                events += 1;
                continue;
            }
            seen.push(object);
            let base = pending.len();
            match heap::kind(header) {
                heap::FRAME => {
                    // The outcome has a private owner. Only retained inputs
                    // follow the Future through lexical ownership transfers.
                    events += 1;
                    let Some(owner) = self.owner_of(object) else { continue };
                    if owner == Owner::Free || Some(owner) == via.map(Owner::Region) {
                        found.push(object);
                    }
                    // Retained inputs keep the #578 transfer rules even when
                    // the Future itself is already owned through another edge.
                    let function = heap::payload(header) as usize;
                    let source = &self.loaded.program.functions[function];
                    let layout = &self.heap.layouts().frames[function];
                    // The retained arguments end before the inline identity's two words (#931).
                    let end = layout.len() - if self.loaded.functions[function].identity_at.is_some() { 2 } else { 0 };
                    let retained = end - source.params.len();
                    for index in (retained..end).rev() {
                        pending.push((self.heap.field(object, index), layout[index]));
                    }
                    continue;
                }
                _ if self.resources.structure.contains_key(&object) => {
                    okkama_devtrace::count!("runtime", "structure index used");
                    pending.extend(self.resources.structure[&object].iter().rev().copied());
                }
                heap::ARRAY => {
                    let repr = Repr::from_bits((header >> 9) & 3);
                    let elements = self.heap.elements(object);
                    // A buffer's slots past its frontier are still written: no index for it.
                    if header & heap::BUFFER != 0 {
                        pending.extend(elements.iter().rev().map(|&e| (e, repr)));
                    } else {
                        let children = elements.iter().map(|&e| (e, repr));
                        let index = self.structure_of(children);
                        pending.extend(index.iter().rev().copied());
                        self.remember_structure(object, index);
                    }
                }
                heap::CLOSURE => {
                    let layout = &self.heap.layouts().closures[heap::payload(header) as usize];
                    let children = layout.iter().enumerate().map(|(index, &field)| (self.heap.field(object, index), field));
                    let index = self.structure_of(children.collect::<Vec<_>>().into_iter());
                    pending.extend(index.iter().rev().copied());
                    self.remember_structure(object, index);
                }
                _ => {
                    let children: Vec<(Word, Repr)> = if let Some(map) = self.heap.payload::<crate::ops::MapValue>(object) {
                        map.entries.values().map(|(_, v)| (*v, map.value)).collect()
                    } else if let Some(boxed) = self.heap.payload::<crate::ops::AnyBox>(object) {
                        vec![(boxed.value, boxed.repr)]
                    } else {
                        // Other payloads (region entries, waiters) change in
                        // place: never summarized.
                        continue;
                    };
                    let index = self.structure_of(children.into_iter());
                    pending.extend(index.iter().rev().copied());
                    self.remember_structure(object, index);
                }
            }
            open.push(Walked { container: object, base, steps, events });
        }
        for &object in &seen {
            self.heap.unvisit(object);
        }
        self.resources.scratch = (pending, seen, open);
        okkama_devtrace::count!("runtime", "resource graph walks");
        okkama_devtrace::count!("runtime", "resource graph steps", steps);
        found.sort_unstable_by_key(|&o| self.serial_of(o));
        found
    }

    /// Remember that `container` reaches no free identity now.
    fn summarize(&mut self, container: Word) {
        let serial = self.resources.serial;
        self.resources.summaries.insert(container, serial);
        self.resources.summarized = self.resources.summarized.max(serial);
        self.heap.summarized(container, true);
        self.heap.watch(container);
    }

    /// Late edges (#907, docs/map/ownership.md): an Atomic only names what it holds, frames are never summarized,
    /// in-place payloads are never indexed, an object with an unbound reference field gets an identity, and a
    /// `once` slot cannot keep a resource; so a summary, once true, stays true until `freed` drops them all.
    ///
    /// #529: a container made from parts (`arr.slice`, `arr.concat`, `map.new`, `map.merge`) reaches what they
    /// reach and nothing else. When a part is a summarized container and no part may reach a free identity, the
    /// result is summarized at birth: no walk ever needs to look inside it. A summary is undone for all at once
    /// (`freed`), so it stays as true as its parts'.
    pub(crate) fn inherit_summary(&mut self, result: Word, parts: &[(Word, Repr)]) {
        if !self.resources.enabled {
            return;
        }
        let mut summarized = false;
        for &(word, repr) in parts {
            let Some(pointer) = repr.pointer(word) else { continue };
            let header = self.heap.header_of(pointer);
            match heap::kind(header) {
                heap::OBJECT => {
                    if header & heap::IDENTITY != 0 && self.owner_of(pointer) == Some(Owner::Free) {
                        return;
                    }
                }
                heap::ARRAY if header & heap::PACKED != 0 || Repr::from_bits((header >> 9) & 3) != Repr::Ref => {}
                heap::ARRAY | heap::PAYLOAD | heap::CLOSURE | heap::FRAME => {
                    if header & heap::SUMMARY == 0 {
                        return;
                    }
                    summarized = true;
                }
                _ => {}
            }
        }
        let header = self.heap.header_of(result);
        if summarized && header & heap::SUMMARY == 0 && matches!(heap::kind(header), heap::ARRAY | heap::PAYLOAD) {
            okkama_devtrace::count!("runtime", "summary inherited");
            self.summarize(result);
        }
    }

    /// May a walk (through `via`, if any) skip `container`? `oldest` caches
    /// the oldest entry of `via` for the rest of the walk.
    fn summary_holds(&self, container: Word, via: Option<Word>, oldest: &mut Option<u64>) -> bool {
        let Some(&serial) = self.resources.summaries.get(&container) else {
            return false;
        };
        let Some(region) = via else {
            return true;
        };
        let oldest = *oldest.get_or_insert_with(|| self.region_oldest(region));
        serial < oldest
    }

    /// An identity no newer than a summary becomes free: the summaries may
    /// reach it now.
    fn freed(&mut self, serial: u64) {
        if serial > self.resources.summarized {
            return;
        }
        okkama_devtrace::count!("runtime", "summaries dropped (freed)", self.resources.summaries.len());
        for (container, _) in self.resources.summaries.drain() {
            self.heap.summarized(container, false);
        }
        self.resources.summarized = 0;
    }

    /// #909 measurement: `object` is now owned by `region`. The first owner is its birth region; any later
    /// owner is a move.
    fn dev_owned(&mut self, object: Word, region: Word) {
        static GATE: okkama_devtrace::Gate = okkama_devtrace::Gate::new("runtime");
        if !GATE.on() {
            return;
        }
        self.resources.origins.entry(object).and_modify(|origin| origin.1 |= origin.0 != region)
            .or_insert((region, false));
    }

    /// #909 measurement: `object` is destroyed by `region`'s close; was it local all its life?
    fn dev_destroyed(&mut self, object: Word, region: Word) {
        static GATE: okkama_devtrace::Gate = okkama_devtrace::Gate::new("runtime");
        if !GATE.on() {
            return;
        }
        let local = self.resources.origins.get(&object).is_some_and(|&(born, moved)| born == region && !moved);
        okkama_devtrace::count("runtime", if local { "destroyed: never left its first owner" }
            else { "destroyed: moved (returned, stored, captured, handed on)" }, 1);
    }

    /// Keep a container's index until the collector reclaims the container (`forget`): a watched address
    /// reports its release, so a reused address never inherits an old index.
    fn remember_structure(&mut self, container: Word, index: Box<[(Word, Repr)]>) {
        self.resources.structure.insert(container, index);
        self.heap.watch(container);
    }

    /// Tweed #906: the children of an immutable container that may hold an identity, in order. A leaf (a scalar, a
    /// string, an object that is not an identity, a container whose index is empty) is left out for good.
    fn structure_of(&self, children: impl Iterator<Item = (Word, Repr)>) -> Box<[(Word, Repr)]> {
        okkama_devtrace::count!("runtime", "structure index built");
        children.filter(|&(word, repr)| {
            let Some(pointer) = repr.pointer(word) else { return false };
            let header = self.heap.header_of(pointer);
            match heap::kind(header) {
                heap::OBJECT => header & heap::IDENTITY != 0,
                heap::ARRAY => header & heap::PACKED == 0 && Repr::from_bits((header >> 9) & 3) == Repr::Ref
                    && self.resources.structure.get(&pointer).is_none_or(|index| !index.is_empty()),
                heap::CLOSURE | heap::PAYLOAD | heap::FRAME => true,
                _ => false,
            }
        }).collect()
    }

    /// Can `value` reach a free identity? False is certain (the common case:
    /// a leaf, an object without identity, or one owned elsewhere).
    fn may_reach_free(&self, value: Word) -> bool {
        if !self.resources.enabled || value == UNBOUND || value & 1 == 1 {
            return false;
        }
        let header = self.heap.header_of(value);
        match heap::kind(header) {
            heap::OBJECT => {
                header & heap::IDENTITY != 0 && self.owner_of(value) == Some(Owner::Free)
            }
            heap::ARRAY => header & heap::PACKED == 0 && Repr::from_bits((header >> 9) & 3) == Repr::Ref,
            heap::CLOSURE | heap::PAYLOAD | heap::FRAME => true,
            _ => false,
        }
    }

    /// `value` if it is a free identity none of whose fields may reach a free
    /// identity (`may_reach_free(value)` holds): the walk would find just it.
    fn lone_free(&self, value: Word) -> Option<Word> {
        let header = self.heap.header_of(value);
        if heap::kind(header) != heap::OBJECT {
            return None;
        }
        let layout = &self.heap.layouts().objects[heap::payload(header) as usize];
        for (index, &field) in layout.iter().enumerate() {
            if field.pointer(self.heap.field(value, index)).is_some_and(|pointer| self.may_reach_free(pointer)) {
                return None;
            }
        }
        Some(value)
    }

    /// What a walk from the Future `value` would find, when its retained inputs reach no free identity: the
    /// Future itself while it is free, else nothing. `None` when the walk is needed (an input may reach one, or
    /// the frame carries a summary). A walk from a frame visits only it and its retained inputs (#932).
    fn lone_frame(&self, value: Word) -> Option<Option<Word>> {
        let header = self.heap.header_of(value);
        if heap::kind(header) != heap::FRAME || header & heap::SUMMARY != 0 {
            return None;
        }
        let function = heap::payload(header) as usize;
        let layout = &self.heap.layouts().frames[function];
        let end = layout.len() - if self.loaded.functions[function].identity_at.is_some() { 2 } else { 0 };
        let retained = end - self.loaded.program.functions[function].params.len();
        for (index, repr) in layout.iter().enumerate().take(end).skip(retained) {
            if repr.pointer(self.heap.field(value, index)).is_some_and(|pointer| self.may_reach_free(pointer)) {
                return None;
            }
        }
        Some((self.owner_of(value) == Some(Owner::Free)).then_some(value))
    }

    /// Destroy order is reverse creation: sort the region's entries by serial. A region that fits its inline
    /// slots (the common close) sorts on the stack and rewrites the slots in place (#909 step 2a, #932).
    fn region_sort(&mut self, region: Word) {
        let length = self.region_len(region);
        if length < 2 {
            return;
        }
        if length <= REGION_SLOTS {
            let mut entries = [UNBOUND; REGION_SLOTS];
            for (index, entry) in entries.iter_mut().enumerate().take(length) {
                *entry = self.region_get(region, index);
            }
            entries[..length].sort_unstable_by_key(|object| self.serial_of(*object));
            // The same entries in new places: the slots are rewritten, no membership check (#932).
            for (index, &object) in entries[..length].iter().enumerate() {
                self.heap.set_field(region, index + 1, object);
            }
        } else {
            let mut entries: Vec<Word> = (0..length).map(|i| self.region_get(region, i)).collect();
            entries.sort_unstable_by_key(|object| self.serial_of(*object));
            self.region_clear(region);
            for object in entries {
                self.region_push(region, object);
            }
        }
    }

    fn region_len(&self, region: Word) -> usize {
        let inline = (1..=REGION_SLOTS).take_while(|&i| self.heap.field(region, i) != UNBOUND).count();
        if inline < REGION_SLOTS {
            return inline;
        }
        match self.heap.field(region, 0) {
            UNBOUND => inline,
            overflow => inline + self.heap.payload::<RegionValue>(overflow).expect("region").entries.len(),
        }
    }

    fn region_get(&self, region: Word, index: usize) -> Word {
        if index < REGION_SLOTS {
            return self.heap.field(region, index + 1);
        }
        let overflow = self.heap.field(region, 0);
        self.heap.payload::<RegionValue>(overflow).expect("region").entries[index - REGION_SLOTS]
    }

    /// Scan the region's entries from `index` down until work needs the whole runtime again. Every entry is
    /// read afresh: nested cleanup can mutate regions and collection can reclaim their payloads.
    fn region_next_action(&mut self, region: Word, index: &mut usize, kept: &HashSet<Word>) -> Option<CloseAction> {
        while *index > 0 {
            *index -= 1;
            let object = self.region_get(region, *index);
            let Some(owner) = self.owner_of(object) else { continue };
            if kept.contains(&object) {
                if owner == Owner::Region(region) {
                    self.set_owner(object, Owner::Free);
                    return Some(CloseAction::Freed(self.serial_of(object)));
                }
                continue;
            }
            if owner != Owner::Free && owner != Owner::Region(region) {
                continue;
            }
            // #915: a pinned identity outlives this owner with its pin's holder.
            if self.hand_to_pin_holder(object).is_some() {
                continue;
            }
            self.set_owner(object, Owner::Dead);
            return Some(CloseAction::Destroy(object, self.serial_of(object)));
        }
        None
    }

    fn region_contains(&self, region: Word, object: Word) -> bool {
        for slot in 1..=REGION_SLOTS {
            match self.heap.field(region, slot) {
                UNBOUND => return false,
                entry if entry == object => return true,
                _ => {}
            }
        }
        match self.heap.field(region, 0) {
            UNBOUND => false,
            overflow => self.heap.payload::<RegionValue>(overflow).expect("region").members.contains(&object),
        }
    }

    fn region_push(&mut self, region: Word, object: Word) {
        if self.region_contains(region, object) {
            return;
        }
        let length = self.region_len(region);
        if length < REGION_SLOTS {
            self.heap.set_field(region, length + 1, object);
            return;
        }
        if self.heap.field(region, 0) == UNBOUND {
            let overflow = self.heap.allocate_payload(Box::new(RegionValue::default()));
            self.heap.set_field(region, 0, overflow);
        }
        let serial = self.serial_of(object);
        let overflow = self.heap.field(region, 0);
        let overflow = self.heap.payload_mut::<RegionValue>(overflow).expect("region");
        overflow.entries.push(object);
        overflow.members.insert(object);
        overflow.oldest = Some(overflow.oldest.map_or(serial, |oldest| oldest.min(serial)));
    }

    fn region_clear(&mut self, region: Word) {
        for slot in 1..=REGION_SLOTS {
            self.heap.set_field(region, slot, UNBOUND);
        }
        let overflow = self.heap.field(region, 0);
        if overflow != UNBOUND {
            let overflow = self.heap.payload_mut::<RegionValue>(overflow).expect("region");
            overflow.entries.clear();
            overflow.members.clear();
            overflow.oldest = None;
        }
    }

    /// Serial of an identity; 0 (older than anything) for none.
    fn serial_of(&self, object: Word) -> u64 {
        if let Some(slot) = self.inline_slot(object) {
            // SAFETY: as in `owner_of`.
            return unsafe { *(slot as *const u64) };
        }
        self.resources.identities.get(&object).map_or(0, |identity| identity.serial)
    }

    /// The oldest serial among the region's entries (`u64::MAX`: none).
    fn region_oldest(&self, region: Word) -> u64 {
        let inline = (1..=REGION_SLOTS)
            .map(|slot| self.heap.field(region, slot))
            .take_while(|&entry| entry != UNBOUND)
            .map(|entry| self.serial_of(entry))
            .min()
            .unwrap_or(u64::MAX);
        match self.heap.field(region, 0) {
            UNBOUND => inline,
            overflow => {
                let overflow = self.heap.payload::<RegionValue>(overflow).expect("region");
                overflow.oldest.map_or(inline, |oldest| oldest.min(inline))
            }
        }
    }

    /// #932: a consumed Future with no ownership region left (a driver's outcome taken, nothing kept): a region
    /// closing it would release nothing.
    fn spent_future(&self, value: Word) -> bool {
        value != UNBOUND
            && value & 1 == 0
            && heap::kind(self.heap.header_of(value)) == heap::FRAME
            && crate::tasks::get(value, crate::tasks::STATE) == crate::tasks::CONSUMED
            && self.held(value, crate::tasks::Held::Owned) == UNBOUND
            && self.held(value, crate::tasks::Held::Inputs) == UNBOUND
    }

    pub(crate) fn region_add(&mut self, region: Word, value: Word, own: bool) {
        if self.resources.pending_pins.is_some() {
            self.claim_pins(region, value);
        }
        if !self.may_reach_free(value) {
            return;
        }
        if self.spent_future(value) {
            okkama_devtrace::count!("runtime", "region add skips a spent Future");
            return;
        }
        // The common add, a cursor or a constructor's result, is one free
        // identity whose fields reach no other: no walk.
        if let Some(object) = self.lone_free(value) {
            if own || self.rehomed(object) {
                self.set_owner(object, Owner::Region(region));
                self.dev_owned(object, region);
            } else if self.region_contains(region, object) {
                return;
            }
            self.region_push(region, object);
            return;
        }
        // A Future whose retained inputs reach no free identity: the walk would find at most the Future.
        if let Some(found) = self.lone_frame(value) {
            if let Some(frame) = found {
                if own || self.rehomed(frame) {
                    self.set_owner(frame, Owner::Region(region));
                    self.dev_owned(frame, region);
                }
                self.region_push(region, frame);
            }
            return;
        }
        // Tracking (not owning) skips what the region already holds; adopting
        // must still take ownership of tracked objects.
        let known = if own { None } else { Some(region) };
        okkama_devtrace::count!("runtime", "walk from: region add");
        for object in self.resource_graph_into(&[value], None, known) {
            if own || self.rehomed(object) {
                self.set_owner(object, Owner::Region(region));
                self.dev_owned(object, region);
            }
            self.region_push(region, object);
        }
        // #529: an adopting walk (from no region) took every free identity the value reaches, so a reference
        // array reaches none now. A big one is summarized at once (the walk's own size rule), so the slices and
        // merges later made from it inherit the summary instead of being walked.
        let header = self.heap.header_of(value);
        if own
            && heap::kind(header) == heap::ARRAY
            && header & (heap::SUMMARY | heap::PACKED) == 0
            && Repr::from_bits((header >> 9) & 3) == Repr::Ref
            && self.heap.array_length(value) >= SUMMARY_STEPS
            // As the walk itself never summarizes over a Future (its state moves on), neither does this.
            && !self.heap.elements(value).iter().any(|&element| {
                element != UNBOUND && element & 1 == 0 && heap::kind(self.heap.header_of(element)) == heap::FRAME
            })
        {
            self.summarize(value);
        }
    }

    fn region_transfer(&mut self, from: Word, to: Word, value: Word) {
        if self.region_len(from) == 0 {
            return;
        }
        okkama_devtrace::count!("runtime", "walk from: region transfer");
        for object in self.resource_graph(&[value], Some(from)) {
            if self.owner_of(object) == Some(Owner::Region(from)) {
                self.set_owner(object, Owner::Region(to));
                self.region_push(to, object);
                self.dev_owned(object, to);
            }
        }
    }

    /// Thrown payloads of a failure, its causes and secondaries.
    pub(crate) fn failure_payloads(&self, failure: Word) -> Vec<Word> {
        // A lone record (no cause, no secondary: a cancellation, most thrown values) needs no walk (#932).
        match self.failure(failure) {
            None => return Vec::new(),
            Some(record) if record.cause.is_none() && record.secondary.is_empty() => {
                return match record.kind {
                    FailureKind::Thrown(payload) => vec![payload],
                    _ => Vec::new(),
                };
            }
            Some(_) => {}
        }
        let mut roots = Vec::new();
        let mut pending = vec![failure];
        let mut seen = HashSet::default();
        while let Some(value) = pending.pop() {
            if !seen.insert(value) {
                continue;
            }
            let Some(record) = self.failure(value) else { continue };
            if let FailureKind::Thrown(payload) = record.kind {
                roots.push(payload);
            }
            pending.extend(record.cause);
            pending.extend(record.secondary.iter().copied());
        }
        roots
    }

    /// Link `other` to `fresh`: as a secondary, or at the end of its cause chain.
    pub fn failure_link(&mut self, fresh: Word, other: Word, secondary: bool) -> Word {
        if secondary {
            self.heap.payload_mut::<Failure>(fresh).expect("failure").secondary.push(other);
            return fresh;
        }
        // `throw caught` makes a temporary carrier around the caught payload.
        // Reuse the original carrier, including its origin and existing causes.
        let same_payload = match (self.failure(fresh), self.failure(other)) {
            (Some(new), Some(handled)) => match (&new.kind, &handled.kind) {
                (FailureKind::Thrown(a), FailureKind::Thrown(b))
                | (FailureKind::Native(a), FailureKind::Native(b)) => a == b,
                _ => false,
            },
            _ => false,
        };
        if same_payload {
            return other;
        }
        if !matches!(self.failure(fresh).map(|f| &f.kind), Some(FailureKind::Thrown(_))) || fresh == other {
            return fresh;
        }
        let mut cursor = fresh;
        let mut seen = HashSet::default();
        loop {
            if !seen.insert(cursor) {
                return fresh;
            }
            match self.failure(cursor).and_then(|f| f.cause) {
                Some(next) if next == other => return fresh,
                Some(next) => cursor = next,
                None => break,
            }
        }
        self.heap.payload_mut::<Failure>(cursor).expect("failure").cause = Some(other);
        fresh
    }

    /// Run the destroy of `object` (identity `serial`, already marked dead), with `primary` the failure so far:
    /// a destroy's failure becomes primary or is linked to it as secondary. One rule for region closes and static
    /// resources (#909 2b).
    fn destroy_one(&mut self, ctx: *mut Ctx, object: Word, serial: u64, mut primary: Option<Word>, awaits: bool) -> DestroyStep {
        let descriptor = heap::payload(self.heap.header_of(object)) as usize;
        let Some(destroy) = self.loaded.program.objects[descriptor].destroy else { return DestroyStep::Next(primary) };
        if let Some(trace) = &mut self.resources.trace {
            use std::io::Write;
            let _ = writeln!(trace, "{} {serial}", self.loaded.program.objects[descriptor].name);
        }
        if self.loaded.program.functions[destroy.0 as usize].is_async {
            if awaits {
                return DestroyStep::Async(primary);
            }
            if let Some(primary) = primary {
                self.roots.push((primary, Repr::Ref));
            }
            let failure = self.failure_new(FailureKind::Program(
                "an object with an async destroy was released in a synchronous scope".into(),
            ));
            if primary.is_some() {
                self.roots.pop();
            }
            return DestroyStep::Next(Some(match primary {
                Some(primary) => self.failure_link(primary, failure, true),
                None => failure,
            }));
        }
        let code = match crate::jit::compile(self, destroy.0 as usize) {
            Ok(code) => code,
            Err(error) => {
                crate::runtime::fatal(unsafe { &mut *ctx }, self, error.0);
                return DestroyStep::Stop(primary);
            }
        };
        if let Some(primary) = primary {
            self.roots.push((primary, Repr::Ref));
        }
        // SAFETY: a destroy body takes its receiver.
        unsafe { crate::jit::invoke(code, ctx, &[object]) };
        if primary.is_some() {
            self.roots.pop();
        }
        let ctx = unsafe { &mut *ctx };
        if ctx.status == FAILED {
            ctx.status = OK;
            let failure = ctx.failure;
            primary = Some(match primary {
                Some(primary) => self.failure_link(primary, failure, true),
                None => failure,
            });
        } else if ctx.status != OK {
            return DestroyStep::Stop(primary);
        }
        DestroyStep::Next(primary)
    }

    /// Has `object`'s owner closed it (its identity is dead)?
    pub(crate) fn owner_is_dead(&self, object: Word) -> bool {
        self.resources.enabled && self.owner_of(object) == Some(Owner::Dead)
    }

    /// #817: the root region, made on first use.
    fn root_region(&mut self) -> Word {
        if let Some(root) = self.resources.root {
            return root;
        }
        let root = self.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
        self.resources.root = Some(root);
        root
    }

    /// #817: a Future that outlives its owner (still running) or starts after its group's owner closed: the root
    /// owns it until shutdown.
    pub(crate) fn detach_to_root(&mut self, future: Word) {
        okkama_devtrace::count!("runtime", "future detached to the root");
        let root = self.root_region();
        self.set_owner(future, Owner::Region(root));
        self.region_push(root, future);
    }

    /// #817: close a Future that nobody can await any more now: its outcome and what it keeps are destroyed.
    pub(crate) fn close_future(&mut self, ctx: *mut Ctx, future: Word) -> Option<Word> {
        let region = self.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
        self.set_owner(future, Owner::Region(region));
        self.region_push(region, future);
        self.region_close(ctx, region, &[], None)
    }

    /// #817: the root closer, after shutdown has drained the spawned tasks: the outcomes of detached Futures.
    pub(crate) fn close_root(&mut self, ctx: *mut Ctx) -> Option<Word> {
        let root = self.resources.root.take()?;
        self.roots.push((root, Repr::Ref));
        self.resources.root_closing = true;
        let failure = self.region_close(ctx, root, &[], None);
        self.resources.root_closing = false;
        self.roots.pop();
        failure
    }

    /// #909 2b: pin a static resource; from now on only its scope's `resource.leave`/`resource.unwind` end it.
    pub(crate) fn resource_pin(&mut self, object: Word) {
        if self.owner_of(object) == Some(Owner::Free) {
            self.set_owner(object, Owner::Static);
        }
    }

    /// #909 2b: a static resource's scope ends (`primary`: the failure on the failure path). Destroys it once and
    /// returns the failure to continue with.
    pub(crate) fn resource_close(&mut self, ctx: *mut Ctx, object: Word, primary: Option<Word>) -> Option<Word> {
        if self.owner_of(object) != Some(Owner::Static) {
            return primary;
        }
        if self.hand_to_pin_holder(object).is_some() {
            return primary;
        }
        self.set_owner(object, Owner::Dead);
        let serial = self.serial_of(object);
        self.dev_destroyed(object, 0);
        self.roots.push((object, Repr::Ref));
        let primary = match self.destroy_one(ctx, object, serial, primary, false) {
            DestroyStep::Next(primary) | DestroyStep::Stop(primary) | DestroyStep::Async(primary) => primary,
        };
        self.roots.pop();
        primary
    }

    /// Close `region`, keeping `keep` (and its graph) alive, running destroys.
    /// Returns the primary failure (the given one, a destroy's, or none).
    fn region_close(&mut self, ctx: *mut Ctx, region: Word, keep: &[Word], primary: Option<Word>) -> Option<Word> {
        match self.region_close_until(ctx, region, keep, primary, false) {
            Closed::Done(primary) => primary,
            Closed::Async(..) => unreachable!("a synchronous close awaits nothing"),
        }
    }

    /// The pending async destroy of a settling region and the failure so far.
    pub(crate) fn region_settling(&self, region: Word) -> (Option<Word>, Option<Word>) {
        match self.heap.field(region, 0) {
            UNBOUND => (None, None),
            overflow => {
                let value = self.heap.payload::<RegionValue>(overflow).expect("region");
                (value.settling, value.settled)
            }
        }
    }

    pub(crate) fn set_region_settling(&mut self, region: Word, settling: Option<Word>, settled: Option<Word>) {
        if self.heap.field(region, 0) == UNBOUND {
            if settling.is_none() && settled.is_none() {
                return;
            }
            let overflow = self.heap.allocate_payload(Box::new(RegionValue::default()));
            self.heap.set_field(region, 0, overflow);
        }
        let overflow = self.heap.field(region, 0);
        let value = self.heap.payload_mut::<RegionValue>(overflow).expect("region");
        value.settling = settling;
        value.settled = settled;
    }

    /// Destroy the region's identities from the last; with `awaits`, stop at
    /// one whose destroy is async (#154), otherwise that one is a failure.
    pub(crate) fn region_close_until(
        &mut self,
        ctx: *mut Ctx,
        region: Word,
        keep: &[Word],
        mut primary: Option<Word>,
        awaits: bool,
    ) -> Closed {
        // #915: a closing region drops its pins, except those a kept value carries on.
        if !self.resources.pinned_by.is_empty() {
            self.release_pins_keeping(region, keep);
        }
        let length = self.region_len(region);
        if length == 0 {
            return Closed::Done(primary);
        }
        // The common close keeps all it holds: a constructor, or an action
        // returning what it made. Nothing dies, so nothing is walked.
        if length <= REGION_SLOTS && (0..length).all(|index| keep.contains(&self.region_get(region, index))) {
            for index in 0..length {
                let object = self.region_get(region, index);
                if self.owner_of(object) == Some(Owner::Region(region)) {
                    self.set_owner(object, Owner::Free);
                    let serial = self.serial_of(object);
                    self.freed(serial);
                }
            }
            self.region_clear(region);
            return Closed::Done(primary);
        }
        let mut kept: HashSet<Word> =
            if keep.is_empty() { HashSet::default() } else {
                okkama_devtrace::count!("runtime", "walk from: region close keep");
                self.resource_graph(keep, Some(region)).into_iter().collect()
            };
        // Groups add ownership edges, including to bound Futures. Drain
        // dying groups first, withdrawing their edges before deciding which
        // Futures die. The surviving group packets retain a Future when its
        // lexical owner ends; the final owner releases its cache exactly once.
        let (mut groups, mut futures) = std::mem::take(&mut self.resources.close_lists);
        groups.clear();
        futures.clear();
        for index in 0..self.region_len(region) {
            self.queue_private(self.region_get(region, index), &mut groups, &mut futures);
        }
        while let Some(object) = groups.pop().or_else(|| futures.pop()) {
            if kept.contains(&object) || !self.owner_of(object)
                .is_some_and(|owner| owner == Owner::Free || owner == Owner::Region(region))
            {
                continue;
            }
            // Only language objects carry a runtime state; a Future never does (#932).
            let frame = heap::kind(self.heap.header_of(object)) == heap::FRAME;
            let group = !frame && matches!(self.states.get(&object), Some(crate::primitives::State::Group(_)));
            if !group
                && let Some(&home) = self.resources.identities.get(&object).and_then(|identity| identity.groups.first())
            {
                okkama_devtrace::count!("runtime", "walk from: region close child");
                for child in self.resource_graph(&[object], Some(region)) {
                    self.set_owner(child, Owner::Region(home));
                    self.region_push(home, child);
                    self.dev_owned(child, home);
                }
                continue;
            }
            if !group && !self.resources.root_closing && frame
                && crate::tasks::get(object, crate::tasks::STATE) == crate::tasks::RUNNING
            {
                // §2.4: a running Future outlives its owner, detached to the root with what it keeps.
                self.detach_to_root(object);
                continue;
            }
            // #915: a pinned Future or capability goes to its pin's holder instead.
            if self.hand_to_pin_holder(object).is_some() {
                continue;
            }
            self.set_owner(object, Owner::Dead);
            // A channel's last capability closes its unread messages with it (#587).
            let regions = match if frame { None } else { self.channel_released(object) } {
                Some(packet) => [Some(packet), None],
                None => self.private_regions(object),
            };
            for owned in regions.into_iter().flatten() {
                self.release_pins(owned);
                for index in 0..self.region_len(owned) {
                    let child = self.region_get(owned, index);
                    if group && let Some(identity) = self.resources.identities.get_mut(&child) {
                        identity.groups.retain(|&packet| packet != owned);
                    }
                    if self.owner_of(child) == Some(Owner::Region(owned)) {
                        self.set_owner(child, Owner::Region(region));
                        self.set_rehomed(child);
                        self.region_push(region, child);
                        self.dev_owned(child, region);
                        self.queue_private(child, &mut groups, &mut futures);
                    }
                }
                self.region_clear(owned);
                if !keep.is_empty() {
                    kept.extend(self.resource_graph(keep, Some(region)));
                }
            }
        }
        self.resources.close_lists = (groups, futures);
        if !keep.is_empty() {
            kept.extend(self.resource_graph(keep, Some(region)));
        }
        self.region_sort(region);
        // The region and the primary failure stay alive while destroys run.
        self.roots.push((region, Repr::Ref));
        let mut index = self.region_len(region);
        while let Some(action) = self.region_next_action(region, &mut index, &kept) {
            let (object, serial) = match action {
                CloseAction::Freed(serial) => {
                    self.freed(serial);
                    continue;
                }
                CloseAction::Destroy(object, serial) => (object, serial),
            };
            self.dev_destroyed(object, region);
            match self.destroy_one(ctx, object, serial, primary, awaits) {
                DestroyStep::Next(next) => primary = next,
                DestroyStep::Async(next) => {
                    self.roots.pop();
                    return Closed::Async(object, next);
                }
                DestroyStep::Stop(next) => {
                    primary = next;
                    break;
                }
            }
        }
        self.roots.pop();
        self.region_clear(region);
        Closed::Done(primary)
    }

    /// Text of a failure that reached the top, running `Error.text` for
    /// thrown values, formatted as the previous engine's report.
    pub fn describe(&mut self, ctx: *mut Ctx, root: Word) -> String {
        self.roots.push((root, Repr::Ref));
        let mut order = Vec::new();
        let mut pending = vec![root];
        let mut seen = HashSet::default();
        while let Some(next) = pending.pop() {
            if !seen.insert(next) {
                continue;
            }
            let Some(record) = self.failure(next) else { continue };
            pending.extend(record.cause);
            pending.extend(record.secondary.iter().rev().copied());
            order.push(next);
        }
        let mut messages: HashMap<Word, String> = HashMap::default();
        for &failure in &order {
            if let Some(FailureKind::Thrown(object)) = self.failure(failure).map(|f| &f.kind) {
                let object = *object;
                if let Some(function) = self.error_text_method(object)
                    && let Ok(code) = crate::jit::compile(self, function) {
                        // SAFETY: Error.text takes its receiver and returns a string.
                        let text = unsafe { crate::jit::invoke(code, ctx, &[object]) };
                        let ctx = unsafe { &mut *ctx };
                        if ctx.status == OK {
                            messages.insert(failure, self.heap.text(text).to_owned());
                        } else {
                            ctx.status = OK;
                        }
                    }
            }
        }
        self.roots.pop();
        let mut uncaught = self.uncaught_of(root, &messages);
        let report = self.report(root, &messages, &mut uncaught);
        self.uncaught = uncaught;
        report
    }

    /// Category, error type and message of a reported failure.
    fn uncaught_of(&self, root: Word, messages: &HashMap<Word, String>) -> Option<crate::Uncaught> {
        let record = self.failure(root)?;
        let (category, error_type, message) = match &record.kind {
            FailureKind::Cancelled => ("runtime", None, "Operation was cancelled".to_owned()),
            FailureKind::Native(object) => {
                ("runtime", Some("NativeFailure".to_owned()), self.heap.text(self.heap.field(*object, 3)).to_owned())
            }
            FailureKind::Program(message) => ("runtime", None, message.clone()),
            FailureKind::Panic(message) => ("panic", None, message.clone()),
            FailureKind::Thrown(object) => {
                let header = self.heap.header_of(*object);
                let name = if heap::kind(header) == heap::OBJECT {
                    self.reported_error_name(*object)
                } else {
                    "value".into()
                };
                ("uncaught-error", Some(name), messages.get(&root).cloned().unwrap_or_default())
            }
        };
        let trace = record.trace.iter().map(|&(function, pc)| crate::Site { function, pc }).collect();
        Some(crate::Uncaught { category, error_type, message, trace, trace_offset: 0, cause_traces: Vec::new() })
    }

    fn error_text_method(&self, object: Word) -> Option<usize> {
        let program = &self.loaded.program;
        let header = self.heap.header_of(object);
        if heap::kind(header) != heap::OBJECT {
            return None;
        }
        let (role, declaration) = program.roles.iter().enumerate().find(|(_, r)| r.is_error())?;
        let witness = program.objects[heap::payload(header) as usize].does.iter().find(|w| w.role.0 as usize == role)?;
        let slot = declaration.methods.iter().position(|m| m.name == "text")?;
        Some(witness.slots[slot].0 as usize)
    }

    fn reported_error_name(&self, object: Word) -> String {
        let full = &self.loaded.program.objects[heap::payload(self.heap.header_of(object)) as usize].name;
        full.rsplit("::").next().unwrap_or(full).to_owned()
    }

    fn report(&self, root: Word, messages: &HashMap<Word, String>, uncaught: &mut Option<crate::Uncaught>) -> String {
        const CHAIN_ENTRIES: usize = 64;
        use std::fmt::Write;
        let mut output = String::new();
        let mut pending = vec![(root, "", false)];
        let mut root_trace_offset = None;
        let mut shown: HashMap<Word, usize> = HashMap::default();
        let mut next_index = 0;
        while let Some((failure, label, root_cause)) = pending.pop() {
            if root_cause {
                root_trace_offset = Some(output.len());
            }
            if !label.is_empty() {
                let _ = write!(output, "\n{label}\n  ");
            }
            if let Some(index) = shown.get(&failure) {
                let _ = write!(output, "[failure #{index} already shown]");
                continue;
            }
            // The same bound as the other engines' reports (#270).
            if next_index == CHAIN_ENTRIES {
                output.push_str("... further failures not shown");
                break;
            }
            shown.insert(failure, next_index);
            next_index += 1;
            let Some(record) = self.failure(failure) else {
                output.push_str("<invalid failure reference>");
                continue;
            };
            match &record.kind {
                FailureKind::Cancelled => output.push_str("Cancelled: Operation was cancelled"),
                FailureKind::Native(object) => {
                    // Host details use the shared diagnostic contract; a
                    // nested entry keeps them indented under its label.
                    let field = |index| self.heap.text(self.heap.field(*object, index));
                    let text = format!(
                        "NativeFailure: {}\ncode: {}\ndomain: {}\noperation: {}",
                        field(3),
                        field(1),
                        field(0),
                        field(2)
                    );
                    output.push_str(&if label.is_empty() { text } else { text.replace('\n', "\n  ") });
                }
                FailureKind::Program(message) => {
                    let _ = write!(output, "RuntimeError: {message}");
                }
                FailureKind::Panic(message) => {
                    let _ = write!(output, "Panic: {message}");
                }
                FailureKind::Thrown(object) => {
                    let header = self.heap.header_of(*object);
                    // The type's own name, as the other engines print it.
                    let name = if heap::kind(header) == heap::OBJECT {
                        self.reported_error_name(*object)
                    } else {
                        "value".into()
                    };
                    match messages.get(&failure) {
                        Some(message) => {
                            let _ = write!(output, "{name}: {message}");
                        }
                        None => {
                            let _ = write!(output, "Uncaught {name}");
                        }
                    }
                }
            }
            // Offsets refer to the plain report. Hosts map these saved stacks
            // to source or SPB terms; secondaries keep their text-only rendering.
            if label == "Caused by:" && let Some(uncaught) = uncaught {
                let trace = record.trace.iter().map(|&(function, pc)| crate::Site { function, pc }).collect();
                uncaught.cause_traces.push((output.len(), trace));
            }
            if let Some(cause) = record.cause {
                pending.push((cause, "Caused by:", failure == root));
            }
            pending.extend(record.secondary.iter().rev().map(|&s| (s, "Secondary cleanup failure:", false)));
        }
        if let Some(uncaught) = uncaught {
            // Keep secondary reports before the root's stack, as before.
            uncaught.trace_offset = root_trace_offset.unwrap_or(output.len());
        }
        output
    }
}

// ---- helpers called from machine code ----

unsafe fn parts<'a>(ctx: *mut Ctx) -> (&'a mut Ctx, &'a mut Runtime) {
    let ctx = unsafe { &mut *ctx };
    let runtime = unsafe { &mut *ctx.runtime };
    (ctx, runtime)
}

/// `region.adopt` (own) and `region.track` of `value`.
#[inline(never)]
pub extern "C" fn rt_region_add(ctx: *mut Ctx, region: Word, value: Word, own: u64) -> Word {
    okkama_devtrace::count!("runtime", "rt_region_add");
    let (_, runtime) = unsafe { parts(ctx) };
    runtime.count(|_| "region add".into());
    // #530: machine code enters a region as 0 and the first add that has something to hold makes it.
    let region = match region {
        UNBOUND if (!runtime.may_reach_free(value) || runtime.spent_future(value)) && !runtime.pins_wait_for(value) => {
            return UNBOUND;
        }
        UNBOUND => {
            okkama_devtrace::count!("runtime", "region made at its first add");
            runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS])
        }
        region => region,
    };
    runtime.region_add(region, value, own != 0);
    region
}

/// #530: a region for a transfer into a region machine code has not made yet.
#[inline(never)]
pub extern "C" fn rt_region_new(ctx: *mut Ctx) -> Word {
    let (_, runtime) = unsafe { parts(ctx) };
    runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS])
}

/// `region.leave` of a region that holds something: close it, keeping what the
/// `count` words at `keep` reach.
#[inline(never)]
pub extern "C" fn rt_region_leave(ctx_ptr: *mut Ctx, region: Word, keep: *const Word, count: u64) {
    okkama_devtrace::count!("runtime", "rt_region_leave");
    let (_, runtime) = unsafe { parts(ctx_ptr) };
    runtime.count(|_| "region leave".into());
    let keep = if count == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(keep, count as usize) } };
    if let Some(failure) = runtime.region_close(ctx_ptr, region, keep, None) {
        let ctx = unsafe { &mut *ctx_ptr };
        ctx.failure = failure;
        ctx.status = FAILED;
    }
}

/// #912: one execution of an instrumented site (`OKKAMA_DEV=runtime`): identity requests and region adds by place.
#[inline(never)]
pub extern "C" fn rt_dev_site(ctx_ptr: *mut Ctx, kind: u64, function: u64, pc: u64) {
    let (_, runtime) = unsafe { parts(ctx_ptr) };
    let what = if kind == 0 { "identity requested at" } else { "region add at" };
    let symbol = &runtime.loaded.program.functions[function as usize].symbol;
    okkama_devtrace::count!("runtime", format!("{what} {symbol}:{pc}"));
}

/// #909 2b: pin a static resource.
#[inline(never)]
pub extern "C" fn rt_resource_pin(ctx_ptr: *mut Ctx, object: Word) {
    okkama_devtrace::count!("runtime", "rt_resource_pin");
    let (_, runtime) = unsafe { parts(ctx_ptr) };
    runtime.resource_pin(object);
}

/// #909 2b: a static resource's scope ends; its destroy's failure fails the instruction.
#[inline(never)]
pub extern "C" fn rt_resource_leave(ctx_ptr: *mut Ctx, object: Word) {
    okkama_devtrace::count!("runtime", "rt_resource_leave");
    let (_, runtime) = unsafe { parts(ctx_ptr) };
    runtime.count(|_| "resource leave".into());
    if let Some(failure) = runtime.resource_close(ctx_ptr, object, None) {
        let ctx = unsafe { &mut *ctx_ptr };
        ctx.failure = failure;
        ctx.status = FAILED;
    }
}

/// Resource identity of a new object with a destroy-reaching descriptor.
#[inline(never)]
pub extern "C" fn rt_created(ctx: *mut Ctx, descriptor: u64, object: Word) {
    okkama_devtrace::count!("runtime", "rt_created");
    let (_, runtime) = unsafe { parts(ctx) };
    runtime.count(|r| format!("resource {}", r.loaded.program.objects[descriptor as usize].name));
    okkama_devtrace::count!("runtime", format!("identity requested: {}",
        runtime.loaded.program.objects[descriptor as usize].name));
    runtime.created_built(descriptor as usize, object);
}

/// Region instructions and failure links of instruction `pc` (uses at `uses`).
#[inline(never)]
pub extern "C" fn rt_failure_op(ctx_ptr: *mut Ctx, function: u64, pc: u64, uses: *const Word) -> Word {
    okkama_devtrace::count!("runtime", "rt_failure_op");
    let frame = crate::runtime::frame_pointer!();
    let (ctx, runtime) = unsafe { parts(ctx_ptr) };
    // SAFETY: the loaded program is never mutated while machine code runs, so
    // the instruction stays valid while the runtime is borrowed mutably.
    let instruction: &I =
        unsafe { &*(&runtime.loaded.program.functions[function as usize].code[pc as usize] as *const I) };
    runtime.count(|r| format!("region {} in {}", instruction.mnemonic(), r.loaded.program.functions[function as usize].symbol));
    // Uses in visit order; a region may keep many registers, so spill past 16.
    let mut inline = [okkama_sparrow::Register(u32::MAX); 16];
    let mut spilled = Vec::new();
    let mut count = 0;
    instruction.visit(&mut |access| {
        if let okkama_sparrow::Access::Use(register) = access {
            if count < inline.len() {
                inline[count] = register;
            } else {
                if spilled.is_empty() {
                    spilled.extend_from_slice(&inline);
                }
                spilled.push(register);
            }
            count += 1;
        }
    });
    let registers: &[okkama_sparrow::Register] = if count <= inline.len() { &inline[..count] } else { &spilled };
    let words = unsafe { std::slice::from_raw_parts(uses, count) };
    let operands = crate::ops::Operands::new(registers, words);
    let value = |register: &okkama_sparrow::Register| operands.value(register);
    let result = match instruction {
        I::RegionEnter { .. } => runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]),
        I::RegionAdopt { region, value: v } | I::RegionTrack { region, value: v } => {
            runtime.region_add(value(region), value(v), matches!(instruction, I::RegionAdopt { .. }));
            UNBOUND
        }
        I::RegionTransfer { from, to, value: v } => {
            // #530: a region never made holds nothing to transfer.
            if value(from) != UNBOUND {
                runtime.region_transfer(value(from), value(to), value(v));
            }
            UNBOUND
        }
        I::RegionLeave { region, keep } => {
            let keep: Vec<Word> = keep.iter().map(&value).collect();
            if let Some(failure) = runtime.region_close(ctx_ptr, value(region), &keep, None) {
                let ctx = unsafe { &mut *ctx_ptr };
                ctx.failure = failure;
                ctx.status = FAILED;
            }
            UNBOUND
        }
        I::RegionUnwind { region, failure, .. } => {
            let primary = value(failure);
            if value(region) == UNBOUND {
                return primary;
            }
            let kept = runtime.failure_payloads(primary);
            runtime.region_close(ctx_ptr, value(region), &kept, Some(primary)).unwrap_or(primary)
        }
        I::ResourceUnwind { value: v, failure, .. } => {
            let primary = value(failure);
            runtime.resource_close(ctx_ptr, value(v), Some(primary)).unwrap_or(primary)
        }
        I::FailSecondary { primary, cleanup, .. } => runtime.failure_link(value(primary), value(cleanup), true),
        I::FailCaused { new, handled, .. } => runtime.failure_link(value(new), value(handled), false),
        I::Raise { failure } => {
            ctx.failure = value(failure);
            ctx.status = FAILED;
            UNBOUND
        }
        I::Panic { message } => {
            let message = runtime.heap.text(value(message)).to_owned();
            ctx.failure = runtime.failure_new(FailureKind::Panic(message));
            ctx.status = FAILED;
            UNBOUND
        }
        I::RoleNarrow { source, role, message, .. } => {
            let object = value(source);
            let header = runtime.heap.header_of(object);
            let does = heap::kind(header) == heap::OBJECT
                && runtime.loaded.program.objects[heap::payload(header) as usize]
                    .does
                    .iter()
                    .any(|w| w.role == *role);
            if does {
                object
            } else {
                let message = if message.is_empty() { "role witness is missing".to_owned() } else { message.clone() };
                ctx.failure = runtime.failure_new(FailureKind::Program(message));
                ctx.status = FAILED;
                UNBOUND
            }
        }
        _ => {
            return crate::runtime::fatal(ctx, runtime, format!("{} is not a failure or region instruction", instruction.mnemonic()));
        }
    };
    let ctx = unsafe { &mut *ctx_ptr };
    if ctx.status == OK {
        runtime.safepoint(frame, ctx, &[(result, Repr::Ref)]);
    }
    result
}

/// `catch`: the payload (in `scratch`) and 1 when the failure matches.
#[inline(never)]
pub extern "C" fn rt_catch(ctx: *mut Ctx, function: u64, pc: u64, failure: Word) -> u64 {
    okkama_devtrace::count!("runtime", "rt_catch");
    let (ctx, runtime) = unsafe { parts(ctx) };
    let I::Catch { kind, ty, .. } = &runtime.loaded.program.functions[function as usize].code[pc as usize] else {
        return 0;
    };
    if matches!(runtime.failure(failure).map(|f| &f.kind), Some(FailureKind::Cancelled)) {
        // Cancelled matches the builtin `Cancelled` object and the Error role.
        let program = &runtime.loaded.program;
        let builtin = program.objects.iter().position(|o| o.builtin && o.name == "Cancelled");
        let matches = match kind {
            CatchKind::Object => builtin == Some(*ty as usize),
            CatchKind::Role => program.roles.get(*ty as usize).is_some_and(|r| r.is_error()),
            CatchKind::Variant => false,
        };
        // The handler's binding is a `Cancelled` value, built from the builtin object's descriptor; the compiler
        // brings the object along with every catch of the Error role. Without it there is no value to bind (an
        // unbound word would be dispatched on by `e.text()`, #744), so the catch does not match and the
        // cancellation goes on unwinding.
        let (true, Some(descriptor)) = (matches, builtin) else {
            return 0;
        };
        let fields = vec![UNBOUND; program.objects[descriptor].fields.len()];
        ctx.scratch = runtime.heap.object_new(descriptor as u32, &fields);
        return 1;
    }
    if let Some(FailureKind::Native(object)) = runtime.failure(failure).map(|f| &f.kind) {
        // NativeFailure matches its builtin object and the Error role.
        let object = *object;
        let program = &runtime.loaded.program;
        let matches = match kind {
            CatchKind::Object => program.objects.get(*ty as usize).is_some_and(|o| o.is_native_failure()),
            CatchKind::Role => program.roles.get(*ty as usize).is_some_and(|r| r.is_error()),
            CatchKind::Variant => false,
        };
        if matches {
            ctx.scratch = object;
        }
        return u64::from(matches);
    }
    let Some(FailureKind::Thrown(payload)) = runtime.failure(failure).map(|f| &f.kind) else {
        return 0;
    };
    let payload = *payload;
    let header = runtime.heap.header_of(payload);
    if heap::kind(header) != heap::OBJECT {
        return 0;
    }
    let owner = heap::payload(header);
    let matches = match kind {
        CatchKind::Object => owner == *ty,
        CatchKind::Role => runtime.loaded.program.objects[owner as usize].does.iter().any(|w| w.role.0 == *ty),
        CatchKind::Variant => runtime.loaded.program.variants[*ty as usize].cases.iter().any(|(_, o)| o.0 == owner),
    };
    if matches {
        ctx.scratch = payload;
    }
    u64::from(matches)
}

#[cfg(test)]
mod cause_stack_tests {
    use super::*;

    fn runtime() -> Runtime {
        let program = okkama_sparrow::assemble(
            "sparrow 16\nmodule \"cause-stacks\" \"1.0.0\"\ntype t0 int\n\
             function f0 \"main\" {\n registers [t0]\n params []\n result t0\n\
             0: const r0, int 0\n 1: ret r0\n}\nentry f0\n",
        ).unwrap();
        Runtime::new(crate::program::Loaded::new(program)).unwrap()
    }

    #[test]
    fn rethrow_reuses_carrier_with_its_trace_causes_and_secondaries() {
        let mut runtime = runtime();
        let payload = runtime.heap.allocate_text("caught");
        let caught = runtime.failure_new(FailureKind::Thrown(payload));
        let cause = runtime.failure_new(FailureKind::Panic("origin".into()));
        let secondary = runtime.failure_new(FailureKind::Panic("cleanup".into()));
        runtime.failure_link(caught, cause, false);
        runtime.failure_link(caught, secondary, true);
        runtime.heap.payload_mut::<Failure>(caught).unwrap().trace.push((1, 2));
        let fresh = runtime.failure_new(FailureKind::Thrown(payload));
        runtime.heap.payload_mut::<Failure>(fresh).unwrap().trace.push((3, 4));
        assert_eq!(runtime.failure_link(fresh, caught, false), caught);
        let record = runtime.failure(caught).unwrap();
        assert_eq!(record.trace, [(1, 2)]);
        assert_eq!(record.cause, Some(cause));
        assert_eq!(record.secondary, [secondary]);
        assert_eq!(runtime.failure_link(caught, caught, false), caught);
    }

    #[test]
    fn distinct_payloads_link_even_when_their_text_is_equal() {
        let mut runtime = runtime();
        let a = runtime.heap.allocate_text("same text");
        let b = runtime.heap.allocate_text("same text");
        assert_ne!(a, b);
        let caught = runtime.failure_new(FailureKind::Thrown(a));
        let fresh = runtime.failure_new(FailureKind::Thrown(b));
        assert_eq!(runtime.failure_link(fresh, caught, false), fresh);
        assert_eq!(runtime.failure(fresh).unwrap().cause, Some(caught));
        assert_eq!(runtime.failure(caught).unwrap().cause, None);
    }

    #[test]
    fn diagnostic_stacks_follow_report_order_and_skip_secondaries() {
        let mut runtime = runtime();
        let a = runtime.heap.allocate_text("root");
        let b = runtime.heap.allocate_text("cause");
        let c = runtime.heap.allocate_text("cleanup");
        let root = runtime.failure_new(FailureKind::Thrown(a));
        let cause = runtime.failure_new(FailureKind::Thrown(b));
        let cleanup = runtime.failure_new(FailureKind::Thrown(c));
        let cleanup_cause = runtime.failure_new(FailureKind::Panic("cleanup cause".into()));
        runtime.failure_link(root, cause, false);
        runtime.failure_link(root, cleanup, true);
        runtime.failure_link(cleanup, cleanup_cause, false);
        for (failure, pc) in [(root, 1), (cause, 2), (cleanup, 3), (cleanup_cause, 4)] {
            runtime.heap.payload_mut::<Failure>(failure).unwrap().trace.push((0, pc));
        }
        let messages = HashMap::default();
        let mut uncaught = runtime.uncaught_of(root, &messages);
        let plain = runtime.report(root, &messages, &mut uncaught);
        let diagnostic = uncaught.unwrap().render_report(&plain, |trace| format!("stack {}\n", trace[0].pc));
        assert_eq!(diagnostic, "Uncaught value\nSecondary cleanup failure:\n  Uncaught value\n\
            Caused by:\n  Panic: cleanup cause\n  stack 4\nstack 1\n\
            Caused by:\n  Uncaught value\n  stack 2");
    }

    #[test]
    fn failure_payloads_collect_thrown_values_of_the_whole_chain() {
        let mut runtime = runtime();
        let a = runtime.heap.allocate_text("root");
        let b = runtime.heap.allocate_text("cause");
        let c = runtime.heap.allocate_text("cleanup");
        let lone = runtime.failure_new(FailureKind::Thrown(a));
        assert_eq!(runtime.failure_payloads(lone), [a]);
        let panic = runtime.failure_new(FailureKind::Panic("lone".into()));
        assert!(runtime.failure_payloads(panic).is_empty());
        assert!(runtime.failure_payloads(a).is_empty(), "not a failure");
        let root = runtime.failure_new(FailureKind::Thrown(a));
        let cause = runtime.failure_new(FailureKind::Thrown(b));
        let cleanup = runtime.failure_new(FailureKind::Thrown(c));
        runtime.failure_link(root, cause, false);
        let mut payloads = runtime.failure_payloads(root);
        payloads.sort_unstable();
        let mut expected = vec![a, b];
        expected.sort_unstable();
        assert_eq!(payloads, expected, "the cause's payload");
        let only_secondary = runtime.failure_new(FailureKind::Panic("primary".into()));
        runtime.failure_link(only_secondary, cleanup, true);
        assert_eq!(runtime.failure_payloads(only_secondary), [c], "a secondary's payload");
    }

    #[test]
    fn native_rethrow_reuses_carrier_but_secondary_link_does_not() {
        let mut runtime = runtime();
        let payload = runtime.heap.allocate_text("native");
        let caught = runtime.failure_new(FailureKind::Native(payload));
        let fresh = runtime.failure_new(FailureKind::Native(payload));
        assert_eq!(runtime.failure_link(fresh, caught, false), caught);
        assert_eq!(runtime.failure_link(fresh, caught, true), fresh);
        assert_eq!(runtime.failure(fresh).unwrap().secondary, [caught]);
    }
}

#[cfg(test)]
mod structure_tests {
    use super::*;

    fn runtime() -> Runtime {
        let program = okkama_sparrow::assemble(
            "sparrow 16\nmodule \"structure\" \"1.0.0\"\ntype t0 int\n\
             function f0 \"main\" {\n registers [t0]\n params []\n result t0\n\
             0: const r0, int 0\n 1: ret r0\n}\nentry f0\n",
        ).unwrap();
        let mut runtime = Runtime::new(crate::program::Loaded::new(program)).unwrap();
        runtime.resources.enabled = true;
        runtime
    }

    /// Tweed #906: a container's index is dropped when the collector reclaims the container, so a container
    /// later allocated at the same address never walks the old one's children.
    #[test]
    fn a_reclaimed_container_takes_its_structure_index_with_it() {
        let mut runtime = runtime();
        let text = runtime.heap.allocate_text("element");
        let array = runtime.heap.allocate_array(Repr::Ref, &[text, text]);
        runtime.resource_graph(&[array], None);
        assert!(runtime.resources.structure.contains_key(&array), "the walk indexes the array");
        runtime.heap.collect(|_| {});
        let released = runtime.heap.take_released();
        runtime.resources.forget(&released);
        assert!(!runtime.resources.structure.contains_key(&array), "the reclaimed array's index is gone");
    }

    /// The index keeps only children that may hold an identity: strings and scalars are left out.
    #[test]
    fn an_index_leaves_out_children_that_can_hold_no_identity() {
        let mut runtime = runtime();
        let text = runtime.heap.allocate_text("element");
        let inner = runtime.heap.allocate_array(Repr::Ref, &[text]);
        let outer = runtime.heap.allocate_array(Repr::Ref, &[text, inner]);
        runtime.resource_graph(&[outer], None);
        assert_eq!(runtime.resources.structure[&inner].len(), 0);
        // The inner array was indexed (empty) in the same walk, after the outer index was built from what was
        // known then: the outer one keeps it (unknown at that moment), never the string.
        assert_eq!(&*runtime.resources.structure[&outer], &[(inner, Repr::Ref)]);
    }
}

#[cfg(test)]
mod inline_identity_tests {
    use super::*;

    fn runtime() -> Runtime {
        let program = okkama_sparrow::assemble(
            "sparrow 16\nmodule \"inline\" \"1.0.0\"\ntype t0 int\ntype t1 object obj0\n\
             object obj0 \"Vault\" {\n field \"n\" t0\n destroy f1\n}\n\
             function f0 \"main\" {\n registers [t0]\n params []\n result t0\n\
             0: const r0, int 0\n 1: ret r0\n}\n\
             function f1 \"Vault.destroy\" {\n registers [t1]\n params [r0]\n result void\n 0: ret _\n}\n\
             function f2 \"work\" async {\n registers [t0]\n params [r0]\n result t0\n 0: ret r0\n}\n\
             function f3 \"hold\" async {\n registers [t1, t0]\n params [r0]\n result t0\n 0: const r1, int 0\n 1: ret r1\n}\n\
             entry f0\n",
        ).unwrap();
        let mut runtime = Runtime::new(crate::program::Loaded::new(program)).unwrap();
        runtime.resources.enabled = true;
        runtime
    }

    /// #931: in a program with resources a Future's serial and owner live at the end of its frame, after the
    /// retained arguments; the identity map never sees it, and a collection keeps the words.
    #[test]
    fn a_future_keeps_its_identity_at_the_end_of_its_frame() {
        let mut runtime = runtime();
        let at = runtime.loaded.functions[2].identity_at.expect("a program with resources");
        let argument = runtime.heap.int(5);
        let frame = runtime.future_new(2, &[argument]);
        assert!(!runtime.resources.identities.contains_key(&frame), "no map entry");
        assert_eq!(runtime.serial_of(frame), runtime.resources.serial);
        assert_eq!(runtime.owner_of(frame), Some(Owner::Free));
        assert_eq!(crate::tasks::get(frame, at - 8), argument, "the retained argument comes before the identity");
        let region = runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
        runtime.set_owner(frame, Owner::Region(region));
        runtime.heap.collect(|visit| { visit(frame, Repr::Ref); visit(region, Repr::Ref); });
        assert_eq!(runtime.owner_of(frame), Some(Owner::Region(region)));
        assert_eq!(runtime.serial_of(frame), runtime.resources.serial);
        let layout = &runtime.heap.layouts().frames[2];
        assert_eq!(&layout[layout.len() - 2..], &[Repr::Raw, Repr::Raw], "the collector skips the identity words");
    }

    /// #932: adding a Future whose retained inputs reach no free identity finds what the walk finds (the Future
    /// while it is free, else nothing) without walking; an input that may reach one still takes the walk (#578).
    #[test]
    fn a_future_with_leaf_inputs_is_added_without_a_walk() {
        let mut runtime = runtime();
        let walked = |runtime: &mut Runtime, frame: Word| runtime.resource_graph_into(&[frame], None, None);
        let five = runtime.heap.int(5);
        let leaf = runtime.future_new(2, &[five]);
        assert_eq!(runtime.lone_frame(leaf), Some(Some(leaf)));
        assert_eq!(walked(&mut runtime, leaf), [leaf], "the walk agrees");
        let region = runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
        runtime.region_add(region, leaf, true);
        assert_eq!(runtime.owner_of(leaf), Some(Owner::Region(region)));
        assert!(runtime.region_contains(region, leaf));
        // Owned elsewhere: neither finds anything, and a tracking add leaves the other region alone.
        assert_eq!(runtime.lone_frame(leaf), Some(None));
        assert!(walked(&mut runtime, leaf).is_empty());
        let other = runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
        runtime.region_add(other, leaf, false);
        assert!(!runtime.region_contains(other, leaf));
        assert_eq!(runtime.owner_of(leaf), Some(Owner::Region(region)));
        // The Future takes its resource input into its INPUTS (#587): the input is no longer free, so the add
        // finds the Future alone, as the walk does.
        let vault = runtime.heap.object_new(0, &[7]);
        runtime.created(0, vault);
        let holder = runtime.future_new(3, &[vault]);
        assert_ne!(runtime.owner_of(vault), Some(Owner::Free));
        assert_eq!(runtime.lone_frame(holder), Some(Some(holder)));
        assert_eq!(walked(&mut runtime, holder), [holder]);
        // A container input may reach a free identity: the add keeps the walk.
        let other_vault = runtime.heap.object_new(0, &[8]);
        runtime.created(0, other_vault);
        let list = runtime.heap.allocate_array(Repr::Ref, &[other_vault]);
        let frame = runtime.future_new(3, &[UNBOUND]);
        let at = runtime.loaded.functions[3].identity_at.unwrap();
        crate::tasks::set(frame, at - 8, list);
        assert_eq!(runtime.lone_frame(frame), None, "a reference array may reach one");
    }

    /// #932: a closing region's entries are sorted by serial, the inline slots rewritten in place and an
    /// overflowing region rebuilt; every entry stays a member exactly once.
    #[test]
    fn a_region_sorts_its_entries_by_serial() {
        let mut runtime = runtime();
        for count in [1, 3, REGION_SLOTS, REGION_SLOTS + 3] {
            let vaults: Vec<Word> = (0..count as u64).map(|n| {
                let vault = runtime.heap.object_new(0, &[n]);
                runtime.created(0, vault);
                vault
            }).collect();
            let region = runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
            // Newest first, then the rest: an order rehoming produces.
            let mut scrambled = vaults.clone();
            scrambled.rotate_right(1);
            scrambled.reverse();
            for &vault in &scrambled {
                runtime.region_push(region, vault);
            }
            runtime.region_sort(region);
            let sorted: Vec<Word> = (0..runtime.region_len(region)).map(|i| runtime.region_get(region, i)).collect();
            assert_eq!(sorted, vaults, "{count} entries");
            assert!(vaults.iter().all(|&vault| runtime.region_contains(region, vault)));
        }
    }

    /// #932: a consumed Future with no regions left (a driver's outcome taken) is not kept by a region; one whose
    /// inputs are still held is, so the region closes them.
    #[test]
    fn a_spent_future_is_not_kept_by_a_region() {
        let mut runtime = runtime();
        let spent = runtime.future_new(2, &[UNBOUND]);
        crate::tasks::set(spent, crate::tasks::STATE, crate::tasks::CONSUMED);
        assert!(runtime.spent_future(spent));
        let region = runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
        runtime.region_add(region, spent, false);
        assert!(!runtime.region_contains(region, spent));
        let holding = runtime.future_new(2, &[UNBOUND]);
        crate::tasks::set(holding, crate::tasks::STATE, crate::tasks::CONSUMED);
        let inputs = runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
        let vault = runtime.heap.object_new(0, &[1]);
        runtime.created(0, vault);
        runtime.region_add(inputs, vault, true);
        runtime.set_held(holding, crate::tasks::Held::Inputs, inputs);
        assert!(!runtime.spent_future(holding));
        runtime.region_add(region, holding, false);
        assert!(runtime.region_contains(region, holding), "its inputs still need a close");
        let running = runtime.future_new(2, &[UNBOUND]);
        assert!(!runtime.spent_future(running), "not consumed");
    }

    /// #528: an object with a destroy of its own carries its serial and owner in two words after its fields;
    /// the identity map never sees it.
    #[test]
    fn an_object_with_a_destroy_keeps_its_identity_in_itself() {
        let mut runtime = runtime();
        assert_eq!(runtime.heap.object_words(0, 1), 2 + heap::IDENTITY_WORDS, "header, one field, identity");
        let vault = runtime.heap.object_new(0, &[7]);
        runtime.created(0, vault);
        assert!(!runtime.resources.identities.contains_key(&vault), "no map entry");
        assert_eq!(runtime.serial_of(vault), runtime.resources.serial);
        assert_eq!(runtime.owner_of(vault), Some(Owner::Free));
        let region = runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
        runtime.set_owner(vault, Owner::Region(region));
        assert_eq!(runtime.owner_of(vault), Some(Owner::Region(region)));
        runtime.set_owner(vault, Owner::Dead);
        assert_eq!(runtime.owner_of(vault), Some(Owner::Dead));
        assert_eq!(runtime.heap.field(vault, 0), 7, "the field is untouched");
    }

    /// `obj.next` copies the source's header word; the successor is a new identity whose cache was never rehomed.
    #[test]
    fn a_successor_does_not_inherit_its_sources_rehomed_cache() {
        let mut runtime = runtime();
        let source = runtime.heap.object_new(0, &[1]);
        runtime.created(0, source);
        runtime.set_rehomed(source);
        assert!(runtime.rehomed(source));
        let successor = runtime.heap.object_new(0, &[2]);
        let header = runtime.heap.header_of(source);
        // SAFETY: both are live objects of the same descriptor; this is the copy `obj.next` makes.
        unsafe { *(successor as *mut u64) = header };
        runtime.created(0, successor);
        assert!(!runtime.rehomed(successor));
        assert!(runtime.serial_of(successor) > runtime.serial_of(source));
    }

    /// #529: a slice of a summarized array is summarized at birth; a part that still reaches a free identity, or
    /// a container nobody summarized, leaves the result to be walked.
    #[test]
    fn a_container_made_from_summarized_parts_is_summarized_at_birth() {
        let mut runtime = runtime();
        let vault = runtime.heap.object_new(0, &[7]);
        runtime.created(0, vault);
        let source = runtime.heap.allocate_array(Repr::Ref, &[vault, vault]);
        let slice = runtime.heap.allocate_array(Repr::Ref, &[vault]);
        // Not summarized yet: the result must be walked.
        runtime.inherit_summary(slice, &[(source, Repr::Ref)]);
        assert_eq!(runtime.heap.header_of(slice) & heap::SUMMARY, 0);
        runtime.summarize(source);
        // A free identity among the parts: no summary either.
        runtime.inherit_summary(slice, &[(source, Repr::Ref), (vault, Repr::Ref)]);
        assert_eq!(runtime.heap.header_of(slice) & heap::SUMMARY, 0);
        let region = runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
        runtime.set_owner(vault, Owner::Region(region));
        runtime.inherit_summary(slice, &[(source, Repr::Ref), (vault, Repr::Ref), (41, Repr::Int)]);
        assert_ne!(runtime.heap.header_of(slice) & heap::SUMMARY, 0, "owned vault, summarized source");
        assert!(runtime.resources.summaries.contains_key(&slice));
    }

    /// #529: an adopting add takes every free identity a big array reaches, so the array is summarized; a small
    /// one is cheaper to walk again.
    #[test]
    fn an_adopted_big_array_is_summarized_and_a_small_one_is_not() {
        let mut runtime = runtime();
        let region = runtime.heap.allocate_array(Repr::Ref, &[UNBOUND; 1 + REGION_SLOTS]);
        let vault = runtime.heap.object_new(0, &[7]);
        runtime.created(0, vault);
        let small = runtime.heap.allocate_array(Repr::Ref, &[vault]);
        runtime.region_add(region, small, true);
        assert_eq!(runtime.heap.header_of(small) & heap::SUMMARY, 0);
        let other = runtime.heap.object_new(0, &[9]);
        runtime.created(0, other);
        let big = runtime.heap.allocate_array(Repr::Ref, &vec![other; SUMMARY_STEPS]);
        runtime.region_add(region, big, true);
        assert_eq!(runtime.owner_of(other), Some(Owner::Region(region)));
        assert_ne!(runtime.heap.header_of(big) & heap::SUMMARY, 0);
    }
}
