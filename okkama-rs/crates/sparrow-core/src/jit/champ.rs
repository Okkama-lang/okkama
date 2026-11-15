//! SPB 17 lowering with typed frame slots and precise liveness maps.
//! Post has call-local storage and never belongs to the helper call map.
use super::*;
use crate::champ::{self as c, machine};
use okkama_sparrow::spb17::{self as s, Argument as A, Location as L, Opcode as O};
use cranelift_codegen::ir::StackSlot;

mod roots;
use roots::Frame;
mod futures;

pub(super) fn compile(runtime: &mut Runtime, index: usize) -> Result<u64, Error> {
    if runtime.invocations[index].code != 0 {
        return Ok(runtime.invocations[index].code);
    }
    let machine = runtime.champ.machine.as_ref().expect("Champ program");
    let program = machine.source.clone();
    let source = &program.functions[index];
    // An async defer entry runs as a shielded child of its caller (C-6), with the runtime of #601 R5.
    if matches!(source.entry_abi, s::EntryAbi::AsyncDefer { .. }) {
        return Err(Error("Champ async defer entries run with the async resources runtime (#601 R5)".into()));
    }
    let frame = Frame::new(source)?;
    // An async body takes the context and its frame (draft 3 §13); owning entries take CallOwn.
    let asynchronous = matches!(source.entry_abi, s::EntryAbi::AsyncBody { .. });
    let owned = source.entry_abi != s::EntryAbi::Observation && !asynchronous;
    let params = if asynchronous {
        1
    } else if owned {
        3
    } else {
        source.params.iter().filter(|location| matches!(location, L::Register(_))).count()
    };
    let signature = runtime.jit.signature(params);
    let id = runtime.jit.module.declare_function(&format!("champ{index}"), Linkage::Local, &signature)
        .map_err(|error| Error(format!("Champ declare: {error}")))?;
    let mut context = runtime.jit.module.make_context();
    context.func.signature = signature;
    let config = runtime.jit.module.target_config();
    let mut frontend = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut context.func, &mut frontend);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let ctx = builder.block_params(entry)[0];
        let zero = builder.ins().iconst(types::I64, 0);
        let io = if owned { builder.block_params(entry)[1] } else { zero };
        let body = if asynchronous { builder.block_params(entry)[1] } else { zero };
        let scope_base = frame.bytes;
        let bytes = scope_base + program.tables.scopes.len() * 16;
        if bytes as u64 > MAX_FRAME / 2 {
            return Err(Error("Champ service area exceeds frame budget".into()));
        }
        let storage = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot, bytes as u32, 3,
        ));
        // All slots, including late-branch services and edge scratch, are safe before entry rejection.
        for offset in (0..bytes).step_by(8) {
            builder.ins().stack_store(types::I64, zero, storage, offset as i32);
        }
        let post = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 64, 3));
        let blocks = (0..source.code.len()).map(|_| builder.create_block()).collect();
        let stop = builder.create_block();
        let invocations = runtime.invocations.as_ptr();
        let polled = runtime.limits.polled();
        let fuel = runtime.budget.fuel.address();
        let mut emit = Emit {
            builder: &mut builder, jit: &mut runtime.jit, program: &program, source, index, invocations,
            ctx, io, owned, body, frame_map: machine.frames[index].as_ref(), schedules: &machine.schedules[index],
            storage, frame: &frame, scope_base, post, blocks, stop, pc: None, polled, fuel,
            plans: &machine.plans[index], selections: &machine.selections[index], entries: &machine.entries,
            shielded_entries: &machine.shielded_entries,
            constants: &mut runtime.constants, heap: &mut runtime.heap, globals: machine.globals.clone(),
        };
        let args = emit.builder.block_params(entry).to_vec();
        let mut language = 0;
        for &location in &source.params {
            let value = match location {
                L::Register(_) if owned => {
                    let value = emit.load(args[2], language * 8);
                    language += 1;
                    value
                }
                L::Register(_) => {
                    language += 1;
                    args[language]
                }
                L::Service(id) if source.services[id as usize] == s::ServiceKind::Packet => {
                    emit.load(io, 0)
                }
                _ => zero,
            };
            emit.write(location, &[value]);
        }
        // Before body and before acceptance. Rejection still owns io.input, not its service alias.
        let sp = emit.builder.ins().get_stack_pointer(types::I64);
        let limit = emit.load(ctx, std::mem::offset_of!(Ctx, stack_limit));
        let low = emit.builder.ins().icmp(IntCC::UnsignedLessThan, sp, limit);
        let overflow = emit.builder.create_block();
        let start = if source.code.is_empty() || asynchronous { emit.builder.create_block() } else { emit.blocks[0] };
        emit.builder.ins().brif(low, overflow, &[], start, &[]);
        emit.builder.switch_to_block(overflow);
        let cleanup = emit.call("champ_machine_cleanup_mode", &[ctx]).unwrap();
        let fatal = emit.builder.create_block();
        let reject = emit.builder.create_block();
        emit.builder.ins().brif(cleanup, fatal, &[], reject, &[]);
        emit.builder.switch_to_block(fatal);
        emit.call("champ_machine_fatal_stack", &[ctx]);
        emit.builder.ins().jump(stop, &[]);
        emit.builder.switch_to_block(reject);
        if asynchronous {
            // The body closes its inputs and publishes the overflow; its Future is settled either way.
            emit.call("champ_body_reject", &[ctx, body]);
            emit.builder.ins().return_(&[zero]);
        } else if owned {
            let ordinal = emit.call("champ_entry_reject", &[ctx, io]).unwrap();
            emit.builder.ins().return_(&[ordinal]);
        } else {
            emit.call("stack_overflow", &[ctx]);
            emit.builder.ins().return_(&[zero]);
        }
        if source.code.is_empty() {
            emit.builder.switch_to_block(start);
            let argv = if owned { args[2] } else {
                let values = args[1..].to_vec();
                emit.record(&values)
            };
            let argc = emit.iconst(language as u64);
            let function = emit.iconst(index as u64);
            let result = emit.call("champ_machine_external", &[ctx, function, io, argv, argc]).unwrap();
            emit.builder.ins().return_(&[result]);
        } else {
            if asynchronous {
                emit.builder.switch_to_block(start);
                emit.resume_dispatch();
            }
            for (pc, instruction) in source.code.iter().enumerate() {
                emit.pc = Some(pc);
                emit.builder.switch_to_block(emit.blocks[pc]);
                if pc == 0 && emit.polled {
                    // No edge enters block 0 (ENTRY_BLOCK_TARGET): it is reached at the function entry and by a
                    // self tail call, each a step as in Tomato.
                    emit.poll();
                }
                if pc == 0 && owned && s::plain_inputs(&program, source) {
                    emit.checked("champ_machine_plain_packet", &[ctx, io]);
                }
                emit.enter_roots();
                emit.instruction(instruction)?;
            }
        }
        emit.builder.switch_to_block(stop);
        let terminal = emit.iconst(c::OUTCOME_STOP);
        emit.builder.ins().return_(&[terminal]);
        builder.seal_all_blocks();
        builder.finalize(config);
    }
    #[cfg(test)]
    {
        cranelift_codegen::verify_function(&context.func, runtime.jit.module.isa())
            .map_err(|error| Error(format!("Champ IR verification: {error}")))?;
        runtime.jit.emitted.insert(index, context.func.clone());
    }
    runtime.jit.module.define_function(id, &mut context)
        .map_err(|error| Error(format!("{}: {error:?}", source.symbol)))?;
    let compiled = context.compiled_code().expect("compiled Champ");
    let maps: Vec<_> = compiled.buffer.user_stack_maps().iter().map(|(ret, _, map)| {
        (*ret, map.entries().map(|(_, offset)| offset).collect::<Vec<_>>())
    }).collect();
    let frame = compiled.buffer.frame_layout().map_or(0, |layout| layout.frame_to_fp_offset);
    if u64::from(frame) > MAX_FRAME {
        return Err(Error("Champ machine frame exceeds headroom".into()));
    }
    let size = compiled.code_info().total_size;
    runtime.jit.module.finalize_definitions().map_err(|error| Error(format!("Champ finalize: {error}")))?;
    let address = runtime.jit.module.get_finalized_function(id) as u64;
    for (ret, offsets) in maps {
        runtime.jit.maps.push((address + u64::from(ret), frame, offsets));
    }
    runtime.jit.maps.sort_by_key(|(address, _, _)| *address);
    runtime.jit.ranges.push((address, address + u64::from(size)));
    runtime.jit.ranges.sort_unstable();
    runtime.jit.largest_frame = runtime.jit.largest_frame.max(frame);
    runtime.invocations[index].code = address;
    Ok(address)
}

struct Emit<'a, 'b> {
    builder: &'a mut FunctionBuilder<'b>,
    jit: &'a mut Jit,
    program: &'a s::Program,
    source: &'a s::Function,
    index: usize,
    /// The runtime's invocation table: a boxed slice, so its code words keep their addresses.
    invocations: *const runtime::Invocation,
    ctx: Value,
    io: Value,
    /// An owning entry (CallOwn in `io`); false for observations and async bodies.
    owned: bool,
    /// An async body's frame word, or zero. The frame is rooted by `Runtime::running` while its body runs.
    body: Value,
    /// An async body's frame map (draft 3 §2).
    frame_map: Option<&'a c::FrameMap>,
    /// Runtime ids of this function's D16 schedules.
    schedules: &'a [u64],
    storage: StackSlot,
    frame: &'a Frame,
    scope_base: usize,
    post: StackSlot,
    blocks: Vec<Block>,
    stop: Block,
    pc: Option<usize>,
    /// Whether a budget is set (`Limits::polled`): only then does machine code count steps.
    polled: bool,
    /// Address of the runtime's fuel cell.
    fuel: i64,
    plans: &'a [u64],
    selections: &'a [(u32, u64)],
    entries: &'a [u64],
    shielded_entries: &'a [u64],
    constants: &'a mut Vec<(Word, Repr)>,
    heap: &'a mut Heap,
    /// The values of the module constants (`machine::Program::globals`).
    globals: Vec<Word>,
}

impl Emit<'_, '_> {
    fn iconst(&mut self, value: u64) -> Value {
        self.builder.ins().iconst(types::I64, value as i64)
    }

    fn load(&mut self, pointer: Value, offset: usize) -> Value {
        self.builder.ins().load(types::I64, MEMORY, pointer, offset as i32)
    }

    fn store(&mut self, pointer: Value, offset: usize, value: Value) {
        self.builder.ins().store(MEMORY, value, pointer, offset as i32);
    }

    fn offset(&self, location: L) -> usize {
        self.frame.offsets[self.frame.allocation.locations[&location] as usize] as usize
    }

    fn width(&self, location: L) -> usize {
        match location {
            L::Register(_) => 1,
            L::Service(id) => s::layout::service_layout(self.source.services[id as usize]).words as usize,
        }
    }

    fn pointer(&mut self, location: L) -> Value {
        let offset = self.offset(location) as i32;
        self.builder.ins().stack_addr(types::I64, self.storage, offset)
    }

    fn read(&mut self, location: L) -> Vec<Value> {
        let offset = self.offset(location);
        (0..self.width(location)).map(|word| {
            self.builder.ins().stack_load(types::I64, types::I64, self.storage, (offset + word * 8) as i32)
        }).collect()
    }

    fn word(&mut self, location: L) -> Value {
        self.read(location)[0]
    }

    fn write(&mut self, location: L, values: &[Value]) {
        let offset = self.offset(location);
        for (word, &value) in values.iter().take(self.width(location)).enumerate() {
            self.builder.ins().stack_store(types::I64, value, self.storage,
                (offset + word * 8) as i32);
        }
    }

    fn record(&mut self, values: &[Value]) -> Value {
        let size = (values.len() * 8).max(8) as u32;
        let slot = self.builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, size, 3));
        for (index, &value) in values.iter().enumerate() {
            self.builder.ins().stack_store(types::I64, value, slot, (index * 8) as i32);
        }
        self.builder.ins().stack_addr(types::I64, slot, 0)
    }

    fn post(&mut self) -> Value {
        self.builder.ins().stack_addr(types::I64, self.post, 0)
    }

    fn call(&mut self, name: &str, arguments: &[Value]) -> Option<Value> {
        let index = HELPERS.iter().position(|(helper, _)| *helper == name).expect("registered helper");
        let id = self.jit.helpers.ids()[index];
        let collects = HELPERS[index].1.iter()
            .any(|effect| matches!(effect, Effect::Collects | Effect::Reenters));
        #[cfg(test)]
        if collects && name != "test_champ_safepoint" {
            self.call("test_champ_safepoint", &[self.ctx]);
        }
        let reference = self.jit.module.declare_func_in_func(id, self.builder.func);
        let instruction = self.builder.ins().call(reference, arguments);
        if collects {
            self.report_roots(instruction);
        }
        self.builder.inst_results(instruction).first().copied()
    }

    /// An observation call. A compiled callee with code is called directly, so a recursion costs one
    /// machine frame per level; otherwise the machine helper compiles it or reaches its provider.
    fn observe(&mut self, target: usize, values: &[Value]) -> Value {
        let ctx = self.ctx;
        let zero = self.iconst(0);
        let function = self.iconst(target as u64);
        let argc = self.iconst(values.len() as u64);
        if self.program.functions[target].code.is_empty() || values.len() > 8 {
            let argv = self.record(values);
            return self.call("champ_machine_external", &[ctx, function, zero, argv, argc]).unwrap();
        }
        let entry = self.invocations.wrapping_add(target) as u64;
        let word = self.iconst(entry + std::mem::offset_of!(runtime::Invocation, code) as u64);
        let code = self.load(word, 0);
        let direct = self.builder.create_block();
        let helper = self.builder.create_block();
        let done = self.builder.create_block();
        let result = self.builder.append_block_param(done, types::I64);
        self.builder.ins().brif(code, direct, &[], helper, &[]);
        self.builder.switch_to_block(direct);
        #[cfg(test)]
        self.call("test_champ_safepoint", &[ctx]);
        let signature = self.jit.signature(values.len());
        let signature = self.builder.import_signature(signature);
        let arguments = std::iter::once(ctx).chain(values.iter().copied()).collect::<Vec<_>>();
        let call = self.builder.ins().call_indirect(signature, code, &arguments);
        self.report_roots(call);
        let value = self.builder.inst_results(call)[0];
        self.builder.ins().jump(done, &[value.into()]);
        self.builder.switch_to_block(helper);
        let argv = self.record(values);
        let value = self.call("champ_machine_external", &[ctx, function, zero, argv, argc]).unwrap();
        self.builder.ins().jump(done, &[value.into()]);
        self.builder.switch_to_block(done);
        result
    }

    /// A self observation call in tail position (its result returned at once, directly or through
    /// one branch) in a function without service state: no scope, packet or cleanup outlives it.
    fn self_tail_call(&self, target: usize, flow: &s::Flow) -> bool {
        let (Some(pc), s::Flow::Next(outputs)) = (self.pc, flow) else {
            return false;
        };
        let [output] = outputs.as_slice() else {
            return false;
        };
        if target != self.index || self.source.entry_abi != s::EntryAbi::Observation
            || !self.source.services.is_empty()
            || self.source.code.iter().any(|instruction| matches!(instruction,
                s::Instruction::Champ { op, .. } if *op != O::InvokeObserve))
        {
            return false;
        }
        let returns = |pc: usize, value: L| matches!(self.source.code.get(pc),
            Some(s::Instruction::Ret(values)) if values.as_slice() == [value].map(|value| match value {
                L::Register(id) => id,
                L::Service(_) => u32::MAX,
            }));
        match self.source.code.get(pc + 1) {
            Some(s::Instruction::Ret(_)) => returns(pc + 1, *output),
            Some(s::Instruction::Br(edge)) => edge.assignments.iter().any(|assignment| {
                assignment.source == s::Source::Location(*output)
                    && returns(edge.target_pc as usize, assignment.destination)
            }),
            _ => false,
        }
    }

    fn enter_roots(&mut self) {
        let live = self.frame.before(self.source, self.pc);
        let zero = self.iconst(0);
        for offset in self.frame.traced.iter().filter(|offset| !live.contains(offset)) {
            self.builder.ins().stack_store(types::I64, zero, self.storage, *offset as i32);
        }
    }

    fn report_roots(&mut self, call: cranelift_codegen::ir::Inst) {
        for offset in self.frame.before(self.source, self.pc) {
            self.builder.func.dfg.append_user_stack_map_entry(call, cranelift_codegen::ir::UserStackMapEntry {
                ty: types::I64, slot: self.storage, offset,
            });
        }
    }

    fn checked(&mut self, name: &str, args: &[Value]) -> Value {
        let ordinal = self.call(name, args).expect("outcome helper");
        let stopped = self.builder.ins().icmp_imm_s(IntCC::Equal, ordinal, -1);
        let returned = self.builder.create_block();
        self.builder.ins().brif(stopped, self.stop, &[], returned, &[]);
        self.builder.switch_to_block(returned);
        ordinal
    }

    fn next(&mut self) {
        self.builder.ins().jump(self.blocks[self.pc.unwrap() + 1], &[]);
    }

    /// One budget step (`budget`), counted as Tomato counts them: below zero fuel calls `rt_poll`, and an
    /// exhausted budget or an interrupt stops the run (no handler catches it, no destroy runs).
    fn poll(&mut self) {
        let cell = self.iconst(self.fuel as u64);
        let fuel = self.load(cell, 0);
        let left = self.builder.ins().iadd_imm_s(fuel, -1);
        self.store(cell, 0, left);
        let empty = self.builder.ins().icmp_imm_s(IntCC::SignedLessThan, left, 0);
        let slow = self.builder.create_block();
        let next = self.builder.create_block();
        self.builder.set_cold_block(slow);
        self.builder.ins().brif(empty, slow, &[], next, &[]);
        self.builder.switch_to_block(slow);
        self.call("poll", &[self.ctx]);
        let status = self.load(self.ctx, std::mem::offset_of!(Ctx, status));
        self.builder.ins().brif(status, self.stop, &[], next, &[]);
        self.builder.switch_to_block(next);
    }

    fn edge(&mut self, edge: &s::Edge, produced: &[Vec<Value>]) {
        let pc = self.pc.expect("edge PC");
        if self.polled && edge.target_pc as usize <= pc {
            // A loop back-edge is a step.
            self.poll();
        }
        let ordinal = roots::outgoing(&self.source.code[pc]).iter()
            .position(|candidate| std::ptr::eq(*candidate, edge)).expect("source edge");
        for copy in &self.frame.allocation.edges[&(pc as u32, ordinal as u32)] {
            let values = match copy.source {
                s::CopySource::Slot(slot) => {
                    let offset = self.frame.offsets[slot as usize];
                    (0..self.frame.words[slot as usize]).map(|word| {
                        self.builder.ins().stack_load(types::I64, types::I64, self.storage,
                            (offset + word * 8) as i32)
                    }).collect::<Vec<_>>()
                }
                s::CopySource::Produced(slot) => produced[slot as usize].clone(),
            };
            let offset = self.frame.offsets[copy.destination as usize];
            for (word, value) in values.into_iter().enumerate() {
                self.builder.ins().stack_store(types::I64, value, self.storage, (offset + word as u32 * 8) as i32);
            }
        }
        self.builder.ins().jump(self.blocks[edge.target_pc as usize], &[]);
    }

    fn post_values(&mut self, op: O, tag: s::Tag, context: &s::ResultContext,
        helper: &str) -> Result<Vec<Vec<Value>>, Error>
    {
        let abi = c::helper_abi(helper).expect("operation ABI");
        let offsets = abi.post_offsets(tag, context).map_err(|error| Error(error.to_string()))?;
        let schema = op.results(tag, context).map_err(|error| Error(error.to_string()))?;
        let post = self.post();
        Ok(schema.iter().zip(offsets).map(|(ty, offset)| {
            let Some(offset) = offset else { return Vec::new() };
            let words = match ty {
                s::ProducedType::Service(kind) => s::layout::service_layout(*kind).words as usize,
                _ => 1,
            };
            (0..words).map(|word| self.load(post, offset + word * 8)).collect()
        }).collect())
    }

    fn flow(&mut self, op: O, flow: &s::Flow, arguments: &[A], ordinal: Value,
        helper: &str) -> Result<(), Error>
    {
        let context = s::result_context(self.program, self.source, op, arguments)
            .map_err(|error| Error(error.to_string()))?;
        match flow {
            s::Flow::Next(destinations) => {
                let valid = self.builder.create_block();
                let invalid = self.builder.create_block();
                self.builder.ins().brif(ordinal, invalid, &[], valid, &[]);
                self.builder.switch_to_block(invalid);
                self.call("champ_machine_invalid", &[self.ctx]);
                self.builder.ins().jump(self.stop, &[]);
                self.builder.switch_to_block(valid);
                let values = self.post_values(op, s::Tag::Normal, &context, helper)?;
                for (&destination, values) in destinations.iter().zip(values) {
                    self.write(destination, &values);
                }
                self.next();
            }
            s::Flow::Outcomes(outcomes) => {
                for (index, outcome) in outcomes.iter().enumerate() {
                    let arm = self.builder.create_block();
                    let other = self.builder.create_block();
                    let matches = self.builder.ins().icmp_imm_s(IntCC::Equal, ordinal, index as i64);
                    self.builder.ins().brif(matches, arm, &[], other, &[]);
                    self.builder.switch_to_block(arm);
                    let values = self.post_values(op, outcome.tag, &context, helper)?;
                    if outcome.tag == s::Tag::Unwind {
                        // Trace before capture, then make failure ordinary rooted data on the explicit edge.
                        let function = self.iconst(self.index as u64);
                        let pc = self.iconst(self.pc.unwrap() as u64);
                        let frame = self.builder.ins().get_frame_pointer(types::I64);
                        self.call("trace", &[self.ctx, function, pc, frame]);
                        let zero = self.iconst(0);
                        self.store(self.ctx, std::mem::offset_of!(Ctx, failure), zero);
                        self.store(self.ctx, std::mem::offset_of!(Ctx, status), zero);
                    }
                    self.edge(&outcome.edge, &values);
                    self.builder.switch_to_block(other);
                }
                self.call("champ_machine_invalid", &[self.ctx]);
                self.builder.ins().jump(self.stop, &[]);
            }
        }
        Ok(())
    }

    fn instruction(&mut self, instruction: &s::Instruction) -> Result<(), Error> {
        match instruction {
            s::Instruction::Br(edge) => self.edge(edge, &[]),
            s::Instruction::Cond { condition, yes, no } => {
                let condition = self.word(L::Register(*condition));
                let yes_block = self.builder.create_block();
                let no_block = self.builder.create_block();
                self.builder.ins().brif(condition, yes_block, &[], no_block, &[]);
                self.builder.switch_to_block(yes_block);
                self.edge(yes, &[]);
                self.builder.switch_to_block(no_block);
                self.edge(no, &[]);
            }
            s::Instruction::Ret(values) => {
                let value = match values.first() {
                    Some(&id) => self.word(L::Register(id)),
                    None => self.iconst(0),
                };
                self.builder.ins().return_(&[value]);
            }
            s::Instruction::Raise(id) => {
                let failure = self.word(L::Register(*id));
                self.store(self.ctx, std::mem::offset_of!(Ctx, failure), failure);
                let failed = self.iconst(FAILED);
                self.store(self.ctx, std::mem::offset_of!(Ctx, status), failed);
                if self.owned {
                    let post = self.load(self.io, 16);
                    let zero = self.iconst(0);
                    self.store(post, 0, zero);
                    self.store(post, 8, zero);
                    self.store(post, 16, failure);
                    let ordinal = self.iconst(match &self.source.entry_abi {
                        s::EntryAbi::Defer { labels } => labels.len() as u64 + 1,
                        _ => 1,
                    });
                    self.builder.ins().return_(&[ordinal]);
                } else {
                    let zero = self.iconst(0);
                    self.builder.ins().return_(&[zero]);
                }
            }
            s::Instruction::Unreachable => {
                self.call("champ_machine_invalid", &[self.ctx]);
                self.builder.ins().jump(self.stop, &[]);
            }
            s::Instruction::Data { op, inputs, output, immediate } => {
                self.data(*op, inputs, *output, immediate)?;
                if sets_status(*op) {
                    // A fallible data form loads only without services (S1). Leave exactly as a failing
                    // call without an unwind outcome does, with the failure (or the stop) still set.
                    let status = self.load(self.ctx, std::mem::offset_of!(Ctx, status));
                    let ok = self.builder.create_block();
                    self.builder.ins().brif(status, self.stop, &[], ok, &[]);
                    self.builder.switch_to_block(ok);
                }
                self.next();
            }
            s::Instruction::Champ { op, arguments, flow } => self.operation(*op, arguments, flow)?,
        }
        Ok(())
    }

    fn operation(&mut self, op: O, arguments: &[A], flow: &s::Flow) -> Result<(), Error> {
        let locations: Vec<_> = arguments.iter().filter_map(|arg| match arg {
            A::Location(location) => Some(*location),
            _ => None,
        }).collect();
        let zero = self.iconst(0);
        let post = self.post();
        // A mapped Post would make forced GC read an invalid pointer before the helper writes it.
        #[cfg(test)]
        {
            let poison = self.iconst(0xdead_beef_0000);
            for offset in (0..64).step_by(8) {
                self.store(post, offset, poison);
            }
        }
        let ctx = self.ctx;
        let helper;
        let mut args = vec![ctx];
        let entry = |arg: &A| if let A::Entry(id) = arg { Some(*id) } else { None };
        let boundary = |arg: &A| if let A::Boundary(id) = arg { Some(*id) } else { None };
        let selection = |arg: &A| if let A::Selection(id) = arg { Some(*id) } else { None };
        match op {
            O::Catch => {
                let failure = self.word(locations[0]);
                let descriptor = arguments.iter().find_map(|arg| match arg { A::Object(id) => Some(*id), _ => None })
                    .map_or(u64::MAX, u64::from);
                let descriptor = self.iconst(descriptor);
                let payload = self.call("champ_machine_catch", &[ctx, failure, descriptor]).unwrap();
                let yes = self.builder.create_block();
                let no = self.builder.create_block();
                let s::Flow::Outcomes(outcomes) = flow else { unreachable!() };
                self.builder.ins().brif(payload, yes, &[], no, &[]);
                self.builder.switch_to_block(yes);
                self.edge(&outcomes[0].edge, &[vec![payload]]);
                self.builder.switch_to_block(no);
                self.edge(&outcomes[1].edge, &[]);
                return Ok(());
            }
            O::Evaluate => {
                let A::Evaluation(evaluation) = &arguments[0] else { unreachable!() };
                let inputs = locations.iter().map(|location| match location {
                    L::Register(id) => *id, _ => unreachable!("verified scalar operands"),
                }).collect::<Vec<_>>();
                let value = self.data_value(evaluation.op, &inputs, evaluation.result, &evaluation.immediate)?;
                let s::Flow::Outcomes(outcomes) = flow else { unreachable!() };
                let status = self.load(ctx, std::mem::offset_of!(Ctx, status));
                let success = self.builder.create_block();
                let failure = self.builder.create_block();
                self.builder.ins().brif(status, failure, &[], success, &[]);
                self.builder.switch_to_block(success);
                self.edge(&outcomes[0].edge, &[vec![value]]);
                self.builder.switch_to_block(failure);
                let failed = self.builder.ins().icmp_imm_s(IntCC::Equal, status, FAILED as i64);
                let unwind = self.builder.create_block();
                self.builder.ins().brif(failed, unwind, &[], self.stop, &[]);
                self.builder.switch_to_block(unwind);
                let function = self.iconst(self.index as u64);
                let pc = self.iconst(self.pc.unwrap() as u64);
                let frame = self.builder.ins().get_frame_pointer(types::I64);
                self.call("trace", &[ctx, function, pc, frame]);
                let failure = self.load(ctx, std::mem::offset_of!(Ctx, failure));
                self.store(ctx, std::mem::offset_of!(Ctx, status), zero);
                self.store(ctx, std::mem::offset_of!(Ctx, failure), zero);
                self.edge(&outcomes[1].edge, &[vec![failure]]);
                return Ok(());
            }
            O::Fail => {
                let kind = arguments.iter().find_map(|arg| match arg { A::Label(id) => Some(*id), _ => None })
                    .unwrap();
                let kind = self.iconst(u64::from(kind));
                let payload = self.word(locations[0]);
                let failure = self.call("champ_machine_fail", &[ctx, kind, payload]).unwrap();
                self.store(ctx, std::mem::offset_of!(Ctx, failure), failure);
                let failed = self.iconst(FAILED);
                self.store(ctx, std::mem::offset_of!(Ctx, status), failed);
                let function = self.iconst(self.index as u64);
                let pc = self.iconst(self.pc.unwrap() as u64);
                let frame = self.builder.ins().get_frame_pointer(types::I64);
                self.call("trace", &[ctx, function, pc, frame]);
                self.store(ctx, std::mem::offset_of!(Ctx, status), zero);
                self.store(ctx, std::mem::offset_of!(Ctx, failure), zero);
                let s::Flow::Outcomes(outcomes) = flow else { unreachable!("verified fail outcome") };
                self.edge(&outcomes[0].edge, &[vec![failure]]);
                return Ok(());
            }
            O::ArrayGet => {
                let s::Flow::Outcomes(outcomes) = flow else { unreachable!("verified array outcomes") };
                // A byte array (`@byte`) is packed: one byte per element after the header, read as a raw byte.
                let bytes = matches!(locations[0],
                    L::Register(id) if self.source.registers[id as usize] == s::Type::Bytes);
                let array = self.word(locations[0]);
                let index = self.word(locations[1]);
                let header = self.load(array, 0);
                let length = self.builder.ins().ushr_imm_u(header, 32);
                let small = self.builder.ins().band_imm_u(index, 1);
                let position = self.builder.ins().sshr_imm_u(index, 1);
                let inside = self.builder.ins().icmp(IntCC::UnsignedLessThan, position, length);
                let inside = self.builder.ins().uextend(types::I64, inside);
                let valid = self.builder.ins().band(small, inside);
                let load = self.builder.create_block();
                let outside = self.builder.create_block();
                self.builder.ins().brif(valid, load, &[], outside, &[]);
                self.builder.switch_to_block(load);
                let value = if bytes {
                    let pointer = self.builder.ins().iadd(array, position);
                    self.builder.ins().uload8(types::I64, MEMORY, pointer, 8)
                } else {
                    let offset = self.builder.ins().ishl_imm_u(position, 3);
                    let pointer = self.builder.ins().iadd(array, offset);
                    self.load(pointer, 8)
                };
                self.edge(&outcomes[0].edge, &[vec![value]]);
                self.builder.switch_to_block(outside);
                let message = self.heap.allocate_text("Array index out of bounds");
                self.constants.push((message, Repr::Ref));
                let message = self.iconst(message);
                self.call("trap", &[ctx, message]);
                let status = self.load(ctx, std::mem::offset_of!(Ctx, status));
                let failed = self.builder.ins().icmp_imm_s(IntCC::Equal, status, FAILED as i64);
                let unwind = self.builder.create_block();
                self.builder.ins().brif(failed, unwind, &[], self.stop, &[]);
                self.builder.switch_to_block(unwind);
                let function = self.iconst(self.index as u64);
                let pc = self.iconst(self.pc.unwrap() as u64);
                let frame = self.builder.ins().get_frame_pointer(types::I64);
                self.call("trace", &[ctx, function, pc, frame]);
                let failure = self.load(ctx, std::mem::offset_of!(Ctx, failure));
                self.store(ctx, std::mem::offset_of!(Ctx, status), zero);
                self.store(ctx, std::mem::offset_of!(Ctx, failure), zero);
                self.edge(&outcomes[1].edge, &[vec![failure]]);
                return Ok(());
            }
            O::Lookup => {
                let s::Flow::Outcomes(outcomes) = flow else { unreachable!("verified lookup outcomes") };
                let collection = self.word(locations[0]);
                let index = self.word(locations[1]);
                let function = self.iconst(self.index as u64);
                let pc = self.iconst(self.pc.unwrap() as u64);
                let found = self.call("champ_machine_lookup", &[ctx, function, pc, collection, index, post]).unwrap();
                let status = self.load(ctx, std::mem::offset_of!(Ctx, status));
                let running = self.builder.create_block();
                self.builder.ins().brif(status, self.stop, &[], running, &[]);
                self.builder.switch_to_block(running);
                let yes = self.builder.create_block();
                let no = self.builder.create_block();
                self.builder.ins().brif(found, yes, &[], no, &[]);
                self.builder.switch_to_block(yes);
                let value = self.load(post, 0);
                self.edge(&outcomes[0].edge, &[vec![value]]);
                self.builder.switch_to_block(no);
                self.edge(&outcomes[1].edge, &[]);
                return Ok(());
            }
            O::BuilderPush | O::BuilderAppend | O::BuilderRange | O::BuilderFinish | O::BuilderAbandon => {
                let (&packet, operands) = locations.split_last().expect("verified journal packet");
                let words = operands.iter().map(|&location| self.word(location)).collect::<Vec<_>>();
                let record = self.record(&words);
                let count = self.iconst(words.len() as u64);
                let function = self.iconst(self.index as u64);
                let pc = self.iconst(self.pc.unwrap() as u64);
                let value = self.call("champ_machine_builder", &[ctx, function, pc, record, count]).unwrap();
                // The journal leaves with the buffer, as own.move moves a packet record. Both are read
                // after the helper's safepoint, from their rooted frame slots.
                let journal = self.read(packet);
                let buffer = self.word(operands[0]);
                match flow {
                    s::Flow::Next(destinations) => {
                        let status = self.load(ctx, std::mem::offset_of!(Ctx, status));
                        let running = self.builder.create_block();
                        self.builder.ins().brif(status, self.stop, &[], running, &[]);
                        self.builder.switch_to_block(running);
                        self.write(destinations[0], &journal);
                        self.next();
                    }
                    s::Flow::Outcomes(outcomes) => {
                        let status = self.load(ctx, std::mem::offset_of!(Ctx, status));
                        let success = self.builder.create_block();
                        let failure = self.builder.create_block();
                        self.builder.ins().brif(status, failure, &[], success, &[]);
                        self.builder.switch_to_block(success);
                        self.edge(&outcomes[0].edge, &[vec![value], journal.clone()]);
                        self.builder.switch_to_block(failure);
                        let failed = self.builder.ins().icmp_imm_s(IntCC::Equal, status, FAILED as i64);
                        let unwind = self.builder.create_block();
                        self.builder.ins().brif(failed, unwind, &[], self.stop, &[]);
                        self.builder.switch_to_block(unwind);
                        let function = self.iconst(self.index as u64);
                        let pc = self.iconst(self.pc.unwrap() as u64);
                        let frame = self.builder.ins().get_frame_pointer(types::I64);
                        self.call("trace", &[ctx, function, pc, frame]);
                        let failure = self.load(ctx, std::mem::offset_of!(Ctx, failure));
                        self.store(ctx, std::mem::offset_of!(Ctx, status), zero);
                        self.store(ctx, std::mem::offset_of!(Ctx, failure), zero);
                        self.edge(&outcomes[1].edge, &[vec![buffer], journal, vec![failure]]);
                    }
                }
                return Ok(());
            }
            O::ScopeBegin => {
                let scope = arguments.iter().find_map(|arg| match arg {
                    A::Scope(id) => Some(*id), _ => None,
                }).unwrap();
                let parent = self.program.tables.scopes[scope as usize].parent;
                let parent_owner = parent.map_or(zero, |id| self.scope_word(id, 8));
                let function = self.iconst(self.index as u64);
                let lexical = self.iconst(scope as u64);
                let ordinal = self.checked("champ_machine_scope", &[ctx, function, lexical, parent_owner, post]);
                let state = self.load(post, 0);
                let destination = self.scope_pointer(scope);
                self.checked("champ_machine_destination", &[ctx, state, destination]);
                return self.flow(op, flow, arguments, ordinal, "champ_scope_begin");
            }
            O::InvokeObserve | O::InvokeDispatch => {
                let (value, has_result) = if op == O::InvokeDispatch {
                    let (function, count, has_result) = self.dispatch(arguments, locations[0], |_, function| {
                        function as u64
                    });
                    let values = locations[..count].iter().map(|&loc| self.word(loc)).collect::<Vec<_>>();
                    let argv = self.record(&values);
                    let argc = self.iconst(count as u64);
                    let value = self.call("champ_machine_external", &[ctx, function, zero, argv, argc]).unwrap();
                    (value, has_result)
                } else {
                    let target = arguments.iter().find_map(entry).unwrap() as usize;
                    let callee = &self.program.functions[target];
                    let count = callee.params.iter().filter(|location| matches!(location, L::Register(_))).count();
                    let has_result = !callee.results.is_empty();
                    let values = locations[..count].iter().map(|&loc| self.word(loc)).collect::<Vec<_>>();
                    if self.self_tail_call(target, flow) {
                        // A self call whose result is returned at once reuses this frame: a loop.
                        let params = self.source.params.iter().filter(|location| matches!(location, L::Register(_)));
                        for (&location, &value) in params.zip(&values).collect::<Vec<_>>() {
                            self.write(location, &[value]);
                        }
                        self.builder.ins().jump(self.blocks[0], &[]);
                        return Ok(());
                    }
                    (self.observe(target, &values), has_result)
                };
                let status = self.load(ctx, std::mem::offset_of!(Ctx, status));
                let success = self.builder.create_block();
                let failure = self.builder.create_block();
                self.builder.ins().brif(status, failure, &[], success, &[]);
                self.builder.switch_to_block(success);
                match flow {
                    s::Flow::Next(outputs) => {
                        if let Some(&output) = outputs.first() {
                            self.write(output, &[value]);
                        }
                        self.next();
                    }
                    s::Flow::Outcomes(outcomes) => {
                        let edge = &outcomes.iter().find(|outcome| outcome.tag == s::Tag::Normal).unwrap().edge;
                        let produced = if has_result { vec![vec![value]] } else { vec![] };
                        self.edge(edge, &produced);
                    }
                }
                self.builder.switch_to_block(failure);
                let failed = self.builder.ins().icmp_imm_s(IntCC::Equal, status, FAILED as i64);
                let unwind = self.builder.create_block();
                self.builder.ins().brif(failed, unwind, &[], self.stop, &[]);
                self.builder.switch_to_block(unwind);
                if let s::Flow::Outcomes(outcomes) = flow
                    && let Some(outcome) = outcomes.iter().find(|outcome| outcome.tag == s::Tag::Unwind)
                {
                    let function = self.iconst(self.index as u64);
                    let pc = self.iconst(self.pc.unwrap() as u64);
                    let frame = self.builder.ins().get_frame_pointer(types::I64);
                    self.call("trace", &[ctx, function, pc, frame]);
                    let failure = self.load(ctx, std::mem::offset_of!(Ctx, failure));
                    self.store(ctx, std::mem::offset_of!(Ctx, status), zero);
                    self.store(ctx, std::mem::offset_of!(Ctx, failure), zero);
                    self.edge(&outcome.edge, &[vec![failure]]);
                } else {
                    self.builder.ins().jump(self.stop, &[]);
                }
                return Ok(());
            }
            O::DepsOf | O::DepsEmpty => {
                // The result has a definition and proof dependencies, but no machine storage.
                self.next();
                return Ok(());
            }
            O::OwnEmpty => helper = "champ_own_empty",
            O::OwnMove => {
                helper = "champ_own_move";
                args.push(self.pointer(locations[0]));
            }
            O::OwnJoin => {
                helper = "champ_own_join";
                let words = locations.iter().map(|&location| self.word(location)).collect::<Vec<_>>();
                args.push(self.record(&words));
                args.push(self.iconst(words.len() as u64));
            }
            O::OwnAdmit | O::OwnAccept => {
                helper = if op == O::OwnAccept { "champ_own_accept" } else { "champ_own_admit" };
                args.push(self.pointer(locations[0]));
                args.push(self.pointer(locations[1]));
            }
            O::OwnBind | O::OwnRebind => {
                helper = if op == O::OwnBind { "champ_own_bind" } else { "champ_own_rebind" };
                let binding = arguments.iter().find_map(|arg| match arg {
                    A::Binding(id) => Some(*id), _ => None,
                }).unwrap();
                args.push(self.pointer(locations[0]));
                args.push(self.iconst(binding as u64));
                args.push(self.word(locations[1]));
                args.push(self.pointer(locations[2]));
            }
            O::OwnCreateCommit => {
                helper = "champ_own_create_commit";
                let descriptor = arguments.iter().find_map(|arg| match arg {
                    A::Object(id) => Some(*id), _ => None,
                }).unwrap();
                let boundary = arguments.iter().find_map(boundary).unwrap();
                let scope = self.program.tables.boundaries[boundary as usize].scope;
                let owner = self.scope_word(scope, 8);
                args.push(self.word(locations[0]));
                args.push(self.iconst(descriptor as u64));
                args.push(owner);
            }
            O::OwnSplit | O::OwnSplitChecked | O::ScopeSplit => {
                args.push(self.pointer(locations[0]));
                if op == O::OwnSplit {
                    helper = "champ_own_split_static";
                    args.push(self.static_selection()?);
                } else {
                    helper = if op == O::ScopeSplit && arguments.contains(&A::Flag(true)) {
                        "champ_scope_split_admissible"
                    } else if op == O::ScopeSplit {
                        "champ_scope_split"
                    } else { "champ_own_split_checked" };
                    let id = arguments.iter().find_map(selection).unwrap();
                    args.push(self.selection_record(id));
                }
            }
            O::OwnDynamic => {
                helper = "champ_own_dynamic";
                args.push(self.pointer(locations[0]));
                args.push(self.pointer(locations[1]));
            }
            O::ScopeTransfer => {
                helper = if s::transfer_admissible(arguments) {
                    "champ_scope_transfer_admissible"
                } else {
                    "champ_scope_transfer"
                };
                let between = arguments.first() == Some(&A::Flag(true));
                let from = self.pointer(locations[0]);
                let to = if between { self.pointer(locations[1]) } else { from };
                args.push(self.record(&[from, to]));
                let id = arguments.iter().find_map(selection).unwrap();
                args.push(self.selection_record(id));
            }
            O::OwnStage => {
                helper = "champ_own_stage";
                let between = arguments.first() == Some(&A::Flag(true));
                let state = self.pointer(locations[0]);
                let source = if between { self.pointer(locations[1]) } else { zero };
                let field = arguments.iter().find_map(|arg| match arg {
                    A::Field(id) => Some(*id), _ => None,
                }).unwrap();
                let field = self.iconst(field as u64);
                let value = self.word(locations[1 + usize::from(between)]);
                let packet = self.pointer(*locations.last().unwrap());
                let plan = if between { self.static_selection()? } else { zero };
                args.push(self.record(&[state, source, plan, field, value, packet]));
            }
            O::DeferRegister => {
                // Registration is explicit CFG metadata. No runtime defer stack exists.
                let value = self.word(locations[0]);
                let s::Flow::Next(outputs) = flow else { unreachable!() };
                self.write(locations[0], &[zero]);
                self.write(outputs[0], &[value]);
                self.next();
                return Ok(());
            }
            O::ExitPrepare => {
                helper = "champ_exit_prepare";
                let kind = arguments.iter().find_map(|arg| match arg {
                    A::Kind(kind) => Some(*kind as u64), _ => None,
                }).unwrap();
                let kind = self.iconst(kind);
                let boundary = self.iconst(arguments.iter().find_map(boundary).unwrap() as u64);
                let (value, repr) = self.optional_value(&locations[..locations.len() - 1]);
                let packet = self.pointer(*locations.last().unwrap());
                args.push(self.record(&[kind, boundary, value, repr, packet, zero]));
            }
            O::ExitExtend => {
                helper = "champ_exit_extend";
                args.push(self.pointer(locations[0]));
                args.push(self.pointer(locations[1]));
            }
            O::ExitReplace => {
                helper = "champ_exit_replace";
                let boundary = self.iconst(arguments.iter().find_map(boundary).unwrap() as u64);
                let (value, repr) = self.optional_value(&locations[1..locations.len() - 1]);
                let packet = self.pointer(*locations.last().unwrap());
                let plan = self.static_selection()?;
                let replacement = self.record(&[value, repr, packet, plan, boundary]);
                args.push(self.pointer(locations[0]));
                args.push(replacement);
            }
            O::ExitCommit => {
                helper = "champ_exit_commit";
                args.push(self.pointer(locations[0]));
                let destination = arguments.iter().find_map(|arg| match arg {
                    A::Destination(destination) => Some(*destination), _ => None,
                }).unwrap();
                let scope = match destination {
                    s::ExitDestination::Caller => None,
                    s::ExitDestination::Loop(id) => Some(self.program.tables.boundaries[id as usize].scope),
                    s::ExitDestination::Scope(id) => Some(id),
                };
                let destination = if let Some(scope) = scope {
                    self.scope_pointer(scope)
                } else if self.frame_map.is_some() {
                    // An async body returns into its Future's cache (ABI C-2).
                    let destination = self.record(&[zero, zero]);
                    self.checked("champ_machine_future_destination", &[ctx, self.body, destination]);
                    destination
                } else {
                    let destination = self.record(&[zero, zero]);
                    self.checked("champ_machine_destination", &[ctx, zero, destination]);
                    destination
                };
                args.push(destination);
            }
            O::ExitAbort => {
                helper = "champ_exit_abort";
                args.push(self.pointer(locations[0]));
                args.push(self.pointer(locations[1]));
            }
            O::CleanupBegin => {
                helper = "champ_cleanup_begin";
                let source = self.pointer(locations[0]);
                let scope = matches!(locations[0], L::Service(id)
                    if self.source.services[id as usize] == s::ServiceKind::ScopeState);
                let mut pending = zero;
                let mut failure = zero;
                for &location in &locations[1..] {
                    match location {
                        L::Service(_) => pending = self.pointer(location),
                        L::Register(_) => failure = self.word(location),
                    }
                }
                let inputs = if scope { [source, zero, pending, failure, zero] }
                    else { [zero, source, pending, failure, zero] };
                args.push(self.record(&inputs));
            }
            // Futures, suspension brackets and duty settles (#601 V9, `futures.rs`).
            O::FutureNew | O::Await | O::FuturePublish | O::FutureCancel | O::FutureDone | O::FutureCancelled
                | O::FrameSave | O::FrameResume | O::AwaitUnwind | O::CleanupDeliverCancel => {
                return self.future_operation(op, arguments, &locations, flow);
            }
            O::CleanupSettle if locations.len() > 1 => {
                return self.future_operation(op, arguments, &locations, flow);
            }
            // The temporary take, its proof and the rehome (#601 R4) have no helpers before the runtime of #601 R5.
            O::InvokeDefer if arguments.iter().filter_map(entry).any(|target| {
                matches!(self.program.functions[target as usize].entry_abi, s::EntryAbi::AsyncDefer { .. })
            }) => {
                return Err(Error("Champ invoke.defer of an async defer entry runs with #601 R5".into()));
            }
            O::AwaitTake | O::UniqueCacheProof | O::CacheRehome => {
                return Err(Error(format!("Champ {} runs with the async resources runtime (#601 R5)", op.name())));
            }
            O::CleanupSettle => {
                helper = "champ_cleanup_settle_empty";
                args.push(self.pointer(locations[0]));
            }
            O::CleanupPlan => {
                helper = "champ_cleanup_plan";
                let id = arguments.iter().find_map(|arg| match arg {
                    A::Plan(id) => Some(*id), _ => None,
                }).unwrap();
                args.push(self.pointer(locations[0]));
                args.push(self.iconst(self.plans[id as usize]));
            }
            O::CleanupNext | O::CleanupFinish => {
                helper = if op == O::CleanupNext { "champ_cleanup_next" } else { "champ_cleanup_finish" };
                args.push(self.pointer(locations[0]));
            }
            O::CleanupFailure => {
                helper = "champ_cleanup_failure";
                args.push(self.pointer(locations[0]));
                args.push(self.word(locations[1]));
            }
            O::CleanupDestroy | O::DestroyDirect => {
                helper = if op == O::DestroyDirect { "champ_destroy_direct" } else { "champ_cleanup_destroy" };
                args.push(self.pointer(locations[0]));
                if op == O::DestroyDirect {
                    let object = self.word(locations[1]);
                    let serial = self.call("champ_machine_serial", &[ctx, object]).unwrap();
                    args.extend([object, serial]);
                    let target = arguments.iter().find_map(entry).unwrap();
                    args.push(self.iconst(target as u64));
                } else {
                    args.extend(self.read(locations[1]));
                }
            }
            O::InvokeCallOwn | O::InvokeDefer | O::InvokeDispatchOwn => {
                helper = if op == O::InvokeDefer { "champ_invoke_defer" } else { "champ_call_owned" };
                let shielded = op == O::InvokeDefer || arguments.contains(&A::Flag(true));
                let (entry_id, count) = if op == O::InvokeDispatchOwn {
                    let (entry_id, count, _) = self.dispatch(arguments, locations[0], |emit, function| {
                        let entries = if shielded { emit.shielded_entries } else { emit.entries };
                        entries[function as usize]
                    });
                    (entry_id, count)
                } else {
                    let target = arguments.iter().find_map(entry).unwrap() as usize;
                    let count = self.program.functions[target].params.iter()
                        .filter(|location| matches!(location, L::Register(_))).count();
                    let entry = if shielded { self.shielded_entries[target] } else { self.entries[target] };
                    (self.iconst(entry), count)
                };
                let values = locations[..count].iter().map(|&location| self.word(location)).collect::<Vec<_>>();
                let argv = self.record(&values);
                let argc = self.iconst(count as u64);
                let packet = self.word(locations[count]);
                let mode = self.iconst(u64::from(shielded));
                let io = self.record(&[packet, entry_id, post, zero, mode]);
                args.extend([entry_id, argv, argc, io]);
                if op == O::InvokeDefer {
                    let target = arguments.iter().find_map(entry).unwrap() as usize;
                    let replacements = match &self.program.functions[target].entry_abi {
                        s::EntryAbi::Defer { labels } => labels.len(),
                        _ => unreachable!(),
                    };
                    let replacements = self.iconst(replacements as u64);
                    args.push(self.record(&[replacements]));
                }
            }
            O::RetOwn | O::RetDefer => {
                let label = arguments.iter().find_map(|arg| match arg {
                    A::Label(id) => Some(*id), _ => None,
                });
                let ordinal = if op == O::RetOwn { 0 } else {
                    match (&self.source.entry_abi, label) {
                        (s::EntryAbi::Defer { labels }, Some(label)) => {
                            labels.iter().position(|item| item.label == label).unwrap() as u64 + 1
                        }
                        _ => 0,
                    }
                };
                let packet = locations.iter().find(|location| matches!(location, L::Service(id)
                    if self.source.services[*id as usize] == s::ServiceKind::Packet)).copied();
                let value = locations.iter().find(|location| matches!(location, L::Register(_))).copied();
                if op == O::RetOwn && s::plain_results(self.program, self.source)
                    && let Some(packet) = packet
                {
                    let pointer = self.pointer(packet);
                    self.checked("champ_machine_plain_packet", &[ctx, pointer]);
                }
                let packet = packet.map_or(zero, |location| self.word(location));
                let value = value.map_or(zero, |location| self.word(location));
                let output = self.load(self.io, 16);
                self.store(output, 0, value);
                self.store(output, 8, packet);
                self.store(output, 16, zero);
                self.store(self.io, 0, zero);
                let ordinal = self.iconst(ordinal);
                self.builder.ins().return_(&[ordinal]);
                return Ok(());
            }
            O::FailureDispatch | O::ExitDispatch => {
                let failure = self.word(locations[usize::from(op == O::ExitDispatch)]);
                let has_failure = self.builder.ins().icmp_imm_s(IntCC::NotEqual, failure, 0);
                let s::Flow::Outcomes(outcomes) = flow else { unreachable!() };
                let yes = self.builder.create_block();
                let no = self.builder.create_block();
                self.builder.ins().brif(has_failure, yes, &[], no, &[]);
                for (block, failed) in [(yes, true), (no, false)] {
                    self.builder.switch_to_block(block);
                    let tag = if op == O::FailureDispatch {
                        if failed { s::Tag::Some } else { s::Tag::None }
                    } else if failed {
                        s::Tag::Failure
                    } else if arguments.iter().filter_map(|arg| match arg {
                        A::Flag(flag) => Some(*flag), _ => None,
                    }).next().unwrap() {
                        s::Tag::Pending
                    } else {
                        s::Tag::Success
                    };
                    if let Some(outcome) = outcomes.iter().find(|outcome| outcome.tag == tag) {
                        let values = match tag {
                            s::Tag::Some => vec![vec![failure]],
                            s::Tag::Failure => vec![vec![], vec![failure]],
                            s::Tag::Pending => vec![vec![], self.read(locations[2])],
                            s::Tag::Success => vec![vec![]],
                            _ => vec![],
                        };
                        self.edge(&outcome.edge, &values);
                    } else {
                        self.call("champ_machine_invalid", &[ctx]);
                        self.builder.ins().jump(self.stop, &[]);
                    }
                }
                return Ok(());
            }
        }
        args.push(post);
        let ordinal = self.checked(helper, &args);
        // Retire copied inputs before the next safepoint. CallOwn owns its packet now.
        if matches!(op, O::InvokeCallOwn | O::InvokeDefer | O::InvokeDispatchOwn | O::OwnJoin) {
            for &location in &locations {
                if matches!(location, L::Service(id) if self.source.services[id as usize] == s::ServiceKind::Packet) {
                    self.write(location, &[zero]);
                    // An owning entry that passes its input packet on without accepting it (a closure
                    // trampoline) has handed it over: every outcome of the entry leaves io.input consumed.
                    if self.source.entry_abi != s::EntryAbi::Observation && self.source.params.contains(&location) {
                        self.store(self.io, 0, zero);
                    }
                }
            }
        }
        if op == O::OwnAccept && self.owned {
            self.store(self.io, 0, zero);
        }
        self.flow(op, flow, arguments, ordinal, helper)
    }

    /// Selects the implementation of a dispatch by the receiver's object descriptor: `select` maps each
    /// target's implementation function to the word the call needs (its index or its entry ID). A receiver
    /// outside the verified targets stops the machine. Returns the word, the language argument count
    /// (receiver first) and whether the slot has a result.
    fn dispatch(&mut self, arguments: &[A], receiver: L, select: impl Fn(&Self, u32) -> u64)
        -> (Value, usize, bool)
    {
        let Some(A::Dispatch(dispatch)) = arguments.first() else { unreachable!("verified dispatch") };
        let slot = &self.program.objects[dispatch.role as usize].role.as_ref().expect("verified role")
            .slots[dispatch.slot as usize];
        let (count, has_result) = (1 + slot.params.len(), !slot.results.is_empty());
        let object = self.word(receiver);
        let header = self.load(object, 0);
        let descriptor = self.builder.ins().ushr_imm_u(header, 32);
        let mut selected = self.iconst(u64::MAX);
        for &target in &dispatch.targets {
            let function = s::implementation(self.program, target, dispatch.role, dispatch.slot)
                .expect("verified implementation");
            let word = self.iconst(select(self, function));
            let matches = self.builder.ins().icmp_imm_s(IntCC::Equal, descriptor, i64::from(target));
            selected = self.builder.ins().select(matches, word, selected);
        }
        let missing = self.builder.ins().icmp_imm_s(IntCC::Equal, selected, -1);
        let outside = self.builder.create_block();
        let selected_block = self.builder.create_block();
        self.builder.ins().brif(missing, outside, &[], selected_block, &[]);
        self.builder.switch_to_block(outside);
        self.call("champ_machine_invalid", &[self.ctx]);
        self.builder.ins().jump(self.stop, &[]);
        self.builder.switch_to_block(selected_block);
        (selected, count, has_result)
    }

    fn selection_record(&mut self, id: u32) -> Value {
        let roots = self.source.proofs.selections[id as usize].locations.iter()
            .map(|&location| self.word(location)).collect::<Vec<_>>();
        let pointer = self.record(&roots);
        let count = self.iconst(roots.len() as u64);
        self.record(&[pointer, count])
    }

    fn optional_value(&mut self, locations: &[L]) -> (Value, Value) {
        let Some(&location) = locations.first() else {
            let zero = self.iconst(0);
            return (zero, zero);
        };
        let L::Register(id) = location else { unreachable!() };
        let value = self.word(location);
        let repr = self.iconst(machine::repr(self.source.registers[id as usize]) as u64);
        (value, repr)
    }

    fn static_selection(&mut self) -> Result<Value, Error> {
        let id = self.selections.iter().find(|(pc, _)| Some(*pc as usize) == self.pc)
            .ok_or_else(|| {
                Error(format!("missing static selection at {}:{}", self.source.symbol, self.pc.unwrap()))
            })?.1;
        Ok(self.iconst(id))
    }

    fn scope_pointer(&mut self, scope: u32) -> Value {
        let offset = self.scope_base + scope as usize * 16;
        self.builder.ins().stack_addr(types::I64, self.storage, offset as i32)
    }

    fn scope_word(&mut self, scope: u32, offset: usize) -> Value {
        let pointer = self.scope_pointer(scope);
        let existing = self.load(pointer, offset);
        let ready = self.builder.create_block();
        let initialize = self.builder.create_block();
        let present = self.builder.ins().icmp_imm_s(IntCC::NotEqual, existing, 0);
        self.builder.ins().brif(present, ready, &[], initialize, &[]);
        self.builder.switch_to_block(initialize);
        let zero = self.iconst(0);
        self.checked("champ_machine_destination", &[self.ctx, zero, pointer]);
        self.builder.ins().jump(ready, &[]);
        self.builder.switch_to_block(ready);
        self.load(pointer, offset)
    }

    /// A collection operation the runtime decodes from this instruction (`rt_machine_collection`).
    fn collection(&mut self, words: &[Value]) -> Value {
        let arguments = self.record(words);
        let count = self.iconst(words.len() as u64);
        let function = self.iconst(self.index as u64);
        let pc = self.iconst(self.pc.expect("collection PC") as u64);
        self.call("champ_machine_collection", &[self.ctx, function, pc, arguments, count]).unwrap()
    }

    fn data(&mut self, op: s::DataOp, inputs: &[u32], output: Option<u32>, immediate: &s::Immediate)
        -> Result<(), Error>
    {
        let ty = output.map_or(s::Type::Never, |id| self.source.registers[id as usize]);
        let value = self.data_value(op, inputs, ty, immediate)?;
        if let Some(output) = output {
            self.write(L::Register(output), &[value]);
        }
        Ok(())
    }

    fn data_value(&mut self, op: s::DataOp, inputs: &[u32], ty: s::Type, immediate: &s::Immediate)
        -> Result<Value, Error>
    {
        use s::DataOp as D;
        let words = inputs.iter().map(|&id| self.word(L::Register(id))).collect::<Vec<_>>();
        let value = match op {
            D::Const => {
                let s::Immediate::Constant(value) = immediate else { return Err(Error("invalid constant".into())) };
                let word = c::collections::constant_word(self.heap, value).map_err(Error)?;
                self.constants.push((word, machine::repr(ty)));
                self.iconst(word)
            }
            // Built before the program ran and kept as a runtime constant: the word itself.
            D::GlobalGet => {
                let s::Immediate::Global(index) = immediate else { return Err(Error("invalid global".into())) };
                self.iconst(self.globals[*index as usize])
            }
            D::FailSecondary | D::FailCaused => {
                let helper = if op == D::FailSecondary { "champ_machine_secondary" } else { "champ_machine_caused" };
                self.call(helper, &[self.ctx, words[0], words[1]]).unwrap()
            }
            D::EnumCase => {
                let s::Immediate::Updates(indices) = immediate else { unreachable!() };
                let tag = self.heap.int(i64::from(indices[1]));
                let object = self.heap.object_new(indices[0], &[tag]);
                self.constants.push((object, Repr::Ref));
                self.iconst(object)
            }
            D::Move => words[0],
            D::Undef => self.iconst(0),
            D::StrConcat => self.call("concat", &[self.ctx, words[0], words[1]]).unwrap(),
            D::StrLen => self.call("champ_machine_text_len", &[self.ctx, words[0]]).unwrap(),
            D::IntEq | D::IntNe | D::IntLt | D::IntLe | D::IntGt | D::IntGe
                | D::StrEq | D::StrNe | D::StrLt | D::StrLe | D::StrGt | D::StrGe => {
                let helper = if matches!(op, D::IntEq | D::IntNe | D::IntLt | D::IntLe | D::IntGt | D::IntGe) {
                    "int_order"
                } else { "champ_machine_text_order" };
                let order = self.call(helper, &[self.ctx, words[0], words[1]]).unwrap();
                let condition = match op {
                    D::IntEq | D::StrEq => IntCC::Equal,
                    D::IntNe | D::StrNe => IntCC::NotEqual,
                    D::IntLt | D::StrLt => IntCC::SignedLessThan,
                    D::IntLe | D::StrLe => IntCC::SignedLessThanOrEqual,
                    D::IntGt | D::StrGt => IntCC::SignedGreaterThan,
                    _ => IntCC::SignedGreaterThanOrEqual,
                };
                let result = self.builder.ins().icmp_imm_s(condition, order, 0);
                self.builder.ins().uextend(types::I64, result)
            }
            D::RangeNew | D::RangeStart | D::RangeEnd | D::RangeLen => {
                let kind = self.iconst(match op {
                    D::RangeNew => 0, D::RangeStart => 1, D::RangeEnd => 2, _ => 3,
                });
                let right = words.get(1).copied().unwrap_or_else(|| self.iconst(0));
                let inclusive = self.iconst(u64::from(*immediate == s::Immediate::Inclusive(true)));
                self.call("champ_machine_range", &[self.ctx, kind, words[0], right, inclusive]).unwrap()
            }
            D::ArrayNew if ty == s::Type::Bytes => self.collection(&words),
            D::ValEq | D::ValNe if !matches!(self.source.registers[inputs[0] as usize], s::Type::Object(_)) => {
                self.collection(&words)
            }
            D::ArrayNew => {
                let s::Type::Array(shape) = ty else {
                    unreachable!("verified plain array type")
                };
                let repr = self.iconst(machine::repr(shape.element().unwrap()) as u64);
                let elements = self.record(&words);
                let count = self.iconst(words.len() as u64);
                self.call("array", &[self.ctx, repr, elements, count]).unwrap()
            }
            D::MapNew | D::MapLen | D::MapHas | D::MapGet | D::MapKeys | D::MapValues | D::MapKeyAt
                | D::MapValueAt | D::MapMerge | D::MapDifference | D::ArrayConcat | D::ArrayJoin | D::ArraySlice
                | D::ArrayRemove | D::ArrayReplace | D::ArrayDifference | D::BuilderNew => self.collection(&words),
            D::ArrayLen => {
                let header = self.load(words[0], 0);
                let length = self.builder.ins().ushr_imm_u(header, 32);
                let doubled = self.builder.ins().ishl_imm_u(length, 1);
                self.builder.ins().bor_imm_u(doubled, 1)
            }
            D::ObjNew | D::ObjNext => {
                let descriptor = match ty {
                    s::Type::Object(id) | s::Type::Successor(id) => id,
                    _ => unreachable!("verified construction"),
                };
                let layout = self.heap.object_runtime_layout(descriptor).clone();
                let count = self.iconst(layout.allocation_words() as u64);
                let pointer = self.call("allocate", &[self.ctx, count]).unwrap();
                let null = self.builder.ins().icmp_imm_s(IntCC::Equal, pointer, 0);
                let allocated = self.builder.create_block();
                self.builder.ins().brif(null, self.stop, &[], allocated, &[]);
                self.builder.switch_to_block(allocated);
                let header = self.iconst(layout.header(descriptor));
                self.store(pointer, 0, header);
                let zero = self.iconst(0);
                for word in 1..layout.allocation_words() {
                    self.store(pointer, word * 8, zero);
                }
                if let s::Immediate::Updates(indices) = immediate {
                    for index in 0..layout.storage_field_count() {
                        let word = if let Some(update) = indices.iter().position(|&field| field as usize == index) {
                            words[update + 1]
                        } else {
                            self.load(words[0], (index + 1) * 8)
                        };
                        self.store(pointer, (index + 1) * 8, word);
                    }
                } else {
                    for (index, &word) in words.iter().enumerate() {
                        self.store(pointer, (index + 1) * 8, word);
                    }
                }
                pointer
            }
            D::ObjGet => {
                let s::Immediate::Field(field) = immediate else { unreachable!() };
                self.load(words[0], (*field as usize + 1) * 8)
            }
            D::ObjInit => {
                let s::Immediate::Field(field) = immediate else { unreachable!() };
                self.store(words[0], (*field as usize + 1) * 8, words[1]);
                words[0]
            }
            D::IntAdd | D::IntSub | D::IntMul | D::IntMod | D::IntNeg => {
                let op = match op {
                    D::IntAdd => runtime::INT_ADD,
                    D::IntSub => runtime::INT_SUB,
                    D::IntMul => runtime::INT_MUL,
                    D::IntMod => runtime::INT_MOD,
                    _ => runtime::INT_SUB,
                };
                let opcode = self.iconst(op);
                let right = words.get(1).copied().unwrap_or_else(|| self.iconst(0));
                let (left, right) = if op == runtime::INT_SUB && words.len() == 1 {
                    (self.iconst(1), words[0])
                } else {
                    (words[0], right)
                };
                self.call("int", &[self.ctx, opcode, left, right]).unwrap()
            }
            D::Convert => {
                let from = self.source.registers[inputs[0] as usize];
                let codes = machine::scalar_code(from).zip(machine::scalar_code(ty))
                    .ok_or_else(|| Error("Champ conversion outside scalar types".into()))?;
                let from = self.iconst(codes.0);
                let to = self.iconst(codes.1);
                self.call("champ_machine_convert", &[self.ctx, from, to, words[0]]).unwrap()
            }
            D::ByteEq | D::ByteNe | D::ByteLt | D::ByteLe | D::ByteGt | D::ByteGe
                | D::CharEq | D::CharNe | D::CharLt | D::CharLe | D::CharGt | D::CharGe => {
                let condition = match op {
                    D::ByteEq | D::CharEq => IntCC::Equal,
                    D::ByteNe | D::CharNe => IntCC::NotEqual,
                    D::ByteLt | D::CharLt => IntCC::UnsignedLessThan,
                    D::ByteLe | D::CharLe => IntCC::UnsignedLessThanOrEqual,
                    D::ByteGt | D::CharGt => IntCC::UnsignedGreaterThan,
                    _ => IntCC::UnsignedGreaterThanOrEqual,
                };
                let result = self.builder.ins().icmp(condition, words[0], words[1]);
                self.builder.ins().uextend(types::I64, result)
            }
            D::FloatEq | D::FloatNe | D::FloatLt | D::FloatLe | D::FloatGt | D::FloatGe => {
                let condition = match op {
                    D::FloatEq => FloatCC::Equal,
                    D::FloatNe => FloatCC::NotEqual,
                    D::FloatLt => FloatCC::LessThan,
                    D::FloatLe => FloatCC::LessThanOrEqual,
                    D::FloatGt => FloatCC::GreaterThan,
                    _ => FloatCC::GreaterThanOrEqual,
                };
                let left = self.builder.ins().bitcast(types::F64, MemFlagsData::new(), words[0]);
                let right = self.builder.ins().bitcast(types::F64, MemFlagsData::new(), words[1]);
                let result = self.builder.ins().fcmp(condition, left, right);
                self.builder.ins().uextend(types::I64, result)
            }
            D::RatEq | D::RatNe | D::RatLt | D::RatLe | D::RatGt | D::RatGe => {
                let order = self.call("champ_machine_rat_order", &[self.ctx, words[0], words[1]]).unwrap();
                let condition = match op {
                    D::RatEq => IntCC::Equal,
                    D::RatNe => IntCC::NotEqual,
                    D::RatLt => IntCC::SignedLessThan,
                    D::RatLe => IntCC::SignedLessThanOrEqual,
                    D::RatGt => IntCC::SignedGreaterThan,
                    _ => IntCC::SignedGreaterThanOrEqual,
                };
                let result = self.builder.ins().icmp_imm_s(condition, order, 0);
                self.builder.ins().uextend(types::I64, result)
            }
            D::RatNew => self.call("rat_new", &[self.ctx, words[0], words[1]]).unwrap(),
            D::EnumEq | D::EnumNe => {
                // A plain enum value is an object whose only storage word is its tagged case number.
                let left = self.load(words[0], 8);
                let right = self.load(words[1], 8);
                let condition = if op == D::EnumEq { IntCC::Equal } else { IntCC::NotEqual };
                let result = self.builder.ins().icmp(condition, left, right);
                self.builder.ins().uextend(types::I64, result)
            }
            D::ValEq | D::ValNe => {
                let ty = self.source.registers[inputs[0] as usize];
                let (s::Type::Object(descriptor) | s::Type::Role(descriptor)) = ty else {
                    return Err(Error("Champ value equality outside objects".into()));
                };
                let descriptor = self.iconst(u64::from(descriptor));
                let equal = self.call("champ_machine_value_eq", &[self.ctx, descriptor, words[0], words[1]]).unwrap();
                if op == D::ValEq { equal } else { self.builder.ins().bxor_imm_s(equal, 1) }
            }
            D::VariantTag => {
                let s::Type::Variant(variant) = self.source.registers[inputs[0] as usize] else {
                    return Err(Error("Champ variant.tag outside variants".into()));
                };
                let header = self.load(words[0], 0);
                let descriptor = self.builder.ins().ushr_imm_u(header, 32);
                let mut tag = self.iconst(0);
                for (case, &object) in self.program.objects[variant as usize].variant_cases.iter().enumerate() {
                    let active = self.builder.ins().icmp_imm_s(IntCC::Equal, descriptor, i64::from(object));
                    let number = self.iconst(case as u64);
                    tag = self.builder.ins().select(active, number, tag);
                }
                let doubled = self.builder.ins().ishl_imm_u(tag, 1);
                self.builder.ins().bor_imm_u(doubled, 1)
            }
            D::VariantGet => {
                let (s::Type::Variant(variant), s::Immediate::Updates(indices)) =
                    (self.source.registers[inputs[0] as usize], immediate) else {
                    return Err(Error("Champ variant.get outside variants".into()));
                };
                let case = self.program.objects[variant as usize].variant_cases[indices[0] as usize];
                let case = self.iconst(u64::from(case));
                let storage = self.iconst(u64::from(indices[1]));
                self.call("champ_machine_variant_get", &[self.ctx, case, storage, words[0]]).unwrap()
            }
            D::RoleNarrow => {
                let s::Type::Role(role) = ty else {
                    return Err(Error("Champ role narrowing outside role types".into()));
                };
                let role = self.iconst(u64::from(role));
                self.call("champ_machine_role_narrow", &[self.ctx, role, words[0]]).unwrap()
            }
            D::TestOk | D::TestIs | D::TestIsnt | D::TestFail => {
                let kind = self.iconst(match op {
                    D::TestOk => 0,
                    D::TestIs => 1,
                    D::TestIsnt => 2,
                    _ => 3,
                });
                let function = self.iconst(self.index as u64);
                let register = self.iconst(u64::from(inputs[0]));
                let right = words.get(1).copied().unwrap_or_else(|| self.iconst(0));
                self.call("champ_machine_test", &[self.ctx, kind, function, register, words[0], right]).unwrap()
            }
            D::FloatNeg | D::FloatAbs => {
                let value = self.builder.ins().bitcast(types::F64, MemFlagsData::new(), words[0]);
                let result = if op == D::FloatNeg {
                    self.builder.ins().fneg(value)
                } else {
                    self.builder.ins().fabs(value)
                };
                self.builder.ins().bitcast(types::I64, MemFlagsData::new(), result)
            }
            D::ByteAnd => self.builder.ins().band(words[0], words[1]),
            D::ByteOr => self.builder.ins().bor(words[0], words[1]),
            D::ByteXor => self.builder.ins().bxor(words[0], words[1]),
            D::ByteNot => self.builder.ins().bxor_imm_u(words[0], 0xff),
            D::BoolNot => self.builder.ins().bxor_imm_s(words[0], 1),
            D::BoolEq | D::BoolNe => {
                let condition = if op == D::BoolEq { IntCC::Equal } else { IntCC::NotEqual };
                let result = self.builder.ins().icmp(condition, words[0], words[1]);
                self.builder.ins().uextend(types::I64, result)
            }
            _ => {
                // Every other scalar family operation runs Tomato's semantics in one helper; it writes its
                // result word last, after every root it may collect around.
                let code = op.scalar_code()
                    .ok_or_else(|| Error("Champ data operation without a translation".into()))?;
                let code = self.iconst(code);
                let zero = self.iconst(0);
                let operand = |index: usize| words.get(index).copied().unwrap_or(zero);
                let arguments = [self.ctx, code, operand(0), operand(1), operand(2)];
                self.call("champ_machine_scalar", &arguments).unwrap()
            }
        };
        Ok(value)
    }
}

/// Data helpers that can return with a non-OK status: an application failure (`DataOp::fallible`)
/// or a terminal stop such as an exhausted memory budget. Their result is then not a value.
fn sets_status(op: s::DataOp) -> bool {
    use s::DataOp as D;
    matches!(op, D::IntMod | D::RangeNew | D::Convert | D::IntMul | D::StrConcat | D::RatNew | D::ValEq | D::ValNe
        | D::VariantGet | D::RoleNarrow | D::MapNew | D::MapLen | D::MapHas | D::MapGet | D::MapKeys | D::MapValues
        | D::MapKeyAt
        | D::MapValueAt | D::MapMerge | D::MapDifference | D::ArrayConcat | D::ArrayJoin | D::ArraySlice
        | D::ArrayRemove | D::ArrayReplace | D::ArrayDifference | D::BuilderNew | D::ArrayNew | D::TestOk
        | D::TestIs | D::TestIsnt | D::TestFail)
        || op.scalar_code().is_some() && !inline_scalar(op)
}

/// Scalar family operations that machine code computes without a helper.
fn inline_scalar(op: s::DataOp) -> bool {
    use s::DataOp as D;
    matches!(op, D::FloatNeg | D::FloatAbs | D::ByteAnd | D::ByteOr | D::ByteXor | D::ByteNot)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod roles_tests;
