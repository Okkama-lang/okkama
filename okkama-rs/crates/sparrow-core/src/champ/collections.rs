//! SPB 17 maps, collection operations and unique builders on Tomato's heap payloads.
//! Every helper reads its operation and operand types from the verified instruction at (function, pc),
//! so machine code passes only words. Arrays take Tomato's persistent forms (#826, `ops::concat_arrays` and the
//! rest): a chain of appends or prepends shares a buffer (#246, #247), a large slice is a view (#249), a chain of
//! replacements a trie (#250). Machine code's array.get reads a flat array in place, a view one load further and
//! a trie through a helper.
#![allow(clippy::not_unsafe_ptr_arg_deref)]
use super::*;
use super::helpers::{error, fail, stop};
use super::machine::repr;
use crate::ops::{Builder, Key, MAP_ENTRY_BYTES, MapValue};
use crate::runtime::{self, Ctx};
use crate::word::Repr;
use okkama_sparrow::spb17 as s;
use std::rc::Rc;
use std::sync::Arc;

type Outcome = Result<Word, String>;

fn program(runtime: &Runtime) -> Option<Arc<s::Program>> {
    runtime.champ.machine.as_ref().map(|machine| machine.source.clone())
}

fn map_shape(program: &s::Program, ty: s::Type) -> Option<(s::Type, s::Type)> {
    match ty {
        s::Type::Map(id) | s::Type::MapBuilder(id) => match program.composites.get(id as usize)? {
            s::Composite::Map { key, value } => Some((*key, *value)),
            s::Composite::Builder { .. } => None,
        },
        _ => None,
    }
}

/// The finished array type of an array builder.
fn builder_array(program: &s::Program, ty: s::Type) -> Option<s::Type> {
    match ty {
        s::Type::Builder(id) => match program.composites.get(id as usize)? {
            s::Composite::Builder { array } => Some(*array),
            s::Composite::Map { .. } => None,
        },
        _ => None,
    }
}

/// The map key of `word`, a value of the verified key type `ty` (`key_type` in the loader).
pub(crate) fn key(runtime: &Runtime, program: &s::Program, word: Word, ty: s::Type) -> Key {
    match ty {
        s::Type::Int => Key::int(runtime.heap.integer(word)),
        s::Type::Float => Key::Float(f64::from_bits(word)),
        s::Type::Str => Key::text(Rc::from(runtime.heap.text(word))),
        s::Type::Rat => Key::Rat(Rc::new(runtime.rational(word).clone())),
        s::Type::Object(id) => {
            let object = &program.objects[id as usize];
            let fields: Vec<Key> = if object.fields.is_empty() {
                vec![key(runtime, program, runtime.heap.field(word, 0), s::Type::Int)]
            } else {
                object.fields.iter().filter(|field| !field.once).map(|field| {
                    let storage = field.storage as usize;
                    key(runtime, program, runtime.heap.field(word, storage), object.layout.storage_fields[storage])
                }).collect()
            };
            Key::Object(id, fields.into())
        }
        _ => Key::Raw(word),
    }
}

/// The text of a key in a "not found" message, as Tomato writes it.
fn key_text(runtime: &Runtime, word: Word, ty: s::Type) -> String {
    match ty {
        s::Type::Str => format!("\"{}\"", runtime.heap.text(word)),
        s::Type::Char => format!("\"{}\"", char::from_u32(word as u32).unwrap_or('?')),
        s::Type::Bool => if word != 0 { "true" } else { "false" }.to_string(),
        s::Type::Rat => runtime.rational(word).to_string(),
        s::Type::Float => crate::scalar::float_text(f64::from_bits(word)),
        s::Type::Byte => word.to_string(),
        s::Type::Int => runtime.big_value(word).to_string(),
        _ => "?".to_string(),
    }
}

fn empty_map(key: s::Type, value: s::Type) -> MapValue {
    MapValue { key: repr(key), value: repr(value), entries: Default::default(), owned: Default::default() }
}

fn map_entries(runtime: &Runtime, map: Word) -> &crate::maps::Entries {
    &runtime.heap.payload::<MapValue>(map).expect("verified map").entries
}

/// A position argument inside `0..length`, or the message of `array.get`/`map.key_at`.
fn position(index: Word, length: usize, message: &str) -> Result<usize, String> {
    crate::word::small_value(index).and_then(|index| usize::try_from(index).ok())
        .filter(|&index| index < length).ok_or_else(|| message.to_string())
}

fn new_array(runtime: &mut Runtime, array: s::Type, items: &[Word]) -> Word {
    if array == s::Type::Bytes {
        let bytes: Vec<u8> = items.iter().map(|&word| word as u8).collect();
        return runtime.heap.allocate_bytes(&bytes);
    }
    let element = s::element_type(array).expect("verified array type");
    runtime.heap.allocate_array(repr(element), items)
}

/// The word of a verified scalar or string constant: an int inline when it fits and a BigInt beyond 64 bits,
/// as Tomato represents it; a rat, a string on the heap; raw bits otherwise.
pub(crate) fn constant_word(heap: &mut crate::heap::Heap, value: &s::Constant) -> Result<Word, String> {
    Ok(match value {
        s::Constant::Int(value) => heap.big(value.parse::<num_bigint::BigInt>()
            .map_err(|_| "invalid integer constant".to_string())?),
        s::Constant::Rat(value) => {
            let part = |text: &str| okkama_runtime_core::IntValue::parse(text)
                .ok_or_else(|| "invalid rat constant".to_string());
            let rat = okkama_runtime_core::rat::Rat::new(&part(&value.numerator)?, &part(&value.denominator)?)
                .map_err(|_| "invalid rat constant".to_string())?;
            heap.allocate_payload(Box::new(crate::ops::Rational(rat)))
        }
        s::Constant::Bool(value) => u64::from(*value),
        s::Constant::Descriptor(value) => *value as u64,
        s::Constant::Str(value) => heap.allocate_text(value),
        s::Constant::Byte(value) | s::Constant::Char(value) => u64::from(*value),
        s::Constant::Float(bits) => bits.parse().map_err(|_| "invalid float constant".to_string())?,
    })
}

/// The values of the program's module constants (#709), built once from their verified data before the
/// program runs. Every datum becomes a runtime constant (a root) as soon as it exists, so a collection
/// while a later datum is built keeps it; `global.get` reads the last datum of each constant.
pub(crate) fn build_globals(runtime: &mut Runtime, program: &s::Program) -> Result<Vec<Word>, String> {
    let mut values = Vec::with_capacity(program.globals.len());
    for global in &program.globals {
        let mut words: Vec<Word> = Vec::with_capacity(global.data.len());
        for datum in &global.data {
            let parts = |items: &[u32]| items.iter().map(|&item| words[item as usize]).collect::<Vec<_>>();
            let word = match &datum.value {
                s::DatumValue::Constant(value) => constant_word(&mut runtime.heap, value)?,
                s::DatumValue::Case(case) => {
                    let s::Type::Object(id) = datum.ty else { return Err("verified enum datum".into()) };
                    let tag = runtime.heap.int(i64::from(*case));
                    runtime.heap.object_new(id, &[tag])
                }
                s::DatumValue::Array(items) => {
                    let items = parts(items);
                    new_array(runtime, datum.ty, &items)
                }
                s::DatumValue::Map(items) => {
                    let items = parts(items);
                    collection(runtime, program, s::DataOp::MapNew, &[], datum.ty, &items)?
                }
            };
            runtime.constants.push((word, repr(datum.ty)));
            words.push(word);
        }
        values.push(*words.last().ok_or_else(|| "verified global data".to_string())?);
    }
    Ok(values)
}

fn items(runtime: &Runtime, array: Word, ty: s::Type) -> Vec<Word> {
    if ty == s::Type::Bytes {
        runtime.heap.byte_view(array).iter().map(|&byte| Word::from(byte)).collect()
    } else {
        runtime.heap.elements(array).into_owned()
    }
}

fn collection(runtime: &mut Runtime, program: &s::Program, op: s::DataOp, types: &[s::Type], result: s::Type,
    words: &[Word]) -> Outcome
{
    use s::DataOp as D;
    Ok(match op {
        D::MapNew => {
            let (key_type, value_type) = map_shape(program, result).expect("verified map.new");
            let mut map = empty_map(key_type, value_type);
            let mut owned = 0;
            for pair in words.chunks(2) {
                let checked = key(runtime, program, pair[0], key_type);
                let bytes = MAP_ENTRY_BYTES + checked.owned_bytes();
                if map.entries.upsert(checked, (pair[0], pair[1])) {
                    owned += bytes;
                }
            }
            map.owned.set(Some(owned));
            runtime.heap.allocate_payload(Box::new(map))
        }
        D::MapLen => runtime.heap.int(map_entries(runtime, words[0]).len() as i64),
        D::MapHas | D::MapGet => {
            let (key_type, _) = map_shape(program, types[0]).expect("verified map operation");
            let found = lookup(runtime, program, words[0], words[1], key_type);
            if op == D::MapHas {
                return Ok(Word::from(found.is_some()));
            }
            found.ok_or_else(|| format!("Map key not found: {}", key_text(runtime, words[1], key_type)))?
        }
        D::MapKeys | D::MapValues => {
            let keys = op == D::MapKeys;
            let items: Vec<Word> = map_entries(runtime, words[0]).values()
                .map(|&(key, value)| if keys { key } else { value }).collect();
            new_array(runtime, result, &items)
        }
        D::MapKeyAt | D::MapValueAt => {
            let entries = map_entries(runtime, words[0]);
            let at = position(words[1], entries.len(), "Map index out of bounds")?;
            let &(key, value) = entries.values().nth(at).expect("position inside the map");
            if op == D::MapKeyAt { key } else { value }
        }
        D::MapMerge => {
            let (key_type, value_type) = map_shape(program, result).expect("verified map.merge");
            let mut map = empty_map(key_type, value_type);
            for &part in words {
                runtime.heap.payload_mut::<MapValue>(part).expect("verified map").entries.make_persistent();
            }
            let mut parts = words.iter().map(|&part| map_entries(runtime, part));
            if let Some(first) = parts.next() {
                map.entries = first.clone();
            }
            for entries in parts {
                map.entries.merge(entries);
            }
            runtime.heap.allocate_payload(Box::new(map))
        }
        D::MapDifference => {
            let (key_type, value_type) = map_shape(program, result).expect("verified map.difference");
            let mut map = empty_map(key_type, value_type);
            runtime.heap.payload_mut::<MapValue>(words[0]).expect("verified map").entries.make_persistent();
            map.entries = map_entries(runtime, words[0]).clone();
            for &part in &words[1..] {
                for removed in map_entries(runtime, part).keys() {
                    map.entries.remove(removed);
                }
            }
            runtime.heap.allocate_payload(Box::new(map))
        }
        D::ArrayConcat => {
            let bytes = result == s::Type::Bytes;
            let element = if bytes { Repr::Raw } else { repr(s::element_type(result).expect("verified array.concat")) };
            crate::ops::concat_arrays(runtime, element, bytes, words)
        }
        D::ArrayJoin => runtime.heap.allocate_join(words[0], words[1])
            .ok_or_else(|| "join: the result would pass the 4 GiB a string can hold".to_string())?,
        D::ArraySlice => crate::ops::slice_array(runtime, words[0], words[1], words[2])?,
        D::ArrayRemove => crate::ops::remove_at(runtime, words[0], words[1])?,
        D::ArrayReplace => crate::ops::replace_at(runtime, words[0], words[1], words[2])?,
        D::ArrayDifference => {
            let element = s::element_type(result).expect("verified array.difference");
            let mut excluded = Vec::new();
            for &part in &words[1..] {
                excluded.extend(items(runtime, part, types[0]));
            }
            let mut kept = Vec::new();
            for item in items(runtime, words[0], types[0]) {
                let mut found = false;
                for &other in &excluded {
                    if super::machine::value_equal(runtime, program, item, other, element)? {
                        found = true;
                        break;
                    }
                }
                if !found {
                    kept.push(item);
                }
            }
            new_array(runtime, result, &kept)
        }
        D::BuilderNew => match result {
            s::Type::MapBuilder(_) => {
                let (key_type, value_type) = map_shape(program, result).expect("verified map builder");
                runtime.heap.allocate_payload(Box::new(empty_map(key_type, value_type)))
            }
            _ => {
                let array = builder_array(program, result).expect("verified builder");
                let element = s::element_type(array).expect("verified builder array");
                crate::ops::builder_new(runtime, repr(element))
            }
        },
        D::ArrayNew => new_array(runtime, result, words),
        D::ValEq | D::ValNe => {
            let equal = super::machine::value_equal(runtime, program, words[0], words[1], types[0])?;
            Word::from(equal == (op == D::ValEq))
        }
        _ => return Err(format!("{op:?} is not a collection operation")),
    })
}

fn lookup(runtime: &Runtime, program: &s::Program, map: Word, word: Word, key_type: s::Type) -> Option<Word> {
    let entries = map_entries(runtime, map);
    let found = if key_type == s::Type::Str {
        entries.get_text(runtime.heap.text(word))
    } else {
        entries.get(&key(runtime, program, word, key_type))
    };
    found.map(|&(_, value)| value)
}

/// The verified operation at (function, pc): its data operation, operand types and result type.
/// What a data helper needs of the verified instruction at a call site (#832), computed once at load: machine
/// code passes `(function, pc)` and words, and the helper reads its site instead of decoding the instruction on
/// every call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Site {
    /// A data operation or an `evaluate`: the operation, its first operand's type (or `Never`) and its result.
    Collection { op: s::DataOp, operand: s::Type, result: s::Type },
    /// A `lookup`: the type of its receiver.
    Lookup { receiver: s::Type },
    /// A builder transition: the opcode, whether a range is inclusive, the type of its buffer.
    Builder { op: s::Opcode, inclusive: bool, buffer: s::Type },
}

/// The site of each instruction of `program`, by function and pc (#832).
pub(crate) fn sites(program: &s::Program) -> Vec<Box<[Option<Site>]>> {
    program.functions.iter().map(|function| {
        let register = |location: &s::Location| match location {
            s::Location::Register(id) => Some(function.registers[*id as usize]),
            _ => None,
        };
        function.code.iter().map(|instruction| match instruction {
            s::Instruction::Data { op, inputs, output: Some(output), .. } => Some(Site::Collection {
                op: *op,
                operand: inputs.first().map_or(s::Type::Never, |&id| function.registers[id as usize]),
                result: function.registers[*output as usize],
            }),
            s::Instruction::Champ { op: s::Opcode::Evaluate, arguments, .. } => {
                let s::Argument::Evaluation(evaluation) = arguments.first()? else { return None };
                let operand = match arguments.get(1) {
                    Some(s::Argument::Location(location)) => register(location)?,
                    _ => s::Type::Never,
                };
                Some(Site::Collection { op: evaluation.op, operand, result: evaluation.result })
            }
            s::Instruction::Champ { op: s::Opcode::Lookup, arguments, .. } => match arguments.first() {
                Some(s::Argument::Location(location)) => Some(Site::Lookup { receiver: register(location)? }),
                _ => None,
            },
            s::Instruction::Champ { op: op @ (s::Opcode::BuilderPush | s::Opcode::BuilderAppend
                | s::Opcode::BuilderRange | s::Opcode::BuilderFinish | s::Opcode::BuilderAbandon), arguments, .. } => {
                let buffer = arguments.iter().find_map(|argument| match argument {
                    s::Argument::Location(location) => Some(location),
                    _ => None,
                })?;
                Some(Site::Builder {
                    op: *op, inclusive: arguments.contains(&s::Argument::Flag(true)), buffer: register(buffer)?,
                })
            }
            _ => None,
        }).collect()
    }).collect()
}

/// The site at `(function, pc)`.
fn site(runtime: &Runtime, function: u64, pc: u64) -> Option<Site> {
    let machine = runtime.champ.machine.as_ref()?;
    machine.sites.get(function as usize)?.get(pc as usize).copied().flatten()
}

/// A map, array or builder.new data operation; a failing one sets the program failure and returns 0.
pub extern "C" fn rt_machine_collection(ctx: *mut Ctx, function: u64, pc: u64, args: *const Word, count: u64)
    -> Word
{
    let frame = runtime::frame_pointer!();
    let runtime = unsafe { &mut *(*ctx).runtime };
    let Some(program) = program(runtime) else {
        return stop(ctx, error("collection operation outside a Champ program"));
    };
    let Some(Site::Collection { op, operand, result }) = site(runtime, function, pc) else {
        return stop(ctx, error("invalid verified collection operation"));
    };
    let types = [operand];
    // SAFETY: machine code passes its record of `count` words, which outlives the call.
    let words = unsafe { std::slice::from_raw_parts(args, count as usize) };
    if op == s::DataOp::ArrayOrder {
        // Tomato's sort of scalars and text (#678): it never fails, but a budget stop ends the run.
        let word = super::ordering::order_natural(ctx, words[0], types[0]);
        if unsafe { (*ctx).status } == runtime::OK {
            runtime.safepoint(frame, unsafe { &*ctx }, &[(word, repr(result))]);
        }
        return word;
    }
    match collection(runtime, &program, op, &types, result, words) {
        Ok(word) => {
            runtime.safepoint(frame, unsafe { &*ctx }, &[(word, repr(result))]);
            word
        }
        Err(message) => {
            // A program failure with Tomato's message (`crate::ops`), as the program sees it.
            fail(ctx, crate::Error(message));
            0
        }
    }
}

/// `lookup`: 1 and the found element at `post`, or 0 when the index or key is absent.
pub extern "C" fn rt_machine_lookup(ctx: *mut Ctx, function: u64, pc: u64, collection: Word, index: Word,
    post: *mut Word) -> Word
{
    let runtime = unsafe { &*(*ctx).runtime };
    let Some(program) = program(runtime) else {
        return stop(ctx, error("lookup outside a Champ program"));
    };
    let Some(Site::Lookup { receiver }) = site(runtime, function, pc) else {
        return stop(ctx, error("invalid verified lookup"));
    };
    let found = match receiver {
        s::Type::Map(id) => match program.composites.get(id as usize) {
            Some(s::Composite::Map { key, .. }) => lookup(runtime, &program, collection, index, *key),
            _ => return stop(ctx, error("invalid verified lookup")),
        },
        ty => {
            let all = items(runtime, collection, ty);
            position(index, all.len(), "").ok().map(|at| all[at])
        }
    };
    match found {
        Some(value) => {
            unsafe { post.write(value) };
            1
        }
        None => 0,
    }
}

fn builder(runtime: &mut Runtime, program: &s::Program, op: s::Opcode, inclusive: bool, buffer_type: s::Type,
    words: &[Word]) -> Outcome
{
    use s::Opcode as O;
    let buffer = words[0];
    if let s::Type::MapBuilder(_) = buffer_type {
        let (key_type, _) = map_shape(program, buffer_type).expect("verified map builder");
        let put = |runtime: &mut Runtime, item_key: Key, entry: (Word, Word)| {
            let map = runtime.heap.payload_mut::<MapValue>(buffer).expect("map builder");
            let charged = (!map.entries.is_persistent()).then(|| MAP_ENTRY_BYTES + item_key.owned_bytes());
            let fresh = map.entries.upsert(item_key, entry);
            if let (true, Some(bytes)) = (fresh, charged) {
                map.owned.set(map.owned.get().map(|owned| owned + bytes));
                runtime.heap.charge_external(bytes);
            }
        };
        return Ok(match op {
            O::BuilderPush => {
                let checked = key(runtime, program, words[1], key_type);
                put(runtime, checked, (words[1], words[2]));
                buffer
            }
            O::BuilderAppend => {
                let entries: Vec<(Key, (Word, Word))> = map_entries(runtime, words[1]).iter()
                    .map(|(item_key, &entry)| (item_key.clone(), entry)).collect();
                for (item_key, entry) in entries {
                    put(runtime, item_key, entry);
                }
                buffer
            }
            O::BuilderFinish => {
                let map = runtime.heap.payload_mut::<MapValue>(buffer).expect("map builder");
                let owned = map.owned.replace(Some(0));
                let finished = MapValue { key: map.key, value: map.value, entries: std::mem::take(&mut map.entries),
                    owned: owned.into() };
                runtime.heap.allocate_payload_moved(Box::new(finished))
            }
            O::BuilderAbandon => 0,
            _ => return Err("invalid map builder transition".into()),
        });
    }
    let array = builder_array(program, buffer_type).expect("verified builder");
    let push = |runtime: &mut Runtime, all: &[Word]| {
        crate::ops::grow_builder(&mut runtime.heap, buffer, |items| items.extend_from_slice(all));
    };
    Ok(match op {
        O::BuilderPush => {
            push(runtime, &words[1..2]);
            buffer
        }
        O::BuilderAppend => {
            let all = items(runtime, words[1], array);
            push(runtime, &all);
            buffer
        }
        O::BuilderRange => {
            let (start, end) = (runtime.big_value(words[1]), runtime.big_value(words[2]));
            if start > end {
                return Err("Range start must be less than or equal to range end".into());
            }
            let stop = if inclusive { end } else { end - 1 };
            let mut from = start;
            while from <= stop {
                let to = (&from + (crate::ops::RANGE_CHUNK - 1)).min(stop.clone());
                let mut chunk = Vec::new();
                let mut next = from.clone();
                while next <= to {
                    chunk.push(runtime.heap.big(next.clone()));
                    next += 1;
                }
                push(runtime, &chunk);
                from = to + 1;
            }
            buffer
        }
        O::BuilderFinish => crate::ops::builder_finish(runtime, buffer, array == s::Type::Bytes),
        O::BuilderAbandon => {
            if let Some(builder) = runtime.heap.payload_mut::<Builder>(buffer) {
                builder.items.clear();
            }
            0
        }
        _ => return Err("invalid builder transition".into()),
    })
}

/// A builder transition at (function, pc) on the buffer and operand words at `args` (journal excluded).
/// Returns the buffer (an update), the finished value (finish) or 0 (abandon); a failing transition sets
/// the program failure and leaves the buffer as it was.
pub extern "C" fn rt_machine_builder(ctx: *mut Ctx, function: u64, pc: u64, args: *const Word, count: u64)
    -> Word
{
    let frame = runtime::frame_pointer!();
    let runtime = unsafe { &mut *(*ctx).runtime };
    let Some(program) = program(runtime) else {
        return stop(ctx, error("builder transition outside a Champ program"));
    };
    let Some(Site::Builder { op, inclusive, buffer }) = site(runtime, function, pc) else {
        return stop(ctx, error("invalid verified builder transition"));
    };
    // SAFETY: machine code passes its record of `count` words, which outlives the call.
    let words = unsafe { std::slice::from_raw_parts(args, count as usize) };
    match builder(runtime, &program, op, inclusive, buffer, words) {
        Ok(word) => {
            let roots = if word == 0 { vec![] } else { vec![(word, Repr::Ref)] };
            runtime.safepoint(frame, unsafe { &*ctx }, &roots);
            word
        }
        Err(message) => {
            // A program failure with Tomato's message (`crate::ops`), as the program sees it.
            fail(ctx, crate::Error(message));
            0
        }
    }
}

#[cfg(test)]
#[path = "collections_tests.rs"]
mod tests;
