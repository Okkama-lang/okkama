//! Canonical ULEB, bounded allocation and one explicit version gate.
//! Assembly uses named records/fields and mnemonics with ordered operands.
use super::*;
use crate::MAX_FILE_BYTES;

const MAX_DECODED_BYTES: usize = 512 * 1024 * 1024;

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
    text: Option<String>,
}

impl Writer {
    fn label(&mut self, label: &str) {
        if let Some(text) = &mut self.text {
            let column = text.rsplit('\n').next().map_or(0, str::len);
            if label != "\n" && column + label.len() + 1 > 120 {
                text.push('\n');
            }
            text.push_str(label);
            text.push(' ');
        }
    }

    fn number(&mut self, mut value: u32) {
        if self.text.is_some() {
            self.label(&value.to_string());
            return;
        }
        loop {
            let low = (value & 127) as u8;
            value >>= 7;
            self.bytes.push(if value == 0 { low } else { low | 128 });
            if value == 0 {
                break;
            }
        }
    }

    fn byte(&mut self, value: u8) {
        if self.text.is_some() {
            self.number(u32::from(value));
        } else {
            self.bytes.push(value);
        }
    }

    fn string(&mut self, value: &str) {
        if self.text.is_some() {
            let mut text = String::new();
            text.push('"');
            for ch in value.chars() {
                match ch {
                    '"' => text.push_str("\\\""),
                    '\\' => text.push_str("\\\\"),
                    '\n' => text.push_str("\\n"),
                    '\r' => text.push_str("\\r"),
                    '\t' => text.push_str("\\t"),
                    _ => text.push(ch),
                }
            }
            text.push('"');
            self.label(&text);
        } else {
            self.number(value.len() as u32);
            self.bytes.extend_from_slice(value.as_bytes());
        }
    }

    fn opcode(&mut self, code: u8, name: &str) {
        if self.text.is_some() {
            self.label(name);
        } else {
            self.byte(code);
        }
    }
}

#[derive(Debug)]
enum Token {
    Word(String),
    Text(String),
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    tokens: Option<std::collections::VecDeque<Token>>,
    decoded: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0, tokens: None, decoded: 0 }
    }

    fn fail(&self, message: impl Into<String>) -> Error {
        Error { offset: Some(self.at), message: message.into(), rule: "SPB_DECODE" }
    }

    fn take(&mut self, size: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(size).filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| self.fail("truncated SPB 17"))?;
        let value = &self.bytes[self.at..end];
        self.at = end;
        Ok(value)
    }

    fn word(&mut self) -> Result<String, Error> {
        match self.tokens.as_mut().and_then(|tokens| tokens.pop_front()) {
            Some(Token::Word(word)) => Ok(word),
            _ => Err(self.fail("expected assembly token")),
        }
    }

    fn next_is(&self, word: &str) -> bool {
        matches!(self.tokens.as_ref().and_then(|tokens| tokens.front()), Some(Token::Word(next)) if next == word)
    }

    fn label(&mut self, expected: &str) -> Result<(), Error> {
        if self.tokens.is_some() && self.word()? != expected {
            return Err(self.fail(format!("expected `{expected}`")));
        }
        Ok(())
    }

    fn byte(&mut self) -> Result<u8, Error> {
        if self.tokens.is_some() {
            u8::try_from(self.number()?).map_err(|_| self.fail("byte overflow"))
        } else {
            Ok(self.take(1)?[0])
        }
    }

    fn number(&mut self) -> Result<u32, Error> {
        if self.tokens.is_some() {
            let word = self.word()?;
            let value: u32 = word.parse().map_err(|_| self.fail("invalid number"))?;
            if value.to_string() != word {
                return Err(self.fail("non-canonical number"));
            }
            return Ok(value);
        }
        let mut value = 0u32;
        for shift in (0..35).step_by(7) {
            let byte = self.byte()?;
            if shift == 28 && byte & 0xf0 != 0 {
                return Err(self.fail("number exceeds u32"));
            }
            value |= u32::from(byte & 127) << shift;
            if byte & 128 == 0 {
                if shift > 0 && byte == 0 {
                    return Err(self.fail("non-canonical number"));
                }
                return Ok(value);
            }
        }
        Err(self.fail("number exceeds u32"))
    }

    fn charge(&mut self, bytes: usize) -> Result<(), Error> {
        self.decoded = self.decoded.saturating_add(bytes);
        if self.decoded > MAX_DECODED_BYTES {
            return Err(self.fail("decoded program exceeds 512 MiB"));
        }
        Ok(())
    }

    fn count<T>(&mut self) -> Result<usize, Error> {
        let count = self.number()? as usize;
        let remaining = self.tokens.as_ref().map_or(self.bytes.len() - self.at, |tokens| tokens.len());
        if count > remaining {
            return Err(self.fail("count exceeds remaining input"));
        }
        self.charge(count.saturating_mul(std::mem::size_of::<T>()))?;
        Ok(count)
    }

    fn string(&mut self) -> Result<String, Error> {
        if self.tokens.is_some() {
            match self.tokens.as_mut().and_then(|tokens| tokens.pop_front()) {
                Some(Token::Text(text)) => {
                    self.charge(text.len())?;
                    return Ok(text);
                }
                _ => return Err(self.fail("expected quoted UTF-8 string")),
            }
        }
        let count = self.count::<u8>()?;
        let bytes = self.take(count)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| self.fail("invalid UTF-8"))
    }

    fn end(&self) -> Result<(), Error> {
        if self.tokens.as_ref().map_or(self.at != self.bytes.len(), |tokens| !tokens.is_empty()) {
            return Err(self.fail("trailing input"));
        }
        Ok(())
    }

    fn opcode(&mut self) -> Result<u8, Error> {
        if self.tokens.is_some() {
            let name = self.word()?;
            for code in 0..=255 {
                if opcode_name(code) == Some(name.as_str()) {
                    return Ok(code);
                }
            }
            Err(self.fail(format!("unknown opcode `{name}`")))
        } else {
            self.byte()
        }
    }
}

trait Wire: Sized {
    fn put(&self, w: &mut Writer);
    fn get(r: &mut Reader<'_>) -> Result<Self, Error>;
}

impl Wire for u32 {
    fn put(&self, w: &mut Writer) { w.number(*self); }
    fn get(r: &mut Reader<'_>) -> Result<Self, Error> { r.number() }
}
impl Wire for bool {
    fn put(&self, w: &mut Writer) { w.byte(u8::from(*self)); }
    fn get(r: &mut Reader<'_>) -> Result<Self, Error> {
        match r.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(r.fail("invalid boolean")),
        }
    }
}
impl Wire for String {
    fn put(&self, w: &mut Writer) { w.string(self); }
    fn get(r: &mut Reader<'_>) -> Result<Self, Error> { r.string() }
}
impl<T: Wire> Wire for Vec<T> {
    fn put(&self, w: &mut Writer) {
        w.number(self.len() as u32);
        for value in self {
            value.put(w);
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, Error> {
        let count = r.count::<T>()?;
        (0..count).map(|_| T::get(r)).collect()
    }
}
impl<T: Wire> Wire for Option<T> {
    fn put(&self, w: &mut Writer) {
        self.is_some().put(w);
        if let Some(value) = self {
            value.put(w);
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, Error> {
        if bool::get(r)? { Ok(Some(T::get(r)?)) } else { Ok(None) }
    }
}
impl<A: Wire, B: Wire> Wire for (A, B) {
    fn put(&self, w: &mut Writer) {
        self.0.put(w);
        self.1.put(w);
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, Error> { Ok((A::get(r)?, B::get(r)?)) }
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Fields after `;` are always present in binary and omitted from assembly when they hold their default,
/// so programs that do not use them keep their assembly; an explicit default there is non-canonical.
macro_rules! record {
    ($name:ident { $( $field:ident ),* $(,)? $(; $( $optional:ident ),* $(,)? )? }) => {
        impl Wire for $name {
            fn put(&self, w: &mut Writer) {
                w.label(concat!(stringify!($name), " {"));
                $(
                    w.label(stringify!($field));
                    self.$field.put(w);
                )*
                $($(
                    if w.text.is_none() || !is_default(&self.$optional) {
                        w.label(stringify!($optional));
                        self.$optional.put(w);
                    }
                )*)?
                w.label("}\n");
            }
            fn get(r: &mut Reader<'_>) -> Result<Self, Error> {
                r.label(stringify!($name))?;
                r.label("{")?;
                $(
                    r.label(stringify!($field))?;
                    let $field = Wire::get(r)?;
                )*
                $($(
                    let $optional = if r.tokens.is_none() {
                        Wire::get(r)?
                    } else if r.next_is(stringify!($optional)) {
                        r.label(stringify!($optional))?;
                        let value = Wire::get(r)?;
                        if is_default(&value) {
                            return Err(r.fail(concat!("non-canonical default `", stringify!($optional), "`")));
                        }
                        value
                    } else {
                        Default::default()
                    };
                )*)?
                r.label("}")?;
                Ok($name { $( $field, )* $($( $optional, )*)? })
            }
        }
    };
}

record! { Program {
    module, version, objects, tables, functions, entry; tests, composites,
} }

record! { Test { name, function } }

record! { ObjectRuntimeLayout {
    storage_fields, allocation_words, identity_offset, has_destroy,
    may_reach_resource, may_async_destroy, trace_layout, selection_layout,
} }

record! { Field { name, storage, once } }
record! { Object {
    name, fields, layout, error_cases; enum_cases, variant_cases, handle,
} }

record! { Assignment {
    destination, source,
} }

record! { Edge {
    target_pc, assignments,
} }

record! { Outcome {
    tag, edge,
} }

record! { Function {
    symbol, entry_abi, registers, services, params, results, blocks, proofs, code,
} }

record! { Block {
    pc, params,
} }

record! { ChampTables {
    scopes, boundaries, summaries,
} }

record! { Scope {
    function, parent, boundary,
} }

record! { Boundary {
    scope,
} }

record! { Summary {
    entry, may_fail, retained_params,
} }

record! { FunctionProofs {
    entry, static_selections, closes, cleanup_plans, post_roots,
    selections, dependencies, registrations, plans, order_witnesses, schedules, envelopes, states, roots,
} }

record! { DeferLabel { label, result } }
record! { RegistrationLabel { label, kind, boundary } }
record! { RegistrationMap { entry, labels } }
record! { Dependency { value, obligations } }
record! { Selection {
    locations,
} }

record! { Plan {
    merge, selection, order_witness,
} }

record! { OrderWitness {
    creation_pcs,
} }

record! { Schedule {
    services,
} }

record! { Envelope {
    failures,
} }

record! { StateRecord {
    pc, locations,
} }

record! { RootRequirement {
    pc, locations,
} }

macro_rules! tagged {
    ($ty:ident { $( $tag:literal => $variant:ident $( ( $field:ident : $field_ty:ty ) )? ),* $(,)? }) => {
        impl Wire for $ty {
            fn put(&self, w: &mut Writer) {
                match self {
                    $( Self::$variant $( ($field) )? => {
                        w.number($tag);
                        $( $field.put(w); )?
                    }, )*
                }
            }
            fn get(r: &mut Reader<'_>) -> Result<Self, Error> {
                match r.number()? {
                    $( $tag => Ok(Self::$variant $( (<$field_ty>::get(r)?) )?), )*
                    tag => Err(r.fail(format!("unknown {} tag {tag}", stringify!($ty)))),
                }
            }
        }
    };
}

tagged! { Type {
    0 => Int, 1 => Bool, 2 => Byte, 3 => Float, 4 => Rat, 5 => Char,
    6 => Str, 20 => Range, 11 => Failure, 21 => Bytes, 22 => Error,
    7 => Object(id: u32), 8 => Successor(id: u32), 13 => Never, 9 => Descriptor,
    23 => Array(shape: ArrayType), 24 => Variant(id: u32), 25 => Handle(id: u32),
    26 => Map(id: u32), 27 => MapBuilder(id: u32), 28 => Builder(id: u32),
} }
impl Wire for Composite {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Map { key, value } => {
                w.number(0);
                key.put(w);
                value.put(w);
            }
            Self::Builder { array } => {
                w.number(1);
                array.put(w);
            }
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, Error> {
        match r.number()? {
            0 => {
                let key = Type::get(r)?;
                let value = Type::get(r)?;
                Ok(Self::Map { key, value })
            }
            1 => Ok(Self::Builder { array: Type::get(r)? }),
            tag => Err(r.fail(format!("unknown Composite tag {tag}"))),
        }
    }
}
record! { ArrayType { leaf, depth } }
tagged! { ArrayLeaf {
    0 => Int, 1 => Bool, 2 => Str, 3 => Range, 4 => Bytes, 5 => Object(id: u32),
    6 => Float, 7 => Char, 8 => Rat, 9 => Variant(id: u32), 10 => Map(id: u32), 11 => Never,
} }
tagged! { ServiceKind {
    0 => Packet, 1 => ScopeState, 2 => Pending, 3 => Cleanup, 4 => StableKey,
    5 => FailureState, 6 => ExitState, 7 => Closed, 8 => DestroyBorrow, 9 => Deps,
} }
tagged! { ExitKind { 0 => Return, 1 => Last, 2 => Next, 3 => Fallthrough } }
tagged! { Location { 0 => Register(id: u32), 1 => Service(id: u32) } }
tagged! { Source { 0 => Location(loc: Location), 1 => ProducedSlot(slot: u32) } }
tagged! { Flow { 0 => Next(locations: Vec<Location>), 1 => Outcomes(outcomes: Vec<Outcome>) } }
tagged! { Constant { 0 => Int(value: String), 1 => Bool(value: bool), 2 => Str(value: String),
    3 => Descriptor(id: u32), 4 => Byte(value: u32), 5 => Char(value: u32), 6 => Float(bits: String),
    7 => Rat(value: RatValue) } }
record! { RatValue { numerator, denominator } }
tagged! { Immediate {
    0 => None, 1 => Constant(value: Constant), 2 => Inclusive(flag: bool),
    3 => Object(id: u32), 4 => Field(index: u32), 5 => Updates(indices: Vec<u32>),
} }
tagged! { Tag {
    0 => Normal, 1 => Unwind, 2 => Selected, 3 => Done, 4 => Continue, 5 => Replace(label: u32),
    6 => Success, 7 => Pending, 8 => Failure, 9 => None, 10 => Some,
} }
tagged! { Argument {
    17 => Evaluation(evaluation: Evaluation),
    18 => Result(result: Option<Type>),
    0 => Location(value: Location), 1 => Scope(id: u32), 2 => Boundary(id: u32), 3 => Summary(id: u32),
    4 => Selection(id: u32), 5 => Plan(id: u32), 6 => Schedule(id: u32), 7 => Envelope(id: u32),
    8 => Entry(id: u32), 9 => Object(id: u32), 10 => Binding(id: u32), 11 => Field(id: u32),
    12 => Label(id: u32), 13 => Flag(flag: bool), 14 => Kind(kind: ExitKind), 15 => Registration(id: u32),
    16 => Destination(destination: ExitDestination),
} }
record! { Evaluation { op, result, immediate } }

impl Wire for DataOp {
    fn put(&self, w: &mut Writer) {
        let (code, sub) = self.wire();
        w.opcode(code, opcode_name(code).unwrap());
        if let Some(sub) = sub {
            sub.put(w);
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, Error> {
        let code = r.opcode()?;
        Self::read(code, r)
    }
}

impl Wire for EntryAbi {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Observation => w.number(0),
            Self::Owned => w.number(1),
            Self::Defer { labels } => {
                w.number(2);
                labels.put(w);
            }
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, Error> {
        match r.number()? {
            0 => Ok(Self::Observation),
            1 => Ok(Self::Owned),
            2 => Ok(Self::Defer { labels: Vec::get(r)? }),
            _ => Err(r.fail("unknown entry ABI")),
        }
    }
}

fn opcode_name(code: u8) -> Option<&'static str> {
    match code {
        1 => Some("const"),
        2 => Some("move"),
        3 => Some("undef"),
        10 => Some("int.add"),
        11 => Some("int.sub"),
        12 => Some("int.mul"),
        13 => Some("int.mod"),
        20 => Some("int.neg"),
        14..=18 => SCALAR_FAMILIES.iter().find(|(family, _, _)| *family == code).map(|(_, name, _)| *name),
        24 => Some("int.cmp"),
        40 => Some("bool.not"),
        41 => Some("bool.cmp"),
        70 => Some("str.concat"),
        71 => Some("str.cmp"),
        72 => Some("str.len"),
        47 => Some("byte.cmp"),
        48 => Some("char.cmp"),
        49 => Some("float.cmp"),
        50 => Some("rat.cmp"),
        51 => Some("rat.new"),
        52 => Some("enum.cmp"),
        53 => Some("val.cmp"),
        54 => Some("variant.tag"),
        55 => Some("variant.get"),
        56 => Some("map.op"),
        57 => Some("array.op"),
        58 => Some("builder.new"),
        26 => Some("cvt"),
        84 => Some("range.new"),
        85 => Some("range.part"),
        80 => Some("array.new"),
        81 => Some("array.len"),
        90 => Some("obj.new"),
        91 => Some("obj.init"),
        92 => Some("obj.get"),
        93 => Some("obj.next"),
        94 => Some("enum.case"),
        140 => Some("br"),
        141 => Some("cond"),
        142 => Some("ret"),
        145 => Some("unreachable"),
        147 => Some("raise"),
        136 => Some("fail.secondary"),
        137 => Some("fail.caused"),
        150 => Some("test.check"),
        255 => Some("champ"),
        _ => None,
    }
}

impl DataOp {
    fn wire(self) -> (u8, Option<u32>) {
        match self {
            Self::Const => (1, None),
            Self::Move => (2, None),
            Self::Undef => (3, None),
            Self::IntAdd => (10, None),
            Self::IntSub => (11, None),
            Self::IntMul => (12, None),
            Self::IntMod => (13, None),
            Self::IntNeg => (20, None),
            Self::IntEq => (24, Some(0)),
            Self::IntNe => (24, Some(1)),
            Self::IntLt => (24, Some(2)),
            Self::IntLe => (24, Some(3)),
            Self::IntGt => (24, Some(4)),
            Self::IntGe => (24, Some(5)),
            Self::BoolNot => (40, None),
            Self::BoolEq => (41, Some(0)),
            Self::BoolNe => (41, Some(1)),
            Self::StrConcat => (70, None),
            Self::StrEq => (71, Some(0)),
            Self::StrNe => (71, Some(1)),
            Self::StrLt => (71, Some(2)),
            Self::StrLe => (71, Some(3)),
            Self::StrGt => (71, Some(4)),
            Self::StrGe => (71, Some(5)),
            Self::StrLen => (72, None),
            Self::ByteEq => (47, Some(0)),
            Self::ByteNe => (47, Some(1)),
            Self::ByteLt => (47, Some(2)),
            Self::ByteLe => (47, Some(3)),
            Self::ByteGt => (47, Some(4)),
            Self::ByteGe => (47, Some(5)),
            Self::CharEq => (48, Some(0)),
            Self::CharNe => (48, Some(1)),
            Self::CharLt => (48, Some(2)),
            Self::CharLe => (48, Some(3)),
            Self::CharGt => (48, Some(4)),
            Self::CharGe => (48, Some(5)),
            Self::FloatEq => (49, Some(0)),
            Self::FloatNe => (49, Some(1)),
            Self::FloatLt => (49, Some(2)),
            Self::FloatLe => (49, Some(3)),
            Self::FloatGt => (49, Some(4)),
            Self::FloatGe => (49, Some(5)),
            Self::RatEq => (50, Some(0)),
            Self::RatNe => (50, Some(1)),
            Self::RatLt => (50, Some(2)),
            Self::RatLe => (50, Some(3)),
            Self::RatGt => (50, Some(4)),
            Self::RatGe => (50, Some(5)),
            Self::RatNew => (51, None),
            Self::EnumEq => (52, Some(0)),
            Self::EnumNe => (52, Some(1)),
            Self::ValEq => (53, Some(0)),
            Self::ValNe => (53, Some(1)),
            Self::VariantTag => (54, None),
            Self::VariantGet => (55, None),
            Self::Convert => (26, None),
            Self::RangeNew => (84, None),
            Self::ArrayNew => (80, None),
            Self::ArrayLen => (81, None),
            Self::RangeStart => (85, Some(0)),
            Self::RangeEnd => (85, Some(1)),
            Self::RangeLen => (85, Some(2)),
            Self::ObjNew => (90, None),
            Self::ObjInit => (91, None),
            Self::ObjGet => (92, None),
            Self::ObjNext => (93, None),
            Self::EnumCase => (94, None),
            Self::FailSecondary => (136, None),
            Self::FailCaused => (137, None),
            Self::MapNew => (56, Some(0)),
            Self::MapLen => (56, Some(1)),
            Self::MapHas => (56, Some(2)),
            Self::MapGet => (56, Some(3)),
            Self::MapKeys => (56, Some(4)),
            Self::MapValues => (56, Some(5)),
            Self::MapKeyAt => (56, Some(6)),
            Self::MapValueAt => (56, Some(7)),
            Self::MapMerge => (56, Some(8)),
            Self::MapDifference => (56, Some(9)),
            Self::ArrayConcat => (57, Some(0)),
            Self::ArrayJoin => (57, Some(1)),
            Self::ArraySlice => (57, Some(2)),
            Self::ArrayRemove => (57, Some(3)),
            Self::ArrayReplace => (57, Some(4)),
            Self::ArrayDifference => (57, Some(5)),
            Self::BuilderNew => (58, None),
            Self::TestOk => (150, Some(0)),
            Self::TestIs => (150, Some(1)),
            Self::TestIsnt => (150, Some(2)),
            Self::TestFail => (150, Some(3)),
            op => {
                let (code, sub) = op.family().expect("every other operation belongs to a scalar family");
                (code, Some(sub))
            }
        }
    }

    fn read(code: u8, r: &mut Reader<'_>) -> Result<Self, Error> {
        let sub = if matches!(code, 14..=18 | 24 | 41 | 71 | 85 | 47..=50 | 52 | 53 | 56 | 57 | 150) {
            Some(r.number()?)
        } else {
            None
        };
        if let (14..=18, Some(sub)) = (code, sub) {
            return Self::of_family(code, sub).ok_or_else(|| r.fail(format!("unknown scalar opcode {code}/{sub}")));
        }
        match (code, sub) {
            (1, None) => Ok(Self::Const),
            (2, None) => Ok(Self::Move),
            (3, None) => Ok(Self::Undef),
            (10, None) => Ok(Self::IntAdd),
            (11, None) => Ok(Self::IntSub),
            (12, None) => Ok(Self::IntMul),
            (13, None) => Ok(Self::IntMod),
            (20, None) => Ok(Self::IntNeg),
            (24, Some(0)) => Ok(Self::IntEq),
            (24, Some(1)) => Ok(Self::IntNe),
            (24, Some(2)) => Ok(Self::IntLt),
            (24, Some(3)) => Ok(Self::IntLe),
            (24, Some(4)) => Ok(Self::IntGt),
            (24, Some(5)) => Ok(Self::IntGe),
            (40, None) => Ok(Self::BoolNot),
            (41, Some(0)) => Ok(Self::BoolEq),
            (41, Some(1)) => Ok(Self::BoolNe),
            (70, None) => Ok(Self::StrConcat),
            (71, Some(0)) => Ok(Self::StrEq),
            (71, Some(1)) => Ok(Self::StrNe),
            (71, Some(2)) => Ok(Self::StrLt),
            (71, Some(3)) => Ok(Self::StrLe),
            (71, Some(4)) => Ok(Self::StrGt),
            (71, Some(5)) => Ok(Self::StrGe),
            (72, None) => Ok(Self::StrLen),
            (47, Some(0)) => Ok(Self::ByteEq),
            (47, Some(1)) => Ok(Self::ByteNe),
            (47, Some(2)) => Ok(Self::ByteLt),
            (47, Some(3)) => Ok(Self::ByteLe),
            (47, Some(4)) => Ok(Self::ByteGt),
            (47, Some(5)) => Ok(Self::ByteGe),
            (48, Some(0)) => Ok(Self::CharEq),
            (48, Some(1)) => Ok(Self::CharNe),
            (48, Some(2)) => Ok(Self::CharLt),
            (48, Some(3)) => Ok(Self::CharLe),
            (48, Some(4)) => Ok(Self::CharGt),
            (48, Some(5)) => Ok(Self::CharGe),
            (49, Some(0)) => Ok(Self::FloatEq),
            (49, Some(1)) => Ok(Self::FloatNe),
            (49, Some(2)) => Ok(Self::FloatLt),
            (49, Some(3)) => Ok(Self::FloatLe),
            (49, Some(4)) => Ok(Self::FloatGt),
            (49, Some(5)) => Ok(Self::FloatGe),
            (50, Some(0)) => Ok(Self::RatEq),
            (50, Some(1)) => Ok(Self::RatNe),
            (50, Some(2)) => Ok(Self::RatLt),
            (50, Some(3)) => Ok(Self::RatLe),
            (50, Some(4)) => Ok(Self::RatGt),
            (50, Some(5)) => Ok(Self::RatGe),
            (51, None) => Ok(Self::RatNew),
            (52, Some(0)) => Ok(Self::EnumEq),
            (52, Some(1)) => Ok(Self::EnumNe),
            (53, Some(0)) => Ok(Self::ValEq),
            (53, Some(1)) => Ok(Self::ValNe),
            (54, None) => Ok(Self::VariantTag),
            (55, None) => Ok(Self::VariantGet),
            (26, None) => Ok(Self::Convert),
            (84, None) => Ok(Self::RangeNew),
            (80, None) => Ok(Self::ArrayNew),
            (81, None) => Ok(Self::ArrayLen),
            (85, Some(0)) => Ok(Self::RangeStart),
            (85, Some(1)) => Ok(Self::RangeEnd),
            (85, Some(2)) => Ok(Self::RangeLen),
            (90, None) => Ok(Self::ObjNew),
            (91, None) => Ok(Self::ObjInit),
            (92, None) => Ok(Self::ObjGet),
            (93, None) => Ok(Self::ObjNext),
            (94, None) => Ok(Self::EnumCase),
            (136, None) => Ok(Self::FailSecondary),
            (137, None) => Ok(Self::FailCaused),
            (56, Some(0)) => Ok(Self::MapNew),
            (56, Some(1)) => Ok(Self::MapLen),
            (56, Some(2)) => Ok(Self::MapHas),
            (56, Some(3)) => Ok(Self::MapGet),
            (56, Some(4)) => Ok(Self::MapKeys),
            (56, Some(5)) => Ok(Self::MapValues),
            (56, Some(6)) => Ok(Self::MapKeyAt),
            (56, Some(7)) => Ok(Self::MapValueAt),
            (56, Some(8)) => Ok(Self::MapMerge),
            (56, Some(9)) => Ok(Self::MapDifference),
            (57, Some(0)) => Ok(Self::ArrayConcat),
            (57, Some(1)) => Ok(Self::ArrayJoin),
            (57, Some(2)) => Ok(Self::ArraySlice),
            (57, Some(3)) => Ok(Self::ArrayRemove),
            (57, Some(4)) => Ok(Self::ArrayReplace),
            (57, Some(5)) => Ok(Self::ArrayDifference),
            (58, None) => Ok(Self::BuilderNew),
            (150, Some(0)) => Ok(Self::TestOk),
            (150, Some(1)) => Ok(Self::TestIs),
            (150, Some(2)) => Ok(Self::TestIsnt),
            (150, Some(3)) => Ok(Self::TestFail),
            _ => Err(r.fail(format!("unknown scalar opcode {code}/{sub:?}"))),
        }
    }
}

impl Wire for Instruction {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Data { op, inputs, output, immediate } => {
                let (code, sub) = op.wire();
                w.opcode(code, opcode_name(code).expect("registered scalar"));
                if let Some(sub) = sub {
                    sub.put(w);
                }
                inputs.put(w);
                output.put(w);
                immediate.put(w);
            }
            Self::Br(edge) => {
                w.opcode(140, "br");
                edge.put(w);
            }
            Self::Cond { condition, yes, no } => {
                w.opcode(141, "cond");
                condition.put(w);
                yes.put(w);
                no.put(w);
            }
            Self::Ret(values) => {
                w.opcode(142, "ret");
                values.put(w);
            }
            Self::Raise(value) => {
                w.opcode(147, "raise");
                value.put(w);
            }
            Self::Unreachable => w.opcode(145, "unreachable"),
            Self::Champ { op, arguments, flow } => {
                w.opcode(255, "champ");
                if w.text.is_some() {
                    w.label(op.name());
                } else {
                    op.id().put(w);
                }
                arguments.put(w);
                flow.put(w);
            }
        }
        w.label("\n");
    }

    fn get(r: &mut Reader<'_>) -> Result<Self, Error> {
        let code = r.opcode()?;
        Ok(match code {
            140 => Self::Br(Edge::get(r)?),
            141 => Self::Cond { condition: u32::get(r)?, yes: Edge::get(r)?, no: Edge::get(r)? },
            142 => Self::Ret(Vec::get(r)?),
            145 => Self::Unreachable,
            147 => Self::Raise(u32::get(r)?),
            255 => Self::Champ {
                op: if r.tokens.is_some() {
                    let name = r.word()?;
                    Opcode::ALL.iter().copied().find(|op| op.name() == name)
                        .ok_or_else(|| r.fail(format!("unknown Champ opcode `{name}`")))?
                } else {
                    Opcode::from_id(r.number()?)?
                },
                arguments: Vec::get(r)?,
                flow: Flow::get(r)?,
            },
            code => Self::Data {
                op: DataOp::read(code, r)?,
                inputs: Vec::get(r)?,
                output: Option::get(r)?,
                immediate: Immediate::get(r)?,
            },
        })
    }
}

fn header(bytes: &[u8]) -> Result<Reader<'_>, Error> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(error("SPB 17 file exceeds 256 MiB"));
    }
    let mut r = Reader::new(bytes);
    if r.take(4)? != b"SPRW" {
        return Err(error("not a Sparrow bytecode file"));
    }
    let version_bytes = r.take(2)?;
    let version = u16::from_be_bytes([version_bytes[0], version_bytes[1]]);
    if version != VERSION {
        return Err(error(format!("unsupported SPB version {version}; this reader requires SPB 17")));
    }
    Ok(r)
}

/// Gate precedes the first table count; no reader fallback or ABI guessing.
pub fn decode(bytes: &[u8]) -> Result<Program, Error> {
    let mut r = header(bytes)?;
    let program = Program::get(&mut r)?;
    r.end()?;
    verify(&program)?;
    Ok(program)
}

pub fn encode(program: &Program) -> Result<Vec<u8>, Error> {
    verify(program)?;
    let mut w = Writer::default();
    w.bytes.extend_from_slice(b"SPRW\0\x11");
    program.put(&mut w);
    if w.bytes.len() > MAX_FILE_BYTES {
        return Err(error("SPB 17 file exceeds 256 MiB"));
    }
    Ok(w.bytes)
}

pub fn disassemble(program: &Program) -> Result<String, Error> {
    verify(program)?;
    let mut w = Writer { text: Some("sparrow 17\n".into()), ..Writer::default() };
    program.put(&mut w);
    Ok(w.text.expect("assembly writer"))
}

pub fn assemble(text: &str) -> Result<Program, Error> {
    if text.len() > MAX_FILE_BYTES {
        return Err(error("assembly exceeds 256 MiB"));
    }
    let mut header = text.split_whitespace();
    if header.next() != Some("sparrow") || header.next() != Some("17") {
        return Err(error("assembly reader requires sparrow 17"));
    }
    let mut r = Reader::new(&[]);
    r.tokens = Some(tokenize(text)?);
    r.label("sparrow")?;
    if r.number()? != u32::from(VERSION) {
        return Err(error("assembly reader requires sparrow 17"));
    }
    let program = Program::get(&mut r)?;
    r.end()?;
    verify(&program)?;
    Ok(program)
}

fn tokenize(text: &str) -> Result<std::collections::VecDeque<Token>, Error> {
    let mut out = std::collections::VecDeque::new();
    let mut chars = text.chars().peekable();
    let mut budget = 0usize;
    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        if ch == '#' {
            for ch in chars.by_ref() {
                if ch == '\n' {
                    break;
                }
            }
            continue;
        }
        let token = if ch == '"' {
            let mut value = String::new();
            loop {
                match chars.next() {
                    Some('"') => break,
                    Some('\\') => value.push(match chars.next() {
                        Some('n') => '\n',
                        Some('r') => '\r',
                        Some('t') => '\t',
                        Some('"') => '"',
                        Some('\\') => '\\',
                        _ => return Err(error("invalid assembly string escape")),
                    }),
                    Some(ch) => value.push(ch),
                    None => return Err(error("unterminated assembly string")),
                }
            }
            Token::Text(value)
        } else {
            let mut word = String::from(ch);
            while chars.peek().is_some_and(|ch| !ch.is_whitespace()) {
                word.push(chars.next().expect("peeked"));
            }
            Token::Word(word)
        };
        let size = match &token { Token::Word(text) | Token::Text(text) => text.len() };
        budget = budget.saturating_add(std::mem::size_of::<Token>()).saturating_add(size);
        if budget > MAX_DECODED_BYTES {
            return Err(error("assembly token budget exceeded"));
        }
        out.push_back(token);
    }
    Ok(out)
}

#[cfg(test)]
pub(super) fn encode_unchecked(program: &Program) -> Vec<u8> {
    let mut w = Writer::default();
    w.bytes.extend_from_slice(b"SPRW\0\x11");
    program.put(&mut w);
    w.bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s1_decoded_budget_is_charged_before_storage_allocation() {
        let mut r = Reader::new(&[2, 0, 0]);
        assert!(r.count::<[u8; MAX_DECODED_BYTES]>().unwrap_err().message.contains("512 MiB"));
        let mut r = Reader::new(&[3, 0]);
        assert!(r.count::<u8>().unwrap_err().message.contains("remaining"));
    }
}

record! { EntryContract { external, retained_params, fresh_singleton, failures, destroy } }
record! { CloseContract { begin_pc, boundary, reserve, active_failure, envelope } }
record! { CleanupPlan { plan, reserve, close, reserve_empty, sources, slots, preserves, joins, suffixes } }
record! { DirectSlot { identity, descriptor, destroy, origin, active_pc } }
record! { CleanupSuffix { completed, remaining, roots } }
tagged! { MergeSource { 0 => Scope, 1 => Packet, 2 => Reserve } }
impl Wire for CreationOrigin {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Singleton => w.number(0),
            Self::Create(pc) => {
                w.number(1);
                pc.put(w);
            }
            Self::FreshResult { pc, callee } => {
                w.number(2);
                pc.put(w);
                callee.put(w);
            }
        }
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, Error> {
        match r.number()? {
            0 => Ok(Self::Singleton),
            1 => Ok(Self::Create(u32::get(r)?)),
            2 => Ok(Self::FreshResult { pc: u32::get(r)?, callee: u32::get(r)? }),
            _ => Err(r.fail("unknown creation origin")),
        }
    }
}

tagged! { ExitDestination { 0 => Caller, 1 => Loop(id: u32), 2 => Scope(id: u32) } }

record! { StaticSelection { pc, selection, input, indices } }
