//! `any` values for the bridged origin natives, and the parts of a value for
//! `runtime/debug` (`data_keys`, `data_items`).
//!
//! An `any` box holds a word and its program type. Its value converts to an
//! origin value through that type (objects, arrays and maps recursively), so
//! the origin `runtime/debug` describes it with the same texts as on the
//! other engines. Parts stay core words: each is boxed as `any` with its own
//! static type, and a role- or `any`-typed part resolves when it is inspected.
use crate::heap;
use crate::host::*;
use crate::ops::{AnyBox, MapValue};
use crate::program::Kind;
use crate::word::{UNBOUND, Word};
use okkama_runtime_core::Value;
use okkama_runtime_core::heap::{ArrayHeap, Closure, Map, MapKey, ObjectHeap};
use okkama_sparrow::{Type as T, TypeRef};
use std::rc::Rc;

/// Nesting beyond this is shown as a native value instead of recursing.
const DEPTH: usize = 64;

/// A value resolved from a word and its static type.
enum Node {
    Scalar(Word, Shape),
    /// A Tomato rat: a payload around the origin's own Rat (#489, #508).
    Rat(Word),
    Enum(u32, u32),
    Array(Word, TypeRef),
    Map(Word, TypeRef, TypeRef),
    Object(Word, u32),
    Action,
    Opaque,
}

impl Heap {
    /// Program enum and object names for the origin `runtime/debug`.
    pub(crate) fn origin_names(&mut self) {
        let program = self.program();
        let objects = program
            .objects
            .iter()
            .map(|o| (o.name.clone(), o.fields.iter().map(|(name, _)| name.clone()).collect()))
            .collect();
        let enums = program
            .enums
            .iter()
            .map(|e| (e.name.rsplit("::").next().unwrap_or(&e.name).to_string(), e.cases.clone()))
            .collect();
        self.origin.objects = objects;
        self.origin.enums = enums;
    }

    fn node(&self, word: Word, ty: TypeRef) -> Node {
        let program = self.program();
        match program.ty(ty) {
            Some(T::Int) => Node::Scalar(word, Shape::Int),
            Some(T::Float) => Node::Scalar(word, Shape::Float),
            Some(T::Bool) => Node::Scalar(word, Shape::Bool),
            Some(T::Byte) => Node::Scalar(word, Shape::Byte),
            Some(T::Char) => Node::Scalar(word, Shape::Char),
            Some(T::Str) => Node::Scalar(word, Shape::Str),
            Some(T::Rat) => Node::Rat(word),
            Some(T::Enum(e)) => Node::Enum(e.0, word as u32),
            Some(T::Array(element)) => Node::Array(word, *element),
            Some(T::Map(key, item)) => Node::Map(word, *key, *item),
            Some(T::Object(o) | T::Successor(o)) => Node::Object(word, o.0),
            Some(T::Fn { .. }) => Node::Action,
            Some(T::Any) => match self.core().payload::<AnyBox>(word) {
                Some(any) => self.node(any.value, any.ty),
                None => Node::Opaque,
            },
            Some(T::Role(_)) if word != UNBOUND => {
                let header = self.core().header_of(word);
                match heap::kind(header) {
                    heap::OBJECT => Node::Object(word, heap::payload(header)),
                    heap::CLOSURE => Node::Action,
                    _ => Node::Opaque,
                }
            }
            _ => Node::Opaque,
        }
    }

    /// The origin value of an `any` box (`slot` holds the box).
    pub(crate) fn any_to_origin(&self, slot: Slot) -> Result<Value, Error> {
        let any = self.core().payload::<AnyBox>(slot.bits).ok_or_else(|| Error("expected any".into()))?;
        self.value_to_origin(any.value, any.ty, 0, false)
    }

    /// data_dump needs only an unsupported value's category and type, never its fields.
    pub(crate) fn dump_to_origin(&self, slot: Slot) -> Result<Value, Error> {
        let any = self.core().payload::<AnyBox>(slot.bits).ok_or_else(|| Error("expected any".into()))?;
        self.value_to_origin(any.value, any.ty, 0, true)
    }

    fn value_to_origin(&self, word: Word, ty: TypeRef, depth: usize, dumping: bool) -> Result<Value, Error> {
        if depth > DEPTH {
            return Ok(Value::Handle(Rc::new(())));
        }
        let program = self.program();
        if dumping {
            match program.ty(ty) {
                Some(T::Any) => {
                    if let Some(any) = self.core().payload::<AnyBox>(word) {
                        return self.value_to_origin(any.value, any.ty, depth, true);
                    }
                }
                // Category-only stand-ins; no future is started or failure payload inspected.
                Some(T::Future(_)) => return Ok(Value::Future(okkama_runtime_core::sched::resolved(Ok(Value::Undef)))),
                Some(T::Failure) => return Ok(Value::Failure(okkama_runtime_core::sched::cancelled_failure())),
                _ => {}
            }
        }
        Ok(match self.node(word, ty) {
            Node::Scalar(word, shape) => {
                let abi = match shape {
                    Shape::Int => okkama_runtime_abi::Type::Int,
                    Shape::Float => okkama_runtime_abi::Type::Float,
                    Shape::Bool => okkama_runtime_abi::Type::Bool,
                    Shape::Byte => okkama_runtime_abi::Type::Byte,
                    Shape::Char => okkama_runtime_abi::Type::Char,
                    _ => okkama_runtime_abi::Type::Str,
                };
                let slot = self.slot(word, &shape);
                self.to_origin(slot, &abi)?
            }
            Node::Rat(word) => match self.core().payload::<crate::ops::Rational>(word) {
                Some(rational) => Value::Rat(Rc::new(rational.0.clone())),
                None => Value::Handle(Rc::new(())),
            },
            Node::Enum(declaration, case) => Value::Enum { declaration: declaration as usize, case: case as usize },
            Node::Array(word, element) => {
                let items = self.core().elements(word).to_vec();
                ArrayHeap.allocate(items.into_iter().map(|w| self.value_to_origin(w, element, depth + 1, dumping)).collect::<Result<_, _>>()?)
            }
            Node::Map(word, key, item) => {
                let map = self.core().payload::<MapValue>(word).ok_or_else(|| Error("expected map".into()))?;
                let mut entries = std::collections::BTreeMap::new();
                for &(k, v) in map.entries.values() {
                    let k = self.value_to_origin(k, key, depth + 1, dumping)?;
                    let k = MapKey::from_value(&k).map_err(|e| Error(e.message))?;
                    entries.insert(k, self.value_to_origin(v, item, depth + 1, dumping)?);
                }
                Value::Map(Rc::new(Map::new(entries)))
            }
            Node::Object(word, descriptor) => {
                if dumping {
                    return Ok(ObjectHeap.allocate(descriptor as usize, Vec::new()));
                }
                let fields = &program.objects[descriptor as usize].fields;
                let mut values = Vec::with_capacity(fields.len());
                for (index, (_, field)) in fields.iter().enumerate() {
                    let value = self.core().field(word, index);
                    values.push(if value == UNBOUND && Kind::of(program, *field) == Kind::Ref {
                        Value::Undef
                    } else {
                        self.value_to_origin(value, *field, depth + 1, dumping)?
                    });
                }
                ObjectHeap.allocate(descriptor as usize, values)
            }
            Node::Action => Value::Fn(Rc::new(Closure { function: 0, captures: Vec::new(), worker: None })),
            Node::Opaque => Value::Handle(Rc::new(())),
        })
    }

    /// `data_keys` / `data_items`: the parts of an `any` value as `@any`.
    pub(crate) fn debug_parts(&mut self, keys: bool, slot: Slot) -> Result<Slot, Error> {
        let any = self.core().payload::<AnyBox>(slot.bits).ok_or_else(|| Error("expected any".into()))?;
        let (word, ty) = (any.value, any.ty);
        let program = self.program();
        let parts: Vec<(Word, TypeRef)> = match self.node(word, ty) {
            Node::Array(word, element) if !keys => self.core().elements(word).iter().map(|&w| (w, element)).collect(),
            Node::Map(word, key, item) => {
                let map = self.core().payload::<MapValue>(word).ok_or_else(|| Error("expected map".into()))?;
                map.entries.values().map(|&(k, v)| if keys { (k, key) } else { (v, item) }).collect()
            }
            Node::Object(word, descriptor) if !keys => program.objects[descriptor as usize]
                .fields
                .iter()
                .enumerate()
                .map(|(index, (_, field))| (self.core().field(word, index), *field))
                .collect(),
            _ => Vec::new(),
        };
        let mut boxed = Vec::with_capacity(parts.len());
        for (word, ty) in parts {
            let program = self.program();
            let repr = Kind::of(program, ty).repr();
            let value = if matches!(program.ty(ty), Some(T::Any)) {
                word
            } else {
                self.core_mut().allocate_payload(Box::new(AnyBox { value: word, ty, repr }))
            };
            boxed.push(Slot { bits: value, tag: OBJECT });
        }
        self.allocate_array(boxed)
    }
}

#[cfg(test)]
mod data_dump_tests {
    use crate::ops::AnyBox;
    use crate::program::Loaded;
    use crate::runtime::Runtime;
    use crate::word::Repr;
    use okkama_sparrow::TypeRef;

    fn runtime() -> Box<Runtime> {
        let program = okkama_sparrow::assemble(r#"sparrow 16
module "data-dump-965" "1.0.0"
type t0 int
type t1 str
type t2 any
type t3 object obj0
type t4 array t3
type t5 array t0
type t6 future t0
type t7 failure
type t8 handle 0
type t9 fn [] -> t0
type t10 map t0 t3
type t11 map t3 t0
handle "TestHandle"
object obj0 "O" { field "n" t0 }
object obj1 "NativeFailure" builtin {
 field "domain" t1
 field "code" t1
 field "operation" t1
 field "message" t1
}
function f0 "data_dump" native "runtime/debug@1::data_dump/1" {
 registers [t2]
 params [r0]
 result t1
}
function f1 "dump" {
 registers [t2, t1]
 params [r0]
 result t1
 0: call r1, f0, [r0]
 1: ret r1
}
"#).unwrap();
        Box::new(Runtime::new(Loaded::new(program)).unwrap())
    }

    fn boxed(runtime: &mut Runtime, value: u64, ty: u32) -> u64 {
        runtime.heap.allocate_payload(Box::new(AnyBox { value, ty: TypeRef(ty), repr: Repr::Ref }))
    }

    #[test]
    fn data_dump_object_diagnostic_reaches_runtime_error() {
        for nesting in 0..4 {
            let mut runtime = runtime();
            let three = runtime.heap.int(3);
            let object = runtime.heap.object_new(0, &[three]);
            let (value, ty) = match nesting {
                0 => (object, 3),
                1 => (runtime.heap.allocate_array(Repr::Ref, &[object]), 4),
                _ => {
                    use crate::ops::{Key, MapValue};
                    let (key, words, key_repr, value_repr, ty) = if nesting == 2 {
                        (Key::Small(3), (three, object), Repr::Int, Repr::Ref, 10)
                    } else {
                        (Key::Object(0, vec![Key::Small(3)].into()), (object, three), Repr::Ref, Repr::Int, 11)
                    };
                    let map = MapValue {
                        key: key_repr, value: value_repr,
                        entries: crate::maps::Entries::BTree([(key, words)].into()),
                        owned: std::cell::Cell::new(None),
                    };
                    (runtime.heap.allocate_payload(Box::new(map)), ty)
                }
            };
            let argument = boxed(&mut runtime, value, ty);
            let error = runtime.run_function_with(1, &[argument], crate::STACK_BYTES).unwrap_err().to_string();
            assert!(error.contains("data_dump does not support object values (O)"), "{error}");
            assert!(error.contains("debug/dumper::Dumper"), "{error}");
            assert!(error.contains("domain: core.debug"), "{error}");
            assert!(error.contains("code: unsupported_type"), "{error}");
            assert!(error.contains("operation: runtime/debug@1::data_dump/1"), "{error}");
            assert!(runtime.uncaught.as_ref().unwrap().message.contains("debug/dumper::Dumper"));
        }
    }

    #[test]
    fn data_dump_supported_array_survives_bridge() {
        let mut runtime = runtime();
        let one = runtime.heap.int(1);
        let two = runtime.heap.int(2);
        let array = runtime.heap.allocate_array(Repr::Int, &[one, two]);
        let argument = boxed(&mut runtime, array, 5);
        let result = runtime.run_function_with(1, &[argument], crate::STACK_BYTES).unwrap();
        assert_eq!(runtime.heap.text(result), "[1, 2]");
    }

    #[test]
    fn data_dump_bridge_preserves_unsupported_categories() {
        // The bridge must classify these by type without dereferencing or running them.
        for (ty, kind) in [(6, "future"), (7, "failure"), (8, "native"), (9, "action")] {
            let mut runtime = runtime();
            let argument = boxed(&mut runtime, crate::word::UNBOUND, ty);
            let crate::host::Result2::Raise(failure) = crate::host::call(
                &mut runtime, okkama_runtime_abi::Native::DebugDataDump, &[argument],
            ) else { panic!("expected structured native failure for {kind}") };
            let message = runtime.describe(std::ptr::null_mut(), failure);
            assert!(message.contains(&format!("does not support {kind} values")), "{message}");
            assert!(message.contains("debug/dumper::Dumper"));
        }
    }
}
