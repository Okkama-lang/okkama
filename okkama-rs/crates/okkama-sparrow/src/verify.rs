//! Проверка программы до исполнения: ссылки таблиц, регистры, переходы и
//! обработчики, завершённость кода, и что каждый читаемый регистр записан на
//! всех путях к чтению (прямой анализ потока данных над битовыми множествами),
//! и типы операндов каждой инструкции (`typing`).
use crate::{Access, Function, Instruction, Program, Register, Target, Type, VerifyError};

fn program_error(message: impl Into<String>) -> VerifyError {
    VerifyError {
        function: None,
        instruction: None,
        message: message.into(),
    }
}

struct Context<'a> {
    program: &'a Program,
    function: &'a Function,
}

impl Context<'_> {
    fn error(&self, instruction: Option<usize>, message: impl Into<String>) -> VerifyError {
        VerifyError {
            function: Some(self.function.symbol.clone()),
            instruction,
            message: message.into(),
        }
    }

    fn register(&self, pc: Option<usize>, register: Register) -> Result<(), VerifyError> {
        if register.0 as usize >= self.function.registers.len() {
            return Err(self.error(pc, format!("register r{} is out of bounds", register.0)));
        }
        Ok(())
    }

    fn target(&self, pc: Option<usize>, target: &Target) -> Result<(), VerifyError> {
        if target.pc as usize >= self.function.code.len() {
            return Err(self.error(pc, format!("jump target @{} is out of bounds", target.pc)));
        }
        let mut seen = std::collections::HashSet::new();
        for (dst, source) in &target.moves {
            self.register(pc, *dst)?;
            self.register(pc, *source)?;
            if !seen.insert(*dst) {
                return Err(self.error(pc, format!("register r{} is assigned twice by one transfer", dst.0)));
            }
        }
        Ok(())
    }
}

fn check_type_ref(program: &Program, index: u32, own: Option<usize>) -> Result<(), VerifyError> {
    let limit = own.unwrap_or(program.types.len());
    if index as usize >= limit {
        return Err(program_error(format!("type t{index} is out of bounds")));
    }
    Ok(())
}

fn check_types(program: &Program) -> Result<(), VerifyError> {
    for (index, ty) in program.types.iter().enumerate() {
        let earlier = |ty: crate::TypeRef| check_type_ref(program, ty.0, Some(index));
        match ty {
            Type::Object(object) | Type::Successor(object) => {
                if object.0 as usize >= program.objects.len() {
                    return Err(program_error(format!("type t{index} names missing object obj{}", object.0)));
                }
            }
            Type::Role(role) => {
                if role.0 as usize >= program.roles.len() {
                    return Err(program_error(format!("type t{index} names missing role role{}", role.0)));
                }
            }
            Type::Enum(id) => {
                if id.0 as usize >= program.enums.len() {
                    return Err(program_error(format!("type t{index} names missing enum enum{}", id.0)));
                }
            }
            Type::Handle(handle) => {
                if *handle as usize >= program.handles.len() {
                    return Err(program_error(format!("type t{index} names missing handle {handle}")));
                }
            }
            Type::Cap(cap) => {
                if cap.0 as usize >= program.caps.len() {
                    return Err(program_error(format!("type t{index} names missing cap cap{}", cap.0)));
                }
            }
            Type::Variant(variant) => {
                if variant.0 as usize >= program.variants.len() {
                    return Err(program_error(format!("type t{index} names missing variant variant{}", variant.0)));
                }
            }
            Type::Array(item) | Type::Builder(item) => earlier(*item)?,
            Type::Map(key, value) | Type::MapBuilder(key, value) => {
                earlier(*key)?;
                earlier(*value)?;
            }
            Type::Fn { params, result } => {
                for param in params {
                    earlier(*param)?;
                }
                if let Some(result) = result {
                    earlier(*result)?;
                }
            }
            Type::Future(Some(result)) => earlier(*result)?,
            _ => {}
        }
    }
    Ok(())
}

fn check_function_ref(program: &Program, function: u32) -> Result<(), VerifyError> {
    if function as usize >= program.functions.len() {
        return Err(program_error(format!("function f{function} is out of bounds")));
    }
    Ok(())
}

fn check_tables(program: &Program) -> Result<(), VerifyError> {
    check_types(program)?;
    for role in &program.roles {
        for method in &role.methods {
            for param in &method.params {
                check_type_ref(program, param.0, None)?;
            }
            if let Some(result) = method.result {
                check_type_ref(program, result.0, None)?;
            }
        }
    }
    for object in &program.objects {
        for (_, ty) in &object.fields {
            check_type_ref(program, ty.0, None)?;
        }
        for witness in &object.does {
            let Some(role) = program.roles.get(witness.role.0 as usize) else {
                return Err(program_error(format!("object {} does missing role role{}", object.name, witness.role.0)));
            };
            if witness.slots.len() != role.methods.len() {
                return Err(program_error(format!("object {} witness of {} has wrong slot count", object.name, role.name)));
            }
            for slot in &witness.slots {
                check_function_ref(program, slot.0)?;
            }
        }
        if let Some(destroy) = object.destroy {
            check_function_ref(program, destroy.0)?;
        }
    }
    // Случай варианта — свой объект раскладки (#290): существующий, не
    // встроенный, без `destroy`, не общий с другим случаем.
    let mut cases = std::collections::HashSet::new();
    for variant in &program.variants {
        if variant.cases.is_empty() {
            return Err(program_error(format!("variant {} has no cases", variant.name)));
        }
        for (name, object) in &variant.cases {
            let plain = program
                .objects
                .get(object.0 as usize)
                .is_some_and(|declared| !declared.builtin && declared.destroy.is_none());
            if !plain || !cases.insert(*object) {
                return Err(program_error(format!("variant {} case {name} needs its own plain object", variant.name)));
            }
        }
    }
    for cap in &program.caps {
        if cap.owner.0 as usize >= program.objects.len() {
            return Err(program_error(format!("cap {} owner is missing", cap.name)));
        }
        for (_, function) in &cap.fields {
            check_function_ref(program, function.0)?;
        }
    }
    for (index, global) in program.globals.iter().enumerate() {
        // The type is resolved by typing; an index past the table panicked there (#974).
        if global.ty.0 as usize >= program.types.len() {
            return Err(program_error(format!("global g{index} has type t{}, which is out of bounds", global.ty.0)));
        }
        check_function_ref(program, global.init.0)?;
    }
    if let Some(entry) = program.entry {
        check_function_ref(program, entry.0)?;
    }
    for (_, function) in program.tests.iter().flatten() {
        check_function_ref(program, function.0)?;
    }
    Ok(())
}

/// Ссылки и регистры одной инструкции.
fn check_instruction(context: &Context<'_>, pc: usize, instruction: &Instruction) -> Result<(), VerifyError> {
    let program = context.program;
    let mut failure = None;
    instruction.visit(&mut |access| {
        if failure.is_some() {
            return;
        }
        let result = match access {
            Access::Use(register) | Access::Def(register) => context.register(Some(pc), register),
            Access::Target(target) => context.target(Some(pc), target),
            Access::Function(function) => check_function_ref(program, function.0).map_err(|e| context.error(Some(pc), e.message)),
            Access::Object(object) if object.0 as usize >= program.objects.len() => {
                Err(context.error(Some(pc), format!("object obj{} is out of bounds", object.0)))
            }
            Access::Role(role) if role.0 as usize >= program.roles.len() => {
                Err(context.error(Some(pc), format!("role role{} is out of bounds", role.0)))
            }
            Access::Enum(id) if id.0 as usize >= program.enums.len() => {
                Err(context.error(Some(pc), format!("enum enum{} is out of bounds", id.0)))
            }
            Access::Cap(cap) if cap.0 as usize >= program.caps.len() => {
                Err(context.error(Some(pc), format!("cap cap{} is out of bounds", cap.0)))
            }
            Access::Variant(variant) if variant.0 as usize >= program.variants.len() => {
                Err(context.error(Some(pc), format!("variant variant{} is out of bounds", variant.0)))
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            failure = Some(error);
        }
    });
    if let Some(error) = failure {
        return Err(error);
    }
    match instruction {
        Instruction::EnumCase { ty, case, .. } => {
            if *case as usize >= program.enums[ty.0 as usize].cases.len() {
                return Err(context.error(Some(pc), format!("enum case {case} is out of bounds")));
            }
        }
        Instruction::CallRole { role, slot, .. } => {
            if *slot as usize >= program.roles[role.0 as usize].methods.len() {
                return Err(context.error(Some(pc), format!("role slot {slot} is out of bounds")));
            }
        }
        Instruction::CallCap { ty, field, .. } => {
            if *field as usize >= program.caps[ty.0 as usize].fields.len() {
                return Err(context.error(Some(pc), format!("cap field {field} is out of bounds")));
            }
        }
        Instruction::ObjNew { object, fields, .. } => {
            if fields.len() != program.objects[object.0 as usize].fields.len() {
                return Err(context.error(Some(pc), "obj.new field count does not match the object"));
            }
        }
        Instruction::VariantNew { variant, case, fields, .. } => {
            let Some((_, object)) = program.variants[variant.0 as usize].cases.get(*case as usize) else {
                return Err(context.error(Some(pc), format!("variant.new case {case} is out of bounds")));
            };
            if fields.len() != program.objects[object.0 as usize].fields.len() {
                return Err(context.error(Some(pc), "variant.new field count does not match the case"));
            }
        }
        Instruction::Catch { kind, ty, .. } => {
            let limit = match kind {
                crate::CatchKind::Object => program.objects.len(),
                crate::CatchKind::Role => program.roles.len(),
                crate::CatchKind::Variant => program.variants.len(),
            };
            if *ty as usize >= limit {
                return Err(context.error(Some(pc), format!("catch type {ty} is out of bounds")));
            }
        }
        Instruction::Bits { op, count, .. } => {
            // The OIR verifier's two texts (#1070), one per mistake (#1097).
            match (matches!(op, crate::BitOp::RotateLeft | crate::BitOp::RotateRight), count.is_some()) {
                (true, false) => return Err(context.error(Some(pc), "a rotation needs a count")),
                (false, true) => return Err(context.error(Some(pc), "only rotations take a count")),
                _ => {}
            }
        }
        _ => {}
    }
    Ok(())
}

/// Битовое множество регистров.
#[derive(Clone, PartialEq, Eq)]
struct Set(Vec<u64>);

impl Set {
    fn empty(size: usize) -> Self {
        Set(vec![0; size.div_ceil(64)])
    }
    fn insert(&mut self, register: Register) {
        self.0[register.0 as usize / 64] |= 1 << (register.0 % 64);
    }
    fn remove(&mut self, register: Register) {
        self.0[register.0 as usize / 64] &= !(1 << (register.0 % 64));
    }
    fn contains(&self, register: Register) -> bool {
        self.0[register.0 as usize / 64] & (1 << (register.0 % 64)) != 0
    }
    /// Пересечение; `true`, если множество изменилось.
    fn intersect(&mut self, other: &Set) -> bool {
        let mut changed = false;
        for (word, other) in self.0.iter_mut().zip(&other.0) {
            let next = *word & other;
            changed |= next != *word;
            *word = next;
        }
        changed
    }
}

/// What the initialization analysis knows at one point (#1069).
///
/// `undef` writes a stub (unbound storage), so its register counts as written,
/// but the stub may only be carried, never used: copied by `move` and edge
/// moves, or stored by `obj.new`/`obj.init`, as the compiler places a
/// constructor's fields not bound yet. Any other read needs a binding check:
/// a `bool` register, the guard, that is `false` wherever the stub may still be
/// there, so its `true` branch proves the register bound. That is the shape the
/// compiler emits for a field bound on some arms and for the last value of a
/// `for` in value position (`ran`, `last`).
#[derive(Clone, PartialEq, Eq)]
struct State {
    /// Registers written on every path.
    written: Set,
    /// `bool` registers that hold `false` on every path (a `const false`, a
    /// copy of one, the `else` edge of a branch on it). A false register
    /// guards every stub.
    falses: Set,
    /// Registers that may hold a stub.
    stubs: Set,
    /// Further guards of a stub, beyond `falses`: sorted, disjoint from
    /// `falses`, at most [`MAX_GUARDS`], never empty.
    guards: std::collections::BTreeMap<u32, Vec<u32>>,
}

/// Guards kept per stub besides the false registers. Dropping one only makes
/// the analysis stricter; the compiler's checks need one.
const MAX_GUARDS: usize = 4;

/// The registers of a set, in order.
fn members(set: &Set) -> impl Iterator<Item = u32> + '_ {
    set.0.iter().enumerate().flat_map(|(index, word)| {
        let mut bits = *word;
        std::iter::from_fn(move || {
            (bits != 0).then(|| {
                let bit = bits.trailing_zeros();
                bits &= bits - 1;
                index as u32 * 64 + bit
            })
        })
    })
}

/// The first [`MAX_GUARDS`] registers of two sorted lists together.
fn union_head(left: impl Iterator<Item = u32>, right: impl Iterator<Item = u32>) -> Vec<u32> {
    let mut all: Vec<u32> = left.take(MAX_GUARDS).chain(right.take(MAX_GUARDS)).collect();
    all.sort_unstable();
    all.dedup();
    all.truncate(MAX_GUARDS);
    all
}

impl State {
    fn new(size: usize) -> Self {
        State {
            written: Set::empty(size),
            falses: Set::empty(size),
            stubs: Set::empty(size),
            guards: Default::default(),
        }
    }

    /// Guard entries this state stores (for the memory bound).
    fn entries(&self) -> usize {
        self.guards.values().map(Vec::len).sum::<usize>() + self.guards.len()
    }

    fn explicit(&self, register: u32) -> &[u32] {
        self.guards.get(&register).map_or(&[], Vec::as_slice)
    }

    fn set_guards(&mut self, register: u32, mut guards: Vec<u32>) {
        guards.retain(|guard| *guard != register && !self.falses.contains(Register(*guard)));
        guards.sort_unstable();
        guards.dedup();
        guards.truncate(MAX_GUARDS);
        if guards.is_empty() {
            self.guards.remove(&register);
        } else {
            self.guards.insert(register, guards);
        }
    }

    /// `register` is overwritten: it holds no stub, is no longer known false and guards nothing.
    fn clobber(&mut self, register: Register) {
        self.written.insert(register);
        self.falses.remove(register);
        self.stubs.remove(register);
        self.guards.remove(&register.0);
        self.guards.retain(|_, guards| {
            guards.retain(|guard| *guard != register.0);
            !guards.is_empty()
        });
    }

    /// A def by an instruction: `undef` writes a stub, `move` copies what its source is known to hold,
    /// `const false` is a false register.
    fn define(&mut self, register: Register, instruction: &Instruction) {
        let (stub, falsy) = match instruction {
            Instruction::Undef { .. } => (Some(Vec::new()), false),
            Instruction::Move { source, .. } => (
                self.stubs.contains(*source).then(|| self.explicit(source.0).to_vec()),
                self.falses.contains(*source),
            ),
            Instruction::Const {
                value: crate::Constant::Bool(false),
                ..
            } => (None, true),
            _ => (None, false),
        };
        self.clobber(register);
        if falsy {
            self.falses.insert(register);
        }
        if let Some(guards) = stub {
            self.stubs.insert(register);
            self.set_guards(register.0, guards);
        }
    }

    /// Parallel edge moves: each destination gets what its source held before the edge, and a guard
    /// moved along with its stub keeps guarding it under the destination's name.
    fn transfer(&mut self, moves: &[(Register, Register)]) {
        if moves.is_empty() {
            return;
        }
        let before = self.clone();
        for (dst, _) in moves {
            self.clobber(*dst);
        }
        for (dst, source) in moves {
            if before.falses.contains(*source) {
                self.falses.insert(*dst);
            }
        }
        for (dst, source) in moves {
            if !before.stubs.contains(*source) {
                continue;
            }
            // A guard keeps its value unless an edge move overwrites it, and travels with every copy.
            let mut renamed = Vec::new();
            for guard in before.explicit(source.0).iter().copied().chain(members(&before.falses)) {
                if !moves.iter().any(|(dst, _)| dst.0 == guard) {
                    renamed.push(guard);
                }
                renamed.extend(moves.iter().filter(|(_, source)| source.0 == guard).map(|(dst, _)| dst.0));
            }
            self.stubs.insert(*dst);
            self.set_guards(dst.0, renamed);
        }
    }

    /// The `then` edge of `br guard`: every stub the guard proves bound is bound.
    fn holds(&mut self, guard: Register) {
        if self.falses.contains(guard) {
            // The edge is never taken.
            self.stubs = Set::empty(self.stubs.0.len() * 64);
            self.guards.clear();
            return;
        }
        let bound: Vec<u32> = self.guards.iter().filter(|(_, guards)| guards.contains(&guard.0)).map(|(register, _)| *register).collect();
        for register in bound {
            self.stubs.remove(Register(register));
            self.guards.remove(&register);
        }
    }

    /// The `else` edge of `br guard`: the guard is false there.
    fn fails(&mut self, guard: Register) {
        self.falses.insert(guard);
        for guards in self.guards.values_mut() {
            guards.retain(|other| *other != guard.0);
        }
        self.guards.retain(|_, guards| !guards.is_empty());
    }

    /// Meet with another path; `true`, if this state changed. A stub's guards on the meet are those
    /// it has on both paths, a path where it is bound granting every guard; the result never has more
    /// guards than this state had, so the analysis terminates.
    fn meet(&mut self, other: &State) -> bool {
        let mut falses = self.falses.clone();
        falses.intersect(&other.falses);
        let mut stubs = self.stubs.clone();
        for (word, other) in stubs.0.iter_mut().zip(&other.stubs.0) {
            *word |= other;
        }
        // False on one path only: still a guard of a stub that is there on that path alone.
        let only = |set: &Set| {
            let mut only = set.clone();
            for (word, common) in only.0.iter_mut().zip(&falses.0) {
                *word &= !common;
            }
            only
        };
        let (mine_only, theirs_only) = (only(&self.falses), only(&other.falses));
        let mut guards = std::collections::BTreeMap::new();
        for register in members(&stubs) {
            let (mine, theirs) = (self.stubs.contains(Register(register)), other.stubs.contains(Register(register)));
            let (left, right) = (self.explicit(register), other.explicit(register));
            let next: Vec<u32> = match (mine, theirs) {
                (true, true) => left
                    .iter()
                    .copied()
                    .filter(|guard| right.contains(guard) || theirs_only.contains(Register(*guard)))
                    .chain(right.iter().copied().filter(|guard| mine_only.contains(Register(*guard))))
                    .collect(),
                (true, false) => union_head(left.iter().copied(), members(&mine_only)),
                (false, true) => union_head(right.iter().copied(), members(&theirs_only)),
                (false, false) => unreachable!(),
            };
            let mut next: Vec<u32> = next.into_iter().filter(|guard| !falses.contains(Register(*guard))).collect();
            next.sort_unstable();
            next.dedup();
            next.truncate(MAX_GUARDS);
            if !next.is_empty() {
                guards.insert(register, next);
            }
        }
        let mut written = self.written.clone();
        written.intersect(&other.written);
        let next = State {
            written,
            falses,
            stubs,
            guards,
        };
        let changed = next != *self;
        *self = next;
        changed
    }
}

/// Reads that carry a stub without using it: `move`, and the fields `obj.new` and `obj.init` store.
fn carries(instruction: &Instruction, register: Register) -> bool {
    match instruction {
        Instruction::Move { .. } => true,
        Instruction::ObjNew { fields, .. } => fields.contains(&register),
        Instruction::ObjInit { object, value, .. } => *value == register && *object != register,
        _ => false,
    }
}

/// The object fields a stub of `undef` may reach (#1076), per object: `true` at `[object][field]`, if some
/// `obj.new` or `obj.init` may store a stub there. A stub reaches a field only so ([`carries`]); `obj.next`
/// copies fields within one object type, so the answer per type covers every copy. A register may hold a
/// stub, if it is an `undef` destination or a `move`, edge move or handler move copies one into it, on
/// any path: flow-insensitive, so it can only say `true` more often than [`check_initialization`] lets a
/// stub through, never less. Linear in the code (and a sort of the copies). Sparrow checks reads of these
/// fields for the stub.
pub(crate) fn stub_fields(program: &Program) -> Vec<Vec<bool>> {
    let mut fields: Vec<Vec<bool>> = program.objects.iter().map(|_| Vec::new()).collect();
    let mut mark = |object: usize, field: usize| {
        let count = program.objects.get(object).map_or(0, |declared| declared.fields.len());
        if field < count {
            fields[object].resize(count, false);
            fields[object][field] = true;
        }
    };
    for function in &program.functions {
        // Most functions have no `undef`: they are passed over without looking at their copies.
        if !function.code.iter().any(|instruction| matches!(instruction, Instruction::Undef { .. })) {
            continue;
        }
        let mut stub = vec![false; function.registers.len()];
        let mut work = Vec::new();
        // Copies as (source, destination).
        let mut copies: Vec<(u32, u32)> = Vec::new();
        for (pc, instruction) in function.code.iter().enumerate() {
            match instruction {
                Instruction::Undef { dst } => {
                    if let Some(flag) = stub.get_mut(dst.0.0 as usize).filter(|flag| !**flag) {
                        *flag = true;
                        work.push(dst.0.0);
                    }
                }
                Instruction::Move { dst, source } => copies.push((source.0, dst.0.0)),
                _ => {}
            }
            // Only a terminator has edge moves.
            if instruction.is_terminator() {
                for (_, moves, _) in edges(pc, instruction) {
                    copies.extend(moves.iter().map(|(dst, source)| (source.0, dst.0)));
                }
            }
        }
        for handler in &function.handlers {
            copies.extend(handler.target.moves.iter().map(|(dst, source)| (source.0, dst.0)));
        }
        copies.sort_unstable();
        while let Some(register) = work.pop() {
            let from = copies.partition_point(|(source, _)| *source < register);
            for &(_, dst) in copies[from..].iter().take_while(|(source, _)| *source == register) {
                if let Some(flag) = stub.get_mut(dst as usize).filter(|flag| !**flag) {
                    *flag = true;
                    work.push(dst);
                }
            }
        }
        let holds = |register: &Register| stub.get(register.0 as usize).copied().unwrap_or(false);
        for instruction in &function.code {
            match instruction {
                Instruction::ObjNew { object, fields, .. } => {
                    for (index, _) in fields.iter().enumerate().filter(|(_, register)| holds(register)) {
                        mark(object.0 as usize, index);
                    }
                }
                Instruction::ObjInit { object, field, value } if holds(value) => {
                    let declared = function.registers.get(object.0 as usize).and_then(|ty| program.ty(*ty));
                    if let Some(Type::Object(owner)) = declared {
                        mark(owner.0 as usize, *field as usize);
                    }
                }
                _ => {}
            }
        }
    }
    fields
}

/// Ребро инструкции: цель, присваивания и регистр значения ребра.
pub(crate) type Edge<'a> = (u32, &'a [(Register, Register)], Option<Register>);

/// Рёбра инструкции.
pub(crate) fn edges(pc: usize, instruction: &Instruction) -> Vec<Edge<'_>> {
    match instruction {
        Instruction::Jump { target } => vec![(target.pc, &target.moves[..], None)],
        Instruction::Branch {
            then_target,
            else_target,
            ..
        } => vec![
            (then_target.pc, &then_target.moves[..], None),
            (else_target.pc, &else_target.moves[..], None),
        ],
        Instruction::Catch { value, handler, next, .. } => vec![
            (handler.pc, &handler.moves[..], Some(value.0)),
            (next.pc, &next.moves[..], None),
        ],
        Instruction::ArrayLookup { value, found, missing, .. } | Instruction::MapLookup { value, found, missing, .. } => vec![
            (found.pc, &found.moves[..], Some(value.0)),
            (missing.pc, &missing.moves[..], None),
        ],
        instruction if instruction.is_terminator() => Vec::new(),
        _ => vec![(pc as u32 + 1, &[][..], None)],
    }
}

/// Memory the initialization analysis of one function may use: three register
/// sets per block leader, checked before they are allocated, and the guards of
/// stubs (see [`State`]) within what is left.
pub const MAX_ANALYSIS_BYTES: usize = 64 * 1024 * 1024;

/// The bytes the three register sets of `blocks` block leaders over `size`
/// registers take (#1069), or the refusal when they are over
/// [`MAX_ANALYSIS_BYTES`]: it prints the computed bytes, which the two counts
/// alone do not give (#1097).
fn analysis_bytes(blocks: usize, size: usize) -> Result<usize, String> {
    let bytes = blocks.checked_mul(size.div_ceil(64)).and_then(|words| words.checked_mul(3 * 8));
    match bytes {
        Some(bytes) if bytes <= MAX_ANALYSIS_BYTES => Ok(bytes),
        _ => Err(format!(
            "function is too large to verify: {blocks} blocks × {size} registers need {} bytes \
             for three register sets per block, over {MAX_ANALYSIS_BYTES}",
            bytes.map_or_else(|| format!("more than {}", usize::MAX), |bytes| bytes.to_string())
        )),
    }
}

/// Does `pc` only fall through to the next instruction?
fn falls_through(pc: usize, edges: &[Edge<'_>]) -> bool {
    matches!(edges, [(target, moves, None)] if *target as usize == pc + 1 && moves.is_empty())
}

/// Every read sees a written register on every path, and a stub of `undef`
/// is used only where a binding check proves it bound (see [`State`]). States
/// are kept only at block leaders (the entry and the targets of jumps,
/// branches, catches, lookups and handlers); a block's instructions are
/// walked in order, so the memory is leaders × registers, not instructions ×
/// registers.
fn check_initialization(context: &Context<'_>) -> Result<(), VerifyError> {
    let function = context.function;
    let size = function.registers.len();
    let count = function.code.len();
    if count == 0 {
        return Ok(());
    }
    let mut leader = vec![false; count];
    leader[0] = true;
    for (pc, instruction) in function.code.iter().enumerate() {
        let edges = edges(pc, instruction);
        if !falls_through(pc, &edges) {
            for (target, ..) in edges {
                if let Some(slot) = leader.get_mut(target as usize) {
                    *slot = true;
                }
            }
        }
    }
    for handler in &function.handlers {
        if let Some(slot) = leader.get_mut(handler.target.pc as usize) {
            *slot = true;
        }
    }
    let leaders: Vec<usize> = (0..count).filter(|&pc| leader[pc]).collect();
    let mut block_of = vec![u32::MAX; count];
    for (block, &pc) in leaders.iter().enumerate() {
        block_of[pc] = block as u32;
    }
    let bytes = analysis_bytes(leaders.len(), size).map_err(|message| context.error(None, message))?;
    let mut state: Vec<Option<State>> = vec![None; leaders.len()];
    let mut entry = State::new(size);
    for param in &function.params {
        entry.written.insert(*param);
    }
    state[0] = Some(entry);
    let mut work = vec![0usize];
    let mut queued = vec![false; leaders.len()];
    queued[0] = true;
    // Guard entries of all stored states, bounded with the sets (8 bytes each).
    let budget = (MAX_ANALYSIS_BYTES - bytes) / 8;
    let mut entries = 0usize;
    let mut merge = |state: &mut Vec<Option<State>>, work: &mut Vec<usize>, queued: &mut Vec<bool>, block: usize, incoming: State| {
        let before = state[block].as_ref().map_or(0, State::entries);
        let changed = match &mut state[block] {
            None => {
                state[block] = Some(incoming);
                true
            }
            Some(existing) => existing.meet(&incoming),
        };
        entries = entries - before + state[block].as_ref().map_or(0, State::entries);
        if changed && !queued[block] {
            queued[block] = true;
            work.push(block);
        }
        entries <= budget
    };
    let too_large = || {
        context.error(None, format!("function is too large to verify: the guards of `undef` stubs exceed {MAX_ANALYSIS_BYTES} bytes"))
    };
    while let Some(block) = work.pop() {
        queued[block] = false;
        let mut current = state[block].clone().expect("reached");
        let mut pc = leaders[block];
        loop {
            let instruction = &function.code[pc];
            // Чтения инструкции и источники её переходов — до записи.
            let mut defs = Vec::new();
            let mut unread = None;
            let mut stub = None;
            instruction.visit(&mut |access| match access {
                Access::Use(register) if !current.written.contains(register) && unread.is_none() => unread = Some(register),
                Access::Use(register) if current.stubs.contains(register) && !carries(instruction, register) && stub.is_none() => {
                    stub = Some(register)
                }
                Access::Target(target) => {
                    for (_, source) in &target.moves {
                        if !current.written.contains(*source) && unread.is_none() {
                            unread = Some(*source);
                        }
                    }
                }
                Access::Def(register) => defs.push(register),
                _ => {}
            });
            if let Some(register) = unread {
                return Err(context.error(Some(pc), format!("register r{} may be read before it is written", register.0)));
            }
            if let Some(register) = stub {
                return Err(context.error(
                    Some(pc),
                    format!("register r{} may hold the stub of `undef`: no binding check proves it bound", register.0),
                ));
            }
            if let Some(handler) = function.handler_at(pc as u32) {
                for (_, source) in &handler.target.moves {
                    if !current.written.contains(*source) {
                        return Err(context.error(Some(pc), format!("handler reads r{} before it is written", source.0)));
                    }
                }
                let mut incoming = current.clone();
                incoming.transfer(&handler.target.moves);
                incoming.clobber(handler.failure);
                let target = block_of[handler.target.pc as usize] as usize;
                if !merge(&mut state, &mut work, &mut queued, target, incoming) {
                    return Err(too_large());
                }
            }
            let payload = match instruction {
                Instruction::Catch { value, .. } | Instruction::ArrayLookup { value, .. } | Instruction::MapLookup { value, .. } => Some(value.0),
                _ => None,
            };
            for register in defs {
                if Some(register) != payload {
                    current.define(register, instruction);
                }
            }
            let edges = edges(pc, instruction);
            if edges.iter().any(|(target, ..)| *target as usize >= count) {
                return Err(context.error(Some(pc), "execution falls off the end of the code"));
            }
            if falls_through(pc, &edges) && !leader[pc + 1] {
                pc += 1;
                continue;
            }
            for (index, (target, moves, value)) in edges.into_iter().enumerate() {
                let mut incoming = current.clone();
                if let Instruction::Branch { condition, .. } = instruction {
                    if index == 0 {
                        incoming.holds(*condition);
                    } else {
                        incoming.fails(*condition);
                    }
                }
                if let Some(value) = value {
                    incoming.clobber(value);
                }
                incoming.transfer(moves);
                if !merge(&mut state, &mut work, &mut queued, block_of[target as usize] as usize, incoming) {
                    return Err(too_large());
                }
            }
            break;
        }
    }
    Ok(())
}

/// Register types, result and parameters: checked for every function before
/// any body, since a call reads the callee's signature.
fn check_signature(program: &Program, function: &Function) -> Result<(), VerifyError> {
    let context = Context { program, function };
    for ty in &function.registers {
        check_type_ref(program, ty.0, None).map_err(|e| context.error(None, e.message))?;
    }
    if let Some(result) = function.result {
        check_type_ref(program, result.0, None).map_err(|e| context.error(None, e.message))?;
    }
    let mut params = std::collections::HashSet::new();
    for param in &function.params {
        context.register(None, *param)?;
        if !params.insert(*param) {
            return Err(context.error(None, format!("parameter register r{} is repeated", param.0)));
        }
    }
    Ok(())
}

fn check_function(program: &Program, function: &Function) -> Result<(), VerifyError> {
    let context = Context { program, function };
    if let Some(intrinsic) = &function.intrinsic {
        for object in &intrinsic.objects {
            if object.0 as usize >= program.objects.len() {
                return Err(context.error(None, format!("intrinsic object obj{} is out of bounds", object.0)));
            }
        }
        for helper in &intrinsic.helpers {
            check_function_ref(program, helper.0).map_err(|e| context.error(None, e.message))?;
        }
    }
    let bodiless = function.native.is_some() || function.intrinsic.is_some();
    if function.native.is_some() && function.intrinsic.is_some() {
        return Err(context.error(None, "a function cannot be both native and intrinsic"));
    }
    if bodiless {
        if !function.code.is_empty() || !function.handlers.is_empty() {
            return Err(context.error(None, "native and intrinsic functions have no code"));
        }
        return Ok(());
    }
    if function.code.is_empty() {
        return Err(context.error(None, "function has no code"));
    }
    for (pc, instruction) in function.code.iter().enumerate() {
        check_instruction(&context, pc, instruction)?;
    }
    let mut previous_end = 0;
    for handler in &function.handlers {
        if handler.start >= handler.end || handler.end as usize > function.code.len() {
            return Err(context.error(None, format!("handler {}..{} is out of bounds", handler.start, handler.end)));
        }
        if handler.start < previous_end {
            return Err(context.error(None, "handlers must be sorted and disjoint"));
        }
        previous_end = handler.end;
        context.target(None, &handler.target)?;
        context.register(None, handler.failure)?;
    }
    check_initialization(&context)?;
    let typing = crate::typing::Typing::new(program, function);
    for (pc, instruction) in function.code.iter().enumerate() {
        typing.instruction(instruction).map_err(|message| context.error(Some(pc), message))?;
    }
    typing.handlers().map_err(|message| context.error(None, message))
}

pub(crate) fn verify(program: &Program) -> Result<(), VerifyError> {
    check_tables(program)?;
    for function in &program.functions {
        check_signature(program, function)?;
    }
    for function in &program.functions {
        check_function(program, function)?;
    }
    crate::typing::tables(program).map_err(program_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal prints the bytes the analysis would take (three 64-register words per block and word), so its
    /// numbers add up (#1097); the budget itself is inclusive.
    #[test]
    fn the_analysis_budget_refusal_names_the_bytes_it_computed() {
        assert_eq!(analysis_bytes(2, 65), Ok(2 * 2 * 3 * 8));
        assert_eq!(analysis_bytes(0, 1 << 20), Ok(0));
        let words = MAX_ANALYSIS_BYTES / 24;
        assert_eq!(analysis_bytes(words, 64), Ok(MAX_ANALYSIS_BYTES - MAX_ANALYSIS_BYTES % 24));
        assert_eq!(
            analysis_bytes(1000, 64 * 3000),
            Err("function is too large to verify: 1000 blocks × 192000 registers need 72000000 bytes \
                 for three register sets per block, over 67108864".to_string())
        );
        let overflow = analysis_bytes(usize::MAX, 128).unwrap_err();
        assert!(overflow.contains(&format!("need more than {} bytes", usize::MAX)), "{overflow}");
    }
}
