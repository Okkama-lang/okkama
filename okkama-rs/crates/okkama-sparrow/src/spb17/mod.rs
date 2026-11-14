//! Champ SPB 17 data transformations. Never selected by the default runtime.
//! Wire format and P1 IDs follow Champ G1 revision 2, §§4 and 9.
mod allocation;
mod codec;
pub mod layout;
mod registry;
mod verify;

pub use allocation::{Allocation, Copy, CopySource, allocate, check_allocation, live_locations};
pub use codec::{assemble, decode, disassemble, encode};
pub use registry::{CallAbi, Opcode, ProducedType, ResultContext, Tag};
pub use verify::verify;

pub const VERSION: u16 = 17;
pub use crate::CodecError as Error;

pub(crate) fn error(message: impl Into<String>) -> Error {
    Error { offset: None, message: message.into() }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Program {
    pub module: String,
    pub version: String,
    pub objects: Vec<Object>,
    pub tables: ChampTables,
    pub functions: Vec<Function>,
    pub entry: Option<u32>,
}

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
    pub name: String,
    pub fields: Vec<Field>,
    pub layout: ObjectRuntimeLayout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub storage: u32,
    pub once: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Int,
    Bool,
    Str,
    Range,
    Failure,
    Object(u32),
    Successor(u32),
    Never,
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

/// Operands are typed references, not an untyped helper argument buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Argument {
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
    Convert,
    RangeNew,
    RangeStart,
    RangeEnd,
    ObjNew,
    ObjInit,
    ObjGet,
    ObjNext,
}

/// Shared declarations. Function-local proof references never index these
/// arenas accidentally: their Argument variants select the appropriate arena.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChampTables {
    pub scopes: Vec<Scope>,
    pub boundaries: Vec<Boundary>,
    pub summaries: Vec<Summary>,
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
    pub selections: Vec<Selection>,
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

/// D18 mask: Thrown=1, Program=2, Panic=4, Native=8. No async/cancel bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub failures: u32,
}

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
