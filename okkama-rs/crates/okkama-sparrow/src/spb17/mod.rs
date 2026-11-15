//! Champ SPB 17 data transformations. Never selected by the default runtime.
//! Wire format and P1 IDs follow Champ G1 revision 2, §§4 and 9.
mod allocation;
mod codec;
pub mod layout;
mod registry;
pub mod p1;
mod verify;
mod load;
mod evidence;
pub use evidence::{static_selections, plain_inputs, plain_results, redundant_admission_filters};
mod automaton;
pub use automaton::infer_plan_reserves;
mod suspension;

pub use allocation::{AllocatedRoot, Allocation, Copy, CopySource, allocate, check_allocation, live_locations};
pub use codec::{assemble, decode, disassemble, encode};
pub use registry::{CallAbi, CleanupAbi, Helper, HelperClass, MachineArgument, Opcode, ProducedType, ResultContext};
pub use registry::Tag;
pub use registry::{helper, HELPER_TABLE, HELPER_ALLOCATES, HELPER_COLLECTS, HELPER_REENTERS, HELPER_UNWINDS, HELPER_STOPS};
pub use registry::HELPER_SUSPENDS;
pub use verify::{any_reaches_resource, implementation, result_context, role_reaches_resource, verify};
pub use load::{ExternalContracts, NoExternalContracts, verify_load, service_states};

pub const VERSION: u16 = 17;
/// Stable load-gate diagnostics; prose can change without breaking consumers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub offset: Option<usize>,
    pub message: String,
    pub rule: &'static str,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.rule, self.message)
    }
}
impl std::error::Error for Error {}
impl From<Error> for crate::CodecError {
    fn from(error: Error) -> Self {
        Self { offset: error.offset, message: error.to_string() }
    }
}

pub(crate) fn violation(rule: &'static str, message: impl Into<String>) -> Error {
    Error { offset: None, message: message.into(), rule }
}

pub(crate) fn enforce(condition: bool, rule: &'static str, message: &str) -> Result<(), Error> {
    if condition { Ok(()) } else { Err(violation(rule, message)) }
}

pub(crate) fn error(message: impl Into<String>) -> Error {
    violation("SPB_STRUCTURE", message)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Program {
    pub module: String,
    pub version: String,
    pub objects: Vec<Object>,
    pub tables: ChampTables,
    pub functions: Vec<Function>,
    pub entry: Option<u32>,
    /// The test profile (§14): the tests in run order. `Some([])` is an empty suite, which the runner
    /// rejects; `None` is a production program. Test operations and test intrinsics need `Some`.
    pub tests: Option<Vec<Test>>,
    /// Map and builder shapes named by `Type::Map`, `Type::MapBuilder` and `Type::Builder`. An entry refers
    /// only to earlier entries, so the table is acyclic (`COMPOSITE_ORDER`).
    pub composites: Vec<Composite>,
}

/// A collection shape that does not fit inline in `Type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Composite {
    /// `map<key, value>`: the key is a key type (`MAP_KEY`), the value closed plain data (`MAP_PLAIN`).
    Map { key: Type, value: Type },
    /// A unique array builder whose finished value has type `array` (an `Array` or `Bytes`).
    Builder { array: Type },
}

/// One test of the test profile: a `() -> void` observation entry run on a fresh runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Test {
    pub name: String,
    pub function: u32,
}

/// The test intrinsics (`okkama.test.*` external entries) and test operations need a tests table.
pub const TEST_INTRINSIC_PREFIX: &str = "okkama.test.";

/// Immutable final descriptor supplied by the runtime layout registry (§2.7).
/// Physical storage includes hidden construction/$once slots, never the tail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectRuntimeLayout {
    pub storage_fields: Vec<Type>,
    pub allocation_words: u32,
    pub identity_offset: Option<u32>,
    pub has_destroy: bool,
    pub may_reach_resource: bool,
    pub may_async_destroy: bool,
    pub trace_layout: Vec<u32>,
    pub selection_layout: Vec<u32>,
}

impl ObjectRuntimeLayout {
    pub fn storage_field_count(&self) -> usize {
        self.storage_fields.len()
    }
}

/// Maps source field indices to final physical storage indices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Object {
    pub error_cases: Vec<String>,
    /// Number of cases of a plain (non-Error) enum; zero for every other declaration.
    pub enum_cases: u32,
    /// Case object descriptors of a plain (non-Error) variant, in case order; empty otherwise.
    pub variant_cases: Vec<u32>,
    /// A native handle declaration: no fields, no storage and never constructed by a program.
    pub handle: bool,
    pub name: String,
    pub fields: Vec<Field>,
    pub layout: ObjectRuntimeLayout,
    /// A role declaration: a placeholder descriptor that names a role type, never an allocated object.
    pub role: Option<RoleDecl>,
    /// The roles this object implements, one implementation table per role (`ROLE_IMPL`).
    pub implements: Vec<RoleImpl>,
    /// An any box: the one field holds a boxed value, and the descriptor names its type (`ANY_BOX`).
    pub any_box: bool,
    /// The case texts (`Enum.case`) of a plain enum, in case order, or empty (`ENUM_NAMES`).
    pub enum_names: Vec<String>,
}

/// The slots of a role: each slot's entry ABI and its signature without the receiver.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleDecl {
    pub slots: Vec<RoleSlot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleSlot {
    pub name: String,
    pub entry_abi: EntryAbi,
    pub params: Vec<Type>,
    pub results: Vec<Type>,
}

/// One object's implementation table for one role: the function of every slot, in slot order.
/// Each function takes the implementing object first, then the slot's parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleImpl {
    pub role: u32,
    pub slots: Vec<u32>,
}

/// A dynamic call of one role slot. The receiver's object descriptor selects its implementation; only the
/// listed objects may be dispatched to, every other receiver stops the machine. Every listed implementation
/// refines the site contract: its failures and retained parameters (language positions, receiver first)
/// are within `failures` and `retained_params` (`DISPATCH_TARGET`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dispatch {
    pub role: u32,
    pub slot: u32,
    pub failures: u32,
    pub retained_params: Vec<u32>,
    pub targets: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub storage: u32,
    pub once: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Error,
    Bytes,
    Array(ArrayType),
    Int,
    Bool,
    /// Raw words, as in Tomato: a byte 0..=255, an IEEE-754 binary64 bit pattern, a Unicode scalar value.
    Byte,
    Float,
    Char,
    /// A reference to Tomato's exact rational.
    Rat,
    Str,
    Range,
    Failure,
    Object(u32),
    Successor(u32),
    /// A value of a plain variant declaration: a reference to one of its case objects (`Object::variant_cases`).
    Variant(u32),
    /// A native handle (`native object`): a reference to the native registry's state, declared by
    /// the handle object `Object::handle` with this index. It owns no Okkama obligation (P2.4 §4.7).
    Handle(u32),
    Never,
    Descriptor,
    /// A role value: a reference to an object that implements the role declaration `id` (`Object::role`).
    Role(u32),
    /// A role value that still carries a successor's must-use permission (`^Role`).
    RoleSuccessor(u32),
    /// A finished map: `Composite::Map` at this index of `Program::composites`.
    Map(u32),
    /// A unique map builder whose finished value is `Type::Map` of the same index.
    MapBuilder(u32),
    /// A unique array builder: `Composite::Builder` at this index.
    Builder(u32),
    /// A Future: a reference to its frame. Its outcome type is `ChampTables::future_results` at this index.
    Future(u32),
    /// An `any` value: a reference to an any box (`Object::any_box`), which holds the value in its one
    /// field and the value's type in its descriptor.
    Any,
}

impl Type {
    /// Words the collector must trace as references (tagged integers are traced separately).
    pub fn traced(self) -> bool {
        matches!(self, Self::Error | Self::Bytes | Self::Array(_) | Self::Str | Self::Object(_) | Self::Successor(_)
            | Self::Failure | Self::Rat | Self::Variant(_) | Self::Handle(_) | Self::Map(_) | Self::MapBuilder(_)
            | Self::Builder(_) | Self::Future(_) | Self::Role(_) | Self::RoleSuccessor(_) | Self::Any)
    }
}

/// A finite array shape, including nested plain arrays. No resource/service leaf is representable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ArrayType {
    pub leaf: ArrayLeaf,
    pub depth: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArrayLeaf {
    Int,
    Bool,
    Str,
    Range,
    Bytes,
    Object(u32),
    Role(u32),
    Any,
    Float,
    Char,
    Rat,
    Variant(u32),
    Map(u32),
    /// The leaf of an uninferred empty literal: no element value exists.
    Never,
}

impl ArrayType {
    pub fn element(self) -> Option<Type> {
        match self.depth {
            0 => None,
            1 => Some(match self.leaf {
                ArrayLeaf::Int => Type::Int,
                ArrayLeaf::Bool => Type::Bool,
                ArrayLeaf::Str => Type::Str,
                ArrayLeaf::Range => Type::Range,
                ArrayLeaf::Bytes => Type::Bytes,
                ArrayLeaf::Object(id) => Type::Object(id),
                ArrayLeaf::Role(id) => Type::Role(id),
                ArrayLeaf::Any => Type::Any,
                ArrayLeaf::Float => Type::Float,
                ArrayLeaf::Char => Type::Char,
                ArrayLeaf::Rat => Type::Rat,
                ArrayLeaf::Variant(id) => Type::Variant(id),
                ArrayLeaf::Map(id) => Type::Map(id),
                ArrayLeaf::Never => Type::Never,
            }),
            depth => Some(Type::Array(Self { depth: depth - 1, ..self })),
        }
    }

    /// The array whose elements have type `element`; `None` for an element no array leaf represents.
    /// An array of bytes is `Type::Bytes`, not an array shape.
    pub fn of(element: Type) -> Option<Type> {
        let leaf = match element {
            Type::Int => ArrayLeaf::Int,
            Type::Bool => ArrayLeaf::Bool,
            Type::Str => ArrayLeaf::Str,
            Type::Range => ArrayLeaf::Range,
            Type::Bytes => ArrayLeaf::Bytes,
            Type::Object(id) => ArrayLeaf::Object(id),
            Type::Role(id) => ArrayLeaf::Role(id),
            Type::Any => ArrayLeaf::Any,
            Type::Float => ArrayLeaf::Float,
            Type::Char => ArrayLeaf::Char,
            Type::Rat => ArrayLeaf::Rat,
            Type::Variant(id) => ArrayLeaf::Variant(id),
            Type::Map(id) => ArrayLeaf::Map(id),
            Type::Never => ArrayLeaf::Never,
            Type::Byte => return Some(Type::Bytes),
            Type::Array(shape) => {
                return shape.depth.checked_add(1).map(|depth| Type::Array(Self { depth, ..shape }));
            }
            _ => return None,
        };
        Some(Type::Array(Self { leaf, depth: 1 }))
    }
}

/// A retag of a collection whose leaves cannot exist (an uninferred `[]` or `[:]`) to a collection of the same
/// shape: no element is reinterpreted. Byte arrays have their own representation and are excluded. A map
/// without keys has no values either, but its value type is kept unless it is `never` too: a value word is
/// never read as another type.
pub fn never_retag(composites: &[Composite], from: Type, to: Type) -> bool {
    match (from, to) {
        (Type::Array(from), Type::Array(to)) => from.leaf == ArrayLeaf::Never && from.depth == to.depth,
        (Type::Map(from), Type::Map(to)) => match (composites.get(from as usize), composites.get(to as usize)) {
            (Some(Composite::Map { key: Type::Never, value: old }), Some(Composite::Map { value: new, .. })) => {
                *old == Type::Never || old == new
            }
            _ => false,
        },
        _ => false,
    }
}

/// The element type of an array or byte array value.
pub fn element_type(array: Type) -> Option<Type> {
    match array {
        Type::Array(shape) => shape.element(),
        Type::Bytes => Some(Type::Byte),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ServiceKind {
    Packet,
    ScopeState,
    Pending,
    Cleanup,
    StableKey,
    FailureState,
    ExitState,
    Closed,
    DestroyBorrow,
    /// Proof position only: zero machine words, not a GC pointer.
    Deps,
    /// The running async body's frame token (OIR `frame<@f>`): linear, zero words. The frame word itself is
    /// the activation's entry argument, never a slot.
    Frame,
    /// The owner's cache duty of one Future: one word, the Future frame.
    CacheState,
    /// A waiter's handle on the Future it awaited, from an await's unwind outcome: one word, the Future frame.
    TaskState,
    /// An await receipt (NotStarted 0, Settled 1, Withdrawn 2, ChildCancelled 3): one raw word.
    Receipt,
}

impl ServiceKind {
    /// A linear token: consumed exactly once on every path, never copied.
    pub fn linear(self) -> bool {
        matches!(self, Self::Packet | Self::ScopeState | Self::Pending | Self::Cleanup | Self::StableKey
            | Self::FailureState | Self::ExitState | Self::Frame | Self::CacheState | Self::TaskState | Self::Receipt)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Location {
    Register(u32),
    Service(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Location(Location),
    ProducedSlot(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assignment {
    pub destination: Location,
    pub source: Source,
}

/// Sources are read simultaneously, before any destination is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub target_pc: u32,
    pub assignments: Vec<Assignment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub tag: Tag,
    pub edge: Edge,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Flow {
    Next(Vec<Location>),
    Outcomes(Vec<Outcome>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryAbi {
    Observation,
    Owned,
    Defer { labels: Vec<DeferLabel> },
    /// An async body (Champ draft 3 §13): it takes exactly its frame and publishes its outcome through
    /// `fut.publish`. `layout` indexes `ChampTables::frames`.
    AsyncBody { layout: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferLabel {
    pub label: u32,
    pub result: Option<Type>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitKind {
    Return,
    Last,
    Next,
    Fallthrough,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationLabel {
    pub label: u32,
    pub kind: ExitKind,
    pub boundary: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationMap {
    pub entry: u32,
    pub labels: Vec<RegistrationLabel>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub symbol: String,
    pub entry_abi: EntryAbi,
    pub registers: Vec<Type>,
    pub services: Vec<ServiceKind>,
    pub params: Vec<Location>,
    pub results: Vec<Type>,
    pub blocks: Vec<Block>,
    pub proofs: FunctionProofs,
    pub code: Vec<Instruction>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub pc: u32,
    pub params: Vec<Location>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Constant {
    Int(String),
    Bool(bool),
    Str(String),
    Descriptor(u32),
    Byte(u32),
    Char(u32),
    /// The IEEE-754 binary64 bit pattern as a decimal u64.
    Float(String),
    /// An exact rational in lowest terms with a positive denominator.
    Rat(RatValue),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RatValue {
    pub numerator: String,
    pub denominator: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Immediate {
    None,
    Constant(Constant),
    Inclusive(bool),
    Object(u32),
    Field(u32),
    Updates(Vec<u32>),
}

/// A checked synchronous dynamic transfer protocol (`own.dynamic`, OIR-2 §6.4, §7.1): the open Closure
/// protocol of an open action call's result packet, or the `any` protocol of a boxed value's packet. Both
/// move a packet of unknown retention into the scope inventory after the owner, live-dependency, snapshot
/// and transferability checks; on failure the packet comes back unaccepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Protocol {
    Closure,
    Any,
}

/// Operands are typed references, not an untyped helper argument buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Argument {
    Evaluation(Evaluation),
    Result(Option<Type>),
    Location(Location),
    Scope(u32),
    Boundary(u32),
    Summary(u32),
    Selection(u32),
    Plan(u32),
    Schedule(u32),
    Envelope(u32),
    Entry(u32),
    Object(u32),
    Binding(u32),
    Field(u32),
    Label(u32),
    Flag(bool),
    Kind(ExitKind),
    Registration(u32),
    Destination(ExitDestination),
    /// A continuation of the function's frame layout (`frame.save`, `frame.resume`).
    Continuation(u32),
    Dispatch(Dispatch),
    /// The protocol of `own.dynamic`.
    Protocol(Protocol),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    pub op: DataOp,
    pub result: Type,
    pub immediate: Immediate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Instruction {
    Data {
        op: DataOp,
        inputs: Vec<u32>,
        output: Option<u32>,
        immediate: Immediate,
    },
    Br(Edge),
    Cond { condition: u32, yes: Edge, no: Edge },
    Ret(Vec<u32>),
    Unreachable,
    Raise(u32),
    Champ { op: Opcode, arguments: Vec<Argument>, flow: Flow },
}

// Scalar families use their existing IDs. range.len and switch expand into
// sequences; obj.pending reads its registered bool field via obj.get.
// Only Champ operations introduce instructions, using the 0xff escape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataOp {
    Const,
    Move,
    Undef,
    IntAdd,
    IntSub,
    IntMul,
    IntMod,
    IntNeg,
    IntEq,
    IntNe,
    IntLt,
    IntLe,
    IntGt,
    IntGe,
    BoolNot,
    BoolEq,
    BoolNe,
    StrConcat,
    StrEq,
    StrNe,
    StrLt,
    StrLe,
    StrGt,
    StrGe,
    StrLen,
    ByteEq,
    ByteNe,
    ByteLt,
    ByteLe,
    ByteGt,
    ByteGe,
    CharEq,
    CharNe,
    CharLt,
    CharLe,
    CharGt,
    CharGe,
    FloatEq,
    FloatNe,
    FloatLt,
    FloatLe,
    FloatGt,
    FloatGe,
    RatEq,
    RatNe,
    RatLt,
    RatLe,
    RatGt,
    RatGe,
    /// Exact `int / int` and `rat(n, d)`; a zero denominator fails.
    RatNew,
    EnumEq,
    EnumNe,
    /// Deep value equality of plain objects (§16.3).
    ValEq,
    ValNe,
    /// The active case number of a variant value.
    VariantTag,
    /// A payload field of one case (`Updates([case, storage])`); another active case fails.
    VariantGet,
    Convert,
    RangeNew,
    RangeStart,
    RangeEnd,
    RangeLen,
    ObjNew,
    ObjInit,
    ObjGet,
    ObjNext,
    EnumCase,
    ArrayNew,
    ArrayLen,
    FailSecondary,
    FailCaused,
    /// A checked conversion of a role value to another role: fails unless the object implements it.
    RoleNarrow,
    /// `map.new k0, v0, k1, v1, ...`: later equal keys replace the value and keep the first key word.
    MapNew,
    MapLen,
    MapHas,
    /// The value under a key; a missing key fails.
    MapGet,
    MapKeys,
    MapValues,
    /// The key or value at a position in key order; a position outside the map fails.
    MapKeyAt,
    MapValueAt,
    /// The union of maps: the rightmost value wins, the leftmost key word stays.
    MapMerge,
    /// The first map without the keys of the others.
    MapDifference,
    ArrayConcat,
    /// The elements' text joined by a separator (string elements only).
    ArrayJoin,
    /// `array.slice a, start, end`; bounds outside the array fail.
    ArraySlice,
    /// A copy without the element at an index; an index outside the array fails.
    ArrayRemove,
    /// A copy with the element at an index replaced; an index outside the array fails.
    ArrayReplace,
    /// The first array without the elements equal to an element of the others.
    ArrayDifference,
    /// An empty unique builder of the result's builder type.
    BuilderNew,
    /// Test assertions (`test.ok`, `test.is`, `test.isnt`, `test.fail`) of the test profile. Each
    /// records a failed check in the running case and continues; the bool result says whether the
    /// check held. Outside a running case each fails with a program failure.
    TestOk,
    TestIs,
    TestIsnt,
    TestFail,
    // int.ext (wire 14): exact int operations beyond add/sub/mul/mod/neg. Bit operations and shifts take
    // a signed 64-bit operand and fail outside it; shift and rotation counts fail outside 0..63.
    IntAnd,
    IntOr,
    IntXor,
    IntShl,
    IntShr,
    IntUshr,
    IntNot,
    IntAbs,
    IntSign,
    IntBitCount,
    IntLeadingZeros,
    IntTrailingZeros,
    IntReverseBits,
    IntSwapBytes,
    IntRotl,
    IntRotr,
    // byte.ext (wire 15). Shift and rotation counts fail outside 0..7; byte.ushr counts in byte.
    ByteAnd,
    ByteOr,
    ByteXor,
    ByteShl,
    ByteShr,
    ByteUshr,
    ByteNot,
    ByteBitCount,
    ByteLeadingZeros,
    ByteTrailingZeros,
    ByteReverseBits,
    ByteSwapBytes,
    ByteRotl,
    ByteRotr,
    // float.ext (wire 16). A result that is not finite fails, as do a zero divisor and a negative sqrt.
    FloatAdd,
    FloatSub,
    FloatMul,
    FloatDiv,
    FloatMod,
    FloatPow,
    FloatNeg,
    FloatAbs,
    FloatSign,
    FloatSqrt,
    // rat.ext (wire 17): exact rationals; a zero divisor fails. Numerator and denominator are decimal text.
    RatAdd,
    RatSub,
    RatMul,
    RatDiv,
    RatMod,
    RatNeg,
    RatAbs,
    RatSign,
    RatNumerator,
    RatDenominator,
    // str.ext (wire 18). Slice bounds and a negative repeat count fail.
    StrSlice,
    StrSplit,
    StrReplace,
    StrReplaceFirst,
    StrRepeat,
    StrTrim,
    StrChomp,
    StrTrimStart,
    StrTrimEnd,
    StrLower,
    StrUpper,
    StrFind,
    StrFindLast,
    StrStartsWith,
    StrEndsWith,
    StrUtf8,
    /// The code points of a text as an array of char (#679).
    StrCharacters,
}

/// The int, byte, float, rat and str families of wire codes 14..=18, in sub-code order. A family's
/// position in its list is its sub-code; the list never reorders and only grows at its end.
pub(crate) const SCALAR_FAMILIES: [(u8, &str, &[DataOp]); 5] = {
    use DataOp::*;
    [
        (14, "int.ext", &[IntAnd, IntOr, IntXor, IntShl, IntShr, IntUshr, IntNot, IntAbs, IntSign, IntBitCount,
            IntLeadingZeros, IntTrailingZeros, IntReverseBits, IntSwapBytes, IntRotl, IntRotr]),
        (15, "byte.ext", &[ByteAnd, ByteOr, ByteXor, ByteShl, ByteShr, ByteUshr, ByteNot, ByteBitCount,
            ByteLeadingZeros, ByteTrailingZeros, ByteReverseBits, ByteSwapBytes, ByteRotl, ByteRotr]),
        (16, "float.ext", &[FloatAdd, FloatSub, FloatMul, FloatDiv, FloatMod, FloatPow, FloatNeg, FloatAbs,
            FloatSign, FloatSqrt]),
        (17, "rat.ext", &[RatAdd, RatSub, RatMul, RatDiv, RatMod, RatNeg, RatAbs, RatSign, RatNumerator,
            RatDenominator]),
        (18, "str.ext", &[StrSlice, StrSplit, StrReplace, StrReplaceFirst, StrRepeat, StrTrim, StrChomp,
            StrTrimStart, StrTrimEnd, StrLower, StrUpper, StrFind, StrFindLast, StrStartsWith, StrEndsWith,
            StrUtf8, StrCharacters]),
    ]
};

impl DataOp {
    /// The wire code and sub-code of an operation of the scalar families (wire codes 14..=18).
    pub(crate) fn family(self) -> Option<(u8, u32)> {
        SCALAR_FAMILIES.iter().find_map(|(code, _, ops)| {
            ops.iter().position(|op| *op == self).map(|sub| (*code, sub as u32))
        })
    }

    /// The operation of a scalar family code and sub-code.
    pub(crate) fn of_family(code: u8, sub: u32) -> Option<Self> {
        let (_, _, ops) = SCALAR_FAMILIES.iter().find(|(family, _, _)| *family == code)?;
        ops.get(sub as usize).copied()
    }

    /// A stable number for a scalar family operation (wire code × 256 + sub-code), which a runtime helper
    /// receives instead of the operation; `from_scalar_code` inverts it.
    pub fn scalar_code(self) -> Option<u64> {
        self.family().map(|(code, sub)| u64::from(code) * 256 + u64::from(sub))
    }

    pub fn from_scalar_code(code: u64) -> Option<Self> {
        let family = u8::try_from(code / 256).ok()?;
        Self::of_family(family, (code % 256) as u32)
    }

    /// Operand and result types of a scalar family operation (wire codes 14..=18).
    pub fn scalar_signature(self) -> Option<(&'static [Type], Type)> {
        use DataOp::*;
        use Type::{Bool, Byte, Bytes, Float, Int, Rat, Str};
        const STR_ARRAY: Type = Type::Array(ArrayType { leaf: ArrayLeaf::Str, depth: 1 });
        const CHAR_ARRAY: Type = Type::Array(ArrayType { leaf: ArrayLeaf::Char, depth: 1 });
        Some(match self {
            IntAnd | IntOr | IntXor | IntShl | IntShr | IntUshr | IntRotl | IntRotr => (&[Int, Int], Int),
            IntNot | IntAbs | IntSign | IntBitCount | IntLeadingZeros | IntTrailingZeros | IntReverseBits
                | IntSwapBytes => (&[Int], Int),
            ByteAnd | ByteOr | ByteXor | ByteUshr => (&[Byte, Byte], Byte),
            ByteShl | ByteShr | ByteRotl | ByteRotr => (&[Byte, Int], Byte),
            ByteNot | ByteReverseBits | ByteSwapBytes => (&[Byte], Byte),
            ByteBitCount | ByteLeadingZeros | ByteTrailingZeros => (&[Byte], Int),
            FloatAdd | FloatSub | FloatMul | FloatDiv | FloatMod | FloatPow => (&[Float, Float], Float),
            FloatNeg | FloatAbs | FloatSqrt => (&[Float], Float),
            FloatSign => (&[Float], Int),
            RatAdd | RatSub | RatMul | RatDiv | RatMod => (&[Rat, Rat], Rat),
            RatNeg | RatAbs => (&[Rat], Rat),
            RatSign => (&[Rat], Int),
            RatNumerator | RatDenominator => (&[Rat], Str),
            StrSlice => (&[Str, Int, Int], Str),
            StrSplit => (&[Str, Str], STR_ARRAY),
            StrReplace | StrReplaceFirst => (&[Str, Str, Str], Str),
            StrRepeat => (&[Str, Int], Str),
            StrTrim | StrChomp | StrTrimStart | StrTrimEnd | StrLower | StrUpper => (&[Str], Str),
            StrFind | StrFindLast => (&[Str, Str], Int),
            StrStartsWith | StrEndsWith => (&[Str, Str], Bool),
            StrUtf8 => (&[Str], Bytes),
            StrCharacters => (&[Str], CHAR_ARRAY),
            _ => return None,
        })
    }
}

impl DataOp {
    /// Operations that can fail with an application failure, given their operand and result types.
    /// Their data form has no outcome edge, so it is admitted only where nothing needs cleanup (no
    /// service locations); elsewhere the operation is an `evaluate` with explicit normal and unwind
    /// outcomes. A conversion fails only where its pair of types can trap (`conversion`).
    pub fn fallible(self, operands: &[Type], result: Option<Type>) -> bool {
        match self {
            Self::IntMod | Self::RangeNew | Self::RatNew | Self::ValEq | Self::ValNe | Self::VariantGet
                | Self::RoleNarrow | Self::MapGet | Self::MapKeyAt | Self::MapValueAt | Self::MapDifference
                | Self::ArraySlice
                | Self::ArrayRemove | Self::ArrayReplace | Self::ArrayDifference => true,
            // A joined string longer than the 4 GiB a string can hold fails, as in Tomato.
            Self::ArrayJoin => true,
            Self::TestOk | Self::TestIs | Self::TestIsnt | Self::TestFail => true,
            // Tomato's failures: an int outside signed 64 bits in a bit operation, a shift or rotation
            // count out of range, a float result that is not finite, a zero rat divisor, slice bounds and
            // a negative repeat count. Every other scalar family operation is total.
            Self::IntAnd | Self::IntOr | Self::IntXor | Self::IntShl | Self::IntShr | Self::IntUshr | Self::IntNot
                | Self::IntBitCount | Self::IntLeadingZeros | Self::IntTrailingZeros | Self::IntReverseBits
                | Self::IntSwapBytes | Self::IntRotl | Self::IntRotr => true,
            Self::ByteShl | Self::ByteShr | Self::ByteUshr | Self::ByteRotl | Self::ByteRotr => true,
            Self::FloatAdd | Self::FloatSub | Self::FloatMul | Self::FloatDiv | Self::FloatMod | Self::FloatPow
                | Self::FloatSqrt => true,
            Self::RatDiv | Self::RatMod | Self::StrSlice | Self::StrRepeat => true,
            Self::Convert => matches!((operands, result), ([from], Some(to)) if conversion(*from, to) == Some(true)),
            _ => false,
        }
    }
}

/// The conversions SPB 17 defines: `Some(true)` where the conversion can fail, `Some(false)` where it is
/// total, `None` where it is not a conversion. The same table as OIR-2 `cvt`.
pub fn conversion(from: Type, to: Type) -> Option<bool> {
    match (from, to) {
        // `char(n)` fails outside the Unicode scalar values (#679).
        (Type::Int, Type::Float | Type::Byte | Type::Char) | (Type::Float, Type::Int | Type::Byte)
            | (Type::Rat, Type::Float | Type::Byte) => Some(true),
        (Type::Byte, Type::Int | Type::Float | Type::Rat) | (Type::Int | Type::Float, Type::Rat)
            | (Type::Rat, Type::Int) | (Type::Char, Type::Int)
            | (Type::Int | Type::Byte | Type::Float | Type::Rat | Type::Bool | Type::Char, Type::Str) => {
            Some(false)
        }
        _ => None,
    }
}

impl Function {
    /// Whether a data instruction of this function can fail (`DataOp::fallible` on its register types).
    pub fn data_fallible(&self, instruction: &Instruction) -> bool {
        let Instruction::Data { op, inputs, output, .. } = instruction else { return false };
        let operands = inputs.iter()
            .map(|&id| self.registers.get(id as usize).copied().unwrap_or(Type::Never))
            .collect::<Vec<_>>();
        let result = output.and_then(|id| self.registers.get(id as usize).copied());
        op.fallible(&operands, result)
    }
}

/// Shared declarations. Function-local proof references never index these
/// arenas accidentally: their Argument variants select the appropriate arena.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChampTables {
    pub scopes: Vec<Scope>,
    pub boundaries: Vec<Boundary>,
    pub summaries: Vec<Summary>,
    /// D21 frame layouts, one for each async body (`EntryAbi::AsyncBody`).
    pub frames: Vec<FrameLayout>,
    /// The outcome value of each Future type (`Type::Future`); `None` is a void Future. Entries are distinct.
    pub future_results: Vec<Option<Type>>,
}

/// D21 layout of one async body: typed slots and its continuations. Continuation 0 is the start.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameLayout {
    pub slots: Vec<FrameSlot>,
    pub continuations: Vec<Continuation>,
}

/// One type per slot for the whole function. Machine words and traced offsets follow from the type
/// (`layout::service_layout` for services, the register representation for values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSlot {
    pub ty: ProducedType,
    pub linear: bool,
}

/// What one suspension stores in the frame. `site` is the pc of the suspending operation; the start has
/// none. `saved` lists the slots of `frame.save`/`frame.resume` in order; `operands` the slots of the
/// suspending operation's location operands, in operand order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Continuation {
    pub site: Option<u32>,
    pub saved: Vec<u32>,
    pub operands: Vec<u32>,
}

impl FrameLayout {
    /// The slot types of a continuation's saved set, in order.
    pub fn saved_types(&self, continuation: u32) -> Option<Vec<ProducedType>> {
        let continuation = self.continuations.get(continuation as usize)?;
        continuation.saved.iter().map(|&slot| self.slots.get(slot as usize).map(|slot| slot.ty)).collect()
    }
}

impl Program {
    /// The frame layout of an async body.
    pub fn frame_layout(&self, f: &Function) -> Option<&FrameLayout> {
        match f.entry_abi {
            EntryAbi::AsyncBody { layout } => self.tables.frames.get(layout as usize),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub function: u32,
    pub parent: Option<u32>,
    pub boundary: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Boundary {
    pub scope: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub entry: u32,
    pub may_fail: bool,
    pub retained_params: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionProofs {
    pub entry: Option<EntryContract>,
    pub static_selections: Vec<StaticSelection>,
    pub closes: Vec<CloseContract>,
    pub cleanup_plans: Vec<CleanupPlan>,
    pub post_roots: Vec<RootRequirement>,
    pub selections: Vec<Selection>,
    /// Conservative extra liveness edges only. Never authorize ownership transfer or omit a root.
    /// The loader reconstructs service lifetimes, inventory and origin independently. Actual operand
    /// liveness always participates in before/after root checking, even when no dependency is supplied.
    /// Extra edges can only retain storage longer; ordinary SSA availability is checked independently.
    pub dependencies: Vec<Dependency>,
    pub registrations: Vec<RegistrationMap>,
    pub plans: Vec<Plan>,
    pub order_witnesses: Vec<OrderWitness>,
    pub schedules: Vec<Schedule>,
    pub envelopes: Vec<Envelope>,
    pub states: Vec<StateRecord>,
    pub roots: Vec<RootRequirement>,
}

/// Abstract deps retain their proof positions and take zero machine words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    pub value: Location,
    pub obligations: Vec<Location>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub locations: Vec<Location>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub merge: bool,
    pub selection: u32,
    pub order_witness: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderWitness {
    pub creation_pcs: Vec<u32>,
}

/// D16 P1 schedules have an empty service set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub services: Vec<Location>,
}

/// D18 mask: Thrown=1, Program=2, Panic=4, Native=8, and `ENVELOPE_CANCEL` for the final exit of an async body
/// whose close delivers a deferred cancellation (`cleanup.deliver_cancel`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub failures: u32,
}

/// The D18 bit of a close that may deliver a deferred cancellation (OIR `scope_effects(cancel=true)`).
pub const ENVELOPE_CANCEL: u32 = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateRecord {
    pub pc: u32,
    pub locations: Vec<Location>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootRequirement {
    pub pc: u32,
    pub locations: Vec<Location>,
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod layout_tests;

/// Function contracts are verified against bodies or a trusted external catalog at load time.
/// In particular, escaping body failures (not just fresh_singleton) must fit the declared mask.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntryContract {
    pub external: Option<String>,
    pub retained_params: Vec<u32>,
    pub fresh_singleton: bool,
    pub failures: u32,
    pub destroy: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseContract {
    pub begin_pc: u32,
    pub boundary: u32,
    pub reserve: bool,
    pub active_failure: bool,
    pub envelope: u32,
}

/// Function-local IDs are relocated separately from runtime PlanId (which starts at one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupPlan {
    pub plan: u32,
    pub reserve: bool,
    pub close: u32,
    pub reserve_empty: bool,
    pub sources: Vec<MergeSource>,
    pub slots: Vec<DirectSlot>,
    /// Diagnostic Low relocation only; runtime/JIT must never use these as ownership/order authority.
    /// The loader derives inventory and creation origins from instructions and all incoming edges.
    pub preserves: Vec<Dependency>,
    /// Diagnostic Low relocation only, with the same non-authoritative status as preserves.
    pub joins: Vec<Dependency>,
    pub suffixes: Vec<CleanupSuffix>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeSource { Scope, Packet, Reserve }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectSlot {
    pub identity: Location,
    pub descriptor: Location,
    pub destroy: u32,
    pub origin: CreationOrigin,
    /// Diagnostic source anchor, checked for CFG reachability only. Presence and identity are proved
    /// independently from the close inventory; J1/J2 may not use this as an active-slot certificate.
    pub active_pc: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreationOrigin {
    Singleton,
    Create(u32),
    FreshResult { pc: u32, callee: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupSuffix {
    pub completed: u32,
    pub remaining: Vec<Location>,
    pub roots: Vec<Location>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitDestination { Caller, Loop(u32), Scope(u32) }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticSelection {
    pub pc: u32,
    pub selection: u32,
    pub input: Location,
    pub indices: Vec<u32>,
}

#[cfg(test)]
mod load_tests;
#[cfg(test)]
mod arrays_tests;
#[cfg(test)]
mod natives_tests;
#[cfg(test)]
mod scalars_tests;
#[cfg(test)]
mod collections_tests;
#[cfg(test)]
mod managed_tests;
#[cfg(test)]
mod builders_tests;
#[cfg(test)]
mod testmode_tests;
#[cfg(test)]
mod rules_tests;
#[cfg(test)]
mod budget_tests;
#[cfg(test)]
mod async_tests;
#[cfg(test)]
mod roles_tests;
